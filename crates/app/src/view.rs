//! The terminal grid: draws the panes' owned display data with GPUI
//! (`docs/ARCHITECTURE.md`).
//!
//! Terminal columns come from the display data, never from text layout.
//! Ligatures are off. With a monospaced font, each run of plain ASCII text
//! with one style is shaped as one line, with every character's advance
//! forced to one cell, and placed at its first column; every other
//! character, and every character of a proportional font, is shaped on its
//! own and placed at its own column.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui::{
    App, Bounds, Context, FocusHandle, Font, FontFeatures, FontStyle, FontWeight, Hsla,
    IntoElement, KeyDownEvent, Keystroke, Pixels, Render, Rgba, SharedString, Size,
    StrikethroughStyle, TextAlign, TextRun, TextSystem, UnderlineStyle, Window, canvas, div, fill,
    point, prelude::*, px, size,
};
use tmux_workspaces_term::{
    Attributes, Color, Screen, Style, Underline, Width,
    input::{Button, Input, KeyPress, MouseAction, Pointer, Scroll},
};
use tmux_workspaces_tmux::control::PaneId;

use crate::panes::{PaneLayout, Panes};

mod accessibility;
mod clipboard;
mod scroll;
mod selection;
use scroll::WheelGesture;
use selection::Selection;

/// The font and colours of the terminal grid.
#[derive(Clone, Debug, PartialEq)]
pub struct Appearance {
    /// The font family, such as `Menlo`.
    pub font_family: SharedString,
    /// The font size, in points.
    pub font_size: f32,
    /// The default text colour.
    pub foreground: Rgba,
    /// The default background colour.
    pub background: Rgba,
    /// The cursor's colour.
    pub cursor: Rgba,
    /// The first sixteen palette colours. The rest of the 256-colour palette
    /// is the standard colour cube and grey ramp.
    pub palette: [Rgba; 16],
}

/// The size of one terminal cell, in pixels.
pub fn cell_size(text_system: &TextSystem, appearance: &Appearance) -> Size<Pixels> {
    let font = terminal_font(&appearance.font_family);
    let font_id = text_system.resolve_font(&font);
    let font_size = px(appearance.font_size);
    let width = text_system
        .advance(font_id, font_size, 'M')
        .map(|advance| advance.width)
        .unwrap_or(font_size * 0.6);
    // Font metrics give the descent as a negative distance below the baseline.
    let ascent = f32::from(text_system.ascent(font_id, font_size));
    let descent = f32::from(text_system.descent(font_id, font_size)).abs();
    size(width, px((ascent + descent).ceil()))
}

/// Returns the font for terminal text: `family` with ligatures and other
/// contextual substitutions off, so that each character keeps its own cell.
fn terminal_font(family: &SharedString) -> Font {
    let mut font = gpui::font(family.clone());
    font.features = FontFeatures(Arc::new(
        ["calt", "liga", "dlig", "clig"]
            .into_iter()
            .map(|tag| (tag.to_owned(), 0))
            .collect(),
    ));
    font
}

/// Returns whether every printable ASCII character of the font advances as
/// far as `M`, so a run of them can be shaped as one line.
fn is_monospaced(text_system: &TextSystem, appearance: &Appearance) -> bool {
    let font_id = text_system.resolve_font(&terminal_font(&appearance.font_family));
    let size = px(appearance.font_size);
    let advance = |ch| text_system.advance(font_id, size, ch).map(|a| a.width).ok();
    let cell = advance('M');
    cell.is_some() && (b' '..=b'~').all(|byte| advance(char::from(byte)) == cell)
}

/// A view that draws the panes of one tmux window and sends typed keys to one
/// of them.
pub struct TerminalView {
    panes: Option<Arc<Panes>>,
    grid: Rc<RefCell<Grid>>,
    // Input goes to this exact pane after explicit clicking or navigation.
    input: Option<PaneId>,
    initial_input: PaneId,
    focus: FocusHandle,
    failure_reported: bool,
    embedded: bool,
    input_queue: InputQueue,
    input_error: Option<String>,
    input_rejection: Option<String>,
    closed_reported: bool,
    wheel: Option<(PaneId, f32)>,
    program_press: Option<(PaneId, Button, Pointer)>,
    pointer_subscriptions: Vec<gpui::Subscription>,
    wheel_gesture: WheelGesture,
}

pub(crate) struct Disconnected(pub Option<String>);
impl gpui::EventEmitter<Disconnected> for TerminalView {}

pub(crate) struct Focus;
impl gpui::EventEmitter<Focus> for TerminalView {}

/// What the grid draws, shared with the paint callback of each frame.
struct Grid {
    layout: Vec<PaneLayout>,
    screens: HashMap<PaneId, Screen>,
    appearance: Appearance,
    cell: Size<Pixels>,
    /// Whether runs of ASCII text may be shaped together: only with a
    /// monospaced font, so that no character moves off its column.
    batch_ascii: bool,
    /// The terminal font, built once: building it allocates.
    font: Font,
    bounds: Option<Bounds<Pixels>>,
    original_cell: Size<Pixels>,
    original_font_size: f32,
    selection: Option<Selection>,
    /// The size last declared to tmux for an embedded view, in cells.
    requested: Option<(u16, u16)>,
    contributes_size: bool,
}

impl TerminalView {
    /// Creates a view of the panes in `layout`, which `panes` keeps up to
    /// date. Typed keys go to `input`. `woken` receives a message whenever
    /// `panes` has an update.
    pub fn new(
        panes: Arc<Panes>,
        layout: Vec<PaneLayout>,
        input: PaneId,
        appearance: Appearance,
        woken: UnboundedReceiver<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self::create(panes, layout, input, appearance, woken, false, cx);
        window.focus(&view.focus, cx);
        view
    }

    /// A terminal inside a workspace. It takes focus only on a click and a
    /// detached session does not close the application.
    pub(crate) fn embedded(
        panes: Arc<Panes>,
        layout: Vec<PaneLayout>,
        input: PaneId,
        appearance: Appearance,
        woken: UnboundedReceiver<()>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::create(panes, layout, input, appearance, woken, true, cx)
    }

