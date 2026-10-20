//! Cell geometry for validated saved layouts, independent of a renderer.

use crate::{Direction, Leaf, Node, Tab};

/// One visible leaf's content rectangle, relative to the terminal content area.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneRect<'a> {
    /// Saved leaf, not a tmux pane identity.
    pub leaf: &'a Leaf,
    /// Leftmost column.
    pub left: u16,
    /// Topmost row.
    pub top: u16,
    /// Content columns, excluding separators.
    pub cols: u16,
    /// Content rows, excluding separators.
    pub rows: u16,
}

impl Node {
    /// Minimum columns and rows for the complete tree: each leaf needs 34 by 6
    /// cells, with one separator cell between siblings, as in the Python model.
    /// The tree must satisfy the schema's depth limit.
    pub fn minimum_size(&self) -> (u32, u32) {
        match self {
            Self::Leaf(_) => (34, 6),
            Self::Split {
                direction,
                first,
                second,
                ..
            } => {
                let a = first.minimum_size();
                let b = second.minimum_size();
                match direction {
                    Direction::Right => (a.0.saturating_add(b.0).saturating_add(1), a.1.max(b.1)),
                    Direction::Below => (a.0.max(b.0), a.1.saturating_add(b.1).saturating_add(1)),
                }
            }
        }
    }
}

impl Tab {
    /// Lay out a validated tab in the available terminal content cells.
    ///
    /// Explicit focus mode or insufficient space shows only the focused leaf
    /// (the first leaf if focus is stale). The saved tree and ratios never change.
    /// Zero width or height returns no rectangles. Otherwise siblings share a
    /// one-cell separator; ratios use Python's ties-to-even rounding and clamp
    /// to both subtrees' minimum sizes. No window or tmux geometry is changed.
    pub fn pane_rectangles(&self, cols: u16, rows: u16, focus: bool) -> Vec<PaneRect<'_>> {
        if cols == 0 || rows == 0 {
            return Vec::new();
        }
        let needed = self.tree.minimum_size();
        if focus || u32::from(cols) < needed.0 || u32::from(rows) < needed.1 {
            let leaves = self.tree.leaves();
            let leaf = leaves
                .iter()
                .find(|leaf| leaf.id == self.focus)
                .copied()
                .unwrap_or(leaves[0]);
            return vec![PaneRect {
                leaf,
                left: 0,
                top: 0,
                cols,
                rows,
            }];
        }
        let mut panes = Vec::new();
        arrange(&self.tree, 0, 0, cols, rows, &mut panes);
        panes
    }
}

fn arrange<'a>(
    node: &'a Node,
    left: u16,
    top: u16,
    cols: u16,
    rows: u16,
    panes: &mut Vec<PaneRect<'a>>,
) {
    match node {
        Node::Leaf(leaf) => panes.push(PaneRect {
            leaf,
            left,
            top,
            cols,
            rows,
        }),
        Node::Split {
            direction,
            ratio,
            first,
            second,
            ..
        } => {
            let a = first.minimum_size();
            let b = second.minimum_size();
            let (content, lower, upper) = match direction {
                Direction::Right => (cols - 1, a.0, b.0),
                Direction::Below => (rows - 1, a.1, b.1),
            };
            // The complete tree fit before recursion, so both minima fit u16.
            let size = (ratio * f64::from(content))
                .round_ties_even()
                .clamp(f64::from(lower), f64::from(content) - f64::from(upper))
                as u16;
            match direction {
                Direction::Right => {
                    arrange(first, left, top, size, rows, panes);
                    arrange(second, left + size + 1, top, content - size, rows, panes);
                }
                Direction::Below => {
                    arrange(first, left, top, cols, size, panes);
                    arrange(second, left, top + size + 1, cols, content - size, panes);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Layout;

    #[test]
    fn four_panes_fit_clamp_and_temporarily_focus_without_changing_the_tree() {
        let mut layout = Layout::initial("/tmp".into());
        let first = layout.pane().unwrap().id.clone();
        layout.split(Direction::Right, "/tmp".into(), false);
        layout.split(Direction::Below, "/tmp".into(), false);
        layout.workspaces[0].tabs[0].focus = first;
        layout.split(Direction::Below, "/tmp".into(), false);
        let tab = &mut layout.workspaces[0].tabs[0];
        assert_eq!(tab.tree.minimum_size(), (69, 13));
        let geometry = |panes: Vec<PaneRect<'_>>| {
            panes
                .into_iter()
                .map(|p| (p.left, p.top, p.cols, p.rows))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            geometry(tab.pane_rectangles(140, 40, false)),
            [
                (0, 0, 70, 20),
                (0, 21, 70, 19),
                (71, 0, 69, 20),
                (71, 21, 69, 19)
            ]
        );
        if let Node::Split { ratio, .. } = &mut tab.tree {
            *ratio = 0.01;
        }
        assert_eq!(
            geometry(tab.pane_rectangles(140, 13, false)),
            [
                (0, 0, 34, 6),
                (0, 7, 34, 6),
                (35, 0, 105, 6),
                (35, 7, 105, 6)
            ]
        );
        let saved = tab.clone();
        for (cols, rows, focus) in [(68, 40, false), (140, 12, false), (140, 40, true)] {
            let panes = tab.pane_rectangles(cols, rows, focus);
            assert_eq!(panes.len(), 1);
            assert_eq!(panes[0].leaf.id, tab.focus);
            assert_eq!(geometry(panes), [(0, 0, cols, rows)]);
        }
        assert!(tab.pane_rectangles(0, 40, false).is_empty());
        assert!(tab.pane_rectangles(140, 0, false).is_empty());
        assert_eq!(*tab, saved);
        assert_eq!(tab.pane_rectangles(69, 13, false).len(), 4);
    }
}
