//! The Browser view: a live picture of an agent's browser that the person can
//! use as a browser.
//!
//! The view exists while its pane is shown. It holds a [`Stream`] only then,
//! so a hidden or closed view receives no frames and sends no input. Clicks,
//! drags and the wheel in the frame reach the page at the same spot; once a
//! click in the frame gives the view the keyboard, keys and text reach it
//! too. Open in Chrome brings the agent's own window forward, or opens the
//! page's address in the person's Chrome when the agent's browser has no
//! window. The pane is saved as an empty pane, so a Browser view is not
//! restored.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    ops::Range,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Bounds, Context, Corners, DispatchPhase, ElementInputHandler, EntityInputHandler, FocusHandle,
    HitboxBehavior, KeyDownEvent, Keystroke, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, SharedString, Task,
    UTF16Selection, Window, canvas, div, prelude::*, px,
};

use super::{
    components as ui,
    design::{colors, tokens},
};
use crate::agent_browser::{
    self, Session, Viewport,
    stream::{ALT, Button, CTRL, Frame, Input, Key, KeyKind, META, Pointer, SHIFT, State, Stream},
};

pub(super) struct BrowserView {
    pub agent: String,
    /// The agent's session, or none once it has ended.
    session: Option<Session>,
    /// The name of the browser Agent Backbone runs that this view watches,
    /// through the DevTools endpoint Backbone gives the owner; none for an
    /// agent's session.
    watched: Option<String>,
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
    /// Holds the keyboard while the person uses the page.
    focus: FocusHandle,
    /// The mouse as the page last heard of it.
    pointing: Pointing,
    /// The printable keystroke just pressed, whose text the input handler
    /// brings next.
    typed: Option<(Keystroke, bool)>,
    /// The text an input method is composing.
    marked: Option<String>,
    /// Where the person last pressed, near which the input method shows its
    /// window.
    caret: Option<Point<Pixels>>,
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
            watched: None,
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
            focus: cx.focus_handle(),
            pointing: Pointing::default(),
            typed: None,
            marked: None,
            caret: None,
        };
        view.set_session(session, cx);
        view
    }

    /// A view, under `key`, of the browser `name` Agent Backbone runs,
    /// through the DevTools endpoint at `port` it gives the owner: its page
    /// `target` when the holder's session names one, else its first page.
    /// Backbone sizes and shows its window, so this view does neither.
    pub fn watching(
        key: String,
        name: String,
        port: u16,
        target: Option<String>,
        shapes: Shapes,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self::new(key, None, shapes, cx);
        view.watched = Some(name);
        let (wake, woken) = (view.wake.clone(), view.woken.clone());
        view.stream = Some(Stream::open_port(port, target, move || {
            if !woken.swap(true, Ordering::Relaxed) {
                let _ = wake.unbounded_send(());
            }
        }));
        view
    }

    /// Follow the session as discovery last found it.
    pub fn set_session(&mut self, session: Option<Session>, cx: &mut Context<Self>) {
        if self.session == session || self.watched.is_some() {
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
            // Nothing the person did in the old page belongs to the new one.
            self.pointing = Pointing::default();
            (self.typed, self.marked, self.caret) = (None, None, None);
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

    /// This view is the one chosen: it sizes its browser, also one started
    /// anew under another daemon.
    pub fn chosen(&mut self) {
        if let Some(session) = &self.session {
            self.shapes.choose(session.pid, self.id);
        }
    }

    /// Whether the view draws the page, as its frame, now.
    fn showing(&self) -> bool {
        (self.session.is_some() || self.watched.is_some())
            && self.frame.is_some()
            && self
                .stream
                .as_ref()
                .is_some_and(|stream| stream.state() != State::Unreachable)
    }

    /// Send `input` to the page shown, if one is.
    fn send(&self, input: Input) {
        if self.showing()
            && let Some(stream) = &self.stream
        {
            stream.send(input);
        }
    }

    /// Whether the view has the keyboard.
    pub fn has_focus(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// Whether an input method is composing text for the page.
    pub fn composing(&self) -> bool {
        self.marked.is_some()
    }

    /// A press at `at`, the page point under it, or beside the frame.
    fn pressed(&mut self, event: &MouseDownEvent, at: Option<(f32, f32)>) {
        let (Some(at), Some(button)) = (at, pressable(event.button, &event.modifiers)) else {
            return;
        };
        if !self.showing() {
            return;
        }
        self.caret = Some(event.position);
        let pressed = self.pointing.press(at, button, event.click_count);
        self.send(pressed);
    }

    /// A release, wherever it is, of a button pressed in the frame.
    fn released(&mut self, event: &MouseUpEvent, at: Option<(f32, f32)>) {
        let (Some(at), Some(button)) = (at, button(event.button)) else {
            return;
        };
        // Sent even while the page can't be reached, so the next connection
        // still releases the button in the page.
        if let Some(released) = self.pointing.release(at, button, event.click_count)
            && let Some(stream) = &self.stream
        {
            stream.send(released);
        }
    }

    /// A move, at the page point `at` or beside the frame.
    fn moved(&mut self, event: &MouseMoveEvent, at: Option<(f32, f32)>) {
        let pressed = event.pressed_button.is_some();
        for input in self
            .pointing
            .moved(at, pressed, mouse_modifiers(&event.modifiers))
        {
            self.send(input);
        }
    }

    /// Keys for the page while the view has the keyboard. App shortcuts ran
    /// before this, and a printable key waits for its text.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.typed = None;
        // Without a page shown, keys stay the app's; while an input method
        // composes, they are its own.
        if !window.is_window_active() || !self.showing() || self.marked.is_some() {
            return;
        }
        let keystroke = &event.keystroke;
        // A key the app or its menu acts on is never also the page's: its
        // binding or workspace command ran before this and may have let the
        // key go on, as Copy does.
        let owned = || {
            !cx.key_bindings()
                .borrow()
                .all_bindings_for_input(std::slice::from_ref(keystroke))
                .is_empty()
                || super::shortcuts::command_for(keystroke).is_some()
                || super::shortcuts::numbered(keystroke).is_some()
        };
        match key_press(keystroke, event.is_held, cfg!(target_os = "macos")) {
            Press::Send(_) if owned() => {}
            Press::Send(keys) => {
                for key in keys {
                    self.send(Input::Key(key));
                }
                // The app's own Cmd keys are never sent, so none acts twice.
                if !goes_on(keystroke, cfg!(target_os = "macos")) {
                    cx.stop_propagation();
                }
            }
            Press::Type => self.typed = Some((event.keystroke.clone(), event.is_held)),
            Press::Pass => {}
        }
    }

    /// Select All, from its shortcut or the Edit menu, selects in the page.
    fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shortcut = if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        };
        let keystroke = Keystroke::parse(shortcut).expect("select all");
        match key_press(&keystroke, false, cfg!(target_os = "macos")) {
            Press::Send(keys) if window.is_window_active() && self.showing() => {
                for key in keys {
                    self.send(Input::Key(key));
                }
            }
            _ => cx.propagate(),
        }
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
        let focused = self.focus.is_focused(window);
        let live = window.is_window_active() && self.showing();
        // A key or composition begun before the keyboard or page went is over.
        if !(focused && live) {
            (self.typed, self.marked) = (None, None);
        }
        if let Some(stream) = &self.stream {
            stream.set_focused(focused && live);
            if !live {
                stream.set_hovered(false);
            }
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
            .child(
                named(self.watched.clone().unwrap_or_else(|| self.agent.clone())).flex_shrink_0(),
            )
            .when_some(site, |names, site| names.child("·").child(named(site)));
        let header = ui::view_header("globe", "Browser")
            .child(names)
            // Backbone's own browser is shown to the person from the Access pane.
            .when(self.watched.is_none(), |header| header.child(open));
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
            // Without the page, a press does not give the view the keyboard;
            // anything drawn over this keeps its own.
            .on_any_mouse_down(|_, window, _| window.prevent_default())
            .child(text)
        };
        let state = self.stream.as_ref().map(Stream::state);
        let body = match (&self.session, &self.frame, state) {
            (None, _, _) if self.watched.is_none() => {
                quiet("The browser ended.").into_any_element()
            }
            (_, _, None) => quiet("This browser has no page yet.").into_any_element(),
            (_, _, Some(State::Unreachable)) => {
                quiet("Can't reach the browser's page. Trying again…").into_any_element()
            }
            // The whole page, as large as the pane holds it, drawn from the
            // full-resolution frame.
            (_, Some(frame), _) => {
                let (image, shape) = (frame.image.clone(), (frame.width, frame.height));
                let (drawn, entity) = (self.area.clone(), cx.entity());
                let (view, focus) = (entity.downgrade(), self.focus.clone());
                let ring = focused.then_some(c.focus);
                canvas(
                    move |area, window, _| {
                        let shown = fit(shape, area);
                        let band = window.insert_hitbox(area, HitboxBehavior::Normal);
                        (
                            shown,
                            band,
                            window.insert_hitbox(shown, HitboxBehavior::Normal),
                        )
                    },
                    move |area, (shown, band, hitbox), window, cx| {
                        let size = (
                            f32::from(area.size.width),
                            f32::from(area.size.height),
                            window.scale_factor(),
                        );
                        if drawn.replace(Some(size)) != Some(size) {
                            let view = view.clone();
                            cx.defer(move |cx| {
                                let _ = view.update(cx, |view, cx| view.claim(false, cx));
                            });
                        }
                        let _ =
                            window.paint_image(shown, shown, Corners::default(), image, 0, false);
                        if let Some(ring) = ring {
                            window.paint_quad(gpui::outline(shown, ring, gpui::BorderStyle::Solid));
                        }
                        window.handle_input(
                            &focus,
                            // The page shows no text the accent menu could
                            // edit, so a held key repeats, as in a browser.
                            super::zoom::repeating_input(ElementInputHandler::new(shown, entity)),
                            cx,
                        );
                        listen(window, view, &focus, band, shown, shape, hitbox);
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
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(Self::key_down))
                    .on_action(cx.listener(|view, _: &super::SelectAll, window, cx| {
                        view.select_all(window, cx);
                    }))
                    .child(body),
            )
    }
}

/// The page's own input method: text the person types or composes goes to
/// the page; nothing of the page's text is read back.
impl EntityInputHandler for BrowserView {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let end = self.marked.as_deref().map_or(0, utf16_len);
        Some(UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_deref().map(|marked| 0..utf16_len(marked))
    }

    /// The input method accepts what it composed as it stands.
    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        if let Some(marked) = self.marked.take() {
            self.send(Input::Text(marked));
        }
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        let composed = self.marked.take().is_some();
        let typed = self.typed.take();
        if text.is_empty() || !window.is_window_active() {
            return;
        }
        for input in text_input(text, typed.as_ref(), composed) {
            self.send(input);
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.typed = None;
        self.marked = (!text.is_empty()).then(|| text.to_owned());
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        frame: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let at = self.caret.unwrap_or(frame.origin);
        Some(Bounds::new(at, gpui::size(px(1.), px(tokens().size.icon))))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// Send the mouse in the frame `shown`, of a page of `page` CSS pixels, to
/// the view for the next frame. A press in the frame gives the view the
/// keyboard; one beside it, in the rest of `area`, does not. While a button
/// pressed in the frame is held, moves and its release anywhere reach the
/// page at the frame's nearest edge.
fn listen(
    window: &mut Window,
    view: gpui::WeakEntity<BrowserView>,
    focus: &FocusHandle,
    band: gpui::Hitbox,
    shown: Bounds<Pixels>,
    page: (f32, f32),
    hitbox: gpui::Hitbox,
) {
    let point = move |position, held| page_point(position, shown, page, held);
    window.on_mouse_event({
        let (view, focus, hitbox) = (view.clone(), focus.clone(), hitbox.clone());
        move |event: &MouseDownEvent, phase, window, cx| {
            // This runs before the view's own focus on a press, which
            // `prevent_default` stops.
            if phase != DispatchPhase::Bubble {
                return;
            }
            if !hitbox.is_hovered(window) {
                // Beside the frame, in the pane's band, the view does not
                // take the keyboard; anything drawn over the band does.
                if band.is_hovered(window) {
                    window.prevent_default();
                }
                return;
            }
            window.prevent_default();
            // Only a press the page is sent takes the keyboard.
            if pressable(event.button, &event.modifiers).is_none() {
                return;
            }
            window.focus(&focus, cx);
            let at = point(event.position, false);
            let _ = view.update(cx, |view, _| {
                // A click with no move before it is over the page too.
                if let Some(stream) = &view.stream {
                    stream.set_hovered(true);
                }
                view.pressed(event, at);
            });
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture {
                let at = point(event.position, true);
                let _ = view.update(cx, |view, _| view.released(event, at));
            }
        }
    });
    window.on_mouse_event({
        let (view, hitbox) = (view.clone(), hitbox.clone());
        move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            let over = hitbox.is_hovered(window) && window.is_window_active();
            let _ = view.update(cx, |view, _| {
                let held = view.pointing.held != 0;
                let at = (held || over)
                    .then(|| point(event.position, held))
                    .flatten();
                if let Some(stream) = &view.stream {
                    stream.set_hovered(over);
                }
                view.moved(event, at);
            });
        }
    });
    window.on_mouse_event({
        let view = view.clone();
        move |event: &gpui::MouseExitEvent, phase, _, cx| {
            if phase != DispatchPhase::Bubble {
                return;
            }
            // The pointer left the window, from over the page or not.
            let _ = view.update(cx, |view, _| {
                if let Some(stream) = &view.stream {
                    stream.set_hovered(false);
                }
                let modifiers = mouse_modifiers(&event.modifiers);
                let left = view
                    .pointing
                    .moved(None, event.pressed_button.is_some(), modifiers);
                for input in left {
                    view.send(input);
                }
            });
        }
    });
    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble
            || !hitbox.should_handle_scroll(window)
            || !window.is_window_active()
        {
            return;
        }
        let Some((x, y)) = point(event.position, false) else {
            return;
        };
        let (dx, dy) = wheel(event.delta, shown, page);
        let _ = view.update(cx, |view, _| {
            view.send(Input::Wheel {
                x,
                y,
                dx,
                dy,
                modifiers: modifiers(&event.modifiers) & SHIFT,
            });
        });
    });
}

