use super::*;
use crate::{library::WindowLayout, roster::content::tests::provider};
use agent_interface_core::{Attachment, Layout};
use agent_interface_roster::profiles::Connection;
use agent_interface_store::Document;

fn browser(a: &Connection, b: &Connection) -> Browser {
    let path = std::path::Path::new("layouts.db");
    let base = Document::parse(
        r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#.into(), path,
    ).unwrap();
    let mut model = Layout::initial("/".into());
    model.workspaces[0].profile_id = Some(a.identity().profile_id.clone());
    model.attach(Some(Attachment {
        session: "worker".into(),
        source_socket: None,
    }));
    model.add_workspace("Same display name".into());
    model.add_tab(None, "/".into(), true);
    model.workspaces[1].profile_id = Some(b.identity().profile_id.clone());
    model.attach(Some(Attachment {
        session: "worker".into(),
        source_socket: None,
    }));
    let mut browser = Browser {
        layout: Some(WindowLayout::new(base.with_model(&model, path).unwrap())),
        ..Browser::default()
    };
    browser
        .workspace_profiles
        .bind(&model.workspaces[0], a.clone());
    browser
        .workspace_profiles
        .bind(&model.workspaces[1], b.clone());
    // These observations populate the actual sidebar/Jump source. They never
    // substitute for the independently authenticated connection fixtures.
    for (space, connection) in model.workspaces.iter().zip([a, b]) {
        let agents = super::super::agent_status::LiveAgents::default();
        let mut snapshot = agent_interface_roster::Snapshot::default();
        snapshot.update(br#"{"items":[{"name":"worker","state":"waiting_for_human"}]}"#);
        agents.update(snapshot);
        browser
            .workspace_rosters
            .insert(space.id.clone(), (connection.connection_key(), agents));
    }
    browser
}

fn selected(browser: &Browser) -> String {
    browser.layout.as_ref().unwrap().model().selected.clone()
}

#[test]
fn hidden_profile_invalidation_preserves_the_healthy_workspace_focus() {
    let (_a_dir, a, a_server) = provider("a", vec![]);
    let (_b_dir, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    let pane_a = &spaces[0].tabs[0].tree.leaves()[0].id;
    let pane_b = &spaces[1].tabs[0].tree.leaves()[0].id;
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    // Both failed observations and changed owners use this retirement policy.
    browser.invalidate_profile_pane(pane_a);
    assert!(!browser.reset_focus);
    browser.layout.as_mut().unwrap().model_mut().workspaces[0].profile_id = None;
    browser.invalidate_profile_pane(pane_a);
    assert!(!browser.reset_focus);
    assert_eq!(selected(&browser), spaces[1].id);
    browser.invalidate_profile_pane(pane_b);
    assert!(browser.reset_focus);
}

#[test]
fn delayed_sidebar_view_and_jump_targets_return_to_the_displayed_profile() {
    let (_a_dir, a, a_server) = provider("a", vec![]);
    let (_b_dir, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    let sidebar_a = browser.agent_target("worker").unwrap();
    let pane_a = &spaces[0].tabs[0].tree.leaves()[0].id;
    let view_a = AgentTarget {
        origin: browser
            .navigation_origin(
                &browser
                    .layout
                    .as_ref()
                    .unwrap()
                    .model()
                    .workspace_for_pane(pane_a)
                    .unwrap()
                    .id,
            )
            .unwrap(),
        name: "worker".into(),
    };
    let old_jump = browser
        .jump_hits()
        .into_iter()
        .find(|hit| hit.group == "AGENTS")
        .unwrap();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    let new_jump = browser
        .jump_hits()
        .into_iter()
        .find(|hit| hit.group == "AGENTS")
        .unwrap();
    let super::super::jump::Target::Agent(jump_a) = old_jump.target else {
        panic!("agent result")
    };
    let super::super::jump::Target::Agent(jump_b) = new_jump.target else {
        panic!("agent result")
    };
    assert_eq!(jump_a.name, jump_b.name);
    // Deliver held callbacks after B is selected. Each must select A's saved
    // pane, not the identical name at the same result position in B.
    for target in [&sidebar_a, &view_a, &jump_a] {
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .choose_workspace(&spaces[1].id);
        assert!(browser.select_agent_target(target));
        assert_eq!(selected(&browser), spaces[0].id);
        assert_eq!(
            &browser.layout.as_ref().unwrap().model().pane().unwrap().id,
            pane_a
        );
    }
    assert!(browser.select_agent_target(&jump_b));
    assert_eq!(selected(&browser), spaces[1].id);
}

#[test]
fn refused_replaced_and_restored_connections_reject_old_callbacks_without_fallback() {
    let (_a_dir, a, a_server) = provider("a", vec![]);
    let (_b_dir, b, b_server) = provider("b", vec![]);
    let (_replacement_dir, replacement, replacement_server) = provider("a", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    replacement_server.join().unwrap();
    assert_eq!(a.identity(), replacement.identity());
    assert!(a.connection_key() != replacement.connection_key());
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    let old = browser.agent_target("worker").unwrap();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    browser.workspace_profiles.retry(&spaces[0]);
    assert!(!browser.select_agent_target(&old));
    assert_eq!(selected(&browser), spaces[1].id);
    browser
        .workspace_profiles
        .bind(&spaces[0], replacement.clone());
    assert!(!browser.select_agent_target(&old));
    assert!(!browser.select_navigation(&old.origin, &spaces[0].tabs[0].id, None));
    assert_eq!(selected(&browser), spaces[1].id);

    // A reloaded persisted document and fresh handshake must not resurrect the
    // previous window's retained callback, even with the same IDs and tuple.
    let path = std::path::Path::new("layouts.db");
    let (base, model) = browser.layout.as_ref().unwrap().save_snapshot();
    let raw = base.with_model(&model, path).unwrap().raw_json().to_owned();
    browser.layout = Some(WindowLayout::new(
        Document::parse(raw, std::path::Path::new("layouts.db")).unwrap(),
    ));
    assert!(!browser.select_agent_target(&old));
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    let fresh = browser.agent_target("worker").unwrap();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    assert!(browser.select_agent_target(&fresh));
    browser.directory = Some("/another-library".into());
    assert!(!browser.select_agent_target(&fresh));
}

#[test]
fn stale_jump_panes_do_not_select_foreign_bindings_or_partially_change_selection() {
    let (_a_dir, a, a_server) = provider("a", vec![]);
    let (_b_dir, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    let origin = browser.navigation_origin(&spaces[0].id).unwrap();
    assert!(!browser.select_navigation(&origin, &spaces[0].tabs[0].id, Some("removed-pane")));
    assert_eq!(selected(&browser), spaces[1].id);
    browser.layout.as_mut().unwrap().model_mut().workspaces[0].profile_id =
        Some(b.identity().profile_id.clone());
    let changed = browser.layout.as_ref().unwrap().model().workspaces[0].clone();
    browser.workspace_profiles.bind(&changed, b);
    assert!(!browser.select_navigation(&origin, &spaces[0].tabs[0].id, None));
    assert_eq!(selected(&browser), spaces[1].id);
}

#[test]
fn moved_view_refreshes_events_and_survives_source_workspace_deletion() {
    let path = std::path::Path::new("layouts.db");
    let base = Document::parse(
        r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#.into(),
        path,
    )
    .unwrap();
    let mut model = Layout::initial("/".into());
    model.attach(Some(Attachment {
        session: "worker".into(),
        source_socket: None,
    }));
    let source = model.selected.clone();
    let pane = model.pane().unwrap().id.clone();
    model.add_workspace("Destination".into());
    let destination = model.selected.clone();
    model.choose_workspace(&source);
    let mut browser = Browser {
        layout: Some(WindowLayout::new(base.with_model(&model, path).unwrap())),
        source: Some(Ok("/fixture-socket".into())),
        ..Browser::default()
    };
    let mut events = ViewEvents::default();
    let old = events
        .refresh(browser.pane_navigation_origin(&pane))
        .unwrap();
    assert!(events.accepts(&old, browser.pane_navigation_origin(&pane).as_ref()));
    assert!(
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .transfer_tab(&destination)
    );
    // Reject a queued source event before the next render renews subscriptions.
    assert!(!events.accepts(&old, browser.pane_navigation_origin(&pane).as_ref()));
    let fresh = events
        .refresh(browser.pane_navigation_origin(&pane))
        .unwrap();
    assert!(!events.accepts(&old, browser.pane_navigation_origin(&pane).as_ref()));
    let model = browser.layout.as_mut().unwrap().model_mut();
    model.choose_workspace(&source);
    assert!(model.delete_workspace());
    assert!(events.accepts(&fresh, browser.pane_navigation_origin(&pane).as_ref()));
    assert!(browser.select_agent_target(&AgentTarget {
        origin: fresh.as_ref().clone(),
        name: "worker".into()
    }));
    assert_eq!(selected(&browser), destination);
    // Moving away and back must not reactivate the old subscription generation.
    let model = browser.layout.as_mut().unwrap().model_mut();
    model.add_workspace("Third".into());
    let third = model.selected.clone();
    model.choose_workspace(&destination);
    assert!(model.transfer_tab(&third));
    events
        .refresh(browser.pane_navigation_origin(&pane))
        .unwrap();
    assert!(
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .transfer_tab(&destination)
    );
    let newest = events
        .refresh(browser.pane_navigation_origin(&pane))
        .unwrap();
    assert!(!events.accepts(&fresh, browser.pane_navigation_origin(&pane).as_ref()));
    assert!(events.accepts(&newest, browser.pane_navigation_origin(&pane).as_ref()));
}

#[test]
fn control_origins_allow_local_focus_changes_but_refuse_selection_and_peer_rebinding() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let origin = browser.selected_navigation_origin().unwrap();
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    assert!(browser.selected_origin_is_current(Some(&origin)));
    browser.layout.as_mut().unwrap().model_mut().split(
        agent_interface_core::Direction::Right,
        "/".into(),
        true,
    );
    assert!(browser.selected_origin_is_current(Some(&origin)));
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    assert!(!browser.selected_origin_is_current(Some(&origin)));
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    let (base, mut peer) = browser.layout.as_ref().unwrap().save_snapshot();
    peer.workspaces[1].profile_id = Some(a.identity().profile_id.clone());
    browser.apply(crate::library::Update {
        document: Some(
            base.with_model(&peer, std::path::Path::new("layouts.db"))
                .unwrap(),
        ),
        error: None,
    });
    assert!(!browser.selected_origin_is_current(Some(&origin)));
    assert!(!browser.selected_origin_is_current(None));
}

#[test]
fn retired_icon_events_cannot_change_a_closed_or_replaced_picker() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let origin = browser.selected_navigation_origin().unwrap();
    browser.icon_picker = true;
    let old = browser.icon_ticket.clone();
    assert!(browser.icon_event_is_current(&old, Some(&origin)));
    browser.close_menus();
    assert!(!browser.icon_event_is_current(&old, Some(&origin)));
    browser.icon_picker = true;
    browser.icon_ticket = std::rc::Rc::new(());
    assert!(!browser.icon_event_is_current(&old, Some(&origin)));
    assert!(browser.icon_event_is_current(&browser.icon_ticket, Some(&origin)));
    let other = browser.layout.as_ref().unwrap().model().workspaces[0]
        .id
        .clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&other);
    assert!(!browser.icon_event_is_current(&browser.icon_ticket, Some(&origin)));
}

#[test]
fn pane_opening_origins_refuse_binding_removal_before_legacy_fallback() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let pane = browser
        .layout
        .as_ref()
        .unwrap()
        .model()
        .pane()
        .unwrap()
        .id
        .clone();
    let origin = browser.pane_navigation_origin(&pane).unwrap();
    assert!(browser.pane_origin_is_current(&pane, Some(&origin)));
    let (base, mut peer) = browser.layout.as_ref().unwrap().save_snapshot();
    peer.workspaces[1].profile_id = None;
    browser.apply(crate::library::Update {
        document: Some(
            base.with_model(&peer, std::path::Path::new("layouts.db"))
                .unwrap(),
        ),
        error: None,
    });
    // Without a pre-refresh capture the consumer would now discover Legacy.
    assert!(matches!(browser.pane_connection(&pane), Ok(None)));
    assert!(!browser.pane_origin_is_current(&pane, Some(&origin)));
    assert!(!browser.pane_origin_is_current(&pane, None));
    assert!(!browser.pane_origin_is_current("missing-pane", Some(&origin)));
}

#[test]
fn pane_opening_origins_keep_their_pane_across_selection_but_expire_on_reconnect() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    let (_replacement, replacement, replacement_server) = provider("a", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    replacement_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    let pane = spaces[0].tabs[0].tree.leaves()[0].id.clone();
    let origin = browser.pane_navigation_origin(&pane).unwrap();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    assert!(browser.pane_origin_is_current(&pane, Some(&origin)));
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    assert!(browser.pane_origin_is_current(&pane, Some(&origin)));
    browser.workspace_profiles.bind(&spaces[0], replacement);
    assert!(!browser.pane_origin_is_current(&pane, Some(&origin)));
}

#[test]
fn held_workspace_and_tab_choices_reject_same_id_binding_replacement() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    let origin = browser.navigation_origin(&spaces[0].id).unwrap();
    assert_eq!(selected(&browser), spaces[1].id);
    assert!(browser.select_workspace_origin(&origin));
    assert_eq!(selected(&browser), spaces[0].id);
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    assert!(browser.select_navigation(&origin, &spaces[0].tabs[0].id, None));
    let (base, mut peer) = browser.layout.as_ref().unwrap().save_snapshot();
    peer.workspaces[0].profile_id = None;
    browser.apply(crate::library::Update {
        document: Some(
            base.with_model(&peer, std::path::Path::new("layouts.db"))
                .unwrap(),
        ),
        error: None,
    });
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    assert!(!browser.select_workspace_origin(&origin));
    assert!(!browser.select_navigation(&origin, &spaces[0].tabs[0].id, None));
    assert_eq!(selected(&browser), spaces[1].id);
}