    #[allow(clippy::too_many_arguments)]
    fn create(
        panes: Arc<Panes>,
        layout: Vec<PaneLayout>,
        input: PaneId,
        appearance: Appearance,
        mut woken: UnboundedReceiver<()>,
        embedded: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        cx.on_release(|view, cx| {
            view.release_mouse();
            let drained = view.input_queue.close();
            let panes = view.panes.take();
            cx.background_executor()
                .spawn(async move {
                    if let Some(panes) = panes {
                        if let Some(drained) = drained {
                            let _ = drained.recv_timeout(std::time::Duration::from_secs(1));
                        }
                        panes.shutdown();
                    }
                })
                .detach();
        })
        .detach();
        let (status, mut statuses) = futures::channel::mpsc::unbounded();
        let input_queue = input_queue(Arc::clone(&panes), move |error| {
            let _ = status.unbounded_send(error);
        });
        cx.spawn(async move |this, cx| {
            while let Some(error) = statuses.next().await {
                if this
                    .update(cx, |view, cx| {
                        if view.input_error != error {
                            view.input_error = error;
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // An update is taken when the next frame is drawn, so the worker
        // copies at most one update per frame.
        cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                let closed = this.update(cx, |view, cx| {
                    let closed = view.panes().is_finished();
                    if closed {
                        view.report_failure();
                        if !view.embedded {
                            cx.quit();
                        }
                        cx.notify();
                    } else {
                        cx.notify();
                    }
                    closed
                });
                if closed.unwrap_or(true) {
                    break;
                }
            }
        })
        .detach();
        let cell = cell_size(cx.text_system(), &appearance);
        let batch_ascii = is_monospaced(cx.text_system(), &appearance);
        let font = terminal_font(&appearance.font_family);
        Self {
            panes: Some(panes),
            grid: Rc::new(RefCell::new(Grid {
                layout,
                screens: HashMap::new(),
                original_font_size: appearance.font_size,
                appearance,
                cell,
                original_cell: cell,
                bounds: None,
                batch_ascii,
                font,
                selection: None,
                requested: None,
                contributes_size: false,
            })),
            input: (!embedded).then_some(input),
            initial_input: input,
            focus,
            failure_reported: false,
            embedded,
            input_queue,
            input_error: None,
            input_rejection: None,
            closed_reported: false,
            wheel: None,
            program_press: None,
            pointer_subscriptions: Vec::new(),
            wheel_gesture: WheelGesture::default(),
        }
    }

    /// Use another font or colours. A new font size changes the cell size, so
    /// the session's size is declared again on the next frame.
    pub(crate) fn set_appearance(&mut self, appearance: &Appearance, cx: &mut Context<Self>) {
        let mut grid = self.grid.borrow_mut();
        let same_font = grid.original_font_size == appearance.font_size
            && grid.appearance.font_family == appearance.font_family;
        let unchanged = same_font
            && grid.appearance.foreground == appearance.foreground
            && grid.appearance.background == appearance.background
            && grid.appearance.cursor == appearance.cursor
            && grid.appearance.palette == appearance.palette;
        if unchanged {
            return;
        }
        if !same_font {
            let cell = cell_size(cx.text_system(), appearance);
            grid.original_cell = cell;
            grid.cell = cell;
            grid.original_font_size = appearance.font_size;
            grid.batch_ascii = is_monospaced(cx.text_system(), appearance);
            grid.font = terminal_font(&appearance.font_family);
            grid.requested = None;
        }
        let scaled = grid.appearance.font_size;
        grid.appearance = appearance.clone();
        if same_font {
            grid.appearance.font_size = scaled;
        }
        drop(grid);
        cx.notify();
    }

    fn panes(&self) -> &Panes {
        self.panes.as_deref().expect("terminal entity is alive")
    }

    /// A retained hidden view must not constrain a displayed client's size.
    pub(crate) fn hide(&mut self, cx: &mut Context<Self>) {
        let mut grid = self.grid.borrow_mut();
        if grid.contributes_size && self.panes().ignore_size().is_ok() {
            grid.contributes_size = false;
            // The next paint restores participation even at the same size.
            grid.requested = None;
            cx.notify();
        }
    }

    pub(crate) fn suspend_input(&mut self, cx: &mut Context<Self>) {
        self.input = None;
        self.release_mouse();
        self.clear_selection(cx);
    }

    /// Whether this terminal has keyboard focus.
    pub(crate) fn has_focus(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    pub(crate) fn focus_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.take_update();
        if self.panes().is_finished() {
            return;
        }
        let grid = self.grid.borrow();
        let preferred = self.input.unwrap_or(self.initial_input);
        let ready = |id: PaneId| {
            grid.layout.iter().any(|pane| pane.id == id) && grid.screens.contains_key(&id)
        };
        let input = if ready(preferred) {
            Some(preferred)
        } else {
            grid.layout.iter().map(|pane| pane.id).find(|id| ready(*id))
        };
        drop(grid);
        if let Some(input) = input {
            self.input = Some(input);
            window.focus(&self.focus, cx);
            cx.notify();
        }
    }

    fn report_failure(&mut self) {
        if !self.failure_reported
            && let Some(error) = self.panes().failure()
        {
            eprintln!("terminal connection: {error}");
            self.failure_reported = true;
        }
    }

    fn take_update(&mut self) {
        let Some(update) = self.panes().take_update() else {
            return;
        };
        let mut grid = self.grid.borrow_mut();
        if grid.layout != update.layout {
            grid.selection = None;
        }
        grid.layout = update.layout;
        let visible: Vec<_> = grid.layout.iter().map(|pane| pane.id).collect();
        grid.screens.retain(|id, _| visible.contains(id));
        let replaced: Vec<Screen> = update
            .screens
            .into_iter()
            .filter_map(|(pane, screen)| grid.screens.insert(pane, screen))
            .collect();
        self.panes().recycle(replaced);
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let copy = (key.modifiers.platform
            && !key.modifiers.control
            && !key.modifiers.alt
            && key.key == "c")
            || (!cfg!(target_os = "macos")
                && key.modifiers.control
                && key.modifiers.shift
                && !key.modifiers.alt
                && key.key == "c");
        if copy {
            if let Some(selection) = self
                .grid
                .borrow()
                .selection
                .as_ref()
                .filter(|selection| !selection.is_empty())
            {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(selection.text()));
            }
            cx.stop_propagation();
            return;
        }
        if key.key == "escape"
            && self
                .grid
                .borrow()
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.is_empty())
        {
            self.clear_selection(cx);
            cx.stop_propagation();
            return;
        }
        let Some(input) = self.input.filter(|input| {
            self.grid
                .borrow()
                .layout
                .iter()
                .any(|pane| pane.id == *input)
                && self.grid.borrow().screens.contains_key(input)
                && !self.panes().is_finished()
        }) else {
            return;
        };
        let key = &event.keystroke;
        let paste = (key.modifiers.platform
            && !key.modifiers.control
            && !key.modifiers.alt
            && key.key == "v")
            || (!cfg!(target_os = "macos")
                && key.modifiers.control
                && key.modifiers.shift
                && !key.modifiers.alt
                && key.key == "v");
        let scroll = if key.modifiers.shift
            && !key.modifiers.control
            && !key.modifiers.alt
            && !key.modifiers.platform
        {
            let rows = self
                .grid
                .borrow()
                .screens
                .get(&input)
                .map_or(1, |screen| screen.rows().saturating_sub(1).max(1) as isize);
            match key.key.as_str() {
                "pageup" => Some(Scroll::Lines(-rows)),
                "pagedown" => Some(Scroll::Lines(rows)),
                "home" => Some(Scroll::Top),
                "end" => Some(Scroll::Bottom),
                _ => None,
            }
        } else {
            None
        };
        let operation = if let Some(scroll) = scroll {
            cx.stop_propagation();
            Some(Input::Scroll(scroll))
        } else if paste {
            cx.stop_propagation();
            cx.read_from_clipboard()
                .and_then(clipboard::text)
                .map(Input::Paste)
        } else {
            input_key(key, event.is_held)
        };
        if let Some(operation) = operation {
            self.send_input(input, operation, cx);
        }
    }

    fn send_input(&mut self, input: PaneId, operation: Input, cx: &mut Context<Self>) {
        if operation.size() > crate::panes::INPUT_LIMIT {
            self.input_rejection = Some(
                "Input was not sent: paste exceeds 4 MiB. Click the terminal to dismiss.".into(),
            );
            cx.notify();
            return;
        }
        // Both keyboard pastes and drops use the same ordered, bounded queue.
        if self.input_queue.try_send((input, operation)).is_err() {
            self.input_rejection = Some("Input was not sent: terminal queue is busy or disconnected. Click the terminal to dismiss.".into());
            cx.notify();
        } else {
            self.clear_selection(cx);
        }
    }

    fn drop_files(
        &mut self,
        paths: &gpui::ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.panes().is_finished() {
            return;
        }
        let input = {
            let grid = self.grid.borrow();
            grid.bounds.and_then(|bounds| {
                hit_pane(
                    &grid.layout,
                    &grid.screens,
                    grid.cell,
                    bounds,
                    window.mouse_position(),
                )
            })
        };
        let Some(input) = input else { return };
        let Some(text) = clipboard::paths(paths) else {
            return;
        };
        self.input = Some(input);
        window.focus(&self.focus, cx);
        cx.emit(Focus);
        self.send_input(input, Input::Paste(text), cx);
        cx.stop_propagation();
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if self.grid.borrow_mut().selection.take().is_some() {
            cx.notify();
        }
    }

    fn selection_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mouse_move(event, window, cx);
        let mut grid = self.grid.borrow_mut();
        let Some(selection) = grid
            .selection
            .as_ref()
            .filter(|selection| selection.dragging)
        else {
            return;
        };
        if !event.dragging() {
            return;
        }
        let pane = selection.pane;
        if let Some((row, col)) =
            selection_point(&grid, pane, event.position, selection.uses_boundaries())
        {
            grid.selection.as_mut().unwrap().extend(row, col);
            cx.notify();
        }
    }

