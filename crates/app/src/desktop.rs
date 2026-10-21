//! Native workspace layout and existing tmux session connections.

use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc, sync::Arc};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    AnyElement, App, Bounds, Context, KeyBinding, Menu, MenuItem, PathPromptOptions, SharedString,
    Task, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
    size,
};
use gpui_platform::application;
use tmux_workspaces_core::{Direction, Node};

use crate::library::{Library, Update, WindowLayout};
mod terminal;
use terminal::{LiveTerminal, Target};

actions!(
    library_browser,
    [
        /// Close the library browser without changing saved layouts or sessions.
        Quit
    ]
);

/// Open the native workspace window and connect its saved, existing sessions.
/// An absent directory uses the shared library's normal path lookup.
pub fn run(directory: Option<PathBuf>) -> Result<(), String> {
    let source = crate::sessions::source_socket(None).map_err(|e| e.to_string());
    let failure = Rc::new(RefCell::new(None));
    let opened = Rc::clone(&failure);
    application().run(move |cx: &mut App| {
        if cfg!(target_os = "linux") && cx.graphical_environment().is_none() {
            *opened.borrow_mut() =
                Some("the workspace browser requires a graphical desktop".into());
            // Linux's event loop resets its stop flag when run() begins, after
            // this launch callback. Request exit on its first dispatched task.
            cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
            return;
        }
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: "tmux-workspaces".into(),
            items: vec![MenuItem::action("Quit tmux-workspaces", Quit)],
            disabled: false,
        }]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1040.0), px(700.0)),
                cx,
            ))),
            window_min_size: Some(size(px(640.0), px(420.0))),
            titlebar: Some(TitlebarOptions {
                title: Some("tmux-workspaces".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        if let Err(error) = cx.open_window(options, |_, cx| {
            cx.new(|cx| {
                let mut browser = Browser {
                    source: Some(source),
                    navigation: Some(cx.focus_handle()),
                    ..Browser::default()
                };
                browser.open(directory, cx);
                browser
            })
        }) {
            *opened.borrow_mut() = Some(format!("cannot open window: {error}"));
            cx.spawn(async |cx| cx.update(|cx| cx.quit())).detach();
            return;
        }
        cx.activate(true);
    });
    failure.borrow_mut().take().map_or(Ok(()), Err)
}

#[derive(Default)]
struct Browser {
    library: Option<Library>,
    directory: Option<PathBuf>,
    source: Option<Result<PathBuf, String>>,
    terminals: HashMap<String, gpui::Entity<LiveTerminal>>,
    layout: Option<WindowLayout>,
    error: Option<String>,
    loading: bool,
    workspace_menu: bool,
    reset_focus: bool,
    navigation: Option<gpui::FocusHandle>,
    updates: Option<Task<()>>,
}

