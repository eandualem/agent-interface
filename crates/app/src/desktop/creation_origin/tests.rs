use super::*;
use crate::{
    library::{Update, WindowLayout},
    roster::content::tests::provider,
};
use agent_interface_core::{Direction, Layout};
use agent_interface_roster::profiles::Connection;
use agent_interface_store::Document;

fn browser(a: &Connection, b: &Connection) -> Browser {
    let path = std::path::Path::new("layouts.db");
    let base = Document::parse(r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[],"selected":""}],"selected":"000000000001","focus":false}"#.into(), path).unwrap();
    let mut model = Layout::initial("/".into());
    model.workspaces[0].profile_id = Some(a.identity().profile_id.clone());
    model.add_workspace("Same display name".into());
    model.add_tab(None, "/".into(), true);
    model.workspaces[1].profile_id = Some(b.identity().profile_id.clone());
    let first = model.workspaces[0].id.clone();
    model.choose_workspace(&first);
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
    browser
}

#[test]
fn held_creation_commands_refuse_a_later_selected_workspace_or_pane() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let target = browser.creation_target().unwrap();
    let commands: Vec<_> = browser
        .jump_hits()
        .into_iter()
        .filter_map(|hit| match hit.target {
            super::super::jump::Target::Command(_, Some(target)) => Some(target),
            _ => None,
        })
        .collect();
    assert_eq!(commands.len(), 3);
    let spaces = browser.layout.as_ref().unwrap().model().workspaces.clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[1].id);
    assert!(!browser.creation_is_current(&target));
    assert!(
        commands
            .iter()
            .all(|target| !browser.creation_is_current(target))
    );
    let fresh = browser.creation_target().unwrap();
    assert!(browser.creation_is_current(&fresh));
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&spaces[0].id);
    assert!(browser.creation_is_current(&target));
    assert!(
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .split(Direction::Right, "/".into(), true)
    );
    assert!(!browser.creation_is_current(&target));
    assert!(browser.pending_creation.is_none());
}

#[test]
fn pending_library_binding_change_invalidates_a_previously_valid_creation() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let target = browser.creation_target().unwrap();
    assert!(browser.creation_is_current(&target));
    let (base, mut peer) = browser.layout.as_ref().unwrap().save_snapshot();
    peer.workspaces[0].profile_id = Some(b.identity().profile_id.clone());
    let document = base
        .with_model(&peer, std::path::Path::new("layouts.db"))
        .unwrap();
    browser.apply(Update {
        document: Some(document),
        error: None,
    });
    // Same saved workspace/tab/pane IDs are not enough after the library update.
    assert!(!browser.creation_is_current(&target));
    assert!(browser.pending_creation.is_none());
}

#[test]
fn reconnect_reload_and_library_replacement_expire_creation_targets() {
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    let (_replacement, replacement, replacement_server) = provider("a", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    replacement_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let target = browser.creation_target().unwrap();
    let space = browser.layout.as_ref().unwrap().model().workspaces[0].clone();
    browser.workspace_profiles.retry(&space);
    assert!(!browser.creation_is_current(&target));
    let (base, model) = browser.layout.as_ref().unwrap().save_snapshot();
    let path = std::path::Path::new("layouts.db");
    let raw = base.with_model(&model, path).unwrap().raw_json().to_owned();
    browser.layout = Some(WindowLayout::new(Document::parse(raw, path).unwrap()));
    browser.workspace_profiles.bind(&space, replacement);
    assert!(!browser.creation_is_current(&target));
    let fresh = browser.creation_target().unwrap();
    assert!(browser.creation_is_current(&fresh));
    browser.directory = Some("/another-library".into());
    assert!(!browser.creation_is_current(&fresh));
}

#[test]
fn retired_menu_dispatch_cannot_consume_a_replacement_menus_creation_target() {
    fn menu(browser: &Browser) -> super::super::TabMenu {
        let model = browser.layout.as_ref().unwrap().model();
        super::super::TabMenu {
            origin: browser.selected_navigation_origin(),
            ticket: std::rc::Rc::new(()),
            workspace: model.selected.clone(),
            tab: model.tab().unwrap().clone(),
            shell: None,
            creation: browser.creation_target(),
        }
    }
    let (_a, a, a_server) = provider("a", vec![]);
    let (_b, b, b_server) = provider("b", vec![]);
    a_server.join().unwrap();
    b_server.join().unwrap();
    let mut browser = browser(&a, &b);
    let old_menu = menu(&browser);
    let old_ticket = old_menu.ticket.clone();
    browser.tab_menu = Some(old_menu);
    let other = browser.layout.as_ref().unwrap().model().workspaces[1]
        .id
        .clone();
    browser
        .layout
        .as_mut()
        .unwrap()
        .model_mut()
        .choose_workspace(&other);
    browser.tab_menu = Some(menu(&browser));
    let fresh_ticket = browser.tab_menu.as_ref().unwrap().ticket.clone();
    // This is the same consuming gate used before tab_action reads its target.
    assert!(browser.take_tab_menu(&old_ticket).is_none());
    assert!(browser.tab_menu.is_some());
    assert!(browser.pending_creation.is_none());
    let fresh = browser.take_tab_menu(&fresh_ticket).unwrap();
    assert!(browser.creation_is_current(fresh.creation.as_ref().unwrap()));
    assert_eq!(fresh.workspace, other);
    assert!(browser.tab_menu.is_none());
    // Close/reopen at an unchanged destination also expires the old listener.
    browser.tab_menu = Some(menu(&browser));
    assert!(browser.take_tab_menu(&fresh_ticket).is_none());
    assert!(browser.tab_menu.is_some());
}
