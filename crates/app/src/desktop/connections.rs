//! Navigation to the selected workspace's profile connections.

use super::{
    Browser, Selection, creation_origin, peer_inbox::Summary, views::peer_rows::Badge,
    workspace_profiles::Route,
};
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

/// Where a sidebar, Jump or chooser action opens Connections.
#[derive(Debug, PartialEq, Eq)]
enum Place {
    /// The workspace's Connections pane, by tab and pane.
    Existing(String, String),
    /// An empty pane: the chooser's, else a visible one in the current tab.
    Here(String),
    /// A new pane split to the right of the focused one.
    Beside,
    /// A new tab, as Option-click asks.
    NewTab,
}

/// Choose where Connections opens: the workspace's Connections pane, else
/// the chooser's pane, else (unless `new_tab`) a visible empty pane, else
/// beside the focused pane. A running terminal is never replaced.
fn place(model: &Layout, chooser: Option<&str>, new_tab: bool) -> Option<Place> {
    let space = model.workspace()?;
    if let Some((tab, leaf)) = existing_pane(space) {
        return Some(Place::Existing(tab.to_owned(), leaf.to_owned()));
    }
    if let Some(pane) = chooser {
        return Some(Place::Here(pane.to_owned()));
    }
    if new_tab {
        return Some(Place::NewTab);
    }
    Some(match visible_empty(model) {
        Some(leaf) => Place::Here(leaf.to_owned()),
        None => Place::Beside,
    })
}

fn add_connections_tab(model: &mut Layout) -> bool {
    if !model.add_tab(Some("Connections".into()), "/".into(), true) {
        return false;
    }
    let tab = model.tab().expect("added tab").id.clone();
    let pane = model.pane().expect("new tab has a pane").id.clone();
    model.show_view(&tab, &pane, View::of(ViewKind::Connections))
}

/// How many connection rows the panel shows before "N more".
const PANEL_ROWS: usize = 5;

/// The panel's connection rows: every one up to [`PANEL_ROWS`]; past that,
/// those waiting on the person first, cut to [`PANEL_ROWS`], with how many
/// more there are.
pub(super) fn panel_rows<T>(mut rows: Vec<T>, waits: impl Fn(&T) -> bool) -> (Vec<T>, usize) {
    if rows.len() <= PANEL_ROWS {
        return (rows, 0);
    }
    // Stable, so rows keep their order within each group.
    rows.sort_by_key(|row| !waits(row));
    let more = rows.len() - PANEL_ROWS;
    rows.truncate(PANEL_ROWS);
    (rows, more)
}

/// Whether the panel offers "Connect a profile" when it lists no
/// connection: only once a read found none and the newest read did not fail,
/// since a failed read keeps the last overview and the heading then says so.
pub(super) fn offers_connect(read: bool, unavailable: bool) -> bool {
    read && !unavailable
}

