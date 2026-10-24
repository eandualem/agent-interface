//! Popover menus: tab options, the workspace menu and the icon picker.

use gpui::{Context, Div, SharedString, div, prelude::*, px};
use tmux_workspaces_core::Direction;

use super::{
    Browser, TabAction, components as ui,
    design::{colors, tokens},
    icons,
    shortcuts::{self, Command},
};

impl Browser {
    /// A tab's options, opened from its ⋯ or a right-click.
    pub(super) fn tab_options(&self, cx: &mut Context<Self>) -> Div {
        let mut menu = ui::menu();
        let bindings = shortcuts::current();
        let key = |command| bindings.chord(command).map(shortcuts::Chord::label);
        // Naming and pane actions, then reordering, then moving elsewhere.
        let mut items: Vec<Option<(String, TabAction, Option<String>)>> = vec![
            Some((
                "Rename tab".into(),
                TabAction::Rename,
                key(Command::RenameTab),
            )),
            Some((
                "Split right".into(),
                TabAction::Split(Direction::Right),
                key(Command::SplitRight),
            )),
            Some((
                "Split below".into(),
                TabAction::Split(Direction::Below),
                key(Command::SplitBelow),
            )),
            Some((
                "Attach session".into(),
                TabAction::Attach,
                key(Command::Attach),
            )),
        ];
        let mut destinations = Vec::new();
        if let (Some(tab_menu), Some(layout)) = (&self.tab_menu, &self.layout) {
            let model = layout.model();
            items.push(Some(if tab_menu.shell.is_some() {
                ("Return to shell".into(), TabAction::ReturnToShell, None)
            } else {
                ("Open terminal".into(), TabAction::OpenTerminal, None)
            }));
            if tab_menu.shell.is_some() {
                items.push(Some((
                    "Detach session".into(),
                    TabAction::Detach,
                    key(Command::Detach),
                )));
            }
            let focus = if model.focus {
                "Restore layout"
            } else {
                "Focus one pane"
            };
            items.push(Some((
                focus.into(),
                TabAction::Focus,
                key(Command::FocusPane),
            )));
            if let Some(space) = model.workspace()
                && let Some(index) = space.tabs.iter().position(|tab| tab.id == tab_menu.tab.id)
            {
                let up = index > 0;
                let down = index + 1 < space.tabs.len();
                if up || down {
                    items.push(None);
                }
                if up {
                    items.push(Some(("Move up".into(), TabAction::Move(-1), None)));
                }
                if down {
                    items.push(Some(("Move down".into(), TabAction::Move(1), None)));
                }
            }
            destinations = model
                .workspaces
                .iter()
                .filter(|space| space.id != tab_menu.workspace)
                .map(|space| (space.name.clone(), space.id.clone()))
                .collect();
        }
        for (index, item) in items.into_iter().enumerate() {
            let Some((label, action, key)) = item else {
                menu = menu.child(ui::menu_rule());
                continue;
            };
            menu = menu.child(
                ui::menu_item(
                    SharedString::from(format!("tab-action-{index}")),
                    None,
                    label,
                    key,
                )
                .on_click(cx.listener(move |browser, _, window, cx| {
                    browser.tab_action(action.clone(), window, cx)
                })),
            );
        }
        if !destinations.is_empty() {
            menu = menu
                .child(ui::menu_rule())
                .child(ui::menu_heading("MOVE TO"));
            for (name, id) in destinations {
                menu = menu.child(
                    ui::menu_item(
                        SharedString::from(format!("tab-move-{id}")),
                        None,
                        name,
                        None,
                    )
                    .on_click(cx.listener(move |browser, _, window, cx| {
                        browser.tab_action(TabAction::Transfer(id.clone()), window, cx)
                    })),
                );
            }
        }
        menu
    }

