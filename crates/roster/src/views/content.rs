//! Content read through one verified connection. No desktop actions or local files.

use super::{Api, DocumentRevision, Provider, ReportPage, ReportQuery, documents, reports};
use crate::profiles::{Connection, Identity};

/// Retained content and explicitly supported read-only observation views.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Configured agents and swarms from the captured profile.
    Team,
    /// Tag membership and shared library associations.
    Tags,
    /// Policy layers with catalog identities and provider-owned base/swarm text.
    Policies,
    /// Skill catalog and provider previews, without reading local files.
    Skills,
    /// Token usage on the captured profile.
    Usage(super::Range),
    /// Deliveries, events and state history on the captured profile.
    Activity,
    /// A filtered feed page, including its original pagination cursor.
    Reports(ReportQuery, Option<String>),
    /// An explicitly opened answer or correction.
    Response(u64),
    /// An explicitly opened progress report.
    Report(u64),
    /// An explicitly opened HTML revision.
    Document(u64),
}

impl Request {
    /// Documents are read only on an explicit request, never periodically.
    pub fn explicit(&self) -> bool {
        matches!(
            self,
            Self::Document(_) | Self::Reports(ReportQuery::Documents(..), _)
        )
    }
}

/// Retained content without legacy URL-based action authority.
#[derive(Clone, Debug, PartialEq)]
pub enum Data {
    /// Team observation from the originating connection.
    Team(super::Team),
    /// Tag library from the originating connection.
    Tags(Box<super::TagLibrary>),
    /// Policy library from the originating connection.
    Policies(Box<super::PolicyLibrary>),
    /// Skill catalog from the originating connection.
    Skills(Box<super::SkillLibrary>),
    /// Token usage from the originating connection.
    Usage(super::Usage),
    /// Deliveries, events and state history from the originating connection.
    Activity(super::Activity),
    /// A feed page or a single report/answer. Item provider URLs stay empty.
    Reports(ReportPage),
    /// Size- and digest-checked HTML from the originating connection.
    Document(DocumentRevision),
}

/// A bounded failure with no credentials or provider-supplied error prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The captured connection failed, including a binding refusal.
    Connection(crate::profiles::Error),
    /// Existing fixed content diagnostics, including unavailable or not found.
    Content(&'static str),
    /// The caller no longer wants this read chain.
    Cancelled,
}

/// One observation, always labelled with its original identity and request.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    /// Full authenticated origin, including the accepting lifecycle.
    pub identity: Identity,
    /// Feed filters, cursor or detail ID answered by this observation.
    pub request: Request,
    /// Content or a failure from this origin only.
    pub result: Result<Data, Error>,
}

/// Read on an I/O worker, retaining the same connection for every chained GET.
/// Pagination and later detail requests must use this same connection. No
/// environment, legacy configuration, local runtime files or actions are read.
pub fn read(connection: &Connection, request: &Request, live: &dyn Fn() -> bool) -> Read {
    let observation = match request {
        Request::Team => Some(super::bound::Request::Team),
        Request::Tags => Some(super::bound::Request::Tags),
        Request::Policies => Some(super::bound::Request::Policies),
        Request::Skills => Some(super::bound::Request::Skills),
        Request::Usage(range) => Some(super::bound::Request::Usage(*range)),
        Request::Activity => Some(super::bound::Request::Activity),
        _ => None,
    };
    if let Some(observation) = observation {
        let read = super::bound::read(connection, &observation, live);
        return Read {
            identity: read.identity,
            request: request.clone(),
            result: read.result.and_then(|data| match data {
                super::Data::Team(team) => Ok(Data::Team(team)),
                super::Data::Tags(tags) => Ok(Data::Tags(tags)),
                super::Data::Policies(policies) => Ok(Data::Policies(policies)),
                super::Data::Skills(skills) => Ok(Data::Skills(skills)),
                super::Data::Usage(usage) => Ok(Data::Usage(usage)),
                super::Data::Activity(activity) => Ok(Data::Activity(activity)),
                _ => Err(Error::Content(super::UNREADABLE)),
            }),
        };
    }
    let api = Api {
        provider: Provider::Bound {
            connection,
            error: Default::default(),
        },
        live,
    };
    let result = match request {
        Request::Team
        | Request::Tags
        | Request::Policies
        | Request::Skills
        | Request::Usage(_)
        | Request::Activity => {
            unreachable!("observation handled above")
        }
        Request::Reports(query, agent) => {
            reports::read(&api, query, agent.as_deref()).map(Data::Reports)
        }
        Request::Response(id) => reports::read_response(&api, *id).map(Data::Reports),
        Request::Report(id) => reports::read_report(&api, *id).map(Data::Reports),
        Request::Document(id) => documents::read(&api, *id).map(Data::Document),
    };
    let Provider::Bound { error, .. } = &api.provider else {
        unreachable!()
    };
    // Enrichment is optional for ordinary HTTP refusals or unreadable metadata,
    // but a failed bound transport invalidates the whole chain, even if the
    // content decoder otherwise succeeded. Cancellation also discards results.
    let result = if let Some(error) = error.get() {
        Err(Error::Connection(error))
    } else if !live() {
        Err(Error::Cancelled)
    } else {
        result.map_err(Error::Content)
    };
    Read {
        identity: connection.identity().clone(),
        request: request.clone(),
        result,
    }
}

/// Read one agent card section on the same verified connection, into an agent
/// already inspected under its saved registration. No local files are read.
pub fn agent_part(
    connection: &Connection,
    part: &super::Part,
    agent: &mut super::Agent,
    live: &dyn Fn() -> bool,
) -> Result<(), Error> {
    if !crate::valid_agent(&agent.name) {
        return Err(Error::Content(super::UNREADABLE));
    }
    let api = Api {
        provider: Provider::Bound {
            connection,
            error: Default::default(),
        },
        live,
    };
    let name = agent.name.clone();
    super::read_part(&api, &name, part, agent);
    let Provider::Bound { error, .. } = &api.provider else {
        unreachable!()
    };
    if let Some(error) = error.get() {
        Err(Error::Connection(error))
    } else if !live() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

/// Read plan-view runtime metadata on the same verified profile as its plans.
/// A binding refusal invalidates the complete metadata read.
pub fn members(
    connection: &Connection,
) -> Result<std::collections::BTreeMap<String, super::Member>, Error> {
    let api = Api {
        provider: Provider::Bound {
            connection,
            error: Default::default(),
        },
        live: &|| true,
    };
    let result = super::library::agents(&api).map(|agents| {
        agents
            .into_iter()
            .map(|agent| (agent.name.clone(), agent))
            .collect()
    });
    let Provider::Bound { error, .. } = &api.provider else {
        unreachable!()
    };
    match error.get() {
        Some(error) => Err(Error::Connection(error)),
        None => result.map_err(Error::Content),
    }
}
