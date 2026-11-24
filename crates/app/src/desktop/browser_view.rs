//! The Browser view: a live picture of an agent's browser, for watching only.
//!
//! The view exists while its pane is shown. It holds a [`Stream`] only then,
//! so a hidden or closed view receives no frames. It sends the page nothing:
//! to act in the page, Open in Chrome brings the agent's own window forward,
//! or opens the page's address in the person's Chrome when the agent's
//! browser has no window. The pane is saved as an empty pane, so a Browser
//! view is not restored.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Bounds, Context, Corners, Pixels, SharedString, Task, Window, canvas, div, prelude::*, px,
};

use super::{
    components as ui,
    design::{colors, tokens},
};
use crate::agent_browser::{
    self, Session, Viewport,
    stream::{Frame, State, Stream},
};

pub(super) struct BrowserView {
    pub agent: String,
    /// The agent's session, or none once it has ended.
    session: Option<Session>,
    stream: Option<Stream>,
    /// The frame on screen.
    frame: Option<Frame>,
    /// Frames of an earlier session, to release when the view next paints.
    retired: Vec<Frame>,
    /// Whether the browser has a window to bring forward; none while unknown.
    window: Option<agent_browser::Window>,
    /// Whether the stream has been live since its last connection began.
    live: bool,
    /// The newest look for the window; a newer look replaces and cancels it,
    /// so an earlier answer never overwrites a later one.
    probe: Option<Task<()>>,
    /// Set while a wake from the stream waits to be taken.
    woken: Arc<AtomicBool>,
    wake: mpsc::UnboundedSender<()>,
    _woken: Task<()>,
    /// The size in points the page was last drawn in, and the window's scale.
    area: Rc<Cell<Option<(f32, f32, f32)>>>,
    /// The viewport last set for each page, shared with every view.
    shapes: Shapes,
    /// When to give the agent's browser the pane's size, and the wait for it.
    settle: Settle,
    fitting: Option<Task<()>>,
    /// Whether the page's first frame has asked for the pane's size.
    attached: bool,
    /// Whether a failed resize has been logged.
    fit_failed: bool,
    /// Tells this view from others in `shapes`.
    id: u64,
}

impl BrowserView {
    pub fn new(
        agent: String,
        session: Option<Session>,
        shapes: Shapes,
        cx: &mut Context<Self>,
    ) -> Self {
        let (wake, mut wakes) = mpsc::unbounded();
        let woken = Arc::new(AtomicBool::new(false));
        let taken = woken.clone();
        let task = cx.spawn(async move |this, cx| {
            while wakes.next().await.is_some() {
                taken.store(false, Ordering::Relaxed);
                if this.update(cx, |view, cx| view.woke(cx)).is_err() {
                    break;
                }
            }
        });
        let mut view = Self {
            agent,
            session: None,
            stream: None,
            frame: None,
            retired: Vec::new(),
            window: None,
            live: false,
            probe: None,
            woken,
            wake,
            _woken: task,
            area: Rc::default(),
            shapes,
            settle: Settle::default(),
            fitting: None,
            attached: false,
            fit_failed: false,
            id: {
                static VIEWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                VIEWS.fetch_add(1, Ordering::Relaxed)
            },
        };
        view.set_session(session, cx);
        view
    }

    /// Follow the session as discovery last found it.
    pub fn set_session(&mut self, session: Option<Session>, cx: &mut Context<Self>) {
        if self.session == session {
            return;
        }
        let page = |session: &Option<Session>| session.as_ref().map(|s| (s.pid, s.target.clone()));
        let same_page = page(&self.session) == page(&session);
        if !same_page {
            // A new browser or tab never shows the picture of the one before it.
            self.retired.extend(self.frame.take());
            self.stream = session
                .as_ref()
                .and_then(|session| Some((session.pid, session.target.clone()?)))
                .map(|(daemon, target)| {
                    let (wake, woken) = (self.wake.clone(), self.woken.clone());
                    Stream::open(daemon, target, move || {
                        if !woken.swap(true, Ordering::Relaxed) {
                            let _ = wake.unbounded_send(());
                        }
                    })
                });
            self.window = None;
            self.live = false;
            self.attached = false;
            if let Some(pid) = session.as_ref().map(|session| session.pid) {
                self.check_window(pid, false, None, cx);
            }
        } else if
        // Only a missing window can change for the same daemon.
        matches!(self.window, None | Some(agent_browser::Window::NotStarted))
            && let Some(pid) = session.as_ref().map(|session| session.pid)
        {
            // The daemon may have started its Chrome since the last look.
            self.check_window(pid, false, None, cx);
        }
        self.session = session;
        cx.notify();
    }