/// The page point, in CSS pixels, at `position` in a frame of a page of
/// `page` CSS pixels drawn at `shown`: none beside the frame, unless `clamp`
/// brings it to the frame's nearest edge.
fn page_point(
    position: Point<Pixels>,
    shown: Bounds<Pixels>,
    page: (f32, f32),
    clamp: bool,
) -> Option<(f32, f32)> {
    let (width, height) = (f32::from(shown.size.width), f32::from(shown.size.height));
    if width <= 0. || height <= 0. {
        return None;
    }
    let across = (f32::from(position.x) - f32::from(shown.origin.x)) / width;
    let down = (f32::from(position.y) - f32::from(shown.origin.y)) / height;
    if !clamp && !((0.0..1.0).contains(&across) && (0.0..1.0).contains(&down)) {
        return None;
    }
    Some((across.clamp(0., 1.) * page.0, down.clamp(0., 1.) * page.1))
}

/// A wheel turn's distance in the page's CSS pixels, positive down and
/// right: a trackpad's points scaled as the frame is, a wheel's lines at
/// Chrome's 40 pixels each.
fn wheel(delta: ScrollDelta, shown: Bounds<Pixels>, page: (f32, f32)) -> (f32, f32) {
    match delta {
        ScrollDelta::Pixels(points) => (
            -f32::from(points.x) * page.0 / f32::from(shown.size.width),
            -f32::from(points.y) * page.1 / f32::from(shown.size.height),
        ),
        ScrollDelta::Lines(lines) => (-lines.x * 40., -lines.y * 40.),
    }
}

