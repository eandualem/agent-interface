//! Comparing final screens, written by [`crate::dump::dump`]: between
//! engines, and with tmux's own capture of the pane.

use tmux_workspaces_term::{Cursor, Screen, Terminal};

use crate::engine::Grid;
use crate::record::Capture;

/// How two dumped screens differ.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Difference {
    /// Whether the size or the cursor differs.
    pub header: bool,
    /// The number of cells that differ in any way.
    pub cells: usize,
    /// The number of rows with a differing cell.
    pub rows: usize,
    /// The number of rows whose characters differ, ignoring widths and
    /// styles: where the screens hold different text, as when they wrapped
    /// or scrolled differently.
    pub text_rows: usize,
    /// The first row with a differing cell.
    pub first_row: Option<u16>,
    /// The first few differing lines, from both screens.
    pub examples: Vec<(String, String)>,
}

impl Difference {
    /// Returns whether the screens are the same.
    pub fn is_none(&self) -> bool {
        !self.header && self.cells == 0
    }
}

/// Returns the row of a cell's line in a dump.
fn row(line: &str) -> Option<u16> {
    line.split(',').next().and_then(|row| row.parse().ok())
}

/// Compares two screens written by [`crate::dump::dump`].
pub fn compare(a: &str, b: &str) -> Difference {
    const EXAMPLES: usize = 5;
    let mut difference = Difference {
        text_rows: differing_rows(&dump_rows(a), &dump_rows(b)),
        ..Difference::default()
    };
    let mut a_lines = a.lines();
    let mut b_lines = b.lines();
    let (a_header, b_header) = (a_lines.next(), b_lines.next());
    if a_header != b_header {
        difference.header = true;
        difference.examples.push((
            a_header.unwrap_or_default().to_owned(),
            b_header.unwrap_or_default().to_owned(),
        ));
    }
    let mut last_row = None;
    for (a_line, b_line) in a_lines.zip(b_lines) {
        if a_line == b_line {
            continue;
        }
        difference.cells += 1;
        let row = row(a_line);
        if row != last_row {
            difference.rows += 1;
            last_row = row;
        }
        difference.first_row = difference.first_row.or(row);
        if difference.examples.len() < EXAMPLES {
            difference
                .examples
                .push((a_line.to_owned(), b_line.to_owned()));
        }
    }
    difference
}

/// Returns each row's text from a dump: its cells' text in order, a blank
/// cell as a space and a wide character's spacer as nothing, without
/// trailing spaces. This is how `tmux capture-pane` writes a row.
pub fn dump_rows(dump: &str) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    for line in dump.lines().skip(1) {
        let Some(row) = row(line) else { continue };
        let row = usize::from(row);
        if rows.len() <= row {
            rows.resize(row + 1, String::new());
        }
        let mut fields = line.splitn(7, ' ');
        let spacer = matches!(fields.nth(1), Some("spacer" | "wrap-spacer"));
        match fields.nth(4).unwrap_or("") {
            "" if !spacer => rows[row].push(' '),
            text => rows[row].push_str(text),
        }
    }
    for row in &mut rows {
        row.truncate(row.trim_end().len());
    }
    rows
}

