//! Boards F and F2: the browsers the owner signed in to, a chosen one's
//! holder, who may use it and give it out, what it is signed in to, its
//! policies and recent use, and the sheet that makes a new one.
use super::{
    super::WatchBrowser, BrowserChange, Change, Menu, Pane, Shown, agents::this_machine, model,
};
use crate::desktop::{
    components::{self as ui, Button},
    design::{colors, tokens},
    views::connections::hinted,
};
use agent_interface_roster::profiles::{
    access::{Agent, Grant, Role},
    browsers::{Browser, Create, Detail, Policies, Save, Session, State},
};
use chrono::{DateTime, FixedOffset};
use gpui::{AnyElement, Context, Div, SharedString, Stateful, Window, div, prelude::*, px};

/// The browsers view's own state.
#[derive(Default)]
pub(super) struct Browsing {
    /// The chosen browser, by ID.
    pub chosen: Option<String>,
    /// Its detail as last read or returned; none before the first read.
    pub detail: Option<Detail>,
    /// Older use read so far, below the detail's recent use.
    pub older: Vec<Session>,
    /// The `before` that reads older use, when there is more.
    pub older_before: Option<i64>,
    /// The new browser being made.
    pub new: Option<NewBrowser>,
}

/// The new browser sheet.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct NewBrowser {
    /// Its ID once Backbone has made it; none while it is being named.
    pub id: Option<String>,
    /// Whether its name and address are being changed before saving.
    pub renaming: bool,
    /// The agents chosen to use it.
    pub chosen: Vec<String>,
    /// The agents offered as chips, in their order.
    pub offered: Vec<String>,
    /// Keep agents on its sites.
    pub keep_on_sites: bool,
}

impl Browsing {
    /// The browser whose detail the pane reads: the new one being made, else
    /// the chosen one; none while the browsers are not shown.
    pub fn shown_id(&self, shown: bool) -> Option<String> {
        if !shown {
            return None;
        }
        match &self.new {
            Some(new) => new.id.clone(),
            None => self.chosen.clone(),
        }
    }

    /// Keep a detail that was read, unless another browser is shown by now.
    pub fn read(&mut self, detail: Option<Detail>) {
        if let Some(detail) = detail
            && self.shown_id(true).as_deref() == Some(detail.browser.id.as_str())
        {
            self.detail = Some(detail);
        }
    }
}

