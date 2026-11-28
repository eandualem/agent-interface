//! Native navigation around an isolated, explicitly loaded document revision.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use agent_interface_roster::views::{
    DocumentMetadata, DocumentRevision, ReportContent, ReportItem, ReportQuery,
};
use gpui::{AnyElement, Context, SharedString, Task, div, prelude::*, px};

use super::{ViewEvent, ViewPane, artifact_web::Surface};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

/// A revision as served: its ID and the digest of its stored HTML.
type Revision = (u64, String);

fn revision(document: &DocumentRevision) -> Revision {
    (
        document.metadata.id,
        document.metadata.content_sha256.clone(),
    )
}

#[derive(Default)]
pub(super) struct State {
    pub loaded: Option<DocumentRevision>,
    /// The page served for a revision: with views of Git diffs and code in
    /// place of its pointers, once Git has been read.
    served: Option<(Revision, Arc<str>)>,
    /// The work making a revision's page.
    serving: Option<Serving>,
    pub surface: Rc<RefCell<Surface>>,
    pub error: Option<String>,
}

/// The work making a revision's page. Dropping it stops the work, Git
/// included, at its next check.
struct Serving {
    revision: Revision,
    until: crate::code_figures::Until,
    _waiting: Task<()>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.until.cancel();
    }
}

impl State {
    pub fn clear(&mut self) {
        self.surface.borrow_mut().clear();
        self.loaded = None;
        self.served = None;
        self.serving = None;
        self.error = None;
    }

    /// Load `document`, and say whether its page has yet to be made. A
    /// changed revision drops the last one's page and its work at once; the
    /// same revision read again keeps them.
    fn load(&mut self, document: &DocumentRevision) -> bool {
        let key = revision(document);
        self.loaded = Some(document.clone());
        let known = |entry: Option<&Revision>| entry == Some(&key);
        if known(self.served.as_ref().map(|(served, _)| served))
            || known(self.serving.as_ref().map(|serving| &serving.revision))
        {
            return false;
        }
        if self.served.take().is_some() {
            self.surface.borrow_mut().clear();
        }
        self.serving = None;
        true
    }

    /// Serve `page` for `key`, if that revision is still the one loaded.
    fn finish(&mut self, key: Revision, page: Arc<str>) -> bool {
        if self.loaded.as_ref().map(revision) != Some(key.clone()) {
            return false;
        }
        self.surface.borrow_mut().clear();
        self.served = Some((key, page));
        true
    }

    /// The page to show: the one served for the revision loaded.
    fn page(&self) -> Option<&Arc<str>> {
        let (key, page) = self.served.as_ref()?;
        (self.loaded.as_ref().map(revision).as_ref() == Some(key)).then_some(page)
    }
}

/// How long a Document waits for Git before it is served as stored.
const GIT_WAIT: Duration = Duration::from_secs(8);

/// Load `document` and, when it is a revision not yet served, make its page
/// on a thread of its own once Git has been read for its pointers, or serve
/// it as stored after [`GIT_WAIT`].
pub(super) fn serve(pane: &mut ViewPane, document: &DocumentRevision, cx: &mut Context<ViewPane>) {
    if !pane.documents.load(document) {
        return;
    }
    let key = revision(document);
    let html = document.html.clone();
    // Workspace folders first, so many agents cannot use up the folders read.
    let mut folders = (pane.folders)(cx);
    folders.extend(pane.agents.folders());
    let (done, served) = futures::channel::oneshot::channel();
    let stored = html.clone();
    let until = crate::code_figures::Until::new(Instant::now() + GIT_WAIT);
    let worker = until.clone();
    // Without the thread the sender is dropped, and the page is served as stored.
    let _ = std::thread::Builder::new()
        .name("document code".into())
        .spawn(move || {
            let _ = done.send(crate::code_figures::transform(&stored, &folders, &worker));
        });
    let made = key.clone();
    let task = cx.spawn(async move |pane, cx| {
        let wait = std::pin::pin!(cx.background_executor().timer(GIT_WAIT));
        // A late answer is dropped; what it read stays cached for next time.
        let served: Arc<str> = match futures::future::select(served, wait).await {
            futures::future::Either::Left((Ok(Some(page)), _)) => page.into(),
            _ => html,
        };
        let _ = pane.update(cx, |pane, cx| {
            if pane.documents.finish(made, served) {
                cx.notify();
            }
        });
    });
    pane.documents.serving = Some(Serving {
        revision: key,
        until,
        _waiting: task,
    });
}

pub(super) fn selected(pane: &ViewPane) -> Option<&DocumentMetadata> {
    match &pane.reports.open.as_ref()?.content {
        ReportContent::Document(metadata) => Some(metadata),
        _ => None,
    }
}

fn open(pane: &mut ViewPane, metadata: DocumentMetadata, cx: &mut Context<ViewPane>) {
    let item = ReportItem {
        id: metadata.id,
        author: metadata.author().to_owned(),
        author_id: Some(metadata.author_id.clone()),
        registered: false,
        created_at: metadata.created_at.clone(),
        conversation_id: None,
        content: ReportContent::Document(metadata),
    };
    if let Some(connection) = &pane.connection {
        super::seen::mark(&item, connection.identity(), cx);
    }
    pane.documents.clear();
    pane.reports.open = Some(item);
    pane.refetch(false, cx);
}

