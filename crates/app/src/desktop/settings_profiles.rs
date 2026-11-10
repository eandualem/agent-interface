//! Settings › Profiles: every profile, and one profile's accounts, agents and
//! service. Sessions it opens show in the main window.
use super::{
    Browser, components as ui,
    design::{colors, tokens},
    profile_management::{Back, Management, Open},
};
use crate::profiles::Client;
use agent_interface_roster::profiles::{Item, Kind, State};
use gpui::{App, Context, Entity, SharedString, Subscription, Window, div, prelude::*, px};

enum List {
    Loading,
    /// Agent Backbone is not installed, or has no profiles command.
    Absent,
    Failed(&'static str),
    Ready(Vec<Item>),
}

pub(super) struct Profiles {
    client: Option<Client>,
    list: List,
    detail: Option<(Entity<Management>, Vec<Subscription>)>,
}

impl Profiles {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut page = Self {
            client: None,
            list: List::Loading,
            detail: None,
        };
        page.refresh(cx);
        page
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.list = List::Loading;
        let client = self.client.clone();
        let work = cx.background_executor().spawn(async move {
            let client = client.or_else(super::onboarding::probe::profile_client)?;
            let items = client
                .list(&|| true)
                .map_err(|error| super::profile_picker::message(&error));
            Some((client, items))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |page, cx| {
                page.list = match result {
                    None => List::Absent,
                    Some((client, items)) => {
                        page.client = Some(client);
                        match items {
                            Ok(items) => List::Ready(items),
                            Err(message) => List::Failed(message),
                        }
                    }
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn open(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let directory = library_directory(cx);
        let pane = cx.new(|cx| Management::new(client, directory, Some(id), cx));
        window.focus(&pane.read(cx).focus.clone(), cx);
        let back = cx.subscribe(&pane, |page, _, _: &Back, cx| {
            page.detail = None;
            page.refresh(cx);
        });
        let open = cx.subscribe(&pane, |page, _, open: &Open, cx| {
            let (open, client) = (open.clone(), page.client.clone());
            cx.defer(move |cx| show_in_browser(open, client, cx));
        });
        self.detail = Some((pane, vec![back, open]));
        cx.notify();
    }

    /// A sign-in pane closed in a main window.
    pub fn sign_in_ended(&mut self, profile_id: &str, runtime: &str, cx: &mut Context<Self>) {
        if let Some((pane, _)) = &self.detail {
            pane.update(cx, |pane, cx| pane.sign_in_ended(profile_id, runtime, cx));
        }
    }
}

/// The open library's folder, from the first main window.
fn library_directory(cx: &App) -> Option<std::path::PathBuf> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<Browser>())
        .find_map(|window| window.read(cx).ok().and_then(|b| b.directory.clone()))
}

fn show_in_browser(open: Open, client: Option<Client>, cx: &mut App) {
    if let Some(handle) = super::browser_window(cx) {
        let _ = handle.update(cx, |browser, window, cx| {
            if let Some(client) = client {
                browser.use_profile_client(client);
            }
            browser.open_from_setup(open, None, window, cx);
        });
    }
}

fn new_profile(client: Option<Client>, cx: &mut App) {
    if let Some(handle) = super::browser_window(cx) {
        let _ = handle.update(cx, |browser, window, cx| {
            if let Some(client) = client {
                browser.use_profile_client(client);
            }
            window.activate_window();
            browser.open_profile_flow(String::new(), window, cx);
        });
    }
}

impl Render for Profiles {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((pane, _)) = &self.detail {
            return div().size_full().child(pane.clone()).into_any_element();
        }
        let (t, c) = (tokens(), colors());
        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .child(ui::group_title(
                "Profiles",
                "Each one keeps its own accounts, agents, skills and policies.",
            ));
        match &self.list {
            List::Loading => page = page.child(ui::group_note("Loading profiles…")),
            List::Absent => {
                page = page.child(ui::group_note(
                    "Agent Backbone is not installed, so there are no profiles.",
                ))
            }
            List::Failed(message) => {
                page = page.child(ui::group_note(*message)).child(
                    div().flex().child(
                        ui::button("profiles-retry", "Check again", ui::Button::Secondary)
                            .on_click(cx.listener(|page, _, _, cx| page.refresh(cx))),
                    ),
                )
            }
            List::Ready(items) => {
                let mut card = ui::group_card();
                for (index, item) in items.iter().enumerate() {
                    let id = item.profile_id.clone();
                    let state: SharedString = match (&item.state, &item.kind) {
                        (State::Ready, Kind::LegacyDefault) => "Your own sign-ins".into(),
                        (State::Ready, _) => "".into(),
                        (State::Creating, _) => "Creating…".into(),
                        (State::Failed, _) => "Didn't finish".into(),
                    };
                    card =
                        card.child(
                            ui::group_row(
                                SharedString::from(format!("settings-profile-{id}")),
                                index == 0,
                                Some(
                                    ui::profile_mark(
                                        &id,
                                        &item.display_name,
                                        item.kind == Kind::LegacyDefault,
                                        t.size.profile_mark,
                                    )
                                    .into_any_element(),
                                ),
                                item.display_name.clone(),
                                div().text_color(c.muted).child(state),
                            )
                            .on_click(cx.listener(
                                move |page, _, window, cx| page.open(id.clone(), window, cx),
                            )),
                        );
                }
                if !items.is_empty() {
                    page = page.child(card);
                }
                page = page.child(
                    div().flex().pt(px(t.space.s8)).child(
                        ui::button("settings-new-profile", "New profile", ui::Button::Secondary)
                            .on_click(cx.listener(|page, _, _, cx| {
                                let client = page.client.clone();
                                cx.defer(move |cx| new_profile(client, cx));
                            })),
                    ),
                );
            }
        }
        page.into_any_element()
    }
}
