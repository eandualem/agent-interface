//! Navigation to the selected workspace's profile connections.

use super::{Browser, Selection, creation_origin, workspace_profiles::Route};
use agent_interface_core::{Layout, View, ViewKind, Workspace};
use gpui::{Context, Window};

fn existing_pane(space: &Workspace) -> Option<(&str, &str)> {
    space
        .connections_panes()
        .next()
        .map(|(tab, leaf)| (tab.id.as_str(), leaf.id.as_str()))
}

fn visible_empty(model: &Layout) -> Option<&str> {
    let tab = model.tab()?;
    model
        .pane()
        .filter(|pane| pane.empty)
        .or_else(|| {
            (!model.focus_mode())
                .then(|| tab.tree.leaves().into_iter().find(|pane| pane.empty))
                .flatten()
        })
        .map(|leaf| leaf.id.as_str())
}

fn add_connections_tab(model: &mut Layout) -> bool {
    if !model.add_tab(Some("Connections".into()), "/".into(), true) {
        return false;
    }
    let tab = model.tab().expect("added tab").id.clone();
    let pane = model.pane().expect("new tab has a pane").id.clone();
    model.show_view(&tab, &pane, View::of(ViewKind::Connections))
}

impl Browser {
    pub(super) fn connections_workspace(&self) -> Option<&Workspace> {
        let space = self.layout.as_ref()?.model().workspace()?;
        matches!(self.workspace_profiles.route(space), Route::Bound(_)).then_some(space)
    }

    pub(super) fn open_profile_connections(
        &mut self,
        profile: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(update) = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update)
        {
            self.apply(update);
        }
        if self.saving || self.save_failed {
            return;
        }
        let Some(layout) = &self.layout else {
            return;
        };
        let existing = layout
            .model()
            .workspace()
            .filter(|s| s.profile_id.as_deref() == Some(profile))
            .or_else(|| {
                layout
                    .model()
                    .workspaces
                    .iter()
                    .find(|s| s.profile_id.as_deref() == Some(profile))
            });
        if let Some(space) = existing
            && let Some((tab, pane)) = existing_pane(space)
        {
            let (id, t, p) = (space.id.clone(), tab.to_owned(), pane.to_owned());
            let model = self.layout.as_mut().unwrap().model_mut();
            model.choose_workspace(&id);
            model.choose_tab(&t);
            model.choose_pane(&p);
        } else {
            let (base, mut proposed) = if let Some(space) = existing {
                let (base, mut proposed) = layout.save_snapshot();
                proposed.choose_workspace(&space.id);
                (base, proposed)
            } else {
                let Some((base, mut proposed)) = super::workspace::create_profile(layout, name)
                else {
                    return;
                };
                let id = proposed.selected.clone();
                let Some(space) = proposed.workspaces.iter_mut().find(|s| s.id == id) else {
                    return;
                };
                space.profile_id = Some(profile.to_owned());
                (base, proposed)
            };
            let added = match visible_empty(&proposed).map(str::to_owned) {
                Some(leaf) => {
                    let Some(tab) = proposed.tab() else {
                        return;
                    };
                    let tab = tab.id.clone();
                    proposed.choose_pane(&leaf);
                    proposed.show_view(&tab, &leaf, View::of(ViewKind::Connections))
                }
                None => add_connections_tab(&mut proposed),
            };
            if !added {
                return;
            }
            self.creation_from = Some(Selection::capture(layout.model()));
            self.pending_save = Some((base, proposed));
            self.save(cx);
        }
        window.activate_window();
        self.reset_focus = true;
        cx.notify();
    }

    pub(super) fn open_connections(
        &mut self,
        target: Option<&creation_origin::Target>,
        chooser: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(update) = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update)
        {
            self.apply(update);
        }
        if self.saving || self.save_failed || !target.is_some_and(|t| self.creation_is_current(t)) {
            return;
        }
        let Some(space) = self.connections_workspace() else {
            return;
        };
        if let Some(pane) = chooser
            && !space
                .tabs
                .iter()
                .find(|tab| tab.id == space.selected)
                .is_some_and(|tab| {
                    tab.tree
                        .leaves()
                        .iter()
                        .any(|leaf| leaf.id == pane && leaf.empty)
                })
        {
            return;
        }
        let existing = existing_pane(space).map(|(tab, leaf)| (tab.to_owned(), leaf.to_owned()));
        self.editing = None;
        self.chooser = None;
        self.tab_menu = None;
        self.workspace_menu = false;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        if let Some((tab, leaf)) = existing {
            let model = self.layout.as_mut().unwrap().model_mut();
            model.choose_tab(&tab);
            model.choose_pane(&leaf);
            self.reset_focus = true;
            cx.notify();
            return;
        }

        let empty = self
            .layout
            .as_ref()
            .and_then(|layout| visible_empty(layout.model()))
            .map(str::to_owned);
        if let Some(pane) = chooser.or(empty.as_deref()) {
            self.open_view(pane, View::of(ViewKind::Connections), window, cx);
            return;
        }

        // A sidebar or Jump action never replaces a running terminal.
        let layout = self.layout.as_ref().unwrap();
        let (base, mut proposed) = layout.save_snapshot();
        if add_connections_tab(&mut proposed) {
            self.creation_from = Some(Selection::capture(layout.model()));
            self.pending_save = Some((base, proposed));
            self.save(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_reuses_a_visible_empty_sibling() {
        let mut model = Layout::initial("/".into());
        let terminal = model.pane().unwrap().id.clone();
        assert!(model.split(agent_interface_core::Direction::Right, "/".into(), true));
        let chooser = model.pane().unwrap().id.clone();
        assert!(model.choose_pane(&terminal));
        assert_eq!(visible_empty(&model), Some(chooser.as_str()));
        model.set_focus_mode(true);
        assert_eq!(visible_empty(&model), None);
    }

    #[test]
    fn navigation_finds_the_saved_view_in_its_own_workspace() {
        let mut model = Layout::initial("/".into());
        let terminal = model.pane().unwrap().clone();
        assert!(add_connections_tab(&mut model));
        assert!(
            model
                .workspace()
                .unwrap()
                .tabs
                .iter()
                .flat_map(|t| t.tree.leaves())
                .any(|leaf| *leaf == terminal)
        );
        let tab = model.tab().unwrap().id.clone();
        let pane = model.pane().unwrap().id.clone();
        assert!(!model.pane().unwrap().runs_terminal());
        model.add_tab(Some("Connections".into()), "/".into(), true);
        assert_eq!(
            existing_pane(model.workspace().unwrap()),
            Some((tab.as_str(), pane.as_str()))
        );
        model.add_workspace("Other".into());
        assert_eq!(existing_pane(model.workspace().unwrap()), None);
    }
}