/// An arrow of the reader's revision stepper, opening revision `to`; inert
/// at either end.
fn revision_step(
    metadata: &DocumentMetadata,
    id: &'static str,
    icon: &str,
    to: Option<u64>,
    cx: &mut Context<ViewPane>,
) -> gpui::Stateful<gpui::Div> {
    let step = ui::revision_step(id, icon, to.is_some());
    let Some(to) = to else {
        return step;
    };
    let mut revision = metadata.clone();
    revision.id = to;
    revision.supersedes_id = None;
    revision.superseded_by = None;
    step.on_click(cx.listener(move |pane, _, _, cx| open(pane, revision.clone(), cx)))
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
        .child(ui::revision_stepper(
            revision_step(
                metadata,
                "document-revision-older",
                "chevron-left",
                metadata.supersedes_id,
                cx,
            ),
            format!("rev #{id}"),
            metadata.superseded_by.is_none(),
            revision_step(
                metadata,
                "document-revision-newer",
                "chevron-right",
                metadata.superseded_by,
                cx,
            ),
        ))
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
                    // Read again from Git as well: commits may have arrived.
                    pane.documents.clear();
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
    } else if let Some(html) = pane.documents.page() {
        let surface = pane.documents.surface.clone();
        let html = html.clone();
        let theme = super::reading::document_script();
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
                        .paint(bounds, window, cx, &html, &theme, move || {
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
            // A loaded revision is being read from Git: a failed poll's stale
            // error is not about it.
            .child(match pane.documents.loaded {
                Some(_) => "Reading document…",
                None => pane.error.unwrap_or("Reading document…"),
            })
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
                "· {} · revision #{id}{}",
                super::reports::when(&metadata.created_at),
                metadata
                    .supersedes_id
                    .map(|older| format!(" · replaces #{older}"))
                    .unwrap_or_default()
            ))),
        )
        .children(newer)
        .child(div().flex_1().min_h_0().child(body));
    super::reports::reader_frame(pane, toolbar, content, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(id: u64, html: &str) -> DocumentRevision {
        DocumentRevision {
            metadata: DocumentMetadata {
                id,
                document_id: "a".repeat(32),
                author_id: "b".repeat(32),
                author_name: "worker".into(),
                agent_name: None,
                title: "Title".into(),
                created_at: "2026-11-19T00:00:00Z".into(),
                content_bytes: html.len(),
                content_sha256: format!("{id:0>64}"),
                supersedes_id: None,
                superseded_by: None,
            },
            html: html.into(),
        }
    }

    #[test]
    fn a_changed_document_never_shows_the_last_page() {
        let mut state = State::default();
        let (one, two) = (document(1, "<p>one</p>"), document(2, "<p>two</p>"));
        assert!(state.load(&one));
        assert!(state.finish(revision(&one), "first".into()));
        assert_eq!(state.page().map(|page| &**page), Some("first"));
        assert!(state.load(&two));
        assert_eq!(state.page(), None);
        // A late page for the first revision is not served for the second.
        assert!(!state.finish(revision(&one), "first".into()));
        assert_eq!(state.page(), None);
    }

    fn serving(document: &DocumentRevision) -> Serving {
        Serving {
            revision: revision(document),
            until: crate::code_figures::Until::new(Instant::now() + GIT_WAIT),
            _waiting: Task::ready(()),
        }
    }

    #[test]
    fn the_same_revision_read_again_keeps_its_work() {
        let mut state = State::default();
        let one = document(1, "<p>one</p>");
        assert!(state.load(&one));
        state.serving = Some(serving(&one));
        assert!(!state.load(&one));
        assert!(state.finish(revision(&one), "first".into()));
        assert!(!state.load(&one));
        assert_eq!(state.page().map(|page| &**page), Some("first"));
    }

    #[test]
    fn refresh_makes_the_same_revision_page_again() {
        let mut state = State::default();
        let one = document(1, "<p>one</p>");
        assert!(state.load(&one));
        assert!(state.finish(revision(&one), "first".into()));
        // A poll or a pane shown again keeps the page.
        assert!(!state.load(&one));
        // Refresh clears first, so the page is made again.
        state.clear();
        assert!(state.load(&one));
    }

    #[test]
    fn leaving_the_reader_stops_the_work_and_drops_its_page() {
        let mut state = State::default();
        let (one, two) = (document(1, "<p>one</p>"), document(2, "<p>two</p>"));
        assert!(state.load(&one));
        let work = serving(&one);
        let until = work.until.clone();
        state.serving = Some(work);
        // Another revision stops the last one's work.
        assert!(state.load(&two));
        assert!(until.passed());
        // Leaving stops the work, Git included (code_figures' tests cancel a
        // running transform).
        state.serving = Some(serving(&two));
        let until = state.serving.as_ref().unwrap().until.clone();
        assert!(!until.passed());
        state.clear();
        assert!(until.passed() && state.serving.is_none());
        assert!(!state.finish(revision(&one), "late".into()));
        assert_eq!(state.page(), None);
    }
}
