//! What each agent may reach, and changes to it, through the profile's
//! access routes: the access fields of `GET /api/agents`,
//! `GET /api/access/requests`, `POST /api/agents/{name}/grant` and
//! `.../revoke`, `PUT /api/agents/{name}/mode` and `.../place`, and `POST /api/access/requests/{id}/approve` and
//! `.../refuse`.
//!
//! Backbone judges every change and replies with what the record holds
//! now, or a refusal whose message is written for the person.
use super::{Connection, Error, successful};
use crate::{RESPONSE_LIMIT, Swarm, transport::local, views::encode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json, value::RawValue};

/// An agent's role below the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Above every other agent.
    Orchestrator,
    /// Above the agents under it.
    Coordinator,
    /// A main agent of a repository.
    Main,
    /// A swarm's coordinator.
    SwarmCoordinator,
    /// A member of a swarm.
    SwarmMember,
    /// A role this version does not know; never sent.
    #[serde(other)]
    Unknown,
}

impl Role {
    /// Every role an agent can be placed in, in the order a person picks one.
    pub const ALL: [Self; 5] = [
        Self::Orchestrator,
        Self::Coordinator,
        Self::Main,
        Self::SwarmCoordinator,
        Self::SwarmMember,
    ];
}

/// How an agent is confined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// In the agent sandbox; every agent starts here.
    #[default]
    Sandboxed,
    /// With no agent sandbox. Only the owner sets it.
    Unsandboxed,
    /// A mode this version does not know; never sent.
    #[serde(other)]
    Unknown,
}

/// Reading, or reading and writing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Reading only.
    Read,
    /// Reading and writing.
    Write,
}

/// Something outside its own workspace an agent may reach. Every kind but a
/// repository is on one machine, named by ID with its name beside it in a
/// reply; a body may leave the machine out for the agent's own. A grant from
/// a reply is sent back as it is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Grant {
    /// A folder and everything below it.
    Folder {
        /// The machine it is on, by ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// That machine's name, in a reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine_name: Option<String>,
        /// Its absolute path.
        path: String,
        /// What the agent may do there.
        access: Access,
    },
    /// A repository on GitHub.
    Repository {
        /// `owner/name`.
        repo: String,
        /// What the agent may ask of it.
        access: Access,
    },
    /// A secret a program reads as a file.
    SecretFile {
        /// The machine it is on, by ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// That machine's name, in a reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine_name: Option<String>,
        /// Its absolute path.
        path: String,
    },
    /// A secret a program reads as an environment variable.
    SecretVariable {
        /// The machine it is placed on, by ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// That machine's name, in a reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine_name: Option<String>,
        /// The variable's name.
        name: String,
    },
    /// The profile's browser, with its data on the machine.
    Browser {
        /// The machine its data is on, by ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// That machine's name, in a reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine_name: Option<String>,
    },
    /// Control of Docker on the machine. Only the owner grants it.
    Docker {
        /// The machine Docker runs on, by ID.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine: Option<String>,
        /// That machine's name, in a reply.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        machine_name: Option<String>,
    },
    /// A kind this version does not know, or one it could not read; never
    /// sent.
    #[serde(other)]
    Unknown,
}

/// What an agent reaches on GitHub, from its role, its place, its own
/// repository and its grants.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reach {
    /// A top-level orchestrator's: reading every repository the profile's
    /// GitHub connection reaches; `items` are what it writes.
    Profile {
        /// The repositories it writes.
        #[serde(default, deserialize_with = "reached")]
        items: Vec<Reached>,
    },
    /// Exactly the repositories listed.
    Repositories {
        /// The repositories, each once.
        #[serde(default, deserialize_with = "reached")]
        items: Vec<Reached>,
    },
    /// A kind this version does not know, or one it could not read.
    #[serde(other)]
    Unknown,
}

impl Default for Reach {
    /// No repository at all, as Backbone has it for a field left out.
    fn default() -> Self {
        Self::Repositories { items: Vec::new() }
    }
}

/// A repository an agent reaches.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Reached {
    /// `owner/name`.
    pub repo: String,
    /// What the agent may ask of it.
    pub access: Access,
    /// Why it reaches it.
    pub source: Source,
}

/// Why an agent reaches a repository.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// It is the repository the agent works on.
    OwnRepository,
    /// Its role reaches it.
    Role,
    /// It was granted.
    Grant,
    /// A source this version does not know.
    #[serde(other)]
    Unknown,
}

