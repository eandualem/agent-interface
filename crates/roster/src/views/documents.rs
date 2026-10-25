//! Durable Backbone document metadata and explicitly requested revision bodies.

use std::sync::Arc;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{Api, ReportContent, ReportItem, ReportPage, ReportQuery, UNAVAILABLE, UNREADABLE};

/// Maximum decoded UTF-8 document size, including inline assets.
pub const DOCUMENT_HTML_LIMIT: usize = 4 * 1024 * 1024;
const METADATA_LIMIT: usize = 8192;
const WIRE_LIMIT: usize = 6 * DOCUMENT_HTML_LIMIT + METADATA_LIMIT;
const NOT_SERVED: &str = "This Backbone does not serve Documents yet; update Agent Backbone.";
const NOT_FOUND: &str = "This document revision is no longer available from Agent Backbone.";

/// Server-owned identity and immutable publication metadata.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct DocumentMetadata {
    /// Revision ID, independent of the document's stable root.
    pub id: u64,
    /// Stable document root, preserved through corrections.
    pub document_id: String,
    /// Stable author identity, preserved through rename and forget.
    pub author_id: String,
    /// Name at publication time.
    pub author_name: String,
    /// Current registered name, if the author still exists.
    pub agent_name: Option<String>,
    /// Authored title.
    pub title: String,
    /// Server publication time in UTC.
    pub created_at: String,
    /// Exact decoded UTF-8 byte count.
    pub content_bytes: usize,
    /// SHA-256 of the exact HTML bytes.
    pub content_sha256: String,
    /// Earlier revision replaced by this publication.
    pub supersedes_id: Option<u64>,
    /// Direct correction, if any; refreshed only on request.
    pub superseded_by: Option<u64>,
}

impl DocumentMetadata {
    /// Current name, falling back to the name retained at publication.
    pub fn author(&self) -> &str {
        self.agent_name.as_deref().unwrap_or(&self.author_name)
    }

    fn valid(&self) -> bool {
        valid_id(self.id)
            && hex(&self.document_id, 32)
            && hex(&self.author_id, 32)
            && hex(&self.content_sha256, 64)
            && self.content_bytes <= DOCUMENT_HTML_LIMIT
            && !self.title.is_empty()
            && self.title.chars().count() <= 240
            && self
                .supersedes_id
                .is_none_or(|id| valid_id(id) && id < self.id)
            && self
                .superseded_by
                .is_none_or(|id| valid_id(id) && id > self.id)
    }
}

fn valid_id(id: u64) -> bool {
    id > 0 && id <= i64::MAX as u64
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// One retained revision. Its JavaScript state is not retained by the reader.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocumentRevision {
    /// Server publication metadata, including any known correction.
    pub metadata: DocumentMetadata,
    /// Exact, size- and digest-checked HTML; never passed credentials.
    pub html: Arc<str>,
}

pub(super) fn read_list(
    api: &Api<'_>,
    before: Option<u64>,
    author_id: Option<&str>,
    agent: Option<&str>,
) -> Result<ReportPage, &'static str> {
    let mut path = "/api/documents?limit=20".to_owned();
    if let Some(id) = before {
        if !valid_id(id) {
            return Err(UNREADABLE);
        }
        path.push_str(&format!("&before_id={id}"));
    }
    if let Some(author) = author_id {
        if !hex(author, 32) {
            return Err(UNREADABLE);
        }
        path.push_str("&author_id=");
        path.push_str(author);
    }
    if let Some(agent) = agent {
        path.push_str("&agent=");
        path.push_str(&super::reports::encode(agent));
    }
    let body = fetch(api, &path, super::VIEW_LIMIT, false)?;
    let mut page = decode_list(&body, author_id)?;
    if let Ok(agents) = super::library::agents(api) {
        page.agents
            .extend(agents.into_iter().map(|agent| agent.name));
    }
    page.agents.sort_unstable();
    page.agents.dedup();
    Ok(page)
}

fn decode_list(body: &[u8], author_id: Option<&str>) -> Result<ReportPage, &'static str> {
    super::document(body)?;
    #[derive(Deserialize)]
    struct Page {
        items: Vec<DocumentMetadata>,
        has_more: bool,
        next_before_id: Option<u64>,
    }
    let page: Page = serde_json::from_slice(body).map_err(|_| UNREADABLE)?;
    if page.items.len() > 20 || page.items.iter().any(|item| !item.valid()) {
        return Err(UNREADABLE);
    }
    let next = if page.has_more {
        let id = page
            .next_before_id
            .filter(|id| valid_id(*id))
            .ok_or(UNREADABLE)?;
        if page.items.last().is_none_or(|item| item.id != id) {
            return Err(UNREADABLE);
        }
        Some(ReportQuery::Documents(
            Some(id),
            author_id.map(str::to_owned),
        ))
    } else {
        None
    };
    let agents = page
        .items
        .iter()
        .map(|item| item.author().to_owned())
        .collect();
    let items = page
        .items
        .into_iter()
        .map(|metadata| ReportItem {
            id: metadata.id,
            author: metadata.author().to_owned(),
            created_at: metadata.created_at.clone(),
            content: ReportContent::Document(metadata),
        })
        .collect();
    Ok(ReportPage {
        items,
        next,
        agents,
    })
}