/// The mouse as the page last heard of it: the buttons pressed in the frame,
/// as bits, and the last point and buttons sent.
#[derive(Default)]
struct Pointing {
    held: u8,
    last: Option<(f32, f32, u8)>,
}

impl Pointing {
    /// The press of `button` at `at`, a point of the page.
    fn press(&mut self, at: (f32, f32), button: Button, clicks: usize) -> Input {
        self.held |= button.bit();
        self.last = Some((at.0, at.1, self.held));
        Input::Pressed(Pointer {
            x: at.0,
            y: at.1,
            button,
            buttons: self.held,
            clicks,
            modifiers: CLICK,
        })
    }

    /// The release of `button` at `at`, if it was pressed in the frame;
    /// the other buttons stay held.
    fn release(&mut self, at: (f32, f32), button: Button, clicks: usize) -> Option<Input> {
        if self.held & button.bit() == 0 {
            return None;
        }
        self.held &= !button.bit();
        self.last = Some((at.0, at.1, self.held));
        Some(Input::Released(Pointer {
            x: at.0,
            y: at.1,
            button,
            buttons: self.held,
            clicks,
            modifiers: CLICK,
        }))
    }

    /// A move to `at`, a point of the page, or beside the frame, with
    /// `pressed` when the platform reports a button down. Buttons released
    /// while no frame was drawn are released in the page first. A pointer
    /// that leaves the frame with no button held moves just outside the page,
    /// so its hover ends; a drag that began elsewhere is not the page's, and
    /// a point sent already is not sent again, as the platform repeats a
    /// drag held still.
    fn moved(&mut self, at: Option<(f32, f32)>, pressed: bool, modifiers: u8) -> Vec<Input> {
        let mut sent = Vec::new();
        if !pressed && self.held != 0 {
            let point = at.or(self.last.map(|(x, y, _)| (x, y))).unwrap_or_default();
            for button in [Button::Left, Button::Right, Button::Middle] {
                sent.extend(self.release(point, button, 1));
            }
        }
        let (x, y) = match at {
            Some(_) if self.held == 0 && pressed => return sent,
            Some(at) => at,
            None if self.held == 0 && self.last.is_some_and(|(x, y, _)| x >= 0. && y >= 0.) => {
                (-1., -1.)
            }
            None => return sent,
        };
        if self.last.replace((x, y, self.held)) == Some((x, y, self.held)) {
            return sent;
        }
        let button = [Button::Left, Button::Right, Button::Middle]
            .into_iter()
            .find(|button| self.held & button.bit() != 0)
            .unwrap_or(Button::None);
        sent.push(Input::Moved(Pointer {
            x,
            y,
            button,
            buttons: self.held,
            clicks: 0,
            modifiers,
        }));
        sent
    }
}

