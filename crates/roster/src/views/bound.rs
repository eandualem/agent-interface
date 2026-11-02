//! General read-only views on one verified connection. The closed request set
//! excludes agent inspection, local runtime configuration and filesystem reads.

pub use super::content::Error;
use super::{Api, Data, Provider, Range};
use crate::profiles::{Connection, Identity};

/// GET-only view families that can be read without ambient runtime state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// The configured agents and running swarms.
    Team,
    /// Token usage over a recent period.
    Usage(Range),
    /// Recorded deliveries, events and optional state history.
    Activity,
    /// The provider's skill catalog and bounded agent previews.
    Skills,
    /// The provider's policy library and bounded agent previews.
    Policies,
    /// Tags and their configured skill and policy associations.
    Tags,
}

impl Request {
    fn remote(&self) -> super::Request {
        match self {
            Self::Team => super::Request::Team,
            Self::Usage(range) => super::Request::Usage(*range),
            Self::Activity => super::Request::Activity,
            Self::Skills => super::Request::Skills,
            Self::Policies => super::Request::Policies,
            Self::Tags => super::Request::Tags,
        }
    }
}

/// One observation labelled with the full origin and original request.
#[derive(Clone, Debug, PartialEq)]
pub struct Read {
    /// The authenticated profile, installation and lifecycle identity.
    pub identity: Identity,
    /// The view answered by this observation.
    pub request: Request,
    /// Data or a bounded failure from the captured connection only.
    pub result: Result<Data, Error>,
}

/// Read a bounded GET chain using the original binding and credentials.
/// Optional enrichment can fail ordinarily, but any connection refusal or
/// cancellation discards the complete observation. Call on an I/O worker.
/// This does not enable native navigation, local skill previews or actions.
pub fn read(connection: &Connection, request: &Request, live: &dyn Fn() -> bool) -> Read {
    let api = Api {
        provider: Provider::Bound {
            connection,
            error: Default::default(),
        },
        live,
    };
    // The closed request set cannot reach legacy agent/runtime file reads.
    let result = super::read_using(&api, &request.remote());
    let Provider::Bound { error, .. } = &api.provider else {
        unreachable!()
    };
    let result = if let Some(error) = error.get() {
        Err(Error::Connection(error))
    } else if !live() {
        Err(Error::Cancelled)
    } else {
        result.map_err(Error::Content)
    };
    let result = result.and_then(|mut data| {
        if let Data::Policies(library) = &mut data {
            library.bind_shared(connection, live)?;
        }
        Ok(data)
    });
    Read {
        identity: connection.identity().clone(),
        request: request.clone(),
        result,
    }
}
