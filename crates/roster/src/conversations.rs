//! One conversation per artifact, read and written through Backbone's
//! authenticated local API (provider contract r2).
//!
//! A conversation lasts across all of an artifact's versions. Its entries are
//! owner and agent messages, and the events the artifact produces when a new
//! version or plan step appears. Transport and parsing run on I/O workers,
//! never the UI thread.

use serde::{Deserialize, Serialize};

/// What a conversation is about.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A document, across its revisions.
    Document,
    /// A workbook, across its revisions.
    Sheet,
    /// An answer and its corrections.
    Response,
    /// A progress report and its corrections.
    Report,
    /// An agent's current plan.
    Plan,
    /// A shared skill.
    Skill,
    /// A policy.
    Policy,
}

/// One entry, in the order Backbone stored it.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Entry {
    /// Stable and increasing across the installation.
    pub id: u64,
    #[serde(flatten)]
    /// What it says.
    pub body: Body,
    /// When Backbone stored it, RFC 3339 UTC.
    pub created_at: String,
    /// Who wrote it; for events, who published.
    pub author: Author,
    /// The version or place the author was looking at.
    #[serde(default)]
    pub anchor: Option<Anchor>,
}

/// What an entry says.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    /// Something the owner or an agent wrote.
    Message {
        /// At most 4,000 characters.
        text: String,
        /// Owner messages: the agent it was addressed to.
        #[serde(default)]
        recipient: Option<Agent>,
        /// Agent messages: the entry this answers.
        #[serde(default)]
        in_reply_to: Option<u64>,
        /// Agent messages: the version this reply produced.
        #[serde(default)]
        resulting_revision_id: Option<String>,
        /// Owner messages: how far delivery got.
        #[serde(default)]
        delivery: Option<Delivery>,
    },
    /// A new version of the artifact appeared.
    Revision {
        /// The version.
        revision: Revision,
    },
    /// A newly published plan changed a step's status.
    Step {
        /// The step and its new state.
        step: Step,
    },
}

/// Who wrote an entry.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Author {
    /// The person using the app.
    Owner,
    /// An agent, by registration.
    Agent {
        /// Stable registration identity.
        agent_id: String,
        /// Current registered name.
        name: String,
    },
}

/// An agent registration: stable identity plus its current name.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct Agent {
    /// Stable registration identity.
    pub agent_id: String,
    /// Current registered name.
    pub name: String,
}

/// How far an owner message's delivery got. A reply is a later entry, not a
/// delivery state.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    /// Stored, queued or retrying.
    Pending,
    /// Pasted, or acknowledged from the inbox.
    Delivered,
    /// Waiting in the recipient's inbox.
    Held,
    /// Refused or expired; it can be redelivered.
    Failed,
}

/// Where an entry points: a version, and a range or plan item within it.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
pub struct Anchor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// The version the author was viewing.
    pub revision_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Sheets: the selected cells.
    pub sheet: Option<SheetAnchor>,
    /// A plan item or dependency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// The text shown when it was written, such as `Runs!B3:D5`; display only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Cells by stable tab, row and column identity, so a sort or move keeps them.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct SheetAnchor {
    /// Stable tab identity.
    pub tab_id: String,
    /// Rows, in selection order.
    pub row_ids: Vec<String>,
    /// Columns, in selection order.
    pub column_ids: Vec<String>,
}

/// A version that appeared.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Revision {
    /// The new version's identity.
    pub revision_id: String,
    /// Such as `rev 12`.
    pub label: String,
    /// Sheets: the cells that changed, when the provider could compute them.
    #[serde(default)]
    pub changed: Vec<Changed>,
}

/// Changed cells, by label and stable identity.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Changed {
    /// Display text.
    pub label: String,
    /// The cells, by stable identity.
    pub sheet: SheetAnchor,
}

/// A plan step's new state.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Step {
    /// Stable across plan revisions.
    pub item_id: String,
    /// What the step is.
    pub title: String,
    /// `pending`, `in_progress`, `blocked` or `done`.
    pub status: String,
    /// `now`, `next` or `later`.
    #[serde(default)]
    pub section: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_read_messages_revisions_and_steps() {
        let entries: Vec<Entry> = serde_json::from_str(
            r#"[
              {"id":4182,"kind":"message","created_at":"2026-10-27T14:30:00Z",
               "author":{"type":"owner"},"text":"Could History come first?",
               "recipient":{"agent_id":"a1","name":"builder"},"delivery":"held",
               "anchor":{"revision_id":"r11","label":"Runs!B3:D5",
                 "sheet":{"tab_id":"t","row_ids":["r3","r4"],"column_ids":["c2"]}}},
              {"id":4183,"kind":"message","created_at":"2026-10-27T14:41:03Z",
               "author":{"type":"agent","agent_id":"a1","name":"builder"},
               "text":"Yes.","in_reply_to":4182,"resulting_revision_id":"r12"},
              {"id":4184,"kind":"revision","created_at":"2026-10-27T14:42:00Z",
               "author":{"type":"agent","agent_id":"a1","name":"builder"},
               "revision":{"revision_id":"r12","label":"rev 12","changed":[
                 {"label":"Runs!D4","sheet":{"tab_id":"t","row_ids":["r4"],"column_ids":["c4"]}}]}},
              {"id":4185,"kind":"step","created_at":"2026-10-27T14:43:00Z",
               "author":{"type":"agent","agent_id":"a1","name":"builder"},
               "step":{"item_id":"b","title":"Verify candidate","status":"in_progress","section":"now"}}
            ]"#,
        )
        .unwrap();
        assert!(matches!(
            entries[0].body,
            Body::Message {
                delivery: Some(Delivery::Held),
                ..
            }
        ));
        assert_eq!(
            entries[0]
                .anchor
                .as_ref()
                .unwrap()
                .sheet
                .as_ref()
                .unwrap()
                .row_ids
                .len(),
            2
        );
        assert!(matches!(&entries[1].author, Author::Agent { name, .. } if name == "builder"));
        assert!(
            matches!(&entries[2].body, Body::Revision { revision } if revision.changed[0].label == "Runs!D4")
        );
        assert!(matches!(&entries[3].body, Body::Step { step } if step.status == "in_progress"));
    }
}