/// The CONNECTIONS heading's summary, each part with its colour: what waits
/// on the person in accent, refusals in attention, else how many are
/// connected.
pub(super) fn heading(summary: &Summary) -> Vec<(String, Badge)> {
    let counted = |count: usize, one: &str, many: &str| match count {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    };
    let overview = &summary.overview;
    let refused = overview
        .connections
        .iter()
        .filter(|p| p.state == "revocation_pending" && p.revocation_failure.is_some())
        .count();
    let parts = [
        (
            overview.pending_reviews,
            "to review",
            "to review",
            Badge::Waiting,
        ),
        (summary.requests(), "request", "requests", Badge::Waiting),
        (
            summary.declined_outgoing(),
            "declined",
            "declined",
            Badge::Failed,
        ),
        (refused, "refused", "refused", Badge::Failed),
    ];
    let mut heading: Vec<_> = parts
        .into_iter()
        .filter(|(count, ..)| *count > 0)
        .map(|(count, one, many, badge)| (counted(count, one, many), badge))
        .collect();
    if heading.is_empty() {
        let connected = overview
            .connections
            .iter()
            .filter(|p| p.state == "connected")
            .count();
        heading.push((format!("{connected} connected"), Badge::Quiet));
    }
    heading
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

    /// Open Connections where [`place`] says; `new_tab` is Option-click.
    pub(super) fn open_connections(
        &mut self,
        target: Option<&creation_origin::Target>,
        chooser: Option<&str>,
        new_tab: bool,
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
        let Some(place) = place(self.layout.as_ref().unwrap().model(), chooser, new_tab) else {
            return;
        };
        self.editing = None;
        self.chooser = None;
        self.tab_menu = None;
        self.workspace_menu = false;
        if let Some(focus) = &self.navigation {
            window.focus(focus, cx);
        }
        let add: fn(&mut Layout) -> bool = match place {
            Place::Existing(tab, leaf) => {
                let model = self.layout.as_mut().unwrap().model_mut();
                model.choose_tab(&tab);
                model.choose_pane(&leaf);
                self.reset_focus = true;
                cx.notify();
                return;
            }
            Place::Here(pane) => {
                self.open_view(&pane, View::of(ViewKind::Connections), window, cx);
                return;
            }
            Place::Beside => {
                // Split the focused pane as any view opened beside it is.
                if let Some(pane) = self
                    .layout
                    .as_ref()
                    .and_then(|layout| layout.model().pane())
                    .map(|pane| pane.id.clone())
                {
                    self.open_beside(
                        &pane,
                        super::Beside::View(View::of(ViewKind::Connections)),
                        window,
                        cx,
                    );
                    return;
                }
                add_connections_tab
            }
            Place::NewTab => add_connections_tab,
        };
        let layout = self.layout.as_ref().unwrap();
        let (base, mut proposed) = layout.save_snapshot();
        if add(&mut proposed) {
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
    fn connections_open_in_its_pane_else_an_empty_one_else_beside_the_focused_one() {
        let mut model = Layout::initial("/".into());
        let terminal = model.pane().unwrap().id.clone();
        let tab = model.tab().unwrap().id.clone();
        // Only a terminal: beside it, or a new tab with Option.
        assert_eq!(place(&model, None, false), Some(Place::Beside));
        assert_eq!(place(&model, None, true), Some(Place::NewTab));
        // The chooser names its own pane.
        assert_eq!(
            place(&model, Some("chooser"), true),
            Some(Place::Here("chooser".into()))
        );

        // Once a Connections pane is open beside it, every way of opening
        // focuses that pane.
        let mut beside = model.clone();
        assert!(beside.split(agent_interface_core::Direction::Right, "/".into(), true));
        let opened = beside.pane().unwrap().id.clone();
        assert!(beside.show_view(&tab, &opened, View::of(ViewKind::Connections)));
        assert!(beside.choose_pane(&terminal));
        for new_tab in [false, true] {
            assert_eq!(
                place(&beside, None, new_tab),
                Some(Place::Existing(tab.clone(), opened.clone()))
            );
        }

        // A visible empty pane is used, unless Option asks for a new tab.
        assert!(model.split(agent_interface_core::Direction::Below, "/".into(), true));
        let empty = model.pane().unwrap().id.clone();
        assert!(model.choose_pane(&terminal));
        assert_eq!(place(&model, None, false), Some(Place::Here(empty)));
        assert_eq!(place(&model, None, true), Some(Place::NewTab));
    }

    #[test]
    fn past_five_rows_those_waiting_come_first_then_how_many_more() {
        let few: Vec<(u8, bool)> = (0..5).map(|n| (n, n == 4)).collect();
        assert_eq!(panel_rows(few.clone(), |row| row.1), (few, 0));
        let many: Vec<(u8, bool)> = (0..8).map(|n| (n, n % 3 == 2)).collect();
        let (shown, more) = panel_rows(many, |row| row.1);
        assert_eq!(more, 3);
        assert_eq!(
            shown.iter().map(|row| row.0).collect::<Vec<_>>(),
            [2, 5, 0, 1, 3]
        );
        assert_eq!(
            panel_rows(Vec::<(u8, bool)>::new(), |row| row.1),
            (Vec::new(), 0)
        );
    }

    #[test]
    fn connect_a_profile_shows_only_for_a_good_read_that_found_none() {
        assert!(offers_connect(true, false));
        // Not read yet, or the newest read failed with an older one kept.
        assert!(!offers_connect(false, false));
        assert!(!offers_connect(true, true));
        assert!(!offers_connect(false, true));
    }

    #[test]
    fn the_heading_says_what_waits_in_accent_and_refusals_in_attention() {
        use agent_interface_roster::profiles::peers as api;
        let invitation = |id: &str, direction, state: &str| api::Invitation {
            request_id: id.into(),
            peer_id: format!("peer-{id}"),
            display_name: None,
            direction,
            expires_at: "2026-11-30T00:00:00Z".into(),
            state: state.into(),
            generation: None,
        };
        let overview = |pending_reviews, invitations| api::Overview {
            local_id: "local".into(),
            invitations,
            connections: Vec::new(),
            pending_reviews,
        };
        let summary = Summary::from_overview(overview(
            2,
            vec![
                invitation("a", api::Direction::Incoming, "pending"),
                invitation("b", api::Direction::Outgoing, "declined"),
                // Neither waits on the person nor failed.
                invitation("c", api::Direction::Outgoing, "pending"),
            ],
        ));
        assert_eq!(
            heading(&summary),
            [
                ("2 to review".to_owned(), Badge::Waiting),
                ("1 request".to_owned(), Badge::Waiting),
                ("1 declined".to_owned(), Badge::Failed),
            ]
        );
        let quiet = Summary::from_overview(overview(0, Vec::new()));
        assert_eq!(heading(&quiet), [("0 connected".to_owned(), Badge::Quiet)]);
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