    /// Something new came from the stream. Its first frames after a
    /// connection mean the daemon's Chrome is running, so look for its
    /// window again if none was found.
    fn woke(&mut self, cx: &mut Context<Self>) {
        let live = self.stream.as_ref().map(Stream::state) == Some(State::Live);
        if live
            && !self.live
            // Only a missing window can change for the same daemon.
            && matches!(self.window, None | Some(agent_browser::Window::NotStarted))
            && let Some(pid) = self.session.as_ref().map(|session| session.pid)
        {
            self.check_window(pid, false, None, cx);
        }
        self.live = live;
        cx.notify();
    }

    /// The page's title as the stream last reported it.
    pub fn title(&self) -> Option<String> {
        self.stream.as_ref()?.page().1
    }

    /// The page's current address: as Chrome lists it, else as agent-browser
    /// last recorded it.
    fn address(&self) -> Option<String> {
        let (listed, _) = self.stream.as_ref().map(Stream::page).unwrap_or_default();
        listed.or_else(|| self.session.as_ref()?.address.clone())
    }

    /// Give the agent's browser this pane's size once the pane holds still:
    /// the view shows a page for the first time, was resized, or, with
    /// `chosen`, was chosen, which makes it the view that sizes the browser.
    pub fn claim(&mut self, chosen: bool, cx: &mut Context<Self>) {
        if chosen && let Some(session) = &self.session {
            self.shapes.choose(session.pid, self.id);
        }
        self.settle.moved(Instant::now());
        self.fitting = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SETTLE).await;
            let _ = this.update(cx, |view, cx| view.fit(cx));
        }));
    }

    /// Set the agent's viewport to the pane's size in points, at the window's
    /// scale, unless the page has it already.
    fn fit(&mut self, cx: &mut Context<Self>) {
        if !self.settle.due(Instant::now()) || !sizable(self.window) {
            return;
        }
        let (Some((across, down, scale)), Some(session)) = (self.area.get(), &self.session) else {
            return;
        };
        let Some(target) = session.target.clone() else {
            return;
        };
        if !self.shapes.may(session.pid, self.id) {
            return;
        }
        let (page, viewport) = ((session.pid, target), viewport((across, down), scale));
        let shown = self.frame.as_ref().map(|frame| (frame.width, frame.height));
        if !self.shapes.needed(&page, viewport, shown) || !self.shapes.start(&page, viewport) {
            return;
        }
        let (session, shapes) = (session.clone(), self.shapes.clone());
        cx.spawn(async move |this, cx| {
            let mut viewport = viewport;
            loop {
                let (asked, size) = (session.clone(), viewport);
                let set = cx
                    .background_executor()
                    .spawn(async move { agent_browser::set_viewport(&asked, size) })
                    .await;
                match &set {
                    // The session ended since discovery last looked.
                    Err(error) if error.kind() == std::io::ErrorKind::NotConnected => {
                        tracing::debug!(event = "browser_viewport_ended", %error, "Agent browser ended before its resize");
                    }
                    Err(error) => {
                        let _ = this.update(cx, |view, _| {
                            if !std::mem::replace(&mut view.fit_failed, true) {
                                tracing::warn!(event = "browser_viewport_failed", %error, "Agent browser not resized");
                            }
                        });
                    }
                    Ok(()) => {}
                }
                match shapes.ended(&page, viewport, set.is_ok()) {
                    Some(next) => viewport = next,
                    None => break,
                }
            }
        })
        .detach();
    }

    /// The frames holding a texture, to release when the view goes.
    pub fn take_shown(&mut self) -> Vec<Frame> {
        self.retired.drain(..).chain(self.frame.take()).collect()
    }

    /// Learn whether the daemon `pid`'s browser has a window, and bring it
    /// forward when `open` asks for that, or open `address`, the page's
    /// address when the person clicked, when it has none.
    fn check_window(
        &mut self,
        pid: u32,
        open: bool,
        address: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let found = cx
            .background_executor()
            .spawn(async move { agent_browser::window(pid) });
        self.probe = Some(cx.spawn(async move |this, cx| {
            let window = found.await;
            let _ = this.update(cx, |view, cx| {
                if view.session.as_ref().map(|session| session.pid) != Some(pid) {
                    return;
                }
                #[cfg(target_os = "macos")]
                if open {
                    match window {
                        agent_browser::Window::Shown(chrome) => {
                            agent_browser::bring_forward(chrome);
                        }
                        // Without a window, the person's own Chrome shows the
                        // page, unless the agent's Chrome may be using its profile.
                        agent_browser::Window::Headless { own_profile: true } => {
                            if let Some(address) = address {
                                cx.background_executor()
                                    .spawn(async move { agent_browser::open_in_chrome(&address) })
                                    .detach();
                            }
                        }
                        agent_browser::Window::Headless { own_profile: false }
                        | agent_browser::Window::NotStarted => {}
                    }
                }
                #[cfg(not(target_os = "macos"))]
                let _ = (open, address);
                // A browser the daemon started may now be given the pane's
                // size, which its page's first frame may have asked for.
                if !sizable(view.window) && sizable(Some(window)) {
                    view.claim(false, cx);
                }
                view.window = Some(window);
                cx.notify();
            });
        }));
    }

    /// Why Open in Chrome cannot be used now, if it cannot. `address` is the
    /// page's current address.
    fn open_refused(&self, address: Option<&str>) -> Option<&'static str> {
        refusal(self.session.is_none(), self.window, address)
    }
}

