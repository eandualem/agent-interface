//! Expose the active terminal and its input caret through the native text APIs.
//! Terminal output is inspectable, but cannot be replaced as an editable document.

use gpui::accesskit::{Node, Rect, Role, TextDirection, TextPosition, TextSelection};
use tmux_workspaces_term::{Screen, Width};
use tmux_workspaces_tmux::control::PaneId;

use super::Grid;

pub(super) fn terminal(
    builder: &mut gpui::A11ySubtreeBuilder,
    grid: &Grid,
    pane: PaneId,
    display_scale: f32,
    embedded: bool,
) {
    let Some(screen) = grid.screens.get(&pane) else {
        return;
    };
    let Some(layout) = grid.layout.iter().find(|layout| layout.id == pane) else {
        return;
    };
    let Some(bounds) = builder.parent_node().bounds() else {
        return;
    };
    let cols = grid
        .layout
        .iter()
        .map(|p| u32::from(p.left) + u32::from(p.cols))
        .max()
        .unwrap_or(1)
        .max(1);
    let rows = grid
        .layout
        .iter()
        .map(|p| u32::from(p.top) + u32::from(p.rows))
        .max()
        .unwrap_or(1)
        .max(1);
    // Match the grid's fit while the server is still resizing the viewport.
    let original_width = f64::from(f32::from(grid.original_cell.width) * display_scale);
    let original_height = f64::from(f32::from(grid.original_cell.height) * display_scale);
    let scale = if embedded {
        (bounds.width() / (f64::from(cols) * original_width))
            .min(bounds.height() / (f64::from(rows) * original_height))
            .clamp(0.01, 1.0)
    } else {
        1.0
    };
    let width = original_width * scale;
    let height = original_height * scale;
    let cursor = screen.cursor();
    for row in 0..screen.rows() {
        let (mut run, caret) = text_run(screen, row, width);
        let x = bounds.x0 + f64::from(layout.left) * width;
        let y = bounds.y0 + (f64::from(layout.top) + f64::from(row)) * height;
        run.set_bounds(Rect::new(
            x,
            y,
            x + f64::from(screen.cols()) * width,
            y + height,
        ));
        let id = builder.synthetic_node_id((pane.0, row));
        builder.push_child(id, run);
        if screen.cursor_in_viewport() && row == cursor.row {
            let caret = TextPosition {
                node: id,
                character_index: caret,
            };
            // A terminal's mouse selection copies output; it does not select
            // editable input. Dictation follows the program's input cursor.
            builder.parent_node().set_text_selection(TextSelection {
                anchor: caret,
                focus: caret,
            });
        }
    }
}

fn text_run(screen: &Screen, row: u16, width: f64) -> (Node, usize) {
    let mut text = String::new();
    let mut lengths = Vec::new();
    let mut positions = Vec::new();
    let mut widths = Vec::new();
    let mut caret = 0;
    for col in 0..screen.cols() {
        if col <= screen.cursor().col {
            caret = lengths.len();
        }
        let Some((value, cell)) = screen.cell(row, col) else {
            continue;
        };
        if matches!(cell.width, Width::Spacer | Width::WrapSpacer) {
            continue;
        }
        let value = if value.is_empty() { " " } else { value };
        text.push_str(value);
        // A terminal cell holds one grapheme cluster. AccessKit characters
        // must not split it, except for the u8 limit on a character's bytes.
        let mut remaining = value;
        let mut first = true;
        while !remaining.is_empty() {
            let mut end = remaining.len().min(usize::from(u8::MAX));
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            lengths.push(end as u8);
            positions.push((f64::from(col) * width) as f32);
            widths.push(if first {
                (width * if cell.width == Width::Wide { 2.0 } else { 1.0 }) as f32
            } else {
                0.0
            });
            remaining = &remaining[end..];
            first = false;
        }
    }
    if screen.cursor().col >= screen.cols() {
        caret = lengths.len();
    }
    if row + 1 < screen.rows() && !screen.row_wrapped(row) {
        text.push('\n');
        lengths.push(1);
        positions.push((f64::from(screen.cols()) * width) as f32);
        widths.push(0.0);
    }
    let mut run = Node::new(Role::TextRun);
    run.set_value(text);
    run.set_character_lengths(lengths);
    run.set_character_positions(positions);
    run.set_character_widths(widths);
    run.set_text_direction(TextDirection::LeftToRight);
    (run, caret)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_workspaces_term::{Cursor, Style};

    #[test]
    fn terminal_text_preserves_unicode_and_maps_the_input_caret() {
        let mut screen = Screen::default();
        screen.clear(5, 2);
        for (text, width) in [
            ("a", Width::Narrow),
            ("😀", Width::Wide),
            ("", Width::Spacer),
            ("e\u{301}", Width::Narrow),
            ("", Width::Narrow),
        ] {
            screen.push(text, width, Style::default());
        }
        screen.set_cursor(Cursor {
            row: 0,
            col: 4,
            ..Cursor::default()
        });
        assert!(!screen.cursor().visible);
        assert!(screen.cursor_in_viewport());
        let (run, caret) = text_run(&screen, 0, 8.0);
        assert_eq!(run.value(), Some("a😀e\u{301} \n"));
        assert_eq!(run.character_lengths(), &[1, 4, 3, 1, 1]);
        assert_eq!(
            run.value()
                .unwrap()
                .get(
                    ..run.character_lengths()[..caret]
                        .iter()
                        .map(|&len| usize::from(len))
                        .sum()
                )
                .unwrap()
                .encode_utf16()
                .count(),
            5
        );
        assert_eq!(run.character_positions().unwrap()[caret], 32.0);
        screen.set_row_wrapped(0, true);
        assert_eq!(text_run(&screen, 0, 8.0).0.value(), Some("a😀e\u{301} "));

        let long_cluster = format!("e{}", "\u{301}".repeat(150));
        screen.clear(1, 1);
        assert!(!screen.cursor_in_viewport());
        screen.push(&long_cluster, Width::Narrow, Style::default());
        let (run, _) = text_run(&screen, 0, 8.0);
        assert_eq!(run.value(), Some(long_cluster.as_str()));
        assert_eq!(run.character_lengths(), &[255, 46]);
        assert_eq!(run.character_widths(), Some([8.0, 0.0].as_slice()));
    }
}
