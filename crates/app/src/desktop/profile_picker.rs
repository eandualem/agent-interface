//! The New workspace popover: a name, the profile it joins, or a new profile.
//! Its owner binds the verified result to a workspace explicitly; constructing
//! this popover does not change app sources.

use super::{
    components as ui,
    design::{colors, tokens},
    name::NameField,
};
use crate::profiles::{
    Client, Error,
    selection::{Selected, Selection},
};
use agent_interface_roster::profiles::{Item, Kind, State};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, ScrollHandle, SharedString, Subscription, Window,
    div, prelude::*, px,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) enum Choice {
    /// A verified profile, and the name the person typed (empty for its own).
    Selected {
        selected: Box<Selected>,
        name: String,
    },
    /// Make a profile first; the workspace takes `name`.
    NewProfile {
        name: String,
    },
    /// Profiles are unavailable: a workspace that runs terminals only.
    Plain {
        name: String,
    },
    Cancel,
}

#[derive(Default)]
struct Catalog {
    items: Vec<Item>,
    loading: bool,
    focused: Option<String>,
    error: Option<Error>,
    ticket: Arc<AtomicBool>,
}
impl Catalog {
    fn begin(&mut self) -> Arc<AtomicBool> {
        self.ticket.store(false, Ordering::Release);
        self.ticket = Arc::new(AtomicBool::new(true));
        self.focused = None;
        self.items.clear();
        self.error = None;
        self.loading = true;
        self.ticket.clone()
    }
    fn complete(&mut self, ticket: &Arc<AtomicBool>, result: Result<Vec<Item>, Error>) -> bool {
        if !Arc::ptr_eq(&self.ticket, ticket) || !ticket.load(Ordering::Acquire) {
            return false;
        }
        self.loading = false;
        match result {
            Ok(items) => self.items = items,
            Err(error) => {
                self.items.clear();
                self.error = Some(error);
            }
        }
        self.focused = self
            .items
            .iter()
            .find(|item| item.state == State::Ready)
            .map(|item| item.profile_id.clone());
        true
    }
    fn navigate(&mut self, by: isize) {
        let ready: Vec<_> = self
            .items
            .iter()
            .filter(|item| item.state == State::Ready)
            .collect();
        if ready.is_empty() {
            self.focused = None;
            return;
        }
        let at = self
            .focused
            .as_ref()
            .and_then(|id| ready.iter().position(|item| &item.profile_id == id))
            .unwrap_or(0);
        let next = at.saturating_add_signed(by).min(ready.len() - 1);
        self.focused = Some(ready[next].profile_id.clone());
    }
    /// Whether moving down leaves the profiles for the New profile row.
    fn at_end(&self) -> bool {
        let ready: Vec<_> = self
            .items
            .iter()
            .filter(|item| item.state == State::Ready)
            .collect();
        self.focused
            .as_ref()
            .and_then(|id| ready.iter().position(|item| &item.profile_id == id))
            .is_none_or(|at| at + 1 == ready.len())
    }
    fn focus_last(&mut self) {
        self.focused = self
            .items
            .iter()
            .rfind(|item| item.state == State::Ready)
            .map(|item| item.profile_id.clone());
    }
    fn focused_index(&self) -> Option<usize> {
        let id = self.focused.as_ref()?;
        self.items.iter().position(|item| &item.profile_id == id)
    }
    fn can_choose(&self, id: &str) -> bool {
        !self.loading
            && self.error.is_none()
            && self
                .items
                .iter()
                .any(|item| item.profile_id == id && item.state == State::Ready)
    }
}
impl Drop for Catalog {
    fn drop(&mut self) {
        self.ticket.store(false, Ordering::Release);
    }
}

