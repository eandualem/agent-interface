//! Stable-cell feedback. A prepared request retains its exact bytes and retry
//! key, including when the first reply is lost.

use super::Id;
use serde::Serialize;

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