impl Pane {
    /// Show the browsers, or the agents again.
    pub(super) fn toggle_browsers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = match self.shown {
            Shown::Browsers => Shown::Agents,
            Shown::Agents | Shown::Requests => Shown::Browsers,
        };
        self.menu = None;
        self.noting = None;
        self.adding = None;
        self.leave_field(window, cx);
        self.read(cx);
    }

    /// The switch beside the requests that shows the browsers.
    pub(super) fn browsers_switch(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = tokens();
        ui::chip(
            "access-browsers-switch",
            "Browsers",
            self.shown == Shown::Browsers,
        )
        .gap(px(t.space.s6))
        .on_click(cx.listener(|pane, _, window, cx| pane.toggle_browsers(window, cx)))
    }

    /// Keep the chosen browser while it is listed; otherwise choose the
    /// first.
    pub(super) fn keep_browser(&mut self) {
        let Some(model) = &self.model else { return };
        let items = &model.browsers.items;
        let listed = |id: &String| items.iter().any(|b| &b.id == id);
        if self.browsing.chosen.as_ref().is_some_and(listed) {
            return;
        }
        self.browsing.chosen = items.first().map(|b| b.id.clone());
        self.browsing.detail = None;
        self.browsing.older.clear();
        self.browsing.older_before = None;
    }

    /// Choose the browser `id` and read it.
    pub(super) fn choose_browser(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.browsing.chosen != id {
            self.browsing.chosen = id;
            self.browsing.detail = None;
            self.browsing.older.clear();
            self.browsing.older_before = None;
            self.menu = None;
            self.notice = None;
        }
        self.read(cx);
        cx.notify();
    }

    /// Open the new browser sheet, named for the browser `wanted` asks for
    /// with the agents that asked chosen.
    pub(super) fn start_new(
        &mut self,
        wanted: Option<(String, Vec<String>)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (name, asking) = match wanted {
            Some((id, agents)) => (name_for(&id), agents),
            None => (String::new(), Vec::new()),
        };
        let agents = self
            .model
            .as_ref()
            .map(|model| model.overview.agents.as_slice())
            .unwrap_or_default();
        self.browsing.new = Some(NewBrowser {
            id: None,
            renaming: false,
            offered: offered(&asking, agents),
            chosen: asking,
            keep_on_sites: true,
        });
        self.name_field
            .update(cx, |field, cx| field.set_value(&name, cx));
        self.opens_field.update(cx, |field, cx| field.clear(cx));
        self.shown = Shown::Browsers;
        self.menu = None;
        self.notice = None;
        let field = if name.is_empty() {
            self.name_field.read(cx).focus.clone()
        } else {
            self.opens_field.read(cx).focus.clone()
        };
        self.enter_field(field, window, cx);
    }

    /// Make the new browser on this machine, which opens its sign-in window.
    pub(super) fn create_browser(&mut self, cx: &mut Context<Self>) {
        let create = Create {
            name: self.name_field.read(cx).value().trim().to_owned(),
            opens_at: self.opens_field.read(cx).value().trim().to_owned(),
            machine: None,
        };
        self.send(
            "access-browser-create".into(),
            Change::Browser(BrowserChange::Create(create)),
            cx,
        );
    }

    /// Keep the new browser with its name, address, agents and policy.
    pub(super) fn save_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(new) = &self.browsing.new else {
            return;
        };
        let Some(id) = new.id.clone() else { return };
        let policies = self
            .browsing
            .detail
            .as_ref()
            .filter(|d| d.browser.id == id)
            .map(|d| d.policies.clone())
            .unwrap_or_default();
        let save = Save {
            name: Some(self.name_field.read(cx).value().trim().to_owned()),
            opens_at: Some(self.opens_field.read(cx).value().trim().to_owned()),
            grants: new.chosen.clone(),
            policies: Some(Policies {
                keep_on_sites: new.keep_on_sites,
                ..policies
            }),
        };
        self.leave_field(window, cx);
        self.send(
            "access-browser-save".into(),
            Change::Browser(BrowserChange::Save(id, save)),
            cx,
        );
    }

    /// Close the new browser sheet; once Backbone has made the browser, this
    /// signs it out and deletes it.
    pub(super) fn cancel_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let made = self.browsing.new.as_ref().and_then(|new| new.id.clone());
        self.leave_field(window, cx);
        match made {
            Some(id) => self.send(
                "access-browser-cancel".into(),
                Change::Browser(BrowserChange::Delete(id)),
                cx,
            ),
            None => {
                self.browsing.new = None;
                cx.notify();
            }
        }
    }

    /// Show a browser Backbone returned for the change `key`.
    pub(super) fn browsed(&mut self, key: &str, detail: Detail) {
        let id = detail.browser.id.clone();
        if let Some(model) = self.model.as_mut() {
            model.apply_browser(detail.browser.clone());
        }
        match (key, &mut self.browsing.new) {
            ("access-browser-create", Some(new)) => {
                new.id = Some(id.clone());
                new.renaming = false;
                // The agents that asked for it come first, chosen.
                let asking: Vec<String> = detail.requests.iter().map(|a| a.agent.clone()).collect();
                for agent in asking.iter().rev() {
                    new.offered.retain(|offered| offered != agent);
                    new.offered.insert(0, agent.clone());
                    if !new.chosen.contains(agent) {
                        new.chosen.push(agent.clone());
                    }
                }
                self.browsing.chosen = Some(id);
            }
            ("access-browser-save", Some(_)) => {
                self.browsing.new = None;
                self.browsing.chosen = Some(id);
            }
            _ => {}
        }
        self.browsing.read(Some(detail));
    }

    /// A browser was taken back or, when `deleted`, deleted.
    pub(super) fn released(&mut self, id: &str, deleted: bool) {
        if !deleted {
            return;
        }
        if let Some(model) = self.model.as_mut() {
            model.remove_browser(id);
        }
        if self.browsing.new.as_ref().and_then(|new| new.id.as_deref()) == Some(id) {
            self.browsing.new = None;
        }
        if self.browsing.chosen.as_deref() == Some(id) {
            self.browsing.chosen = None;
            self.browsing.detail = None;
        }
        self.keep_browser();
    }

    /// Read the browser `id`'s use older than the session `before`, below
    /// what is shown.
    pub(super) fn read_older(&mut self, id: &str, before: i64, cx: &mut Context<Self>) {
        let (connection, id) = (self.connection.clone(), id.to_owned());
        let read = cx
            .background_executor()
            .spawn(async move { connection.browser_sessions(&id, Some(before), None) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                match result {
                    Ok(page) => {
                        let shown: Vec<i64> = pane
                            .browsing
                            .detail
                            .iter()
                            .flat_map(|d| &d.recent)
                            .chain(&pane.browsing.older)
                            .map(|s| s.id)
                            .collect();
                        pane.browsing
                            .older
                            .extend(page.items.into_iter().filter(|s| !shown.contains(&s.id)));
                        pane.browsing.older_before = page.next_before.filter(|_| page.has_more);
                    }
                    Err(error) => pane.error = Some(super::failure(error)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn browsers_view(&self, cx: &mut Context<Self>) -> Div {
        let Some(model) = &self.model else {
            return div();
        };
        let body = match &self.browsing.new {
            Some(new) => self.new_browser(new, model, cx),
            None => div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(self.browser_list(model, cx))
                .child(self.browser_detail(model, cx)),
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.browsers_header(cx))
            .children(self.notice_band(cx))
            .child(body)
    }

    fn browsers_header(&self, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let profile = self.profile_name.as_deref().unwrap_or("this profile");
        let note = if self.browsing.new.is_some() {
            "new browser".to_owned()
        } else {
            format!("browsers you signed in to for {profile}, and which agents may use each")
        };
        ui::view_header("globe", "Browsers")
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(note),
            )
            .children(self.live_note().map(|note| ui::text(note, &t.text.meta)))
            .when(self.browsing.new.is_none(), |el| {
                el.child(self.browsers_switch(cx))
            })
    }

    fn browser_list(&self, model: &model::Model, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut list = div()
            .w(px(t.size.browser_list))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .p(px(t.space.s8))
            .border_r(px(t.size.hairline))
            .border_color(c.line);
        let mut rows = div()
            .id("access-browsers")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(t.size.hairline));
        for (at, (machine, browsers)) in machines(&model.browsers.items).into_iter().enumerate() {
            let name = browsers.first().and_then(|b| b.machine_name.clone());
            rows = rows.child(
                ui::text(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(t.space.s2))
                        .h(px(t.size.browser_heading))
                        .pl(px(t.space.s8))
                        .text_color(c.muted),
                    &t.text.label,
                )
                .child(div().flex_1().child("BROWSERS"))
                .child(
                    self.machine_chip(Some(&machine), name.as_ref())
                        .mr(px(t.space.s4)),
                )
                .when(at == 0, |el| {
                    el.child(ui::icon_button("access-browser-new-plus", "plus").on_click(
                        cx.listener(|pane, _, window, cx| pane.start_new(None, window, cx)),
                    ))
                }),
            );
            rows = rows.children(
                browsers
                    .into_iter()
                    .map(|browser| self.browser_row(browser, cx)),
            );
        }
        if model.browsers.items.is_empty() {
            rows = rows.child(
                ui::text(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .h(px(t.size.browser_heading))
                        .pl(px(t.space.s8))
                        .text_color(c.muted),
                    &t.text.label,
                )
                .child("BROWSERS"),
            );
        }
        rows = rows.child(
            ui::text(
                div()
                    .id("access-browser-new")
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.group_row))
                    .mt(px(t.space.s4))
                    .px(px(t.space.s8))
                    .rounded(px(t.radius.row))
                    .border(px(t.size.hairline))
                    .border_dashed()
                    .border_color(c.line_strong)
                    .text_color(c.muted)
                    .cursor_pointer()
                    .hover(|el| el.bg(colors().hover)),
                &t.text.row,
            )
            .child(
                div()
                    .w(px(t.size.browser_mark))
                    .flex()
                    .justify_center()
                    .child(ui::icon("plus", t.size.icon, c.muted)),
            )
            .child("New browser")
            .on_click(cx.listener(|pane, _, window, cx| pane.start_new(None, window, cx))),
        );
        list = list.child(rows).child(
            ui::text(
                div()
                    .flex_shrink_0()
                    .mx(px(-t.space.s8))
                    .pt(px(t.space.s10))
                    .px(px(t.space.s16))
                    .pb(px(t.space.s4))
                    .border_t(px(t.size.hairline))
                    .border_color(c.line)
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(
                "Each browser is its own Chrome, kept apart from yours and from the others. \
                 Agents get the signed-in session, never the password.",
            ),
        );
        list
    }

    fn browser_row(&self, browser: &Browser, cx: &mut Context<Self>) -> Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let selected = self.browsing.chosen.as_deref() == Some(browser.id.as_str());
        let id = browser.id.clone();
        let (label, tone) = state_label(browser);
        let color = match tone {
            Tone::Accent => c.accent_text,
            Tone::Attention => c.attention_text,
            Tone::Muted => c.muted,
        };
        ui::row(
            SharedString::from(format!("access-browser-{}", browser.id)),
            selected,
        )
        .h(px(t.size.browser_row))
        .px(px(t.space.s8))
        .gap(px(t.space.s10))
        .child(letter_mark(&browser.name, t.size.browser_mark, selected))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(t.space.s4))
                .child(
                    ui::text(div(), &t.text.row)
                        .text_color(if selected { c.text } else { c.text_2 })
                        .when(selected, |el| el.font_weight(t.text.row_selected.weight()))
                        .child(browser.name.clone()),
                )
                .child(
                    ui::text(
                        ui::mono(sites(browser)).overflow_hidden().text_ellipsis(),
                        &t.text.key,
                    )
                    .text_color(c.muted),
                ),
        )
        .child(
            ui::text(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s4 + 1.0))
                    .text_color(color),
                &t.text.meta,
            )
            .when(browser.state == State::InUse, |el| {
                el.child(ui::sending_spinner())
            })
            .child(label),
        )
        .on_click(cx.listener(move |pane, _, _, cx| pane.choose_browser(Some(id.clone()), cx)))
    }

    fn browser_detail(&self, model: &model::Model, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let Some(browser) = self
            .browsing
            .chosen
            .as_deref()
            .and_then(|id| model.browsers.items.iter().find(|b| b.id == id))
        else {
            return div().flex_1().child(ui::empty_state(
                ui::empty_tile("globe", c.selected, c.muted),
                "No browser chosen",
                "Make a browser, sign in to a site once, and choose which agents may use it.",
            ));
        };
        let detail = self
            .browsing
            .detail
            .as_ref()
            .filter(|d| d.browser.id == browser.id);
        // Backbone serves its DevTools endpoint while its Chrome runs, which
        // can be reached only on its own machine.
        let port = detail
            .and_then(|d| d.devtools.as_deref())
            .and_then(devtools_port)
            .filter(|_| self.on_this_machine(Some(&browser.machine)));
        let now = chrono::Local::now().fixed_offset();
        let signed_in = browser.state != State::SignedOut && browser.signed_in_since.is_some();
        let since = browser
            .signed_in_since
            .as_deref()
            .and_then(|at| model::time(at, &now))
            .map(|at| format!("since {}", at.format("%-d %b")));
        let id = browser.id.clone();
        let more = vec![
            ui::menu_item("access-browser-delete", None, "Delete", None)
                .text_color(c.attention_text)
                .on_click(cx.listener({
                    let id = id.clone();
                    move |pane, _, _, cx| {
                        pane.menu = None;
                        pane.send(
                            "access-browser-delete".into(),
                            Change::Browser(BrowserChange::Delete(id.clone())),
                            cx,
                        );
                    }
                })),
        ];
        let header = ui::text(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s10))
                .h(px(t.size.browser_header))
                .pl(px(t.space.s20))
                .pr(px(t.space.s14))
                .border_b(px(t.size.hairline))
                .border_color(c.line)
                .whitespace_nowrap(),
            &t.text.meta,
        )
        .child(letter_mark(&browser.name, t.size.access_mark, true))
        .child(ui::text(div().text_color(c.text), &t.text.empty_title).child(browser.name.clone()))
        .child(if signed_in {
            ui::tag("Signed in", ui::Tone::Done)
        } else {
            ui::tag("Signed out", ui::Tone::Attention)
        })
        .children(
            since
                .filter(|_| signed_in)
                .map(|since| div().text_color(c.muted).child(since)),
        )
        .child(div().flex_1())
        .children(port.map(|port| {
            let watch = WatchBrowser {
                id: browser.id.clone(),
                name: browser.name.clone(),
                port,
                session: detail
                    .and_then(|d| d.holder.as_ref())
                    .and_then(|holder| holder.agent_browser_session.clone()),
            };
            ui::button("access-browser-watch", "Watch", Button::Secondary)
                .h(px(t.size.chip + t.space.s4))
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(watch.clone())))
        }))
        .child(
            self.busy(
                "access-browser-window",
                ui::button(
                    "access-browser-window",
                    "Open it yourself",
                    Button::Secondary,
                )
                .h(px(t.size.chip + t.space.s4))
                .on_click(cx.listener({
                    let id = id.clone();
                    move |pane, _, _, cx| {
                        pane.send(
                            "access-browser-window".into(),
                            Change::Browser(BrowserChange::Show(id.clone())),
                            cx,
                        );
                    }
                })),
            ),
        )
        .child(self.with_menu(
            Menu::BrowserMore,
            ui::icon_button("access-browser-more", "ellipsis"),
            more,
            cx,
        ));
        let mut body = div()
            .id("access-browser-detail")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(t.space.s16))
            .px(px(t.space.s24))
            .pt(px(t.space.s16))
            .pb(px(t.space.s20));
        if let Some(detail) = detail {
            body = body
                .children(self.holder_band(detail, &now, cx))
                .child(self.who_may_use(detail, model, &now, cx))
                .child(self.who_may_give(detail, model, cx))
                .child(self.signed_in_to(detail, &now, cx))
                .child(self.recent_use(detail, &now, cx));
        }
        let given = model.browsers.items.iter().filter(|b| b.agents > 0).count();
        let total = model.browsers.items.len();
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .child(
                ui::text(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap(px(t.space.s16))
                        .h(px(t.size.view_footer))
                        .px(px(t.space.s20))
                        .border_t(px(t.size.hairline))
                        .border_color(c.line)
                        .text_color(c.muted)
                        .whitespace_nowrap(),
                    &t.text.meta,
                )
                .child(
                    div()
                        .flex_1()
                        .child("Taking a browser away closes the agent's tabs at once."),
                )
                .child(format!(
                    "{given} of {total} browser{} {} given to an agent",
                    if total == 1 { "" } else { "s" },
                    if given == 1 { "is" } else { "are" },
                )),
            )
    }

    /// "<agent> is using it now · since 10:12 · 3 tabs", and Take it back.
    fn holder_band(
        &self,
        detail: &Detail,
        now: &DateTime<FixedOffset>,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let (t, c) = (tokens(), colors());
        let holder = detail.holder.as_ref()?;
        let since = model::time(&holder.since, now)
            .map(|at| format!(" · since {}", at.format("%H:%M")))
            .unwrap_or_default();
        let tabs = format!(
            " · {} tab{}",
            holder.tabs,
            if holder.tabs == 1 { "" } else { "s" }
        );
        let who = match &holder.agent {
            Some(agent) => div()
                .flex()
                .gap(px(t.space.s4))
                .child(ui::text(ui::mono(agent.clone()), &t.text.status))
                .child("is using it now"),
            None => div().child("You are using it now"),
        };
        let id = detail.browser.id.clone();
        Some(
            ui::text(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s10))
                    .h(px(t.size.browser_band))
                    .px(px(t.space.s12))
                    .rounded(px(t.radius.card))
                    .bg(c.accent_soft)
                    .border(px(t.size.hairline))
                    .border_color(c.accent)
                    .whitespace_nowrap()
                    .text_color(c.text),
                &t.text.row,
            )
            .child(ui::sending_spinner())
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .child(who)
                    .child(div().text_color(c.text_2).child(format!("{since}{tabs}"))),
            )
            .child(
                ui::text(div().text_color(c.text_2), &t.text.meta)
                    .child("One agent drives a browser at a time."),
            )
            .when(holder.agent.is_some(), |el| {
                el.child(
                    self.busy(
                        "access-browser-take-back",
                        ui::text_action("access-browser-take-back", "Take it back")
                            .text_color(c.accent_text)
                            .on_click(cx.listener(move |pane, _, _, cx| {
                                pane.send(
                                    "access-browser-take-back".into(),
                                    Change::Browser(BrowserChange::TakeBack(id.clone())),
                                    cx,
                                );
                            })),
                    ),
                )
            }),
        )
    }

    fn who_may_use(
        &self,
        detail: &Detail,
        model: &model::Model,
        now: &DateTime<FixedOffset>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let browser = &detail.browser;
        let mut card = ui::group_card();
        let mut first = true;
        for given in &detail.grants {
            let key = format!("access-browser-grant-{}", given.agent);
            let by = given.given_by.as_deref().unwrap_or("you");
            let at = model::time(&given.given_at, now)
                .map(|at| format!(" · {}", at.format("%-d %b")))
                .unwrap_or_default();
            let grant = browser_grant(model.agent(&given.agent), browser);
            let (agent, revoked) = (given.agent.clone(), grant.clone());
            card = card.child(
                line(first)
                    .h(px(t.size.browser_line))
                    .child(ui::agent_mark(&given.agent, t.space.s20))
                    .child(ui::text(
                        ui::mono(given.agent.clone()).text_color(c.text),
                        &t.text.status,
                    ))
                    .child(meta(format!("given by {by}{at}")))
                    .child(div().flex_1())
                    .child(meta(format!(
                        "used {} time{}",
                        given.uses,
                        if given.uses == 1 { "" } else { "s" }
                    )))
                    .child(self.busy(
                        &key,
                        ui::icon_button(SharedString::from(format!("{key}-remove")), "x").on_click(
                            cx.listener({
                                let key = key.clone();
                                move |pane, _, _, cx| {
                                    pane.send(
                                        key.clone(),
                                        Change::Revoke(agent.clone(), revoked.clone()),
                                        cx,
                                    );
                                }
                            }),
                        ),
                    )),
            );
            first = false;
        }
        for ask in &detail.requests {
            let key = format!("access-decide-{}", ask.id);
            let asked = model::time(&ask.requested_at, now)
                .map(|at| format!("asks · {} ago", model::age(&at, now)))
                .unwrap_or_else(|| "asks".into());
            let id = ask.id;
            card = card.child(
                line(first)
                    .h(px(t.size.browser_line))
                    .child(ui::agent_mark(&ask.agent, t.space.s20))
                    .child(ui::text(
                        ui::mono(ask.agent.clone()).text_color(c.text),
                        &t.text.status,
                    ))
                    .child(ui::text(div().text_color(c.accent_text), &t.text.meta).child(asked))
                    .child(
                        meta(ask.reason.clone())
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis(),
                    )
                    .child(
                        self.busy(
                            &key,
                            div()
                                .flex()
                                .gap(px(t.space.s4))
                                .child(
                                    ui::text_action(
                                        SharedString::from(format!("{key}-refuse")),
                                        "Refuse",
                                    )
                                    .on_click(cx.listener(
                                        move |pane, _, _, cx| {
                                            pane.decide_with(vec![id], false, String::new(), cx);
                                        },
                                    )),
                                )
                                .child(
                                    ui::text_action(
                                        SharedString::from(format!("{key}-approve")),
                                        "Approve",
                                    )
                                    .text_color(c.accent_text)
                                    .on_click(cx.listener(
                                        move |pane, _, _, cx| {
                                            pane.decide_with(vec![id], true, String::new(), cx);
                                        },
                                    )),
                                ),
                        ),
                    ),
            );
            first = false;
        }
        let given: Vec<&str> = detail.grants.iter().map(|g| g.agent.as_str()).collect();
        let choices = model
            .overview
            .agents
            .iter()
            .filter(|agent| !given.contains(&agent.name.as_str()))
            .map(|agent| {
                let grant = browser_grant(None, browser);
                let name = agent.name.clone();
                ui::menu_item(
                    SharedString::from(format!("access-browser-give-{name}")),
                    None,
                    name.clone(),
                    None,
                )
                .font_family(t.font.mono.clone())
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.menu = None;
                    pane.send(
                        format!("access-browser-grant-{name}"),
                        Change::Grant(name.clone(), grant.clone()),
                        cx,
                    );
                }))
            })
            .collect();
        card = card.child(self.with_menu(
            Menu::BrowserUser,
            add_line(first, "access-browser-give", "Let another agent use it"),
            choices,
            cx,
        ));
        let n = detail.grants.len();
        section(
            "WHO MAY USE IT",
            Some(format!("{n} agent{}", if n == 1 { "" } else { "s" })),
            card,
        )
    }

    fn who_may_give(&self, detail: &Detail, model: &model::Model, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let browser = &detail.browser;
        let mut card = ui::group_card();
        let mut first = true;
        for giver in &detail.givers {
            let key = format!("access-browser-giver-{}", giver.agent);
            let ceiling = model
                .agent(&giver.agent)
                .map(|agent| without(&agent.ceiling, &browser.id))
                .unwrap_or_default();
            let agent = giver.agent.clone();
            card = card.child(
                line(first)
                    .h(px(t.size.browser_line))
                    .child(ui::agent_mark(&giver.agent, t.space.s20))
                    .child(ui::text(
                        ui::mono(giver.agent.clone()).text_color(c.text),
                        &t.text.status,
                    ))
                    .child(ui::pill(model::role_label(giver.role)))
                    .child(meta(format!(
                        "to the {} agent{} under it, without asking you",
                        giver.under,
                        if giver.under == 1 { "" } else { "s" }
                    )))
                    .child(div().flex_1())
                    .child(self.busy(
                        &key,
                        ui::icon_button(SharedString::from(format!("{key}-remove")), "x").on_click(
                            cx.listener({
                                let key = key.clone();
                                move |pane, _, _, cx| {
                                    pane.send(
                                        key.clone(),
                                        Change::Browser(BrowserChange::Ceiling(
                                            agent.clone(),
                                            ceiling.clone(),
                                        )),
                                        cx,
                                    );
                                }
                            }),
                        ),
                    )),
            );
            first = false;
        }
        let givers: Vec<&str> = detail.givers.iter().map(|g| g.agent.as_str()).collect();
        let choices = model
            .overview
            .agents
            .iter()
            .filter(|agent| matches!(agent.role, Some(Role::Orchestrator | Role::Coordinator)))
            .filter(|agent| !givers.contains(&agent.name.as_str()))
            .map(|agent| {
                let mut ceiling = agent.ceiling.clone();
                ceiling.push(browser_grant(None, browser));
                let name = agent.name.clone();
                ui::menu_item(
                    SharedString::from(format!("access-browser-may-give-{name}")),
                    None,
                    name.clone(),
                    agent.role.map(|role| model::role_label(role).to_owned()),
                )
                .font_family(t.font.mono.clone())
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.menu = None;
                    pane.send(
                        format!("access-browser-giver-{name}"),
                        Change::Browser(BrowserChange::Ceiling(name.clone(), ceiling.clone())),
                        cx,
                    );
                }))
            })
            .collect();
        card = card.child(self.with_menu(
            Menu::BrowserGiver,
            add_line(
                first,
                "access-browser-add-giver",
                "Let an orchestrator or coordinator give it out",
            ),
            choices,
            cx,
        ));
        section("WHO MAY GIVE IT OUT", Some("besides you".into()), card)
    }

    fn signed_in_to(
        &self,
        detail: &Detail,
        now: &DateTime<FixedOffset>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let browser = &detail.browser;
        let id = browser.id.clone();
        let checked = browser
            .checked_at
            .as_deref()
            .and_then(|at| model::time(at, now))
            .map_or_else(
                || "check the session now".to_owned(),
                |at| format!("session checked {}", at.format("%H:%M")),
            );
        let mut card = ui::group_card();
        let mut first = true;
        for signed in &detail.signed_in_to {
            card = card.child(
                line(first)
                    .pr(px(t.space.s12))
                    .child(ui::icon("check", t.size.icon, c.ok))
                    .child(ui::text(
                        ui::mono(signed.domain.clone()).text_color(c.text),
                        &t.text.status,
                    ))
                    .child(div().flex_1())
                    .when(first, |el| {
                        let id = id.clone();
                        el.child(self.busy(
                            "access-browser-check",
                            ui::text_action("access-browser-check", checked.clone()).on_click(
                                cx.listener(move |pane, _, _, cx| {
                                    pane.send(
                                        "access-browser-check".into(),
                                        Change::Browser(BrowserChange::Check(id.clone())),
                                        cx,
                                    );
                                }),
                            ),
                        ))
                    }),
            );
            first = false;
        }
        if first {
            card = card.child(
                line(true)
                    .pr(px(t.space.s12))
                    .child(ui::icon("x", t.size.icon, c.attention_text))
                    .child(
                        div()
                            .text_color(c.text_2)
                            .child("Not signed in to any site"),
                    )
                    .child(div().flex_1())
                    .child(self.busy(
                        "access-browser-check",
                        ui::text_action("access-browser-check", checked).on_click(cx.listener(
                            move |pane, _, _, cx| {
                                pane.send(
                                    "access-browser-check".into(),
                                    Change::Browser(BrowserChange::Check(id.clone())),
                                    cx,
                                );
                            },
                        )),
                    )),
            );
            first = false;
        }
        for note in detail.signed_in_to.iter().filter_map(|s| s.note.clone()) {
            card = card.child(note_line(note));
        }
        let policies = &detail.policies;
        let keep = Policies {
            keep_on_sites: !policies.keep_on_sites,
            ..policies.clone()
        };
        let stop = Policies {
            stop_before_last_button: !policies.stop_before_last_button,
            ..policies.clone()
        };
        let sites = if policies.sites.is_empty() {
            "They cannot open any other address in this browser.".to_owned()
        } else {
            format!(
                "They cannot open any other address in this browser: {}.",
                policies.sites.join(" · ")
            )
        };
        card = card
            .child(self.policy(
                first,
                "access-browser-keep",
                "Keep agents on these sites",
                sites,
                policies.keep_on_sites,
                &browser.id,
                keep,
                cx,
            ))
            .child(self.policy(
                false,
                "access-browser-stop",
                "Stop before the last button",
                "Posting, sending and paying wait for you. The agent drafts; you press.".into(),
                policies.stop_before_last_button,
                &browser.id,
                stop,
                cx,
            ));
        section(
            "SIGNED IN TO",
            Some("what an agent holding this browser can act on as you".into()),
            card,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn policy(
        &self,
        first: bool,
        key: &'static str,
        label: &'static str,
        note: String,
        on: bool,
        id: &str,
        changed: Policies,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let id = id.to_owned();
        line(first)
            .h(px(t.size.browser_band))
            .pr(px(t.space.s12))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s2 + 1.0))
                    .child(div().text_color(c.text).child(label))
                    .child(meta(note).overflow_hidden().text_ellipsis()),
            )
            .child(self.busy(
                key,
                ui::switch(key, on).on_click(cx.listener(move |pane, _, _, cx| {
                    pane.send(
                        key.into(),
                        Change::Browser(BrowserChange::Policies(id.clone(), changed.clone())),
                        cx,
                    );
                })),
            ))
    }

    fn recent_use(
        &self,
        detail: &Detail,
        now: &DateTime<FixedOffset>,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let mut card = ui::group_card();
        let sessions: Vec<&Session> = detail.recent.iter().chain(&self.browsing.older).collect();
        for (at, session) in sessions.iter().enumerate() {
            card = card.child(
                line(at == 0)
                    .pr(px(t.space.s12))
                    .child(
                        meta(when(&session.started_at, now))
                            .w(px(t.size.browser_when))
                            .flex_shrink_0(),
                    )
                    .child(match &session.agent {
                        Some(agent) => {
                            ui::text(ui::mono(agent.clone()).text_color(c.text), &t.text.status)
                        }
                        None => div().text_color(c.text).child("You"),
                    })
                    .child(
                        meta(used(session, now))
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis(),
                    ),
            );
        }
        if sessions.is_empty() {
            card = card.child(line(true).child(meta("Nobody has used it yet.".into())));
        }
        let more = if self.browsing.older.is_empty() {
            detail
                .has_more_recent
                .then(|| detail.recent.last().map(|s| s.id))
                .flatten()
        } else {
            self.browsing.older_before
        };
        let id = detail.browser.id.clone();
        section("RECENT USE", None, card).children(more.map(|before| {
            ui::link(div().id("access-browser-older").child("Older use"))
                .on_click(cx.listener(move |pane, _, _, cx| pane.read_older(&id, before, cx)))
        }))
    }

    /// The new browser sheet: name it, sign in, choose who may use it.
    fn new_browser(&self, new: &NewBrowser, model: &model::Model, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let detail = new
            .id
            .as_deref()
            .and_then(|id| self.browsing.detail.as_ref().filter(|d| d.browser.id == id));
        let name = self.name_field.read(cx).value().trim().to_owned();
        let named = new.id.is_some() && !new.renaming;
        let signed: Vec<&str> = detail
            .map(|d| d.signed_in_to.iter().map(|s| s.domain.as_str()).collect())
            .unwrap_or_default();
        let creating = self.sending.as_deref() == Some("access-browser-create");
        let step_name =
            if named {
                let browser = detail.map(|d| &d.browser);
                let (shown, opens) = browser.map_or_else(
                    || (name.clone(), self.opens_field.read(cx).value().to_owned()),
                    |b| (b.name.clone(), b.opens_at.clone()),
                );
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .whitespace_nowrap()
                    .child(letter_mark(&shown, t.space.s20, true))
                    .child(div().text_color(c.text).child(shown))
                    .child(meta("opens at".into()))
                    .child(ui::text(ui::mono(opens).text_color(c.text_2), &t.text.meta))
                    .child(ui::machine_chip(this_machine()))
                    .child(div().flex_1())
                    .child(ui::text_action("access-browser-rename", "Change").on_click(
                        cx.listener(|pane, _, window, cx| {
                            if let Some(new) = &mut pane.browsing.new {
                                new.renaming = true;
                            }
                            let field = pane.name_field.read(cx).focus.clone();
                            pane.enter_field(field, window, cx);
                        }),
                    ))
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        ui::field_frame(false)
                            .w(px(t.size.access_label + t.space.s24))
                            .h(px(t.size.chip + t.space.s4))
                            .child(hinted(&self.name_field, "Name", cx)),
                    )
                    .child(meta("opens at".into()))
                    .child(
                        ui::field_frame(false)
                            .flex_1()
                            .h(px(t.size.chip + t.space.s4))
                            .child(hinted(&self.opens_field, "site.com", cx)),
                    )
                    .child(ui::machine_chip(this_machine()))
                    .when(new.id.is_none(), |el| {
                        el.child(if creating {
                            ui::busy_button("access-browser-create", "Opening")
                        } else {
                            ui::button("access-browser-create", "Open it", Button::Secondary)
                                .h(px(t.size.chip + t.space.s4))
                                .on_click(cx.listener(|pane, _, _, cx| pane.create_browser(cx)))
                        })
                    })
            };
        let mut steps = div().flex().flex_col().child(step(
            Step::Done.or(new.id.is_none() || new.renaming),
            true,
            div()
                .flex()
                .flex_col()
                .gap(px(t.space.s8))
                .child(
                    div()
                        .text_color(c.text)
                        .font_weight(t.text.row_selected.weight())
                        .child("Name it"),
                )
                .child(step_name),
        ));
        if let Some(detail) = detail {
            let id = detail.browser.id.clone();
            let mut signing = div().flex().flex_col().gap(px(t.space.s8)).child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .child(
                        div()
                            .flex_1()
                            .text_color(c.text)
                            .font_weight(t.text.row_selected.weight())
                            .child("Sign in, in the window that opened"),
                    )
                    .child(
                        self.busy(
                            "access-browser-window",
                            ui::button("access-browser-show", "Show the window", Button::Secondary)
                                .h(px(t.size.chip))
                                .on_click(cx.listener(move |pane, _, _, cx| {
                                    pane.send(
                                        "access-browser-window".into(),
                                        Change::Browser(BrowserChange::Show(id.clone())),
                                        cx,
                                    );
                                })),
                        ),
                    ),
            );
            if !signed.is_empty() {
                signing = signing.child(
                    ui::text(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(t.space.s8))
                            .text_color(c.text_2),
                        &t.text.status,
                    )
                    .child(ui::icon("check", t.size.icon - 1.0, c.ok))
                    .child("Signed in to")
                    .child(ui::text(
                        ui::mono(signed.join(" · ")).text_color(c.text),
                        &t.text.meta,
                    )),
                );
            }
            signing = signing
                .children(
                    detail
                        .signed_in_to
                        .iter()
                        .filter_map(|s| s.note.clone())
                        .map(warning_note),
                )
                .child(meta(
                    "Nothing you typed there passed through this app or any agent.".into(),
                ));
            steps = steps.child(step(
                if signed.is_empty() {
                    Step::Now
                } else {
                    Step::Done
                },
                true,
                signing,
            ));
            steps = steps.child(step(
                if signed.is_empty() {
                    Step::Later
                } else {
                    Step::Now
                },
                false,
                self.choose_users(new, detail, model, cx),
            ));
        }
        let saving = self.sending.as_deref() == Some("access-browser-save");
        let buttons = div()
            .flex()
            .items_center()
            .gap(px(t.space.s8))
            .pt(px(t.space.s2))
            .whitespace_nowrap()
            .when(new.id.is_some(), |el| {
                el.child(if saving {
                    ui::busy_button("access-browser-save", "Saving")
                } else {
                    ui::button("access-browser-save", "Save browser", Button::Primary)
                        .on_click(cx.listener(|pane, _, window, cx| pane.save_new(window, cx)))
                })
            })
            .child(
                ui::text_action("access-browser-cancel", "Cancel")
                    .on_click(cx.listener(|pane, _, window, cx| pane.cancel_new(window, cx))),
            )
            .child(div().flex_1())
            .when(new.id.is_some(), |el| {
                el.child(meta("Cancelling signs out and deletes this Chrome.".into()))
            });
        div().flex_1().min_h_0().flex().items_center().justify_center().p(px(t.space.s16)).child(
            div()
                .id("access-browser-new-sheet")
                .w(px(t.size.browser_sheet))
                .max_h_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap(px(t.space.s16))
                .pt(px(t.space.s20 + t.space.s2))
                .px(px(t.space.s24))
                .pb(px(t.space.s20))
                .rounded(px(t.radius.card + t.space.s2))
                .bg(c.panel)
                .border(px(t.size.hairline))
                .border_color(c.line)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s6))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(t.space.s8))
                                .child(
                                    ui::text(div().flex_1().text_color(c.text), &t.text.empty_title)
                                        .child("New browser"),
                                )
                                .child(ui::keycap("esc")),
                        )
                        .child(meta(
                            "A separate Chrome that stays signed in. You sign in once, in its own \
                             window. The agents you choose then use that session."
                                .into(),
                        )),
                )
                .child(steps)
                .child(buttons),
        )
    }

    /// The sheet's last step: the agents offered, a waiting request's line,
    /// and keeping agents on the browser's sites.
    fn choose_users(
        &self,
        new: &NewBrowser,
        detail: &Detail,
        model: &model::Model,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let now = chrono::Local::now().fixed_offset();
        let mut chips = div().flex().flex_wrap().items_center().gap(px(t.space.s6));
        for agent in &new.offered {
            let on = new.chosen.contains(agent);
            let name = agent.clone();
            chips = chips.child(
                agent_chip(
                    SharedString::from(format!("access-browser-choose-{agent}")),
                    agent,
                    on,
                )
                .on_click(cx.listener(move |pane, _, _, cx| {
                    if let Some(new) = &mut pane.browsing.new {
                        match new.chosen.iter().position(|chosen| chosen == &name) {
                            Some(at) => {
                                new.chosen.remove(at);
                            }
                            None => new.chosen.push(name.clone()),
                        }
                    }
                    cx.notify();
                })),
            );
        }
        let others = model
            .overview
            .agents
            .iter()
            .filter(|agent| !new.offered.contains(&agent.name))
            .map(|agent| {
                let name = agent.name.clone();
                ui::menu_item(
                    SharedString::from(format!("access-browser-another-{name}")),
                    None,
                    name.clone(),
                    None,
                )
                .font_family(t.font.mono.clone())
                .on_click(cx.listener(move |pane, _, _, cx| {
                    pane.menu = None;
                    if let Some(new) = &mut pane.browsing.new {
                        new.offered.push(name.clone());
                        new.chosen.push(name.clone());
                    }
                    cx.notify();
                }))
            })
            .collect();
        chips = chips.child(
            self.with_menu(
                Menu::BrowserAnother,
                ui::text(
                    div()
                        .id("access-browser-another")
                        .flex()
                        .items_center()
                        .gap(px(t.space.s4 + 1.0))
                        .h(px(t.size.chip + t.space.s4))
                        .pl(px(t.space.s8))
                        .pr(px(t.space.s10))
                        .rounded(px(t.radius.pill))
                        .border(px(t.size.hairline))
                        .border_dashed()
                        .border_color(c.muted)
                        .text_color(c.muted)
                        .cursor_pointer(),
                    &t.text.meta,
                )
                .child(ui::icon("plus", t.size.icon - 2.0, c.muted))
                .child("another agent"),
                others,
                cx,
            ),
        );
        let asked = detail
            .requests
            .iter()
            .filter(|ask| new.chosen.contains(&ask.agent))
            .map(|ask| {
                let ago = model::time(&ask.requested_at, &now)
                    .map(|at| format!(" {} ago", model::age(&at, &now)))
                    .unwrap_or_default();
                ui::text(div().text_color(c.accent_text), &t.text.meta).child(format!(
                    "{} asked for this{ago}. Saving answers its request.",
                    ask.agent
                ))
            });
        let sites = if detail.policies.sites.is_empty() {
            detail.browser.signed_in_to.join(" · ")
        } else {
            detail.policies.sites.join(" · ")
        };
        let name = detail.browser.name.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(t.space.s10))
            .child(
                div()
                    .text_color(c.text)
                    .font_weight(t.text.row_selected.weight())
                    .child("Choose who may use it"),
            )
            .child(chips)
            .children(asked)
            .child(
                ui::text(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s10))
                        .py(px(t.space.s8))
                        .px(px(t.space.s10))
                        .rounded(px(t.radius.row))
                        .bg(c.field)
                        .border(px(t.size.hairline))
                        .border_color(c.line_strong)
                        .whitespace_nowrap(),
                    &t.text.row,
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s2 + 1.0))
                        .child(
                            div()
                                .text_color(c.text)
                                .child(format!("Keep agents on {name}")),
                        )
                        .when(!sites.is_empty(), |el| {
                            el.child(ui::text(ui::mono(sites).text_color(c.muted), &t.text.key))
                        }),
                )
                .child(
                    ui::switch("access-browser-new-keep", new.keep_on_sites).on_click(cx.listener(
                        |pane, _, _, cx| {
                            if let Some(new) = &mut pane.browsing.new {
                                new.keep_on_sites = !new.keep_on_sites;
                            }
                            cx.notify();
                        },
                    )),
                ),
            )
    }
}

