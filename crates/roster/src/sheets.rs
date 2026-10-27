//! Retained workbooks, edited through Backbone's authenticated local API.
//!
//! Reads pin immutable revisions. Writes are explicit, atomic operator actions;
//! a lost reply retains a byte-identical request for a deliberate safe retry.
//! Transport, parsing and validation run on I/O workers, never the UI thread.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

mod edit;
mod model;
#[cfg(test)]
mod tests;
pub use edit::*;
pub use model::*;

const METADATA_LIMIT: usize = 1024 * 1024;
const RANGE_LIMIT: usize = 4 * 1024 * 1024;
const FILE_LIMIT: usize = 16 * 1024 * 1024;

/// A fixed explanation without leaking response bodies or credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Configuration no longer names the provider whose data was displayed.
    ProviderChanged,
    /// No authenticated private local provider is configured.
    Unavailable,
    /// Invalid or oversized client input, before any request.
    Invalid,
    /// Response violates the bounded wire contract.
    Unreadable,
    /// A newer revision exists. Pending edits must be kept.
    Stale {
        /// Provider's current revision; no automatic rebase is permitted.
        current: Id,
    },
    /// The server has already seen this key with different content.
    RequestConflict,
    /// An explicit provider refusal. No response prose is rendered.
    Refused(u16),
    /// A range page must shrink at the same revision.
    RangeTooLarge,
    /// A metadata page exceeds the provider's response budget.
    ResponseTooLarge,
    /// A mutation may have committed. Retry only the saved exact request.
    Unknown,
}
impl Error {
    /// Safe, actionable user-facing message.
    pub fn message(&self) -> &'static str {
        match self {
            Self::ProviderChanged => {
                "The Backbone connection changed. Keep this draft and reconnect to its original provider."
            }
            Self::Unavailable => "Sheets requires an authenticated private Backbone connection.",
            Self::Invalid => {
                "This request is invalid or exceeds the supported size. The draft is unchanged."
            }
            Self::Unreadable => {
                "Backbone returned an unreadable Sheets response. Existing data is unchanged."
            }
            Self::Stale { .. } => {
                "A newer revision exists. Your draft is retained; review the new revision before explicitly reapplying it."
            }
            Self::RequestConflict => {
                "This retry key already belongs to different content. Keep the draft and inspect the original operation."
            }
            Self::Refused(404) => {
                "This workbook is unavailable, or Backbone needs updating to support Sheets."
            }
            Self::Refused(401 | 403) => {
                "Backbone refused this operation with the current identity."
            }
            Self::Refused(413) | Self::RangeTooLarge | Self::ResponseTooLarge => {
                "This data exceeds the supported size. The draft is unchanged."
            }
            Self::Refused(422) => {
                "Backbone could not validate or calculate this change. The draft is unchanged."
            }
            Self::Refused(503) => {
                "Backbone is busy calculating another workbook. Keep the draft and retry later."
            }
            Self::Refused(507) => {
                "Retained Sheets storage is full. Existing history and this draft are unchanged."
            }
            Self::Refused(_) => "Backbone refused this operation. The draft is unchanged.",
            Self::Unknown => {
                "The reply was lost or unreadable. The change may have committed. Retry the same saved request to learn its outcome."
            }
        }
    }
}