pub(super) struct ProfilePicker {
    client: Client,
    catalog: Catalog,
    selection: Selection,
    selection_error: Option<(String, Error)>,
    name: Entity<NameField>,
    _name: Subscription,
    /// The New profile row has the keyboard choice instead of a profile.
    on_new: bool,
    closed: bool,
    pub focus: FocusHandle,
    scroll: ScrollHandle,
    has_bound_workspaces: bool,
}
impl EventEmitter<Choice> for ProfilePicker {}
impl ProfilePicker {
    pub fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.name.read(cx).focus.clone()
    }
    /// The caller supplies the already resolved executable and captured caller
    /// context. No ambient provider or Default profile is selected implicitly.
    pub fn new(
        client: Client,
        has_bound_workspaces: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name = cx.new(|cx| NameField::search(window, cx));
        let observe = cx.observe(&name, |_, _, cx| cx.notify());
        let mut picker = Self {
            client,
            catalog: Catalog::default(),
            selection: Selection::default(),
            selection_error: None,
            name,
            _name: observe,
            on_new: false,
            closed: false,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            has_bound_workspaces,
        };
        picker.refresh(cx);
        picker
    }
    fn typed(&self, cx: &gpui::App) -> String {
        self.name.read(cx).value().trim().to_owned()
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        self.selection.cancel();
        self.selection_error = None;
        self.on_new = false;
        self.scroll.scroll_to_item(0);
        let ticket = self.catalog.begin();
        let worker_ticket = ticket.clone();
        let client = self.client.clone();
        let read = cx
            .background_executor()
            .spawn(async move { client.list(&|| worker_ticket.load(Ordering::Acquire)) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |picker, cx| {
                let result = first_run(result, picker.has_bound_workspaces);
                if !picker.closed && picker.catalog.complete(&ticket, result) {
                    picker.on_new = picker.catalog.focused.is_none();
                    picker.reveal_focus();
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn choose(&mut self, id: &str, cx: &mut Context<Self>) {
        // Recheck the current catalog: a click may belong to an older frame.
        if self.closed || !self.catalog.can_choose(id) {
            return;
        }
        self.on_new = false;
        self.catalog.focused = Some(id.to_owned());
        self.selection_error = None;
        let attempt = match self.selection.begin(&self.client, id) {
            Ok(attempt) => attempt,
            Err(error) => {
                self.selection_error = Some((id.to_owned(), error));
                cx.notify();
                return;
            }
        };
        let work = cx.background_executor().spawn(async move { attempt.run() });
        let chosen = id.to_owned();
        cx.spawn(async move |this, cx| {
            let completion = work.await;
            let _ = this.update(cx, |picker, cx| {
                if picker.closed {
                    return;
                }
                if let Some(result) = picker.selection.accept(completion) {
                    match result {
                        Ok(selected) => {
                            picker.closed = true;
                            picker.catalog.ticket.store(false, Ordering::Release);
                            let name = picker.typed(cx);
                            cx.emit(Choice::Selected {
                                selected: Box::new(selected),
                                name,
                            });
                        }
                        Err(error) => picker.selection_error = Some((chosen, error)),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn finish(&mut self, choice: Choice, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.selection.cancel();
        self.catalog.ticket.store(false, Ordering::Release);
        cx.emit(choice);
        cx.notify();
    }
    fn new_profile(&mut self, cx: &mut Context<Self>) {
        let name = self.typed(cx);
        self.finish(Choice::NewProfile { name }, cx);
    }
    /// Enter: the chosen row, or a terminals-only workspace without profiles.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.catalog.error.is_some() {
            let name = self.typed(cx);
            if !name.is_empty() {
                self.finish(Choice::Plain { name }, cx);
            }
        } else if self.on_new {
            self.new_profile(cx);
        } else if let Some(id) = self.catalog.focused.clone() {
            self.choose(&id, cx);
        }
    }
    fn step(&mut self, by: isize) {
        if self.catalog.loading || self.catalog.error.is_some() {
            return;
        }
        if self.on_new {
            if by < 0 {
                self.catalog.focus_last();
                self.on_new = self.catalog.focused.is_none();
            }
        } else if by > 0 && self.catalog.at_end() {
            self.on_new = true;
        } else {
            self.catalog.navigate(by);
        }
    }
    fn reveal_focus(&self) {
        if let Some(index) = self.catalog.focused_index() {
            self.scroll.scroll_to_item(index);
        }
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.finish(Choice::Cancel, cx);
    }
    fn row_meta(&self, item: &Item) -> (SharedString, bool) {
        let id = item.profile_id.as_str();
        if self.selection.pending() == Some(id) {
            return ("Connecting…".into(), false);
        }
        if self
            .selection_error
            .as_ref()
            .is_some_and(|(failed, _)| failed == id)
        {
            return ("Didn't start".into(), true);
        }
        match (&item.state, &item.kind) {
            (State::Ready, Kind::LegacyDefault) => ("Your own sign-ins".into(), false),
            (State::Ready, _) => ("".into(), false),
            (State::Creating, _) => ("Creating…".into(), false),
            (State::Failed, _) => ("Didn't finish".into(), true),
        }
    }
}

/// An absent registry is a fresh machine, unless a saved workspace needs a profile:
/// then the registry was lost, and showing an empty list would hide that.
fn first_run(
    result: Result<Vec<Item>, Error>,
    has_bound_workspaces: bool,
) -> Result<Vec<Item>, Error> {
    match result {
        Err(Error::RegistryAbsent) if !has_bound_workspaces => Ok(Vec::new()),
        other => other,
    }
}

pub(super) fn message(error: &Error) -> &'static str {
    use agent_interface_roster::profiles::Error as ProfileError;
    match error {
        Error::CommandFailed => "Profile discovery was refused or failed.",
        Error::TimedOut => "Profile discovery timed out. Try again.",
        Error::Cancelled => "Profile selection was cancelled.",
        Error::Profile(ProfileError::NotReady) => "This profile is not ready to connect.",
        Error::Profile(ProfileError::BindingMismatch | ProfileError::BindingRequired) => {
            "The profile identity changed or its binding was refused. Refresh and try again."
        }
        Error::Profile(ProfileError::Unsupported) => {
            "This provider does not support profile connections."
        }
        Error::Profile(
            ProfileError::Unavailable
            | ProfileError::Refused(_)
            | ProfileError::RegistrationChanged,
        ) => "The profile could not be reached or authenticated. Its service may be stopped.",
        Error::Unavailable => "The profile discovery command is unavailable.",
        Error::RegistryAbsent => "The profile registry is missing.",
        Error::Invalid | Error::TooLarge | Error::Profile(ProfileError::Invalid) => {
            "The provider returned invalid profile information."
        }
    }
}

impl Render for ProfilePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let empty = self.name.read(cx).value().is_empty();
        let field = div()
            .relative()
            .px(px(t.space.s6))
            .pb(px(t.space.s8))
            .child(self.name.clone())
            .when(empty, |el| {
                el.child(
                    ui::text(
                        div()
                            .absolute()
                            .top_0()
                            .left(px(t.space.s6 * 2.0 + t.size.hairline))
                            .h(px(t.size.jump_field))
                            .flex()
                            .items_center()
                            .text_color(c.faint),
                        &t.text.body,
                    )
                    .child("Workspace name"),
                )
            });
        let mut menu = ui::menu()
            .w(px(t.size.profile_popover))
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|picker, event: &gpui::KeyDownEvent, _, cx| {
                let keys = &event.keystroke;
                if keys.modifiers.platform
                    || keys.modifiers.control
                    || keys.modifiers.alt
                    || picker.name.read(cx).composing()
                {
                    return;
                }
                match keys.key.as_str() {
                    "escape" => picker.cancel(cx),
                    "up" => picker.step(-1),
                    "down" => picker.step(1),
                    "enter" => picker.confirm(cx),
                    _ => return,
                }
                picker.reveal_focus();
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                ui::text(
                    div()
                        .px(px(t.space.s10))
                        .pt(px(t.space.s8))
                        .pb(px(t.space.s6))
                        .font_weight(t.text.row_selected.weight()),
                    &t.text.row,
                )
                .child("New workspace"),
            )
            .child(field);
        if let Some(error) = self.catalog.error {
            return menu.child(
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(t.space.s8))
                    .px(px(t.space.s10))
                    .pb(px(t.space.s10))
                    .child(
                        ui::text(div().text_color(c.text_2), &t.text.status).child(
                            "Profiles aren't available right now. This workspace can still run terminals.",
                        ),
                    )
                    .child(ui::text(div().text_color(c.muted), &t.text.meta).child(message(&error)))
                    .child(
                        ui::button("profiles-refresh", "Check again", ui::Button::Secondary)
                            .on_click(cx.listener(|picker, _, _, cx| picker.refresh(cx))),
                    ),
            );
        }
        let mut rows = div()
            .id("profile-rows")
            .flex()
            .flex_col()
            .gap(px(t.size.hairline * 2.0))
            .max_h(px(t.size.row * 8.0))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        if self.catalog.loading {
            rows = rows.child(
                ui::text(
                    div()
                        .px(px(t.space.s10))
                        .h(px(t.size.row))
                        .flex()
                        .items_center()
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .child("Loading profiles…"),
            );
        }
        for item in &self.catalog.items {
            let id = item.profile_id.clone();
            let ready = item.state == State::Ready;
            let (meta, attention) = self.row_meta(item);
            let selected = !self.on_new && self.catalog.focused.as_deref() == Some(id.as_str());
            rows = rows.child(
                ui::row(SharedString::from(format!("profile-{id}")), selected)
                    .when(!ready, |row| row.cursor_default().opacity(0.65))
                    .child(ui::profile_mark(
                        &id,
                        &item.display_name,
                        item.kind == Kind::LegacyDefault,
                        t.size.profile_mark,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(c.text)
                            .child(item.display_name.clone()),
                    )
                    .when(attention, |row| row.child(ui::step_dot(ui::Step::Waits)))
                    .child(
                        ui::text(
                            div().flex_shrink_0().text_color(if attention {
                                c.attention_text
                            } else {
                                c.muted
                            }),
                            &t.text.meta,
                        )
                        .child(meta),
                    )
                    .on_click(cx.listener(move |picker, _, _, cx| picker.choose(&id, cx))),
            );
        }
        menu = menu.child(ui::menu_heading("PROFILE")).child(rows);
        if let Some((_, error)) = &self.selection_error {
            menu = menu.child(
                ui::text(
                    div()
                        .px(px(t.space.s10))
                        .py(px(t.space.s4))
                        .text_color(c.attention_text),
                    &t.text.meta,
                )
                .child(message(error)),
            );
        }
        menu.child(ui::menu_rule()).child(
            ui::row("profiles-new", self.on_new)
                .text_color(c.text_2)
                .child(
                    div()
                        .w(px(t.size.profile_mark))
                        .flex()
                        .justify_center()
                        .child(ui::icon("plus", t.size.icon, c.muted)),
                )
                .child("New profile")
                .on_click(cx.listener(|picker, _, _, cx| picker.new_profile(cx))),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_roster::profiles::Kind;
    fn item(id: &str, state: State) -> Item {
        Item {
            profile_id: id.into(),
            display_name: "same label".into(),
            kind: Kind::Isolated,
            state,
        }
    }
    #[test]
    fn moving_down_past_the_last_ready_profile_reaches_new_profile() {
        let mut catalog = Catalog::default();
        let ticket = catalog.begin();
        assert!(catalog.complete(
            &ticket,
            Ok(vec![
                item("a", State::Ready),
                item("b", State::Ready),
                item("c", State::Failed)
            ])
        ));
        assert!(!catalog.at_end());
        catalog.navigate(1);
        assert!(catalog.at_end());
        catalog.focused = None;
        catalog.focus_last();
        assert_eq!(catalog.focused.as_deref(), Some("b"));
    }

    #[test]
    fn absent_registry_is_first_run_only_without_bound_workspaces() {
        assert!(first_run(Err(Error::RegistryAbsent), false).is_ok_and(|items| items.is_empty()));
        assert_eq!(
            first_run(Err(Error::RegistryAbsent), true).err(),
            Some(Error::RegistryAbsent)
        );
        assert_eq!(
            first_run(Err(Error::CommandFailed), false).err(),
            Some(Error::CommandFailed)
        );
    }

    #[test]
    fn refresh_invalidates_stale_rows_results_and_readiness_without_using_labels() {
        let mut catalog = Catalog::default();
        let first = catalog.begin();
        assert!(catalog.complete(
            &first,
            Ok(vec![item("a", State::Ready), item("b", State::Creating)])
        ));
        assert!(catalog.can_choose("a"));
        assert_eq!(catalog.focused.as_deref(), Some("a"));
        catalog.navigate(1);
        assert_eq!(catalog.focused.as_deref(), Some("a"));
        assert!(!catalog.can_choose("b"));
        assert!(!catalog.can_choose("same label"));
        let next = catalog.begin();
        assert!(!first.load(Ordering::Acquire));
        assert!(!catalog.can_choose("a"));
        assert!(catalog.focused.is_none());
        assert!(!catalog.complete(&first, Ok(vec![item("a", State::Ready)])));
        assert!(catalog.complete(&next, Err(Error::CommandFailed)));
        assert!(catalog.items.is_empty());
        assert!(!catalog.can_choose("a"));
        let fresh = catalog.begin();
        assert!(catalog.complete(
            &fresh,
            Ok(vec![item("a", State::Failed), item("b", State::Ready)])
        ));
        assert!(!catalog.can_choose("a"));
        assert!(catalog.can_choose("b"));
        assert_eq!(catalog.focused_index(), Some(1));
        let large = catalog.begin();
        let mut items = (0..40)
            .map(|index| item(&format!("p-{index}"), State::Ready))
            .collect::<Vec<_>>();
        items[20].state = State::Creating;
        assert!(catalog.complete(&large, Ok(items)));
        for _ in 0..20 {
            catalog.navigate(1);
        }
        assert_eq!(catalog.focused.as_deref(), Some("p-21"));
        assert_eq!(catalog.focused_index(), Some(21));
        drop(catalog);
        assert!(!fresh.load(Ordering::Acquire));
        assert!(!large.load(Ordering::Acquire));
    }
}
