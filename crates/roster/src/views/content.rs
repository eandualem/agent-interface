//! Retained content read through one verified connection. No desktop actions.

use super::{Api, DocumentRevision, Provider, ReportPage, ReportQuery, documents, reports};
use crate::profiles::{Connection, Identity};

/// Only retained-content endpoints; general views may still read local settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
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
    let api = Api {
        provider: Provider::Bound {
            connection,
            error: Default::default(),
        },
        live,
    };
    let result = match request {
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