/// How a state label is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Accent,
    Attention,
    Muted,
}

/// A browser's state as its row says it: "in use", "1 agent", "1 asks",
/// "nobody", "signed out".
pub(super) fn state_label(browser: &Browser) -> (String, Tone) {
    let plural = |n: u32, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    match browser.state {
        State::InUse => ("in use".into(), Tone::Accent),
        State::Asked => (format!("{} asks", browser.asks), Tone::Accent),
        State::Given => (plural(browser.agents, "agent"), Tone::Muted),
        State::Nobody => ("nobody".into(), Tone::Muted),
        State::SignedOut => ("signed out".into(), Tone::Attention),
        State::SigningIn => ("signing in".into(), Tone::Muted),
        State::Unknown => (String::new(), Tone::Muted),
    }
}

/// The sites a browser's row names: what it is signed in to, or where it
/// opens before it is.
pub(super) fn sites(browser: &Browser) -> String {
    if browser.signed_in_to.is_empty() {
        host(&browser.opens_at).to_owned()
    } else {
        browser.signed_in_to.join(" · ")
    }
}

/// The port of a DevTools endpoint on this machine, such as
/// `ws://127.0.0.1:53121/devtools/browser/…`; none for one elsewhere.
pub(super) fn devtools_port(endpoint: &str) -> Option<u16> {
    let rest = endpoint.strip_prefix("ws://")?;
    let host = rest.split('/').next()?;
    let (name, port) = host.rsplit_once(':')?;
    matches!(name, "127.0.0.1" | "localhost")
        .then(|| port.parse().ok())
        .flatten()
}