#[test]
fn retired_workspace_menu_callbacks_do_not_act_after_close_or_replacement() {
    let mut browser = Browser {
        workspace_menu: true,
        ..Browser::default()
    };
    let old = browser.workspace_menu_ticket.clone();
    assert!(browser.workspace_menu_is_current(&old));
    browser.close_menus();
    assert!(!browser.workspace_menu_is_current(&old));
    browser.workspace_menu = true;
    browser.workspace_menu_ticket = std::rc::Rc::new(());
    assert!(!browser.workspace_menu_is_current(&old));
    assert!(browser.workspace_menu_is_current(&browser.workspace_menu_ticket));
}

#[test]
fn delayed_retry_never_resets_a_different_or_recovered_connection() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    browser.workspace_profiles.retry(&spaces[0]);
    let origin = browser.selected_navigation_origin().unwrap();
    assert!(browser.profile_content_unavailable());
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    browser.reset_focus = false;
    assert!(!browser.retry_profile_origin(Some(&origin)));
    assert!(!browser.reset_focus);
    assert!(
        matches!(browser.workspace_profiles.route(&spaces[1]), Route::Bound(connection) if connection.connection_key() == b.connection_key())
    );
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    browser.workspace_profiles.bind(&spaces[0], a.clone());
    assert!(!browser.retry_profile_origin(Some(&origin)));
    assert!(
        matches!(browser.workspace_profiles.route(&spaces[0]), Route::Bound(connection) if connection.connection_key() == a.connection_key())
    );
    let ready_origin = browser.selected_navigation_origin().unwrap();
    assert!(!browser.retry_profile_origin(Some(&ready_origin)));
    assert!(!browser.reset_focus);
}

