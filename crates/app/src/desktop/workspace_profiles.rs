//! Per-workspace lookup ownership. A stored ID never authorizes legacy I/O.

#[cfg(test)]
mod restoration_tests;

use std::collections::HashMap;

use agent_interface_core::{Layout, Workspace};
use agent_interface_roster::profiles::Connection;

use super::{Browser, agent_status, profile_picker};
use crate::profiles::{
    Client, Error,
    selection::{Attempt, Completion, Selection},
};
use gpui::{Context, Window, prelude::*};

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Owner {
    Legacy,
    Bound(agent_interface_roster::profiles::ConnectionKey),
}

#[derive(Default)]
pub(super) struct Registry {
    entries: HashMap<String, Entry>,
}

struct Entry {
    profile_id: String,
    selection: Selection,
    result: Option<Result<Connection, Error>>,
}

pub(super) enum Route<'a> {
    Legacy,
    Bound(&'a Connection),
    Unavailable,
}

pub(super) struct Work {
    workspace: String,
    attempt: Attempt,
}

pub(super) struct Finished {
    workspace: String,
    completion: Completion,
}

impl Work {
    pub fn run(self) -> Finished {
        Finished {
            workspace: self.workspace,
            completion: self.attempt.run(),
        }
    }
}

impl Registry {
    /// New native windows share successful snapshots, never a selected-profile pointer.
    pub fn inherit(&self) -> Self {
        Self {
            entries: self
                .entries
                .iter()
                .map(|(id, entry)| {
                    (
                        id.clone(),
                        Entry {
                            profile_id: entry.profile_id.clone(),
                            selection: Selection::default(),
                            result: entry.result.clone().filter(|result| result.is_ok()),
                        },
                    )
                })
                .collect(),
        }
    }
    /// Reconcile every workspace, not just the currently selected one.
    /// At most two read/handshake attempts run concurrently per native window.
    pub fn reconcile(&mut self, layout: &Layout, client: Option<&Client>) -> Vec<Work> {
        self.entries.retain(|id, entry| {
            layout.workspaces.iter().any(|space| {
                &space.id == id && space.profile_id.as_deref() == Some(&entry.profile_id)
            })
        });
        for space in &layout.workspaces {
            let Some(profile_id) = &space.profile_id else {
                continue;
            };
            self.entries
                .entry(space.id.clone())
                .or_insert_with(|| Entry {
                    profile_id: profile_id.clone(),
                    selection: Selection::default(),
                    result: None,
                });
        }
        let Some(client) = client else {
            return Vec::new();
        };
        let mut running = self
            .entries
            .values()
            .filter(|entry| entry.selection.pending().is_some())
            .count();
        let mut work = Vec::new();
        // Saved order makes startup deterministic; an unsuccessful read waits
        // for explicit retry instead of continuously probing the provider.
        for space in &layout.workspaces {
            if running >= 2 {
                break;
            }
            let Some(entry) = self.entries.get_mut(&space.id) else {
                continue;
            };
            if entry.result.is_some() || entry.selection.pending().is_some() {
                continue;
            }
            match entry.selection.begin(client, &entry.profile_id) {
                Ok(attempt) => {
                    running += 1;
                    work.push(Work {
                        workspace: space.id.clone(),
                        attempt,
                    });
                }
                Err(error) => entry.result = Some(Err(error)),
            }
        }
        work
    }

    pub fn accept(&mut self, finished: Finished) -> bool {
        let Some(entry) = self.entries.get_mut(&finished.workspace) else {
            return false;
        };
        let Some(result) = entry.selection.accept(finished.completion) else {
            return false;
        };
        entry.result = Some(result.map(|selected| selected.connection().clone()));
        true
    }

    /// Install a picker result only after that exact workspace binding is saved.
    pub fn bind(&mut self, space: &Workspace, connection: Connection) -> bool {
        if space.profile_id.as_deref() != Some(&connection.identity().profile_id) {
            return false;
        }
        self.entries.insert(
            space.id.clone(),
            Entry {
                profile_id: connection.identity().profile_id.clone(),
                selection: Selection::default(),
                result: Some(Ok(connection)),
            },
        );
        true
    }