/// An address without its scheme and trailing slash.
fn host(address: &str) -> &str {
    let address = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"))
        .unwrap_or(address);
    address.strip_suffix('/').unwrap_or(address)
}

/// The browsers by machine, in the order each machine is first met.
pub(super) fn machines(browsers: &[Browser]) -> Vec<(String, Vec<&Browser>)> {
    let mut machines: Vec<(String, Vec<&Browser>)> = Vec::new();
    for browser in browsers {
        match machines
            .iter_mut()
            .find(|(machine, _)| machine == &browser.machine)
        {
            Some((_, list)) => list.push(browser),
            None => machines.push((browser.machine.clone(), vec![browser])),
        }
    }
    machines
}

/// The grant that names `browser`: as the agent's record has it, or made
/// from the browser.
pub(super) fn browser_grant(agent: Option<&Agent>, browser: &Browser) -> Grant {
    agent
        .and_then(|agent| {
            agent
                .grants
                .iter()
                .find(|grant| names(grant, &browser.id))
                .cloned()
        })
        .unwrap_or_else(|| Grant::Browser {
            machine: Some(browser.machine.clone()),
            machine_name: None,
            browser: Some(browser.id.clone()),
        })
}

/// Whether `grant` names the browser `id`.
pub(super) fn names(grant: &Grant, id: &str) -> bool {
    matches!(grant, Grant::Browser { browser: Some(named), .. } if named == id)
}

