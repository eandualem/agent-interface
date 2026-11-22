//! One owner-bound overview and invalidation stream per verified profile connection.
use agent_interface_roster::profiles::{Connection, ConnectionKey, Error, peers as api};
use futures::{StreamExt, channel::mpsc};
use gpui::{App, AppContext, Context, Entity, Global, Task, WeakEntity};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Selection {
    Request(String),
    Peer(api::Generation),
}

pub(super) struct Summary {
    pub overview: api::Overview,
    counts: HashMap<String, usize>,
}

impl Summary {
    fn read(c: &Connection) -> Result<Self, Error> {
        let overview = c.peer_overview()?;
        let mut counts = HashMap::new();
        if overview.pending_reviews > 0 {
            let mut before = None;
            let mut seen = HashSet::new();
            loop {
                let page = c.peer_reviews(before.as_deref())?;
                for review in page.items {
                    *counts
                        .entry(review.message.connection.generation)
                        .or_default() += 1;
                }
                let Some(cursor) = page.older else {
                    break;
                };
                if !seen.insert(cursor.clone()) {
                    return Err(Error::Invalid);
                }
                before = Some(cursor);
            }
        }
        Ok(Self { overview, counts })
    }

    pub fn count(&self, generation: &str) -> usize {
        self.counts.get(generation).copied().unwrap_or(0)
    }

    pub fn requests(&self) -> usize {
        self.overview
            .invitations
            .iter()
            .filter(|r| r.state == "pending" && r.direction == api::Direction::Incoming)
            .count()
    }

    pub fn attention(&self) -> bool {
        self.overview.pending_reviews > 0
            || self.requests() > 0
            || self
                .overview
                .invitations
                .iter()
                .any(|r| r.direction == api::Direction::Outgoing && r.state == "declined")
    }

    pub fn label(&self) -> String {
        let reviews = self.overview.pending_reviews;
        let requests = self.requests();
        let mut labels = Vec::new();
        if reviews > 0 {
            labels.push(format!("{reviews} review"));
        }
        if requests > 0 {
            labels.push(format!("{requests} req"));
        }
        if labels.is_empty() {
            format!(
                "{} connected",
                self.overview
                    .connections
                    .iter()
                    .filter(|p| p.state == "connected")
                    .count()
            )
        } else {
            labels.join(" · ")
        }
    }

    pub fn attention_rows(&self) -> Vec<(Selection, String, String)> {
        let mut rows = Vec::new();
        for r in &self.overview.invitations {
            let label = match (r.direction, r.state.as_str()) {
                (api::Direction::Incoming, "pending") => "request",
                (api::Direction::Outgoing, "declined") => "declined",
                _ => continue,
            };
            rows.push((
                Selection::Request(r.request_id.clone()),
                r.display_name.clone().unwrap_or_else(|| r.peer_id.clone()),
                label.into(),
            ));
        }
        for p in &self.overview.connections {
            let n = self.count(&p.connection.generation);
            if n > 0 {
                rows.push((
                    Selection::Peer(p.connection.clone()),
                    p.display_name
                        .clone()
                        .unwrap_or_else(|| p.connection.peer_id.clone()),
                    format!("{n} review"),
                ));
            }
        }
        rows
    }
}

#[derive(Default)]
pub(super) struct Sources(HashMap<ConnectionKey, WeakEntity<Inbox>>);
impl Global for Sources {}

pub(super) struct Inbox {
    pub summary: Option<Arc<Summary>>,
    pub error: Option<String>,
    pub read_at: Option<String>,
    service: Connection,
    alive: Arc<AtomicBool>,
    online: Arc<AtomicBool>,
    reading: bool,
    dirty: bool,
    _events: Task<()>,
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
    }
}

impl Inbox {
    pub fn shared(connection: &Connection, cx: &mut App) -> Entity<Self> {
        let key = connection.connection_key();
        if let Some(existing) = cx
            .try_global::<Sources>()
            .and_then(|s| s.0.get(&key))
            .and_then(|e| e.upgrade())
        {
            return existing;
        }
        let inbox = cx.new(|cx| Self::new(connection.clone(), cx));
        if !cx.has_global::<Sources>() {
            cx.set_global(Sources::default());
        }
        let sources = cx.global_mut::<Sources>();
        sources.0.retain(|_, e| e.upgrade().is_some());
        sources.0.insert(key, inbox.downgrade());
        inbox
    }

    fn new(service: Connection, cx: &mut Context<Self>) -> Self {
        let alive = Arc::new(AtomicBool::new(true));
        let online = Arc::new(AtomicBool::new(false));
        let opened = online.clone();
        let running = alive.clone();
        let c = service.clone();
        let (mut tx, mut rx) = mpsc::channel(1);
        let worker = std::thread::Builder::new()
            .name("peer-connections".into())
            .spawn(move || {
                let mut cursor = None;
                let live = || running.load(Ordering::Acquire);
                while live() {
                    let _ = c.watch_peers(&mut cursor, &live, &mut || {
                        opened.store(true, Ordering::Release);
                        let _ = tx.try_send(());
                    });
                    if !live() {
                        break;
                    }
                    opened.store(false, Ordering::Release);
                    let _ = tx.try_send(());
                    for _ in 0..12 {
                        if !live() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(250));
                    }
                }
            });
        let events = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                if this.update(cx, |inbox, cx| inbox.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            summary: None,
            error: worker.err().map(|e| e.to_string()),
            read_at: None,
            service,
            alive,
            online,
            reading: false,
            dirty: false,
            _events: events,
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.online.load(Ordering::Acquire) {
            self.error = Some("Agent Backbone stream unavailable".into());
            cx.notify();
            return;
        }
        if self.reading {
            self.dirty = true;
            return;
        }
        self.reading = true;
        let c = self.service.clone();
        let read = cx
            .background_executor()
            .spawn(async move { Summary::read(&c) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |inbox, cx| {
                inbox.reading = false;
                if inbox.online.load(Ordering::Acquire) {
                    match result {
                        Ok(summary) => {
                            inbox.summary = Some(Arc::new(summary));
                            inbox.error = None;
                            inbox.read_at = Some(chrono::Local::now().format("%H:%M").to_string());
                        }
                        Err(error) => {
                            inbox.error = Some(format!("Agent Backbone unavailable · {error:?}"))
                        }
                    }
                }
                if cx.has_global::<Sources>() {
                    let _ = cx.global_mut::<Sources>();
                }
                if std::mem::take(&mut inbox.dirty) {
                    inbox.refresh(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Sources {
    pub fn attention(profile: &str, cx: &App) -> Option<String> {
        let sources = cx.try_global::<Sources>()?;
        sources
            .0
            .values()
            .filter_map(|s| s.upgrade())
            .find_map(|e| {
                let inbox = e.read(cx);
                (inbox.service.identity().profile_id == profile)
                    .then(|| {
                        inbox
                            .summary
                            .as_ref()
                            .filter(|s| s.attention())
                            .map(|s| s.label())
                    })
                    .flatten()
            })
    }
}