    fn selection_up(
        &mut self,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mouse_up(event, window);
        let mut grid = self.grid.borrow_mut();
        let position = grid
            .selection
            .as_ref()
            .filter(|selection| selection.dragging)
            .and_then(|selection| {
                selection_point(
                    &grid,
                    selection.pane,
                    event.position,
                    selection.uses_boundaries(),
                )
            });
        if let Some(selection) = &mut grid.selection {
            if let Some((row, col)) = position {
                selection.extend(row, col);
            }
            selection.dragging = false;
        }
        if grid.selection.as_ref().is_some_and(Selection::is_empty) {
            grid.selection = None;
        }
        drop(grid);
        cx.notify();
    }

    fn release_mouse(&mut self) {
        if let Some((pane, button, pointer)) = self.program_press.take() {
            let _ = self
                .input_queue
                .try_send((pane, Input::Mouse(pointer, MouseAction::Release(button))));
        }
    }

    fn mouse_down(
        &mut self,
        pane: PaneId,
        button: Button,
        pointer: Pointer,
        cx: &mut Context<Self>,
    ) {
        // One application gesture at a time. A second button does not retarget it.
        if self.program_press.is_some() || self.panes().is_finished() {
            return;
        }
        if self
            .input_queue
            .try_send((pane, Input::Mouse(pointer, MouseAction::Press(button))))
            .is_ok()
        {
            self.program_press = Some((pane, button, pointer));
        } else {
            self.input_rejection = Some("Click was not sent: terminal queue is busy or disconnected. Click the terminal to dismiss.".into());
            cx.notify();
        }
    }

