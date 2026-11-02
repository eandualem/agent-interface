//! A native skill opening owns its catalog row, connection and completion ticket.

use agent_interface_roster::{
    library_snapshots::{Artifact, Kind, Snapshot},
    profiles::Connection,
    views::Skill,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Open {
    pub record: Option<Skill>,
    pub artifact: Option<Artifact>,
    pub snapshot: Option<Snapshot>,
    live: Arc<AtomicBool>,
}
impl Open {
    pub fn capture(connection: &Connection, record: Option<&Skill>) -> Self {
        let record = record.cloned();
        let artifact = record.as_ref().and_then(|record| {
            Artifact::from_catalog(connection.clone(), Kind::Skill, record.artifact_id.clone()?)
                .ok()
        });
        Self {
            record,
            artifact,
            snapshot: None,
            live: Arc::new(AtomicBool::new(true)),
        }
    }
    pub fn ticket(&self) -> Arc<AtomicBool> {
        self.live.clone()
    }
    pub fn accepts(&self, ticket: &Arc<AtomicBool>) -> bool {
        Arc::ptr_eq(&self.live, ticket) && ticket.load(Ordering::Acquire)
    }
}
impl Drop for Open {
    fn drop(&mut self) {
        self.live.store(false, Ordering::Release);
    }
}

pub(super) fn read(
    artifact: Option<Artifact>,
    live: &AtomicBool,
) -> Result<Snapshot, &'static str> {
    let artifact = artifact.ok_or("This skill has no provider snapshot available.")?;
    artifact
        .latest(&|| live.load(Ordering::Acquire))
        .map_err(|error| match error {
            agent_interface_roster::library_snapshots::Error::Connection(
                agent_interface_roster::profiles::Error::BindingRequired
                | agent_interface_roster::profiles::Error::BindingMismatch,
            ) => "The original profile binding was refused. Reconnect to that profile.",
            _ => "The skill snapshot is unavailable from its original profile.",
        })
}
pub(super) fn key(snapshot: &Snapshot) -> String {
    format!(
        "{}\nskill\n{}\n{}",
        super::seen::origin_key(Some(snapshot.artifact.identity()), ""),
        snapshot.artifact.id(),
        snapshot.revision_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::content::tests::{Reply, provider};
    use agent_interface_roster::views;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::{sync::mpsc, thread, time::Duration};

    fn row() -> serde_json::Value {
        json!({"items":[{"name":"original","path":"/display/only/skill", "artifact_id":"artifact-1","description":"original description","tags":[],"reaches":[]}]})
    }
    fn version(text: &str) -> serde_json::Value {
        json!({"revision_id":"7","label":"version 7","created_at":"date","content":text,"sha256":format!("{:x}",Sha256::digest(text.as_bytes()))})
    }
    fn catalog(connection: &Connection) -> Skill {
        let (wake, updates) = mpsc::channel();
        let source = super::super::source::Source::start(
            None,
            Some(connection),
            views::Request::Skills,
            Duration::from_secs(3600),
            move || {
                let _ = wake.send(());
            },
        )
        .unwrap();
        updates.recv_timeout(Duration::from_secs(5)).unwrap();
        let (request, data) = source.take_update().unwrap();
        assert_eq!(request, views::Request::Skills);
        let views::Data::Skills(mut library) = data.unwrap() else {
            panic!("wrong data")
        };
        library.store.items.remove(0)
    }
    fn metadata() -> Vec<Reply> {
        vec![
            Reply::ok("/api/skills", row()),
            Reply::ok("/api/agents", json!({"items":[]})),
        ]
    }

    #[test]
    fn native_skill_openings_keep_delayed_snapshot_and_discussion_origin() {
        let (arrived, wait) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let mut replies = metadata();
        replies.push(Reply {
            delay: Some((arrived, held)),
            ..Reply::ok(
                "/api/skills/artifact-1/versions?limit=1",
                json!({"items":[{"revision_id":"7"}]}),
            )
        });
        replies.push(Reply::ok("/api/skills/artifact-1/versions/7", version("A")));
        let (_dir, a, server_a) = provider("a", replies);
        let mut replies = metadata();
        replies.push(Reply::ok(
            "/api/skills/artifact-1/versions?limit=1",
            json!({"items":[{"revision_id":"7"}]}),
        ));
        replies.push(Reply::ok("/api/skills/artifact-1/versions/7", version("B")));
        let (_other, b, server_b) = provider("b", replies);
        let mut record = catalog(&a);
        let open = Open::capture(&a, Some(&record));
        record.name = "replacement".into();
        record.artifact_id = Some("replacement".into());
        assert_eq!(open.record.as_ref().unwrap().name, "original");
        assert_eq!(open.artifact.as_ref().unwrap().id(), "artifact-1");
        let ticket = open.ticket();
        let artifact = open.artifact.clone();
        let job = thread::spawn(move || read(artifact, &ticket).unwrap());
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        std::fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
        let other = Open::capture(&b, Some(&catalog(&b)));
        let other_snapshot = read(other.artifact.clone(), &other.ticket()).unwrap();
        release.send(()).unwrap();
        let snapshot = job.join().unwrap();
        assert_eq!(snapshot.content.as_deref(), Some("A"));
        assert_eq!(other_snapshot.content.as_deref(), Some("B"));
        assert_ne!(key(&snapshot), key(&other_snapshot));
        assert_eq!(snapshot.revision_id, "7");
        assert_eq!(snapshot.artifact.identity(), a.identity());
        server_a.join().unwrap();
        server_b.join().unwrap();
    }

    #[test]
    fn closing_and_reopening_the_same_path_rejects_the_previous_completion() {
        let (_dir, connection, server) = provider("a", vec![]);
        let record = Skill {
            name: "skill".into(),
            path: "/same/path".into(),
            artifact_id: Some("artifact-1".into()),
            ..Default::default()
        };
        let first = Open::capture(&connection, Some(&record));
        let old = first.ticket();
        assert!(first.accepts(&old));
        drop(first);
        let next = Open::capture(&connection, Some(&record));
        assert!(!old.load(Ordering::Acquire));
        assert!(!next.accepts(&old));
        assert!(next.accepts(&next.ticket()));
        assert!(read(next.artifact.clone(), &old).is_err());
        server.join().unwrap();
    }

    #[test]
    fn local_paths_missing_ids_and_binding_refusals_never_open_local_content() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SKILL.md"), "local secret").unwrap();
        let (_fixture, connection, server) = provider(
            "a",
            vec![Reply {
                status: 409,
                replace: true,
                ..Reply::ok(
                    "/api/skills/artifact-1/versions?limit=1",
                    json!({"detail":{"code":"binding_mismatch"}}),
                )
            }],
        );
        let mut record = Skill {
            name: "skill".into(),
            path: dir.path().display().to_string(),
            ..Default::default()
        };
        for id in [None, Some("../unsafe".into())] {
            record.artifact_id = id;
            let open = Open::capture(&connection, Some(&record));
            assert_eq!(
                read(open.artifact.clone(), &open.ticket()).err(),
                Some("This skill has no provider snapshot available.")
            );
        }
        record.artifact_id = Some("artifact-1".into());
        let open = Open::capture(&connection, Some(&record));
        assert_eq!(
            read(open.artifact.clone(), &open.ticket()).err(),
            Some("The original profile binding was refused. Reconnect to that profile.")
        );
        server.join().unwrap();
    }
}
