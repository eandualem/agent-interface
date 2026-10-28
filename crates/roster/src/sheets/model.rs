//! Typed wire records. IDs cannot become arbitrary URL paths. Cell results are
//! supplied by Backbone; this module never evaluates formulas.

use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashSet, fmt};

/// An opaque, validated lowercase UUID, without separators.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Id(String);
impl Id {
    /// Validate an externally supplied identifier.
    pub fn parse(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (value.len() == 32
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then_some(Self(value))
    }
}
impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(d)?)
            .ok_or_else(|| serde::de::Error::custom("invalid sheet identifier"))
    }
}

/// An explicit input type; text beginning with '=' is still text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Input {
    /// Literal UTF-8 text.
    Text(String),
    /// A finite number.
    Number(f64),
    /// A literal boolean.
    Boolean(bool),
    /// ISO calendar date, independently of display formatting.
    Date(String),
    /// An A1 expression including its leading '='.
    Formula(String),
}
impl Input {
    pub(super) fn valid(&self) -> bool {
        match self {
            Self::Text(s) => s.len() <= 16 * 1024,
            Self::Number(n) => n.is_finite(),
            Self::Boolean(_) => true,
            Self::Date(s) => valid_date(s),
            Self::Formula(s) => {
                s.strip_prefix('=').is_some_and(|v| !v.trim().is_empty()) && s.len() <= 4096
            }
        }
    }
    /// Original input, for the formula bar rather than the rounded display.
    pub fn text(&self) -> String {
        match self {
            Self::Text(s) | Self::Date(s) | Self::Formula(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Boolean(b) => b.to_string(),
        }
    }
}
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, b)| i != 4 && i != 7 && !b.is_ascii_digit())
    {
        return false;
    }
    let year: u32 = s[..4].parse().unwrap_or(0);
    let month: u32 = s[5..7].parse().unwrap_or(0);
    let day: u32 = s[8..].parse().unwrap_or(0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    };
    year >= 1900 && day > 0 && day <= days
}

/// The provider's stored, typed calculation result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Value {
    /// A true blank, encoded with a null value.
    Blank(()),
    /// Text, bounded separately from the input.
    Text(String),
    /// A finite result, including formatted date serials.
    Number(f64),
    /// Boolean result.
    Boolean(bool),
    /// ISO date literal result.
    Date(String),
    /// An explicit calculation error, never an invented zero.
    Error(String),
}
impl Value {
    fn valid(&self) -> bool {
        match self {
            Self::Blank(()) | Self::Boolean(_) => true,
            Self::Number(n) => n.is_finite(),
            Self::Date(s) => valid_date(s),
            Self::Text(s) | Self::Error(s) => s.len() <= 64 * 1024,
        }
    }
}

/// A saved view changes presentation, not canonical formula coordinates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct View {
    /// Header pinned before filtered and sorted rows.
    pub header_row_id: Option<Id>,
    /// Ordered sort keys.
    pub sort: Vec<Sort>,
    /// AND-combined filters.
    pub filters: Vec<Filter>,
}
/// A typed sort key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sort {
    /// Stable column identifier.
    pub column_id: Id,
    /// Ascending or descending result order.
    pub direction: Direction,
}
/// Saved-view sort direction.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Ascending.
    Asc,
    /// Descending.
    Desc,
}
/// A filter compares calculated values, never display strings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    /// Stable column identifier.
    pub column_id: Id,
    /// Comparison operation.
    pub operator: Operator,
    /// Typed literal; absent for blank tests. Formulas are not literals.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Input>,
}
/// The provider's supported AND-filter operators.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    /// Typed equality.
    Eq,
    /// Typed inequality.
    Ne,
    /// Less than.
    Lt,
    /// Less than or equal.
    Lte,
    /// Greater than.
    Gt,
    /// Greater than or equal.
    Gte,
    /// Case-folded text containment.
    Contains,
    /// True blank.
    IsBlank,
    /// Nonblank.
    IsNotBlank,
}
impl View {
    pub(super) fn valid(&self) -> bool {
        self.sort.len() <= 256
            && self.filters.len() <= 256
            && self.filters.iter().all(|f| {
                if matches!(f.operator, Operator::IsBlank | Operator::IsNotBlank) {
                    f.value.is_none()
                } else {
                    f.value.as_ref().is_some_and(|v| {
                        v.valid()
                            && !matches!(v, Input::Formula(_))
                            && (f.operator != Operator::Contains || matches!(v, Input::Text(_)))
                    })
                }
            })
    }
}