    fn mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if let Some((pane, button, previous)) = self.program_press {
            // Backends report one pressed button, not the complete held set.
            // Only this button's up event or explicit cancellation ends ownership.
            let pointer = drag_pointer(
                &self.grid.borrow(),
                pane,
                event.position,
                window.scale_factor(),
                event.modifiers,
            )
            .unwrap_or(previous);
            self.program_press = Some((pane, button, pointer));
            // Intermediate motions may be dropped under pressure; the final release
            // retains its reserved queue slot and last pointer position.
            let _ = self
                .input_queue
                .try_send((pane, Input::Mouse(pointer, MouseAction::Move(Some(button)))));
        }
    }

    fn hover_mouse(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        if self.program_press.is_none() && event.pressed_button.is_none() {
            let grid = self.grid.borrow();
            if grid
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.is_empty())
            {
                return;
            }
            let Some(pane) = grid.bounds.and_then(|bounds| {
                hit_pane(
                    &grid.layout,
                    &grid.screens,
                    grid.cell,
                    bounds,
                    event.position,
                )
            }) else {
                return;
            };
            if !grid.screens[&pane].mouse_input() {
                return;
            }
            if let Some(pointer) = pointer_at(
                &grid,
                pane,
                event.position,
                window.scale_factor(),
                event.modifiers,
            ) {
                let _ = self
                    .input_queue
                    .try_send((pane, Input::Mouse(pointer, MouseAction::Move(None))));
            }
        }
    }

    fn mouse_up(&mut self, event: &gpui::MouseUpEvent, window: &mut Window) {
        let Some((pane, button, previous)) = self.program_press else {
            return;
        };
        if !mouse_button(event.button)
            .is_some_and(|released| ends_mouse_gesture(button, released, cfg!(target_os = "macos")))
        {
            return;
        }
        let pointer = drag_pointer(
            &self.grid.borrow(),
            pane,
            event.position,
            window.scale_factor(),
            event.modifiers,
        )
        .unwrap_or(previous);
        self.program_press = Some((pane, button, pointer));
        self.release_mouse();
    }

    fn secondary_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let grid = self.grid.borrow();
        if event.modifiers.shift
            || grid
                .selection
                .as_ref()
                .is_some_and(|selection| !selection.is_empty())
        {
            return;
        }
        let Some(pane) = grid.bounds.and_then(|bounds| {
            hit_pane(
                &grid.layout,
                &grid.screens,
                grid.cell,
                bounds,
                event.position,
            )
        }) else {
            return;
        };
        if !grid.screens[&pane].mouse_input() {
            return;
        }
        let pointer = pointer_at(
            &grid,
            pane,
            event.position,
            window.scale_factor(),
            event.modifiers,
        );
        drop(grid);
        if let Some(pointer) = pointer {
            self.input = Some(pane);
            window.focus(&self.focus, cx);
            self.mouse_down(pane, mouse_button(event.button).unwrap(), pointer, cx);
        }
    }

    fn secondary_up(
        &mut self,
        event: &gpui::MouseUpEvent,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.mouse_up(event, window);
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.take_update();
        if self.panes().is_finished() {
            self.report_failure();
            if self.embedded && !self.closed_reported {
                self.closed_reported = true;
                cx.emit(Disconnected(self.panes().failure()));
            }
            // The client has gone: close once this frame, which shows its last
            // output, has been drawn.
            if !self.embedded {
                window.on_next_frame(|_, cx| cx.quit());
            }
        }
        if self.pointer_subscriptions.is_empty() {
            self.pointer_subscriptions
                .push(cx.on_blur(&self.focus, window, |view, _, _| view.release_mouse()));
            self.pointer_subscriptions
                .push(cx.observe_window_activation(window, |view, window, _| {
                    if !window.is_window_active() {
                        view.release_mouse();
                    }
                }));
        }
        let grid = Rc::clone(&self.grid);
        let accessible_grid = Rc::clone(&self.grid);
        let display_scale = window.scale_factor();
        let input = self.input.filter(|_| !self.panes().is_finished());
        let selection_move = Rc::new(cx.listener(Self::selection_move));
        let background = self.grid.borrow().appearance.background;
        let embedded = self.embedded;
        let panes = self.panes.clone();
        let message = self
            .input_rejection
            .clone()
            .or(self.input_error.clone())
            .or_else(|| {
                self.panes().is_finished().then(|| {
                    self.panes()
                        .failure()
                        .unwrap_or_else(|| "Session disconnected.".into())
                })
            });
        div()
            .id(("terminal-input", cx.entity_id()))
            .size_full()
            .overflow_hidden()
            .relative()
            .bg(background)
            .track_focus(&self.focus)
            .when_some(input, |element, pane| {
                element
                    .role(gpui::Role::Terminal)
                    .aria_label("Terminal input")
                    .a11y_synthetic_children(move |builder| {
                        accessibility::terminal(builder, &accessible_grid.borrow(), pane, display_scale, embedded);
                    })
            })
            .on_key_down(cx.listener(Self::key_down))
            .on_drop(cx.listener(Self::drop_files))
            .on_mouse_move(cx.listener(Self::hover_mouse))
            .on_scroll_wheel(cx.listener(|view, event: &gpui::ScrollWheelEvent, window, cx| {
                let previous_gesture = view.wheel_gesture.id();
                let gesture = matches!(event.delta, gpui::ScrollDelta::Pixels(_))
                    .then(|| view.wheel_gesture.update(event.touch_phase, std::time::Instant::now()));
                if gesture.is_some_and(|id| id != previous_gesture) { view.wheel = None; }
                let grid = view.grid.borrow();
                let Some(pane) = grid.bounds.and_then(|bounds| {
                    hit_pane(
                        &grid.layout,
                        &grid.screens,
                        grid.cell,
                        bounds,
                        event.position,
                    )
                }) else {
                    view.wheel = None;
                    return;
                };
                if gesture.is_some() && !view.wheel_gesture.accepts(pane) {
                    cx.stop_propagation();
                    return;
                }
                let Some(mut pointer) = pointer_at(&grid, pane, event.position, window.scale_factor(), event.modifiers) else { return; };
                pointer.gesture = gesture;
                if gesture.is_some() {
                    pointer.shift = view.wheel_gesture.keep_local(pointer.shift || !grid.screens[&pane].mouse_input());
                }
                let delta = match event.delta {
                    gpui::ScrollDelta::Pixels(delta) => {
                        f32::from(delta.y) / f32::from(grid.cell.height).max(1.0)
                    }
                    gpui::ScrollDelta::Lines(delta) => delta.y,
                };
                drop(grid);
                view.clear_selection(cx);
                let previous = view
                    .wheel
                    .filter(|(id, _)| *id == pane)
                    .map_or(0.0, |(_, rows)| rows);
                let accumulated = previous - delta;
                let rows = accumulated.trunc() as isize;
                view.wheel = Some((pane, accumulated - rows as f32));
                if (rows != 0 || pointer.gesture.is_some())
                    && view
                        .input_queue
                        .try_send((pane, Input::Wheel(pointer, rows)))
                        .is_err()
                {
                    view.input_rejection = Some("Scroll was not applied: terminal queue is busy or disconnected. Click the terminal to dismiss.".into());
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .capture_any_mouse_down(|_, window, _| window.prevent_default())
            .on_mouse_down(gpui::MouseButton::Right, cx.listener(Self::secondary_down))
            .on_mouse_down(gpui::MouseButton::Middle, cx.listener(Self::secondary_down))
            .on_mouse_up(gpui::MouseButton::Right, cx.listener(Self::secondary_up))
            .on_mouse_up_out(gpui::MouseButton::Right, cx.listener(Self::secondary_up))
            .on_mouse_up(gpui::MouseButton::Middle, cx.listener(Self::secondary_up))
            .on_mouse_up_out(gpui::MouseButton::Middle, cx.listener(Self::secondary_up))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(Self::selection_up))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(Self::selection_up))
            .on_mouse_down_out(cx.listener(|view, event: &gpui::MouseDownEvent, _, cx| { if event.button == gpui::MouseButton::Left { view.clear_selection(cx); } }))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    // track_focus otherwise focuses the entire div, including
                    // blank padding and separators. Only a ready cell can arm input.
                    window.prevent_default();
                    let mut grid = view.grid.borrow_mut();
                    let hit = grid.bounds.and_then(|bounds| {
                        hit_pane(
                            &grid.layout,
                            &grid.screens,
                            grid.cell,
                            bounds,
                            event.position,
                        )
                    });
                    if view.program_press.is_some() { return; }
                    let mut program_pointer = None;
                    view.input = hit.filter(|_| !view.panes().is_finished());
                    if let Some(pane) = hit {
                        let (row, cell) = selection_point(&grid, pane, event.position, false).unwrap();
                        let keep = event.click_count == 1 && grid.selection.as_ref().is_some_and(|selection| selection.pane == pane && selection.contains(row, cell));
                        if grid.screens[&pane].mouse_input() && !event.modifiers.shift && !keep {
                            grid.selection = None;
                            program_pointer = pointer_at(&grid, pane, event.position, window.scale_factor(), event.modifiers);
                        } else if keep {
                            grid.selection.as_mut().unwrap().dragging = false;
                        } else {
                            let (row, col) = selection_point(&grid, pane, event.position, event.click_count < 2).unwrap();
                            let selection = grid.selection.as_ref()
                                .filter(|selection| event.click_count >= 2 && selection.pane == pane && !selection.is_empty())
                                .map(|selection| selection.reselect(row, col, event.click_count))
                                .unwrap_or_else(|| Selection::new(pane, grid.screens[&pane].clone(), row, col, event.click_count));
                            grid.selection = Some(selection);
                        }
                        view.input_rejection = None;
                        window.focus(&view.focus, cx);
                    } else {
                        grid.selection = None;
                        window.blur(cx);
                    }
                    drop(grid);
                    if let (Some(pane), Some(pointer)) = (hit, program_pointer) {
                        view.mouse_down(pane, Button::Left, pointer, cx);
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        let mut grid = grid.borrow_mut();
                        if embedded {
                            // Size the session to this leaf as a terminal of
                            // these bounds would, then scale only until tmux
                            // reports the new layout.
                            let size = grid.cells_in(bounds);
                            if grid.requested != Some(size)
                                && let Some(panes) = &panes
                                && panes.set_size(size.0, size.1).is_ok()
                            {
                                grid.requested = Some(size);
                                grid.contributes_size = true;
                            }
                            grid.fit(bounds);
                        }
                        grid.bounds = Some(bounds);
                        grid.paint(bounds, window, cx);
                        // The element's hovered-only move handler loses drags
                        // outside its bounds. Register during paint for the
                        // whole window and clamp to the selected pane instead.
                        let listener = Rc::clone(&selection_move);
                        window.on_mouse_event(move |event: &gpui::MouseMoveEvent, phase, window, cx| {
                            if phase == gpui::DispatchPhase::Capture { listener(event, window, cx); }
                        });
                    },
                )
                .size_full(),
            )
            .when_some(message, |el, message| {
                el.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .bg(gpui::rgb(0x402020))
                        .text_color(gpui::rgb(0xffffff))
                        .text_sm()
                        .child(message),
                )
            })
    }
}