#[test]
fn original_unavailable_workspace_accepts_explicit_retry() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    // An unavailable roster may require retry even with a verified connection.
    browser.workspace_rosters.remove(&spaces[0].id);
    let origin = browser.selected_navigation_origin().unwrap();
    assert!(browser.profile_content_unavailable());
    assert!(browser.retry_profile_origin(Some(&origin)));
    assert!(browser.reset_focus);
    assert!(matches!(
        browser.workspace_profiles.route(&spaces[0]),
        Route::Unavailable
    ));
    assert!(
        matches!(browser.workspace_profiles.route(&spaces[1]), Route::Bound(connection) if connection.connection_key() == b.connection_key())
    );
    // A held callback cannot retry again once its connection epoch is retired.
    assert!(!browser.retry_profile_origin(Some(&origin)));
}

#[test]
fn close_actions_reject_same_id_profile_changes_after_library_refresh() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    for changed_profile in [Some(a.identity().profile_id.clone()), None] {
        let mut browser = browser(&a, &b);
        let origin = browser.selected_navigation_origin().unwrap();
        let model = browser.layout.as_ref().unwrap().model();
        let tab = model.tab().unwrap().id.clone();
        let pane = model.pane().unwrap().clone();
        assert!(browser.close_tab_for(&origin, &tab).is_some());
        assert!(browser.close_pane_for(&origin, &tab, &pane).is_some());
        let (base, mut peer) = browser.layout.as_ref().unwrap().save_snapshot();
        peer.workspaces[1].profile_id = changed_profile;
        browser.apply(crate::library::Update {
            document: Some(
                base.with_model(&peer, std::path::Path::new("layouts.db"))
                    .unwrap(),
            ),
            error: None,
        });
        let model = browser.layout.as_ref().unwrap().model();
        assert_eq!(model.tab().unwrap().id, tab);
        assert_eq!(model.pane(), Some(&pane));
        // The old ID-only guards would accept this unchanged tab and leaf.
        assert!(
            super::super::close_tab_proposal(
                browser.layout.as_ref().unwrap(),
                &model.selected,
                &tab
            )
            .is_some()
        );
        assert!(
            super::super::close_pane_proposal(
                browser.layout.as_ref().unwrap(),
                &model.selected,
                &tab,
                &pane
            )
            .is_some()
        );
        assert!(browser.close_tab_for(&origin, &tab).is_none());
        assert!(browser.close_pane_for(&origin, &tab, &pane).is_none());
        assert_eq!(browser.layout.as_ref().unwrap().model().pane(), Some(&pane));
        // Closing remains a local operation: even unavailable bound workspaces
        // may be closed once the user acts on their current displayed origin.
        let fresh = browser.selected_navigation_origin().unwrap();
        assert!(browser.close_tab_for(&fresh, &tab).is_some());
        assert!(browser.close_pane_for(&fresh, &tab, &pane).is_some());
    }
}

