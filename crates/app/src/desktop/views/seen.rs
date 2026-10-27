//! Which authored records this person has opened, kept beside the settings in
//! `agent-interface/seen.json`, so lists can tell new records from read ones.
//!
//! A document is seen per revision, so a newly published revision is new
//! again. Records published before the file was first written count as seen.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

use agent_interface_roster::views::{ReportContent, ReportItem, epoch_seconds};
use serde::{Deserialize, Serialize};

/// The most records remembered; the oldest are forgotten first.
const LIMIT: usize = 4096;

#[derive(Default, Deserialize, Serialize)]
struct Seen {
    /// When this file began; anything published earlier counts as seen.
    since: f64,
    /// Opened records, oldest first.
    keys: VecDeque<String>,
}

static SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(load()));

fn path() -> Option<PathBuf> {
    Some(crate::desktop::settings::path()?.with_file_name("seen.json"))
}

fn load() -> Seen {
    let read = path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Seen>(&bytes).ok());
    read.unwrap_or_else(|| {
        let seen = Seen {
            since: agent_interface_roster::views::now() as f64,
            keys: VecDeque::new(),
        };
        save();
        seen
    })
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
fn keys(item: &ReportItem) -> (String, Option<String>) {
    let (kind, thing) = match &item.content {
        ReportContent::Progress { .. } => ("report", None),
        ReportContent::Response { .. } => ("response", None),
        ReportContent::Document(metadata) => ("document", Some(metadata.document_id.clone())),
    };
    let base = format!("{}|{kind}", item.provider_url);
    (
        format!("{base}|{}", item.id),
        thing.map(|thing| format!("{base}-thing|{thing}")),
    )
}

/// Whether revision `key`, published at `published`, has been opened or
/// predates the record of what was opened.
pub(super) fn is_seen_key(key: &str, published: &str) -> bool {
    let seen = lock();
    epoch_seconds(published).is_some_and(|at| at < seen.since) || seen.keys.iter().any(|k| k == key)
}

/// Whether any revision of the record `thing` has been opened.
pub(super) fn known(thing: &str) -> bool {
    lock().keys.iter().any(|key| key == thing)
}

/// Remember that these keys were opened.
pub(super) fn mark_keys(keys: &[String]) {
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
    save();
}

/// Whether the person has opened `item`, or it predates the record of that.
pub(super) fn is_seen(item: &ReportItem) -> bool {
    is_seen_key(&keys(item).0, &item.created_at)
}

/// Whether `item` is a newer revision of a record the person has opened.
pub(super) fn is_update(item: &ReportItem) -> bool {
    keys(item).1.is_some_and(|thing| known(&thing))
}

/// Remember that the person opened `item`.
pub(super) fn mark(item: &ReportItem) {
    let (revision, thing) = keys(item);
    mark_keys(
        &[Some(revision), thing]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
    );
}
