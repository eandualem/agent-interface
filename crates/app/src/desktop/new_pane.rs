//! The new-pane chooser: what an empty pane offers before anything runs.
//!
//! Two buttons open a terminal or switch to the Backbone views. Under a filter,
//! the sessions list shows Backbone agents first, needing you first, then the
//! other tmux sessions; the views list shows what a pane can display instead of
//! a terminal. Nothing is created until a choice is made, and the owner makes
//! it from the [`NewPaneEvent`] this emits.

use std::{path::PathBuf, time::Duration};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    Context, Entity, EventEmitter, ScrollHandle, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use tmux_workspaces_core::{View, ViewKind};
use tmux_workspaces_roster::views::{Data, Member, Request, Team};

use super::{
    components::{self as ui, AgentState},
    design::{colors, tokens},
    name::{NameEvent, NameField},
};
use crate::{roster::ViewSource, sessions::SessionSource};

/// What the person chose.
pub(super) enum NewPaneEvent {
    OpenTerminal,
    Attach(String),
    OpenView(View),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Sessions,
    Views,
    /// Choosing the agent for an agent view.
    Agents,
}

/// One entry of the views list, in the design's order.
struct ViewChoice {
    glyph: &'static str,
    label: &'static str,
    description: &'static str,
    scope: &'static str,
    kind: ViewKind,
}

const VIEWS: [ViewChoice; 7] = [
    ViewChoice {
        glyph: "◆",
        label: "Skills",
        description: "Every skill by layer: store, repositories, user-level, and its tags",
        scope: "whole team",
        kind: ViewKind::Skills,
    },
    ViewChoice {
        glyph: "≡",
        label: "Policies",
        description: "Base, global, tag and swarm layers, each readable in full",
        scope: "whole team",
        kind: ViewKind::Policies,
    },
    ViewChoice {
        glyph: "#",
        label: "Tags",
        description: "What each tag gives the agents that carry it",
        scope: "whole team",
        kind: ViewKind::Tags,
    },
    ViewChoice {
        glyph: "⌥",
        label: "Team",
        description: "Agents by repository, tag or swarm",
        scope: "whole team",
        kind: ViewKind::Team,
    },
    ViewChoice {
        glyph: "Σ",
        label: "Usage",
        description: "Tokens by agent, CLI session and model",
        scope: "whole team",
        kind: ViewKind::Usage,
    },
    ViewChoice {
        glyph: "∿",
        label: "Activity",
        description: "State timeline, deliveries, held and expired messages",
        scope: "whole team",
        kind: ViewKind::Activity,
    },
    ViewChoice {
        glyph: "⚿",
        label: "Agent",
        description: "One agent: permissions, connections, its skills, policies, usage",
        scope: "pick an agent",
        kind: ViewKind::Agent,
    },
];

enum Row {
    Heading(&'static str, &'static str),
    Agent(Member),
    Session(String),
    View(usize),
    Pick(String),
}

fn reconcile_selection(rows: &[Row], selected: &mut Option<String>, disarmed: &mut bool) {
    let mut choices = rows.iter().filter(|row| NewPane::selectable(row));
    if let Some(key) = selected {
        if !choices.any(|row| row_key(row) == *key) {
            *selected = None;
            *disarmed = true;
        }
    } else if !*disarmed {
        *selected = choices.next().map(row_key);
    }
}

pub(super) struct NewPane {
    pane: String,
    field: Entity<NameField>,
    query: String,
    mode: Mode,
    /// The chosen row by identity, so refreshes that reorder or remove rows
    /// never move Enter onto another session.
    selected: Option<String>,
    /// The chosen row disappeared; nothing is chosen until the next move.
    disarmed: bool,
    /// Scroll the chosen row into view at the next frame.
    reveal: bool,
    sessions: Vec<String>,
    sessions_error: Option<String>,
    loading: bool,
    team: Option<Team>,
    team_loading: bool,
    visible: bool,
    backbone: bool,
    scroll: ScrollHandle,
    session_source: Option<SessionSource>,
    team_source: Option<ViewSource>,
    _updates: Task<()>,
    _input: Subscription,
}

impl EventEmitter<NewPaneEvent> for NewPane {}

