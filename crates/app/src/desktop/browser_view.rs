//! The Browser view: a live picture of an agent's browser, for watching only.
//!
//! The view exists while its pane is shown. It holds a [`Stream`] only then,
//! so a hidden or closed view receives no frames. It sends the page nothing:
//! to act in the page, Open in Chrome brings the agent's own window forward,
//! or opens the page's address in the person's Chrome when the agent's
//! browser has no window. The pane is saved as an empty pane, so a Browser
//! view is not restored.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
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
    self, Session,
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
}

impl BrowserView {
    pub fn new(agent: String, session: Option<Session>, cx: &mut Context<Self>) -> Self {
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
            if cfg!(target_os = "macos")
                && let Some(pid) = session.as_ref().map(|session| session.pid)
            {
                self.check_window(pid, false, cx);
            }
        } else if cfg!(target_os = "macos")
            // Only a missing window can change for the same daemon.
            && matches!(self.window, None | Some(agent_browser::Window::NotStarted))
            && let Some(pid) = session.as_ref().map(|session| session.pid)
        {
            // The daemon may have started its Chrome since the last look.
            self.check_window(pid, false, cx);
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
            && cfg!(target_os = "macos")
            // Only a missing window can change for the same daemon.
            && matches!(self.window, None | Some(agent_browser::Window::NotStarted))
            && let Some(pid) = self.session.as_ref().map(|session| session.pid)
        {
            self.check_window(pid, false, cx);
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

    /// The frames holding a texture, to release when the view goes.
    pub fn take_shown(&mut self) -> Vec<Frame> {
        self.retired.drain(..).chain(self.frame.take()).collect()
    }

    /// Learn whether the daemon `pid`'s browser has a window, and bring it
    /// forward when `open` asks for that.
    fn check_window(&mut self, pid: u32, open: bool, cx: &mut Context<Self>) {
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
                        // Without a window, the person's own Chrome shows the page.
                        agent_browser::Window::Headless => {
                            if let Some(address) = view.address() {
                                cx.background_executor()
                                    .spawn(async move { agent_browser::open_in_chrome(&address) })
                                    .detach();
                            }
                        }
                        agent_browser::Window::NotStarted => {}
                    }
                }
                #[cfg(not(target_os = "macos"))]
                let _ = open;
                view.window = Some(window);
                cx.notify();
            });
        }));
    }

    /// Why Open in Chrome cannot be used now, if it cannot. `address` is the
    /// page's current address.
    fn open_refused(&self, address: Option<&str>) -> Option<&'static str> {
        if cfg!(not(target_os = "macos")) {
            return Some("Not available on Linux yet");
        }
        match (&self.session, self.window) {
            (None, _) => Some("The browser ended"),
            (_, None) => Some("Looking for its window…"),
            (_, Some(agent_browser::Window::Headless))
                if address.and_then(agent_browser::web_address).is_none() =>
            {
                Some("This page has no web address to open")
            }
            (_, Some(agent_browser::Window::Headless)) => None,
            (_, Some(agent_browser::Window::NotStarted)) => {
                Some("agent-browser did not open this browser, so its window can't be found")
            }
            (_, Some(agent_browser::Window::Shown(_))) => None,
        }
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
                view.check_window(pid, true, cx);
            }))
        });
        let named = |name: String| {
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .child(name)
        };
        let header = ui::view_header("globe", "Browser")
            .child("·")
            .child(named(self.agent.clone()))
            .when_some(site, |header, site| header.child("·").child(named(site)))
            .child(div().flex_1())
            .child(open);
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
                canvas(
                    |_, _, _| {},
                    move |area, (), window, _| {
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
                let view = cx.new(|cx| BrowserView::new(agent, session, cx));
                self.browser_views.insert(id, view);
            }
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
