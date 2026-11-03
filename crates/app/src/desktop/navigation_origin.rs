//! Delayed navigation retains the displayed workspace and connection epoch.

use super::{Browser, agent_destination, workspace_profiles::Route};
use agent_interface_roster::profiles::ConnectionKey;
use std::{path::PathBuf, rc::Rc};

#[cfg(test)]
mod tests;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Origin {
    workspace: String,
    profile: Option<String>,
    connection: Option<ConnectionKey>,
    directory: Option<PathBuf>,
    legacy_source: Option<Result<PathBuf, String>>,
}

#[derive(Clone)]
pub(super) struct AgentTarget {
    pub origin: Origin,
    pub name: String,
}

/// A new subscription generation follows a moved pane; queued old events expire.
#[derive(Default)]
pub(super) struct ViewEvents {
    pub subscription: Option<gpui::Subscription>,
    ticket: Option<Rc<Origin>>,
}

impl ViewEvents {
    pub fn refresh(&mut self, origin: Option<Origin>) -> Option<Rc<Origin>> {
        if self.ticket.as_deref() == origin.as_ref() {
            return None;
        }
        self.subscription = None;
        self.ticket = origin.map(Rc::new);
        self.ticket.clone()
    }

    pub fn accepts(&self, ticket: &Rc<Origin>, origin: Option<&Origin>) -> bool {
        self.ticket
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(active, ticket))
            && origin == Some(ticket.as_ref())
    }
}

impl Browser {
    /// Close only content from the displayed workspace, including its connection epoch.
    pub(super) fn close_tab_for(
        &self,
        origin: &Origin,
        tab: &str,
    ) -> Option<(
        agent_interface_store::Document,
        agent_interface_core::Layout,
    )> {
        if !self.selected_origin_is_current(Some(origin)) {
            return None;
        }
        super::close_tab_proposal(self.layout.as_ref()?, &origin.workspace, tab)
    }

    pub(super) fn close_pane_for(
        &self,
        origin: &Origin,
        tab: &str,
        pane: &agent_interface_core::Leaf,
    ) -> Option<(
        agent_interface_store::Document,
        agent_interface_core::Layout,
    )> {
        if !self.selected_origin_is_current(Some(origin)) {
            return None;
        }
        super::close_pane_proposal(self.layout.as_ref()?, &origin.workspace, tab, pane)
    }

    pub(super) fn select_workspace_origin(&mut self, origin: &Origin) -> bool {
        if self.navigation_origin(&origin.workspace).as_ref() != Some(origin) {
            return false;
        }
        self.layout
            .as_mut()
            .is_some_and(|layout| layout.model_mut().choose_workspace(&origin.workspace))
    }

    pub(super) fn selected_navigation_origin(&self) -> Option<Origin> {
        self.navigation_origin(&self.layout.as_ref()?.model().workspace()?.id)
    }

    pub(super) fn selected_origin_is_current(&self, origin: Option<&Origin>) -> bool {
        origin.is_some() && self.selected_navigation_origin().as_ref() == origin
    }

    pub(super) fn navigation_origin(&self, workspace: &str) -> Option<Origin> {
        let space = self
            .layout
            .as_ref()?
            .model()
            .workspaces
            .iter()
            .find(|s| s.id == workspace)?;
        Some(Origin {
            workspace: space.id.clone(),
            profile: space.profile_id.clone(),
            connection: match self.workspace_profiles.route(space) {
                Route::Bound(connection) => Some(connection.connection_key()),
                _ => None,
            },
            directory: self.directory.clone(),
            legacy_source: space
                .profile_id
                .is_none()
                .then(|| self.source.clone())
                .flatten(),
        })
    }

    pub(super) fn pane_origin_is_current(&self, pane: &str, origin: Option<&Origin>) -> bool {
        origin.is_some() && self.pane_navigation_origin(pane).as_ref() == origin
    }

    pub(super) fn pane_navigation_origin(&self, leaf: &str) -> Option<Origin> {
        let space = self.layout.as_ref()?.model().workspace_for_pane(leaf)?;
        self.navigation_origin(&space.id)
    }

    pub(super) fn agent_target(&self, name: &str) -> Option<AgentTarget> {
        let workspace = &self.layout.as_ref()?.model().workspace()?.id;
        Some(AgentTarget {
            origin: self.navigation_origin(workspace)?,
            name: name.to_owned(),
        })
    }

    /// Resolve the original agent row without consulting the selected profile.
    pub(super) fn select_agent_target(&mut self, target: &AgentTarget) -> bool {
        if self.navigation_origin(&target.origin.workspace).as_ref() != Some(&target.origin) {
            return false;
        }
        let Some(layout) = self.layout.as_ref() else {
            return false;
        };
        let model = layout.model();
        let Some(space) = model
            .workspaces
            .iter()
            .find(|s| s.id == target.origin.workspace)
        else {
            return false;
        };
        let destination = match self.workspace_profiles.route(space) {
            Route::Bound(connection) => {
                let mut tabs: Vec<_> = space.tabs.iter().collect();
                tabs.sort_by_key(|tab| tab.id != space.selected);
                tabs.into_iter().find_map(|tab| {
                    tab.tree
                        .leaves()
                        .into_iter()
                        .find(|leaf| {
                            leaf.attachment.as_ref().is_some_and(|attachment| {
                                attachment.session == target.name
                                    && attachment.source_socket.as_deref().is_none_or(|path| {
                                        std::path::Path::new(path) == connection.tmux_socket()
                                    })
                            })
                        })
                        .map(|leaf| (space.id.clone(), tab.id.clone(), leaf.id.clone()))
                })
            }
            Route::Unavailable => None,
            Route::Legacy => {
                let source = target
                    .origin
                    .legacy_source
                    .as_ref()
                    .and_then(|s| s.as_ref().ok())
                    .and_then(|path| crate::library::location::resolve_path(path).ok());
                agent_destination(model, &target.name, |socket| match socket {
                    None => source.is_some(),
                    Some(socket) => {
                        source.is_some()
                            && crate::library::location::resolve_path(std::path::Path::new(socket))
                                .ok()
                                == source
                    }
                })
            }
        };
        let Some((workspace, tab, pane)) = destination else {
            return false;
        };
        let model = self.layout.as_mut().unwrap().model_mut();
        model.choose_workspace(&workspace) && model.choose_tab(&tab) && model.choose_pane(&pane)
    }

    /// A stale Jump row cannot select a replacement workspace or pane.
    pub(super) fn select_navigation(
        &mut self,
        origin: &Origin,
        tab: &str,
        pane: Option<&str>,
    ) -> bool {
        if self.navigation_origin(&origin.workspace).as_ref() != Some(origin) {
            return false;
        }
        let Some(layout) = &mut self.layout else {
            return false;
        };
        let model = layout.model_mut();
        let valid = model
            .workspaces
            .iter()
            .find(|space| space.id == origin.workspace)
            .and_then(|space| space.tabs.iter().find(|candidate| candidate.id == tab))
            .is_some_and(|tab| {
                pane.is_none_or(|id| tab.tree.leaves().iter().any(|leaf| leaf.id == id))
            });
        if !valid {
            return false;
        }
        model.choose_workspace(&origin.workspace)
            && model.choose_tab(tab)
            && pane.is_none_or(|pane| model.choose_pane(pane))
    }
}