impl NewPane {
    pub fn new(
        pane: String,
        socket: Option<PathBuf>,
        backbone: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = cx.new(|cx| NameField::filter(window, cx));
        let input = cx.subscribe_in(&field, window, |chooser, _, event, _, cx| {
            match event {
                NameEvent::Changed(query) => {
                    chooser.query = query.clone();
                    chooser.selected = None;
                    chooser.disarmed = false;
                    chooser.scroll.scroll_to_item(0);
                }
                NameEvent::Save => chooser.activate(cx),
                NameEvent::Cancel => {}
            }
            cx.notify();
        });
        let (wake, mut woken) = mpsc::unbounded::<()>();
        let session_source = socket.and_then(|socket| {
            let wake = wake.clone();
            SessionSource::start(socket, move || {
                let _ = wake.unbounded_send(());
            })
            .ok()
        });
        let team_source = backbone.as_ref().and_then(|directory| {
            let wake = wake.clone();
            ViewSource::start(
                directory.clone(),
                Request::Team,
                Duration::from_secs(3),
                move || {
                    let _ = wake.unbounded_send(());
                },
            )
            .ok()
        });
        let updates = cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                if this.update(cx, |chooser, cx| chooser.refresh(cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            pane,
            field,
            query: String::new(),
            mode: Mode::Sessions,
            selected: None,
            disarmed: false,
            reveal: false,
            sessions: Vec::new(),
            sessions_error: None,
            loading: session_source.is_some(),
            team: None,
            team_loading: team_source.is_some(),
            visible: true,
            backbone: backbone.is_some(),
            scroll: ScrollHandle::new(),
            session_source,
            team_source,
            _updates: updates,
            _input: input,
        }
    }

    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if let Some(source) = &self.session_source {
            source.set_active(visible);
        }
        if let Some(source) = &self.team_source {
            source.set_active(visible);
        }
        self.sessions.clear();
        self.sessions_error = None;
        self.team = None;
        self.loading = self.session_source.is_some();
        self.team_loading = self.team_source.is_some();
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if !self.visible {
            return;
        }
        if let Some(observation) = self
            .session_source
            .as_ref()
            .and_then(SessionSource::take_update)
        {
            self.sessions = observation.sessions;
            self.sessions_error = observation.error;
            self.loading = false;
            cx.notify();
        }
        if let Some((_, update)) = self.team_source.as_ref().and_then(ViewSource::take_update) {
            self.team_loading = false;
            // Without a current read, show plain sessions rather than old states.
            self.team = match update {
                Ok(Data::Team(team)) => Some(team),
                _ => None,
            };
            cx.notify();
        }
    }

    fn matches(&self, text: &str) -> bool {
        self.query.is_empty() || text.to_lowercase().contains(&self.query.to_lowercase())
    }