    pub fn route(&self, space: &Workspace) -> Route<'_> {
        let Some(profile_id) = &space.profile_id else {
            return Route::Legacy;
        };
        match self
            .entries
            .get(&space.id)
            .filter(|entry| &entry.profile_id == profile_id)
            .and_then(|entry| entry.result.as_ref())
        {
            Some(Ok(connection)) => Route::Bound(connection),
            _ => Route::Unavailable,
        }
    }

    /// Moving a retained tab may not change its authenticated authority.
    pub fn can_transfer(&self, source: &Workspace, destination: &Workspace) -> bool {
        match (self.route(source), self.route(destination)) {
            (Route::Legacy, Route::Legacy) => true,
            (Route::Bound(a), Route::Bound(b)) => a.connection_key() == b.connection_key(),
            _ => false,
        }
    }

    /// The saved connection's failure; `None` while it is still being attempted.
    pub fn failure(&self, space: &Workspace) -> Option<&Error> {
        self.entries
            .get(&space.id)
            .filter(|entry| space.profile_id.as_ref() == Some(&entry.profile_id))?
            .result
            .as_ref()?
            .as_ref()
            .err()
    }

    /// Retrying affects this workspace only. Retained panes still own their old
    /// connection until desktop reconciliation explicitly closes those panes.
    pub fn retry(&mut self, space: &Workspace) {
        self.entries.remove(&space.id);
    }
}

/// Run the provider's start on an I/O worker, with the same journal Manage uses.
fn start_service(
    client: Client,
    directory: Option<std::path::PathBuf>,
    profile_id: &str,
) -> Result<(), &'static str> {
    use crate::profiles::{controller::Controller, journal};
    use agent_interface_roster::profiles::operations::{Prepared, Status};
    const FAILED: &str = "This profile's service could not be started.";
    let live = || true;
    let profile = client.show(profile_id, &live).map_err(|_| FAILED)?;
    let directory = crate::library::library_directory(directory.as_deref()).map_err(|_| FAILED)?;
    let journal =
        journal::Journal::open(&directory.join("profile-operations")).map_err(|_| FAILED)?;
    let controller = Controller::new(journal, &client);
    let prepared = Prepared::start(&uuid::Uuid::new_v4().simple().to_string(), &profile)
        .map_err(|_| FAILED)?;
    let key = Controller::key(prepared.request_id());
    controller.enqueue(prepared).map_err(|_| FAILED)?;
    match controller.submit(&key, &live).map_err(|_| FAILED)?.status {
        Status::Failed => Err(FAILED),
        Status::Pending | Status::Succeeded => Ok(()),
    }
}