/// Why Open in Chrome cannot be used, if it cannot: `ended` once the browser
/// has ended, `window` once known, and `address` the page's current address.
fn refusal(
    ended: bool,
    window: Option<agent_browser::Window>,
    address: Option<&str>,
) -> Option<&'static str> {
    use agent_browser::Window;
    if cfg!(not(target_os = "macos")) {
        return Some("Not available on Linux yet");
    }
    match window {
        _ if ended => Some("The browser ended"),
        None => Some("Looking for its window…"),
        // Opening it could reach the agent's Chrome instead of the person's.
        Some(Window::Headless { own_profile: false }) => {
            Some("This browser has no profile of its own")
        }
        Some(Window::Headless { .. }) if address.and_then(agent_browser::web_address).is_none() => {
            Some("This page has no web address to open")
        }
        Some(Window::Headless { .. } | Window::Shown(_)) => None,
        Some(Window::NotStarted) => {
            Some("agent-browser did not open this browser, so its window can't be found")
        }
    }
}

impl Drop for BrowserView {
    fn drop(&mut self) {
        self.shapes.release(self.id);
    }
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        for old in self.retired.drain(..) {
            let _ = window.drop_image(old.image);
        }
        if let Some(frame) = self.stream.as_ref().and_then(Stream::take_frame)
            && let Some(old) = self.frame.replace(frame)
        {
            let _ = window.drop_image(old.image);
        }
        if !self.attached && self.frame.is_some() {
            self.attached = true;
            self.claim(false, cx);
        }
        let address = self.address();
        let site = site_name(self.title().as_deref(), address.as_deref());
        let refused = self.open_refused(address.as_deref());
        let pid = self.session.as_ref().map(|session| session.pid);
        let open = ui::button(
            SharedString::from(format!("browser-open-{}", self.agent)),
            "Open in Chrome",
            ui::Button::Secondary,
        )
        .child(ui::icon("arrow-up-right", t.size.icon - 2.0, c.muted))
        .when_some(refused, |button, why| {
            button
                .opacity(0.5)
                .cursor_default()
                .tooltip(move |_, cx| cx.new(|_| Refused(why)).into())
        })
        .when_some(pid.filter(|_| refused.is_none()), |button, pid| {
            button.on_click(cx.listener(move |view, _, _, cx| {
                // Look again: the browser may have been started anew.
                let address = view.address();
                view.check_window(pid, true, address, cx);
            }))
        });
        let named = |name: String| {
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name)
        };
        // The names give way before Open in Chrome, and a long title before
        // the agent's name.
        let names = div()
            .flex()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .items_center()
            .gap(px(t.space.s8))
            .child("·")
            .child(named(self.agent.clone()).flex_shrink_0())
            .when_some(site, |names, site| names.child("·").child(named(site)));
        let header = ui::view_header("globe", "Browser").child(names).child(open);
        let quiet = |text: &'static str| {
            ui::text(
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(c.muted),
                &t.text.body,
            )
            .child(text)
        };
        let state = self.stream.as_ref().map(Stream::state);
        let body = match (&self.session, &self.frame, state) {
            (None, _, _) => quiet("The browser ended.").into_any_element(),
            (_, _, None) => quiet("This browser has no page yet.").into_any_element(),
            (_, _, Some(State::Unreachable)) => {
                quiet("Can't reach the browser's page. Trying again…").into_any_element()
            }
            // The whole page, as large as the pane holds it, drawn from the
            // full-resolution frame.
            (_, Some(frame), _) => {
                let (image, shape) = (frame.image.clone(), (frame.width, frame.height));
                let (drawn, view) = (self.area.clone(), cx.entity().downgrade());
                canvas(
                    |_, _, _| {},
                    move |area, (), window, cx| {
                        let size = (
                            f32::from(area.size.width),
                            f32::from(area.size.height),
                            window.scale_factor(),
                        );
                        if drawn.replace(Some(size)) != Some(size) {
                            cx.defer(move |cx| {
                                let _ = view.update(cx, |view, cx| view.claim(false, cx));
                            });
                        }
                        let shown = fit(shape, area);
                        let _ =
                            window.paint_image(shown, shown, Corners::default(), image, 0, false);
                    },
                )
                .size_full()
                .into_any_element()
            }
            (_, None, Some(State::Connecting)) => {
                quiet("Connecting to the browser…").into_any_element()
            }
            (_, None, _) => quiet("Waiting for the page…").into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(c.surface)
            .child(header)
            .child(div().flex_1().min_h_0().child(body))
    }
}

