//! Captured policy layer metadata and provider-owned immutable snapshot reads.
use agent_interface_roster::{
    library_snapshots::{Artifact, Kind, Snapshot},
    profiles::Connection,
    views::Source,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) struct Open {
    pub source: Source,
    pub candidates: Vec<String>,
    pub artifact: Option<Artifact>,
    pub snapshot: Option<Snapshot>,
    live: Arc<AtomicBool>,
}
impl Open {
    pub fn capture(connection: &Connection, source: Source, candidates: Vec<String>) -> Self {
        let artifact = source.artifact_id.as_ref().and_then(|id| {
            Artifact::from_catalog(connection.clone(), Kind::Policy, id.clone()).ok()
        });
        Self {
            source,
            candidates,
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
    let artifact = artifact.ok_or("This policy has no provider snapshot available.")?;
    artifact
        .latest(&|| live.load(Ordering::Acquire))
        .map_err(|error| match error {
            agent_interface_roster::library_snapshots::Error::Connection(
                agent_interface_roster::profiles::Error::BindingRequired
                | agent_interface_roster::profiles::Error::BindingMismatch,
            ) => "The original profile binding was refused. Reconnect to that profile.",
            _ => "The policy snapshot is unavailable from its original profile.",
        })
}
pub(super) fn key(snapshot: &Snapshot) -> String {
    format!(
        "{}\npolicy\n{}\n{}",
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

    fn metadata() -> Vec<Reply> {
        vec![
            Reply::ok(
                "/api/agents",
                json!({"items":[{"name":"worker","configured":true}]}),
            ),
            Reply::ok(
                "/api/agents/worker/instructions",
                json!({"sources":[
                    {"name":"base","path":"/display/base","scope":"base","text":"provider base", "artifact_id":"forged"},
                    {"name":"original","path":"/display/policy","scope":"global","text":"old preview", "artifact_id":"forged"}
                ]}),
            ),
            Reply::ok(
                "/api/policies",
                json!({"items":[{"name":"original","path":"/display/policy","artifact_id":"artifact-1"}]}),
            ),
        ]
    }
    fn version(text: &str) -> serde_json::Value {
        json!({"revision_id":"7","label":"version 7","created_at":"date","content":text,"sha256":format!("{:x}",Sha256::digest(text.as_bytes()))})
    }
    fn catalog(connection: &Connection) -> Result<views::PolicyLibrary, &'static str> {
        let (wake, updates) = mpsc::channel();
        let source = super::super::source::Source::start(
            None,
            Some(connection),
            views::Request::Policies,
            Duration::from_secs(3600),
            move || {
                let _ = wake.send(());
            },
        )
        .unwrap();
        updates.recv_timeout(Duration::from_secs(5)).unwrap();
        let (request, data) = source.take_update().unwrap();
        assert_eq!(request, views::Request::Policies);
        let views::Data::Policies(library) = data? else {
            panic!("wrong data")
        };
        Ok(*library)
    }

    #[test]
    fn native_policy_snapshot_keeps_catalog_identity_recipients_and_original_connection() {
        let (arrived, wait) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let mut replies = metadata();
        replies.push(Reply {
            delay: Some((arrived, held)),
            ..Reply::ok(
                "/api/policies/artifact-1/versions?limit=1",
                json!({"items":[{"revision_id":"7"}]}),
            )
        });
        replies.push(Reply::ok(
            "/api/policies/artifact-1/versions/7",
            version("A"),
        ));
        let (_dir, a, server_a) = provider("a", replies);
        let mut replies = metadata();
        replies.push(Reply::ok(
            "/api/policies/artifact-1/versions?limit=1",
            json!({"items":[{"revision_id":"7"}]}),
        ));
        replies.push(Reply::ok(
            "/api/policies/artifact-1/versions/7",
            version("B"),
        ));
        let (_other, b, server_b) = provider("b", replies);
        let mut library = catalog(&a).unwrap();
        assert_eq!(library.global[0].artifact_id.as_deref(), Some("artifact-1"));
        assert!(library.global[0].text.is_none());
        assert!(library.base[0].artifact_id.is_none());
        assert_eq!(library.base[0].text.as_deref(), Some("provider base"));
        let open = Open::capture(&a, library.global[0].clone(), library.members.clone());
        library.global[0].name = "replacement".into();
        library.global[0].artifact_id = Some("replacement".into());
        library.members.clear();
        assert_eq!(open.source.name.as_ref(), "original");
        assert_eq!(open.candidates, vec!["worker"]);
        let ticket = open.ticket();
        let artifact = open.artifact.clone();
        let job = thread::spawn(move || read(artifact, &ticket).unwrap());
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        std::fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
        let other = catalog(&b).unwrap();
        let other = Open::capture(&b, other.global[0].clone(), other.members);
        let other = read(other.artifact.clone(), &other.ticket()).unwrap();
        release.send(()).unwrap();
        let snapshot = job.join().unwrap();
        assert_eq!(snapshot.content.as_deref(), Some("A"));
        assert_eq!(other.content.as_deref(), Some("B"));
        assert_ne!(key(&snapshot), key(&other));
        assert_eq!(snapshot.artifact.identity(), a.identity());
        assert_eq!(snapshot.artifact.kind(), Kind::Policy);
        assert_eq!(snapshot.revision_id, "7");
        server_a.join().unwrap();
        server_b.join().unwrap();
    }

    #[test]
    fn policy_catalog_binding_refusal_discards_instruction_previews() {
        let mut replies = metadata();
        replies.pop();
        replies.push(Reply {
            status: 409,
            replace: true,
            ..Reply::ok(
                "/api/policies",
                json!({"detail":{"code":"binding_mismatch"}}),
            )
        });
        let (_dir, connection, server) = provider("a", replies);
        assert_eq!(
            catalog(&connection).err(),
            Some("The original profile binding was refused. Reconnect to that profile.")
        );
        server.join().unwrap();
    }

    #[test]
    fn missing_catalog_identity_never_uses_preview_or_local_policy_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("policy.md");
        std::fs::write(&path, "local secret").unwrap();
        let mut replies = metadata();
        replies.pop();
        replies.push(Reply::ok("/api/policies", json!({"items":[]})));
        let (_dir, connection, server) = provider("a", replies);
        let library = catalog(&connection).unwrap();
        assert!(library.global[0].artifact_id.is_none());
        assert!(library.global[0].text.is_none());
        let mut source = library.global[0].clone();
        source.path = path.to_string_lossy().as_ref().into();
        for id in [None, Some("../unsafe".into())] {
            source.artifact_id = id;
            let open = Open::capture(&connection, source.clone(), library.members.clone());
            assert_eq!(
                read(open.artifact.clone(), &open.ticket()).err(),
                Some("This policy has no provider snapshot available.")
            );
        }
        let base = Open::capture(&connection, library.base[0].clone(), library.members);
        assert!(base.artifact.is_none());
        assert_eq!(base.source.text.as_deref(), Some("provider base"));
        server.join().unwrap();
    }

    #[test]
    fn closing_policy_reader_cancels_its_read_and_rejects_reopened_completion() {
        let (_dir, connection, server) = provider("a", vec![]);
        let source = Source {
            artifact_id: Some("artifact-1".into()),
            path: "/same/path".into(),
            ..Default::default()
        };
        let first = Open::capture(&connection, source.clone(), vec![]);
        let old = first.ticket();
        assert!(first.accepts(&old));
        drop(first);
        let next = Open::capture(&connection, source, vec![]);
        assert!(!old.load(Ordering::Acquire));
        assert!(!next.accepts(&old));
        assert!(next.accepts(&next.ticket()));
        assert!(read(next.artifact.clone(), &old).is_err());
        server.join().unwrap();
    }
}