/// One registered agent's place and access, as `GET /api/agents` lists it.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Agent {
    /// Its name.
    pub name: String,
    /// The repository it owns, or empty.
    #[serde(deserialize_with = "crate::lenient_string")]
    pub repo: String,
    /// Its tags.
    #[serde(deserialize_with = "crate::lenient_strings")]
    pub tags: Vec<String>,
    /// The machine it runs on, when reported.
    #[serde(deserialize_with = "crate::lenient_object")]
    pub machine: Option<crate::Machine>,
    /// Its role; none for a session that is not registered.
    #[serde(deserialize_with = "crate::lenient")]
    pub role: Option<Role>,
    /// The agent directly above it; none directly under the owner.
    #[serde(deserialize_with = "crate::lenient")]
    pub superior: Option<String>,
    /// How it is confined.
    #[serde(deserialize_with = "mode")]
    pub mode: Mode,
    /// What it has been granted, in the order the grants were made.
    #[serde(deserialize_with = "grants")]
    pub grants: Vec<Grant>,
    /// The widest access it may grant to the agents below it.
    #[serde(deserialize_with = "grants")]
    pub ceiling: Vec<Grant>,
    /// What it reaches on GitHub.
    #[serde(deserialize_with = "reach")]
    pub reach: Reach,
}

/// The agents whose access is recorded, and the running swarms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    /// Registered agents in Backbone's order.
    pub agents: Vec<Agent>,
    /// Running swarms; empty when their read failed, as they only group agents.
    pub swarms: Vec<Swarm>,
}

/// The reply to a change of an agent's access: what its record holds now.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct AgentAccess {
    /// The agent's name.
    pub name: String,
    /// How it is confined.
    #[serde(deserialize_with = "mode")]
    pub mode: Mode,
    /// What it has been granted.
    #[serde(deserialize_with = "grants")]
    pub grants: Vec<Grant>,
    /// What it may grant to the agents below it.
    #[serde(deserialize_with = "grants")]
    pub ceiling: Vec<Grant>,
    /// What it reaches on GitHub.
    #[serde(deserialize_with = "reach")]
    pub reach: Reach,
    /// What the change leaves it able to reach beyond its sandbox, or that a
    /// running session keeps what it started with, written for the person.
    #[serde(deserialize_with = "crate::lenient")]
    pub warning: Option<String>,
}

/// The reply to placing an agent: where it stands now.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Placed {
    /// The agent's name.
    pub name: String,
    /// Its role.
    pub role: Role,
    /// The agent directly above it; none directly under the owner.
    #[serde(default)]
    pub superior: Option<String>,
}

/// Where a request for access stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestState {
    /// It waits for a decision.
    Pending,
    /// It was approved and granted.
    Approved,
    /// It was refused.
    Refused,
    /// A state this version does not know.
    #[default]
    #[serde(other)]
    Unknown,
}

/// An agent's request for access.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Request {
    /// Its ID.
    pub id: i64,
    /// The agent that asked, for itself.
    pub agent: String,
    /// What it asks for.
    #[serde(deserialize_with = "grant")]
    pub grant: Grant,
    /// Why, as the agent said it.
    #[serde(default, deserialize_with = "crate::lenient_string")]
    pub reason: String,
    /// When it asked, in RFC 3339.
    #[serde(default, deserialize_with = "crate::lenient_string")]
    pub requested_at: String,
    /// Where it stands.
    #[serde(default, deserialize_with = "state")]
    pub state: RequestState,
    /// While it waits, the agent it waits on; none for the owner.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub waits_on: Option<String>,
    /// When it was decided, in RFC 3339.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub decided_at: Option<String>,
    /// The agent that decided it; none for the owner, and while it waits.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub decided_by: Option<String>,
    /// What the decider said.
    #[serde(default, deserialize_with = "crate::lenient_string")]
    pub note: String,
    /// What the grant leaves the agent able to reach beyond its sandbox.
    #[serde(default, deserialize_with = "crate::lenient")]
    pub warning: Option<String>,
}

/// A page of requests, the newest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestPage {
    /// The requests on this page that could be read.
    pub items: Vec<Request>,
    /// Whether older requests follow.
    pub has_more: bool,
    /// The `before` that reads the next page, when older requests follow.
    pub next_before_id: Option<i64>,
}

