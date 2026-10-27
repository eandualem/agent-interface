//! Native read-only spreadsheets with contextual feedback. Agents own edits;
//! immutable displayed revisions and feedback retries remain pinned in this pane.

use super::feedback::{Feedback, FeedbackEvent};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};
use agent_interface_roster::{
    feedback::Outcome,
    sheets::{self as api, *},
};
use gpui::{
    Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*, px,
};
use std::path::PathBuf;
mod controls;
mod grid;

pub(super) struct Close;
impl EventEmitter<Close> for Sheets {}

struct Open {
    record: Record,
    grid: Grid,
    query: RangeQuery,
    pages: PageTrail,
}
#[derive(Clone, Default)]
struct PageTrail {
    rows: Vec<usize>,
    columns: Vec<usize>,
}
impl PageTrail {
    fn move_axis(trail: &mut Vec<usize>, from: usize, to: usize) {
        if from == to {
            return;
        }
        if let Some(index) = trail.iter().position(|offset| *offset == to) {
            trail.truncate(index);
        } else if to > from {
            trail.push(from);
        } else {
            trail.clear();
        }
    }
    fn moved(mut self, from: (usize, usize), to: (usize, usize)) -> Self {
        Self::move_axis(&mut self.rows, from.0, to.0);
        Self::move_axis(&mut self.columns, from.1, to.1);
        self
    }
}
#[derive(Clone, Copy)]
struct Selection {
    anchor: (usize, usize),
    end: (usize, usize),
}
impl Selection {
    fn bounds(
        self,
    ) -> (
        std::ops::RangeInclusive<usize>,
        std::ops::RangeInclusive<usize>,
    ) {
        (
            self.anchor.0.min(self.end.0)..=self.anchor.0.max(self.end.0),
            self.anchor.1.min(self.end.1)..=self.anchor.1.max(self.end.1),
        )
    }
    fn contains(self, r: usize, c: usize) -> bool {
        let (rows, cols) = self.bounds();
        rows.contains(&r) && cols.contains(&c)
    }
}

