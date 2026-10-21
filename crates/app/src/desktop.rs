//! Native, read-only browsing of the shared workspace library.

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc};

use futures::{StreamExt, channel::mpsc};
use gpui::{
    AnyElement, App, Bounds, Context, KeyBinding, Menu, MenuItem, PathPromptOptions, SharedString,
    Task, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div, prelude::*, px, rgb,
    size,
};
use gpui_platform::application;
use tmux_workspaces_core::{Direction, Node};

use crate::library::{Library, Update, WindowLayout};

actions!(
    library_browser,
    [
        /// Close the library browser without changing saved layouts or sessions.
        Quit
    ]
);

/// Open the native workspace browser. No tmux connection or database writer is
/// started. An absent directory uses the shared library's normal path lookup.
pub fn run(directory: Option<PathBuf>) -> Result<(), String> {
    let failure = Rc::new(RefCell::new(None));
    let opened = Rc::clone(&failure);
    application().run(move |cx: &mut App| {
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
                let mut browser = Browser::default();
                browser.open(directory, cx);
                browser
            })
        }) {
            *opened.borrow_mut() = Some(format!("cannot open window: {error}"));
            cx.quit();
            return;
        }
        cx.activate(true);
    });
    failure.borrow_mut().take().map_or(Ok(()), Err)
}

#[derive(Default)]
struct Browser {
    library: Option<Library>,
    layout: Option<WindowLayout>,
    error: Option<String>,
    loading: bool,
    updates: Option<Task<()>>,
}

impl Browser {
    fn open(&mut self, directory: Option<PathBuf>, cx: &mut Context<Self>) {
        self.updates = None;
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
                        self.error =
                            Some("The library changed. Showing the latest saved layout.".into());
                    }
                    Ok(_) => {}
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

    fn tree(node: &Node, focus: &str, cx: &mut Context<Self>) -> AnyElement {
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
                    .gap_2()
                    .p_4()
                    .rounded_md()
                    .bg(rgb(0x171d28))
                    .border_1()
                    .border_color(rgb(if id == focus { 0x82aaff } else { 0x303b4d }))
                    .cursor_pointer()
                    .on_click(cx.listener(move |browser, _, _, cx| {
                        if let Some(layout) = &mut browser.layout {
                            layout.model_mut().choose_pane(&id);
                            cx.notify();
                        }
                    }))
                    .child(div().text_ellipsis().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x9aa9bf))
                            .child("Saved pane · not connected"),
                    )
                    .children(
                        leaf.cwd
                            .as_ref()
                            .map(|cwd| div().text_sm().text_ellipsis().child(cwd.clone())),
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
                .gap_2()
                .when(*direction == Direction::Below, |el| el.flex_col())
                .child(
                    div()
                        .flex_basis(px(0.0))
                        .flex_grow(*ratio as f32)
                        .min_w_0()
                        .min_h_0()
                        .child(Self::tree(first, focus, cx)),
                )
                .child(
                    div()
                        .flex_basis(px(0.0))
                        .flex_grow((1.0 - ratio) as f32)
                        .min_w_0()
                        .min_h_0()
                        .child(Self::tree(second, focus, cx)),
                )
                .into_any_element(),
        }
    }
}

impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut sidebar = div()
            .w(px(240.0))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .p_4()
            .gap_3()
            .bg(rgb(0x161e2b))
            .border_r_1()
            .border_color(rgb(0x303b4d))
            .child(div().text_lg().child("tmux-workspaces"))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9aa9bf))
                    .child("WORKSPACES"),
            );
        let mut spaces = div().id("workspaces").max_h(px(200.0)).overflow_y_scroll();
        let mut tabs = div().id("tabs").flex_1().min_h_0().overflow_y_scroll();
        let mut content = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .p_6()
            .gap_4();
        if let Some(layout) = &self.layout {
            let model = layout.model();
            for space in &model.workspaces {
                let id = space.id.clone();
                spaces = spaces.child(
                    div()
                        .id(SharedString::from(format!("workspace-{id}")))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .cursor_pointer()
                        .text_ellipsis()
                        .when(model.selected == id, |el| el.bg(rgb(0x2a3c58)))
                        .hover(|el| el.bg(rgb(0x25334a)))
                        .child(space.name.clone())
                        .on_click(cx.listener(move |browser, _, _, cx| {
                            if let Some(layout) = &mut browser.layout {
                                layout.model_mut().choose_workspace(&id);
                                cx.notify();
                            }
                        })),
                );
            }
            if let Some(space) = model.workspace() {
                for tab in &space.tabs {
                    let id = tab.id.clone();
                    tabs = tabs.child(
                        div()
                            .id(SharedString::from(format!("tab-{id}")))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .cursor_pointer()
                            .text_ellipsis()
                            .when(space.selected == id, |el| el.bg(rgb(0x2a3c58)))
                            .hover(|el| el.bg(rgb(0x25334a)))
                            .child(tab.name.clone())
                            .on_click(cx.listener(move |browser, _, _, cx| {
                                if let Some(layout) = &mut browser.layout {
                                    layout.model_mut().choose_tab(&id);
                                    cx.notify();
                                }
                            })),
                    );
                }
                content = content.child(
                    div()
                        .text_sm()
                        .text_color(rgb(0x9aa9bf))
                        .child(space.name.clone()),
                );
            }
            if let Some(tab) = model.tab() {
                content = content
                    .child(div().text_xl().child(tab.name.clone()))
                    .child(
                        div()
                            .text_sm()
                            .text_color(rgb(0x9aa9bf))
                            .child("Saved layout · terminals are not connected"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(Self::tree(&tab.tree, &tab.focus, cx)),
                    );
            } else {
                content = content.child("This workspace has no tabs.");
            }
        } else {
            content = content.child(div().text_xl().child(if self.loading { "Opening library…" } else { "Open a workspace library" }))
                .child("Choose a folder containing layouts.db to browse its saved workspaces and tabs.");
        }
        if let Some(error) = &self.error {
            content = content.child(
                div()
                    .text_sm()
                    .text_color(rgb(0xffb4a5))
                    .child(error.clone()),
            );
        }
        sidebar = sidebar
            .child(spaces)
            .child(div().text_xs().text_color(rgb(0x9aa9bf)).child("TABS"))
            .child(tabs)
            .child(
                div()
                    .id("open-library")
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(rgb(0x2a3c58))
                    .cursor_pointer()
                    .hover(|el| el.bg(rgb(0x344e73)))
                    .child("Open library…")
                    .on_click(cx.listener(|browser, _, _, cx| browser.pick_library(cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x9aa9bf))
                    .child("Read only · changes are not saved"),
            );
        div()
            .size_full()
            .flex()
            .bg(rgb(0x101620))
            .text_color(rgb(0xe2e9f3))
            .font_family(Arc::<str>::from(".SystemUIFont"))
            .child(sidebar)
            .child(content)
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