/// A ceiling without the browser `id`.
pub(super) fn without(ceiling: &[Grant], id: &str) -> Vec<Grant> {
    ceiling
        .iter()
        .filter(|grant| !names(grant, id))
        .cloned()
        .collect()
}

/// A name for a browser asked for by `id`: the ID with its first letter in
/// upper case, which Backbone makes the same ID from.
pub(super) fn name_for(id: &str) -> String {
    let mut chars = id.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The agents a new browser's sheet offers: those that asked for it first,
/// then those already given another browser.
pub(super) fn offered(asking: &[String], agents: &[Agent]) -> Vec<String> {
    let mut offered: Vec<String> = asking.to_vec();
    for agent in agents {
        let has_browser = agent.grants.iter().any(|grant| {
            matches!(
                grant,
                Grant::Browser {
                    browser: Some(_),
                    ..
                }
            )
        });
        if has_browser && !offered.contains(&agent.name) {
            offered.push(agent.name.clone());
        }
    }
    offered
}

/// When a browser was last used, from its newest session: "in use now"
/// while it is open, else "last used 2 hours ago".
pub(super) fn last_used(session: &Session, now: &DateTime<FixedOffset>) -> String {
    match session.ended_at.as_deref() {
        None => "in use now".into(),
        Some(ended) => match model::time(ended, now) {
            Some(ended) => format!("last used {} ago", model::age(&ended, now)),
            None => "last used".into(),
        },
    }
}

/// When a session began: "today 10:12", "yesterday", or its date.
pub(super) fn when(at: &str, now: &DateTime<FixedOffset>) -> String {
    let Some(at) = model::time(at, now) else {
        return String::new();
    };
    let days = (now.date_naive() - at.date_naive()).num_days();
    match days {
        0 => format!("today {}", at.format("%H:%M")),
        1 => "yesterday".into(),
        _ => at.format("%-d %b").to_string(),
    }
}

/// What a session's row says of it: "still open · 3 tabs", "6 minutes ·
/// drafted 1 post, waited for you", "opened it yourself for 2 minutes".
pub(super) fn used(session: &Session, now: &DateTime<FixedOffset>) -> String {
    let tabs = |n: u32| format!("{n} tab{}", if n == 1 { "" } else { "s" });
    let (Some(started), Some(ended)) = (
        model::time(&session.started_at, now),
        session
            .ended_at
            .as_deref()
            .and_then(|at| model::time(at, now)),
    ) else {
        return format!("still open · {}", tabs(session.tabs));
    };
    let minutes = (ended - started).num_minutes().max(1);
    let length = match minutes {
        1 => "1 minute".to_owned(),
        2..60 => format!("{minutes} minutes"),
        60..120 => "1 hour".to_owned(),
        _ => format!("{} hours", minutes / 60),
    };
    if session.agent.is_none() {
        return format!("opened it yourself for {length}");
    }
    let mut did: Vec<String> = Vec::new();
    if !session.summary.trim().is_empty() {
        did.push(session.summary.trim().to_owned());
    }
    match session.waited {
        0 => {}
        1 => did.push("waited for you".into()),
        n => did.push(format!("waited for you {n} times")),
    }
    if did.is_empty() {
        length
    } else {
        format!("{length} · {}", did.join(", "))
    }
}

/// A step of the new browser sheet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Done,
    Now,
    Later,
}

