//! Native navigation around an isolated, explicitly loaded document revision.

use std::{cell::RefCell, rc::Rc};

use agent_interface_roster::views::{
    DocumentMetadata, DocumentRevision, ReportContent, ReportItem, ReportQuery,
};
use gpui::{AnyElement, Context, SharedString, div, prelude::*, px};

use super::{
    ViewEvent, ViewPane,
    artifact_web::{self, Surface},
};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

#[derive(Default)]
pub(super) struct State {
    pub loaded: Option<DocumentRevision>,
    pub surface: Rc<RefCell<Surface>>,
    pub error: Option<String>,
}

impl State {
    pub fn clear(&mut self) {
        self.surface.borrow_mut().clear();
        self.loaded = None;
        self.error = None;
    }
}

pub(super) fn selected(pane: &ViewPane) -> Option<&DocumentMetadata> {
    match &pane.reports.open.as_ref()?.content {
        ReportContent::Document(metadata) => Some(metadata),
        _ => None,
    }
}

fn open(pane: &mut ViewPane, metadata: DocumentMetadata, cx: &mut Context<ViewPane>) {
    let provider_url = pane
        .reports
        .open
        .as_ref()
        .map(|item| item.provider_url.clone())
        .unwrap_or_default();
    let item = ReportItem {
        id: metadata.id,
        author: metadata.author().to_owned(),
        author_id: Some(metadata.author_id.clone()),
        provider_url,
        created_at: metadata.created_at.clone(),
        content: ReportContent::Document(metadata),
    };
    super::seen::mark(&item);
    pane.documents.clear();
    pane.reports.open = Some(item);
    pane.refetch(false, cx);
}

pub(super) fn reader(pane: &ViewPane, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let selected_meta = selected(pane).expect("selected document reader");
    let metadata = pane
        .documents
        .loaded
        .as_ref()
        .map(|doc| &doc.metadata)
        .unwrap_or(selected_meta);
    let id = metadata.id;
    let author_id = metadata.author_id.clone();
    let toolbar = ui::view_toolbar()
        .child(
            ui::back_button("documents-back", "Documents").on_click(cx.listener(
                |pane, _, _, cx| {
                    pane.documents.clear();
                    pane.reports.open = None;
                    pane.refetch(true, cx);
                },
            )),
        )
        .child(
            ui::text(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(c.text),
                &t.text.row_selected,
            )
            .child(metadata.title.clone()),
        )
        .child(div().flex_1())
        .child(
            ui::chip(
                "document-author",
                format!("More from {}", metadata.author()),
                false,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.documents.clear();
                pane.reports.open = None;
                pane.reports.agent = None;
                pane.reports.query = ReportQuery::Documents(None, Some(author_id.clone()));
                pane.reports.previous.clear();
                pane.refetch(true, cx);
            })),
        )
        .child(
            ui::icon_button("document-refresh", "refresh-cw").on_click(cx.listener(
                |pane, _, _, cx| {
                    pane.documents.error = None;
                    pane.refetch(false, cx);
                },
            )),
        );
    let newer = metadata.superseded_by.map(|next| {
        let mut newer = metadata.clone();
        newer.id = next;
        newer.superseded_by = None;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s8))
            .h(px(t.size.view_toolbar))
            .pl(px(t.space.s16))
            .pr(px(t.space.s10))
            .bg(c.accent_soft)
            .border_b(px(t.size.hairline))
            .border_color(c.line)
            .child(ui::icon("history", t.size.icon, c.accent_text))
            .child(
                ui::text(div().flex_1().min_w_0().text_color(c.text), &t.text.body)
                    .child("You're reading an earlier revision. A newer one is published."),
            )
            .child(ui::link(
                div()
                    .id("document-newer")
                    .px(px(t.space.s8))
                    .child("Open newer revision →")
                    .on_click(cx.listener(move |pane, _, _, cx| {
                        open(pane, newer.clone(), cx);
                    })),
            ))
    });
    let body = if let Some(error) = &pane.documents.error {
        div()
            .p(px(t.space.s16))
            .text_color(c.attention_text)
            .child(error.clone())
            .into_any_element()
    } else if let Some(document) = &pane.documents.loaded {
        let surface = pane.documents.surface.clone();
        let html = document.html.clone();
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
                            let _ = focus_entity.update(&mut *async_cx.borrow_mut(), |_, cx| {
                                cx.emit(ViewEvent::Focus)
                            });
                        })
                {
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |pane, cx| {
                            if selected(pane).is_some_and(|metadata| metadata.id == id) {
                                pane.documents.surface.borrow_mut().clear();
                                pane.documents.error = Some(error);
                                cx.notify();
                            }
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
            .child(pane.error.unwrap_or("Reading document…"))
            .into_any_element()
    };
    let content = div()
        .size_full()
        .flex()
        .flex_col()
        .child(
            ui::text(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(t.space.s6))
                    .h(px(t.size.pane_header))
                    .px(px(t.space.s16))
                    .border_b(px(t.size.hairline))
                    .border_color(c.line)
                    .text_color(c.muted),
                &t.text.meta,
            )
            .child(super::mono(metadata.author().to_owned()).text_color(c.text_2))
            .child(SharedString::from(format!(
                "· {} · revision #{id}",
                metadata.created_at
            ))),
        )
        .children(newer)
        .child(div().flex_1().min_h_0().child(body));
    super::reports::reader_frame(pane, toolbar, content)
}