impl Browser {
    fn open(&mut self, directory: Option<PathBuf>, cx: &mut Context<Self>) {
        self.updates = None;
        self.terminals.clear();
        self.reset_focus = true;
        self.directory = directory.clone();
        // A previous connection may be finishing a bounded SQLite read. Join its
        // worker off the UI thread so opening another library stays responsive.
        if let Some(previous) = self.library.take() {
            cx.background_executor()
                .spawn(async move { drop(previous) })
                .detach();
        }
        self.layout = None;
        self.error = None;
        self.loading = true;
        let (wake, mut woken) = mpsc::unbounded();
        match Library::start(directory, move || {
            let _ = wake.unbounded_send(());
        }) {
            Ok(library) => self.library = Some(library),
            Err(error) => {
                self.error = Some(error.to_string());
                self.loading = false;
            }
        }
        self.updates = Some(cx.spawn(async move |this, cx| {
            while woken.next().await.is_some() {
                if this
                    .update(cx, |browser, cx| {
                        if let Some(update) =
                            browser.library.as_ref().and_then(Library::take_update)
                        {
                            browser.apply(update);
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn apply(&mut self, update: Update) {
        self.loading = false;
        self.error = update.error.map(|error| error.to_string());
        if let Some(document) = update.document {
            if let Some(layout) = &mut self.layout {
                match layout.refresh(document, "layouts.db".as_ref()) {
                    Ok(result) if result.conflict => {
                        self.reset_focus = true;
                        self.error =
                            Some("The library changed. Showing the latest saved layout.".into());
                    }
                    Ok(result) => {
                        self.reset_focus |= result.active_tree_changed;
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            } else {
                self.layout = Some(WindowLayout::new(document));
            }
        }
    }

    fn pick_library(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open library".into()),
        });
        cx.spawn(async move |this, cx| match paths.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |browser, cx| browser.open(Some(path), cx));
                }
            }
            Ok(Err(error)) => {
                let _ = this.update(cx, |browser, cx| {
                    browser.error = Some(error.to_string());
                    cx.notify();
                });
            }
            _ => {}
        })
        .detach();
    }

    fn tree(
        node: &Node,
        focus: &str,
        terminals: &HashMap<String, gpui::Entity<LiveTerminal>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            Node::Leaf(leaf) => {
                let id = leaf.id.clone();
                let title = leaf.attachment.as_ref().map_or_else(
                    || {
                        if leaf.empty {
                            "Empty pane".to_owned()
                        } else {
                            "Terminal".to_owned()
                        }
                    },
                    |attachment| attachment.session.clone(),
                );
                div()
                    .id(SharedString::from(format!("pane-{id}")))
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap_0()
                    .bg(rgb(0x171d28))
                    .border_1()
                    .border_color(rgb(if id == focus { 0x627a9c } else { 0x303b4d }))
                    .cursor_pointer()
                    .on_click(cx.listener(move |browser, _, _, cx| {
                        if let Some(layout) = &mut browser.layout {
                            layout.model_mut().choose_pane(&id);
                            cx.notify();
                        }
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .px_1()
                            .child(title)
                            .child(
                                div()
                                    .id(SharedString::from(format!("zoom-{}", leaf.id)))
                                    .cursor_pointer()
                                    .child("⤢")
                                    .on_click(cx.listener({
                                        let id = leaf.id.clone();
                                        move |browser, _, _, cx| {
                                            if let Some(layout) = &mut browser.layout {
                                                let model = layout.model_mut();
                                                model.choose_pane(&id);
                                                model.focus = !model.focus;
                                                cx.notify();
                                            }
                                        }
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .children(terminals.get(&leaf.id).cloned())
                            .when(leaf.empty, |el| el.child("Empty pane")),
                    )
                    .into_any_element()
            }
            Node::Split {
                direction,
                ratio,
                first,
                second,
                ..
            } => div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .flex()
                .gap_0()
                .when(*direction == Direction::Below, |el| el.flex_col())
                .child(
                    div()
                        .flex_basis(px(0.0))
                        .flex_grow(*ratio as f32)
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(Self::tree(first, focus, terminals, cx)),
                )
                .child(
                    div()
                        .flex_basis(px(0.0))
                        .flex_grow((1.0 - ratio) as f32)
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(Self::tree(second, focus, terminals, cx)),
                )
                .into_any_element(),
        }
    }
}

impl Render for Browser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.reset_focus)
            && let Some(focus) = &self.navigation
        {
            window.focus(focus, cx);
        }
        // A new entity owns each connection generation. Results from a removed
        // leaf cannot install a view or accept keys in its replacement.
        let leaves = self
            .layout
            .as_ref()
            .and_then(|layout| layout.model().tab())
            .map(|tab| {
                tab.tree
                    .leaves()
                    .into_iter()
                    .filter(|leaf| !leaf.empty)
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        self.terminals
            .retain(|id, _| leaves.iter().any(|leaf| &leaf.id == id));
        for leaf in leaves {
            let target = Target {
                leaf: leaf.clone(),
                directory: self.directory.clone(),
                source: self
                    .source
                    .clone()
                    .unwrap_or_else(|| Err("Source socket unavailable".into())),
            };
            if !self
                .terminals
                .get(&leaf.id)
                .is_some_and(|view| view.read(cx).target == target)
            {
                self.terminals
                    .insert(leaf.id, cx.new(|cx| LiveTerminal::new(target, cx)));
            }
        }
        let mut tabs = div().id("tabs").flex_1().min_h_0().overflow_y_scroll();
        let mut heading = "Workspaces".to_owned();
        let mut status = "Opening library…".to_owned();
        let mut content = div().flex_1().min_w_0().h_full().overflow_hidden();
        if let Some(layout) = &self.layout {
            let model = layout.model();
            if let Some(space) = model.workspace() {
                heading = space.name.clone();
                if self.workspace_menu {
                    tabs = tabs.child(div().px_2().py_1().text_xs().child("WORKSPACES"));
                    for space in &model.workspaces {
                        let id = space.id.clone();
                        tabs = tabs.child(
                            div()
                                .id(SharedString::from(format!("workspace-{id}")))
                                .px_2()
                                .py_1()
                                .text_ellipsis()
                                .cursor_pointer()
                                .when(model.selected == id, |el| el.bg(rgb(0x2a3c58)))
                                .hover(|el| el.bg(rgb(0x25334a)))
                                .child(space.name.clone())
                                .on_click(cx.listener(move |browser, _, window, cx| {
                                    if let Some(layout) = &mut browser.layout {
                                        layout.model_mut().choose_workspace(&id);
                                    }
                                    browser.workspace_menu = false;
                                    if let Some(focus) = &browser.navigation {
                                        window.focus(focus, cx);
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                } else {
                    tabs = tabs.child(
                        div()
                            .px_2()
                            .py_1()
                            .text_xs()
                            .text_color(rgb(0x9aa9bf))
                            .child("TABS"),
                    );
                    for (index, tab) in space.tabs.iter().enumerate() {
                        let id = tab.id.clone();
                        tabs = tabs.child(
                            div()
                                .id(SharedString::from(format!("tab-{id}")))
                                .px_2()
                                .py_1()
                                .text_ellipsis()
                                .cursor_pointer()
                                .when(space.selected == id, |el| el.bg(rgb(0x2a3c58)))
                                .hover(|el| el.bg(rgb(0x25334a)))
                                .child(format!(
                                    "{}  {}  {}",
                                    index + 1,
                                    tab.name,
                                    "▮".repeat(tab.tree.leaves().len())
                                ))
                                .on_click(cx.listener(move |browser, _, window, cx| {
                                    if let Some(layout) = &mut browser.layout {
                                        layout.model_mut().choose_tab(&id);
                                    }
                                    if let Some(focus) = &browser.navigation {
                                        window.focus(focus, cx);
                                    }
                                    cx.notify();
                                })),
                        );
                    }
                }
                status = format!("{} · no tabs", space.name);
            }
            if let Some(tab) = model.tab() {
                let leaves = tab.tree.leaves();
                let selected = leaves
                    .iter()
                    .position(|leaf| leaf.id == tab.focus)
                    .unwrap_or(0);
                status = format!(
                    "{} · {} · pane {}/{}",
                    heading,
                    tab.name,
                    selected + 1,
                    leaves.len()
                );
                let tree = if model.focus {
                    leaves.get(selected).map(|leaf| Node::Leaf((*leaf).clone()))
                } else {
                    None
                };
                content = content.child(Self::tree(
                    tree.as_ref().unwrap_or(&tab.tree),
                    &tab.focus,
                    &self.terminals,
                    cx,
                ));
            }
        } else if !self.loading {
            content = content.child(
                div()
                    .p_4()
                    .child("Open a folder containing layouts.db to use your saved workspaces."),
            );
            status = "No library open".into();
        }
        if let Some(error) = &self.error {
            status = error.clone();
        }
        let sidebar = div()
            .w(px(210.0))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(0x161e2b))
            .border_r_1()
            .border_color(rgb(0x303b4d))
            .child(
                div()
                    .id("workspace-menu")
                    .px_2()
                    .py_2()
                    .text_ellipsis()
                    .cursor_pointer()
                    .bg(rgb(0x25334a))
                    .child(format!("{heading}  ▾"))
                    .on_click(cx.listener(|browser, _, window, cx| {
                        browser.workspace_menu = !browser.workspace_menu;
                        if let Some(focus) = &browser.navigation {
                            window.focus(focus, cx);
                        }
                        cx.notify();
                    })),
            )
            .child(tabs)
            .child(
                div()
                    .id("open-library")
                    .px_2()
                    .py_2()
                    .text_sm()
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x25334a)))
                    .child("Open library…")
                    .on_click(cx.listener(|browser, _, _, cx| browser.pick_library(cx))),
            );
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x101620))
            .text_color(rgb(0xe2e9f3))
            .font_family(Arc::<str>::from(".SystemUIFont"))
            .when_some(self.navigation.clone(), |el, focus| el.track_focus(&focus))
            .capture_key_down(
                cx.listener(|browser, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.platform
                        && let Ok(number @ 1..=9) = event.keystroke.key.parse::<usize>()
                        && let Some(layout) = &mut browser.layout
                    {
                        let model = layout.model_mut();
                        if event.keystroke.modifiers.alt {
                            if let Some(id) = model
                                .workspaces
                                .get(number - 1)
                                .map(|space| space.id.clone())
                            {
                                model.choose_workspace(&id);
                            }
                        } else if let Some(id) = model
                            .workspace()
                            .and_then(|space| space.tabs.get(number - 1))
                            .map(|tab| tab.id.clone())
                        {
                            model.choose_tab(&id);
                        }
                        if let Some(focus) = &browser.navigation {
                            window.focus(focus, cx);
                        }
                        browser.workspace_menu = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .child(sidebar)
                    .child(content),
            )
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(0x9aa9bf))
                    .bg(rgb(0x161e2b))
                    .child(format!(
                        "{status}    ⌘1–9 tabs · ⌘⌥1–9 workspaces · click terminal to type"
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tmux_workspaces_store::{Document, Error};

    #[test]
    fn refresh_keeps_local_navigation_and_read_errors_keep_the_last_good_layout() {
        let path = std::path::Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"First","tree":{"id":"000000000003","agent":null},"focus":"000000000003"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let base = Document::parse(raw.into(), path).unwrap();
        let mut saved = base.model().clone();
        saved.add_tab(Some("Second".into()), "/tmp".into(), true);
        let second = saved.tab().unwrap().id.clone();
        saved.choose_tab("000000000002");
        let mut browser = Browser::default();
        let mut other = Browser::default();
        for window in [&mut browser, &mut other] {
            window.apply(Update {
                document: Some(base.with_model(&saved, path).unwrap()),
                error: None,
            });
        }
        browser
            .layout
            .as_mut()
            .unwrap()
            .model_mut()
            .choose_tab(&second);
        saved.rename_workspace("Updated workspace");
        browser.apply(Update {
            document: Some(base.with_model(&saved, path).unwrap()),
            error: None,
        });
        let model = browser.layout.as_ref().unwrap().model();
        assert_eq!(model.workspace().unwrap().name, "Updated workspace");
        assert_eq!(model.tab().unwrap().id, second);
        assert_eq!(
            other.layout.as_ref().unwrap().model().tab().unwrap().id,
            "000000000002"
        );

        let before = model.clone();
        browser.apply(Update {
            document: None,
            error: Some(Error {
                path: path.into(),
                detail: "fixture read failure".into(),
            }),
        });
        assert_eq!(browser.layout.as_ref().unwrap().model(), &before);
        assert!(
            browser
                .error
                .as_ref()
                .unwrap()
                .contains("fixture read failure")
        );
        browser.apply(Update {
            document: Some(base.with_model(&saved, path).unwrap()),
            error: None,
        });
        assert!(browser.error.is_none());
        assert_eq!(
            browser.layout.as_ref().unwrap().model().tab().unwrap().id,
            second
        );
    }
}
