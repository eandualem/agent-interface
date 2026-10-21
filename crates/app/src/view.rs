//! The terminal grid: draws the panes' owned display data with GPUI
//! (`docs/decisions/0005-ui-framework-and-terminal-renderer.md`).
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
use tmux_workspaces_term::{Attributes, Color, Screen, Style, Underline, Width};
use tmux_workspaces_tmux::control::PaneId;

use crate::panes::{PaneLayout, Panes};

/// The font and colours of the terminal grid.
#[derive(Clone, Debug)]
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
    // Input goes to this exact pane, changed only by an explicit visible-pane click.
    input: PaneId,
    focus: FocusHandle,
    failure_reported: bool,
    embedded: bool,
    input_queue: std::sync::mpsc::SyncSender<(PaneId, Vec<u8>)>,
    input_error: Option<String>,
    closed_reported: bool,
}

pub(crate) struct Disconnected;
impl gpui::EventEmitter<Disconnected> for TerminalView {}

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
            let panes = view.panes.take();
            cx.background_executor()
                .spawn(async move { drop(panes) })
                .detach();
        })
        .detach();
        let (input_queue, pending) = std::sync::mpsc::sync_channel::<(PaneId, Vec<u8>)>(64);
        let writing = Arc::clone(&panes);
        // Keep both pipe writes and the final connection join off the UI thread.
        // Dropping the view closes this bounded queue; no hidden view keeps focus.
        std::thread::spawn(move || {
            while let Ok((pane, bytes)) = pending.recv() {
                if writing.send_input(pane, &bytes).is_err() {
                    break;
                }
            }
        });
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
            })),
            input,
            focus,
            failure_reported: false,
            embedded,
            input_queue,
            input_error: None,
            closed_reported: false,
        }
    }

    fn panes(&self) -> &Panes {
        self.panes.as_deref().expect("terminal entity is alive")
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
        let bytes = encode_key(&event.keystroke);
        if !bytes.is_empty()
            && self
                .grid
                .borrow()
                .layout
                .iter()
                .any(|pane| pane.id == self.input)
            && self.grid.borrow().screens.contains_key(&self.input)
            && !self.panes().is_finished()
        {
            // Queue input without waiting for a pipe write on the UI thread.
            self.input_error = self
                .input_queue
                .try_send((self.input, bytes))
                .err()
                .map(|_| "Terminal input is busy or disconnected; key was not sent.".into());
            if self.input_error.is_some() {
                cx.notify();
            }
        }
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.take_update();
        if self.panes().is_finished() {
            self.report_failure();
            if self.embedded && !self.closed_reported {
                self.closed_reported = true;
                cx.emit(Disconnected);
            }
            // The client has gone: close once this frame, which shows its last
            // output, has been drawn.
            if !self.embedded {
                window.on_next_frame(|_, cx| cx.quit());
            }
        }
        let grid = Rc::clone(&self.grid);
        let background = self.grid.borrow().appearance.background;
        let embedded = self.embedded;
        let message = self.input_error.clone().or_else(|| {
            self.panes().is_finished().then(|| {
                self.panes()
                    .failure()
                    .unwrap_or_else(|| "Session disconnected.".into())
            })
        });
        div()
            .size_full()
            .overflow_hidden()
            .relative()
            .bg(background)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|view, event: &gpui::MouseDownEvent, window, cx| {
                    let grid = view.grid.borrow();
                    if let Some(bounds) = grid.bounds {
                        let position = event.position - bounds.origin;
                        let col = f32::from(position.x) / f32::from(grid.cell.width);
                        let row = f32::from(position.y) / f32::from(grid.cell.height);
                        if let Some(pane) = grid.layout.iter().find(|p| {
                            col >= f32::from(p.left)
                                && col < f32::from(p.left) + f32::from(p.cols)
                                && row >= f32::from(p.top)
                                && row < f32::from(p.top) + f32::from(p.rows)
                        }) {
                            view.input = pane.id;
                            window.focus(&view.focus, cx);
                        }
                    }
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        let mut grid = grid.borrow_mut();
                        if embedded {
                            grid.fit(bounds);
                        }
                        grid.bounds = Some(bounds);
                        grid.paint(bounds, window, cx);
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

impl Grid {
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
            let Some(screen) = self.screens.get(&pane.id) else {
                continue;
            };
            let origin = bounds.origin
                + point(
                    self.cell.width * f32::from(pane.left),
                    self.cell.height * f32::from(pane.top),
                );
            for row in 0..screen.rows() {
                let y = origin.y + self.cell.height * f32::from(row);
                self.paint_backgrounds(screen, row, origin.x, y, window);
                self.paint_text(screen, row, origin.x, y, window, cx);
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
                if is_cursor {
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
    fn paint_text(
        &self,
        screen: &Screen,
        row: u16,
        x: Pixels,
        y: Pixels,
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
            if is_cursor {
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

/// Returns the bytes a key sends to the pane, or none for keys the app keeps.
///
/// This covers ordinary text, control letters and basic navigation keys. Full
/// mode-aware input still requires the terminal engine's key encoder
/// (`docs/decisions/0015-input-routing.md`).
fn encode_key(keystroke: &Keystroke) -> Vec<u8> {
    let modifiers = &keystroke.modifiers;
    if modifiers.platform {
        return Vec::new();
    }
    let named: &[u8] = match keystroke.key.as_str() {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        _ => b"",
    };
    if !named.is_empty() {
        return named.to_vec();
    }
    if modifiers.control
        && let [letter] = keystroke.key.as_bytes()
        && letter.is_ascii_alphabetic()
    {
        return vec![letter.to_ascii_lowercase() & 0x1f];
    }
    keystroke
        .key_char
        .as_ref()
        .map(|text| text.as_bytes().to_vec())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(encode_key(&typed("a", "a")), b"a");
        assert_eq!(encode_key(&typed("shift-a", "A")), b"A");
        assert_eq!(encode_key(&key("enter")), b"\r");
        assert_eq!(encode_key(&key("ctrl-c")), b"\x03");
        assert_eq!(encode_key(&key("cmd-q")), b"");
    }
}
