//! Native profile picker with explicit service management. Its owner must bind the verified result to
//! a workspace explicitly; constructing this pane does not change app sources.

use super::{
    components as ui,
    design::{colors, tokens},
};
use crate::profiles::{
    Client, Error,
    selection::{Selected, Selection},
};
use agent_interface_roster::profiles::{Item, State};
use gpui::{
    Context, EventEmitter, FocusHandle, ScrollHandle, SharedString, Window, div, prelude::*, px,
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(super) enum Choice {
    Selected(Box<Selected>),
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
    selection_error: Option<Error>,
    closed: bool,
    pub focus: FocusHandle,
    scroll: ScrollHandle,
    directory: Option<std::path::PathBuf>,
    has_bound_workspaces: bool,
    management: Option<(
        gpui::Entity<super::profile_management::Management>,
        gpui::Subscription,
    )>,
}
impl EventEmitter<Choice> for ProfilePicker {}
impl ProfilePicker {
    pub fn focus_handle(&self, cx: &gpui::App) -> FocusHandle {
        self.management.as_ref().map_or_else(
            || self.focus.clone(),
            |(pane, _)| pane.read(cx).focus_handle(cx),
        )
    }
    /// The caller supplies the already resolved executable and captured caller
    /// context. No ambient provider or Default profile is selected implicitly.
    pub fn new(
        client: Client,
        directory: Option<std::path::PathBuf>,
        has_bound_workspaces: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut picker = Self {
            client,
            catalog: Catalog::default(),
            selection: Selection::default(),
            selection_error: None,
            closed: false,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            directory,
            has_bound_workspaces,
            management: None,
        };
        picker.refresh(cx);
        picker
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        self.selection.cancel();
        self.selection_error = None;
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
        if self.closed || self.management.is_some() || !self.catalog.can_choose(id) {
            return;
        }
        self.selection_error = None;
        let attempt = match self.selection.begin(&self.client, id) {
            Ok(attempt) => attempt,
            Err(error) => {
                self.selection_error = Some(error);
                cx.notify();
                return;
            }
        };
        let work = cx.background_executor().spawn(async move { attempt.run() });
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
                            cx.emit(Choice::Selected(Box::new(selected)));
                        }
                        Err(error) => picker.selection_error = Some(error),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn manage(&mut self, profile: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.closed || self.management.is_some() {
            return;
        }
        if let Some(id) = &profile
            && !self.catalog.can_choose(id)
        {
            return;
        }
        self.selection.cancel();
        self.selection_error = None;
        let client = self.client.clone();
        let directory = self.directory.clone();
        let pane =
            cx.new(|cx| super::profile_management::Management::new(client, directory, profile, cx));
        window.focus(&pane.read(cx).focus.clone(), cx);
        let subscription = cx.subscribe_in(
            &pane,
            window,
            |picker, pane, _: &super::profile_management::Back, window, cx| {
                if !picker
                    .management
                    .as_ref()
                    .is_some_and(|(active, _)| active == pane)
                {
                    return;
                }
                picker.management = None;
                picker.refresh(cx);
                window.focus(&picker.focus, cx);
            },
        );
        self.management = Some((pane, subscription));
        cx.notify();
    }
    fn reveal_focus(&self) {
        if let Some(index) = self.catalog.focused_index() {
            self.scroll.scroll_to_item(index);
        }
    }
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.selection.cancel();
        self.catalog.ticket.store(false, Ordering::Release);
        cx.emit(Choice::Cancel);
        cx.notify();
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
        Error::Profile(ProfileError::Unavailable | ProfileError::Refused(_)) => {
            "The profile could not be reached or authenticated. Its service may be stopped."
        }
        Error::Unavailable => "The profile discovery command is unavailable.",
        Error::RegistryAbsent => "The profile registry is missing.",
        Error::Invalid | Error::TooLarge | Error::Profile(ProfileError::Invalid) => {
            "The provider returned invalid profile information."
        }
    }
}

impl ProfilePicker {
    /// A management pane may be running a provider command or holding a form.
    pub fn managing(&self) -> bool {
        self.management.is_some()
    }
}