    /// Agents with a running session, needing you first, then the others.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        match self.mode {
            Mode::Sessions => {
                let mut agents: Vec<Member> = self
                    .team
                    .iter()
                    .flat_map(|team| team.members.iter())
                    .filter(|member| self.sessions.contains(&member.name))
                    .filter(|member| self.matches(&member.name) || self.matches(&member.repo))
                    .cloned()
                    .collect();
                agents.sort_by_key(|member| {
                    let rank = match AgentState::from_report(&member.state) {
                        AgentState::NeedsYou => 0,
                        AgentState::Working => 1,
                        _ => 2,
                    };
                    (rank, member.name.to_ascii_lowercase())
                });
                let others: Vec<String> = self
                    .sessions
                    .iter()
                    .filter(|name| {
                        !self
                            .team
                            .iter()
                            .any(|team| team.members.iter().any(|m| &&m.name == name))
                    })
                    .filter(|name| self.matches(name))
                    .cloned()
                    .collect();
                if !agents.is_empty() {
                    rows.push(Row::Heading("BACKBONE AGENTS", "needs you first"));
                    rows.extend(agents.into_iter().map(Row::Agent));
                }
                if !others.is_empty() {
                    rows.push(Row::Heading(
                        if self.team.is_some() {
                            "OTHER TMUX SESSIONS"
                        } else {
                            "TMUX SESSIONS"
                        },
                        "",
                    ));
                    rows.extend(others.into_iter().map(Row::Session));
                }
            }
            Mode::Views => {
                rows.extend(
                    VIEWS
                        .iter()
                        .enumerate()
                        .filter(|(_, view)| self.matches(view.label))
                        .map(|(index, _)| Row::View(index)),
                );
            }
            Mode::Agents => {
                let mut names: Vec<String> = self
                    .team
                    .iter()
                    .flat_map(|team| team.members.iter())
                    .map(|member| member.name.clone())
                    .filter(|name| self.matches(name))
                    .collect();
                names.sort_by_key(|name| name.to_ascii_lowercase());
                rows.extend(names.into_iter().map(Row::Pick));
            }
        }
        rows
    }

    fn selectable(row: &Row) -> bool {
        !matches!(row, Row::Heading(..))
    }

    fn choices(&self) -> Vec<Row> {
        self.rows().into_iter().filter(Self::selectable).collect()
    }

    fn selected_index(&self, choices: &[Row]) -> Option<usize> {
        let selected = self.selected.as_deref()?;
        choices.iter().position(|row| row_key(row) == selected)
    }

    /// Move the selection by `offset` choices and scroll it into view.
    pub fn move_by(&mut self, offset: isize, cx: &mut Context<Self>) {
        let choices = self.choices();
        if choices.is_empty() {
            return;
        }
        let next = match self.selected_index(&choices) {
            Some(at) => (at as isize + offset).clamp(0, choices.len() as isize - 1) as usize,
            None => 0,
        };
        self.selected = Some(row_key(&choices[next]));
        self.disarmed = false;
        self.reveal = true;
        cx.notify();
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        self.selected = None;
        self.disarmed = false;
        // A filter typed for one list means nothing in the next.
        self.query.clear();
        self.field.update(cx, |field, cx| field.clear(cx));
        cx.notify();
    }

    /// Switch between sessions and views; from the agent list, back to views.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        let mode = match self.mode {
            Mode::Sessions if self.backbone => Mode::Views,
            Mode::Sessions => return,
            Mode::Views | Mode::Agents => Mode::Sessions,
        };
        self.set_mode(mode, cx);
    }

    /// Choose the selected row.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        let mut choices = self.choices();
        if let Some(at) = self.selected_index(&choices) {
            let row = choices.swap_remove(at);
            self.choose(row, cx);
        }
    }

    /// Choose the numbered row, 1 to 9.
    pub fn choose_number(&mut self, number: usize, cx: &mut Context<Self>) {
        let mut choices = self.choices();
        if (1..=choices.len()).contains(&number) {
            let row = choices.swap_remove(number - 1);
            self.choose(row, cx);
        }
    }

    fn choose(&mut self, row: Row, cx: &mut Context<Self>) {
        match row {
            Row::Heading(..) => {}
            Row::Agent(member) => cx.emit(NewPaneEvent::Attach(member.name)),
            Row::Session(name) => cx.emit(NewPaneEvent::Attach(name)),
            Row::View(index) => match VIEWS[index].kind {
                ViewKind::Agent => self.set_mode(Mode::Agents, cx),
                kind => cx.emit(NewPaneEvent::OpenView(View::of(kind))),
            },
            Row::Pick(name) => cx.emit(NewPaneEvent::OpenView(View::agent(name, None))),
        }
    }

    pub fn pane(&self) -> &str {
        &self.pane
    }
}

impl Drop for NewPane {
    fn drop(&mut self) {
        // Both workers may be inside a bounded read; never wait on the UI thread.
        let sources = (self.session_source.take(), self.team_source.take());
        std::thread::spawn(move || drop(sources));
    }
}

