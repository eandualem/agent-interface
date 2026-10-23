//! Agent Backbone views: information shown in a pane in place of a terminal.
//!
//! A [`ViewPane`] exists only while its pane is shown, and polls its data on
//! an I/O worker until then (`crate::roster::ViewSource`). Each view draws its
//! data in its own module from the shared pieces in `components`, following
//! the design boards. Backbone is only read; nothing here changes an agent.

use std::{collections::HashSet, path::PathBuf, time::Duration};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use tmux_workspaces_core::{View, ViewKind};
use tmux_workspaces_roster::views::{self, Data, Range, Request};

use super::{
    components as ui,
    design::{colors, tokens},
    name::{NameEvent, NameField},
};
use crate::roster::ViewSource;

mod activity;
mod agent;
mod markdown;
mod skills;
mod team;
mod usage;

/// A kind of view's icon and name.
pub(super) struct About {
    pub icon: &'static str,
    pub label: &'static str,
}

/// The icon and name of each kind.
pub(super) fn about(kind: ViewKind) -> About {
    let (icon, label) = match kind {
        ViewKind::Skills => ("diamond", "Skills"),
        ViewKind::Agent => ("user", "Agent"),
        ViewKind::Usage => ("chart-line", "Usage"),
        ViewKind::Activity => ("activity", "Activity"),
        ViewKind::Team => ("bot", "Team"),
    };
    About { icon, label }
}

/// What a view asks its owner to do.
pub(super) enum ViewEvent {
    /// Show the tab holding this agent's terminal.
    ShowAgent(String),
    /// Open this view in a new pane beside this one.
    OpenBeside(View),
}

/// How the team view groups agents.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Grouping {
    Repository,
    Tag,
    Swarm,
}

/// How the usage view groups its rows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UsageBy {
    Agent,
    Model,
    Cli,
}

/// Which records the activity view lists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Records {
    All,
    Deliveries,
    GitHub,
    Held,
}

/// The agent card's sections.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Section {
    Overview,
    Permissions,
    Connections,
}

/// A skill being read: its place in the shown list, its text, how it is shown.
pub(super) struct Reading {
    pub index: usize,
    pub text: Option<Result<(String, bool), &'static str>>,
    pub source: bool,
}

/// One view in one pane, with its newest data and what the person chose in it.
pub(super) struct ViewPane {
    pub view: View,
    source: Option<ViewSource>,
    data: Option<Data>,
    error: Option<&'static str>,
    range: Range,
    grouping: Grouping,
    usage_by: UsageBy,
    cost: bool,
    expanded: HashSet<String>,
    records: Records,
    section: Section,
    tag: Option<String>,
    query: String,
    search: Option<Entity<NameField>>,
    reading: Option<Reading>,
    _updates: Option<Task<()>>,
    _search: Option<Subscription>,
}

impl EventEmitter<ViewEvent> for ViewPane {}