/// Which requests to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requests {
    /// Every waiting request, on one page.
    Pending,
    /// Every request, decided ones included, older than `before` when given.
    All {
        /// The `next_before_id` of the page before.
        before: Option<i64>,
    },
}

/// A change Backbone did not make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// Backbone refused it and said why, for the person.
    Refused {
        /// Its code, such as `outside_authority`.
        code: String,
        /// Its message.
        message: String,
    },
    /// The connection failed, the change could not be sent, or Backbone
    /// gave no reason; a refusal without one keeps its status.
    Connection(Error),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Connection(error)
    }
}

impl Connection {
    /// Read every registered agent's place and access, and the running
    /// swarms. Runs on an I/O worker.
    pub fn access_overview(&self) -> Result<Overview, Error> {
        let response = self.response("/api/agents", None, RESPONSE_LIMIT)?;
        successful(&response)?;
        let agents = decode_agents(&response.body).ok_or(Error::Invalid)?;
        // Swarms only group agents, so their own failure leaves them out.
        let swarms = match self.response("/api/swarms", None, RESPONSE_LIMIT) {
            Err(error @ (Error::BindingRequired | Error::BindingMismatch)) => return Err(error),
            Err(_) => None,
            Ok(response) => successful(&response).is_ok().then_some(response),
        };
        let mut snapshot = crate::Snapshot::default();
        snapshot.update_swarms(swarms.as_ref().map(|response| response.body.as_slice()));
        Ok(Overview {
            agents,
            swarms: snapshot.swarms,
        })
    }

    /// Read requests for access. Runs on an I/O worker.
    pub fn access_requests(&self, which: Requests) -> Result<RequestPage, Error> {
        let path = match which {
            Requests::Pending => "/api/access/requests?state=pending".to_owned(),
            Requests::All { before: None } => "/api/access/requests?state=all".to_owned(),
            Requests::All {
                before: Some(before),
            } => format!("/api/access/requests?state=all&before_id={before}"),
        };
        let response = self.response(&path, None, RESPONSE_LIMIT)?;
        successful(&response)?;
        decode_requests(&response.body).ok_or(Error::Invalid)
    }

    /// Grant `grant` to the agent `name`; granting a repository or folder it
    /// already has at the other access replaces it. Runs on an I/O worker.
    pub fn grant(&self, name: &str, grant: &Grant) -> Result<AgentAccess, Failure> {
        let body = json!({ "grant": sendable(grant)? });
        self.change_access("POST", agent_path(name, "grant")?, body)
    }

    /// Take `grant` away from the agent `name`. Runs on an I/O worker.
    pub fn revoke(&self, name: &str, grant: &Grant) -> Result<AgentAccess, Failure> {
        let body = json!({ "grant": sendable(grant)? });
        self.change_access("POST", agent_path(name, "revoke")?, body)
    }

    /// Set how the agent `name` is confined. Runs on an I/O worker.
    pub fn set_mode(&self, name: &str, mode: Mode) -> Result<AgentAccess, Failure> {
        if mode == Mode::Unknown {
            return Err(Error::Invalid.into());
        }
        self.change_access("PUT", agent_path(name, "mode")?, json!({ "mode": mode }))
    }

    /// Give the agent `name` a role and the agent directly above it, none
    /// for directly under the owner. Runs on an I/O worker.
    pub fn place(&self, name: &str, role: Role, superior: Option<&str>) -> Result<Placed, Failure> {
        if role == Role::Unknown || superior.is_some_and(|above| !crate::valid_agent(above)) {
            return Err(Error::Invalid.into());
        }
        self.change_access(
            "PUT",
            agent_path(name, "place")?,
            json!({ "role": role, "superior": superior }),
        )
    }

    /// Approve request `id`, with the decider's note, possibly empty. Runs
    /// on an I/O worker.
    pub fn approve(&self, id: i64, note: &str) -> Result<Request, Failure> {
        let path = format!("/api/access/requests/{id}/approve");
        self.change_access("POST", path, json!({ "note": note }))
    }

    /// Refuse request `id`, with the decider's note, possibly empty. Runs
    /// on an I/O worker.
    pub fn refuse(&self, id: i64, note: &str) -> Result<Request, Failure> {
        let path = format!("/api/access/requests/{id}/refuse");
        self.change_access("POST", path, json!({ "note": note }))
    }