    /// Switch workspaces, then act on the current one.
    pub(super) fn workspace_options(&self, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut menu = ui::menu().child(ui::menu_heading("WORKSPACES"));
        let Some(model) = self.layout.as_ref().map(|layout| layout.model()) else {
            return menu;
        };
        let states = self.agent_states();
        for (index, space) in model.workspaces.iter().enumerate() {
            let id = space.id.clone();
            let current = model.selected == id;
            let needs_you = space
                .tabs
                .iter()
                .any(|tab| self.tab_needs_you(tab, states.as_ref()));
            let key = (index < 9).then(|| {
                if cfg!(target_os = "macos") {
                    format!("⌥⌘{}", index + 1)
                } else {
                    format!("Ctrl+Alt+{}", index + 1)
                }
            });
            menu = menu.child(
                ui::menu_item(
                    SharedString::from(format!("workspace-{id}")),
                    current.then(|| ui::icon("check", t.size.icon, c.accent)),
                    space.name.clone(),
                    key,
                )
                .text_color(if current { c.text } else { c.text_2 })
                .when(needs_you, |el| el.child(ui::dot(c.attention)))
                .on_click(cx.listener(move |browser, _, window, cx| {
                    if let Some(layout) = &mut browser.layout {
                        layout.model_mut().choose_workspace(&id);
                    }
                    browser.close_menus();
                    if let Some(focus) = &browser.navigation {
                        window.focus(focus, cx);
                    }
                    cx.notify();
                })),
            );
        }
        let Some(space) = model.workspace() else {
            return menu;
        };
        let blocked = if !space.tabs.is_empty() {
            Some("Only empty workspaces")
        } else if model.workspaces.len() == 1 {
            Some("The last workspace stays")
        } else {
            None
        };
        let expected = space.id.clone();
        let mark = |name: &str, color| Some(ui::icon(name, t.size.icon, color));
        let delete = ui::menu_item(
            "delete-workspace",
            mark(
                "x",
                if blocked.is_some() {
                    c.faint
                } else {
                    c.attention_text
                },
            ),
            "Delete",
            None,
        );
        let delete = match blocked {
            Some(reason) => delete
                .text_color(c.faint)
                .cursor_default()
                .child(ui::text(div().text_color(c.muted), &t.text.meta).child(reason)),
            None => delete.text_color(c.attention_text).on_click(cx.listener(
                move |browser, _, window, cx| {
                    browser.close_menus();
                    browser.delete_workspace(&expected, window, cx);
                },
            )),
        };
        menu.child(ui::menu_rule())
            .child(ui::menu_heading("THIS WORKSPACE"))
            .child(
                ui::menu_item(
                    "rename-workspace",
                    mark("pencil", c.muted),
                    "Rename",
                    Some(shortcuts::label(Command::RenameWorkspace)),
                )
                .on_click(cx.listener(|browser, _, window, cx| {
                    browser.close_menus();
                    browser.begin_name(false, window, cx)
                })),
            )
            .child(
                ui::menu_item(
                    "choose-icon",
                    mark("sparkles", c.muted),
                    "Choose icon…",
                    None,
                )
                .on_click(cx.listener(|browser, _, _, cx| {
                    browser.workspace_menu = false;
                    browser.icon_picker = true;
                    cx.notify();
                })),
            )
            .child(delete)
            .child(ui::menu_rule())
            .child(
                ui::menu_item(
                    "new-workspace",
                    mark("plus", c.muted),
                    "New workspace",
                    Some(shortcuts::label(Command::NewWorkspace)),
                )
                .on_click(cx.listener(|browser, _, window, cx| {
                    browser.close_menus();
                    browser.begin_workspace(window, cx)
                })),
            )
    }

    /// A grid of icons for the current workspace, and the plain number.
    pub(super) fn icon_picker(&self, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let current = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().workspace())
            .and_then(|space| icons::workspace(space.symbol.as_deref(), space.icon.as_deref()));
        let columns = 8.0;
        let mut grid = div()
            .flex()
            .flex_wrap()
            .gap(px(t.space.s2))
            .w(px(columns * (t.size.slot + t.space.s2) + t.space.s12))
            .px(px(t.space.s6))
            .pt(px(t.space.s4))
            .pb(px(t.space.s6));
        for name in icons::WORKSPACE {
            let chosen = current == Some(*name);
            grid = grid.child(
                div()
                    .id(SharedString::from(format!("icon-{name}")))
                    .size(px(t.size.slot))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(t.radius.row))
                    .cursor_pointer()
                    .when(chosen, |el| el.bg(c.accent_soft))
                    .when(!chosen, |el| el.hover(|el| el.bg(colors().selected)))
                    .child(ui::icon(
                        name,
                        t.size.icon_large,
                        if chosen { c.accent } else { c.text_2 },
                    ))
                    .on_click(
                        cx.listener(move |browser, _, _, cx| browser.set_symbol(Some(name), cx)),
                    ),
            );
        }
        let number = ui::text(
            div().text_color(c.muted).font_family(t.font.mono.clone()),
            &t.text.key,
        )
        .child("1");
        ui::menu()
            .child(ui::menu_heading("CHOOSE ICON"))
            .child(grid)
            .child(ui::menu_rule())
            .child(
                ui::menu_item("icon-number", None, "Show number instead", None)
                    .child(number)
                    .on_click(cx.listener(|browser, _, _, cx| browser.set_symbol(None, cx))),
            )
    }
}
