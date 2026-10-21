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

mod controls;
mod del;
pub mod input;
mod rename;
pub mod restore;
mod screen;

use libghostty_vt::build_info;
use libghostty_vt::render::{CellIterator, RenderState, RowIterator};
use libghostty_vt::screen::{CellContentTag, CellWide};
use libghostty_vt::style::{self as engine_style, StyleColor};
use libghostty_vt::terminal::{Mode, Options, Point, PointCoordinate, Terminal as Engine};

pub use screen::{Attributes, Cell, Color, Cursor, Screen, Style, Underline, Width};

/// An error reported by the terminal engine.
#[derive(Debug, thiserror::Error)]
#[error("terminal engine error: {0}")]
pub struct Error(libghostty_vt::Error);

/// What a cell holds besides what a [`Screen`] copies, for comparing
/// terminals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CellDetails {
    /// The underline colour.
    pub underline_color: Color,
    /// The target of the hyperlink the cell belongs to, if any.
    pub link: Option<String>,
}

/// The terminal state of one pane.
pub struct Terminal {
    engine: Engine<'static, 'static>,
    render: RenderState<'static>,
    rows: RowIterator<'static>,
    cells: CellIterator<'static>,
    /// The text of the cell being copied: the engine overwrites the buffer it
    /// is given.
    grapheme: String,
    del: del::Del,
    extended_keys_csi_u: bool,
    rename: rename::Rename,
}

impl Terminal {
    /// Creates a terminal of `cols` by `rows` cells that keeps about
    /// `scrollback` lines of history.
    ///
    /// The engine limits history by memory, not lines. The terminal gives it
    /// room for `scrollback` lines of plain text with headroom, so it usually
    /// keeps more, and output with very many styles on every line can keep
    /// fewer. [`Terminal::history`] says how many lines it holds.
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Result<Self, Error> {
        let mut engine = Engine::new(Options {
            cols,
            rows,
            max_scrollback: history_bytes(cols, scrollback),
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
            del: del::Del::default(),
            extended_keys_csi_u: false,
            rename: rename::Rename::default(),
        })
    }

    /// Feeds pane output to the terminal, in the order the pane produced it.
    ///
    /// A window-rename sequence, `ESC k name ESC \`, is dropped as tmux
    /// drops it, also when it arrives split across writes, and so are the
    /// control bytes tmux ignores.
    pub fn write(&mut self, bytes: &[u8]) {
        let engine = &mut self.engine;
        let rename = &mut self.rename;
        self.del.filter(bytes, |part| {
            rename.filter(part, |part| {
                controls::filter(part, |part| engine.vt_write(part));
            });
        });
    }

    /// Returns the number of lines of history the terminal holds.
    pub fn history(&self) -> Result<usize, Error> {
        self.engine.scrollback_rows().map_err(Error)
    }

    /// Returns the cursor's column and row, counted from zero at the top left
    /// of the screen.
    pub fn cursor(&self) -> Result<(u16, u16), Error> {
        let column = self.engine.cursor_x().map_err(Error)?;
        let row = self.engine.cursor_y().map_err(Error)?;
        Ok((column, row))
    }

    /// Whether the engine has reached its right margin. It can set this
    /// flag even with wrapping disabled, so it does not by itself establish
    /// tmux's logical cursor column.
    pub fn cursor_pending_wrap(&self) -> Result<bool, Error> {
        self.engine.is_cursor_pending_wrap().map_err(Error)
    }

    /// Whether the pane has enabled its cursor, independently of whether
    /// that cursor is currently inside the displayed grid.
    pub fn cursor_enabled(&self) -> Result<bool, Error> {
        self.engine.is_cursor_visible().map_err(Error)
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

    /// Returns the underline colour and hyperlink of the cell at `row` and
    /// `col` of the screen, counted from zero at the top left.
    ///
    /// This reads one cell at a time, too slowly for drawing; it is for
    /// comparing terminals.
    pub fn cell_details(&self, row: u16, col: u16) -> Result<CellDetails, Error> {
        let cell = self
            .engine
            .grid_ref(Point::Active(PointCoordinate {
                x: col,
                y: u32::from(row),
            }))
            .map_err(Error)?;
        let underline_color = if cell.cell().map_err(Error)?.has_styling().map_err(Error)? {
            color_from_engine(cell.style().map_err(Error)?.underline_color)
        } else {
            Color::Default
        };
        let link = match cell.hyperlink_uri(&mut []) {
            Ok(_) => None,
            Err(libghostty_vt::Error::OutOfSpace { required }) => {
                let mut uri = vec![0; required];
                let len = cell.hyperlink_uri(&mut uri).map_err(Error)?;
                uri.truncate(len);
                Some(String::from_utf8_lossy(&uri).into_owned())
            }
            Err(error) => return Err(Error(error)),
        };
        Ok(CellDetails {
            underline_color,
            link,
        })
    }
}

/// Returns the engine's history limit, in bytes, for about `lines` lines of
/// history at `cols` columns.
///
/// The engine keeps history in pages of 8-byte cells and frees whole pages
/// when the limit is reached. The limit is room for one and a half times
/// `lines` rows at the wider of `cols` and 215 columns, the engine's standard
/// page width. A page grows when it holds many styles, so styled output keeps
/// fewer lines in the same room: at 200 columns, the benchmark's most heavily
/// styled payload still keeps 10,000 lines (`docs/benchmarks/gate-1.md`).
fn history_bytes(cols: u16, lines: usize) -> usize {
    const CELL_BYTES: usize = 8;
    const PAGE_COLS: usize = 215;
    let row_bytes = CELL_BYTES * usize::from(cols).max(PAGE_COLS);
    lines.saturating_mul(row_bytes).saturating_mul(3) / 2
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
    fn keeps_about_the_history_asked_for() {
        for cols in [80, 200, 300] {
            let mut terminal = Terminal::new(cols, 24, 1_000).unwrap();
            for line in 0..5_000 {
                terminal.write(format!("line {line}\r\n").as_bytes());
            }
            let history = terminal.history().unwrap();
            // Plain lines fit at least as many as asked for, and the limit
            // holds: an unlimited history would keep 4,977.
            assert!(
                (1_000..4_000).contains(&history),
                "{cols} columns: {history} lines"
            );
        }
        let mut terminal = Terminal::new(80, 24, 0).unwrap();
        terminal.write("line\r\n".repeat(100).as_bytes());
        assert_eq!(terminal.history().unwrap(), 0);
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
