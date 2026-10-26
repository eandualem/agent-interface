//! The comparison format for final screens: one line per cell, so screens
//! from different engines and processes compare as text.
//!
//! `agent-interface-bench-rio` includes this file.

use std::fmt::Write as _;

use crate::display::{Attributes, Color, Screen, Underline, Width};

/// Writes `screen` in the comparison format: a line with the size and the
/// cursor, then one line per cell.
///
/// Blank text, empty or a single space, is written as nothing, because
/// engines store a blank cell either way.
pub fn dump(screen: &Screen) -> String {
    let cursor = screen.cursor();
    let mut out = format!(
        "screen {}x{} cursor {},{} {}\n",
        screen.cols(),
        screen.rows(),
        cursor.col,
        cursor.row,
        if cursor.visible { "shown" } else { "hidden" }
    );
    for row in 0..screen.rows() {
        for col in 0..screen.cols() {
            let (text, cell) = screen.cell(row, col).expect("a cell inside the screen");
            let width = match cell.width {
                Width::Narrow => "narrow",
                Width::Wide => "wide",
                Width::Spacer => "spacer",
                Width::WrapSpacer => "wrap-spacer",
            };
            let underline = match cell.style.underline {
                Underline::None => "-",
                Underline::Single => "single",
                Underline::Double => "double",
                Underline::Curly => "curly",
                Underline::Dotted => "dotted",
                Underline::Dashed => "dashed",
            };
            let text = if text == " " { "" } else { text };
            let _ = writeln!(
                out,
                "{row},{col} {width} fg={} bg={} ul={underline} attrs={} {text}",
                color(cell.style.foreground),
                color(cell.style.background),
                attributes(cell.style.attributes),
            );
        }
    }
    out
}

fn color(color: Color) -> String {
    match color {
        Color::Default => "default".to_owned(),
        Color::Palette(index) => format!("p{index}"),
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

fn attributes(attributes: Attributes) -> String {
    let names = [
        (Attributes::BOLD, 'b'),
        (Attributes::FAINT, 'f'),
        (Attributes::ITALIC, 'i'),
        (Attributes::BLINK, 'k'),
        (Attributes::INVERSE, 'r'),
        (Attributes::INVISIBLE, 'h'),
        (Attributes::STRIKETHROUGH, 's'),
        (Attributes::OVERLINE, 'o'),
    ];
    let set: String = names
        .iter()
        .filter(|(attribute, _)| attributes.contains(*attribute))
        .map(|(_, name)| name)
        .collect();
    if set.is_empty() { "-".to_owned() } else { set }
}