impl Render for NewPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let rows = self.rows();
        let count = rows.iter().filter(|row| Self::selectable(row)).count();
        let refreshing = match self.mode {
            Mode::Sessions => self.loading || self.team_loading,
            Mode::Agents => self.team_loading,
            Mode::Views => false,
        };
        if !refreshing {
            reconcile_selection(&rows, &mut self.selected, &mut self.disarmed);
        }
        let views = self.mode != Mode::Sessions;
        let buttons = div()
            .flex()
            .flex_shrink_0()
            .gap(px(t.space.s10))
            .child(
                ui::choice_button(
                    "new-pane-terminal",
                    "terminal",
                    "Open terminal",
                    "⌘↵",
                    false,
                )
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                    cx.emit(NewPaneEvent::OpenTerminal);
                })),
            )
            .when(self.backbone, |buttons| {
                buttons.child(
                    ui::choice_button(
                        "new-pane-views",
                        "layout-grid",
                        "Skills, usage, team…",
                        "⇥",
                        views,
                    )
                    .on_click(cx.listener(|chooser, _, window, cx| {
                        cx.stop_propagation();
                        // Keys now belong to this list, not a terminal beside it.
                        let focus = chooser.field.read(cx).focus.clone();
                        window.focus(&focus, cx);
                        chooser.toggle(cx);
                    })),
                )
            });
        let mut list = div()
            .id(SharedString::from(format!("new-pane-rows-{}", self.pane)))
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s2))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if self.mode == Mode::Views {
            list = list.child(
                ui::view_heading("AGENT BACKBONE · LIVE, READ-ONLY", "").child(back(
                    &self.pane,
                    "← back to sessions",
                    Mode::Sessions,
                    cx,
                )),
            );
        } else if self.mode == Mode::Agents {
            list = list.child(ui::view_heading("WHICH AGENT?", "").child(back(
                &self.pane,
                "← views",
                Mode::Views,
                cx,
            )));
        }
        let mut number = 0;
        // Children before the rows: the list's heading in the views lists.
        for (child, row) in (usize::from(self.mode != Mode::Sessions)..).zip(rows) {
            let selectable = Self::selectable(&row);
            let selected = selectable && self.selected.as_deref() == Some(row_key(&row).as_str());
            if selectable {
                number += 1;
            }
            let key = if selectable && number <= 9 {
                number.to_string()
            } else {
                String::new()
            };
            let chosen = row_key(&row);
            if selected && std::mem::take(&mut self.reveal) {
                self.scroll.scroll_to_item(child);
            }
            let id =
                SharedString::from(format!("new-pane-{}-{number}-{}", self.pane, row_key(&row)));
            let element = match &row {
                Row::Heading(label, note) => ui::view_heading(*label, *note).into_any_element(),
                Row::Agent(member) => agent_row(id, member, &key, selected).into_any_element(),
                Row::Session(name) => ui::choice_frame(id, selected)
                    .child(ui::icon("terminal", t.size.icon, c.muted))
                    .child(mono(name.clone()).flex_1().text_color(c.text))
                    .child(ui::choice_key(key))
                    .into_any_element(),
                Row::View(view) => {
                    let view = &VIEWS[*view];
                    ui::view_choice(
                        id,
                        view.glyph,
                        view.label,
                        view.description,
                        view.scope,
                        key,
                        selected,
                    )
                    .into_any_element()
                }
                Row::Pick(name) => ui::choice_frame(id, selected)
                    .child(ui::icon("user", t.size.icon, c.muted))
                    .child(mono(name.clone()).flex_1().text_color(c.text))
                    .child(ui::choice_key(key))
                    .into_any_element(),
            };
            list = list.child(if selectable {
                div()
                    .id(SharedString::from(format!(
                        "new-pane-pick-{}-{chosen}",
                        self.pane
                    )))
                    .on_click(cx.listener(move |chooser, _, _, cx| {
                        cx.stop_propagation();
                        chooser.selected = Some(chosen.clone());
                        chooser.activate(cx);
                    }))
                    .child(element)
                    .into_any_element()
            } else {
                element
            });
        }
        let status = if self.mode == Mode::Sessions {
            if let Some(error) = &self.sessions_error {
                Some(error.clone())
            } else if self.loading {
                Some("Looking for sessions…".to_owned())
            } else if count == 0 {
                Some("No session matches the filter.".to_owned())
            } else {
                None
            }
        } else if self.mode == Mode::Agents && count == 0 {
            Some("No agent matches. Backbone may not have answered yet.".to_owned())
        } else {
            None
        };
        if let Some(status) = status {
            list = list.child(
                ui::text(
                    div()
                        .px(px(t.space.s10))
                        .py(px(t.space.s8))
                        .text_color(c.muted),
                    &t.text.body,
                )
                .child(status),
            );
        }
        let (enter, tab): (SharedString, SharedString) = match self.mode {
            Mode::Sessions => ("attach here".into(), "skills, usage, team…".into()),
            Mode::Views => ("open view here".into(), "back to sessions".into()),
            Mode::Agents => ("open its card here".into(), "back to sessions".into()),
        };
        let mut hints = vec![("↑↓", SharedString::from("move")), ("↵", enter)];
        if self.backbone {
            hints.push(("⇥", tab));
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s12))
                    .pt(px(t.space.s20))
                    .px(px(t.space.s28))
                    .child(buttons)
                    .child(
                        ui::filter_frame()
                            .child(div().flex_1().min_w_0().child(self.field.clone())),
                    )
                    .child(list),
            )
            .child(ui::hint_bar(&hints, "Nothing is created until you choose."))
            .capture_key_down(cx.listener(|chooser, event: &gpui::KeyDownEvent, _, cx| {
                if chooser.field.read(cx).composing() {
                    return;
                }
                if chooser.key(&event.keystroke, false, cx) {
                    cx.stop_propagation();
                }
            }))
    }
}

