//! Converts tmux snapshots into engine-independent terminal restore data.
//! Shared with the fidelity harness so it exercises the app's conversion.

use agent_interface_term::restore::{self, Alternate, Restore};
use agent_interface_term::{Attributes, Color, Style, Underline};
use agent_interface_tmux::capture;
use agent_interface_tmux::recovery::{self, Snapshot};

/// Converts a snapshot into the terminal state it describes.
pub(crate) fn restore(snapshot: &Snapshot) -> Restore {
    let state = &snapshot.state;
    let modes = &state.modes;
    Restore {
        width: state.width,
        height: state.height,
        rows: snapshot.rows.iter().map(row).collect(),
        history: state.history,
        alternate: state.alternate_saved_cursor.map(|saved_cursor| Alternate {
            saved_rows: snapshot.saved.iter().map(row).collect(),
            saved_cursor,
        }),
        cursor: state.cursor,
        cursor_visible: state.cursor_visible,
        scroll_region: state.scroll_region,
        tab_stops: state.tab_stops.clone(),
        modes: restore::Modes {
            cursor_keys: modes.cursor_keys,
            keypad: modes.keypad,
            insert: modes.insert,
            wrap: modes.wrap,
            origin: modes.origin,
            mouse_standard: modes.mouse_standard,
            mouse_button: modes.mouse_button,
            mouse_all: modes.mouse_all,
            mouse_utf8: modes.mouse_utf8,
            mouse_sgr: modes.mouse_sgr,
            bracketed_paste: modes.bracketed_paste,
            extended_keys: modes.extended_keys.map(|keys| match keys {
                recovery::ExtendedKeys::Off => restore::ExtendedKeys::Off,
                recovery::ExtendedKeys::Level1 => restore::ExtendedKeys::Level1,
                recovery::ExtendedKeys::Level2 => restore::ExtendedKeys::Level2,
            }),
            focus_events: modes.focus_events,
        },
        pending: snapshot.pending.clone(),
    }
}

fn row(row: &capture::Row) -> restore::Row {
    restore::Row {
        segments: row
            .segments
            .iter()
            .map(|segment| restore::Segment {
                style: style(&segment.style),
                underline_color: color(segment.style.underline_color),
                link: segment.link.as_ref().map(|link| restore::Link {
                    params: link.params.clone(),
                    uri: link.uri.clone(),
                }),
                line_drawing: segment.line_drawing,
                text: segment.text.clone(),
            })
            .collect(),
    }
}

fn style(style: &capture::Style) -> Style {
    let mut attributes = Attributes::default();
    for (from, to) in [
        (capture::Attributes::BOLD, Attributes::BOLD),
        (capture::Attributes::FAINT, Attributes::FAINT),
        (capture::Attributes::ITALIC, Attributes::ITALIC),
        (capture::Attributes::BLINK, Attributes::BLINK),
        (capture::Attributes::INVERSE, Attributes::INVERSE),
        (capture::Attributes::INVISIBLE, Attributes::INVISIBLE),
        (
            capture::Attributes::STRIKETHROUGH,
            Attributes::STRIKETHROUGH,
        ),
        (capture::Attributes::OVERLINE, Attributes::OVERLINE),
    ] {
        if style.attributes.contains(from) {
            attributes.insert(to);
        }
    }
    Style {
        foreground: color(style.foreground),
        background: color(style.background),
        underline: match style.underline {
            capture::Underline::None => Underline::None,
            capture::Underline::Single => Underline::Single,
            capture::Underline::Double => Underline::Double,
            capture::Underline::Curly => Underline::Curly,
            capture::Underline::Dotted => Underline::Dotted,
            capture::Underline::Dashed => Underline::Dashed,
        },
        attributes,
    }
}

fn color(color: capture::Color) -> Color {
    match color {
        capture::Color::Default => Color::Default,
        capture::Color::Palette(index) => Color::Palette(index),
        capture::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