/// How long a pane's size holds before the agent's browser takes it.
const SETTLE: Duration = Duration::from_millis(300);

/// The viewport that shows a page at one CSS pixel per point in `area`, the
/// width and height in points it is drawn in, at the window's `scale`.
fn viewport(area: (f32, f32), scale: f32) -> Viewport {
    let side = |points: f32, least: f32| points.round().clamp(least, 4096.) as u32;
    Viewport {
        width: side(area.0, 320.),
        height: side(area.1, 240.),
        scale,
    }
}

/// Whether the agent's browser may be given the pane's size: only one its
/// daemon started. One agent-browser attached to may be the person's own.
fn sizable(window: Option<agent_browser::Window>) -> bool {
    matches!(
        window,
        Some(agent_browser::Window::Headless { .. } | agent_browser::Window::Shown(_))
    )
}

/// Which view sizes each agent's browser, and the viewport set for each of
/// its pages, shared by every view.
#[derive(Clone, Default)]
pub(super) struct Shapes(Rc<RefCell<Record>>);

#[derive(Default)]
struct Record {
    /// The view that sizes each browser, by its daemon: the view chosen
    /// last, as tmux sizes a window for its latest client.
    owners: HashMap<u32, u64>,
    /// Each page, by its browser's daemon and its tab.
    pages: HashMap<(u32, String), Page>,
}

#[derive(Default)]
struct Page {
    /// The viewport last set.
    set: Option<Viewport>,
    /// Whether a command is running, and the viewport to send after it.
    sending: bool,
    next: Option<Viewport>,
}

impl Shapes {
    /// `view` was chosen: it sizes the browser of `daemon` from now on.
    fn choose(&self, daemon: u32, view: u64) {
        self.0.borrow_mut().owners.insert(daemon, view);
    }

    /// `view` has gone: the browsers it sized go to the next view to ask.
    fn release(&self, view: u64) {
        self.0.borrow_mut().owners.retain(|_, owner| *owner != view);
    }

    /// Whether `view` sizes the browser of `daemon`: the view chosen last,
    /// or the first to ask while none has been chosen.
    fn may(&self, daemon: u32, view: u64) -> bool {
        *self.0.borrow_mut().owners.entry(daemon).or_insert(view) == view
    }

    /// Whether `page`, showing a page of `shown` CSS pixels, needs
    /// `viewport`: a command is running, the last one set differs, or the
    /// page shows another size.
    fn needed(&self, page: &(u32, String), viewport: Viewport, shown: Option<(f32, f32)>) -> bool {
        let record = self.0.borrow();
        let page = record.pages.get(page);
        // While a command runs, the newest size waits behind it, whatever it
        // is: the size before it may be wanted back.
        if page.is_some_and(|page| page.sending) {
            return true;
        }
        let set = page.and_then(|page| page.set);
        let size = shown.map(|(width, height)| (width.round() as u32, height.round() as u32));
        set != Some(viewport) || size != Some((viewport.width, viewport.height))
    }

