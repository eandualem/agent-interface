//! The Rust service's push stream: agent states and waits as they change,
//! and a signal to read the roster again for everything else.

use std::time::Duration;

use serde::Deserialize;
use serde_json::value::RawValue;

use super::{Connection, Error};
use crate::{Background, HeldQueue, Waiting, lenient, lenient_object, transport::local};

/// The service writes a comment every 15 seconds while nothing else happens,
/// so three missed ones mean a broken stream.
const QUIET: Duration = Duration::from_secs(45);
/// Far above any frame of the contract.
const LINE_LIMIT: usize = 64 * 1024;

/// What one frame of the push stream changes in the roster.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Read the roster again: the stream opened, a run or an agent's own
    /// restart changed, events were missed, or a change could not be read.
    Read,
    /// An agent's new state, from `agent.state`.
    State {
        /// The agent's name.
        agent: String,
        /// The agent's registration.
        registration_id: String,
        /// The new state, as the agents list writes it.
        state: String,
        /// Why, for a waiting or blocked agent.
        reason: Option<String>,
        /// Since when, in Unix seconds.
        since: Option<f64>,
        /// The issue it works on: `Some(None)` for none, and `None` when the
        /// frame does not say, as from an older service.
        issue: Option<Option<u64>>,
    },
    /// What an agent waits on, its background counts and its terminal
    /// queue, from `agent.changed`.
    Waits {
        /// The agent's name.
        agent: String,
        /// The agent's registration.
        registration_id: String,
        /// What it waits on; none when it waits on nothing.
        waiting: Option<Waiting>,
        /// Its background counts.
        background: Option<Background>,
        /// What holds its terminal deliveries and how many wait, the held
        /// one included; none from a service that does not send `queued`.
        queue: Option<(Option<HeldQueue>, u64)>,
    },
}

impl Connection {
    /// Follow this profile's push stream until `live` is false, calling
    /// `changed` with each frame that changes the roster: a state or wait to
    /// apply to an agent, or [`Change::Read`] when the stream opens, a run or
    /// an agent's own restart changes or the service says events were missed. A service that doesn't
    /// know a kind ignores it. No cursor is kept: a stream that is opened
    /// again starts with a read.
    ///
    /// A service without the stream is `Unsupported`. A stream that ends while `live` is an error. Blocks its
    /// caller, which is an I/O worker of its own.
    pub fn watch(
        &self,
        live: &dyn Fn() -> bool,
        changed: &mut dyn FnMut(Change),
    ) -> Result<(), Error> {
        let mut frames = Frames::default();
        local::stream(
            self.socket.to_str().ok_or(Error::Invalid)?,
            "/api/stream?kinds=agent.state,agent.changed,agent.restart,run.changed,resync",
            &self.identity.profile_id,
            None,
            QUIET,
            live,
            &mut |bytes| frames.push(bytes, changed),
        )
        .map_err(|status| match status {
            Some(404) => Error::Unsupported,
            Some(status) => Error::Refused(status),
            None => Error::Unavailable,
        })
    }
}

/// Server-Sent Events, read as far as each frame's kind and data.
#[derive(Default)]
struct Frames {
    line: Vec<u8>,
    /// The kind of the frame being read.
    kind: Vec<u8>,
    /// Its data lines, joined.
    data: Vec<u8>,
}