/// The page's name for a mouse button the view sends: the left one only. A
/// right or middle press could open Chrome's own menu, or a link in a new
/// tab, in a browser window the view never shows.
fn button(button: MouseButton) -> Option<Button> {
    (button == MouseButton::Left).then_some(Button::Left)
}

/// The button of a press the view sends: a left press without Ctrl or Cmd,
/// since a Ctrl-click is a right click on macOS and a Cmd- or Ctrl-click
/// opens a link in a new tab.
fn pressable(pressed: MouseButton, held: &Modifiers) -> Option<Button> {
    button(pressed).filter(|_| !(held.control || held.platform))
}

/// The modifier keys a press or release carries: none, since a Shift-click
/// opens a link in a new window and an Option-click downloads it, outside the
/// view. The wheel carries Shift only, which scrolls across.
const CLICK: u8 = 0;

/// The page's bits for the modifier keys held while the mouse moves: Shift
/// and Option only, as a press with Ctrl or Cmd is not sent.
fn mouse_modifiers(held: &Modifiers) -> u8 {
    modifiers(held) & (ALT | SHIFT)
}

/// The page's bits for the modifier keys held; Cmd is Meta.
fn modifiers(held: &Modifiers) -> u8 {
    [
        (held.alt, ALT),
        (held.control, CTRL),
        (held.platform, META),
        (held.shift, SHIFT),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(0, |bits, (_, bit)| bits | bit)
}

/// What a key press in the view does.
#[derive(Debug, PartialEq)]
enum Press {
    /// Its key down and up go to the page.
    Send([Key; 2]),
    /// A printable key, sent once the input handler brings its text, so an
    /// input method can compose it first.
    Type,
    /// The app's, or nothing the page is sent.
    Pass,
}

/// What `keystroke`, `held` down, does in the view, on macOS when `mac`:
/// named keys and combinations with Ctrl or Cmd go to the page, except the
/// clipboard's keys, which are not sent. Chrome on macOS runs no editing
/// command for Cmd-A, Cmd-Z or Cmd-Shift-Z by itself, so they carry theirs.
fn key_press(keystroke: &Keystroke, held: bool, mac: bool) -> Press {
    let held_down = &keystroke.modifiers;
    let pair = |kind, key: String, code, virtual_key, text, commands| {
        let down = Key {
            kind,
            key,
            code,
            virtual_key,
            text,
            modifiers: modifiers(held_down),
            repeat: held,
            commands,
        };
        let up = Key {
            kind: KeyKind::Up,
            text: None,
            repeat: false,
            commands: &[],
            ..down.clone()
        };
        Press::Send([down, up])
    };
    let command = held_down.control || held_down.platform;
    if let Some((key, code, virtual_key, text)) = named(&keystroke.key) {
        // Enter types a line only without Ctrl or Cmd.
        let text = text.filter(|_| !command).map(str::to_owned);
        let kind = if text.is_some() {
            KeyKind::Down
        } else {
            KeyKind::RawDown
        };
        let commands = if mac { mac_editing(keystroke) } else { &[] };
        return pair(kind, key.to_owned(), code, virtual_key, text, commands);
    }
    if !command {
        let printable = keystroke
            .key_char
            .as_deref()
            .is_some_and(|text| !text.is_empty() && !text.chars().any(char::is_control));
        return if printable { Press::Type } else { Press::Pass };
    }
    let clipboard = if mac {
        held_down.platform
    } else {
        held_down.control
    };
    if clipboard && matches!(keystroke.key.as_str(), "c" | "v" | "x") {
        return Press::Pass;
    }
    let Some((code, virtual_key)) = physical(&keystroke.key) else {
        return Press::Pass;
    };
    let key = match keystroke.key.as_str() {
        "space" => " ".to_owned(),
        letter if held_down.shift && letter.len() == 1 => letter.to_ascii_uppercase(),
        key => key.to_owned(),
    };
    let commands = if mac { mac_editing(keystroke) } else { &[] };
    pair(KeyKind::RawDown, key, code, virtual_key, None, commands)
}

/// The editing command Chrome on macOS runs for `keystroke`, which it does
/// not find itself there: select all, undo and redo, moving by word with
/// Option and to the line's or document's ends with Cmd, Shift extending
/// the selection, and deleting by word or to the line's start.
fn mac_editing(keystroke: &Keystroke) -> &'static [&'static str] {
    let held = &keystroke.modifiers;
    if held.control || held.function {
        return &[];
    }
    let (option, command, shift) = (held.alt, held.platform, held.shift);
    match (keystroke.key.as_str(), option, command, shift) {
        ("a", false, true, false) => &["selectAll"],
        ("z", false, true, false) => &["undo"],
        ("z", false, true, true) => &["redo"],
        ("left", true, false, false) => &["moveWordLeft"],
        ("left", true, false, true) => &["moveWordLeftAndModifySelection"],
        ("right", true, false, false) => &["moveWordRight"],
        ("right", true, false, true) => &["moveWordRightAndModifySelection"],
        ("left", false, true, false) => &["moveToBeginningOfLine"],
        ("left", false, true, true) => &["moveToBeginningOfLineAndModifySelection"],
        ("right", false, true, false) => &["moveToEndOfLine"],
        ("right", false, true, true) => &["moveToEndOfLineAndModifySelection"],
        ("up", false, true, false) => &["moveToBeginningOfDocument"],
        ("up", false, true, true) => &["moveToBeginningOfDocumentAndModifySelection"],
        ("down", false, true, false) => &["moveToEndOfDocument"],
        ("down", false, true, true) => &["moveToEndOfDocumentAndModifySelection"],
        ("backspace", true, false, false) => &["deleteWordBackward"],
        ("backspace", false, true, false) => &["deleteToBeginningOfLine"],
        ("delete", true, false, false) => &["deleteWordForward"],
        _ => &[],
    }
}

