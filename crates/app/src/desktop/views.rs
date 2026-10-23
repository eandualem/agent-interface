//! Agent Backbone views: information shown in a pane in place of a terminal.
//!
//! A [`ViewPane`] exists only while its pane is shown, and polls its data on
//! an I/O worker until then (`crate::roster::ViewSource`). Each view draws its
//! data in its own module from the shared pieces in `components`. Backbone is
//! only read; nothing here starts a process or changes an agent.

use std::{path::PathBuf, time::Duration};

use futures::{StreamExt, channel::mpsc};
use gpui::{Context, EventEmitter, SharedString, Task, div, prelude::*, px};
use tmux_workspaces_core::{View, ViewKind};
use tmux_workspaces_roster::views::{self, Data, Range, Request};

use super::{
    components as ui,
    design::{colors, tokens},
};
use crate::roster::ViewSource;

mod activity;
mod agent;
mod skills;
mod team;
mod usage;

/// What the new-pane views list shows for one kind of view.
pub(super) struct About {
    pub icon: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub scope: &'static str,
}

/// The views list entry and pane title of each kind.
pub(super) fn about(kind: ViewKind) -> About {
    let (icon, label, description, scope) = match kind {
        ViewKind::Skills => (
            "diamond",
            "Skills",
            "The shared store, with who gets each skill",
            "all agents",
        ),
        ViewKind::Agent => (
            "user",
            "Agent",
            "One agent's state, question, work and recent messages",
            "pick an agent",
        ),
        ViewKind::Usage => (
            "chart-line",
            "Usage",
            "Tokens by agent, as each CLI recorded them",
            "all agents",
        ),
        ViewKind::Activity => (
            "activity",
            "Activity",
            "Recent deliveries and inbound events",
            "all agents",
        ),
        ViewKind::Team => (
            "users",
            "Team",
            "Agents by repository, with their states and swarms",
            "all agents",
        ),
    };
    About {
        icon,
        label,
        description,
        scope,
    }
}

/// What a view asks its owner to do.
pub(super) enum ViewEvent {
    /// Show the tab holding this agent's terminal.
    ShowAgent(String),
}

/// Which records the activity view lists.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Records {
    All,
    Deliveries,
    Events,
}

/// One view in one pane, with its newest data.
pub(super) struct ViewPane {
    pub view: View,
    source: Option<ViewSource>,
    data: Option<Data>,
    error: Option<&'static str>,
    range: Range,
    records: Records,
    _updates: Option<Task<()>>,
}

impl EventEmitter<ViewEvent> for ViewPane {}

impl ViewPane {
    /// Start polling `view`. Without Backbone, the pane says so and polls nothing.
    pub fn new(view: View, backbone: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let range = Range::Day;
        let mut pane = Self {
            view,
            source: None,
            data: None,
            error: None,
            range,
            records: Records::All,
            _updates: None,
        };
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
            self.error = None;
            if let Some(source) = &self.source {
                source.set_request(self.request());
            }
            cx.notify();
        }
    }

    fn set_records(&mut self, records: Records, cx: &mut Context<Self>) {
        self.records = records;
        cx.notify();
    }

    /// The title shown in the pane header.
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
    fn render(&mut self, _: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let body = match &self.data {
            None => match self.error {
                Some(error) => ui::empty_state(
                    ui::empty_tile("unplug", c.selected, c.muted),
                    "No data",
                    error,
                )
                .into_any_element(),
                None => ui::empty_state(
                    ui::empty_tile(about(self.view.kind).icon, c.selected, c.muted),
                    "Reading Agent Backbone…",
                    "",
                )
                .into_any_element(),
            },
            Some(Data::Team(data)) => team::body(data, cx),
            Some(Data::Usage(data)) => usage::body(data, self.range, cx),
            Some(Data::Activity(data)) => activity::body(data, self.records, cx),
            Some(Data::Skills(data)) => skills::body(data),
            Some(Data::Agent(data)) => agent::body(data, cx),
        };
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
        .py(px(t.space.s4))
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
        let labels: std::collections::HashSet<_> = ViewKind::ALL
            .iter()
            .map(|kind| about(*kind).label)
            .collect();
        assert_eq!(labels.len(), ViewKind::ALL.len());
    }
}