/// A tab's dimensions and saved view at one revision.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Tab {
    /// Immutable identifier.
    pub tab_id: Id,
    /// User-visible name.
    pub name: String,
    /// Canonical rows, including hidden rows.
    pub row_count: usize,
    /// Canonical columns.
    pub column_count: usize,
    /// Saved sort and filter configuration.
    pub view: View,
}
/// Immutable revision metadata with a separately reported current pointer.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Record {
    /// Workbook identity.
    pub sheet_id: Id,
    /// Human-readable title.
    pub title: String,
    /// Stable registered owner.
    pub author_id: String,
    /// Historical author name.
    pub author_name: String,
    /// Current owner name, absent if registration no longer exists.
    pub agent_name: Option<String>,
    /// Current revision as observed during this read.
    pub current_revision_id: Id,
    /// Exact revision whose tabs/results are being read.
    pub revision_id: Id,
    /// Previous immutable revision, absent at creation.
    pub parent_revision_id: Option<Id>,
    /// Workbook creation time.
    pub created_at: String,
    /// This revision's creation time.
    pub updated_at: String,
    /// Agent or operator attribution.
    pub editor_kind: String,
    /// Stable editor identity, if present.
    pub editor_id: Option<String>,
    /// Human-readable editor attribution.
    pub editor_name: Option<String>,
    /// Provider engine name; the client does not calculate.
    pub calculation_engine: String,
    /// Engine version used for this immutable snapshot.
    pub calculation_version: String,
    /// Ordered tabs.
    pub tabs: Vec<Tab>,
    /// Provider's snapshot digest.
    pub content_sha256: String,
}
impl Record {
    pub(super) fn valid(&self) -> bool {
        let mut ids = HashSet::new();
        self.title.chars().count() <= 240
            && !self.title.trim().is_empty()
            && (1..=16).contains(&self.tabs.len())
            && self.tabs.iter().all(|t| {
                ids.insert(&t.tab_id)
                    && (1..=10_000).contains(&t.row_count)
                    && (1..=256).contains(&t.column_count)
                    && !t.name.is_empty()
                    && t.name.chars().count() <= 31
                    && t.view.valid()
            })
            && self.content_sha256.len() == 64
            && self.content_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            && [
                &self.author_id,
                &self.author_name,
                &self.created_at,
                &self.updated_at,
                &self.editor_kind,
                &self.calculation_engine,
                &self.calculation_version,
            ]
            .iter()
            .all(|s| s.len() <= 512)
            && [&self.agent_name, &self.editor_id, &self.editor_name]
                .iter()
                .all(|s| s.as_ref().is_none_or(|s| s.len() <= 512))
    }
}
/// Discovery or history page; cursors are opaque and query-specific.
#[derive(Clone, Debug, Deserialize)]
pub struct Page {
    /// At most the requested number of revisions.
    pub items: Vec<Record>,
    /// Next page, absent at the end.
    pub next_cursor: Option<String>,
}
/// Position of a row within the canonical workbook, even in sorted views.
#[derive(Clone, Debug, Deserialize)]
pub struct Row {
    /// Stable row identity.
    pub row_id: Id,
    /// One-based canonical index.
    pub index: usize,
}
/// Position of a column within the canonical workbook.
#[derive(Clone, Debug, Deserialize)]
pub struct Column {
    /// Stable column identity.
    pub column_id: Id,
    /// One-based canonical index.
    pub index: usize,
}
/// One nonblank or formatted cell.
#[derive(Clone, Debug, Deserialize)]
pub struct Cell {
    /// Stable row identity.
    pub row_id: Id,
    /// Stable column identity.
    pub column_id: Id,
    /// Original typed input; absent for a formatted blank.
    pub input: Option<Input>,
    /// Immutable calculated result.
    pub value: Value,
    /// Plain display text supplied by the engine.
    pub display: String,
    /// Optional number/date formatting.
    pub number_format: Option<String>,
}
/// Requested row ordering.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    /// Canonical formula order.
    Canonical,
    /// Saved sort and filter order, with canonical indices retained.
    View,
}
/// A coherent rectangular page pinned to one provider, revision and tab.
#[derive(Clone, Debug, Deserialize)]
pub struct Grid {
    /// Workbook identity.
    pub sheet_id: Id,
    /// Immutable revision identity.
    pub revision_id: Id,
    /// Stable tab identity.
    pub tab_id: Id,
    /// Actual returned row ordering.
    pub order: Order,
    /// Total rows in this order, including a pinned header.
    pub total_rows: usize,
    /// Total canonical columns.
    pub total_columns: usize,
    /// Every selected row, even when all cells are blank.
    pub rows: Vec<Row>,
    /// Every selected column.
    pub columns: Vec<Column>,
    /// Sparse cells; omitted coordinates are truly blank and unformatted.
    pub cells: Vec<Cell>,
    /// Continuation offset in the same order and revision.
    pub next_row_offset: Option<usize>,
}
impl Grid {
    /// Read a coordinate without inventing a cell for an absent blank.
    pub fn cell(&self, row: &Id, column: &Id) -> Option<&Cell> {
        self.cells
            .iter()
            .find(|c| &c.row_id == row && &c.column_id == column)
    }
    pub(super) fn valid(&self) -> bool {
        let rows: HashSet<_> = self.rows.iter().map(|r| &r.row_id).collect();
        let columns: HashSet<_> = self.columns.iter().map(|c| &c.column_id).collect();
        let mut coords = HashSet::new();
        let mut row_indices = HashSet::new();
        let mut column_indices = HashSet::new();
        self.total_rows <= 10_000
            && (1..=256).contains(&self.total_columns)
            && self.rows.len() <= 200
            && self.columns.len() <= 100
            && self.rows.len() * self.columns.len() <= 2000
            && rows.len() == self.rows.len()
            && columns.len() == self.columns.len()
            && self
                .rows
                .iter()
                .all(|r| (1..=10_000).contains(&r.index) && row_indices.insert(r.index))
            && self
                .columns
                .iter()
                .all(|c| (1..=256).contains(&c.index) && column_indices.insert(c.index))
            && self.cells.iter().all(|c| {
                rows.contains(&c.row_id)
                    && columns.contains(&c.column_id)
                    && coords.insert((&c.row_id, &c.column_id))
                    && c.input.as_ref().is_none_or(Input::valid)
                    && c.value.valid()
                    && c.display.len() <= 64 * 1024
                    && valid_format(c.number_format.as_deref())
            })
    }
}
pub(super) fn valid_format(s: Option<&str>) -> bool {
    s.is_none_or(|s| s.chars().count() <= 128 && !s.chars().any(char::is_control))
}