/// Whether a key sent to the page also goes on to the system: on macOS, a
/// Cmd key that types nothing, so its own key equivalents, as Cmd-`
/// switching windows, still work. Cmd-Enter, Cmd-Space and Cmd-Tab carry a
/// character, which a held key would also insert as text, so they stop here.
fn goes_on(keystroke: &Keystroke, mac: bool) -> bool {
    mac && keystroke.modifiers.platform && keystroke.key_char.is_none()
}

/// What text the input handler brings sends: the text of the printable key
/// just pressed, `typed`, as that key, so the page sees it pressed and
/// typed; text an input method `composed`, or any other, inserted as it is.
fn text_input(text: &str, typed: Option<&(Keystroke, bool)>, composed: bool) -> Vec<Input> {
    match typed {
        Some((keystroke, held)) if !composed && keystroke.key_char.as_deref() == Some(text) => {
            let (code, virtual_key) = physical(&keystroke.key).unwrap_or(("", 0));
            let down = Key {
                kind: KeyKind::Down,
                key: text.to_owned(),
                code,
                virtual_key,
                text: Some(text.to_owned()),
                modifiers: modifiers(&keystroke.modifiers),
                repeat: *held,
                commands: &[],
            };
            let up = Key {
                kind: KeyKind::Up,
                text: None,
                repeat: false,
                ..down.clone()
            };
            vec![Input::Key(down), Input::Key(up)]
        }
        _ => vec![Input::Text(text.to_owned())],
    }
}

/// A named key's DOM key and code, its virtual key code, and the text it
/// types.
fn named(key: &str) -> Option<(&'static str, &'static str, u32, Option<&'static str>)> {
    let (name, virtual_key, text) = match key {
        "enter" => ("Enter", 13, Some("\r")),
        "tab" => ("Tab", 9, None),
        "backspace" => ("Backspace", 8, None),
        "delete" => ("Delete", 46, None),
        "escape" => ("Escape", 27, None),
        "insert" => ("Insert", 45, None),
        "up" => ("ArrowUp", 38, None),
        "down" => ("ArrowDown", 40, None),
        "left" => ("ArrowLeft", 37, None),
        "right" => ("ArrowRight", 39, None),
        "home" => ("Home", 36, None),
        "end" => ("End", 35, None),
        "pageup" => ("PageUp", 33, None),
        "pagedown" => ("PageDown", 34, None),
        "f1" => ("F1", 112, None),
        "f2" => ("F2", 113, None),
        "f3" => ("F3", 114, None),
        "f4" => ("F4", 115, None),
        "f5" => ("F5", 116, None),
        "f6" => ("F6", 117, None),
        "f7" => ("F7", 118, None),
        "f8" => ("F8", 119, None),
        "f9" => ("F9", 120, None),
        "f10" => ("F10", 121, None),
        "f11" => ("F11", 122, None),
        "f12" => ("F12", 123, None),
        _ => return None,
    };
    Some((name, name, virtual_key, text))
}

/// The DOM code and virtual key code of the key that types `key` on a US
/// layout, shifted or not.
fn physical(key: &str) -> Option<(&'static str, u32)> {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK",
        "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV",
        "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = [
        "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
        "Digit9",
    ];
    let mut chars = key.chars();
    let (Some(char), None) = (chars.next(), chars.next()) else {
        return (key == "space").then_some(("Space", 32));
    };
    let digit = |digit: u8| (DIGITS[usize::from(digit)], 48 + u32::from(digit));
    Some(match char.to_ascii_lowercase() {
        letter @ 'a'..='z' => {
            let index = letter as u8 - b'a';
            (LETTERS[usize::from(index)], 65 + u32::from(index))
        }
        number @ '0'..='9' => digit(number as u8 - b'0'),
        ')' => digit(0),
        '!' => digit(1),
        '@' => digit(2),
        '#' => digit(3),
        '$' => digit(4),
        '%' => digit(5),
        '^' => digit(6),
        '&' => digit(7),
        '*' => digit(8),
        '(' => digit(9),
        ' ' => ("Space", 32),
        '`' | '~' => ("Backquote", 192),
        '-' | '_' => ("Minus", 189),
        '=' | '+' => ("Equal", 187),
        '[' | '{' => ("BracketLeft", 219),
        ']' | '}' => ("BracketRight", 221),
        '\\' | '|' => ("Backslash", 220),
        ';' | ':' => ("Semicolon", 186),
        '\'' | '"' => ("Quote", 222),
        ',' | '<' => ("Comma", 188),
        '.' | '>' => ("Period", 190),
        '/' | '?' => ("Slash", 191),
        _ => return None,
    })
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
        // A command that failed or timed out may still have taken effect, so
        // the size is then unknown.
        page.set = set.then_some(viewport);
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

