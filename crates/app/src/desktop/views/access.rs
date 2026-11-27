//! The Access pane: what each agent may reach, who is above whom, and the
//! requests for more. It reads Agent Backbone live and draws boards A (by
//! agent) and E (requests); every change goes to Backbone, and the pane shows
//! what Backbone returns.
use super::{NameEvent, NameField, colors, tokens, ui};
use agent_interface_roster::profiles::{
    Connection, Error,
    access::{Overview, RequestPage, Requests},
};
use gpui::{Context, Div, Entity, Render, Subscription, Task, Window, div, prelude::*, px};
use std::{cell::Cell, collections::HashSet, rc::Rc, time::Duration};

mod agents;
// Used by the requests view and the actions, which are not drawn yet.
#[cfg_attr(not(test), allow(dead_code))]
mod model;

/// How often the pane reads Backbone again, as the other live views do.
const EVERY: Duration = Duration::from_secs(3);

pub(super) struct Pane {
    connection: Connection,
    visible: Rc<Cell<bool>>,
    /// The records last read; none before the first read.
    model: Option<model::Model>,
    /// When they were read, as HH:MM.
    read_at: Option<String>,
    /// Why the last read failed, shown over the records still drawn.
    error: Option<String>,
    reading: bool,
    filter: model::Filter,
    query: Entity<NameField>,
    /// The chosen agent, by name.
    selected: Option<String>,
    /// Swarms shown with their members.
    open_swarms: HashSet<String>,
    /// Whether every main agent is listed, not only the first few.
    all_main: bool,
    /// This profile's name, for the header.
    profile_name: Option<String>,
    _poll: Task<()>,
    _query: Subscription,
}

impl Pane {
    pub(super) fn new(
        connection: Connection,
        visible: Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| {
            let mut field = NameField::bare_filter(window, cx);
            field.focus = field.focus.clone().tab_stop(true);
            field
        });
        let changed = cx.subscribe(&query, |_, _, event: &NameEvent, cx| {
            if matches!(event, NameEvent::Changed(_)) {
                cx.notify();
            }
        });
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if this.update(cx, |pane, cx| pane.read(cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(EVERY).await;
            }
        });
        Self {
            connection,
            visible,
            model: None,
            read_at: None,
            error: None,
            reading: false,
            filter: model::Filter::All,
            query,
            selected: None,
            open_swarms: HashSet::new(),
            all_main: false,
            profile_name: None,
            _poll: poll,
            _query: changed,
        }
    }

    /// Tell the pane its profile's name, which can arrive after it was made.
    /// Says whether it changed.
    pub(super) fn place(&mut self, name: Option<&str>) -> bool {
        let name = name.map(str::to_owned);
        let changed = self.profile_name != name;
        self.profile_name = name;
        changed
    }

    /// Read the agents and waiting requests, unless a read is under way or
    /// the pane is hidden.
    fn read(&mut self, cx: &mut Context<Self>) {
        if self.reading || !self.visible.get() {
            return;
        }
        self.reading = true;
        let connection = self.connection.clone();
        let read = cx.background_executor().spawn(async move {
            let overview = connection.access_overview()?;
            let pending = connection.access_requests(Requests::Pending)?;
            Ok::<(Overview, RequestPage), Error>((overview, pending))
        });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                pane.reading = false;
                match result {
                    Ok((overview, pending)) => {
                        let model = pane.model.get_or_insert_with(Default::default);
                        model.overview = overview;
                        model.pending = pending.items;
                        pane.read_at = Some(chrono::Local::now().format("%H:%M").to_string());
                        pane.error = None;
                        pane.keep_selection();
                    }
                    Err(error) => pane.error = Some(failure(error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Keep the chosen agent while it is listed; otherwise choose the first
    /// one the list shows.
    fn keep_selection(&mut self) {
        let Some(model) = &self.model else { return };
        if self
            .selected
            .as_ref()
            .is_some_and(|name| model.agent(name).is_some())
        {
            return;
        }
        let groups = model::groups(&model.overview, model::Filter::All);
        self.selected = groups
            .orchestrators
            .iter()
            .chain(&groups.coordinators)
            .chain(&groups.main)
            .chain(groups.swarms.iter().flat_map(|swarm| &swarm.members))
            .next()
            .map(|agent| agent.name.clone());
    }

    /// "live from Agent Backbone · read 10:31", once something was read.
    fn live_note(&self) -> Option<Div> {
        let (t, c) = (tokens(), colors());
        let at = self.read_at.as_ref()?;
        Some(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s6))
                .text_color(c.muted)
                .child(ui::dot(c.ok))
                .child(format!("live from Agent Backbone · read {at}")),
        )
    }
}

/// What the pane says when a read fails, over the records still drawn.
fn failure(error: Error) -> String {
    match error {
        Error::Unavailable => "Backbone is not answering. Showing the last data read.".into(),
        _ => format!(
            "{} Showing the last data read.",
            agent_interface_roster::views::UNAVAILABLE
        ),
    }
}

/// A band at the bottom of the pane, for a read that failed.
fn band(message: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex_shrink_0()
            .px(px(t.space.s12))
            .py(px(t.space.s6))
            .bg(c.attention_soft)
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.attention_text),
        &t.text.meta,
    )
    .child(message)
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = colors();
        let body = match &self.model {
            None => match &self.error {
                Some(error) => ui::empty_state(
                    ui::empty_tile("shield", c.selected, c.muted),
                    "Access unavailable",
                    error.clone(),
                ),
                None => div().flex_1(),
            },
            Some(_) => self.agents_view(cx),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(body)
            .children(
                self.error
                    .clone()
                    .filter(|_| self.model.is_some())
                    .map(band),
            )
    }
}