/// A connection bound to the provider that supplied the displayed workbook.
#[derive(Clone, Debug)]
pub struct Client {
    directory: PathBuf,
    provider: String,
}
impl Client {
    /// Resolve the existing configuration on a worker; no new credentials.
    pub fn connect(directory: &Path) -> Result<Self, Error> {
        let (provider, key) =
            crate::configuration::configured(directory, None).ok_or(Error::Unavailable)?;
        if !provider.starts_with("unix:") || key.is_empty() {
            return Err(Error::Unavailable);
        }
        Ok(Self {
            directory: directory.into(),
            provider,
        })
    }
    fn request(&self, path: &str, body: Option<Vec<u8>>, limit: usize) -> Result<Vec<u8>, Error> {
        self.response(path, body, limit)
            .map(|response| response.body)
    }
    fn response(
        &self,
        path: &str,
        body: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<crate::transport::local::Response, Error> {
        let (provider, key) =
            crate::configuration::configured(&self.directory, None).ok_or(Error::Unavailable)?;
        if provider != self.provider {
            return Err(Error::ProviderChanged);
        }
        if key.is_empty() {
            return Err(Error::Unavailable);
        }
        let mutation = body.is_some();
        let result = crate::transport::local::response(
            provider.strip_prefix("unix:").ok_or(Error::Unavailable)?,
            path,
            &key,
            body,
            limit,
            Duration::from_secs(if mutation { 20 } else { 15 }),
        )
        .map_err(|_| {
            if mutation {
                Error::Unknown
            } else {
                Error::Unavailable
            }
        })?;
        if result.body.len() > limit {
            return Err(if mutation {
                Error::Unknown
            } else {
                Error::Unreadable
            });
        }
        if (200..300).contains(&result.status) {
            return Ok(result);
        }
        let detail: Option<Failure> = serde_json::from_slice::<FailureEnvelope>(&result.body)
            .ok()
            .map(|e| e.detail);
        Err(match (result.status, detail) {
            (409, Some(d)) if d.code == "stale_revision" => d
                .current_revision_id
                .map(|current| Error::Stale { current })
                .unwrap_or(Error::Refused(409)),
            (409, Some(d)) if d.code == "request_conflict" => Error::RequestConflict,
            (413, Some(d)) if d.code == "range_too_large" => Error::RangeTooLarge,
            (413, Some(d)) if d.code == "response_too_large" => Error::ResponseTooLarge,
            (500..=599, _) if mutation && !matches!(result.status, 503 | 507) => Error::Unknown,
            (status, _) => Error::Refused(status),
        })
    }
    fn read<T: DeserializeOwned>(&self, path: &str, limit: usize) -> Result<T, Error> {
        serde_json::from_slice(&self.request(path, None, limit)?).map_err(|_| Error::Unreadable)
    }
    /// List latest workbooks, optionally by stable author identity.
    pub fn list(&self, author_id: Option<&str>, cursor: Option<&str>) -> Result<Page, Error> {
        if author_id.is_some_and(|id| Id::parse(id).is_none()) {
            return Err(Error::Invalid);
        }
        self.page("/api/sheets", author_id, cursor)
    }
    fn page(
        &self,
        base: &str,
        author_id: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page, Error> {
        let mut limit: usize = 50;
        loop {
            let mut path = page_path(base, cursor, limit)?;
            if let Some(author) = author_id {
                path.push_str("&author_id=");
                path.push_str(&encode(author));
            }
            let page: Page = match self.read(&path, METADATA_LIMIT) {
                Err(Error::ResponseTooLarge) if limit > 1 => {
                    limit = limit.div_ceil(2);
                    continue;
                }
                result => result?,
            };
            if page.items.len() > limit
                || !page.items.iter().all(Record::valid)
                || page
                    .next_cursor
                    .as_ref()
                    .is_some_and(|c| c.chars().count() > 512)
            {
                return Err(Error::Unreadable);
            }
            return Ok(page);
        }
    }
    /// Read metadata at the latest or an explicitly selected historical revision.
    pub fn record(&self, sheet: &Id, revision: Option<&Id>) -> Result<Record, Error> {
        let mut path = format!("/api/sheets/{sheet}");
        if let Some(revision) = revision {
            path.push_str(&format!("?revision_id={revision}"));
        }
        let record: Record = self.read(&path, METADATA_LIMIT)?;
        if !record.valid()
            || &record.sheet_id != sheet
            || revision.is_some_and(|r| r != &record.revision_id)
        {
            return Err(Error::Unreadable);
        }
        Ok(record)
    }
    /// Descending immutable revision history.
    pub fn history(&self, sheet: &Id, cursor: Option<&str>) -> Result<Page, Error> {
        let page = self.page(&format!("/api/sheets/{sheet}/revisions"), None, cursor)?;
        if page.items.iter().any(|r| &r.sheet_id != sheet) {
            return Err(Error::Unreadable);
        }
        Ok(page)
    }
    /// Read one bounded page. A size refusal shrinks the page without changing
    /// its revision, tab, order or offsets; transport failures are not retried.
    pub fn range(&self, query: &RangeQuery) -> Result<Grid, Error> {
        if !(1..=200).contains(&query.row_limit)
            || !(1..=100).contains(&query.column_limit)
            || query.row_limit * query.column_limit > 2000
            || query.row_offset > 10_000
            || query.column_offset > 256
        {
            return Err(Error::Invalid);
        }
        let mut query = query.clone();
        loop {
            let order = match query.order {
                Order::View => "view",
                Order::Canonical => "canonical",
            };
            let path = format!(
                "/api/sheets/{}/tabs/{}/range?revision_id={}&row_offset={}&row_limit={}&column_offset={}&column_limit={}&order={order}",
                query.sheet_id,
                query.tab_id,
                query.revision_id,
                query.row_offset,
                query.row_limit,
                query.column_offset,
                query.column_limit
            );
            match self.read::<Grid>(&path, RANGE_LIMIT) {
                Err(Error::RangeTooLarge) if query.row_limit > 1 || query.column_limit > 1 => {
                    if query.row_limit >= query.column_limit && query.row_limit > 1 {
                        query.row_limit = query.row_limit.div_ceil(2);
                    } else {
                        query.column_limit = query.column_limit.div_ceil(2);
                    }
                }
                Ok(grid) => {
                    let remaining_rows = grid.total_rows.saturating_sub(query.row_offset);
                    let remaining_columns = grid.total_columns.saturating_sub(query.column_offset);
                    let next = query.row_offset + grid.rows.len();
                    if !grid.valid()
                        || grid.sheet_id != query.sheet_id
                        || grid.revision_id != query.revision_id
                        || grid.tab_id != query.tab_id
                        || grid.order != query.order
                        || grid.rows.len() != query.row_limit.min(remaining_rows)
                        || grid.columns.len() != query.column_limit.min(remaining_columns)
                        || grid.next_row_offset != (next < grid.total_rows).then_some(next)
                        || grid
                            .columns
                            .iter()
                            .enumerate()
                            .any(|(i, c)| c.index != query.column_offset + i + 1)
                        || (query.order == Order::Canonical
                            && grid
                                .rows
                                .iter()
                                .enumerate()
                                .any(|(i, r)| r.index != query.row_offset + i + 1))
                    {
                        return Err(Error::Unreadable);
                    }
                    return Ok(grid);
                }
                Err(error) => return Err(error),
            }
        }
    }
    /// Freeze a validated human edit before sending it.
    pub fn prepare_edit(&self, sheet: &Id, edit: &Edit) -> Result<Prepared, Error> {
        if !edit.valid() {
            return Err(Error::Invalid);
        }
        self.prepare(
            format!("/api/sheets/{sheet}/revisions"),
            edit,
            METADATA_LIMIT,
            Expected::Revision {
                sheet: sheet.clone(),
                base: edit.base_revision_id.clone(),
            },
        )
    }
    /// Freeze initial workbook creation with the owner's explicit attribution.
    pub fn prepare_create(&self, create: &Create) -> Result<Prepared, Error> {
        if !create.valid() {
            return Err(Error::Invalid);
        }
        self.prepare("/api/sheets".into(), create, FILE_LIMIT, Expected::Create)
    }
    /// Import a new workbook; existing workbooks are never replaced by import.
    pub fn prepare_import(&self, import: &Import) -> Result<Prepared, Error> {
        if import.author.len() > 128
            || !crate::valid_agent(&import.author)
            || import.title.trim().is_empty()
            || import.title.chars().count() > 240
        {
            return Err(Error::Invalid);
        }
        self.prepare(
            "/api/sheets/import".into(),
            import,
            FILE_LIMIT,
            Expected::Import,
        )
    }
    /// Freeze feedback at its observed revision and stable selection.
    pub fn prepare_feedback(&self, sheet: &Id, feedback: &Feedback) -> Result<Prepared, Error> {
        if !feedback.valid() {
            return Err(Error::Invalid);
        }
        self.prepare(
            format!("/api/sheets/{sheet}/feedback"),
            feedback,
            METADATA_LIMIT,
            Expected::Feedback {
                sheet: sheet.clone(),
                revision: feedback.observed_revision_id.clone(),
            },
        )
    }
    fn prepare(
        &self,
        path: String,
        body: &impl Serialize,
        limit: usize,
        expected: Expected,
    ) -> Result<Prepared, Error> {
        let body = serde_json::to_vec(body).map_err(|_| Error::Invalid)?;
        if body.len() > limit {
            return Err(Error::Invalid);
        }
        Ok(Prepared {
            provider: self.provider.clone(),
            path,
            body,
            expected,
        })
    }
    /// Send exactly once. An explicit retry calls this again with the same
    /// prepared value, not a freshly constructed edit or feedback key.
    pub fn submit(&self, request: &Prepared) -> Result<Accepted, Error> {
        if request.provider != self.provider {
            return Err(Error::ProviderChanged);
        }
        let bytes = self.request(&request.path, Some(request.body.clone()), METADATA_LIMIT)?;
        match &request.expected {
            Expected::Feedback { sheet, revision } => {
                let result: FeedbackReceipt =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Unknown)?;
                if &result.sheet_id != sheet
                    || &result.observed_revision_id != revision
                    || result.operation_id.len() > 512
                    || result.delivery.outcome.len() > 100
                {
                    return Err(Error::Unknown);
                }
                Ok(Accepted::Feedback(result))
            }
            Expected::Import => {
                let result: Imported =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Unknown)?;
                if !result.record.valid() || !valid_warnings(&result.warnings) {
                    return Err(Error::Unknown);
                }
                Ok(Accepted::Imported {
                    record: result.record,
                    warnings: result.warnings,
                })
            }
            expected => {
                let result: Mutation =
                    serde_json::from_slice(&bytes).map_err(|_| Error::Unknown)?;
                if !result.record.valid() {
                    return Err(Error::Unknown);
                }
                if let Expected::Revision { sheet, base } = expected
                    && (&result.record.sheet_id != sheet
                        || result.record.parent_revision_id.as_ref() != Some(base))
                {
                    return Err(Error::Unknown);
                }
                Ok(Accepted::Revision(result.record))
            }
        }
    }
    /// Export one immutable revision. CSV/TSV require a specific tab and contain
    /// values only; JSON/XLSX cover the provider's supported workbook features.
    pub fn export(
        &self,
        sheet: &Id,
        revision: &Id,
        format: Format,
        tab: Option<&Id>,
    ) -> Result<Exported, Error> {
        if matches!(format, Format::Csv | Format::Tsv) && tab.is_none() {
            return Err(Error::Invalid);
        }
        let mut path = format!(
            "/api/sheets/{sheet}/export?revision_id={revision}&format={}",
            format.name()
        );
        if let Some(tab) = tab {
            path.push_str(&format!("&tab_id={tab}"));
        }
        let response = self.response(&path, None, FILE_LIMIT)?;
        let warnings = match response.sheet_warnings {
            Some(header) if header.len() <= 4096 => {
                serde_json::from_slice::<Vec<String>>(&header).map_err(|_| Error::Unreadable)?
            }
            Some(_) => return Err(Error::Unreadable),
            None => Vec::new(),
        };
        if !valid_warnings(&warnings) {
            return Err(Error::Unreadable);
        }
        Ok(Exported {
            bytes: response.body,
            warnings,
        })
    }
}
#[derive(Deserialize)]
struct FailureEnvelope {
    detail: Failure,
}
#[derive(Deserialize)]
struct Failure {
    code: String,
    current_revision_id: Option<Id>,
}
#[derive(Deserialize)]
struct Mutation {
    record: Record,
}
fn page_path(base: &str, cursor: Option<&str>, limit: usize) -> Result<String, Error> {
    let mut path = format!("{base}?limit={limit}");
    if let Some(cursor) = cursor {
        if cursor.chars().count() > 512 {
            return Err(Error::Invalid);
        }
        path.push_str("&cursor=");
        path.push_str(&encode(cursor));
    }
    Ok(path)
}
fn encode(s: &str) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}
/// All coordinates needed to read one immutable page.
#[derive(Clone, Debug)]
pub struct RangeQuery {
    /// Workbook identity.
    pub sheet_id: Id,
    /// Immutable revision.
    pub revision_id: Id,
    /// Tab identity.
    pub tab_id: Id,
    /// Offset in the requested row order.
    pub row_offset: usize,
    /// At most 200 rows.
    pub row_limit: usize,
    /// Offset in canonical column order.
    pub column_offset: usize,
    /// At most 100 columns; rows times columns at most 2000.
    pub column_limit: usize,
    /// Canonical or saved-view order.
    pub order: Order,
}
#[derive(Clone, Debug)]
enum Expected {
    Create,
    Import,
    Revision { sheet: Id, base: Id },
    Feedback { sheet: Id, revision: Id },
}
/// Immutable request, retained while its outcome is uncertain. Bytes and route
/// cannot be edited; a caller must explicitly prepare a different operation.
#[derive(Clone, Debug)]
pub struct Prepared {
    provider: String,
    path: String,
    body: Vec<u8>,
    expected: Expected,
}
/// A confirmed mutation response.
#[derive(Clone, Debug)]
pub enum Accepted {
    /// The committed immutable revision (which may since have been superseded).
    Revision(Record),
    /// Imported workbook and explicit interchange loss diagnostics.
    Imported {
        /// New immutable workbook revision.
        record: Record,
        /// Provider-supplied bounded diagnostics, never discarded.
        warnings: Vec<String>,
    },
    /// Actual feedback routing receipt, not an agent answer.
    Feedback(FeedbackReceipt),
}
/// Bounded existing ordinary-message receipt; it does not prove an answer.
#[derive(Clone, Debug, Deserialize)]
pub struct Delivery {
    /// Whether the immediate route succeeded.
    pub ok: bool,
    /// Whether the message is stored for later delivery.
    #[serde(default)]
    pub queued: bool,
    /// Provider's actual route state.
    pub outcome: String,
}
impl Delivery {
    /// Describe the actual feedback route without implying an agent answer.
    pub fn message(&self) -> &'static str {
        match self.outcome.as_str() {
            "delivered" if self.ok => {
                "Feedback delivered to the agent. An answer has not been received."
            }
            "uncertain" => {
                "Feedback retained; delivery is uncertain. Check its status before sending another copy."
            }
            "expired" => "Feedback retained; its delivery attempt expired.",
            "unknown" => "Feedback retained; delivery status is unknown.",
            "queued" if self.queued => "Feedback retained and queued for the agent.",
            _ => "Feedback retained; delivery needs checking.",
        }
    }
}
/// Durable contextual feedback result.
#[derive(Clone, Debug, Deserialize)]
pub struct FeedbackReceipt {
    /// Stable feedback identity.
    pub feedback_id: Id,
    /// Workbook identity.
    pub sheet_id: Id,
    /// Revision observed by the person who sent it.
    pub observed_revision_id: Id,
    /// Delivery operation identity.
    pub operation_id: String,
    /// Actual existing message receipt.
    pub delivery: Delivery,
}
/// Interchange formats supported by the provider.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    /// Native typed workbook backup.
    Json,
    /// Values only, one tab.
    Csv,
    /// Values only, one tab.
    Tsv,
    /// Supported Excel workbook features, with explicit loss diagnostics.
    Xlsx,
}
impl Format {
    /// File extension and protocol name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Xlsx => "xlsx",
        }
    }
}

