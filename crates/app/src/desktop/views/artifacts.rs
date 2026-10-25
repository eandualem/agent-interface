//! Artifact discovery and document lifecycle, separate from Backbone polling.

use super::artifact_web::{self, Surface};
use crate::{
    artifacts::{self, Catalog, Entry},
    desktop::{
        components as ui,
        design::{colors, tokens},
    },
};
use gpui::{
    AnyElement, Context, Entity, EventEmitter, SharedString, Task, Window, div, prelude::*, px,
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::Duration,
};

pub(super) struct Focus;
impl EventEmitter<Focus> for ArtifactPane {}

pub(super) struct ArtifactPane {
    directory: Option<PathBuf>,
    catalog: Catalog,
    catalog_error: Option<String>,
    error: Option<String>,
    selected: Option<Entry>,
    html: Option<Arc<str>>,
    visible: Rc<Cell<bool>>,
    surface: Rc<RefCell<Surface>>,
    _poll: Task<()>,
}

impl ArtifactPane {
    pub fn new(directory: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let visible = Rc::new(Cell::new(true));
        let active = visible.clone();
        let source = directory.clone();
        let poll = cx.spawn(async move |this, cx| {
            loop {
                if active.get() {
                    let source = source.clone();
                    let catalog = cx
                        .background_executor()
                        .spawn(async move { artifacts::catalog(source.as_deref()) })
                        .await;
                    if this
                        .update(cx, |pane, cx| {
                            match catalog {
                                Ok(catalog) => {
                                    pane.catalog = catalog;
                                    pane.catalog_error = None;
                                }
                                Err(error) => pane.catalog_error = Some(error.to_string()),
                            }
                            if pane.selected.is_none() {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        });
        Self {
            directory,
            catalog: Catalog::default(),
            catalog_error: None,
            error: None,
            selected: None,
            html: None,
            visible,
            surface: Rc::new(RefCell::new(Surface::default())),
            _poll: poll,
        }
    }

    pub fn set_visible(&self, visible: bool) {
        self.visible.set(visible);
        if !visible {
            self.surface.borrow_mut().clear();
        }
    }

    pub fn occlude(&self, occluded: bool) {
        self.surface.borrow_mut().occlude(occluded);
    }

    pub fn focus_parent(&self) {
        self.surface.borrow().focus_parent();
    }

    fn open(&mut self, entry: Entry, cx: &mut Context<Self>) {
        self.surface.borrow_mut().clear();
        self.selected = Some(entry.clone());
        self.html = None;
        self.error = None;
        let directory = self.directory.clone();
        let id = entry.id;
        let read_id = id.clone();
        let read = cx
            .background_executor()
            .spawn(async move { artifacts::read(directory.as_deref(), &read_id) });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |pane, cx| {
                if pane.selected.as_ref().is_some_and(|entry| entry.id == id) {
                    match result {
                        Ok(html) => pane.html = Some(Arc::from(html)),
                        Err(error) => pane.error = Some(error.to_string()),
                    }
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.surface.borrow_mut().clear();
        self.selected = None;
        self.html = None;
        self.error = None;
        cx.notify();
    }
}

impl Render for ArtifactPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let mut header = ui::view_header("layout-grid", "Artifacts");
        if let Some(entry) = &self.selected {
            header = header.child(div().flex_1()).child(
                ui::text(div().text_color(c.muted), &t.text.meta).child(entry.author.clone()),
            );
        }
        let content: AnyElement = if let Some(entry) = self.selected.clone() {
            let toolbar = ui::view_toolbar()
                .gap(px(t.space.s8))
                .child(
                    ui::back_button("artifacts-back", "Artifacts")
                        .on_click(cx.listener(|pane, _, _, cx| pane.close(cx))),
                )
                .child(super::mono(entry.title.clone()).flex_1().min_w_0())
                .child(
                    ui::chip("artifact-reload", "Reload", false)
                        .on_click(cx.listener(move |pane, _, _, cx| pane.open(entry.clone(), cx))),
                );
            let body = if let Some(error) = &self.error {
                ui::text(
                    div().p(px(t.space.s16)).text_color(c.attention_text),
                    &t.text.body,
                )
                .child(error.clone())
                .into_any_element()
            } else if let Some(html) = &self.html {
                let surface = self.surface.clone();
                let html = html.clone();
                let theme = artifact_web::theme();
                let entity = cx.entity().downgrade();
                let focus_entity = entity.clone();
                let async_cx = Rc::new(RefCell::new(cx.to_async()));
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, (), window, cx| {
                        let focus_entity = focus_entity.clone();
                        let async_cx = async_cx.clone();
                        if let Err(error) =
                            surface
                                .borrow_mut()
                                .paint(bounds, window, &html, &theme, move || {
                                    let _ = focus_entity
                                        .update(&mut *async_cx.borrow_mut(), |_, cx| {
                                            cx.emit(Focus)
                                        });
                                })
                        {
                            cx.defer(move |cx| {
                                let _ = entity.update(cx, |pane, cx| {
                                    pane.surface.borrow_mut().clear();
                                    pane.error = Some(error);
                                    cx.notify();
                                });
                            });
                        }
                    },
                )
                .size_full()
                .into_any_element()
            } else {
                div()
                    .p(px(t.space.s16))
                    .child("Reading artifact…")
                    .into_any_element()
            };
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(toolbar)
                .child(div().flex_1().min_h_0().child(body))
                .into_any_element()
        } else {
            let mut list = super::list("artifact-list").gap(px(t.space.s6));
            for entry in &self.catalog.entries {
                let entry = entry.clone();
                list = list.child(
                    div()
                        .id(SharedString::from(format!("artifact-{}", entry.id)))
                        .flex_shrink_0()
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s6))
                        .px(px(t.space.s12))
                        .py(px(t.space.s10))
                        .border(px(t.size.hairline))
                        .border_color(c.line)
                        .rounded(px(t.radius.slot))
                        .cursor_pointer()
                        .hover(|style| style.bg(c.hover))
                        .child(
                            ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                                "{} · {} ago",
                                entry.author,
                                super::ago(
                                    tmux_workspaces_roster::views::now() as f64
                                        - entry.created_ms as f64 / 1000.
                                )
                            )),
                        )
                        .child(
                            ui::text(div().text_color(c.text), &t.text.body)
                                .child(entry.title.clone()),
                        )
                        .on_click(cx.listener(move |pane, _, _, cx| pane.open(entry.clone(), cx))),
                );
            }
            if self.catalog.entries.is_empty() {
                list = list.child(ui::text(div().p(px(t.space.s16)).text_color(c.muted), &t.text.body)
                    .child("No artifacts yet. Agents can publish a self-contained HTML file with the app’s artifact publish command."));
            }
            if let Some(error) = &self.catalog_error {
                list = list.child(div().text_color(c.attention_text).child(error.clone()));
            }
            if self.catalog.unreadable > 0 {
                list = list.child(div().text_color(c.muted).child(format!(
                    "{} artifact records could not be read.",
                    self.catalog.unreadable
                )));
            }
            if self.catalog.more {
                list = list.child(
                    div()
                        .text_color(c.muted)
                        .child("Showing the newest 200 artifacts."),
                );
            }
            list.into_any_element()
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(content)
    }
}

pub(super) fn create(
    directory: Option<PathBuf>,
    cx: &mut Context<super::ViewPane>,
) -> Entity<ArtifactPane> {
    cx.new(|cx| ArtifactPane::new(directory, cx))
}