impl Step {
    /// This step, or the one under way when `now` holds.
    fn or(self, now: bool) -> Self {
        if now { Self::Now } else { self }
    }
}

/// A step's mark and the line down to the next, beside its content.
fn step(state: Step, more: bool, content: Div) -> Div {
    let (t, c) = (tokens(), colors());
    let mark: AnyElement = match state {
        Step::Done => ui::icon("circle-check", t.size.icon, c.ok).into_any_element(),
        Step::Now => ui::sending_spinner().into_any_element(),
        Step::Later => ui::icon("circle", t.size.icon, c.muted).into_any_element(),
    };
    div()
        .flex()
        .gap(px(t.space.s12))
        .child(
            div()
                .w(px(t.size.icon))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .child(mark)
                .when(more, |el| {
                    el.child(
                        div()
                            .w(px(t.size.hairline))
                            .flex_1()
                            .mt(px(t.space.s4))
                            .bg(c.line_strong),
                    )
                }),
        )
        .child(
            content
                .flex_1()
                .min_w_0()
                .when(more, |el| el.pb(px(t.space.s16))),
        )
}

/// A browser's mark: the first letter of its name on a tile, filled when
/// chosen.
pub(super) fn letter_mark(name: &str, size: f32, filled: bool) -> Div {
    let (t, c) = (tokens(), colors());
    let letter = name
        .trim()
        .chars()
        .next()
        .map_or_else(|| "?".to_owned(), |ch| ch.to_uppercase().collect());
    ui::text(
        div()
            .flex_shrink_0()
            .size(px(size))
            .rounded(px(size * 0.28))
            .flex()
            .items_center()
            .justify_center()
            .when(filled, |el| el.bg(c.line_strong).text_color(c.text))
            .when(!filled, |el| {
                el.bg(c.selected)
                    .border(px(t.size.hairline))
                    .border_color(c.line_strong)
                    .text_color(c.text_2)
            }),
        if size < t.size.browser_mark {
            &t.text.meta
        } else {
            &t.text.row
        },
    )
    .font_weight(t.text.label.weight())
    .child(letter)
}