/// Import payload. Binary XLSX content is standard base64; other formats are UTF-8.
#[derive(Clone, Debug, Serialize)]
pub struct Import {
    /// Registered owner selected by the operator.
    pub author: String,
    /// Stable idempotency key.
    pub request_id: Id,
    /// New workbook title.
    pub title: String,
    /// Explicit format, never inferred by the server from arbitrary content.
    pub format: Format,
    /// Bounded file content in the format's wire representation.
    pub content: String,
}
#[derive(Deserialize)]
struct Imported {
    record: Record,
    warnings: Vec<String>,
}
/// A bounded immutable export, including diagnostics that must be shown.
pub struct Exported {
    /// File bytes, at most 16 MiB.
    pub bytes: Vec<u8>,
    /// Explicit supported-feature/loss diagnostics.
    pub warnings: Vec<String>,
}
fn valid_warnings(warnings: &[String]) -> bool {
    // ensure_ascii JSON adds six bytes per non-ASCII UTF-16 code unit.
    let encoded = 2
        + warnings.len().saturating_sub(1) * 2
        + warnings
            .iter()
            .map(|s| {
                2 + s
                    .chars()
                    .map(|ch| {
                        if ch == '"' || ch == '\\' {
                            2
                        } else if ch.is_ascii() {
                            1
                        } else {
                            ch.len_utf16() * 6
                        }
                    })
                    .sum::<usize>()
            })
            .sum::<usize>();
    warnings.len() <= 8
        && warnings
            .iter()
            .all(|s| s.len() <= 512 && !s.chars().any(char::is_control))
        && encoded <= 4096
}