pub(super) struct Sheets {
    focus: gpui::FocusHandle,
    directory: PathBuf,
    client: Option<Client>,
    catalog: Vec<Record>,
    next: Option<String>,
    cursors: Vec<Option<String>>,
    cursor: Option<String>,
    open: Option<Open>,
    history: Option<Page>,
    history_cursors: Vec<Option<String>>,
    history_cursor: Option<String>,
    selection: Option<Selection>,
    feedback: Entity<Feedback>,
    export_open: bool,
    pending: Option<Prepared>,
    pending_uncertain: bool,
    feedback_receipt: Option<FeedbackReceipt>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    _input: Subscription,
}
impl Sheets {
    pub fn new(directory: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let feedback = cx.new(|cx| Feedback::for_sheet(window, cx));
        let input = cx.subscribe(&feedback, |s, _, event, cx| match event {
            FeedbackEvent::Send => s.send_feedback(cx),
            FeedbackEvent::Retry => {
                if let Some(request) = s.pending.clone() {
                    s.submit(request, cx);
                }
            }
        });
        let mut state = Self {
            focus: cx.focus_handle(),
            directory,
            client: None,
            catalog: Vec::new(),
            next: None,
            cursors: Vec::new(),
            cursor: None,
            open: None,
            history: None,
            history_cursors: Vec::new(),
            history_cursor: None,
            selection: None,
            feedback,
            export_open: false,
            pending: None,
            pending_uncertain: false,
            feedback_receipt: None,
            busy: false,
            error: None,
            notice: None,
            _input: input,
        };
        state.catalog(None, cx);
        state
    }
    fn key(&mut self, event: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        let command = if cfg!(target_os = "macos") {
            key.modifiers.platform
        } else {
            key.modifiers.control
        };
        if !self.focus.is_focused(window) {
            return;
        } else if command && key.key == "c" {
            self.copy(cx);
        } else if let Some((open, selection)) = self.open.as_ref().zip(self.selection) {
            let (mut row, mut col) = selection.end;
            match key.key.as_str() {
                "up" => row = row.saturating_sub(1),
                "down" => row = (row + 1).min(open.grid.rows.len().saturating_sub(1)),
                "left" => col = col.saturating_sub(1),
                "right" | "tab" if !key.modifiers.shift => {
                    col = (col + 1).min(open.grid.columns.len().saturating_sub(1))
                }
                "right" => col = (col + 1).min(open.grid.columns.len().saturating_sub(1)),
                "tab" => col = col.saturating_sub(1),
                _ => return,
            }
            self.select(row, col, key.modifiers.shift && key.key != "tab", cx);
        } else {
            return;
        }
        window.prevent_default();
        cx.stop_propagation();
    }
    fn clean(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.feedback.read(cx).value(cx).is_empty() {
            self.fail(
                "Send or clear the feedback draft before changing the workbook or revision.",
                cx,
            );
            return false;
        }
        !self.busy && self.pending.is_none()
    }
    fn fail(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }
    fn begin(&mut self, cx: &mut Context<Self>) {
        self.busy = true;
        self.error = None;
        self.notice = None;
        cx.notify();
    }
    fn catalog(&mut self, cursor: Option<String>, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        self.begin(cx);
        let directory = self.directory.clone();
        let client = self.client.clone();
        let requested = cursor.clone();
        let work = cx.background_executor().spawn(async move {
            let client = client
                .map(Ok)
                .unwrap_or_else(|| Client::connect(&directory))?;
            let page = client.list(None, requested.as_deref())?;
            Ok::<_, api::Error>((client, page))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok((client, page)) => {
                        s.client = Some(client);
                        s.catalog = page.items;
                        s.next = page.next_cursor;
                        commit_cursor(&mut s.cursors, &s.cursor, &cursor);
                        s.cursor = cursor;
                        s.open = None;
                        s.history = None;
                        s.history_cursors.clear();
                        s.history_cursor = None;
                    }
                    Err(e) => s.error = Some(e.message().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn open_record(
        &mut self,
        record: Record,
        tab: Option<Id>,
        row: usize,
        column: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.clean(cx) {
            return;
        }
        self.read_grid(record, tab, row, column, cx);
    }
    fn read_grid(
        &mut self,
        record: Record,
        tab: Option<Id>,
        row: usize,
        column: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Some(tab) = tab
            .filter(|id| record.tabs.iter().any(|t| &t.tab_id == id))
            .or_else(|| record.tabs.first().map(|t| t.tab_id.clone()))
        else {
            return;
        };
        let query = RangeQuery {
            sheet_id: record.sheet_id.clone(),
            revision_id: record.revision_id.clone(),
            tab_id: tab,
            row_offset: row,
            row_limit: 40,
            column_offset: column,
            column_limit: 12,
            order: Order::View,
        };
        let pages = self
            .open
            .as_ref()
            .filter(|old| {
                old.record.sheet_id == record.sheet_id
                    && old.record.revision_id == record.revision_id
                    && old.grid.tab_id == query.tab_id
            })
            .map(|old| {
                old.pages.clone().moved(
                    (old.query.row_offset, old.query.column_offset),
                    (row, column),
                )
            })
            .unwrap_or_default();
        self.begin(cx);
        let work = cx.background_executor().spawn(async move {
            let grid = client.range(&query)?;
            Ok::<_, api::Error>(Open {
                record,
                grid,
                query,
                pages,
            })
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(open) => {
                        s.open = Some(open);
                        s.selection = None;
                        s.history = None;
                        s.history_cursors.clear();
                        s.history_cursor = None;
                    }
                    Err(e) => s.error = Some(e.message().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn latest(&mut self, cx: &mut Context<Self>) {
        if !self.feedback.read(cx).value(cx).is_empty() {
            self.fail(
                "Send or clear the feedback draft before changing the revision.",
                cx,
            );
            return;
        }
        if self.busy || self.pending.is_some() {
            return;
        }
        let Some((client, open)) = self.client.clone().zip(self.open.as_ref()) else {
            return;
        };
        let sheet = open.record.sheet_id.clone();
        let tab = open.grid.tab_id.clone();
        let row = open.query.row_offset;
        let col = open.query.column_offset;
        self.begin(cx);
        let work = cx
            .background_executor()
            .spawn(async move { client.record(&sheet, None) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(record) => s.read_grid(record, Some(tab), row, col, cx),
                    Err(e) => s.fail(e.message(), cx),
                }
            });
        })
        .detach();
    }
    fn select(&mut self, row: usize, column: usize, extend: bool, cx: &mut Context<Self>) {
        if self.busy || self.pending.is_some() {
            return;
        }
        let Some(open) = &self.open else {
            return;
        };
        if row >= open.grid.rows.len() || column >= open.grid.columns.len() {
            return;
        }
        self.selection = Some(Selection {
            anchor: if extend {
                self.selection.map(|s| s.anchor).unwrap_or((row, column))
            } else {
                (row, column)
            },
            end: (row, column),
        });
        cx.notify();
    }
    fn submit(&mut self, request: Prepared, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        self.feedback.update(cx, |f, cx| f.retrying(cx));
        self.pending = Some(request.clone());
        self.begin(cx);
        let work = cx
            .background_executor()
            .spawn(async move { client.submit(&request) });
        cx.spawn(async move |this,cx| {
            let result = work.await;
            let _ = this.update(cx, |s,cx| {
                s.busy = false;
                match result {
                    Ok(Accepted::Feedback(receipt)) => {
                        s.pending = None; s.pending_uncertain = false;
                        s.feedback.update(cx, |f,cx| f.complete(Outcome::Accepted(receipt.delivery.message().into()), cx));
                        s.notice = Some(receipt.delivery.message().into());
                        s.feedback_receipt = Some(receipt);
                    }
                    Ok(_) => {
                        s.pending_uncertain = true;
                        s.feedback.update(cx, |f,cx| f.complete(Outcome::Unknown("Could not confirm feedback. Retry the same request to check its outcome.".into()), cx));
                    }
                    Err(error) => {
                        s.pending_uncertain = unresolved_mutation(s.pending_uncertain, &error);
                        if !s.pending_uncertain { s.pending = None; }
                        let outcome = if s.pending_uncertain { Outcome::Unknown(error.message().into()) } else { Outcome::Refused(error.message().into()) };
                        s.feedback.update(cx, |f,cx| f.complete(outcome, cx));
                    }
                }
                cx.notify();
            });
        }).detach();
    }
}
fn unresolved_mutation(previously_uncertain: bool, error: &api::Error) -> bool {
    previously_uncertain || *error == api::Error::Unknown
}
fn commit_cursor(trail: &mut Vec<Option<String>>, current: &Option<String>, next: &Option<String>) {
    if next.is_none() {
        trail.clear();
    } else if next != current {
        if let Some(index) = trail.iter().position(|cursor| cursor == next) {
            trail.truncate(index);
        } else {
            trail.push(current.clone());
        }
    }
}
fn new_id() -> Id {
    Id::parse(uuid::Uuid::new_v4().simple().to_string()).expect("UUID format")
}
fn column_name(mut n: usize) -> String {
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        s.insert(0, (b'A' + (n % 26) as u8) as char);
        n /= 26;
    }
    s
}

impl Render for Sheets {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frozen = self.busy || self.pending.is_some();
        let target = self
            .open
            .as_ref()
            .map(|open| format!("{}:{}", open.record.sheet_id, open.record.revision_id));
        let label = self
            .open
            .as_ref()
            .map(|open| {
                format!(
                    "Feedback to {} · {}{}",
                    open.record.author_name,
                    open.record.title,
                    if self.selection.is_some() {
                        " · selected cells"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default();
        self.feedback.update(cx, |composer, cx| {
            composer.sheet_context(target, label, frozen, cx)
        });
        let (t, c) = (tokens(), colors());
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(
                cx.listener(|s, event: &gpui::KeyDownEvent, window, cx| s.key(event, window, cx)),
            )
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(ui::view_header("layout-grid", "Sheets"))
            .child(self.toolbar(cx))
            .when_some(self.feedback_receipt.clone(), |e, receipt| {
                e.child(self.feedback_receipt_controls(&receipt, cx))
            })
            .when(self.busy, |e| {
                e.child(
                    ui::text(div().px(px(t.space.s12)).text_color(c.muted), &t.text.meta)
                        .child("Working…"),
                )
            })
            .when_some(self.error.clone(), |e, message| {
                e.child(
                    ui::text(
                        div().p(px(t.space.s8)).text_color(c.attention_text),
                        &t.text.meta,
                    )
                    .child(message),
                )
            })
            .when_some(self.notice.clone(), |e, message| {
                e.child(
                    ui::text(div().p(px(t.space.s8)).text_color(c.muted), &t.text.meta)
                        .child(message),
                )
            })
            .child(if self.open.is_some() {
                self.workbook(cx)
            } else {
                self.catalog_body(cx)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn successful_cursor_transitions_preserve_retry_routes_and_restart_cleanly() {
        let (first, second, third) = (None, Some("page2".into()), Some("page3".into()));
        let mut trail = Vec::new();
        commit_cursor(&mut trail, &first, &second);
        assert_eq!(trail.last(), Some(&None));
        // A failed read leaves the displayed cursor and trail untouched; retry
        // uses the same target without consuming or duplicating a boundary.
        let retry = trail.last().cloned();
        assert_eq!(retry, Some(first.clone()));
        commit_cursor(&mut trail, &second, &third);
        assert_eq!(trail.last(), Some(&second));
        commit_cursor(&mut trail, &third, &second);
        assert_eq!(trail, vec![None]);
        commit_cursor(&mut trail, &second, &first);
        assert!(trail.is_empty());
        commit_cursor(&mut trail, &first, &second);
        commit_cursor(&mut trail, &second, &third);
        commit_cursor(&mut trail, &third, &first);
        assert!(
            trail.is_empty(),
            "All Sheets and fresh History start at page one"
        );
    }
    #[test]
    fn shrinking_pages_return_to_the_actual_previous_boundary() {
        let trail = PageTrail::default()
            .moved((0, 0), (20, 0))
            .moved((20, 0), (40, 0));
        assert_eq!(trail.rows.last(), Some(&20));
        let trail = trail.moved((40, 0), (20, 0));
        assert_eq!(trail.rows.last(), Some(&0));
        let trail = trail.moved((20, 0), (20, 6)).moved((20, 6), (20, 9));
        assert_eq!(trail.columns.last(), Some(&6));
        assert_eq!(trail.rows.last(), Some(&0));
        let trail = trail.moved((20, 9), (20, 6)).moved((20, 6), (0, 6));
        assert!(trail.rows.is_empty());
        assert_eq!(trail.columns.last(), Some(&0));
    }
    #[test]
    fn selection_and_a1_labels_use_canonical_indices() {
        assert_eq!(column_name(1), "A");
        assert_eq!(column_name(26), "Z");
        assert_eq!(column_name(27), "AA");
        assert_eq!(column_name(256), "IV");
        let selection = Selection {
            anchor: (5, 4),
            end: (2, 1),
        };
        assert!(selection.contains(3, 2));
        assert!(!selection.contains(1, 2));
    }
    #[test]
    fn uncertain_feedback_retains_its_original_request() {
        assert!(unresolved_mutation(false, &api::Error::Unknown));
        assert!(unresolved_mutation(true, &api::Error::Refused(401)));
        assert!(unresolved_mutation(true, &api::Error::Unavailable));
        assert!(!unresolved_mutation(false, &api::Error::Refused(422)));
    }
}
