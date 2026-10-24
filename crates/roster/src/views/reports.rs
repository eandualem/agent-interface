//! Existing Backbone report and Response reads; these never publish or acknowledge.

use serde::Deserialize;

use super::{Api, UNREADABLE, document};

/// A page in one of Backbone's separate authored feeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportQuery {
    /// Latest report per registered author, with owner blockers first.
    Latest(Option<String>),
    /// Report history, newest publication first.
    Recent(Option<String>),
    /// Answers, newest publication first; the ID is exclusive.
    Responses(Option<u64>),
}

impl Default for ReportQuery {
    fn default() -> Self {
        Self::Recent(None)
    }
}

impl ReportQuery {
    fn path(&self) -> String {
        match self {
            Self::Latest(cursor) | Self::Recent(cursor) => {
                let mut path = format!(
                    "/api/reports?limit=20&history={}",
                    matches!(self, Self::Recent(_))
                );
                if let Some(cursor) = cursor {
                    path.push_str("&cursor=");
                    // Cursors are opaque. Encode bytes rather than interpreting them.
                    for byte in cursor.bytes() {
                        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
                            path.push(char::from(byte));
                        } else {
                            use std::fmt::Write;
                            let _ = write!(path, "%{byte:02X}");
                        }
                    }
                }
                path
            }
            Self::Responses(before) => {
                let mut path = "/api/responses?limit=20".to_owned();
                if let Some(id) = before {
                    path.push_str(&format!("&before_id={id}"));
                }
                path
            }
        }
    }
}

/// One authored section; its links are shown only after a user chooses the record.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ReportSection {
    /// The author's text.
    pub text: String,
    /// Titled links supplied by the author. Nothing fetches them automatically.
    #[serde(default)]
    pub links: Vec<ReportLink>,
    /// Present for a report's blocker section only.
    #[serde(default)]
    pub kind: String,
}

/// A reference in an authored report.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ReportLink {
    /// Display title.
    pub title: String,
    /// Authored URL, kept as text.
    pub url: String,
}

/// Authored text, distinct from runtime state or a permission request.
#[derive(Clone, Debug, PartialEq)]
pub enum ReportContent {
    /// Progress report sections in their original order.
    Progress {
        /// Authored active/blocked/complete/inactive status.
        status: String,
        /// Goal, progress, blocker, next and optional note.
        sections: Vec<(&'static str, ReportSection)>,
    },
    /// A retained direct answer. Action is free text, not a pending-state flag.
    Response {
        /// The person's question.
        question: String,
        /// The author's answer.
        answer: String,
        /// What follows from it.
        action: String,
        /// A direct correction, if the server reports one.
        superseded_by: Option<u64>,
    },
}

/// A record's stable identity and authored contents.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportItem {
    /// Publication ID in its feed.
    pub id: u64,
    /// Current agent name, falling back to the original author name.
    pub author: String,
    /// Server publication timestamp.
    pub created_at: String,
    /// The record itself.
    pub content: ReportContent,
}

impl ReportItem {
    /// The row's complete title.
    pub fn title(&self) -> &str {
        match &self.content {
            ReportContent::Progress { sections, .. } => &sections[0].1.text,
            ReportContent::Response { question, .. } => question,
        }
    }

    /// Only use this on the latest-per-author feed to describe current needs.
    pub fn needs_owner(&self) -> bool {
        matches!(&self.content, ReportContent::Progress { sections, .. }
            if sections.iter().any(|(name, section)| *name == "Blocker" && section.kind == "owner"))
    }
}

/// A bounded page, plus the exact next read if more records exist.
#[derive(Clone, Debug, PartialEq)]
pub struct ReportPage {
    /// Records with content; authors with no report are omitted.
    pub items: Vec<ReportItem>,
    /// Continue this same feed without changing its filters.
    pub next: Option<ReportQuery>,
}

#[derive(Deserialize)]
struct Record<T> {
    id: u64,
    author_name: String,
    agent_name: Option<String>,
    created_at: String,
    #[serde(flatten)]
    content: T,
}

#[derive(Deserialize)]
struct Progress {
    report: ProgressText,
}

#[derive(Deserialize)]
struct ProgressText {
    status: String,
    goal: ReportSection,
    progress: ReportSection,
    blockers: ReportSection,
    next: ReportSection,
    note: Option<ReportSection>,
}

#[derive(Deserialize)]
struct Response {
    response: ResponseText,
    superseded_by: Option<u64>,
}

