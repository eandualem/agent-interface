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
    /// Latest revision per durable document, optionally by stable author.
    Documents(Option<u64>, Option<String>),
}

impl Default for ReportQuery {
    fn default() -> Self {
        Self::Recent(None)
    }
}

impl ReportQuery {
    fn path(&self, agent: Option<&str>) -> String {
        let mut path = match self {
            Self::Latest(cursor) | Self::Recent(cursor) => {
                let mut path = format!(
                    "/api/reports?limit=20&history={}",
                    matches!(self, Self::Recent(_))
                );
                if let Some(cursor) = cursor {
                    path.push_str("&cursor=");
                    path.push_str(&encode(cursor));
                }
                path
            }
            Self::Documents(..) => unreachable!("documents have their own endpoint"),
            Self::Responses(before) => {
                let mut path = "/api/responses?limit=20".to_owned();
                if let Some(id) = before {
                    path.push_str(&format!("&before_id={id}"));
                }
                path
            }
        };
        if let Some(agent) = agent {
            path.push_str("&agent=");
            path.push_str(&encode(agent));
        }
        path
    }
}

pub(super) fn encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
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
    /// Metadata only; HTML is fetched on explicit open.
    Document(super::DocumentMetadata),
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
    /// Stable author identity used to validate feedback routing.
    pub author_id: Option<String>,
    /// Provider address from which this record was read.
    pub provider_url: String,
    /// Server publication timestamp.
    pub created_at: String,
    /// Reports and answers: the root of the correction chain, which keys the
    /// record's conversation. `None` from a provider without conversations.
    pub conversation_id: Option<String>,
    /// The record itself.
    pub content: ReportContent,
}

impl ReportItem {
    /// The row's complete title.
    pub fn title(&self) -> &str {
        match &self.content {
            ReportContent::Document(metadata) => &metadata.title,
            ReportContent::Progress { sections, .. } => &sections[0].1.text,
            ReportContent::Response { question, .. } => question,
        }
    }

    /// The conversation about this record, when the provider names one.
    pub fn conversation(&self) -> Option<(crate::conversations::Kind, String)> {
        use crate::conversations::Kind;
        match &self.content {
            ReportContent::Document(metadata) => {
                Some((Kind::Document, metadata.document_id.clone()))
            }
            ReportContent::Progress { .. } => Some((Kind::Report, self.conversation_id.clone()?)),
            ReportContent::Response { .. } => Some((Kind::Response, self.conversation_id.clone()?)),
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
    /// Configured agents and this page's authors, for choosing a feed filter.
    pub agents: Vec<String>,
}

#[derive(Deserialize)]
struct Record<T> {
    id: u64,
    author_name: String,
    #[serde(default)]
    author_id: Option<String>,
    agent_name: Option<String>,
    created_at: String,
    #[serde(default, deserialize_with = "conversation_id")]
    conversation_id: Option<String>,
    #[serde(flatten)]
    content: T,
}

/// A chain root as the provider writes it, a number or a string.
fn conversation_id<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Id {
        Number(u64),
        Text(String),
    }
    Ok(match Option::<Id>::deserialize(deserializer)? {
        Some(Id::Number(id)) => Some(id.to_string()),
        Some(Id::Text(id)) => Some(id),
        None => None,
    })
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

pub(super) fn read(
    api: &Api<'_>,
    query: &ReportQuery,
    agent: Option<&str>,
) -> Result<ReportPage, &'static str> {
    if let ReportQuery::Documents(before, author) = query {
        return super::documents::read_list(api, *before, author.as_deref(), agent);
    }
    let mut page = decode(&api.get(&query.path(agent))?, query)?;
    // An unavailable roster must not prevent reading reports. Authors on the
    // current page still provide choices, including retained historical names.
    if let Ok(agents) = super::library::agents(api) {
        page.agents
            .extend(agents.into_iter().map(|agent| agent.name));
    }
    page.agents.sort_unstable();
    page.agents.dedup();
    Ok(page)
}

pub(super) fn read_response(api: &Api<'_>, id: u64) -> Result<ReportPage, &'static str> {
    decode_response(&api.get(&format!("/api/responses/{id}"))?)
}

fn response_item(record: Record<Response>) -> ReportItem {
    let text = record.content.response;
    ReportItem {
        id: record.id,
        author: record.agent_name.unwrap_or(record.author_name),
        author_id: record.author_id,
        provider_url: String::new(),
        created_at: record.created_at,
        conversation_id: record.conversation_id,
        content: ReportContent::Response {
            question: text.question,
            answer: text.answer,
            action: text.action,
            superseded_by: record.content.superseded_by,
        },
    }
}

fn decode_response(body: &[u8]) -> Result<ReportPage, &'static str> {
    document(body)?;
    let record = serde_json::from_slice(body).map_err(|_| UNREADABLE)?;
    Ok(ReportPage {
        items: vec![response_item(record)],
        next: None,
        agents: Vec::new(),
    })
}

fn decode(body: &[u8], query: &ReportQuery) -> Result<ReportPage, &'static str> {
    let doc = document(body)?;
    let has_more: bool = doc
        .get("has_more")
        .and_then(|value| serde_json::from_str(value.get()).ok())
        .ok_or(UNREADABLE)?;
    let mut items = Vec::new();
    let next = match query {
        ReportQuery::Documents(..) => return Err(UNREADABLE),
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
                    author_id: record.author_id,
                    provider_url: String::new(),
                    created_at: record.created_at,
                    conversation_id: record.conversation_id,
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
                items.push(response_item(record));
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
    let agents = items.iter().map(|item| item.author.clone()).collect();
    Ok(ReportPage {
        items,
        next,
        agents,
    })
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
            page.next.unwrap().path(None),
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
        let detail = decode_response(&serde_json::to_vec(&answers["items"][0]).unwrap()).unwrap();
        assert_eq!(detail.items, page.items);
        assert!(detail.next.is_none());
        assert_eq!(
            page.next.unwrap().path(None),
            "/api/responses?limit=20&before_id=7"
        );
    }

    #[test]
    fn agent_filters_apply_to_each_feed_and_its_pagination() {
        for (query, expected) in [
            (
                ReportQuery::Latest(Some("page+2".into())),
                "/api/reports?limit=20&history=false&cursor=page%2B2",
            ),
            (
                ReportQuery::Recent(None),
                "/api/reports?limit=20&history=true",
            ),
            (
                ReportQuery::Responses(Some(7)),
                "/api/responses?limit=20&before_id=7",
            ),
        ] {
            assert_eq!(
                query.path(Some("worker&history=false")),
                format!("{expected}&agent=worker%26history%3Dfalse")
            );
            assert_eq!(query.path(None), expected);
        }
    }
}