pub(crate) struct InputQueue {
    sender: Option<std::sync::mpsc::Sender<(PaneId, Input, Arc<InputReservation>)>>,
    drained: Option<std::sync::mpsc::Receiver<()>>,
    budget: Arc<InputBudget>,
}

// Allow two maximum-size pastes per view, including the operation in flight.
const INPUT_BYTES: usize = 2 * crate::panes::INPUT_LIMIT;

#[derive(Default)]
struct InputBudget {
    count: std::sync::atomic::AtomicUsize,
    bytes: std::sync::atomic::AtomicUsize,
}

pub(crate) struct InputReservation {
    budget: Arc<InputBudget>,
    bytes: usize,
}

impl Drop for InputReservation {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.budget.bytes.fetch_sub(self.bytes, Ordering::AcqRel);
        self.budget.count.fetch_sub(1, Ordering::AcqRel);
    }
}

impl InputQueue {
    fn close(&mut self) -> Option<std::sync::mpsc::Receiver<()>> {
        self.sender.take();
        self.drained.take()
    }

    pub(crate) fn try_send(&self, event: (PaneId, Input)) -> Result<(), ()> {
        use std::sync::atomic::Ordering;
        let sender = self.sender.as_ref().ok_or(())?;
        // A view owns at most one outstanding press. Its release must fit even
        // when ordinary input has filled all 64 slots.
        let release = matches!(event.1, Input::Mouse(_, MouseAction::Release(_)));
        let limit = if release { 65 } else { 64 };
        self.budget
            .count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < limit).then_some(count + 1)
            })
            .map_err(|_| ())?;
        let bytes = event.1.size();
        let byte_limit = INPUT_BYTES + if release { 64 } else { 0 };
        if self
            .budget
            .bytes
            .try_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|total| *total <= byte_limit)
            })
            .is_err()
        {
            self.budget.count.fetch_sub(1, Ordering::AcqRel);
            return Err(());
        }
        let reservation = Arc::new(InputReservation {
            budget: Arc::clone(&self.budget),
            bytes,
        });
        sender.send((event.0, event.1, reservation)).map_err(|_| ())
    }
}

pub(crate) fn input_queue(
    panes: Arc<Panes>,
    report: impl Fn(Option<String>) + Send + 'static,
) -> InputQueue {
    let (sender, pending) = std::sync::mpsc::channel::<(PaneId, Input, Arc<InputReservation>)>();
    let (finished, drained) = std::sync::mpsc::channel();
    let budget = Arc::new(InputBudget::default());
    std::thread::spawn(move || {
        while let Ok((pane, input, reservation)) = pending.recv() {
            // A retired pane is a local rejection, not a failed connection.
            report(
                panes
                    .send_reserved_input(pane, input, Some(Arc::clone(&reservation)))
                    .err()
                    .map(|e| e.to_string()),
            );
        }
        let _ = finished.send(());
    });
    InputQueue {
        sender: Some(sender),
        drained: Some(drained),
        budget,
    }
}

fn ends_mouse_gesture(pressed: Button, released: Button, macos: bool) -> bool {
    // macOS GPUI independently remaps Control-left down/up and erases Control.
    // It cannot distinguish that release from a physical right-button release.
    // End either left/right gesture with its original encoded button; simultaneous
    // left/right chords are unsupported on this backend.
    pressed == released || (macos && pressed != Button::Middle && released != Button::Middle)
}

fn mouse_button(button: gpui::MouseButton) -> Option<Button> {
    match button {
        gpui::MouseButton::Left => Some(Button::Left),
        gpui::MouseButton::Middle => Some(Button::Middle),
        gpui::MouseButton::Right => Some(Button::Right),
        _ => None,
    }
}

fn drag_pointer(
    grid: &Grid,
    id: PaneId,
    position: gpui::Point<Pixels>,
    scale: f32,
    mods: gpui::Modifiers,
) -> Option<Pointer> {
    let pane = grid
        .layout
        .iter()
        .find(|pane| pane.id == id && pane.cols > 0 && pane.rows > 0)?;
    let bounds = grid.bounds?;
    let left = bounds.origin.x + grid.cell.width * f32::from(pane.left);
    let top = bounds.origin.y + grid.cell.height * f32::from(pane.top);
    let position = point(
        position.x.clamp(
            left,
            left + grid.cell.width * (f32::from(pane.cols) - 0.001),
        ),
        position
            .y
            .clamp(top, top + grid.cell.height * (f32::from(pane.rows) - 0.001)),
    );
    pointer_at(grid, id, position, scale, mods)
}