impl NewPane {
    /// Handle a chooser key; returns whether it was one. The window also sends
    /// these while navigation has focus and this pane is selected.
    /// Number keys choose only from navigation; in the filter they are text.
    pub fn key(&mut self, key: &gpui::Keystroke, navigating: bool, cx: &mut Context<Self>) -> bool {
        let m = &key.modifiers;
        if key.key == "enter" && m.platform && !m.control && !m.alt {
            cx.emit(NewPaneEvent::OpenTerminal);
            return true;
        }
        if m.modified() {
            return false;
        }
        match key.key.as_str() {
            "up" => self.move_by(-1, cx),
            "down" => self.move_by(1, cx),
            "tab" => self.toggle(cx),
            "enter" => self.activate(cx),
            digit if navigating && digit.len() == 1 => match digit.parse::<usize>() {
                Ok(number) if number > 0 => self.choose_number(number, cx),
                _ => return false,
            },
            _ => return false,
        }
        true
    }
}

fn back(
    pane: &str,
    label: &'static str,
    to: Mode,
    cx: &mut Context<NewPane>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("new-pane-back-{pane}")))
        .cursor_pointer()
        .text_color(colors().accent_text)
        .child(label)
        .on_click(cx.listener(move |chooser, _, _, cx| {
            cx.stop_propagation();
            chooser.set_mode(to, cx);
        }))
}

fn row_key(row: &Row) -> String {
    match row {
        Row::Heading(label, _) => (*label).to_owned(),
        Row::Agent(member) => member.name.clone(),
        Row::Session(name) | Row::Pick(name) => name.clone(),
        Row::View(index) => VIEWS[*index].label.to_owned(),
    }
}

fn agent_row(
    id: SharedString,
    member: &Member,
    key: &str,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let (t, c) = (tokens(), colors());
    let state = AgentState::from_report(&member.state);
    let (word, color) = match state {
        AgentState::NeedsYou => (
            member.reason.clone().unwrap_or_else(|| "waiting".into()),
            c.attention_text,
        ),
        AgentState::Working => (
            match member.current_issue {
                Some(issue) => format!("busy · #{issue}"),
                None => "busy".into(),
            },
            c.accent_text,
        ),
        _ => (member.state.replace('_', " "), c.muted),
    };
    let place = if member.repo.is_empty() {
        member.tags.join(" · ")
    } else {
        member.repo.clone()
    };
    ui::choice_frame(id.clone(), selected)
        .child(ui::agent_symbol(state))
        .child(mono(member.name.clone()).flex_shrink_0().text_color(c.text))
        .child(ui::badge(member.runtime.clone()))
        .child(
            ui::text(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(place),
        )
        .child(ui::text(div().flex_shrink_0().text_color(color), &t.text.meta).child(word))
        .child(ui::choice_key(key.to_owned()))
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
    fn row_reconciliation_skips_headings_and_never_retargets_a_removed_selection() {
        let rows = vec![Row::Heading("SESSIONS", ""), Row::Session("worker".into())];
        let (mut selected, mut disarmed) = (None, false);
        reconcile_selection(&rows, &mut selected, &mut disarmed);
        assert_eq!(selected.as_deref(), Some("worker"));
        let reordered = vec![Row::Session("other".into()), Row::Session("worker".into())];
        reconcile_selection(&reordered, &mut selected, &mut disarmed);
        assert_eq!(selected.as_deref(), Some("worker"));
        reconcile_selection(&reordered[..1], &mut selected, &mut disarmed);
        assert!(selected.is_none());
        assert!(disarmed);
        reconcile_selection(&rows, &mut selected, &mut disarmed);
        assert!(selected.is_none());
    }
}
