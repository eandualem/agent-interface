//! Which authored records this person has opened, kept beside the settings in
//! `agent-interface/seen.json`, so lists can tell new records from read ones.
//!
//! A document is seen per revision, so a newly published revision is new
//! again. Keys belong to the profile the record came from.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

use agent_interface_roster::profiles::Identity;
use agent_interface_roster::views::{ReportContent, ReportItem};
use serde::{Deserialize, Serialize};

/// The most records remembered; the oldest are forgotten first.
const LIMIT: usize = 4096;

#[derive(Default, Deserialize, Serialize)]
struct Seen {
    /// Opened records, oldest first.
    keys: VecDeque<String>,
}

impl Seen {
    fn contains(&self, key: &str) -> bool {
        self.keys.iter().any(|stored| stored == key)
    }
}

static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(load()));

fn path() -> Option<PathBuf> {
    Some(crate::desktop::settings::path()?.with_file_name("seen.json"))
}

fn load() -> Seen {
    let read = path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Seen>(&bytes).ok());
    read.unwrap_or_default()
}

/// Write the current records atomically on a background thread. Each write
/// takes the newest state under one lock, so a slower earlier write never
/// replaces a later one.
fn save() {
    static WRITE: Mutex<()> = Mutex::new(());
    let Some(path) = path() else { return };
    std::thread::spawn(move || {
        let _write = WRITE.lock().unwrap_or_else(|poison| poison.into_inner());
        let json = {
            let seen = SEEN.lock().unwrap_or_else(|poison| poison.into_inner());
            serde_json::to_vec(&*seen)
        };
        let (Ok(json), Some(directory)) = (json, path.parent()) else {
            return;
        };
        let temporary = directory.join(format!(".seen.json.{}.new", std::process::id()));
        let written = std::fs::create_dir_all(directory)
            .and_then(|()| std::fs::write(&temporary, json))
            .and_then(|()| std::fs::rename(&temporary, &path));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            tracing::warn!(event = "seen_save_failed", %error, "Could not save seen records");
        }
    });
}

fn lock() -> std::sync::MutexGuard<'static, Seen> {
    SEEN.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Keys for one revision of a record, then for the record across revisions.
fn keys(item: &ReportItem, origin: &Identity) -> (String, Option<String>) {
    let (kind, thing) = match &item.content {
        ReportContent::Progress { .. } => ("report", None),
        ReportContent::Response { .. } => ("response", None),
        ReportContent::Document(metadata) => ("document", Some(metadata.document_id.clone())),
    };
    let base = format!("{}|{kind}", origin_key(origin));
    (
        format!("{base}|{}", item.id),
        thing.map(|thing| format!("{base}-thing|{thing}")),
    )
}

/// Whether revision `key` has been opened.
pub(super) fn is_seen_key(key: &str) -> bool {
    lock().contains(key)
}

/// Whether any revision of the record `thing` has been opened.
pub(super) fn known(thing: &str) -> bool {
    lock().keys.iter().any(|key| key == thing)
}

/// Told when what was opened changes, so the lists that mark it, observing
/// this, redraw at once.
#[derive(Default)]
pub(super) struct Opened;
impl gpui::Global for Opened {}

/// Remember that these keys were opened, and tell the lists that mark it.
pub(super) fn mark_keys(keys: &[String], cx: &mut gpui::App) {
    let mut seen = lock();
    let mut changed = false;
    for key in keys {
        if !seen.keys.contains(key) {
            seen.keys.push_back(key.clone());
            changed = true;
        }
    }
    if !changed {
        return;
    }
    while seen.keys.len() > LIMIT {
        seen.keys.pop_front();
    }
    drop(seen);
    cx.default_global::<Opened>();
    save();
}

/// Whether the person has opened `item`.
pub(super) fn is_seen(item: &ReportItem, origin: &Identity) -> bool {
    lock().contains(&keys(item, origin).0)
}

/// Whether `item` is a newer revision of a record the person has opened.
pub(super) fn is_update(item: &ReportItem, origin: &Identity) -> bool {
    keys(item, origin).1.is_some_and(|thing| known(&thing))
}

/// Remember that the person opened `item`.
pub(super) fn mark(item: &ReportItem, origin: &Identity, cx: &mut gpui::App) {
    let (revision, thing) = keys(item, origin);
    mark_keys(
        &[Some(revision), thing]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        cx,
    );
}

/// The key of the profile a record came from.
pub(super) fn origin_key(identity: &Identity) -> String {
    format!(
        "bound:{}",
        serde_json::to_string(&[
            &identity.profile_id,
            &identity.installation_id,
            &identity.lifecycle_id
        ])
        .expect("identity key")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seen_and_discussion_keys_belong_to_one_profile_lifecycle() {
        let a = Identity {
            profile_id: "a".into(),
            installation_id: "i".into(),
            lifecycle_id: "l".into(),
        };
        let key = origin_key(&a);
        for other in [
            Identity {
                profile_id: "b".into(),
                ..a.clone()
            },
            Identity {
                installation_id: "j".into(),
                ..a.clone()
            },
            Identity {
                lifecycle_id: "m".into(),
                ..a.clone()
            },
        ] {
            assert_ne!(key, origin_key(&other));
        }
        let item = ReportItem {
            id: 7,
            author: "worker".into(),
            author_id: Some("b".repeat(32)),
            registered: true,
            created_at: "2026-01-01T00:00:00Z".into(),
            conversation_id: Some("7".into()),
            content: ReportContent::Response {
                question: "Q".into(),
                answer: "A".into(),
                action: String::new(),
                superseded_by: None,
            },
        };
        let restored = Identity {
            lifecycle_id: "restored".into(),
            ..a.clone()
        };
        let mut seen = Seen::default();
        let bound = keys(&item, &a).0;
        assert!(!seen.contains(&bound));
        seen.keys.push_back(bound.clone());
        assert!(seen.contains(&bound));
        assert!(!seen.contains(&keys(&item, &restored).0));
    }
}
