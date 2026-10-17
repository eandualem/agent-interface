//! Terminal state for one tmux pane, kept by the terminal engine behind this
//! crate's own types.
//!
//! The engine is `libghostty-vt`
//! (`docs/decisions/0004-terminal-emulation-core.md`). No engine type appears
//! in this crate's public API, so the engine can be replaced without changing
//! other crates. This crate depends on no other project crate.
//!
//! Engine handles are not `Send`: construct, use and drop a [`Terminal`] on
//! the thread that owns it
//! (`docs/decisions/0007-threads-ownership-and-event-ordering.md`). The
//! display data it hands out, a [`Screen`], is owned and can go to any thread.

#![forbid(unsafe_code)]

mod screen;

use libghostty_vt::build_info;
use libghostty_vt::render::{CellIterator, RenderState, RowIterator};
use libghostty_vt::screen::{CellContentTag, CellWide};
use libghostty_vt::style::{self as engine_style, StyleColor};
use libghostty_vt::terminal::{Mode, Options, Terminal as Engine};

pub use screen::{Attributes, Cell, Color, Cursor, Screen, Style, Underline, Width};

/// An error reported by the terminal engine.
#[derive(Debug, thiserror::Error)]
#[error("terminal engine error: {0}")]
pub struct Error(libghostty_vt::Error);

/// The terminal state of one pane.
pub struct Terminal {
    engine: Engine<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    /// The text of the cell being copied: the engine overwrites the buffer it
    /// is given.
    grapheme: String,
}

impl Terminal {
    /// Creates a terminal of `cols` by `rows` cells that keeps up to
    /// `scrollback` lines of history.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Result<Self, Error> {
        let mut engine = Engine::new(Options {
            cols,
            rows,
            max_scrollback: scrollback,
        })
        .map_err(Error)?;
        // tmux treats a grapheme cluster, such as an emoji sequence, as one
        // character of one or two columns. Mode 2027 makes the engine do the
        // same, so both agree where text and the cursor are.
        engine
            .set_mode(Mode::GRAPHEME_CLUSTER, true)
            .map_err(Error)?;
        Ok(Self {
            engine,
            render: RenderState::new().map_err(Error)?,
            rows: RowIterator::new().map_err(Error)?,
            cells: CellIterator::new().map_err(Error)?,
            grapheme: String::new(),
        })
    }

    /// Feeds pane output to the terminal, in the order the pane produced it.
    pub fn write(&mut self, bytes: &[u8]) {
        self.engine.vt_write(bytes);
    }

    /// Returns the cursor's column and row, counted from zero at the top left
    /// of the screen.
    pub fn cursor(&self) -> Result<(u16, u16), Error> {
        let column = self.engine.cursor_x().map_err(Error)?;
        let row = self.engine.cursor_y().map_err(Error)?;
        Ok((column, row))
    }

    /// Copies the visible screen into `screen`, replacing what it held.
    ///
    /// This updates the engine's render state, then copies every cell, so the
    /// result does not depend on what earlier reads copied.
    pub fn read_screen(&mut self, screen: &mut Screen) -> Result<(), Error> {
        let snapshot = self.render.update(&self.engine).map_err(Error)?;
        screen.clear(
            snapshot.cols().map_err(Error)?,
            snapshot.rows().map_err(Error)?,
        );
        let mut rows = self.rows.update(&snapshot).map_err(Error)?;
        while let Some(row) = rows.next() {
            let mut cells = self.cells.update(row).map_err(Error)?;
            while let Some(cell) = cells.next() {
                let raw = cell.raw_cell().map_err(Error)?;
                let width = match raw.wide().map_err(Error)? {
                    CellWide::Narrow => Width::Narrow,
                    CellWide::Wide => Width::Wide,
                    CellWide::SpacerTail => Width::Spacer,
                    CellWide::SpacerHead => Width::WrapSpacer,
                };
                let mut style = if raw.has_styling().map_err(Error)? {
                    style_from_engine(cell.style().map_err(Error)?)
                } else {
                    Style::default()
                };
                // A cell erased with a background colour keeps the colour in
                // the cell rather than in its style.
                match raw.content_tag().map_err(Error)? {
                    CellContentTag::BgColorPalette => {
                        style.background = Color::Palette(raw.bg_color_palette().map_err(Error)?.0);
                    }
                    CellContentTag::BgColorRgb => {
                        let rgb = raw.bg_color_rgb().map_err(Error)?;
                        style.background = Color::Rgb(rgb.r, rgb.g, rgb.b);
                    }
                    CellContentTag::Codepoint | CellContentTag::CodepointGrapheme => {}
                }
                self.grapheme.clear();
                if raw.has_text().map_err(Error)? {
                    cell.graphemes_utf8(&mut self.grapheme).map_err(Error)?;
                }
                screen.push(&self.grapheme, width, style);
            }
        }
        let visible = snapshot.cursor_visible().map_err(Error)?;
        let cursor = match snapshot.cursor_viewport().map_err(Error)? {
            Some(position) => Cursor {
                col: position.x,
                row: position.y,
                visible,
            },
            None => Cursor::default(),
        };
        screen.set_cursor(cursor);
        Ok(())
    }
}