fn hit_pane(
    layout: &[PaneLayout],
    screens: &HashMap<PaneId, Screen>,
    cell: Size<Pixels>,
    bounds: Bounds<Pixels>,
    position: gpui::Point<Pixels>,
) -> Option<PaneId> {
    if !bounds.contains(&position) {
        return None;
    }
    let position = position - bounds.origin;
    let col = f32::from(position.x) / f32::from(cell.width);
    let row = f32::from(position.y) / f32::from(cell.height);
    layout
        .iter()
        .find(|pane| {
            screens.contains_key(&pane.id)
                && col >= f32::from(pane.left)
                && col < f32::from(pane.left) + f32::from(pane.cols)
                && row >= f32::from(pane.top)
                && row < f32::from(pane.top) + f32::from(pane.rows)
        })
        .map(|pane| pane.id)
}

fn pointer_at(
    grid: &Grid,
    id: PaneId,
    position: gpui::Point<Pixels>,
    scale: f32,
    mods: gpui::Modifiers,
) -> Option<Pointer> {
    let bounds = grid.bounds?;
    if hit_pane(&grid.layout, &grid.screens, grid.cell, bounds, position) != Some(id) {
        return None;
    }
    let pane = grid.layout.iter().find(|pane| pane.id == id)?;
    let position = position - bounds.origin;
    Some(Pointer {
        x: f32::from(position.x) / f32::from(grid.cell.width) - f32::from(pane.left),
        y: f32::from(position.y) / f32::from(grid.cell.height) - f32::from(pane.top),
        cols: pane.cols,
        rows: pane.rows,
        cell_width: (f32::from(grid.cell.width) * scale).round().max(1.0) as u32,
        cell_height: (f32::from(grid.cell.height) * scale).round().max(1.0) as u32,
        shift: mods.shift,
        control: mods.control,
        alt: mods.alt,
        gesture: None,
    })
}

fn selection_point(
    grid: &Grid,
    id: PaneId,
    position: gpui::Point<Pixels>,
    boundary: bool,
) -> Option<(u16, u16)> {
    let pane = grid
        .layout
        .iter()
        .find(|pane| pane.id == id && pane.cols > 0 && pane.rows > 0)?;
    let position = position - grid.bounds?.origin;
    let x = f32::from(position.x) / f32::from(grid.cell.width) - f32::from(pane.left);
    let y = f32::from(position.y) / f32::from(grid.cell.height) - f32::from(pane.top);
    let row = y.floor().clamp(0.0, f32::from(pane.rows - 1)) as u16;
    let col = selection_column(x, pane.cols, boundary);
    Some((row, col))
}

fn selection_column(x: f32, cols: u16, boundary: bool) -> u16 {
    (x + if boundary { 0.5 } else { 0.0 }).floor().clamp(
        0.0,
        f32::from(if boundary {
            cols
        } else {
            cols.saturating_sub(1)
        }),
    ) as u16
}

impl Grid {
    /// The whole cells of the unscaled font that fit in `bounds`.
    fn cells_in(&self, bounds: Bounds<Pixels>) -> (u16, u16) {
        let count = |length: Pixels, cell: Pixels| {
            (f32::from(length) / f32::from(cell))
                .floor()
                .clamp(1.0, f32::from(u16::MAX)) as u16
        };
        (
            count(bounds.size.width, self.original_cell.width),
            count(bounds.size.height, self.original_cell.height),
        )
    }

    fn fit(&mut self, bounds: Bounds<Pixels>) {
        let cols = self
            .layout
            .iter()
            .map(|p| u32::from(p.left) + u32::from(p.cols))
            .max()
            .unwrap_or(1)
            .max(1);
        let rows = self
            .layout
            .iter()
            .map(|p| u32::from(p.top) + u32::from(p.rows))
            .max()
            .unwrap_or(1)
            .max(1);
        let scale = (f32::from(bounds.size.width)
            / (f32::from(self.original_cell.width) * cols as f32))
            .min(
                f32::from(bounds.size.height)
                    / (f32::from(self.original_cell.height) * rows as f32),
            )
            .clamp(0.01, 1.0);
        self.cell = size(
            self.original_cell.width * scale,
            self.original_cell.height * scale,
        );
        self.appearance.font_size = self.original_font_size * scale;
    }

    fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        for pane in &self.layout {
            let selection = self
                .selection
                .as_ref()
                .filter(|selection| selection.pane == pane.id && !selection.is_empty());
            let Some(screen) = selection
                .map(|selection| &selection.screen)
                .or_else(|| self.screens.get(&pane.id))
            else {
                continue;
            };
            let origin = bounds.origin
                + point(
                    self.cell.width * f32::from(pane.left),
                    self.cell.height * f32::from(pane.top),
                );
            for row in 0..screen.rows() {
                let y = origin.y + self.cell.height * f32::from(row);
                let selected = selection.map_or(0..0, |selection| selection.row_range(row));
                self.paint_backgrounds(screen, row, origin.x, y, selected.clone(), window);
                self.paint_text(screen, row, origin.x, y, selected, window, cx);
            }
        }
    }

    /// Fills each run of cells whose background is not the default.
    fn paint_backgrounds(
        &self,
        screen: &Screen,
        row: u16,
        x: Pixels,
        y: Pixels,
        selected: std::ops::Range<u16>,
        window: &mut Window,
    ) {
        let cursor = screen.cursor();
        // A block cursor on a wide character covers both of its columns.
        let cursor_cols = match screen.cell(cursor.row, cursor.col) {
            Some((_, cell)) if cell.width == Width::Wide => 2,
            _ => 1,
        };
        let mut run: Option<(u16, Hsla)> = None;
        for col in 0..=screen.cols() {
            let background = screen.cell(row, col).and_then(|(_, cell)| {
                let is_cursor = cursor.visible
                    && cursor.row == row
                    && (cursor.col..cursor.col + cursor_cols).contains(&col);
                if selected.contains(&col) {
                    Some(gpui::rgb(0x345b89).into())
                } else if is_cursor {
                    Some(self.appearance.cursor.into())
                } else {
                    self.colors(cell.style).1
                }
            });
            if run.map(|(_, color)| Some(color)) == Some(background) {
                continue;
            }
            if let Some((start, color)) = run.take() {
                let left = x + self.cell.width * f32::from(start);
                let width = self.cell.width * f32::from(col - start);
                window.paint_quad(fill(
                    Bounds::new(point(left, y), size(width, self.cell.height)),
                    color,
                ));
            }
            run = background.map(|color| (col, color));
        }
    }

    /// Shapes and paints the row's text at the cells' own columns.
    #[allow(clippy::too_many_arguments)]
    fn paint_text(
        &self,
        screen: &Screen,
        row: u16,
        x: Pixels,
        y: Pixels,
        selected: std::ops::Range<u16>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let cursor = screen.cursor();
        let mut run = String::new();
        let mut run_start = 0;
        let mut run_style: Option<TextRun> = None;
        for col in 0..screen.cols() {
            let Some((text, cell)) = screen.cell(row, col) else {
                break;
            };
            if matches!(cell.width, Width::Spacer | Width::WrapSpacer) {
                continue;
            }
            let is_cursor = cursor.visible && cursor.row == row && cursor.col == col;
            let mut style = self.text_run(cell.style);
            if selected.contains(&col) {
                style.color = gpui::rgb(0xffffff).into();
            } else if is_cursor {
                style.color = self.appearance.background.into();
            }
            let text = if text.is_empty() { " " } else { text };
            let plain = self.batch_ascii
                && cell.width == Width::Narrow
                && matches!(text.as_bytes(), [byte] if byte.is_ascii_graphic() || *byte == b' ');
            if plain && !is_cursor && run_style.as_ref() == Some(&style) {
                run.push_str(text);
                continue;
            }
            self.paint_run(&mut run, run_start, run_style.take(), x, y, window, cx);
            if plain && !is_cursor {
                run.push_str(text);
                run_start = col;
                run_style = Some(style);
            } else {
                let mut single = text.to_owned();
                self.paint_run(&mut single, col, Some(style), x, y, window, cx);
            }
        }
        self.paint_run(&mut run, run_start, run_style, x, y, window, cx);
    }

    /// Paints `text`, which starts at column `col`, and empties it.
    #[allow(clippy::too_many_arguments)]
    fn paint_run(
        &self,
        text: &mut String,
        col: u16,
        style: Option<TextRun>,
        x: Pixels,
        y: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(mut style) = style else {
            text.clear();
            return;
        };
        // Blanks without decorations draw nothing.
        if style.underline.is_none() && style.strikethrough.is_none() {
            text.truncate(text.trim_end_matches(' ').len());
        }
        if text.is_empty() || style.color.a == 0.0 {
            text.clear();
            return;
        }
        style.len = text.len();
        // Every character advances exactly one cell, whatever the font's own
        // advances are.
        let line = window.text_system().shape_line(
            SharedString::from(std::mem::take(text)),
            px(self.appearance.font_size),
            &[style],
            Some(self.cell.width),
        );
        let origin = point(x + self.cell.width * f32::from(col), y);
        let _ = line.paint(origin, self.cell.height, TextAlign::Left, None, window, cx);
    }

    /// Returns the text style of a cell, with its length left at zero.
    fn text_run(&self, style: Style) -> TextRun {
        let mut font = self.font.clone();
        if style.attributes.contains(Attributes::BOLD) {
            font.weight = FontWeight::BOLD;
        }
        if style.attributes.contains(Attributes::ITALIC) {
            font.style = FontStyle::Italic;
        }
        let (mut color, _) = self.colors(style);
        if style.attributes.contains(Attributes::INVISIBLE) {
            color.a = 0.0;
        } else if style.attributes.contains(Attributes::FAINT) {
            color.a *= 0.5;
        }
        let underline = (style.underline != Underline::None).then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(color),
            wavy: style.underline == Underline::Curly,
        });
        let strikethrough = style
            .attributes
            .contains(Attributes::STRIKETHROUGH)
            .then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            });
        TextRun {
            len: 0,
            font,
            color,
            background_color: None,
            underline,
            strikethrough,
        }
    }

    /// Returns a cell's text colour and, unless it is the default, its
    /// background colour.
    fn colors(&self, style: Style) -> (Hsla, Option<Hsla>) {
        let appearance = &self.appearance;
        let foreground = self
            .color(style.foreground)
            .unwrap_or(appearance.foreground);
        let background = self.color(style.background);
        if style.attributes.contains(Attributes::INVERSE) {
            let text = background.unwrap_or(appearance.background);
            (text.into(), Some(foreground.into()))
        } else {
            (foreground.into(), background.map(Into::into))
        }
    }

    /// Returns a colour, or `None` for the default colour.
    fn color(&self, color: Color) -> Option<Rgba> {
        match color {
            Color::Default => None,
            Color::Palette(index) => Some(palette(&self.appearance.palette, index)),
            Color::Rgb(r, g, b) => Some(rgb(r, g, b)),
        }
    }
}

/// Returns entry `index` of the 256-colour palette.
fn palette(base: &[Rgba; 16], index: u8) -> Rgba {
    match index {
        0..16 => base[usize::from(index)],
        16..232 => {
            let level = |value: u8| if value == 0 { 0 } else { 55 + 40 * value };
            let cube = index - 16;
            rgb(level(cube / 36), level(cube / 6 % 6), level(cube % 6))
        }
        232.. => {
            let grey = 8 + 10 * (index - 232);
            rgb(grey, grey, grey)
        }
    }
}

fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    Rgba {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: 1.0,
    }
}

