//! A selection holds history rows, not screen rows, so it stays on the same
//! output while the pane scrolls or receives more. Its text is read from the
//! screen that shows those rows, or from the pane's history when they are
//! out of view.

use agent_interface_term::{Screen, Width};
use agent_interface_tmux::control::PaneId;
use std::ops::Range;

#[derive(Clone, Copy)]
enum Unit {
    Character,
    Word,
    Line,
}

/// Positions count cells from the oldest history row: `row * cols + col`.
#[derive(Clone)]
pub(super) struct Selection {
    pub pane: PaneId,
    pub dragging: bool,
    cols: usize,
    anchor: Range<usize>,
    head: Range<usize>,
    unit: Unit,
}

impl Selection {
    pub fn new(pane: PaneId, screen: &Screen, row: u16, col: u16, clicks: usize) -> Self {
        let unit = match clicks {
            2 => Unit::Word,
            3.. => Unit::Line,
            _ => Unit::Character,
        };
        let mut selection = Self {
            pane,
            dragging: true,
            cols: usize::from(screen.cols()),
            anchor: 0..0,
            head: 0..0,
            unit,
        };
        selection.anchor = selection.unit_at(screen, row, col);
        selection.head = selection.anchor.clone();
        selection
    }

    pub fn reselect(&self, screen: &Screen, row: u16, col: u16, clicks: usize) -> Self {
        Self::new(self.pane, screen, row, col, clicks)
    }

    pub fn uses_boundaries(&self) -> bool {
        matches!(self.unit, Unit::Character)
    }

    pub fn extend(&mut self, screen: &Screen, row: u16, col: u16) {
        self.head = self.unit_at(screen, row, col);
    }

    pub fn is_empty(&self) -> bool {
        self.anchor.start.min(self.head.start) == self.anchor.end.max(self.head.end)
    }

    /// The history rows the selection covers.
    pub fn rows(&self) -> Range<usize> {
        let range = self.range(None);
        if range.is_empty() || self.cols == 0 {
            return 0..0;
        }
        range.start / self.cols..(range.end - 1) / self.cols + 1
    }

    /// Whether `screen` shows every row of the selection.
    pub fn shown_by(&self, screen: &Screen) -> bool {
        let rows = self.rows();
        usize::from(screen.cols()) == self.cols
            && rows.start >= screen.top()
            && rows.end <= screen.top() + usize::from(screen.rows())
    }

    /// The selected cells; a wide character `screen` shows is taken whole.
    fn range(&self, screen: Option<&Screen>) -> Range<usize> {
        let mut start = self.anchor.start.min(self.head.start);
        let mut end = self.anchor.end.max(self.head.end);
        if start == end {
            return start..end;
        }
        if let Some(screen) = screen {
            if self
                .cell(screen, start)
                .is_some_and(|(_, cell)| cell.width == Width::Spacer)
            {
                start = start.saturating_sub(1);
            }
            if self
                .cell(screen, end)
                .is_some_and(|(_, cell)| cell.width == Width::Spacer)
            {
                end += 1;
            }
        }
        start..end
    }

    /// The selected columns of `screen`'s `row`.
    pub fn row_range(&self, screen: &Screen, row: u16) -> Range<u16> {
        let cols = self.cols;
        let first = (screen.top() + usize::from(row)) * cols;
        let range = self.range(Some(screen));
        let start = range.start.saturating_sub(first).min(cols) as u16;
        let end = range.end.saturating_sub(first).min(cols) as u16;
        start..end
    }

    pub fn contains(&self, screen: &Screen, row: u16, col: u16) -> bool {
        self.row_range(screen, row).contains(&col)
    }

    /// The selected text, read from `screen`, which shows every selected row.
    pub fn text(&self, screen: &Screen) -> String {
        let mut text = String::new();
        let range = self.range(Some(screen));
        if range.is_empty() || !self.shown_by(screen) {
            return text;
        }
        let cols = self.cols;
        for row in range.start / cols..=(range.end - 1) / cols {
            let line = (row - screen.top()) as u16;
            let mut part = String::new();
            for col in self.row_range(screen, line) {
                if let Some((value, cell)) = screen.cell(line, col) {
                    if matches!(cell.width, Width::Spacer | Width::WrapSpacer) {
                        continue;
                    }
                    part.push_str(if value.is_empty() { " " } else { value });
                }
            }
            let continues = screen.row_wrapped(line) && range.end > (row + 1) * cols;
            text.push_str(if continues {
                &part
            } else {
                part.trim_end_matches(' ')
            });
            if !continues && range.end > (row + 1) * cols {
                text.push('\n');
            }
        }
        text
    }

