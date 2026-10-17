//! `alacritty_terminal`, the fallback engine of decision record 0004.
//!
//! `tmux-workspaces-bench-rio` includes this file for its tests.

use crate::display::{Attributes, Color, Cursor, Screen, Style, Underline, Width};
use crate::engine::Engine;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color as EngineColor, Processor};

/// `alacritty_terminal` with its own parser.
pub struct Alacritty {
    term: Term<VoidListener>,
    parser: Processor,
    /// The text of the cell being copied.
    grapheme: String,
}

/// The terminal size `Term` is created with.
struct Size {
    cols: usize,
    rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

impl Engine for Alacritty {
    const NAME: &'static str = "alacritty_terminal";

    fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let size = Size {
            cols: usize::from(cols),
            rows: usize::from(rows),
        };
        Self {
            term: Term::new(config, &size, VoidListener),
            parser: Processor::new(),
            grapheme: String::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn read_screen(&mut self, screen: &mut Screen) {
        let grid = self.term.grid();
        let cols = grid.columns();
        let rows = grid.screen_lines();
        screen.clear(cols as u16, rows as u16);
        // With no display offset, lines 0 to rows - 1 are the visible screen.
        for line in 0..rows {
            let row = &grid[Line(line as i32)];
            for col in 0..cols {
                let cell = &row[Column(col)];
                let width = if cell.flags.contains(Flags::WIDE_CHAR) {
                    Width::Wide
                } else if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    Width::Spacer
                } else if cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                    Width::WrapSpacer
                } else {
                    Width::Narrow
                };
                self.grapheme.clear();
                if matches!(width, Width::Narrow | Width::Wide) {
                    // The engine marks where a tab started with a tab
                    // character, which it draws as a blank.
                    self.grapheme
                        .push(if cell.c == '\t' { ' ' } else { cell.c });
                    if let Some(marks) = cell.zerowidth() {
                        self.grapheme.extend(marks);
                    }
                }
                screen.push(&self.grapheme, width, style(cell.fg, cell.bg, cell.flags));
            }
        }
        let point = grid.cursor.point;
        screen.set_cursor(Cursor {
            col: point.column.0 as u16,
            row: point.line.0 as u16,
            visible: self.term.mode().contains(TermMode::SHOW_CURSOR),
        });
    }
}

fn style(fg: EngineColor, bg: EngineColor, flags: Flags) -> Style {
    let mut attributes = Attributes::default();
    for (flag, attribute) in [
        (Flags::BOLD, Attributes::BOLD),
        (Flags::DIM, Attributes::FAINT),
        (Flags::ITALIC, Attributes::ITALIC),
        (Flags::INVERSE, Attributes::INVERSE),
        (Flags::HIDDEN, Attributes::INVISIBLE),
        (Flags::STRIKEOUT, Attributes::STRIKETHROUGH),
    ] {
        if flags.contains(flag) {
            attributes.insert(attribute);
        }
    }
    // alacritty_terminal stores no blink or overline attribute.
    let underline = if flags.contains(Flags::DOUBLE_UNDERLINE) {
        Underline::Double
    } else if flags.contains(Flags::UNDERCURL) {
        Underline::Curly
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        Underline::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        Underline::Dashed
    } else if flags.contains(Flags::UNDERLINE) {
        Underline::Single
    } else {
        Underline::None
    };
    Style {
        foreground: color(fg),
        background: color(bg),
        underline,
        attributes,
    }
}

/// Maps a colour as the program set it. The 16 named colours are palette
/// entries 0 to 15; every other named colour, such as the default foreground
/// and background or the dim and cursor colours, is the terminal's default.
fn color(color: EngineColor) -> Color {
    match color {
        EngineColor::Named(named) if (named as usize) < 16 => Color::Palette(named as u8),
        EngineColor::Named(_) => Color::Default,
        EngineColor::Indexed(index) => Color::Palette(index),
        EngineColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}