    /// Whether to send `viewport` for `page` now; while a command runs, it is
    /// sent after that one instead.
    fn start(&self, page: &(u32, String), viewport: Viewport) -> bool {
        let mut record = self.0.borrow_mut();
        let page = record.pages.entry(page.clone()).or_default();
        if page.sending {
            page.next = Some(viewport);
            return false;
        }
        page.sending = true;
        true
    }

    /// The command that sent `viewport` for `page` ended, having `set` it or
    /// not. Returns the viewport to send next, if another is waiting.
    fn ended(&self, page: &(u32, String), viewport: Viewport, set: bool) -> Option<Viewport> {
        let mut record = self.0.borrow_mut();
        let page = record.pages.entry(page.clone()).or_default();
        if set {
            page.set = Some(viewport);
        }
        page.sending = false;
        let next = page.next.take().filter(|next| page.set != Some(*next))?;
        page.sending = true;
        Some(next)
    }
}

/// When a view's pane has held still long enough to give the agent's browser
/// its size. The view exists only while its pane is shown, so a hidden pane
/// asks for nothing.
#[derive(Default)]
struct Settle {
    /// When the pane last moved, until its size is due.
    since: Option<Instant>,
}

impl Settle {
    fn moved(&mut self, now: Instant) {
        self.since = Some(now);
    }

    /// Whether the size is due now: once per stillness.
    fn due(&mut self, now: Instant) -> bool {
        let due = self
            .since
            .is_some_and(|at| now.duration_since(at) >= SETTLE);
        if due {
            self.since = None;
        }
        due
    }
}

/// Why Open in Chrome cannot be used, shown over the button.
struct Refused(&'static str);

impl Render for Refused {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        ui::menu().child(self.0)
    }
}

/// Where a frame of `shape`, its width and height, is drawn in `area`: whole,
/// in its own proportions and as large as fits, from the area's top left
/// corner, so any band left over falls to the right or below it.
fn fit(shape: (f32, f32), area: Bounds<Pixels>) -> Bounds<Pixels> {
    let (width, height) = shape;
    let scale = (f32::from(area.size.width) / width).min(f32::from(area.size.height) / height);
    Bounds::new(
        area.origin,
        gpui::size(px(width * scale), px(height * scale)),
    )
}

/// What the header names the page by: its title, else its address's host.
fn site_name(title: Option<&str>, address: Option<&str>) -> Option<String> {
    let title = title.map(str::trim).filter(|title| !title.is_empty());
    title.or_else(|| host(address?)).map(str::to_owned)
}

/// The host of `address`, without its user, port, path, query or fragment.
fn host(address: &str) -> Option<&str> {
    let (_, rest) = address.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match authority.find(']') {
        // An IPv6 address keeps its brackets.
        Some(end) if authority.starts_with('[') => &authority[..=end],
        _ => authority.split(':').next()?,
    };
    (!host.is_empty()).then_some(host)
}

/// How often discovery looks for sessions.
const DISCOVERY: std::time::Duration = std::time::Duration::from_secs(2);

/// Whether `leaf` is an empty pane, the only kind a Browser view takes.
fn holds_nothing(leaf: &agent_interface_core::Leaf) -> bool {
    leaf.empty && leaf.attachment.is_none() && leaf.view.is_none()
}