impl Browser {
    pub(super) fn pane_connection(&self, pane: &str) -> Result<Option<Connection>, ()> {
        let space = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace_for_pane(pane))
            .ok_or(())?;
        match self.workspace_profiles.route(space) {
            Route::Legacy => Ok(None),
            Route::Bound(connection) => Ok(Some(connection.clone())),
            Route::Unavailable => Err(()),
        }
    }

    pub(super) fn pane_owner(&self, pane: &str) -> Option<Owner> {
        self.pane_connection(pane).ok().map(|connection| {
            connection.map_or(Owner::Legacy, |c| Owner::Bound(c.connection_key()))
        })
    }

    pub(super) fn workspace_agents(&self, space: &Workspace) -> Option<&agent_status::LiveAgents> {
        match self.workspace_profiles.route(space) {
            Route::Legacy => self.roster.as_ref(),
            Route::Bound(connection) => self
                .workspace_rosters
                .get(&space.id)
                .filter(|(key, _)| *key == connection.connection_key())
                .map(|(_, agents)| agents),
            Route::Unavailable => None,
        }
    }

    pub(super) fn active_agents(&self) -> Option<&agent_status::LiveAgents> {
        self.workspace_agents(self.layout.as_ref()?.model().workspace()?)
    }

    pub(super) fn legacy_setup_allowed(&mut self) -> bool {
        let origin = self.selected_navigation_origin();
        let update = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update);
        self.legacy_setup_after_update(origin.as_ref(), update)
    }

    /// A pending peer edit cannot turn a setup request into another profile's flow.
    pub(super) fn legacy_setup_after_update(
        &mut self,
        origin: Option<&super::navigation_origin::Origin>,
        update: Option<crate::library::Update>,
    ) -> bool {
        if let Some(update) = update {
            self.apply(update);
        }
        self.selected_origin_is_current(origin)
            && self
                .layout
                .as_ref()
                .and_then(|layout| layout.model().workspace())
                .is_some_and(|space| space.profile_id.is_none())
    }

    /// Whether the selected workspace's connection state replaces its panes.
    pub(super) fn profile_content_unavailable(&self) -> bool {
        self.layout
            .as_ref()
            .and_then(|layout| layout.model().workspace())
            .is_some_and(|space| {
                space.profile_id.is_some()
                    && matches!(self.workspace_profiles.route(space), Route::Unavailable)
            })
    }

    /// A failed roster read loses status, not access: tmux keeps the sessions
    /// running and the attached panes stay connected while the read retries.
    pub(super) fn profile_status_unavailable(&self) -> bool {
        self.active_agents()
            .is_some_and(|agents| agents.unavailable())
    }

    /// Why the selected workspace has no connection. `None` while still connecting.
    pub(super) fn profile_failure(&self) -> Option<&'static str> {
        if matches!(self.profile_client, Some(Err(_))) {
            return Some("Agent Backbone could not be found on this computer.");
        }
        let space = self.layout.as_ref()?.model().workspace()?;
        self.workspace_profiles
            .failure(space)
            .map(profile_picker::message)
    }

    /// Whether Start service could help: the provider is there and knows profiles.
    pub(super) fn profile_can_start(&self) -> bool {
        matches!(self.profile_client, Some(Ok(_)))
            && self
                .layout
                .as_ref()
                .and_then(|layout| layout.model().workspace())
                .and_then(|space| self.workspace_profiles.failure(space))
                .is_some_and(|error| !matches!(error, Error::RegistryAbsent))
    }

    /// Start the selected workspace's stopped service, then connect again. Nothing
    /// else starts it after a reboot. Agents stay stopped until their panes start them.
    pub(super) fn start_profile_service(
        &mut self,
        origin: Option<&super::navigation_origin::Origin>,
        cx: &mut Context<Self>,
    ) {
        if self.profile_starting || !self.selected_origin_is_current(origin) {
            return;
        }
        let Some(Ok(client)) = &self.profile_client else {
            return;
        };
        let Some(space) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace())
        else {
            return;
        };
        let Some(profile_id) = space.profile_id.clone() else {
            return;
        };
        let workspace = space.id.clone();
        let (client, directory) = (client.clone(), self.directory.clone());
        self.profile_starting = true;
        let work = cx
            .background_executor()
            .spawn(async move { start_service(client, directory, &profile_id) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |browser, cx| {
                browser.profile_starting = false;
                if let Err(error) = result {
                    browser.save_notice = Some(error.into());
                } else if let Some(space) = browser.layout.as_ref().and_then(|layout| {
                    layout
                        .model()
                        .workspaces
                        .iter()
                        .find(|space| space.id == workspace)
                }) {
                    browser.workspace_profiles.retry(space);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn keyboard_pane(&self) -> Option<&agent_interface_core::Leaf> {
        if self.profile_content_unavailable() {
            return None;
        }
        self.layout.as_ref()?.model().pane()
    }

    pub(super) fn terminal_available(&self, pane: &str) -> bool {
        let Some(space) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace_for_pane(pane))
        else {
            return false;
        };
        match self.workspace_profiles.route(space) {
            Route::Legacy => true,
            Route::Bound(_) => true,
            Route::Unavailable => false,
        }
    }

    pub(super) fn retire_profile_panes(&mut self) {
        let owners: HashMap<_, _> = self
            .layout
            .iter()
            .flat_map(|layout| &layout.model().workspaces)
            .flat_map(|space| &space.tabs)
            .flat_map(|tab| tab.tree.leaves())
            .filter_map(|leaf| {
                self.pane_owner(&leaf.id)
                    .map(|owner| (leaf.id.clone(), owner))
            })
            .collect();
        let changed: Vec<_> = self
            .pane_owners
            .iter()
            .filter(|(id, owner)| {
                owners.get(*id) != Some(*owner)
                    && self
                        .layout
                        .as_ref()
                        .is_some_and(|layout| layout.model().workspace_for_pane(id).is_some())
            })
            .map(|(id, _)| id.clone())
            .collect();
        let allowed_terminals: std::collections::HashSet<_> = owners
            .keys()
            .filter(|id| self.terminal_available(id))
            .cloned()
            .collect();
        let unavailable: std::collections::HashSet<_> = owners
            .keys()
            .filter(|id| !allowed_terminals.contains(*id))
            .cloned()
            .collect();
        let mut invalidated: Vec<_> = unavailable
            .difference(&self.unavailable_profile_panes)
            .cloned()
            .collect();
        if let Some(chooser) = &self.chooser
            && unavailable.contains(&chooser.destination.pane)
        {
            invalidated.push(chooser.destination.pane.clone());
        }
        self.unavailable_profile_panes = unavailable;
        for id in invalidated.iter().chain(&changed) {
            self.invalidate_profile_pane(id);
        }
        self.terminals.retain(|id, _| {
            self.pane_owners.get(id) == owners.get(id) && allowed_terminals.contains(id)
        });
        self.window_choices
            .retain(|id, _| self.pane_owners.get(id) == owners.get(id) && owners.contains_key(id));
        self.views
            .retain(|id, _| self.pane_owners.get(id) == owners.get(id) && owners.contains_key(id));
        self.new_panes
            .retain(|id, _| self.pane_owners.get(id) == owners.get(id) && owners.contains_key(id));
        self.pane_owners = owners;
        if !changed.is_empty() {
            self.save_notice =
                Some("The workspace binding changed. Existing panes were closed.".into());
        }
    }

    pub(super) fn invalidate_profile_pane(&mut self, pane: &str) {
        let focused = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().pane())
            .is_some_and(|leaf| leaf.id == pane);
        let choosing = self
            .chooser
            .as_ref()
            .is_some_and(|chooser| chooser.destination.pane == pane);
        if choosing {
            self.chooser = None;
        }
        self.reset_focus |= focused || choosing;
    }

    pub(super) fn refresh_profiles(&mut self, cx: &mut Context<Self>) {
        let needed = self.profile_picker_requested
            || self.layout.as_ref().is_some_and(|layout| {
                layout
                    .model()
                    .workspaces
                    .iter()
                    .any(|space| space.profile_id.is_some())
            });
        if needed && self.profile_client.is_none() && !self.profile_client_loading {
            self.profile_client_loading = true;
            let work = cx
                .background_executor()
                .spawn(async { super::onboarding::probe::profile_client() });
            cx.spawn(async move |this, cx| {
                let client = work.await.ok_or(());
                let _ = this.update(cx, |browser, cx| {
                    browser.profile_client_loading = false;
                    browser.profile_client = Some(client);
                    cx.notify();
                });
            })
            .detach();
        }
        let Some(layout) = &self.layout else { return };
        if !self.saving
            && !self.save_failed
            && let Some((id, connection)) = self.pending_profile.take()
            && let Some(space) = layout
                .model()
                .workspaces
                .iter()
                .find(|space| space.id == id)
        {
            self.workspace_profiles.bind(space, connection);
        }
        let work = self.workspace_profiles.reconcile(
            layout.model(),
            self.profile_client
                .as_ref()
                .and_then(|client| client.as_ref().ok()),
        );
        for work in work {
            let work = cx.background_executor().spawn(async move { work.run() });
            cx.spawn(async move |this, cx| {
                let finished = work.await;
                let _ = this.update(cx, |browser, cx| {
                    // Apply any binding changes before accepting a delayed lookup.
                    if let Some(layout) = &browser.layout {
                        browser.workspace_profiles.reconcile(layout.model(), None);
                    }
                    browser.workspace_profiles.accept(finished);
                    cx.notify();
                });
            })
            .detach();
        }
        let ready: Vec<_> = layout
            .model()
            .workspaces
            .iter()
            .filter_map(|space| match self.workspace_profiles.route(space) {
                Route::Bound(connection) => Some((space.id.clone(), connection.clone())),
                _ => None,
            })
            .collect();
        self.workspace_rosters.retain(|id, (key, _)| {
            ready.iter().any(|(workspace, connection)| {
                workspace == id && *key == connection.connection_key()
            })
        });
        for (id, connection) in ready {
            if !self.workspace_rosters.contains_key(&id)
                && let Some(agents) = agent_status::start_bound(connection.clone(), cx)
            {
                self.workspace_rosters
                    .insert(id, (connection.connection_key(), agents));
            }
        }
        if self.roster_updates.is_none() && cx.has_global::<agent_status::Sources>() {
            self.roster_updates =
                Some(cx.observe_global::<agent_status::Sources>(|browser, cx| {
                    browser.refresh_source_matches(cx);
                    cx.notify();
                }));
        }
    }

    /// A delayed retry may only reset the unavailable workspace it displayed.
    pub(super) fn retry_profile_origin(
        &mut self,
        origin: Option<&super::navigation_origin::Origin>,
    ) -> bool {
        if !self.selected_origin_is_current(origin) || !self.profile_content_unavailable() {
            return false;
        }
        let Some(space) = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace())
        else {
            return false;
        };
        self.workspace_profiles.retry(space);
        if matches!(self.profile_client, Some(Err(_))) {
            self.profile_client = None;
        }
        self.reset_focus = true;
        true
    }

    pub(super) fn retry_profile(
        &mut self,
        origin: Option<&super::navigation_origin::Origin>,
        cx: &mut Context<Self>,
    ) {
        if let Some(update) = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update)
        {
            self.apply(update);
        }
        if self.retry_profile_origin(origin) {
            cx.notify();
        }
    }

    pub(super) fn request_profile_workspace(&mut self, cx: &mut Context<Self>) {
        if self.saving || self.save_failed || self.layout.is_none() {
            return;
        }
        self.close_menus();
        // Explicit user intent starts another attempt after discovery failed.
        // Ordinary background refreshes retain the failure to avoid retry loops.
        if matches!(self.profile_client, Some(Err(_))) {
            self.profile_client = None;
        }
        self.profile_picker_requested = true;
        self.refresh_profiles(cx);
        cx.notify();
    }

    pub(super) fn show_requested_profile_picker(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.profile_picker_requested {
            return;
        }
        let Some(client) = &self.profile_client else {
            return;
        };
        self.profile_picker_requested = false;
        let Ok(client) = client else {
            self.save_notice = Some("Profile discovery is unavailable.".into());
            return;
        };
        let client = client.clone();
        let directory = self.directory.clone();
        let has_bound_workspaces = self.layout.as_ref().is_some_and(|layout| {
            layout
                .model()
                .workspaces
                .iter()
                .any(|space| space.profile_id.is_some())
        });
        let picker = cx.new(|cx| {
            profile_picker::ProfilePicker::new(client, directory, has_bound_workspaces, cx)
        });
        window.focus(&picker.read(cx).focus_handle(cx), cx);
        let events = cx.subscribe_in(&picker, window, |browser, picker, choice, window, cx| {
            if !browser
                .profile_picker
                .as_ref()
                .is_some_and(|(active, _)| active == picker)
            {
                return;
            }
            match choice {
                profile_picker::Choice::Cancel => browser.profile_picker = None,
                profile_picker::Choice::Selected(selected) => {
                    // Selection closes the picker internally. Always retire it,
                    // including when a concurrent save prevents this handoff.
                    browser.profile_picker = None;
                    if let Some(focus) = &browser.navigation {
                        window.focus(focus, cx);
                    }
                    cx.notify();
                    if browser.saving || browser.save_failed {
                        return;
                    }
                    let Some(layout) = &browser.layout else {
                        return;
                    };
                    let Some((base, mut proposed)) =
                        super::workspace::create_profile(layout, &selected.item().display_name)
                    else {
                        return;
                    };
                    let selected_id = proposed.selected.clone();
                    let space = proposed
                        .workspaces
                        .iter_mut()
                        .find(|space| space.id == selected_id)
                        .expect("created workspace");
                    space.profile_id = Some(selected.item().profile_id.clone());
                    browser.pending_profile =
                        Some((space.id.clone(), selected.connection().clone()));
                    browser.creation_from =
                        Some(super::layout_edit::Selection::capture(layout.model()));
                    browser.pending_save = Some((base, proposed));
                    browser.profile_picker = None;
                    browser.save(cx);
                }
            }
            if let Some(focus) = &browser.navigation {
                window.focus(focus, cx);
            }
            cx.notify();
        });
        self.profile_picker = Some((picker, events));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::content::tests::provider;

    #[test]
    fn browser_routes_hidden_panes_attention_and_failures_by_owning_workspace() {
        use super::super::components::AgentState;
        use crate::library::WindowLayout;
        use agent_interface_store::Document;
        let (_a_dir, a, a_server) = provider("a", vec![]);
        let (_b_dir, b, b_server) = provider("b", vec![]);
        a_server.join().unwrap();
        b_server.join().unwrap();
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#;
        let path = std::path::Path::new("layouts.db");
        let base = Document::parse(raw.into(), path).unwrap();
        let mut model = Layout::initial("/tmp".into());
        model.workspaces[0].profile_id = Some("profile-a".into());
        let pane_a = model.pane().unwrap().id.clone();
        model.add_workspace("B".into());
        model.add_tab(None, "/tmp".into(), true);
        model.workspaces[1].profile_id = Some("profile-b".into());
        let pane_b = model.pane().unwrap().id.clone();
        let mut browser = Browser {
            layout: Some(WindowLayout::new(base.with_model(&model, path).unwrap())),
            roster: Some(agent_status::LiveAgents::default()),
            ..Browser::default()
        };
        assert!(!Browser::default().legacy_setup_allowed());
        assert!(!browser.legacy_setup_allowed());
        browser.layout.as_mut().unwrap().model_mut().workspaces[1].profile_id = None;
        assert!(browser.legacy_setup_allowed());
        browser.layout.as_mut().unwrap().model_mut().workspaces[1].profile_id =
            Some("profile-b".into());
        assert!(browser.pane_connection(&pane_a).is_err());
        assert!(browser.active_agents().is_none());
        browser
            .workspace_profiles
            .bind(&model.workspaces[0], a.clone());
        browser
            .workspace_profiles
            .bind(&model.workspaces[1], b.clone());
        assert!(
            browser
                .pane_connection(&pane_a)
                .unwrap()
                .unwrap()
                .identity()
                == a.identity()
        );
        assert!(
            browser
                .pane_connection(&pane_b)
                .unwrap()
                .unwrap()
                .identity()
                == b.identity()
        );
        // Inject observations without launching workers; real bound-worker
        // transport and refusal behavior have separate socket fixture coverage.
        for (space, connection, state) in [
            (&model.workspaces[0], &a, "waiting_for_human"),
            (&model.workspaces[1], &b, "busy"),
        ] {
            let agents = agent_status::LiveAgents::default();
            let mut snapshot = agent_interface_roster::Snapshot::default();
            snapshot.update(
                serde_json::json!({"items":[{"name":"same", "state":state}]})
                    .to_string()
                    .as_bytes(),
            );
            agents.update(snapshot);
            browser
                .workspace_rosters
                .insert(space.id.clone(), (connection.connection_key(), agents));
        }
        assert_eq!(
            browser.active_agents().unwrap().get("same").state,
            AgentState::Working
        );
        let attachment = agent_interface_core::Attachment {
            session: "same".into(),
            source_socket: None,
        };
        assert_eq!(
            browser.attachment_state_in(&model.workspaces[0], &attachment),
            Some(AgentState::NeedsYou)
        );
        browser.workspace_profiles.retry(&model.workspaces[1]);
        assert!(browser.active_agents().is_none());
        assert!(browser.profile_content_unavailable());
        assert!(browser.web_content_occluded());
        assert!(!browser.terminal_available(&pane_b));
        assert!(browser.terminal_available(&pane_a));
        assert_eq!(
            browser.attachment_state_in(&model.workspaces[0], &attachment),
            Some(AgentState::NeedsYou)
        );
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .choose_workspace(&model.workspaces[0].id);
        assert!(!browser.profile_content_unavailable());
        assert!(!browser.web_content_occluded());
        assert_eq!(
            browser.active_agents().unwrap().get("same").state,
            AgentState::NeedsYou
        );
    }

    #[test]
    fn transfers_and_new_windows_retain_exact_connection_ownership() {
        let (_first_dir, first, first_server) = provider("a", vec![]);
        let (_second_dir, second, second_server) = provider("a", vec![]);
        first_server.join().unwrap();
        second_server.join().unwrap();
        let mut layout = Layout::initial("/tmp".into());
        layout.add_workspace("Second".into());
        let mut registry = Registry::default();
        assert!(registry.can_transfer(&layout.workspaces[0], &layout.workspaces[1]));
        for space in &mut layout.workspaces {
            space.profile_id = Some("profile-a".into());
        }
        assert!(!registry.can_transfer(&layout.workspaces[0], &layout.workspaces[1]));
        assert!(registry.bind(&layout.workspaces[0], first.clone()));
        assert!(registry.bind(&layout.workspaces[1], first.clone()));
        assert!(registry.can_transfer(&layout.workspaces[0], &layout.workspaces[1]));
        let inherited = registry.inherit();
        assert!(
            matches!(inherited.route(&layout.workspaces[0]), Route::Bound(c) if c.connection_key() == first.connection_key())
        );
        assert!(registry.bind(&layout.workspaces[1], second));
        assert!(!registry.can_transfer(&layout.workspaces[0], &layout.workspaces[1]));
        registry.retry(&layout.workspaces[0]);
        assert!(matches!(
            registry.route(&layout.workspaces[0]),
            Route::Unavailable
        ));
        assert!(
            matches!(inherited.route(&layout.workspaces[0]), Route::Bound(c) if c.connection_key() == first.connection_key())
        );
    }

    #[test]
    fn new_windows_retry_failed_lookups_without_replacing_successful_connections() {
        let (root, connection, server) = provider("a", vec![]);
        server.join().unwrap();
        let mut layout = Layout::initial("/tmp".into());
        layout.workspaces[0].profile_id = Some("profile-a".into());
        layout.add_workspace("Unavailable".into());
        layout.workspaces[1].profile_id = Some("profile-b".into());
        let failed = layout.workspaces[1].id.clone();
        let mut registry = Registry::default();
        assert!(registry.bind(&layout.workspaces[0], connection.clone()));
        registry.entries.insert(
            failed.clone(),
            Entry {
                profile_id: "profile-b".into(),
                selection: Selection::default(),
                result: Some(Err(Error::Unavailable)),
            },
        );
        let client = Client::new(root.path().join("provider"), "/usr/bin:/bin".into()).unwrap();
        // The original window still waits for explicit Retry. A new window
        // starts its own lookup, without probing again for the healthy sibling.
        assert!(registry.reconcile(&layout, Some(&client)).is_empty());
        let mut inherited = registry.inherit();
        let work = inherited.reconcile(&layout, Some(&client));
        assert_eq!(work.len(), 1);
        assert_eq!(work[0].workspace, failed);
        assert!(matches!(
            inherited.route(&layout.workspaces[1]),
            Route::Unavailable
        ));
        assert!(
            matches!(inherited.route(&layout.workspaces[0]), Route::Bound(c)
                if c.connection_key() == connection.connection_key())
        );
        assert!(matches!(
            registry.entries[&failed].result,
            Some(Err(Error::Unavailable))
        ));
        assert!(registry.entries[&failed].selection.pending().is_none());
    }

    #[test]
    fn saved_bindings_never_fall_back_and_reconciliation_invalidates_changed_owners() {
        let (_a_dir, a, a_server) = provider("a", vec![]);
        let (_b_dir, b, b_server) = provider("b", vec![]);
        a_server.join().unwrap();
        b_server.join().unwrap();
        let mut layout = Layout::initial("/tmp".into());
        let first = layout.selected.clone();
        layout.workspaces[0].profile_id = Some("profile-a".into());
        layout.add_workspace("Second".into());
        layout.workspaces[1].profile_id = Some("profile-b".into());
        let mut registry = Registry::default();
        assert!(registry.reconcile(&layout, None).is_empty());
        assert!(matches!(
            registry.route(&layout.workspaces[0]),
            Route::Unavailable
        ));
        assert!(!registry.bind(&layout.workspaces[0], b.clone()));
        assert!(registry.bind(&layout.workspaces[0], a.clone()));
        assert!(registry.bind(&layout.workspaces[1], b.clone()));
        let retained = match registry.route(&layout.workspaces[0]) {
            Route::Bound(c) => c.clone(),
            _ => panic!("bound"),
        };
        assert_eq!(retained.identity(), a.identity());
        registry.retry(&layout.workspaces[1]);
        assert!(matches!(
            registry.route(&layout.workspaces[1]),
            Route::Unavailable
        ));
        assert!(matches!(
            registry.route(&layout.workspaces[0]),
            Route::Bound(_)
        ));
        layout.workspaces[0].profile_id = Some("profile-b".into());
        assert!(matches!(
            registry.route(&layout.workspaces[0]),
            Route::Unavailable
        ));
        registry.reconcile(&layout, None);
        assert_eq!(retained.identity(), a.identity());
        assert!(registry.bind(&layout.workspaces[0], b));
        layout.workspaces.retain(|space| space.id != first);
        registry.reconcile(&layout, None);
        assert!(!registry.entries.contains_key(&first));
        layout.workspaces[0].profile_id = None;
        registry.reconcile(&layout, None);
        assert!(matches!(
            registry.route(&layout.workspaces[0]),
            Route::Legacy
        ));
        assert!(registry.entries.is_empty());
    }
}