pub(super) fn read(api: &Api<'_>, id: u64) -> Result<DocumentRevision, &'static str> {
    if !valid_id(id) {
        return Err(UNREADABLE);
    }
    decode(
        &fetch(api, &format!("/api/documents/{id}"), WIRE_LIMIT, true)?,
        id,
    )
}

fn fetch(api: &Api<'_>, path: &str, limit: usize, detail: bool) -> Result<Vec<u8>, &'static str> {
    if !api.live() {
        return Err(UNAVAILABLE);
    }
    match crate::transport::fetch(&api.url, path, &api.key, limit) {
        Ok(bytes) if bytes.len() <= limit => Ok(bytes),
        Ok(_) => Err(UNREADABLE),
        Err(Some(404)) if detail => Err(NOT_FOUND),
        Err(Some(404 | 405)) => Err(NOT_SERVED),
        Err(_) => Err(UNAVAILABLE),
    }
}

fn decode(body: &[u8], id: u64) -> Result<DocumentRevision, &'static str> {
    if body.len() > WIRE_LIMIT || !crate::within_depth(body) {
        return Err(UNREADABLE);
    }
    let mut value: serde_json::Value = serde_json::from_slice(body).map_err(|_| UNREADABLE)?;
    let html = value.get_mut("html").ok_or(UNREADABLE)?.take();
    let serde_json::Value::String(html) = html else {
        return Err(UNREADABLE);
    };
    value["html"] = serde_json::Value::String(String::new());
    if html.len() > DOCUMENT_HTML_LIMIT
        || serde_json::to_vec(&value).map_err(|_| UNREADABLE)?.len() > METADATA_LIMIT
    {
        return Err(UNREADABLE);
    }
    let metadata: DocumentMetadata = serde_json::from_value(value).map_err(|_| UNREADABLE)?;
    if !metadata.valid()
        || metadata.id != id
        || metadata.content_bytes != html.len()
        || metadata.content_sha256 != format!("{:x}", Sha256::digest(html.as_bytes()))
    {
        return Err(UNREADABLE);
    }
    Ok(DocumentRevision {
        metadata,
        html: html.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(html: &str) -> serde_json::Value {
        serde_json::json!({"id":7,"document_id":"a".repeat(32),"author_id":"b".repeat(32),
            "author_name":"worker","agent_name":null,"title":"Saved result",
            "created_at":"2026-10-25T10:00:00Z","content_bytes":html.len(),
            "content_sha256":format!("{:x}",Sha256::digest(html.as_bytes())),
            "supersedes_id":3,"superseded_by":9,"html":html})
    }

    #[test]
    fn historical_body_is_exact_and_only_document_reads_accept_large_json() {
        let html = "\u{0001}".repeat(DOCUMENT_HTML_LIMIT);
        let value = record(&html);
        let body = serde_json::to_vec(&value).unwrap();
        assert!(body.len() > super::super::VIEW_LIMIT);
        assert_eq!(&*decode(&body, 7).unwrap().html, html);
        assert!(decode(&body, 8).is_err());
        let mut wrong = record("<p>changed</p>");
        wrong["content_sha256"] = serde_json::json!("0".repeat(64));
        assert!(decode(&serde_json::to_vec(&wrong).unwrap(), 7).is_err());
        let mut oversized = record("ok");
        oversized["extra"] = serde_json::json!("x".repeat(METADATA_LIMIT));
        assert!(decode(&serde_json::to_vec(&oversized).unwrap(), 7).is_err());
    }

    #[test]
    fn missing_documents_endpoint_is_an_upgrade_state_not_an_empty_catalog() {
        for (detail, expected) in [(false, NOT_SERVED), (true, NOT_FOUND)] {
            let (url, server) = crate::transport::tests::server(
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                std::time::Duration::ZERO,
            );
            let api = Api::new(url, "fixture".into(), &|| true);
            assert_eq!(
                fetch(&api, "/api/documents", 1024, detail).unwrap_err(),
                expected
            );
            server.join().unwrap();
        }
    }

    #[test]
    fn pages_preserve_stable_author_filter_and_reject_missing_cursor() {
        let mut item = record("ok");
        item.as_object_mut().unwrap().remove("html");
        let mut page = serde_json::json!({"items":[item],"has_more":true,"next_before_id":7});
        let author = "b".repeat(32);
        let decoded = decode_list(&serde_json::to_vec(&page).unwrap(), Some(&author)).unwrap();
        assert_eq!(
            decoded.next,
            Some(ReportQuery::Documents(Some(7), Some(author)))
        );
        assert_eq!(decoded.items[0].author, "worker");
        page["next_before_id"] = serde_json::Value::Null;
        assert!(decode_list(&serde_json::to_vec(&page).unwrap(), None).is_err());
    }
}