    fn change_access<T: DeserializeOwned>(
        &self,
        method: &'static str,
        path: String,
        body: Value,
    ) -> Result<T, Failure> {
        let body = serde_json::to_vec(&body).map_err(|_| Error::Invalid)?;
        // The access routes take no request ID: a repeated change records
        // the same state, and the pane reads it again after each reply.
        let response = self.write(method, &path, Some(body), None)?;
        refusal(&response)?;
        Ok(serde_json::from_slice(&response.body).map_err(|_| Error::Invalid)?)
    }
}

fn agent_path(name: &str, action: &str) -> Result<String, Error> {
    if !crate::valid_agent(name) {
        return Err(Error::Invalid);
    }
    Ok(format!("/api/agents/{}/{action}", encode(name)))
}

/// A grant as a body carries it; one this version could not read is never
/// sent, as it would not be the same grant.
fn sendable(grant: &Grant) -> Result<Value, Error> {
    if *grant == Grant::Unknown {
        return Err(Error::Invalid);
    }
    serde_json::to_value(grant).map_err(|_| Error::Invalid)
}

/// A refused change keeps Backbone's code and message; one without a message
/// keeps its status.
fn refusal(response: &local::Response) -> Result<(), Failure> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let body: Value = serde_json::from_slice(&response.body).unwrap_or_default();
    let detail = &body["detail"];
    match (detail["code"].as_str(), detail["message"].as_str()) {
        (Some(code), Some(message)) if !message.trim().is_empty() => Err(Failure::Refused {
            code: code.to_owned(),
            message: message.to_owned(),
        }),
        _ => Err(successful(response).unwrap_err().into()),
    }
}

/// Registered agents from an agents reply. An item that cannot be read, a
/// session that is not registered or one Backbone does not manage is left
/// out; the reply itself must be readable.
fn decode_agents(body: &[u8]) -> Option<Vec<Agent>> {
    #[derive(Deserialize)]
    struct Document<'a> {
        #[serde(borrow)]
        items: Vec<&'a RawValue>,
    }
    let document: Document = serde_json::from_slice(body).ok()?;
    Some(
        document
            .items
            .into_iter()
            .filter_map(|raw| {
                let configured = serde_json::from_str::<Value>(raw.get())
                    .is_ok_and(|item| item["configured"] != Value::Bool(false));
                let agent: Agent = serde_json::from_str(raw.get()).ok()?;
                (configured && crate::valid_agent(&agent.name) && agent.role.is_some())
                    .then_some(agent)
            })
            .collect(),
    )
}

/// A page of requests; a request that cannot be read is left out.
fn decode_requests(body: &[u8]) -> Option<RequestPage> {
    #[derive(Deserialize)]
    struct Document<'a> {
        #[serde(borrow)]
        items: Vec<&'a RawValue>,
        #[serde(default)]
        has_more: bool,
        #[serde(default)]
        next_before_id: Option<i64>,
    }
    let document: Document = serde_json::from_slice(body).ok()?;
    Some(RequestPage {
        items: document
            .items
            .into_iter()
            .filter_map(|raw| serde_json::from_str(raw.get()).ok())
            .collect(),
        has_more: document.has_more,
        next_before_id: document.next_before_id,
    })
}

/// Grants, each read on its own: one that cannot be read is `Unknown`.
fn grants<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Grant>, D::Error> {
    let raw = <Box<RawValue> as Deserialize>::deserialize(deserializer)?;
    Ok(serde_json::from_str::<Vec<&RawValue>>(raw.get())
        .unwrap_or_default()
        .into_iter()
        .map(|grant| serde_json::from_str(grant.get()).unwrap_or(Grant::Unknown))
        .collect())
}

fn grant<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Grant, D::Error> {
    let raw = <Box<RawValue> as Deserialize>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).unwrap_or(Grant::Unknown))
}

fn reach<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Reach, D::Error> {
    let raw = <Box<RawValue> as Deserialize>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).unwrap_or(Reach::Unknown))
}

fn reached<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Reached>, D::Error> {
    // A tagged enum's fields arrive buffered, which raw JSON cannot borrow.
    let items = Value::deserialize(deserializer)?;
    let Value::Array(items) = items else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect())
}

fn mode<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Mode, D::Error> {
    let raw = <Box<RawValue> as Deserialize>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).unwrap_or(Mode::Unknown))
}

fn state<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<RequestState, D::Error> {
    let raw = <Box<RawValue> as Deserialize>::deserialize(deserializer)?;
    Ok(serde_json::from_str(raw.get()).unwrap_or(RequestState::Unknown))
}