fn style_from_engine(style: engine_style::Style) -> Style {
    let mut attributes = Attributes::default();
    for (on, attribute) in [
        (style.bold, Attributes::BOLD),
        (style.faint, Attributes::FAINT),
        (style.italic, Attributes::ITALIC),
        (style.blink, Attributes::BLINK),
        (style.inverse, Attributes::INVERSE),
        (style.invisible, Attributes::INVISIBLE),
        (style.strikethrough, Attributes::STRIKETHROUGH),
        (style.overline, Attributes::OVERLINE),
    ] {
        if on {
            attributes.insert(attribute);
        }
    }
    Style {
        foreground: color_from_engine(style.fg_color),
        background: color_from_engine(style.bg_color),
        underline: match style.underline {
            engine_style::Underline::None => Underline::None,
            engine_style::Underline::Double => Underline::Double,
            engine_style::Underline::Curly => Underline::Curly,
            engine_style::Underline::Dotted => Underline::Dotted,
            engine_style::Underline::Dashed => Underline::Dashed,
            // Single, and any kind a later engine version adds.
            _ => Underline::Single,
        },
        attributes,
    }
}

fn color_from_engine(color: StyleColor) -> Color {
    match color {
        StyleColor::None => Color::Default,
        StyleColor::Palette(index) => Color::Palette(index.0),
        StyleColor::Rgb(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

/// How the terminal engine was built, for benchmark reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineBuild {
    /// The engine's version string.
    pub version: &'static str,
    /// The engine's optimisation mode, such as `ReleaseFast`.
    pub optimize: String,
    /// Whether the engine's SIMD code paths are enabled.
    pub simd: bool,
}

/// Returns how the terminal engine was built.
pub fn engine_build() -> Result<EngineBuild, Error> {
    Ok(EngineBuild {
        version: build_info::version_string().map_err(Error)?,
        optimize: format!("{:?}", build_info::optimize_mode().map_err(Error)?),
        simd: build_info::supports_simd().map_err(Error)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_text_moves_the_cursor() {
        let mut terminal = Terminal::new(80, 24, 100).unwrap();
        terminal.write(b"hello");
        assert_eq!(terminal.cursor().unwrap(), (5, 0));
        terminal.write(b"\r\nab");
        assert_eq!(terminal.cursor().unwrap(), (2, 1));
    }

    #[test]
    fn a_read_screen_holds_text_widths_styles_and_the_cursor() {
        let mut terminal = Terminal::new(6, 3, 0).unwrap();
        terminal.write("\x1b[1;31ma\x1b[0m界e\u{301}\r\n".as_bytes());
        terminal.write(b"\x1b[48;5;4m\x1b[K\x1b[0m\r\n\x1b[38;2;1;2;3;4mz");
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();

        assert_eq!((screen.cols(), screen.rows()), (6, 3));
        let (text, cell) = screen.cell(0, 0).unwrap();
        assert_eq!(text, "a");
        assert_eq!(cell.style.foreground, Color::Palette(1));
        assert!(cell.style.attributes.contains(Attributes::BOLD));
        assert_eq!(screen.cell(0, 1).unwrap().0, "界");
        assert_eq!(screen.cell(0, 1).unwrap().1.width, Width::Wide);
        let (text, cell) = screen.cell(0, 2).unwrap();
        assert_eq!((text, cell.width), ("", Width::Spacer));
        assert_eq!(screen.cell(0, 3).unwrap().0, "e\u{301}");
        assert_eq!(
            screen.cell(1, 5).unwrap().1.style.background,
            Color::Palette(4)
        );
        let (text, cell) = screen.cell(2, 0).unwrap();
        assert_eq!(text, "z");
        assert_eq!(cell.style.foreground, Color::Rgb(1, 2, 3));
        assert_eq!(cell.style.underline, Underline::Single);
        assert_eq!(
            screen.cursor(),
            Cursor {
                col: 1,
                row: 2,
                visible: true
            }
        );
        assert_eq!(screen.cell(3, 0), None);
    }

    #[test]
    fn an_emoji_sequence_is_one_cluster_as_tmux_draws_it() {
        let mut terminal = Terminal::new(10, 1, 0).unwrap();
        terminal.write("\u{1f469}\u{200d}\u{1f4bb}\u{26a0}\u{fe0f}".as_bytes());
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        assert_eq!(terminal.cursor().unwrap(), (4, 0));
        let (text, cell) = screen.cell(0, 0).unwrap();
        assert_eq!(
            (text, cell.width),
            ("\u{1f469}\u{200d}\u{1f4bb}", Width::Wide)
        );
        let (text, cell) = screen.cell(0, 2).unwrap();
        assert_eq!((text, cell.width), ("\u{26a0}\u{fe0f}", Width::Wide));
    }

    #[test]
    fn reading_again_replaces_the_screen() {
        let mut terminal = Terminal::new(4, 2, 0).unwrap();
        let mut screen = Screen::default();
        terminal.write(b"ab");
        terminal.read_screen(&mut screen).unwrap();
        terminal.write(b"\x1b[2J\x1b[Hc");
        terminal.read_screen(&mut screen).unwrap();

        let mut expected = Screen::default();
        expected.clear(4, 2);
        expected.push("c", Width::Narrow, Style::default());
        for _ in 1..8 {
            expected.push("", Width::Narrow, Style::default());
        }
        expected.set_cursor(Cursor {
            col: 1,
            row: 0,
            visible: true,
        });
        assert_eq!(screen, expected);
    }
}