    /// The cell at history position `index`, if `screen` shows its row.
    fn cell<'a>(
        &self,
        screen: &'a Screen,
        index: usize,
    ) -> Option<(&'a str, agent_interface_term::Cell)> {
        if self.cols == 0 {
            return None;
        }
        let row = (index / self.cols).checked_sub(screen.top())?;
        screen.cell(u16::try_from(row).ok()?, (index % self.cols) as u16)
    }

    fn class(&self, screen: &Screen, mut index: usize) -> u8 {
        if self
            .cell(screen, index)
            .is_some_and(|(_, cell)| cell.width == Width::Spacer)
        {
            index = index.saturating_sub(1);
        }
        let ch = self
            .cell(screen, index)
            .and_then(|(text, _)| text.chars().next())
            .unwrap_or(' ');
        if ch.is_whitespace() {
            0
        } else if ch.is_alphanumeric() || ch == '_' {
            1
        } else {
            2
        }
    }

    fn unit_at(&self, screen: &Screen, row: u16, col: u16) -> Range<usize> {
        let cols = self.cols;
        let row = screen.top() + usize::from(row.min(screen.rows().saturating_sub(1)));
        let index = row * cols + usize::from(col).min(cols);
        match self.unit {
            Unit::Character => index..index,
            Unit::Line => row * cols..(row + 1) * cols,
            Unit::Word => {
                let index = row * cols + usize::from(col).min(cols.saturating_sub(1));
                let class = self.class(screen, index);
                let mut start = index;
                let mut end = index + 1;
                while start > row * cols && self.class(screen, start - 1) == class {
                    start -= 1;
                }
                while end < (row + 1) * cols && self.class(screen, end) == class {
                    end += 1;
                }
                start..end
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_term::Terminal;

    #[test]
    fn selection_preserves_graphemes_wraps_and_words() {
        let mut terminal = Terminal::new(6, 3, 0).unwrap();
        terminal.write("ab界e\u{301}fg\r\nlast".as_bytes());
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        let mut selection = Selection::new(PaneId(1), &screen, 0, 0, 1);
        selection.extend(&screen, 1, 1);
        assert_eq!(selection.text(&screen), "ab界e\u{301}fg");
        let mut wide = Selection::new(PaneId(1), &screen, 0, 3, 1);
        wide.extend(&screen, 0, 4);
        assert_eq!(wide.text(&screen), "界");
        let word = Selection::new(PaneId(1), &screen, 2, 2, 2);
        assert_eq!(word.text(&screen), "last");
        let line = selection.reselect(&screen, 2, 2, 3);
        assert_eq!(line.text(&screen), "last");
        selection.extend(&screen, 2, 4);
        assert_eq!(selection.text(&screen), "ab界e\u{301}fg\nlast");
    }

    #[test]
    fn selection_follows_its_rows_through_scrolling_and_history() {
        let mut terminal = Terminal::new(4, 3, 100).unwrap();
        let lines: Vec<String> = (0..10).map(|n| format!("l{n}")).collect();
        terminal.write(lines.join("\r\n").as_bytes());
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        // Select from the end of l8, the second row shown, then scroll to
        // the top and extend to the start of l1.
        let mut selection = Selection::new(PaneId(1), &screen, 1, 2, 1);
        terminal.restore_scrolled_row(0);
        terminal.read_screen(&mut screen).unwrap();
        selection.extend(&screen, 1, 0);
        assert_eq!(selection.rows(), 1..9);
        assert!(!selection.shown_by(&screen));
        assert!(selection.row_range(&screen, 0).is_empty());
        assert_eq!(selection.row_range(&screen, 1), 0..4);
        let mut history = Screen::default();
        terminal.read_rows(selection.rows(), &mut history).unwrap();
        assert_eq!(selection.text(&history), "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8");
    }
}
