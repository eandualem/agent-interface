//! `rio-vt`, the engine decision record 0004 measures against
//! `libghostty-vt`, built with its default features off.

use crate::display::{Attributes, Color, Cursor, Screen, Style, Underline, Width};
use crate::engine::Engine;
use rio_vt::ansi::CursorShape;
use rio_vt::config::colors::AnsiColor;
use rio_vt::crosswords::grid::Dimensions;
use rio_vt::crosswords::pos::{Column, Line, Pos};
use rio_vt::crosswords::square::Wide;
use rio_vt::crosswords::style::StyleFlags;
use rio_vt::crosswords::{Crosswords, CrosswordsSize, Mode};
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

/// `rio-vt` with its own parser.
pub struct Rio {
    term: Crosswords<VoidListener>,
    parser: Processor,
    /// The text of the cell being copied.
    grapheme: String,
}

impl Engine for Rio {
    const NAME: &'static str = "rio-vt";

    fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let size = CrosswordsSize::new(usize::from(cols), usize::from(rows));
        Self {
            term: Crosswords::new(
                size,
                CursorShape::Block,
                VoidListener,
                WindowId::from(0),
                0,
                scrollback,
            ),
            parser: Processor::default(),
            grapheme: String::new(),
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn read_screen(&mut self, screen: &mut Screen) {
        let grid = &self.term.grid;
        let cols = grid.columns();
        let rows = grid.screen_lines();
        screen.clear(cols as u16, rows as u16);
        // With no display offset, lines 0 to rows - 1 are the visible screen.
        for line in 0..rows {
            for col in 0..cols {
                let pos = Pos::new(Line(line as i32), Column(col));
                let square = grid[pos];
                let width = match square.wide() {
                    Wide::Narrow => Width::Narrow,
                    Wide::Wide => Width::Wide,
                    Wide::Spacer => Width::Spacer,
                    Wide::LeadingSpacer => Width::WrapSpacer,
                };
                self.grapheme.clear();
                // A blank cell, or one that holds only a background colour,
                // stores no character. The engine marks where a tab started
                // with a tab character, which it draws as a blank.
                match square.c() {
                    _ if !matches!(width, Width::Narrow | Width::Wide) => {}
                    '\0' => {}
                    '\t' => self.grapheme.push(' '),
                    _ => self.grapheme.extend(grid.cell_text(pos)),
                }
                // `style_of` reads a background-only cell's inline colour.
                screen.push(&self.grapheme, width, style(grid.style_of(&square)));
            }
        }
        let pos = grid.cursor.pos;
        screen.set_cursor(Cursor {
            col: pos.col.0 as u16,
            row: pos.row.0 as u16,
            visible: self.term.mode().contains(Mode::SHOW_CURSOR),
        });
    }

    fn history(&self) -> usize {
        self.term.history_size()
    }
}

fn style(style: rio_vt::crosswords::style::Style) -> Style {
    let flags = style.flags;
    let mut attributes = Attributes::default();
    for (flag, attribute) in [
        (StyleFlags::BOLD, Attributes::BOLD),
        (StyleFlags::DIM, Attributes::FAINT),
        (StyleFlags::ITALIC, Attributes::ITALIC),
        (StyleFlags::INVERSE, Attributes::INVERSE),
        (StyleFlags::HIDDEN, Attributes::INVISIBLE),
        (StyleFlags::STRIKEOUT, Attributes::STRIKETHROUGH),
    ] {
        if flags.contains(flag) {
            attributes.insert(attribute);
        }
    }
    // rio-vt keeps slow and rapid blink apart, and stores no overline.
    if flags.intersects(StyleFlags::ALL_BLINK) {
        attributes.insert(Attributes::BLINK);
    }
    let underline = if flags.contains(StyleFlags::DOUBLE_UNDERLINE) {
        Underline::Double
    } else if flags.contains(StyleFlags::UNDERCURL) {
        Underline::Curly
    } else if flags.contains(StyleFlags::DOTTED_UNDERLINE) {
        Underline::Dotted
    } else if flags.contains(StyleFlags::DASHED_UNDERLINE) {
        Underline::Dashed
    } else if flags.contains(StyleFlags::UNDERLINE) {
        Underline::Single
    } else {
        Underline::None
    };
    Style {
        foreground: color(style.fg),
        background: color(style.bg),
        underline,
        attributes,
    }
}

/// Maps a colour as the program set it. The 16 named colours are palette
/// entries 0 to 15; every other named colour, such as the default foreground
/// and background or the dim and cursor colours, is the terminal's default.
fn color(color: AnsiColor) -> Color {
    match color {
        AnsiColor::Named(named) if (named as usize) < 16 => Color::Palette(named as u8),
        AnsiColor::Named(_) => Color::Default,
        AnsiColor::Indexed(index) => Color::Palette(index),
        AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alacritty::Alacritty;
    use crate::common;

    #[test]
    fn rio_agrees_with_alacritty_on_common_output() {
        for (input, bytes) in common::INPUTS {
            common::assert_same_screen(
                input,
                &common::read::<Alacritty>(bytes),
                &common::read::<Rio>(bytes),
            );
        }
    }
}