/// A browser Agent Backbone runs, as a Browser pane watches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Watched {
    /// Its name for people.
    pub name: String,
    /// The port of the DevTools endpoint Backbone gives the owner.
    pub port: u16,
    /// The agent-browser session its holder drives it with, whose tab the
    /// view shows.
    pub session: Option<String>,
}

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
        for (id, view) in &self.browser_views {
            let chosen = self.browser_chosen.as_ref() == Some(id);
            view.update(cx, |view, cx| {
                let session = found.iter().find(|session| session.name == view.agent);
                view.set_session(session.cloned(), cx);
                if chosen {
                    view.chosen();
                }
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

    /// What the agents panel shows for `session`: its page's host, else
    /// [`Self::agent_browser_label`].
    pub(super) fn agent_browser_host(&self, session: &Session, cx: &gpui::App) -> String {
        session
            .address
            .as_deref()
            .and_then(host)
            .map_or_else(|| self.agent_browser_label(session, cx), str::to_owned)
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
        let mut created = Vec::new();
        for (id, agent) in shown {
            if !self.browser_views.contains_key(&id) {
                let shapes = self.browser_shapes.clone();
                let view = match self.watched_browsers.get(&agent) {
                    Some(watched) => {
                        let target = watched
                            .session
                            .as_deref()
                            .and_then(|session| self.agent_browser(session)?.target.clone());
                        let (name, port) = (watched.name.clone(), watched.port);
                        cx.new(|cx| BrowserView::watching(agent, name, port, target, shapes, cx))
                    }
                    None => {
                        let session = self.agent_browser(&agent).cloned();
                        cx.new(|cx| BrowserView::new(agent, session, shapes, cx))
                    }
                };
                self.browser_views.insert(id.clone(), view);
                created.push(id);
            }
        }
        // The view chosen last gives its browser its size, as tmux sizes a
        // window for its latest client.
        let chosen = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().pane().map(|leaf| leaf.id.clone()))
            .filter(|id| self.browser_views.contains_key(id));
        // A view made anew for the chosen pane claims too.
        if chosen != self.browser_chosen || chosen.as_ref().is_some_and(|id| created.contains(id)) {
            if let Some(view) = chosen.as_ref().and_then(|id| self.browser_views.get(id)) {
                view.update(cx, |view, cx| view.claim(true, cx));
            }
            self.browser_chosen = chosen;
        }
    }

    /// Show a browser Agent Backbone runs, through the DevTools endpoint it
    /// gives the owner, as an agent's browser is shown. A view of it at an
    /// earlier endpoint is made anew.
    pub(super) fn watch_browser(
        &mut self,
        watch: &super::views::WatchBrowser,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = format!("browser:{}", watch.id);
        let watched = Watched {
            name: watch.name.clone(),
            port: watch.port,
            session: watch.session.clone(),
        };
        if self.watched_browsers.insert(key.clone(), watched.clone()) != Some(watched) {
            let stale: Vec<String> = self
                .browser_views
                .iter()
                .filter(|(_, view)| view.read(cx).agent == key)
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                if let Some(view) = self.browser_views.remove(&id) {
                    for frame in view.update(cx, |view, _| view.take_shown()) {
                        let _ = window.drop_image(frame.image);
                    }
                }
            }
        }
        self.open_agent_browser(&key, window, cx);
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
        // The first failed: no size is known, and the newest follows.
        assert_eq!(shapes.ended(&page(), at(800, 2.), false), Some(at(600, 2.)));
        assert!(shapes.needed(&page(), at(800, 2.), shown(800)));
        assert_eq!(shapes.ended(&page(), at(600, 2.), true), None);
        assert!(!shapes.needed(&page(), at(600, 2.), shown(600)));
        // A size set before is sent again after a command that failed, as
        // that one may have taken effect.
        assert!(shapes.start(&page(), at(500, 2.)));
        assert!(!shapes.start(&page(), at(600, 2.)));
        assert_eq!(shapes.ended(&page(), at(500, 2.), false), Some(at(600, 2.)));
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
    fn a_point_in_the_frame_is_the_same_point_of_the_page() {
        // A page of 1280 by 800 CSS pixels drawn at 640 by 400 points, below
        // a header and beside a band: two CSS pixels a point.
        let shown = Bounds::new(point(px(10.), px(40.)), size(px(640.), px(400.)));
        let at =
            |x: f32, y: f32, clamp| page_point(point(px(x), px(y)), shown, (1280., 800.), clamp);
        assert_eq!(at(10., 40., false), Some((0., 0.)));
        assert_eq!(at(330., 240., false), Some((640., 400.)));
        // At one CSS pixel a point, as when the viewport follows the pane.
        let same = page_point(point(px(110.), px(90.)), shown, (640., 400.), false);
        assert_eq!(same, Some((100., 50.)));
        // Beside the frame, in the band or the header, is not the page.
        assert_eq!(at(650., 100., false), None);
        assert_eq!(at(100., 30., false), None);
        // A drag that began inside reaches the nearest edge.
        assert_eq!(at(700., 30., true), Some((1280., 0.)));
        assert_eq!(at(-5., 240., true), Some((0., 400.)));
    }

    #[test]
    fn the_wheel_scrolls_the_page_by_its_own_pixels() {
        let shown = Bounds::new(point(px(0.), px(0.)), size(px(640.), px(400.)));
        let trackpad = ScrollDelta::Pixels(point(px(3.), px(-12.5)));
        assert_eq!(wheel(trackpad, shown, (640., 400.)), (-3., 12.5));
        assert_eq!(wheel(trackpad, shown, (1280., 800.)), (-6., 25.));
        // A wheel's line, up: 40 pixels.
        assert_eq!(
            wheel(ScrollDelta::Lines(point(0., 1.)), shown, (1280., 800.)),
            (-0., -40.)
        );
    }

    fn keystroke(text: &str, key_char: Option<&str>) -> Keystroke {
        let mut keystroke = Keystroke::parse(text).unwrap();
        keystroke.key_char = key_char.map(str::to_owned);
        keystroke
    }

    /// The key down a press sends, if it sends one.
    fn down(text: &str, key_char: Option<&str>, mac: bool) -> Option<Key> {
        match key_press(&keystroke(text, key_char), false, mac) {
            Press::Send([down, up]) => {
                assert_eq!(
                    (up.kind, &up.key, up.text.as_ref()),
                    (KeyKind::Up, &down.key, None)
                );
                Some(down)
            }
            _ => None,
        }
    }

    #[test]
    fn printable_keys_wait_for_their_text_and_named_keys_go_at_once() {
        for (text, key_char) in [("a", "a"), ("shift-a", "A"), ("alt-e", "´"), ("space", " ")] {
            assert_eq!(
                key_press(&keystroke(text, Some(key_char)), false, true),
                Press::Type,
                "{text}"
            );
        }
        let enter = down("enter", Some("\n"), true).unwrap();
        assert_eq!(
            (
                enter.kind,
                enter.key.as_str(),
                enter.code,
                enter.virtual_key,
                enter.text.as_deref()
            ),
            (KeyKind::Down, "Enter", "Enter", 13, Some("\r"))
        );
        let tab = down("shift-tab", Some("\t"), true).unwrap();
        assert_eq!(
            (tab.kind, tab.virtual_key, tab.modifiers),
            (KeyKind::RawDown, 9, SHIFT)
        );
        for (text, key, virtual_key) in [
            ("backspace", "Backspace", 8),
            ("delete", "Delete", 46),
            ("escape", "Escape", 27),
            ("left", "ArrowLeft", 37),
            ("pagedown", "PageDown", 34),
            ("f5", "F5", 116),
        ] {
            let sent = down(text, None, true).unwrap();
            assert_eq!((sent.key.as_str(), sent.virtual_key), (key, virtual_key));
            assert_eq!((sent.kind, sent.text), (KeyKind::RawDown, None));
        }
        // A key held down repeats.
        let Press::Send([held, _]) = key_press(&keystroke("down", None), true, true) else {
            panic!("an arrow is sent");
        };
        assert!(held.repeat);
        // Cmd-Enter submits without typing a line.
        let submit = down("cmd-enter", None, true).unwrap();
        assert_eq!(
            (submit.kind, submit.modifiers, submit.text),
            (KeyKind::RawDown, META, None)
        );
    }

    #[test]
    fn shortcuts_go_with_their_modifiers_and_the_clipboard_stays_the_apps() {
        let undo = down("cmd-z", None, true).unwrap();
        assert_eq!(
            (
                undo.key.as_str(),
                undo.code,
                undo.virtual_key,
                undo.modifiers,
                undo.commands
            ),
            ("z", "KeyZ", 90, META, &["undo"][..])
        );
        let redo = down("cmd-shift-z", None, true).unwrap();
        assert_eq!(
            (redo.key.as_str(), redo.modifiers, redo.commands),
            ("Z", META | SHIFT, &["redo"][..])
        );
        assert_eq!(down("cmd-a", None, true).unwrap().commands, ["selectAll"]);
        // Chrome on Linux finds its editing commands itself.
        let all = down("ctrl-a", None, false).unwrap();
        assert_eq!((all.modifiers, all.commands), (CTRL, &[][..]));
        let slash = down("ctrl-/", None, false).unwrap();
        assert_eq!((slash.code, slash.virtual_key), ("Slash", 191));
        for (text, mac) in [
            ("cmd-c", true),
            ("cmd-v", true),
            ("cmd-x", true),
            ("ctrl-v", false),
        ] {
            assert_eq!(
                key_press(&keystroke(text, None), false, mac),
                Press::Pass,
                "{text}"
            );
        }
        // Ctrl-C is the clipboard's only off macOS.
        assert!(down("ctrl-c", None, true).is_some());
    }

    #[test]
    fn typed_text_arrives_as_its_key_and_composed_text_as_text() {
        let typed = |text: &str, key_char: &str| (keystroke(text, Some(key_char)), false);
        let keys = |inputs: Vec<Input>| -> Vec<(KeyKind, String, &'static str, u8)> {
            inputs
                .into_iter()
                .map(|input| match input {
                    Input::Key(key) => (key.kind, key.key, key.code, key.modifiers),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        assert_eq!(
            keys(text_input("a", Some(&typed("a", "a")), false)),
            [
                (KeyKind::Down, "a".into(), "KeyA", 0),
                (KeyKind::Up, "a".into(), "KeyA", 0)
            ]
        );
        let shifted = text_input("A", Some(&typed("shift-a", "A")), false);
        assert_eq!(keys(shifted)[0], (KeyKind::Down, "A".into(), "KeyA", SHIFT));
        let Input::Key(slash) = &text_input("/", Some(&typed("/", "/")), false)[0] else {
            panic!("a key");
        };
        assert_eq!(slash.text.as_deref(), Some("/"));
        // A dead key's accent, a CJK word, or text no key typed: inserted.
        for (text, typed, composed) in [
            ("é", Some(typed("e", "e")), true),
            ("日本", None, true),
            ("´", Some(typed("alt-e", "´")), true),
            ("ü", Some(typed("u", "u")), false),
        ] {
            assert_eq!(
                text_input(text, typed.as_ref(), composed),
                [Input::Text(text.into())]
            );
        }
    }

    /// The kind, point, button and buttons held of each mouse input.
    fn mice(inputs: impl IntoIterator<Item = Input>) -> Vec<(&'static str, f32, f32, Button, u8)> {
        inputs
            .into_iter()
            .map(|input| match input {
                Input::Pressed(p) => ("pressed", p.x, p.y, p.button, p.buttons),
                Input::Released(p) => ("released", p.x, p.y, p.button, p.buttons),
                Input::Moved(p) => ("moved", p.x, p.y, p.button, p.buttons),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn each_button_is_released_on_its_own_and_the_rest_stay_held() {
        let mut pointing = Pointing::default();
        let left = pointing.press((10., 10.), Button::Left, 1);
        let right = pointing.press((10., 10.), Button::Right, 1);
        assert_eq!(
            mice([left, right]),
            [
                ("pressed", 10., 10., Button::Left, 1),
                ("pressed", 10., 10., Button::Right, 3)
            ]
        );
        // A drag with both: the move carries both, the first as its button.
        let drag = pointing.moved(Some((20., 10.)), true, 0);
        assert_eq!(mice(drag), [("moved", 20., 10., Button::Left, 3)]);
        let right = pointing.release((20., 10.), Button::Right, 1).unwrap();
        assert_eq!(mice([right]), [("released", 20., 10., Button::Right, 1)]);
        // A button not pressed in the frame is not released in the page.
        assert!(pointing.release((20., 10.), Button::Middle, 1).is_none());
        let left = pointing.release((20., 10.), Button::Left, 1).unwrap();
        assert_eq!(mice([left]), [("released", 20., 10., Button::Left, 0)]);
    }

    #[test]
    fn a_release_missed_while_no_frame_was_drawn_reaches_the_page() {
        let mut pointing = Pointing::default();
        pointing.press((10., 10.), Button::Left, 1);
        // The next move reports no button down.
        let moved = pointing.moved(Some((30., 40.)), false, 0);
        assert_eq!(
            mice(moved),
            // The release is at the new point, so no move follows it.
            [("released", 30., 40., Button::Left, 0)]
        );
        assert!(pointing.moved(Some((30., 40.)), false, 0).is_empty());
    }

    #[test]
    fn hover_ends_when_the_pointer_leaves_and_foreign_drags_are_ignored() {
        let mut pointing = Pointing::default();
        assert_eq!(
            mice(pointing.moved(Some((5., 5.)), false, 0)),
            [("moved", 5., 5., Button::None, 0)]
        );
        // Out of the frame: once, just outside the page.
        let left = pointing.moved(None, false, 0);
        assert_eq!(mice(left), [("moved", -1., -1., Button::None, 0)]);
        assert!(pointing.moved(None, false, 0).is_empty());
        // A drag that began beside the frame is not the page's.
        assert!(pointing.moved(Some((5., 5.)), true, 0).is_empty());
    }

    #[test]
    fn only_the_left_button_with_shift_or_option_reaches_the_page() {
        assert_eq!(button(MouseButton::Left), Some(Button::Left));
        for other in [
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Navigate(gpui::NavigationDirection::Back),
        ] {
            assert_eq!(button(other), None);
        }
        let held = Modifiers {
            control: true,
            alt: true,
            shift: true,
            platform: true,
            function: false,
        };
        assert_eq!(mouse_modifiers(&held), ALT | SHIFT);
        // A Ctrl- or Cmd-click is not sent at all; Shift and Option are.
        let with = |control, platform| Modifiers {
            control,
            platform,
            shift: true,
            alt: true,
            function: false,
        };
        assert_eq!(
            pressable(MouseButton::Left, &with(false, false)),
            Some(Button::Left)
        );
        assert_eq!(pressable(MouseButton::Left, &with(true, false)), None);
        assert_eq!(pressable(MouseButton::Left, &with(false, true)), None);
        assert_eq!(pressable(MouseButton::Right, &with(false, false)), None);
        // A Shift- or Option-click opens a window or a download outside the
        // view, so presses and releases carry no modifiers at all.
        let mut pointing = Pointing::default();
        let pressed = pointing.press((1., 1.), Button::Left, 1);
        let released = pointing.release((1., 1.), Button::Left, 1).unwrap();
        for input in [pressed, released] {
            let (Input::Pressed(pointer) | Input::Released(pointer)) = input else {
                panic!("{input:?}");
            };
            assert_eq!(pointer.modifiers, 0);
        }
    }

    #[test]
    fn only_a_cmd_key_that_types_nothing_goes_on_to_macos() {
        assert!(goes_on(&keystroke("cmd-`", None), true));
        assert!(goes_on(&keystroke("cmd-z", None), true));
        for (text, key_char) in [("cmd-enter", "\n"), ("cmd-space", " "), ("cmd-tab", "\t")] {
            assert!(!goes_on(&keystroke(text, Some(key_char)), true), "{text}");
        }
        assert!(!goes_on(&keystroke("ctrl-a", None), true));
        assert!(!goes_on(&keystroke("ctrl-z", None), false));
    }

    #[test]
    fn macos_editing_keys_carry_their_commands() {
        let commands = |text: &str| down(text, None, true).unwrap().commands;
        for (text, command) in [
            ("alt-left", "moveWordLeft"),
            ("alt-shift-right", "moveWordRightAndModifySelection"),
            ("cmd-left", "moveToBeginningOfLine"),
            ("cmd-shift-right", "moveToEndOfLineAndModifySelection"),
            ("cmd-up", "moveToBeginningOfDocument"),
            ("cmd-shift-down", "moveToEndOfDocumentAndModifySelection"),
            ("alt-backspace", "deleteWordBackward"),
            ("cmd-backspace", "deleteToBeginningOfLine"),
            ("alt-delete", "deleteWordForward"),
        ] {
            assert_eq!(commands(text), [command], "{text}");
        }
        // Plain keys and other combinations: Chrome's own.
        for text in [
            "left",
            "shift-left",
            "ctrl-left",
            "cmd-alt-left",
            "backspace",
        ] {
            assert!(commands(text).is_empty(), "{text}");
        }
        // Off macOS, none.
        assert!(down("alt-left", None, false).unwrap().commands.is_empty());
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