/// Translate platform input without exposing GPUI types to the terminal worker.
fn input_key(key: &Keystroke, repeat: bool) -> Option<Input> {
    if key.modifiers.platform {
        return None;
    }
    Some(Input::Key(KeyPress {
        key: key.key.clone(),
        text: key.key_char.clone(),
        shift: key.modifiers.shift,
        control: key.modifiers.control,
        alt: key.modifiers.alt,
        repeat,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_input_queue_delivers_mouse_release_before_shutdown() {
        use std::time::{Duration, Instant};
        use tmux_workspaces_test_support::TmuxServer;
        let server = TmuxServer::start().unwrap();
        let session = TmuxServer::SESSION;
        server.run(&["respawn-pane", "-k", "-t", session, "sh", "-c",
            "stty raw -echo; printf '\\033[?1000h\\033[?1006hready'; mouse=$(dd bs=1 count=18 2>/dev/null | od -An -tx1 | tr -d '[:space:]'); printf '\\r\\nmouse:%s' \"$mouse\"; exec cat"
        ]).unwrap();
        let fields = server
            .run(&[
                "display-message",
                "-p",
                "-t",
                session,
                "#{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}",
            ])
            .unwrap();
        let layout =
            tmux_workspaces_tmux::layout::parse(format!("%layout-change {fields}").as_bytes())
                .unwrap();
        let client = server
            .control(&[
                "attach-session",
                "-f",
                "no-output,ignore-size,new-layouts",
                "-t",
                session,
            ])
            .unwrap();
        let panes = Arc::new(Panes::start(client, &layout, 100, || {}).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if panes.take_update().is_some_and(|update| {
                update
                    .screens
                    .iter()
                    .any(|(_, screen)| screen.mouse_input())
            }) {
                break;
            }
            assert!(Instant::now() < deadline, "mouse mode was not observed");
            std::thread::sleep(Duration::from_millis(5));
        }
        let pointer = Pointer {
            x: 2.5,
            y: 3.5,
            cols: layout.panes[0].cols,
            rows: layout.panes[0].rows,
            cell_width: 8,
            cell_height: 16,
            ..Default::default()
        };
        let mut queue = input_queue(Arc::clone(&panes), |_| {});
        for action in [
            MouseAction::Press(Button::Left),
            MouseAction::Release(Button::Left),
        ] {
            queue
                .try_send((layout.panes[0].id, Input::Mouse(pointer, action)))
                .unwrap();
        }
        queue
            .close()
            .unwrap()
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        panes.shutdown();
        loop {
            let captured = server.run(&["capture-pane", "-p", "-t", session]).unwrap();
            if captured.contains("mouse:1b5b3c303b333b344d1b5b3c303b333b346d") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "mouse release was lost: {captured}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn control_click_release_aliases_end_the_original_macos_gesture() {
        assert!(ends_mouse_gesture(Button::Left, Button::Right, true));
        assert!(ends_mouse_gesture(Button::Right, Button::Left, true));
        assert!(!ends_mouse_gesture(Button::Right, Button::Left, false));
        assert!(!ends_mouse_gesture(Button::Middle, Button::Left, true));
    }

    #[test]
    fn saturated_input_queue_reserves_the_drag_release_in_order() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut queue = InputQueue {
            sender: Some(sender),
            drained: None,
            budget: Arc::new(InputBudget::default()),
        };
        let pane = PaneId(1);
        let pointer = Pointer::default();
        queue
            .try_send((
                pane,
                Input::Mouse(pointer, MouseAction::Press(Button::Left)),
            ))
            .unwrap();
        for _ in 0..63 {
            queue.try_send((pane, Input::Paste("x".into()))).unwrap();
        }
        assert!(
            queue
                .try_send((
                    pane,
                    Input::Mouse(pointer, MouseAction::Move(Some(Button::Left)))
                ))
                .is_err()
        );
        queue
            .try_send((
                pane,
                Input::Mouse(pointer, MouseAction::Release(Button::Left)),
            ))
            .unwrap();
        queue.close();
        let events: Vec<_> = receiver.iter().collect();
        assert_eq!(events.len(), 65);
        assert!(matches!(
            events[0].1,
            Input::Mouse(_, MouseAction::Press(Button::Left))
        ));
        assert!(matches!(
            events[64].1,
            Input::Mouse(_, MouseAction::Release(Button::Left))
        ));
    }

    #[test]
    fn input_byte_budget_includes_inflight_work_and_rolls_back_failed_sends() {
        use std::sync::atomic::Ordering;
        let (sender, receiver) = std::sync::mpsc::channel();
        let queue = InputQueue {
            sender: Some(sender),
            drained: None,
            budget: Arc::new(InputBudget::default()),
        };
        let pane = PaneId(1);
        for _ in 0..2 {
            queue
                .try_send((pane, Input::Paste("x".repeat(crate::panes::INPUT_LIMIT))))
                .unwrap();
        }
        let inflight = receiver.recv().unwrap();
        assert!(queue.try_send((pane, Input::Paste("x".into()))).is_err());
        queue
            .try_send((
                pane,
                Input::Mouse(Pointer::default(), MouseAction::Release(Button::Left)),
            ))
            .unwrap();
        assert_eq!(queue.budget.count.load(Ordering::Acquire), 3);
        // An encoding request can outlive its caller's timeout. Its payload
        // must remain charged until that worker releases its shared reservation.
        let encoding_reservation = Arc::clone(&inflight.2);
        drop(inflight);
        assert!(queue.try_send((pane, Input::Paste("x".into()))).is_err());
        drop(encoding_reservation);
        queue.try_send((pane, Input::Paste("x".into()))).unwrap();
        drop(receiver);
        assert_eq!(queue.budget.bytes.load(Ordering::Acquire), 0);
        assert!(queue.try_send((pane, Input::Paste("x".into()))).is_err());
        assert_eq!(queue.budget.bytes.load(Ordering::Acquire), 0);
        assert_eq!(queue.budget.count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn selection_coordinates_distinguish_words_and_clamp_external_drags() {
        assert_eq!(selection_column(2.8, 7, false), 2);
        assert_eq!(selection_column(2.8, 7, true), 3);
        assert_eq!(selection_column(100.0, 7, true), 7);
        assert_eq!(selection_column(100.0, 7, false), 6);
        assert_eq!(selection_column(-10.0, 7, true), 0);
    }

    #[test]
    fn terminal_clicks_require_ready_cells_not_padding_or_separators() {
        let layout = [
            PaneLayout {
                id: PaneId(1),
                left: 0,
                top: 0,
                cols: 4,
                rows: 2,
            },
            PaneLayout {
                id: PaneId(2),
                left: 5,
                top: 0,
                cols: 4,
                rows: 2,
            },
        ];
        let bounds = Bounds::new(point(px(20.0), px(30.0)), size(px(100.0), px(100.0)));
        let cell = size(px(10.0), px(10.0));
        let mut screens = HashMap::new();
        assert_eq!(
            hit_pane(&layout, &screens, cell, bounds, point(px(21.0), px(31.0))),
            None
        );
        screens.insert(PaneId(1), Screen::default());
        screens.insert(PaneId(2), Screen::default());
        for (x, y, expected) in [
            (21.0, 31.0, Some(PaneId(1))),
            (71.0, 31.0, Some(PaneId(2))),
            (61.0, 31.0, None),
            (21.0, 51.0, None),
            (10.0, 31.0, None),
        ] {
            assert_eq!(
                hit_pane(&layout, &screens, cell, bounds, point(px(x), px(y))),
                expected
            );
        }
    }

    #[test]
    fn the_palette_has_the_standard_cube_and_grey_ramp() {
        let base = [rgb(1, 2, 3); 16];
        assert_eq!(palette(&base, 7), rgb(1, 2, 3));
        assert_eq!(palette(&base, 16), rgb(0, 0, 0));
        assert_eq!(palette(&base, 196), rgb(255, 0, 0));
        assert_eq!(palette(&base, 231), rgb(255, 255, 255));
        assert_eq!(palette(&base, 232), rgb(8, 8, 8));
        assert_eq!(palette(&base, 255), rgb(238, 238, 238));
    }

    #[test]
    fn keys_become_the_bytes_a_terminal_sends() {
        let key = |text: &str| Keystroke::parse(text).unwrap();
        let typed = |text: &str, char: &str| Keystroke {
            key_char: Some(char.to_owned()),
            ..key(text)
        };
        let mut terminal = tmux_workspaces_term::Terminal::new(80, 24, 0).unwrap();
        let mut encode_key = |key: &Keystroke| {
            input_key(key, false)
                .map(|input| terminal.encode_input(&input).unwrap())
                .unwrap_or_default()
        };
        assert_eq!(encode_key(&typed("a", "a")), b"a");
        assert_eq!(encode_key(&typed("shift-a", "A")), b"A");
        assert_eq!(encode_key(&key("enter")), b"\r");
        assert_eq!(encode_key(&key("ctrl-c")), b"\x03");
        assert_eq!(encode_key(&key("cmd-q")), b"");
    }
}