impl Render for ProfilePicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((pane, _)) = &self.management {
            return div().size_full().child(pane.clone());
        }
        let (t, c) = (tokens(), colors());
        let mut rows = div()
            .id("profile-rows")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .p(px(t.space.s12));
        if self.catalog.loading {
            rows = rows.child(ui::group_note("Loading profiles…"));
        }
        if let Some(error) = self.catalog.error {
            rows = rows.child(ui::group_note(message(&error)));
        }
        if !self.catalog.loading && self.catalog.error.is_none() && self.catalog.items.is_empty() {
            rows = rows.child(ui::group_note("No profiles are registered."));
        }
        for item in &self.catalog.items {
            let id = item.profile_id.clone();
            let manage_id = id.clone();
            let ready = item.state == State::Ready;
            let pending = self.selection.pending() == Some(id.as_str());
            let state = match item.state {
                State::Ready => "Ready to connect",
                State::Creating => "Creating",
                State::Failed => "Creation failed",
            };
            rows = rows.child(
                ui::row(
                    SharedString::from(format!("profile-{id}")),
                    self.catalog.focused.as_deref() == Some(id.as_str()),
                )
                .h(px(t.size.row * 2.0))
                .when(!ready, |row| row.cursor_default().opacity(0.65))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(item.display_name.clone())
                        .child(
                            div()
                                .text_color(c.muted)
                                .child(format!("{} · {state}", item.profile_id)),
                        ),
                )
                .when(ready, |row| {
                    row.child(
                        ui::button(
                            SharedString::from(format!("manage-{manage_id}")),
                            "Manage",
                            ui::Button::Secondary,
                        )
                        .on_click(cx.listener(
                            move |picker, _, window, cx| {
                                cx.stop_propagation();
                                picker.manage(Some(manage_id.clone()), window, cx);
                            },
                        )),
                    )
                })
                .when(pending, |row| row.child("Connecting…"))
                .on_click(cx.listener(move |picker, _, _, cx| picker.choose(&id, cx))),
            );
        }
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(c.surface)
            .font_family(t.font.ui.clone())
            .track_focus(&self.focus)
            .capture_key_down(cx.listener(|picker, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" => picker.cancel(cx),
                    "up" => picker.catalog.navigate(-1),
                    "down" => picker.catalog.navigate(1),
                    "enter" => {
                        if let Some(id) = picker.catalog.focused.clone() {
                            picker.choose(&id, cx);
                        }
                    }
                    _ => return,
                }
                picker.reveal_focus();
                cx.stop_propagation();
                cx.notify();
            }))
            .child(ui::view_header("layers", "Choose a profile"))
            .child(rows)
            .children(
                self.selection_error
                    .map(|error| ui::group_note(message(&error))),
            )
            .child(
                ui::view_toolbar()
                    .px(px(t.space.s12))
                    .gap(px(t.space.s8))
                    .child(
                        ui::button("profiles-refresh", "Refresh", ui::Button::Secondary)
                            .on_click(cx.listener(|picker, _, _, cx| picker.refresh(cx))),
                    )
                    .when(
                        self.catalog
                            .focused
                            .as_ref()
                            .is_some_and(|id| self.catalog.can_choose(id)),
                        |bar| {
                            bar.child(
                                ui::button(
                                    "profiles-manage",
                                    "Manage selected",
                                    ui::Button::Secondary,
                                )
                                .on_click(cx.listener(
                                    |picker, _, window, cx| {
                                        if let Some(id) = picker.catalog.focused.clone() {
                                            picker.manage(Some(id), window, cx);
                                        }
                                    },
                                )),
                            )
                        },
                    )
                    .child(
                        ui::button(
                            "profiles-operations",
                            "Manage profiles",
                            ui::Button::Secondary,
                        )
                        .on_click(
                            cx.listener(|picker, _, window, cx| picker.manage(None, window, cx)),
                        ),
                    )
                    .child(
                        ui::button("profiles-cancel", "Cancel", ui::Button::Secondary)
                            .on_click(cx.listener(|picker, _, _, cx| picker.cancel(cx))),
                    ),
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