/// Returns each row's text as tmux captured it, without escape sequences
/// and trailing spaces.
pub fn capture_rows(capture: &Capture) -> Vec<String> {
    capture
        .text
        .lines()
        .map(|line| {
            let mut text = String::new();
            let mut chars = line.chars();
            while let Some(c) = chars.next() {
                if c != '\x1b' {
                    text.push(c);
                } else if chars.next() == Some('[') {
                    // A control sequence ends with a byte from `@` to `~`.
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
            }
            text.truncate(text.trim_end().len());
            text
        })
        .collect()
}

/// Returns the cursor's column and row from a dump's first line.
pub fn dump_cursor(dump: &str) -> Option<(u16, u16)> {
    let header = dump.lines().next()?;
    let (col, row) = header.split(' ').nth(3)?.split_once(',')?;
    Some((col.parse().ok()?, row.parse().ok()?))
}

/// Returns the number of rows whose text differs.
pub fn differing_rows(a: &[String], b: &[String]) -> usize {
    (0..a.len().max(b.len()))
        .filter(|&row| a.get(row) != b.get(row))
        .count()
}

/// Rebuilds tmux's view of the pane as a [`Screen`], by writing its capture
/// to a fresh terminal without history and taking tmux's cursor.
pub fn screen_from_capture(capture: &Capture, grid: Grid) -> Screen {
    let mut terminal =
        Terminal::new(grid.cols, grid.rows, 0).expect("create a terminal for the capture");
    let text = capture.text.strip_suffix('\n').unwrap_or(&capture.text);
    terminal.write(text.replace('\n', "\r\n").as_bytes());
    let mut screen = Screen::default();
    terminal
        .read_screen(&mut screen)
        .expect("read the capture's screen");
    screen.set_cursor(Cursor {
        col: capture.cursor.0,
        row: capture.cursor.1,
        visible: capture.cursor_visible,
    });
    screen
}

#[cfg(test)]
mod tests {
    use tmux_workspaces_term::{Attributes, Style, Width};

    use super::*;
    use crate::dump::dump;

    fn screen(cells: &[&str]) -> Screen {
        let mut screen = Screen::default();
        screen.clear(2, 1);
        for text in cells {
            screen.push(text, Width::Narrow, Style::default());
        }
        screen
    }

    #[test]
    fn blank_cells_compare_equal_whether_empty_or_a_space() {
        assert!(compare(&dump(&screen(&["a", ""])), &dump(&screen(&["a", " "]))).is_none());
    }

    #[test]
    fn reports_each_differing_cell() {
        let difference = compare(&dump(&screen(&["a", "b"])), &dump(&screen(&["x", "b"])));
        assert_eq!(difference.cells, 1);
        assert_eq!((difference.rows, difference.text_rows), (1, 1));
        assert_eq!(difference.first_row, Some(0));
        assert!(!difference.header);
        assert_eq!(
            difference.examples,
            [(
                "0,0 narrow fg=default bg=default ul=- attrs=- a".to_owned(),
                "0,0 narrow fg=default bg=default ul=- attrs=- x".to_owned()
            )]
        );
    }

    #[test]
    fn a_dump_row_drops_spacers_and_trailing_blanks() {
        let mut wide = Screen::default();
        wide.clear(4, 1);
        wide.push("界", Width::Wide, Style::default());
        wide.push("", Width::Spacer, Style::default());
        wide.push("a", Width::Narrow, Style::default());
        wide.push(" ", Width::Narrow, Style::default());
        assert_eq!(dump_rows(&dump(&wide)), ["界a"]);
        assert_eq!(dump_cursor(&dump(&wide)), Some((0, 0)));
    }

    #[test]
    fn a_style_difference_is_not_a_text_difference() {
        let mut bold = screen(&["a", "b"]);
        bold.clear(2, 1);
        let style = Style {
            attributes: Attributes::BOLD,
            ..Style::default()
        };
        bold.push("a", Width::Narrow, style);
        bold.push("b", Width::Narrow, Style::default());
        let difference = compare(&dump(&screen(&["a", "b"])), &dump(&bold));
        assert_eq!(
            (difference.cells, difference.rows, difference.text_rows),
            (1, 1, 0)
        );
    }

    #[test]
    fn a_capture_becomes_the_screen_tmux_shows() {
        let capture = Capture {
            text: "\x1b[1mab\x1b[0m\nc\n".to_owned(),
            cursor: (1, 1),
            cursor_visible: true,
        };
        let grid = Grid {
            cols: 3,
            rows: 2,
            scrollback: 0,
        };
        assert_eq!(capture_rows(&capture), ["ab", "c"]);
        let screen = screen_from_capture(&capture, grid);
        assert_eq!(screen.cell(0, 1).unwrap().0, "b");
        assert!(
            screen
                .cell(0, 1)
                .unwrap()
                .1
                .style
                .attributes
                .contains(Attributes::BOLD)
        );
        assert_eq!(screen.cell(1, 0).unwrap().0, "c");
        assert_eq!(
            screen.cursor(),
            Cursor {
                col: 1,
                row: 1,
                visible: true
            }
        );
    }
}