#[test]
fn close_actions_expire_on_selection_reconnect_and_library_replacement() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    let (_replacement, replacement, replacement_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    replacement_server.join().unwrap();
    assert_eq!(b.identity(), replacement.identity());
    assert!(b.connection_key() != replacement.connection_key());
    let mut browser = browser(&a, &b);
    let origin = browser.selected_navigation_origin().unwrap();
    let model = browser.layout.as_ref().unwrap().model();
    let spaces = model.workspaces.clone();
    let tab = model.tab().unwrap().id.clone();
    let pane = model.pane().unwrap().clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    assert!(browser.close_tab_for(&origin, &tab).is_none());
    assert!(browser.close_pane_for(&origin, &tab, &pane).is_none());
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    browser.workspace_profiles.bind(&spaces[1], replacement);
    assert!(browser.close_tab_for(&origin, &tab).is_none());
    assert!(browser.close_pane_for(&origin, &tab, &pane).is_none());
    let fresh = browser.selected_navigation_origin().unwrap();
    assert!(browser.close_tab_for(&fresh, &tab).is_some());
    assert!(browser.close_pane_for(&fresh, &tab, &pane).is_some());
    browser.directory = Some("/another-library".into());
    assert!(browser.close_tab_for(&fresh, &tab).is_none());
    assert!(browser.close_pane_for(&fresh, &tab, &pane).is_none());
    assert_eq!(browser.layout.as_ref().unwrap().model().pane(), Some(&pane));
}