/// A chip that chooses an agent, ticked while it is chosen.
fn agent_chip(id: SharedString, agent: &str, on: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .flex()
            .items_center()
            .gap(px(t.space.s6))
            .h(px(t.size.chip + t.space.s4))
            .px(px(t.space.s10))
            .rounded(px(t.radius.pill))
            .border(px(t.size.hairline))
            .font_family(t.font.mono.clone())
            .cursor_pointer(),
        &t.text.meta,
    )
    .when(on, |el| {
        el.pl(px(t.space.s8))
            .bg(c.accent_soft)
            .border_color(c.accent)
            .text_color(c.text)
            .child(ui::icon("check", t.size.icon - 2.0, c.accent_text))
    })
    .when(!on, |el| {
        el.border_color(c.line_strong).text_color(c.text_2)
    })
    .child(agent.to_owned())
}

/// A card's last line, which opens a menu of agents.
fn add_line(first: bool, id: &'static str, label: &'static str) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.group_row))
            .px(px(t.space.s12))
            .when(!first, |el| {
                el.border_t(px(t.size.hairline)).border_color(c.line)
            })
            .text_color(c.muted)
            .cursor_pointer()
            .hover(|el| el.bg(colors().hover)),
        &t.text.row,
    )
    .child(
        div()
            .w(px(t.space.s20))
            .flex()
            .justify_center()
            .child(ui::icon("plus", t.size.icon, c.muted)),
    )
    .child(label)
}