impl Frames {
    /// Read more of the stream. False when a line or a frame's data outgrows
    /// the limit.
    fn push(&mut self, bytes: &[u8], changed: &mut dyn FnMut(Change)) -> bool {
        for &byte in bytes {
            if byte != b'\n' {
                self.line.push(byte);
                if self.line.len() > LINE_LIMIT {
                    return false;
                }
                continue;
            }
            let line = self.line.strip_suffix(b"\r").unwrap_or(&self.line);
            if line.is_empty() {
                // A blank line ends the frame.
                if let Some(change) = change(&self.kind, &self.data) {
                    changed(change);
                }
                self.kind.clear();
                self.data.clear();
            } else if let Some(kind) = line.strip_prefix(b"event:") {
                self.kind = kind.trim_ascii().to_vec();
            } else if let Some(data) = line.strip_prefix(b"data:") {
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if !self.data.is_empty() {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(data);
                if self.data.len() > LINE_LIMIT {
                    return false;
                }
            }
            self.line.clear();
        }
        true
    }
}

/// What a frame of `kind` with `data` changes; none for a kind that changes
/// nothing shown. A state or wait that can't be read asks for a read instead.
fn change(kind: &[u8], data: &[u8]) -> Option<Change> {
    #[derive(Deserialize)]
    struct State {
        agent: String,
        registration_id: String,
        state: String,
        #[serde(default, deserialize_with = "lenient")]
        reason: Option<String>,
        #[serde(default, deserialize_with = "lenient")]
        since: Option<f64>,
        #[serde(default, deserialize_with = "issue")]
        current_issue: Option<Option<u64>>,
    }
    #[derive(Deserialize)]
    struct Waits {
        agent: String,
        registration_id: String,
        #[serde(default, deserialize_with = "lenient_object")]
        waiting: Option<Waiting>,
        #[serde(default)]
        background: Option<Box<RawValue>>,
        #[serde(default, deserialize_with = "lenient_object")]
        held_queue: Option<HeldQueue>,
        #[serde(default, deserialize_with = "lenient")]
        queued: Option<u64>,
    }
    Some(match kind {
        b"hello" | b"run.changed" | b"agent.restart" | b"resync" => Change::Read,
        b"agent.state" => {
            serde_json::from_slice::<State>(data).map_or(Change::Read, |state| Change::State {
                agent: state.agent,
                registration_id: state.registration_id,
                state: state.state,
                reason: state.reason,
                since: state.since,
                issue: state.current_issue,
            })
        }
        b"agent.changed" => {
            serde_json::from_slice::<Waits>(data).map_or(Change::Read, |waits| Change::Waits {
                agent: waits.agent,
                registration_id: waits.registration_id,
                waiting: waits.waiting,
                background: waits.background.and_then(|raw| Background::read(raw.get())),
                queue: waits.queued.map(|queued| (waits.held_queue, queued)),
            })
        }
        _ => return None,
    })
}

/// A present `current_issue`: `null` is no issue, and a value that is not an
/// issue number reads as though the frame did not say.
fn issue<'de, D>(deserializer: D) -> Result<Option<Option<u64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <Box<RawValue> as Deserialize>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(chunks: &[&[u8]]) -> (Vec<Change>, bool) {
        let mut frames = Frames::default();
        let mut changes = Vec::new();
        let within = chunks
            .iter()
            .all(|chunk| frames.push(chunk, &mut |change| changes.push(change)));
        (changes, within)
    }

    #[test]
    fn frames_carry_states_across_chunks_and_skip_the_rest() {
        let (changes, within) = read(&[
            b"event: hello\ndata: {\"instance_id\":\"a\",\"cursor\":\"a:1\"}\n\n",
            b": heartbeat\n\nid: a:2\r\nevent: agent.st",
            b"ate\r\ndata: {\"agent\":\"worker\",\"registration_id\":\"r1\",",
            b"\"state\":\"waiting_for_human\",\"reason\":\"permission\",\"since\":1.5}\r\n",
            b"\r\nid: a:3\nevent: join.progress\ndata: {}\n\n",
            b"id: a:0\nevent:resync\ndata: {}\n\n",
            &[b'x'; LINE_LIMIT + 1],
        ]);
        assert!(!within);
        assert_eq!(
            changes,
            [
                Change::Read,
                Change::State {
                    agent: "worker".into(),
                    registration_id: "r1".into(),
                    state: "waiting_for_human".into(),
                    reason: Some("permission".into()),
                    since: Some(1.5),
                    issue: None,
                },
                Change::Read,
            ]
        );
    }

    #[test]
    fn the_services_fixtures_give_their_waits_and_queues_and_a_read_for_a_run() {
        let mut stream = String::new();
        for (kind, data) in [
            (
                "run.changed",
                include_str!("runs/fixtures/run-changed.json"),
            ),
            (
                "agent.changed",
                include_str!("runs/fixtures/agent-changed.json"),
            ),
            (
                "agent.changed",
                include_str!("runs/fixtures/agent-changed-held-queue.json"),
            ),
        ] {
            // One frame's data is one line of JSON.
            let data: serde_json::Value = serde_json::from_str(data).unwrap();
            stream.push_str(&format!("id: a:4\nevent: {kind}\ndata: {data}\n\n"));
        }
        let (changes, within) = read(&[stream.as_bytes()]);
        assert!(within);
        assert_eq!(changes[0], Change::Read);
        let Change::Waits {
            agent,
            registration_id,
            waiting,
            background,
            queue,
        } = &changes[1]
        else {
            panic!("{changes:?}");
        };
        assert_eq!(
            (agent.as_str(), registration_id.as_str()),
            ("worker", "1234567890abcdef1234567890abcdef")
        );
        assert_eq!(
            waiting.as_ref().unwrap().on_agent.as_deref(),
            Some("reviewer")
        );
        assert_eq!(
            *background,
            Some(Background {
                running: 0,
                queued: 1,
                processes: 1
            })
        );
        assert_eq!(*queue, Some((None, 0)));
        let Change::Waits { queue, .. } = &changes[2] else {
            panic!("{changes:?}");
        };
        assert_eq!(
            *queue,
            Some((
                Some(HeldQueue {
                    reason: Some("uncertain_delivery".into())
                }),
                8
            ))
        );
    }

    #[test]
    fn a_state_carries_its_issue_when_the_service_says_and_not_otherwise() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("runs/fixtures/stream-agent-state.json")).unwrap();
        let data = &fixture["data"];
        let mut none = data.clone();
        none["current_issue"] = serde_json::Value::Null;
        let mut older = data.clone();
        older.as_object_mut().unwrap().remove("current_issue");
        let mut stream = String::new();
        for data in [data, &none, &older] {
            stream.push_str(&format!(
                "event: {}\ndata: {data}\n\n",
                fixture["event"].as_str().unwrap()
            ));
        }
        let (changes, within) = read(&[stream.as_bytes()]);
        assert!(within);
        assert_eq!(
            changes[0],
            Change::State {
                agent: "worker".into(),
                registration_id: "1234567890abcdef1234567890abcdef".into(),
                state: "idle".into(),
                reason: None,
                since: Some(1794301260.0),
                issue: Some(Some(42)),
            }
        );
        let issues: Vec<_> = changes
            .iter()
            .map(|change| match change {
                Change::State { issue, .. } => *issue,
                _ => panic!("{changes:?}"),
            })
            .collect();
        assert_eq!(issues, [Some(Some(42)), Some(None), None]);
    }

    #[test]
    fn a_frames_data_lines_join_up_to_the_limit() {
        let (changes, within) = read(&[
            b"event: agent.state\ndata: {\"agent\":\"worker\",\n",
            b"data: \"registration_id\":\"r1\",\"state\":\"idle\"}\n\n",
        ]);
        assert!(within);
        assert!(matches!(&changes[..], [Change::State { state, .. }] if state == "idle"));
        let line = [b"data: ".as_slice(), &[b'x'; 1024], b"\n"].concat();
        let (_, within) = read(&[&line.repeat(LINE_LIMIT / 1024 + 1)]);
        assert!(!within);
    }

    #[test]
    fn a_change_that_cannot_be_read_asks_for_a_read() {
        let (changes, _) = read(&[
            b"event: agent.state\ndata: {\"agent\":\"worker\"}\n\n",
            b"event: agent.changed\ndata: [1]\n\n",
        ]);
        assert_eq!(changes, [Change::Read, Change::Read]);
    }

    #[test]
    fn each_step_of_an_agents_restart_signals_a_read() {
        let mut frames = Frames::default();
        let mut count = 0;
        let steps: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "runs/fixtures/stream-agent-restart-fresh.json"
        ))
        .unwrap();
        for step in &steps {
            let kind = step["event"].as_str().unwrap();
            let frame = format!("event: {kind}\ndata: {}\n\n", step["data"]);
            assert!(frames.push(frame.as_bytes(), &mut |change| {
                assert_eq!(change, Change::Read);
                count += 1;
            }));
        }
        assert_eq!(count, steps.len());
    }
}