impl super::Browser {
    /// Look for agent-browser sessions while the window is open.
    pub(super) fn watch_agent_browsers(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = agent_browser::socket_dir() else {
            return;
        };
        self._agent_browsers = Some(cx.spawn(async move |this, cx| {
            loop {
                let read = dir.clone();
                let found = cx
                    .background_executor()
                    .spawn(async move { agent_browser::sessions(&read) })
                    .await;
                if this
                    .update(cx, |browser, cx| browser.found_agent_browsers(found, cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(DISCOVERY).await;
            }
        }));
    }

    fn found_agent_browsers(&mut self, found: Vec<Session>, cx: &mut Context<Self>) {
        if self.agent_browsers == found {
            return;
        }
        for view in self.browser_views.values() {
            view.update(cx, |view, cx| {
                let session = found.iter().find(|session| session.name == view.agent);
                view.set_session(session.cloned(), cx);
            });
        }
        self.agent_browsers = found;
        cx.notify();
    }

    /// The running session of `agent`, a session with exactly its name.
    pub(super) fn agent_browser(&self, agent: &str) -> Option<&Session> {
        self.agent_browsers
            .iter()
            .find(|session| session.name == agent)
    }

    /// What a row shows for `session`: its page title when a view knows it,
    /// else its address.
    pub(super) fn agent_browser_label(&self, session: &Session, cx: &gpui::App) -> String {
        self.browser_views
            .values()
            .map(|view| view.read(cx))
            .find(|view| view.agent == session.name)
            .and_then(|view| view.title())
            .or_else(|| session.address.clone())
            .unwrap_or_else(|| "Browser".into())
    }

    /// Show `agent`'s browser in the empty pane `pane`, as its chooser asked.
    pub(super) fn show_agent_browser_in(
        &mut self,
        pane: &str,
        agent: &str,
        cx: &mut Context<Self>,
    ) {
        let empty = self.layout.as_ref().is_some_and(|layout| {
            layout
                .model()
                .workspaces
                .iter()
                .flat_map(|space| &space.tabs)
                .flat_map(|tab| tab.tree.leaves())
                .any(|leaf| leaf.id == pane && holds_nothing(leaf))
        });
        if empty && self.agent_browser(agent).is_some() {
            self.browser_panes.insert(pane.to_owned(), agent.to_owned());
            self.reset_focus = true;
            cx.notify();
        }
    }

    /// Keep a view for each shown Browser pane and none for any other: a
    /// hidden view closes its stream.
    pub(super) fn sync_browser_views(
        &mut self,
        drawn: &[agent_interface_core::Leaf],
        expanded: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(model) = self.layout.as_ref().map(|layout| layout.model()) {
            // A pane opened by a save that has not landed is not in the model yet.
            let settled = !self.saving && self.pending_save.is_none();
            let leaves: Vec<_> = model
                .workspaces
                .iter()
                .flat_map(|space| &space.tabs)
                .flat_map(|tab| tab.tree.leaves())
                .collect();
            self.browser_panes.retain(|id, _| {
                leaves
                    .iter()
                    .find(|leaf| &leaf.id == id)
                    .map_or(!settled, |leaf| holds_nothing(leaf))
            });
        }
        let shown: Vec<(String, String)> = drawn
            .iter()
            .filter(|leaf| holds_nothing(leaf) && expanded.is_none_or(|id| id == leaf.id))
            .filter_map(|leaf| Some((leaf.id.clone(), self.browser_panes.get(&leaf.id)?.clone())))
            .collect();
        let gone: Vec<String> = self
            .browser_views
            .iter()
            .filter(|(id, view)| {
                !shown
                    .iter()
                    .any(|(shown, agent)| shown == *id && &view.read(cx).agent == agent)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in gone {
            if let Some(view) = self.browser_views.remove(&id) {
                for frame in view.update(cx, |view, _| view.take_shown()) {
                    let _ = window.drop_image(frame.image);
                }
            }
        }
        for (id, agent) in shown {
            if !self.browser_views.contains_key(&id) {
                let session = self.agent_browser(&agent).cloned();
                let shapes = self.browser_shapes.clone();
                let view = cx.new(|cx| BrowserView::new(agent, session, shapes, cx));
                self.browser_views.insert(id, view);
            }
        }
        // The view chosen last gives its browser its size, as tmux sizes a
        // window for its latest client.
        let chosen = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().pane().map(|leaf| leaf.id.clone()))
            .filter(|id| self.browser_views.contains_key(id));
        if chosen != self.browser_chosen {
            if let Some(view) = chosen.as_ref().and_then(|id| self.browser_views.get(id)) {
                view.update(cx, |view, cx| view.claim(true, cx));
            }
            self.browser_chosen = chosen;
        }
    }

    /// Show `agent`'s browser: in its open Browser pane, else in a visible
    /// empty pane, else in a new tab.
    pub(super) fn open_agent_browser(
        &mut self,
        agent: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.saving || self.save_failed {
            return;
        }
        if let Some(update) = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update)
        {
            self.apply(update);
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let model = layout.model();
        let Some(space) = model.workspace() else {
            return;
        };
        let panes = &self.browser_panes;
        let existing = space.tabs.iter().find_map(|tab| {
            tab.tree
                .leaves()
                .into_iter()
                // A pane another writer filled is no longer this Browser pane.
                .find(|leaf| {
                    holds_nothing(leaf) && panes.get(&leaf.id).is_some_and(|shown| shown == agent)
                })
                .map(|leaf| (tab.id.clone(), leaf.id.clone()))
        });
        let free = |leaf: &&agent_interface_core::Leaf| {
            holds_nothing(leaf) && !panes.contains_key(&leaf.id)
        };
        let empty = model.tab().and_then(|tab| {
            model
                .pane()
                .filter(free)
                .or_else(|| {
                    (!model.focus_mode())
                        .then(|| tab.tree.leaves().into_iter().find(free))
                        .flatten()
                })
                .map(|leaf| leaf.id.clone())
        });
        self.close_menus();
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        let layout = self.layout.as_mut().expect("checked above");
        if let Some((tab, leaf)) = existing {
            let model = layout.model_mut();
            model.choose_tab(&tab);
            model.choose_pane(&leaf);
        } else if let Some(leaf) = empty {
            layout.model_mut().choose_pane(&leaf);
            self.browser_panes.insert(leaf, agent.to_owned());
        } else {
            let (base, mut proposed) = layout.save_snapshot();
            if !proposed.add_tab(Some("Browser".into()), "/".into(), true) {
                return;
            }
            let leaf = proposed.pane().expect("a new tab has a pane").id.clone();
            self.browser_panes.insert(leaf, agent.to_owned());
            self.creation_from = Some(super::Selection::capture(layout.model()));
            self.pending_save = Some((base, proposed));
            self.save(cx);
        }
        self.reset_focus = true;
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    #[test]
    fn the_whole_frame_fills_the_pane_from_its_top_left_corner() {
        let area = Bounds::new(point(px(10.), px(20.)), size(px(640.), px(600.)));
        // Wider than the pane: the full width, the band below.
        let wide = fit((1280., 640.), area);
        assert_eq!(wide.origin, area.origin);
        assert_eq!(wide.size, size(px(640.), px(320.)));
        // Taller than the pane: the full height, the band to the right.
        let tall = fit((400., 800.), area);
        assert_eq!(tall.origin, area.origin);
        assert_eq!(tall.size, size(px(300.), px(600.)));
        // Smaller than the pane: made larger to fill it.
        assert_eq!(fit((64., 60.), area).size, area.size);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_headless_page_opens_only_with_a_web_address_and_a_profile_of_its_own() {
        use agent_browser::Window;
        let own = Some(Window::Headless { own_profile: true });
        assert_eq!(refusal(false, own, Some("https://example.com/")), None);
        for address in [Some("chrome://newtab/"), Some("about:blank"), None] {
            assert_eq!(
                refusal(false, own, address),
                Some("This page has no web address to open")
            );
        }
        assert_eq!(
            refusal(
                false,
                Some(Window::Headless { own_profile: false }),
                Some("https://example.com/")
            ),
            Some("This browser has no profile of its own")
        );
    }

    #[test]
    fn the_viewport_is_the_pane_at_true_size_within_limits() {
        let size = |area| {
            let viewport = viewport(area, 2.);
            (viewport.width, viewport.height)
        };
        assert_eq!(size((900.4, 1300.6)), (900, 1301));
        assert_eq!(size((100., 100.)), (320, 240));
        assert_eq!(size((5000., 5000.)), (4096, 4096));
        assert_eq!(viewport((900., 1300.), 2.).scale, 2.);
    }

    fn page() -> (u32, String) {
        (10, "PAGE".into())
    }

    fn at(width: u32, scale: f32) -> Viewport {
        Viewport {
            width,
            height: 600,
            scale,
        }
    }

    fn shown(width: u32) -> Option<(f32, f32)> {
        Some((width as f32, 600.))
    }

    /// Send `viewport` for the page as a view does, with the command ending
    /// at once, and say whether it was sent.
    fn send(shapes: &Shapes, viewport: Viewport, showing: Option<(f32, f32)>) -> bool {
        let sent = shapes.needed(&page(), viewport, showing) && shapes.start(&page(), viewport);
        if sent {
            assert_eq!(shapes.ended(&page(), viewport, true), None);
        }
        sent
    }

    #[test]
    fn a_viewport_is_set_only_when_it_changes() {
        let shapes = Shapes::default();
        assert!(send(&shapes, at(800, 2.), None));
        assert!(!send(&shapes, at(800, 2.), shown(800)));
        assert!(send(&shapes, at(801, 2.), shown(800)));
        assert!(send(&shapes, at(801, 1.), shown(801)));
        // Another tab of the same browser has its own.
        let tab = (10, "TAB".to_owned());
        assert!(shapes.needed(&tab, at(801, 1.), shown(801)));
        // The agent set another size itself: the next trigger sets the pane's.
        assert!(send(&shapes, at(801, 1.), shown(1280)));
    }

    #[test]
    fn the_frame_our_own_size_brings_is_not_the_agents() {
        let shapes = Shapes::default();
        // Sent while the page still shows the agent's 1280 by 600.
        assert!(send(&shapes, at(900, 2.), shown(1280)));
        // The page then shows the size sent: the next trigger sends nothing.
        assert!(!send(&shapes, at(900, 2.), shown(900)));
    }

    #[test]
    fn a_viewport_is_recorded_once_set_and_sent_one_at_a_time() {
        let shapes = Shapes::default();
        assert!(shapes.start(&page(), at(800, 2.)));
        // A newer size waits for the command running, and the last one wins.
        assert!(!shapes.start(&page(), at(700, 2.)));
        assert!(!shapes.start(&page(), at(600, 2.)));
        // The first failed: nothing is recorded, and the newest follows.
        assert_eq!(shapes.ended(&page(), at(800, 2.), false), Some(at(600, 2.)));
        assert!(shapes.needed(&page(), at(800, 2.), shown(800)));
        assert_eq!(shapes.ended(&page(), at(600, 2.), true), None);
        assert!(!shapes.needed(&page(), at(600, 2.), shown(600)));
        // A size already set is not sent again after the command running.
        assert!(shapes.start(&page(), at(500, 2.)));
        assert!(!shapes.start(&page(), at(600, 2.)));
        assert_eq!(shapes.ended(&page(), at(500, 2.), false), None);
    }

    #[test]
    fn a_size_wanted_back_while_another_is_sent_follows_it() {
        let shapes = Shapes::default();
        assert!(send(&shapes, at(800, 2.), None));
        // Resized to 700, then back to 800 while 700 is still being sent.
        let ask = |viewport| {
            shapes.needed(&page(), viewport, shown(800)) && shapes.start(&page(), viewport)
        };
        assert!(ask(at(700, 2.)));
        assert!(!ask(at(800, 2.)));
        assert_eq!(shapes.ended(&page(), at(700, 2.), true), Some(at(800, 2.)));
        assert_eq!(shapes.ended(&page(), at(800, 2.), true), None);
    }

    #[test]
    fn the_size_follows_a_pane_that_has_held_still() {
        let start = Instant::now();
        let mut settle = Settle::default();
        assert!(!settle.due(start + SETTLE));
        settle.moved(start);
        settle.moved(start + SETTLE / 2);
        // Still moving 300 ms after the first move: not yet.
        assert!(!settle.due(start + SETTLE));
        assert!(settle.due(start + SETTLE / 2 + SETTLE));
        assert!(!settle.due(start + 2 * SETTLE));
    }

    #[test]
    fn the_view_chosen_last_sizes_the_browser() {
        let (shapes, first, second) = (Shapes::default(), 1, 2);
        // Before any choice, the first view to ask sizes it.
        assert!(shapes.may(10, second));
        shapes.choose(10, first);
        // A window resize moves both views: only the chosen one counts.
        assert!(shapes.may(10, first));
        assert!(!shapes.may(10, second));
        shapes.choose(10, second);
        assert!(!shapes.may(10, first));
        assert!(shapes.may(10, second));
        // Another browser has its own.
        assert!(shapes.may(20, first));
        // The chosen view goes: the next view to ask sizes its browsers.
        shapes.release(second);
        assert!(shapes.may(10, first));
    }

    #[test]
    fn only_a_browser_the_daemon_started_is_resized() {
        use agent_browser::Window;
        assert!(sizable(Some(Window::Headless { own_profile: true })));
        assert!(sizable(Some(Window::Shown(11))));
        assert!(!sizable(Some(Window::NotStarted)));
        assert!(!sizable(None));
    }

    #[test]
    fn the_page_is_named_by_its_title_else_its_host() {
        let address = Some("https://user@example.com:8443/issues?q=a#top");
        assert_eq!(
            site_name(Some("Issues"), address).as_deref(),
            Some("Issues")
        );
        assert_eq!(
            site_name(Some("  "), address).as_deref(),
            Some("example.com")
        );
        assert_eq!(site_name(None, address).as_deref(), Some("example.com"));
        assert_eq!(
            site_name(None, Some("http://[::1]:8080/")).as_deref(),
            Some("[::1]")
        );
        assert_eq!(site_name(None, Some("about:blank")), None);
        assert_eq!(site_name(None, None), None);
    }
}