#[test]
fn legacy_setup_consumes_peer_binding_updates_before_admission() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    // Exercise both directions with unchanged workspace/tab/pane IDs: an old
    // legacy request cannot open setup after binding, and an old bound request
    // cannot become authority for legacy setup after binding removal.
    for was_bound in [false, true] {
        let mut browser = browser(&a, &b);
        if !was_bound {
            browser.layout.as_mut().unwrap().model_mut().workspaces[1].profile_id = None;
        }
        // Start from a saved binding, rather than an unsaved local edit that
        // reconciliation should preserve against an unchanged peer document.
        let (base, saved) = browser.layout.as_ref().unwrap().save_snapshot();
        browser.layout = Some(WindowLayout::new(
            base.with_model(&saved, std::path::Path::new("layouts.db"))
                .unwrap(),
        ));
        assert_eq!(browser.legacy_setup_allowed(), !was_bound);
        let origin = browser.selected_navigation_origin().unwrap();
        let (base, mut peer) = browser.layout.as_ref().unwrap().save_snapshot();
        let tab = peer.tab().unwrap().id.clone();
        let pane = peer.pane().unwrap().clone();
        peer.workspaces[1].profile_id = (!was_bound).then(|| b.identity().profile_id.clone());
        let update = crate::library::Update {
            document: Some(
                base.with_model(&peer, std::path::Path::new("layouts.db"))
                    .unwrap(),
            ),
            error: None,
        };
        assert!(!browser.legacy_setup_after_update(Some(&origin), Some(update)));
        let model = browser.layout.as_ref().unwrap().model();
        assert_eq!(
            model.workspace().unwrap().profile_id,
            peer.workspaces[1].profile_id
        );
        assert_eq!(model.tab().unwrap().id, tab);
        assert_eq!(model.pane(), Some(&pane));
        assert_eq!(browser.legacy_setup_allowed(), was_bound);
    }
}

#[test]
fn legacy_setup_requires_the_original_selected_library_and_explicit_legacy_workspace() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    // Readiness and a retained verified connection do not authorize legacy setup.
    assert!(!browser.legacy_setup_allowed());
    let space = browser
        .layout
        .as_ref()
        .unwrap()
        .model()
        .workspace()
        .unwrap()
        .clone();
    browser.workspace_profiles.retry(&space);
    assert!(!browser.legacy_setup_allowed());
    for space in &mut browser.layout.as_mut().unwrap().model_mut().workspaces {
        space.profile_id = None;
    }
    let origin = browser.selected_navigation_origin().unwrap();
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    assert!(browser.legacy_setup_after_update(Some(&origin), None));
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    assert!(!browser.legacy_setup_after_update(Some(&origin), None));
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    browser.directory = Some("/another-library".into());
    assert!(!browser.legacy_setup_after_update(Some(&origin), None));
    assert!(browser.legacy_setup_allowed());
    assert!(!browser.legacy_setup_after_update(None, None));
    assert!(!Browser::default().legacy_setup_allowed());
}
