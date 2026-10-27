//! Selection owns the displayed text so background output cannot change a copy.

use agent_interface_term::{Screen, Width};
use agent_interface_tmux::control::PaneId;
use std::ops::Range;

#[derive(Clone, Copy)]
enum Unit {
    Character,
    Word,
    Line,
}

pub(super) struct Selection {
    pub pane: PaneId,
    pub screen: Screen,
    pub dragging: bool,
    anchor: Range<usize>,
    head: Range<usize>,
    unit: Unit,
}

impl Selection {
    pub fn new(pane: PaneId, mut screen: Screen, row: u16, col: u16, clicks: usize) -> Self {
        let mut cursor = screen.cursor();
        cursor.visible = false;
        screen.set_cursor(cursor);
        let unit = match clicks {
            2 => Unit::Word,
            3.. => Unit::Line,
            _ => Unit::Character,
        };
        let mut selection = Self {
            pane,
            screen,
            dragging: true,
            anchor: 0..0,
            head: 0..0,
            unit,
        };
        selection.anchor = selection.unit_at(row, col);
        selection.head = selection.anchor.clone();
        selection
    }

    pub fn reselect(&self, row: u16, col: u16, clicks: usize) -> Self {
        Self::new(self.pane, self.screen.clone(), row, col, clicks)
    }

    pub fn uses_boundaries(&self) -> bool {
        matches!(self.unit, Unit::Character)
    }

    pub fn extend(&mut self, row: u16, col: u16) {
        self.head = self.unit_at(row, col);
    }

    pub fn is_empty(&self) -> bool {
        self.range().is_empty()
    }

    fn range(&self) -> Range<usize> {
        let mut start = self.anchor.start.min(self.head.start);
        let mut end = self.anchor.end.max(self.head.end);
        if start == end {
            return start..end;
        }
        if self
            .cell(start)
            .is_some_and(|(_, cell)| cell.width == Width::Spacer)
        {
            start = start.saturating_sub(1);
        }
        if self
            .cell(end)
            .is_some_and(|(_, cell)| cell.width == Width::Spacer)
        {
            end += 1;
        }
        start..end
    }

    pub fn row_range(&self, row: u16) -> Range<u16> {
        let cols = usize::from(self.screen.cols());
        let first = usize::from(row) * cols;
        let range = self.range();
        let start = range.start.saturating_sub(first).min(cols) as u16;
        let end = range.end.saturating_sub(first).min(cols) as u16;
        start..end
    }

    pub fn contains(&self, row: u16, col: u16) -> bool {
        self.row_range(row).contains(&col)
    }

    pub fn text(&self) -> String {
        let mut text = String::new();
        let range = self.range();
        if range.is_empty() {
            return text;
        }
        let cols = usize::from(self.screen.cols());
        for row in range.start / cols..=(range.end - 1) / cols {
            let mut line = String::new();
            for col in self.row_range(row as u16) {
                if let Some((value, cell)) = self.screen.cell(row as u16, col) {
                    if matches!(cell.width, Width::Spacer | Width::WrapSpacer) {
                        continue;
                    }
                    line.push_str(if value.is_empty() { " " } else { value });
                }
            }
            let continues = self.screen.row_wrapped(row as u16) && range.end > (row + 1) * cols;
            text.push_str(if continues {
                &line
            } else {
                line.trim_end_matches(' ')
            });
            if !continues && range.end > (row + 1) * cols {
                text.push('\n');
            }
        }
        text
    }

    fn cell(&self, index: usize) -> Option<(&str, agent_interface_term::Cell)> {
        let cols = usize::from(self.screen.cols());
        if cols == 0 {
            return None;
        }
        self.screen
            .cell((index / cols) as u16, (index % cols) as u16)
    }

    fn class(&self, mut index: usize) -> u8 {
        if self
            .cell(index)
            .is_some_and(|(_, cell)| cell.width == Width::Spacer)
        {
            index = index.saturating_sub(1);
        }
        let ch = self
            .cell(index)
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

    fn unit_at(&self, row: u16, col: u16) -> Range<usize> {
        let cols = usize::from(self.screen.cols());
        let row = usize::from(row.min(self.screen.rows().saturating_sub(1)));
        let index = row * cols + usize::from(col).min(cols);
        match self.unit {
            Unit::Character => index..index,
            Unit::Line => row * cols..(row + 1) * cols,
            Unit::Word => {
                let index = row * cols + usize::from(col).min(cols.saturating_sub(1));
                let class = self.class(index);
                let mut start = index;
                let mut end = index + 1;
                while start > row * cols && self.class(start - 1) == class {
                    start -= 1;
                }
                while end < (row + 1) * cols && self.class(end) == class {
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
    fn selection_preserves_graphemes_wraps_and_snapshot_text() {
        let mut terminal = Terminal::new(6, 3, 0).unwrap();
        terminal.write("ab界e\u{301}fg\r\nlast".as_bytes());
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        let mut selection = Selection::new(PaneId(1), screen, 0, 0, 1);
        selection.extend(1, 1);
        assert_eq!(selection.text(), "ab界e\u{301}fg");
        terminal.write(b"new output");
        assert_eq!(selection.text(), "ab界e\u{301}fg");
        let mut wide = Selection::new(PaneId(1), selection.screen.clone(), 0, 3, 1);
        wide.extend(0, 4);
        assert_eq!(wide.text(), "界");
        let word = Selection::new(PaneId(1), selection.screen.clone(), 2, 2, 2);
        assert_eq!(word.text(), "last");
        let line = selection.reselect(2, 2, 3);
        assert_eq!(line.text(), "last");
        selection.extend(2, 4);
        assert_eq!(selection.text(), "ab界e\u{301}fg\nlast");
    }
}