impl ViewPane {
    /// Start polling `view`. Without Backbone, the pane says so and polls nothing.
    pub fn new(
        view: View,
        backbone: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut pane = Self {
            view,
            source: None,
            data: None,
            error: None,
            range: Range::Day,
            grouping: Grouping::Repository,
            usage_by: UsageBy::Agent,
            cost: false,
            expanded: HashSet::new(),
            records: Records::All,
            section: Section::Overview,
            tag: None,
            query: String::new(),
            search: None,
            reading: None,
            _updates: None,
            _search: None,
        };
        if pane.view.kind == ViewKind::Skills {
            let field = cx.new(|cx| NameField::search(window, cx));
            pane._search = Some(cx.subscribe(&field, |pane, _, event, cx| {
                if let NameEvent::Changed(query) = event {
                    pane.query = query.clone();
                    cx.notify();
                }
            }));
            pane.search = Some(field);
        }
        let Some(directory) = backbone else {
            pane.error = Some("Agent Backbone is not set up on this computer.");
            return pane;
        };
        // Backbone collects usage about once a minute; everything else is live.
        let every = Duration::from_secs(if pane.view.kind == ViewKind::Usage {
            15
        } else {
            3
        });
        let (wake, mut woken) = mpsc::unbounded();
        match ViewSource::start(directory, pane.request(), every, move || {
            let _ = wake.unbounded_send(());
        }) {
            Ok(source) => pane.source = Some(source),
            Err(_) => pane.error = Some(views::UNAVAILABLE),
        }
        pane._updates = Some(cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                let updated = this.update(cx, |pane, cx| {
                    if let Some((request, update)) =
                        pane.source.as_ref().and_then(ViewSource::take_update)
                    {
                        // A read for an earlier request, such as another
                        // usage range, must not be shown under the new one.
                        if request != pane.request() {
                            return;
                        }
                        match update {
                            Ok(data) => {
                                pane.data = Some(data);
                                pane.error = None;
                            }
                            // Keep the last data and say it is stale.
                            Err(error) => pane.error = Some(error),
                        }
                        cx.notify();
                    }
                });
                if updated.is_err() {
                    break;
                }
            }
        }));
        pane
    }

    fn request(&self) -> Request {
        match self.view.kind {
            ViewKind::Team => Request::Team,
            ViewKind::Usage => Request::Usage(self.range),
            ViewKind::Activity => Request::Activity,
            ViewKind::Skills => Request::Skills,
            ViewKind::Agent => Request::Agent(self.view.agent.clone().unwrap_or_default()),
        }
    }

    fn set_range(&mut self, range: Range, cx: &mut Context<Self>) {
        if self.range != range {
            self.range = range;
            // The old range's totals must not appear under the new label.
            self.data = None;
            // Without a worker nothing will arrive, so keep saying why.
            if self.source.is_some() {
                self.error = None;
            }
            if let Some(source) = &self.source {
                source.set_request(self.request());
            }
            cx.notify();
        }
    }

    /// Open the skill at `index` of the shown list and read its text.
    fn read_skill(&mut self, index: usize, folder: String, cx: &mut Context<Self>) {
        let source = self.reading.as_ref().is_some_and(|reading| reading.source);
        self.reading = Some(Reading {
            index,
            text: None,
            source,
        });
        cx.notify();
        let read = cx
            .background_executor()
            .spawn(async move { views::read_skill(&folder) });
        cx.spawn(async move |this, cx| {
            let text = read.await;
            let _ = this.update(cx, |pane, cx| {
                if let Some(reading) = &mut pane.reading
                    && reading.index == index
                {
                    reading.text = Some(text);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Move to the previous or next skill while reading.
    fn step_skill(&mut self, step: isize, cx: &mut Context<Self>) {
        let (Some(reading), Some(Data::Skills(data))) = (&self.reading, &self.data) else {
            return;
        };
        let shown = skills::shown(data, self.tag.as_deref(), &self.query);
        let next = reading.index as isize + step;
        if let Some(skill) = usize::try_from(next).ok().and_then(|n| shown.get(n)) {
            let folder = skill.path.clone();
            self.read_skill(next as usize, folder, cx);
        }
    }

    /// Keys a view understands while its pane is selected. Returns whether
    /// `key` was one of them.
    pub fn key(&mut self, key: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        if key.modifiers.modified() || self.reading.is_none() {
            return false;
        }
        match key.key.as_str() {
            "escape" => self.reading = None,
            "s" => {
                if let Some(reading) = &mut self.reading {
                    reading.source = !reading.source;
                }
            }
            "[" => self.step_skill(-1, cx),
            "]" => self.step_skill(1, cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// The title shown for a view.
    pub fn title(view: &View) -> SharedString {
        match &view.agent {
            Some(agent) => agent.clone().into(),
            None => about(view.kind).label.into(),
        }
    }
}

impl Drop for ViewPane {
    // Stopping the worker waits for its current request, which can take
    // seconds; never do that on the UI thread.
    fn drop(&mut self) {
        if let Some(source) = self.source.take() {
            std::thread::spawn(move || drop(source));
        }
    }
}

impl Render for ViewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let about = about(self.view.kind);
        // Lent to the body for this frame and put back below.
        let data = self.data.take();
        let body = match &data {
            None => {
                let state = match self.error {
                    Some(error) => ui::empty_state(
                        ui::empty_tile("unplug", c.selected, c.muted),
                        "No data",
                        error,
                    ),
                    None => ui::empty_state(
                        ui::empty_tile(about.icon, c.selected, c.muted),
                        "Reading Agent Backbone…",
                        "",
                    ),
                };
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(ui::view_header(about.icon, ViewPane::title(&self.view)))
                    .when(self.view.kind == ViewKind::Usage, |el| {
                        el.child(usage::toolbar(self, cx))
                    })
                    .child(state)
                    .into_any_element()
            }
            Some(Data::Team(data)) => team::body(self, data, cx),
            Some(Data::Usage(data)) => usage::body(self, data, cx),
            Some(Data::Activity(data)) => activity::body(self, data, cx),
            Some(Data::Skills(data)) => skills::body(self, data, cx),
            Some(Data::Agent(data)) => agent::body(self, data, cx),
        };
        self.data = data;
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(div().flex_1().min_h_0().flex().flex_col().child(body))
            .children(
                self.data
                    .is_some()
                    .then_some(self.error)
                    .flatten()
                    .map(|error| {
                        ui::text(
                            div()
                                .flex_shrink_0()
                                .px(px(t.space.s14))
                                .py(px(t.space.s4))
                                .bg(c.attention_soft)
                                .text_color(c.attention_text),
                            &t.text.meta,
                        )
                        .child(format!("{error} Showing the last data read."))
                    }),
            )
    }
}

/// A scrolling list area under a view's toolbar.
fn list(id: &'static str) -> gpui::Stateful<gpui::Div> {
    let t = tokens();
    div()
        .id(id)
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .overflow_y_scroll()
        .px(px(t.space.s8))
        .py(px(t.space.s6))
}

/// A token count in a few characters: 4.82M, 312k, 950.
fn count(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{}k", tokens / 1_000),
        1_000_000..1_000_000_000 => format!("{:.2}M", tokens as f64 / 1e6),
        _ => format!("{:.2}B", tokens as f64 / 1e9),
    }
}

/// How long ago, in one unit: 40s, 6m, 2h, 3d.
fn ago(seconds: f64) -> String {
    let seconds = seconds.max(0.0) as u64;
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// How long ago an epoch time was.
fn since(at: Option<f64>) -> String {
    at.map(|at| ago(views::now() as f64 - at))
        .unwrap_or_default()
}

/// Text without control characters, on one line.
fn line(text: &str) -> String {
    text.chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect()
}

fn mono(text: impl Into<SharedString>) -> gpui::Div {
    div()
        .font_family(tokens().font.mono.clone())
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_ages_stay_short() {
        assert_eq!(count(950), "950");
        assert_eq!(count(312_400), "312k");
        assert_eq!(count(4_820_000), "4.82M");
        assert_eq!(count(40_180_741_701), "40.18B");
        assert_eq!(ago(40.0), "40s");
        assert_eq!(ago(400.0), "6m");
        assert_eq!(ago(7_300.0), "2h");
        assert_eq!(ago(-5.0), "0s");
        assert_eq!(line("a\nb"), "a b");
    }

    #[test]
    fn every_kind_has_a_distinct_label() {
        let labels: HashSet<_> = ViewKind::ALL
            .iter()
            .map(|kind| about(*kind).label)
            .collect();
        assert_eq!(labels.len(), ViewKind::ALL.len());
    }
}
