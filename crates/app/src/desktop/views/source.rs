//! A pane's worker and origin stay together through navigation and visibility.

use crate::roster::{BoundContentSource, ViewSource, ViewUpdate};
use agent_interface_roster::{
    profiles::{Connection, Identity},
    views::{self, content},
};
use std::{cell::Cell, io, path::PathBuf, time::Duration};

pub(super) enum Source {
    Legacy(ViewSource),
    Bound {
        worker: BoundContentSource,
        identity: Identity,
        generation: Cell<u64>,
    },
}

fn request(request: &views::Request) -> Option<content::Request> {
    Some(match request {
        views::Request::Reports(query, agent) => {
            content::Request::Reports(query.clone(), agent.clone())
        }
        views::Request::Response(id) => content::Request::Response(*id),
        views::Request::Report(id) => content::Request::Report(*id),
        views::Request::Document(id) => content::Request::Document(*id),
        _ => return None,
    })
}

impl Source {
    pub fn start(
        directory: Option<PathBuf>,
        connection: Option<&Connection>,
        wanted: views::Request,
        every: Duration,
        wake: impl Fn() + Send + 'static,
    ) -> io::Result<Self> {
        if let Some(connection) = connection {
            let wanted = request(&wanted).ok_or_else(|| {
                io::Error::other("This view does not support bound profiles yet.")
            })?;
            Ok(Self::Bound {
                worker: BoundContentSource::start(connection.clone(), wanted, every, wake)?,
                identity: connection.identity().clone(),
                generation: Cell::new(0),
            })
        } else {
            Ok(Self::Legacy(ViewSource::start(
                directory.ok_or_else(|| io::Error::other("Backbone is not configured."))?,
                wanted,
                every,
                wake,
            )?))
        }
    }

    pub fn set_active(&self, active: bool) {
        match self {
            Self::Legacy(worker) => worker.set_active(active),
            Self::Bound {
                worker, generation, ..
            } => generation.set(worker.set_active(active)),
        }
    }

    pub fn set_request(&self, wanted: views::Request) {
        match self {
            Self::Legacy(worker) => worker.set_request(wanted),
            Self::Bound {
                worker, generation, ..
            } => {
                if let Some(wanted) = request(&wanted) {
                    generation.set(worker.set_request(wanted));
                } else {
                    generation.set(worker.set_active(false));
                }
            }
        }
    }

    pub fn take_update(&self) -> Option<ViewUpdate> {
        match self {
            Self::Legacy(worker) => worker.take_update(),
            Self::Bound {
                worker,
                identity,
                generation,
            } => {
                let update = worker.take_update()?;
                if !accepts(identity, generation.get(), &update) {
                    return None;
                }
                let wanted = match update.read.request {
                    content::Request::Reports(query, agent) => {
                        views::Request::Reports(query, agent)
                    }
                    content::Request::Response(id) => views::Request::Response(id),
                    content::Request::Report(id) => views::Request::Report(id),
                    content::Request::Document(id) => views::Request::Document(id),
                };
                let result = update
                    .read
                    .result
                    .map(|data| match data {
                        content::Data::Reports(page) => views::Data::Reports(page),
                        content::Data::Document(document) => views::Data::Document(document),
                    })
                    .map_err(|error| match error {
                        content::Error::Content(message) => message,
                        content::Error::Connection(
                            agent_interface_roster::profiles::Error::BindingRequired
                            | agent_interface_roster::profiles::Error::BindingMismatch,
                        ) => "The original profile binding was refused. Reconnect to that profile.",
                        _ => views::UNAVAILABLE,
                    });
                Some((wanted, result))
            }
        }
    }
}

fn accepts(
    identity: &Identity,
    generation: u64,
    update: &crate::roster::BoundContentUpdate,
) -> bool {
    &update.read.identity == identity && update.generation == generation
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deferred_updates_require_original_identity_and_generation() {
        let origin = Identity {
            profile_id: "a".into(),
            installation_id: "install".into(),
            lifecycle_id: "life".into(),
        };
        let mut update = crate::roster::BoundContentUpdate {
            generation: 3,
            read: content::Read {
                identity: origin.clone(),
                request: content::Request::Response(7),
                result: Err(content::Error::Cancelled),
            },
        };
        assert!(accepts(&origin, 3, &update));
        assert!(!accepts(&origin, 4, &update));
        for changed in [
            Identity {
                profile_id: "b".into(),
                ..origin.clone()
            },
            Identity {
                installation_id: "replacement".into(),
                ..origin.clone()
            },
            Identity {
                lifecycle_id: "restored".into(),
                ..origin.clone()
            },
        ] {
            update.read.identity = changed;
            assert!(!accepts(&origin, 3, &update));
        }
        assert!(request(&views::Request::Team).is_none());
    }

    #[test]
    fn pane_sources_keep_a_delayed_read_and_navigation_with_their_original_provider() {
        use crate::roster::content::tests::{Reply, provider, record};
        use std::sync::mpsc;
        let (arrived, reading) = mpsc::channel();
        let (release, delayed) = mpsc::channel();
        let mut next = record("a-next");
        next["id"] = 8.into();
        let (_a, a, server_a) = provider(
            "a",
            vec![
                Reply {
                    delay: Some((arrived, delayed)),
                    ..Reply::ok("/api/responses/7", record("a-old"))
                },
                Reply::ok("/api/responses/8", next),
            ],
        );
        let (_b, b, server_b) = provider("b", vec![Reply::ok("/api/responses/7", record("b"))]);
        let (wa, ra) = mpsc::channel();
        let (wb, rb) = mpsc::channel();
        let source_a = Source::start(
            None,
            Some(&a),
            views::Request::Response(7),
            Duration::from_secs(3600),
            move || {
                let _ = wa.send(());
            },
        )
        .unwrap();
        reading.recv_timeout(Duration::from_secs(5)).unwrap();
        source_a.set_request(views::Request::Response(8));
        std::fs::write(a.credential_reference(), "BACKBONE_API_KEY=rotated\n").unwrap();
        let source_b = Source::start(
            None,
            Some(&b),
            views::Request::Response(7),
            Duration::from_secs(3600),
            move || {
                let _ = wb.send(());
            },
        )
        .unwrap();
        rb.recv_timeout(Duration::from_secs(5)).unwrap();
        let (_, result) = source_b.take_update().unwrap();
        assert!(format!("{result:?}").contains("sentinel-b"));
        release.send(()).unwrap();
        ra.recv_timeout(Duration::from_secs(5)).unwrap();
        let (request, result) = source_a.take_update().unwrap();
        assert_eq!(request, views::Request::Response(8));
        assert!(format!("{result:?}").contains("sentinel-a-next"));
        assert!(source_b.take_update().is_none());
        drop(source_a);
        drop(source_b);
        server_a.join().unwrap();
        server_b.join().unwrap();
    }
}