/// A note Backbone gives for a sign-in, on a card.
fn note_line(note: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .items_start()
            .gap(px(t.space.s8))
            .px(px(t.space.s12))
            .py(px(t.space.s8))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .text_color(c.text_2),
        &t.text.meta,
    )
    .child(ui::icon("info", t.size.icon - 1.0, c.muted).mt(px(t.space.s2)))
    .child(div().flex_1().min_w_0().child(note))
}

/// A note Backbone gives for a sign-in, in the new browser sheet.
fn warning_note(note: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex()
            .items_start()
            .gap(px(t.space.s8))
            .py(px(t.space.s8))
            .px(px(t.space.s10))
            .rounded(px(t.radius.row))
            .bg(c.accent_soft)
            .text_color(c.text),
        &t.text.meta,
    )
    .child(ui::icon("triangle-alert", t.size.icon - 1.0, c.accent_text).mt(px(t.space.s2)))
    .child(div().flex_1().min_w_0().child(note))
}

fn meta(text: String) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(div().text_color(c.muted), &t.text.meta).child(text)
}

/// A section of the detail: its uppercase label and note, then its card.
fn section(label: &'static str, note: Option<String>, card: Div) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_shrink_0()
        .flex_col()
        .gap(px(t.space.s6))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(t.space.s8))
                .px(px(t.space.s2))
                .child(ui::text(div().text_color(c.muted), &t.text.label).child(label))
                .children(
                    note.map(|note| ui::text(div().text_color(c.muted), &t.text.meta).child(note)),
                ),
        )
        .child(card)
}

/// One line on a section's card, separated from the line above by a
/// hairline.
fn line(first: bool) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.group_row))
            .pl(px(t.space.s12))
            .pr(px(t.space.s8))
            .whitespace_nowrap()
            .text_color(c.text_2)
            .when(!first, |el| {
                el.border_t(px(t.size.hairline)).border_color(c.line)
            }),
        &t.text.row,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browsers() -> Vec<Browser> {
        let text =
            include_str!("../../../../../roster/src/profiles/browsers/fixtures/browsers.json");
        let list: serde_json::Value = serde_json::from_str(text).unwrap();
        serde_json::from_value(list["items"].clone()).unwrap()
    }

    fn detail() -> Detail {
        let text =
            include_str!("../../../../../roster/src/profiles/browsers/fixtures/browser.json");
        serde_json::from_str(text).unwrap()
    }

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-11-27T10:31:00+00:00").unwrap()
    }

    #[test]
    fn each_row_says_where_its_browser_stands_and_what_it_is_signed_in_to() {
        let listed = browsers();
        let labels: Vec<_> = listed.iter().map(state_label).collect();
        assert_eq!(
            labels,
            [
                ("in use".to_owned(), Tone::Accent),
                ("1 asks".to_owned(), Tone::Accent),
                ("signed out".to_owned(), Tone::Attention),
            ]
        );
        let given = Browser {
            state: State::Given,
            agents: 2,
            ..listed[0].clone()
        };
        assert_eq!(state_label(&given).0, "2 agents");
        assert_eq!(sites(&listed[1]), "youtube.com · google.com");
        // Not signed in to anything yet: where it opens.
        assert_eq!(sites(&listed[2]), "facebook.com");
        assert_eq!(machines(&listed).len(), 1);
    }

    #[test]
    fn recent_use_says_how_long_what_was_done_and_who_waited() {
        let detail = detail();
        let now = now();
        let said: Vec<_> = detail.recent.iter().map(|s| used(s, &now)).collect();
        assert_eq!(
            said,
            [
                "still open · 3 tabs",
                "6 minutes · drafted 1 post, waited for you",
                "opened it yourself for 2 minutes",
            ]
        );
        assert_eq!(when(&detail.recent[0].started_at, &now), "today 10:12");
        assert_eq!(when(&detail.recent[2].started_at, &now), "yesterday");
        assert_eq!(last_used(&detail.recent[0], &now), "in use now");
        assert_eq!(last_used(&detail.recent[1], &now), "last used 2 hours ago");
    }

    #[test]
    fn a_browser_is_given_and_given_out_by_the_grant_naming_it() {
        let listed = browsers();
        let youtube = &listed[1];
        let grant = browser_grant(None, youtube);
        assert_eq!(
            grant,
            Grant::Browser {
                machine: Some("5f2c9e0d1b7a4e38".into()),
                machine_name: None,
                browser: Some("youtube".into()),
            }
        );
        let other = Grant::Browser {
            browser: Some("instagram".into()),
            ..grant.clone()
        };
        let docker = Grant::Docker {
            machine: None,
            machine_name: None,
        };
        assert_eq!(
            without(&[grant.clone(), other.clone(), docker.clone()], "youtube"),
            [other, docker]
        );
        // An agent's own record is sent back as it has it.
        let agent = Agent {
            name: "poster".into(),
            grants: vec![Grant::Browser {
                machine_name: Some("laptop".into()),
                ..grant
            }],
            ..Agent::default()
        };
        assert!(matches!(
            browser_grant(Some(&agent), youtube),
            Grant::Browser {
                machine_name: Some(_),
                ..
            }
        ));
        assert_eq!(
            offered(&["uploader".into()], &[agent]),
            ["uploader", "poster"]
        );
        assert_eq!(name_for("telegram"), "Telegram");
        assert_eq!(
            devtools_port(detail().devtools.as_deref().unwrap()),
            Some(53121)
        );
        assert_eq!(devtools_port("ws://10.0.0.2:9222/devtools/browser/x"), None);
    }
}
