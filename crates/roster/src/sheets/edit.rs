//! Atomic typed mutations and stable-cell feedback. A prepared request retains
//! its exact bytes and retry key, including when the first reply is lost.

use super::{Id, Input, View, model::valid_format};
use serde::{Serialize, Serializer};

/// Omitted, explicit null, or a replacement value.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Patch<T> {
    /// Preserve the existing property.
    #[default]
    Unchanged,
    /// Clear/reset the property.
    Clear,
    /// Replace the property.
    Set(T),
}
impl<T> Patch<T> {
    fn unchanged(&self) -> bool {
        matches!(self, Self::Unchanged)
    }
}
impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Set(value) => value.serialize(s),
            _ => s.serialize_none(),
        }
    }
}
/// A patch addresses a stable coordinate, never a visible row number.
#[derive(Clone, Debug, Serialize)]
pub struct Change {
    /// Stable row.
    pub row_id: Id,
    /// Stable column.
    pub column_id: Id,
    /// Input patch; clearing produces a real blank.
    #[serde(skip_serializing_if = "Patch::unchanged")]
    pub input: Patch<Input>,
    /// Format patch; setting input alone preserves it.
    #[serde(skip_serializing_if = "Patch::unchanged")]
    pub number_format: Patch<String>,
}
impl Change {
    fn valid(&self) -> bool {
        (!self.input.unchanged() || !self.number_format.unchanged())
            && match &self.input {
                Patch::Set(v) => v.valid(),
                _ => true,
            }
            && match &self.number_format {
                Patch::Set(s) => valid_format(Some(s)),
                _ => true,
            }
    }
}
/// A new tab, with client-generated stable identifiers.
#[derive(Clone, Debug, Serialize)]
pub struct NewTab {
    /// New tab identifier.
    pub tab_id: Id,
    /// Unique engine-valid tab name.
    pub name: String,
    /// Ordered canonical row identifiers.
    pub row_ids: Vec<Id>,
    /// Ordered canonical column identifiers.
    pub column_ids: Vec<Id>,
    /// Sparse initial values and formatting.
    pub cells: Vec<Change>,
    /// Initial saved view.
    pub view: View,
}
impl NewTab {
    fn valid(&self) -> bool {
        use std::collections::HashSet;
        let rows: HashSet<_> = self.row_ids.iter().collect();
        let columns: HashSet<_> = self.column_ids.iter().collect();
        let mut coords = HashSet::new();
        valid_name(&self.name)
            && (1..=10_000).contains(&rows.len())
            && rows.len() == self.row_ids.len()
            && (1..=256).contains(&columns.len())
            && columns.len() == self.column_ids.len()
            && self.cells.len() <= 50_000
            && self.view.valid()
            && self.cells.iter().all(|c| {
                c.valid()
                    && rows.contains(&c.row_id)
                    && columns.contains(&c.column_id)
                    && coords.insert((&c.row_id, &c.column_id))
            })
    }
}
fn valid_name(s: &str) -> bool {
    !s.trim().is_empty()
        && !s.starts_with("'")
        && !s.ends_with("'")
        && s.chars().count() <= 31
        && !s.chars().any(|c| c.is_control() || "[]:*?/\\".contains(c))
}
/// One atomic operation supported by the provider.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Operation {
    /// Patch inputs and formats together.
    SetCells {
        /// Target tab.
        tab_id: Id,
        /// At most 1000 changes in the entire edit.
        cells: Vec<Change>,
    },
    /// Add a new tab.
    AddTab {
        /// Complete initial tab.
        tab: NewTab,
    },
    /// Rename a tab without changing its identity.
    RenameTab {
        /// Target tab.
        tab_id: Id,
        /// New name.
        name: String,
    },
    /// Delete a tab; the provider refuses deleting the last one.
    DeleteTab {
        /// Target tab.
        tab_id: Id,
    },
    /// Insert canonical rows, updating formula references atomically.
    InsertRows {
        /// Target tab.
        tab_id: Id,
        /// Existing row before insertion, or append.
        before_id: Option<Id>,
        /// New stable IDs.
        ids: Vec<Id>,
    },
    /// Insert canonical columns.
    InsertColumns {
        /// Target tab.
        tab_id: Id,
        /// Existing column before insertion, or append.
        before_id: Option<Id>,
        /// New stable IDs.
        ids: Vec<Id>,
    },
    /// Delete rows, retaining historical references.
    DeleteRows {
        /// Target tab.
        tab_id: Id,
        /// Rows to delete.
        ids: Vec<Id>,
    },
    /// Delete columns, retaining historical references.
    DeleteColumns {
        /// Target tab.
        tab_id: Id,
        /// Columns to delete.
        ids: Vec<Id>,
    },
    /// Move a canonical contiguous row interval.
    MoveRows {
        /// Target tab.
        tab_id: Id,
        /// Rows in canonical order.
        ids: Vec<Id>,
        /// Destination before row, or append.
        before_id: Option<Id>,
    },
    /// Move a canonical contiguous column interval.
    MoveColumns {
        /// Target tab.
        tab_id: Id,
        /// Columns in canonical order.
        ids: Vec<Id>,
        /// Destination before column, or append.
        before_id: Option<Id>,
    },
    /// Change presentation without moving canonical formula coordinates.
    SetView {
        /// Target tab.
        tab_id: Id,
        /// Complete sort/filter definition.
        view: View,
    },
    /// Copy a rectangle with provider-adjusted relative formula references.
    CopyCells {
        /// Source tab.
        source_tab_id: Id,
        /// Source rows in selection order.
        source_row_ids: Vec<Id>,
        /// Source columns in selection order.
        source_column_ids: Vec<Id>,
        /// Target tab.
        target_tab_id: Id,
        /// Target rows, matching source dimensions.
        target_row_ids: Vec<Id>,
        /// Target columns, matching source dimensions.
        target_column_ids: Vec<Id>,
    },
}
impl Operation {
    fn affected(&self) -> Option<usize> {
        let n = match self {
            Self::SetCells { cells, .. }
                if !cells.is_empty() && cells.iter().all(Change::valid) =>
            {
                cells.len()
            }
            Self::AddTab { tab } if tab.valid() => {
                tab.row_ids.len() + tab.column_ids.len() + tab.cells.len()
            }
            Self::RenameTab { name, .. } if valid_name(name) => 1,
            Self::DeleteTab { .. } => 1,
            Self::SetView { view, .. } if view.valid() => 1,
            Self::InsertRows { ids, .. }
            | Self::InsertColumns { ids, .. }
            | Self::DeleteRows { ids, .. }
            | Self::DeleteColumns { ids, .. }
            | Self::MoveRows { ids, .. }
            | Self::MoveColumns { ids, .. }
                if !ids.is_empty() =>
            {
                ids.len()
            }
            Self::CopyCells {
                source_row_ids: r,
                source_column_ids: c,
                target_row_ids: tr,
                target_column_ids: tc,
                ..
            } if !r.is_empty() && !c.is_empty() && r.len() == tr.len() && c.len() == tc.len() => {
                r.len().checked_mul(c.len())?
            }
            _ => return None,
        };
        Some(n)
    }
}
/// Human edits retain the registered owner and use operator authority.
#[derive(Clone, Debug, Serialize)]
pub struct Edit {
    author: Option<String>,
    /// Stable retry key, reused with the exact same request.
    pub request_id: Id,
    /// Explicit compare-and-swap base.
    pub base_revision_id: Id,
    /// All operations commit together.
    pub operations: Vec<Operation>,
}
impl Edit {
    /// Create an operator edit without impersonating the owner.
    pub fn operator(request_id: Id, base_revision_id: Id, operations: Vec<Operation>) -> Self {
        Self {
            author: None,
            request_id,
            base_revision_id,
            operations,
        }
    }
    pub(super) fn valid(&self) -> bool {
        (1..=100).contains(&self.operations.len())
            && self
                .operations
                .iter()
                .try_fold(0usize, |n, op| n.checked_add(op.affected()?))
                .is_some_and(|n| n <= 1000)
    }
}
/// Create a workbook owned through the provider's existing attribution rules.
#[derive(Clone, Debug, Serialize)]
pub struct Create {
    /// Registered author chosen by the operator.
    pub author: String,
    /// Stable retry key.
    pub request_id: Id,
    /// Workbook title.
    pub title: String,
    /// Initial tabs and data.
    pub workbook: Workbook,
}
/// Initial canonical workbook.
#[derive(Clone, Debug, Serialize)]
pub struct Workbook {
    /// Between one and sixteen tabs.
    pub tabs: Vec<NewTab>,
}
impl Create {
    pub(super) fn valid(&self) -> bool {
        (self.author.len() <= 128 && crate::valid_agent(&self.author))
            && !self.title.trim().is_empty()
            && self.title.chars().count() <= 240
            && (1..=16).contains(&self.workbook.tabs.len())
            && self.workbook.tabs.iter().all(NewTab::valid)
            && self
                .workbook
                .tabs
                .iter()
                .map(|t| t.cells.len())
                .sum::<usize>()
                <= 50_000
    }
}
/// Historical selection for a question; never a transient display index.
#[derive(Clone, Debug, Serialize)]
pub struct Anchor {
    /// Observed tab.
    pub tab_id: Id,
    /// Observed selected rows.
    pub row_ids: Vec<Id>,
    /// Observed selected columns.
    pub column_ids: Vec<Id>,
}
/// Why the human is contacting the workbook's registered owner.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackKind {
    /// Ask about the observed values.
    Question,
    /// Suggest or report a change.
    Feedback,
}
/// A durable, idempotent feedback request.
#[derive(Clone, Debug, Serialize)]
pub struct Feedback {
    author: Option<String>,
    /// Stable retry key.
    pub request_id: Id,
    /// Exact revision the user saw.
    pub observed_revision_id: Id,
    /// Question or feedback.
    pub kind: FeedbackKind,
    /// Plain human-authored text.
    pub text: String,
    /// Optional observed selection, at most 100 cells.
    pub anchor: Option<Anchor>,
}
impl Feedback {
    /// Create a human-authored question or feedback message.
    pub fn operator(
        request_id: Id,
        observed_revision_id: Id,
        kind: FeedbackKind,
        text: String,
        anchor: Option<Anchor>,
    ) -> Self {
        Self {
            author: None,
            request_id,
            observed_revision_id,
            kind,
            text,
            anchor,
        }
    }
    pub(super) fn valid(&self) -> bool {
        !self.text.trim().is_empty()
            && self.text.chars().count() <= 4000
            && self.anchor.as_ref().is_none_or(|a| {
                !a.row_ids.is_empty()
                    && !a.column_ids.is_empty()
                    && a.row_ids
                        .len()
                        .checked_mul(a.column_ids.len())
                        .is_some_and(|n| n <= 100)
            })
    }
}