#[derive(Deserialize)]
struct ResponseText {
    question: String,
    answer: String,
    action: String,
}

pub(super) fn read(api: &Api<'_>, query: &ReportQuery) -> Result<ReportPage, &'static str> {
    decode(&api.get(&query.path())?, query)
}

fn decode(body: &[u8], query: &ReportQuery) -> Result<ReportPage, &'static str> {
    let doc = document(body)?;
    let has_more: bool = doc
        .get("has_more")
        .and_then(|value| serde_json::from_str(value.get()).ok())
        .ok_or(UNREADABLE)?;
    let mut items = Vec::new();
    let next = match query {
        ReportQuery::Latest(_) | ReportQuery::Recent(_) => {
            #[derive(Deserialize)]
            struct Entry {
                record: Option<Record<Progress>>,
            }
            for entry in super::items::<Entry>(&doc)? {
                let Some(record) = entry.record else { continue };
                let text = record.content.report;
                let mut sections = vec![
                    ("Goal", text.goal),
                    ("Progress", text.progress),
                    ("Blocker", text.blockers),
                    ("Next", text.next),
                ];
                if let Some(note) = text.note {
                    sections.push(("Note", note));
                }
                items.push(ReportItem {
                    id: record.id,
                    author: record.agent_name.unwrap_or(record.author_name),
                    created_at: record.created_at,
                    content: ReportContent::Progress {
                        status: text.status,
                        sections,
                    },
                });
            }
            if has_more {
                let cursor: String = doc
                    .get("next_cursor")
                    .and_then(|raw| serde_json::from_str(raw.get()).ok())
                    .ok_or(UNREADABLE)?;
                if cursor.is_empty() || cursor.len() > 1024 {
                    return Err(UNREADABLE);
                }
                Some(match query {
                    ReportQuery::Latest(_) => ReportQuery::Latest(Some(cursor)),
                    _ => ReportQuery::Recent(Some(cursor)),
                })
            } else {
                None
            }
        }
        ReportQuery::Responses(_) => {
            for record in super::items::<Record<Response>>(&doc)? {
                let text = record.content.response;
                items.push(ReportItem {
                    id: record.id,
                    author: record.agent_name.unwrap_or(record.author_name),
                    created_at: record.created_at,
                    content: ReportContent::Response {
                        question: text.question,
                        answer: text.answer,
                        action: text.action,
                        superseded_by: record.content.superseded_by,
                    },
                });
            }
            if has_more {
                let id: u64 = doc
                    .get("next_before_id")
                    .and_then(|raw| serde_json::from_str(raw.get()).ok())
                    .ok_or(UNREADABLE)?;
                if id == 0 || id > i64::MAX as u64 {
                    return Err(UNREADABLE);
                }
                Some(ReportQuery::Responses(Some(id)))
            } else {
                None
            }
        }
    };
    Ok(ReportPage { items, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn feeds_keep_owner_needs_answers_and_paging_distinct() {
        let section = |text: &str| json!({"text":text,"links":[]});
        let reports = json!({"items":[{"record":null},{"record":{
            "id":8,"author_name":"worker","agent_name":"renamed","created_at":"2026-10-24T10:00:00Z",
            "report":{"status":"active","goal":section("A goal"),"progress":section("Work continues"),
                "blockers":{"text":"Choose one","links":[],"kind":"owner"},"next":section("Continue"),"note":null}
        }}],"has_more":true,"next_cursor":"opaque+/= value"});
        let page = decode(
            &serde_json::to_vec(&reports).unwrap(),
            &ReportQuery::Latest(None),
        )
        .unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].author, "renamed");
        assert!(page.items[0].needs_owner());
        assert_eq!(
            page.next.unwrap().path(),
            "/api/reports?limit=20&history=false&cursor=opaque%2B%2F%3D%20value"
        );
        let answers = json!({"items":[{"id":7,"author_name":"worker","agent_name":null,
            "created_at":"2026-10-24T09:00:00Z","response":{"question":"What happened?","answer":"Done.","action":"Choose one"},
            "superseded_by":9}],"has_more":true,"next_before_id":7});
        let page = decode(
            &serde_json::to_vec(&answers).unwrap(),
            &ReportQuery::Responses(None),
        )
        .unwrap();
        assert_eq!(page.items[0].author, "worker");
        assert!(!page.items[0].needs_owner());
        assert_eq!(
            page.next.unwrap().path(),
            "/api/responses?limit=20&before_id=7"
        );
    }
}
