//! Native retained spreadsheet editor. Calculation and revision authority stay
//! in Backbone; UI drafts and unknown-outcome retries remain local to this pane.

use crate::desktop::{
    components as ui,
    design::{colors, tokens},
    name::{NameEvent, NameField},
};
use agent_interface_roster::sheets::{self as api, *};
use gpui::{
    Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*, px,
};
use std::{collections::BTreeMap, path::PathBuf};
mod controls;
mod grid;
mod structure;

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
#[derive(Default)]
struct Draft {
    base: Option<Id>,
    tab: Option<Id>,
    cells: BTreeMap<(Id, Id), Change>,
}
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Number,
    Boolean,
    Date,
    Formula,
    Blank,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Text => "Text",
            Self::Number => "Number",
            Self::Boolean => "Boolean",
            Self::Date => "Date",
            Self::Formula => "Formula",
            Self::Blank => "Blank",
        }
    }
    fn from(input: Option<&Input>) -> Self {
        match input {
            None => Self::Text,
            Some(Input::Text(_)) => Self::Text,
            Some(Input::Number(_)) => Self::Number,
            Some(Input::Boolean(_)) => Self::Boolean,
            Some(Input::Date(_)) => Self::Date,
            Some(Input::Formula(_)) => Self::Formula,
        }
    }
    fn parse(self, text: &str) -> Result<Patch<Input>, &'static str> {
        Ok(match self {
            Self::Text if text.len() <= 16 * 1024 => Patch::Set(Input::Text(text.into())),
            Self::Number => Patch::Set(Input::Number(
                text.parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .ok_or("Enter a finite number.")?,
            )),
            Self::Boolean => Patch::Set(Input::Boolean(match text.trim() {
                "true" | "TRUE" => true,
                "false" | "FALSE" => false,
                _ => return Err("Enter true or false."),
            })),
            Self::Date => Patch::Set(Input::Date(text.into())),
            Self::Formula if text.starts_with('=') && text.len() <= 4096 => {
                Patch::Set(Input::Formula(text.into()))
            }
            Self::Blank if text.is_empty() => Patch::Clear,
            Self::Blank => return Err("Clear the input or choose Text to keep what you typed."),
            _ => return Err("Use text up to 16 KiB or a formula beginning with = up to 4 KiB."),
        })
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
    draft: Draft,
    selection: Option<Selection>,
    input: Entity<NameField>,
    number_format: Entity<NameField>,
    feedback: Entity<NameField>,
    filter: Entity<NameField>,
    filter_operator: Operator,
    filter_kind: Kind,
    title: Entity<NameField>,
    author: Entity<NameField>,
    kind: Kind,
    input_dirty: bool,
    format_dirty: bool,
    creating: bool,
    feedback_kind: FeedbackKind,
    feedback_open: bool,
    pending: Option<Prepared>,
    pending_uncertain: bool,
    feedback_receipt: Option<FeedbackReceipt>,
    confirm: Option<(String, Prepared)>,
    copied: Option<structure::Copied>,
    busy: bool,
    error: Option<String>,
    notice: Option<String>,
    _inputs: Vec<Subscription>,
}
impl Sheets {
    pub fn new(directory: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| NameField::value_field("", 16384, true, window, cx));
        let number_format = cx.new(|cx| NameField::value_field("", 128, false, window, cx));
        let feedback = cx.new(|cx| NameField::feedback(window, cx));
        let filter = cx.new(|cx| NameField::value_field("", 16384, false, window, cx));
        let title = cx.new(|cx| NameField::value_field("", 240, false, window, cx));
        let author = cx.new(|cx| NameField::value_field("", 128, false, window, cx));
        let inputs = vec![
            cx.subscribe(&input, |s, _, event, cx| {
                if matches!(event, NameEvent::Changed(_)) {
                    s.input_dirty = true;
                    cx.notify();
                }
            }),
            cx.subscribe(&number_format, |s, _, event, cx| {
                if matches!(event, NameEvent::Changed(_)) {
                    s.format_dirty = true;
                    cx.notify();
                }
            }),
        ];
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
            draft: Draft::default(),
            selection: None,
            input,
            number_format,
            feedback,
            filter,
            filter_operator: Operator::Contains,
            filter_kind: Kind::Text,
            title,
            author,
            kind: Kind::Text,
            input_dirty: false,
            format_dirty: false,
            creating: false,
            feedback_kind: FeedbackKind::Question,
            feedback_open: false,
            pending: None,
            pending_uncertain: false,
            feedback_receipt: None,
            confirm: None,
            copied: None,
            busy: false,
            error: None,
            notice: None,
            _inputs: inputs,
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
        if command && key.key == "s" {
            self.save(cx);
        } else if !self.focus.is_focused(window) {
            return;
        } else if command && key.key == "c" {
            self.copy(cx);
        } else if command && key.key == "v" {
            self.paste(cx);
        } else if key.key == "enter" {
            let focus = self.input.read(cx).focus.clone();
            window.focus(&focus, cx);
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

    fn dirty(&self) -> bool {
        self.input_dirty || self.format_dirty || !self.draft.cells.is_empty()
    }
    fn cell_editable(&self) -> bool {
        !self.busy
            && self.pending.is_none()
            && self.selection.is_some()
            && draft_matches_revision(
                &self.draft,
                self.open.as_ref().map(|o| &o.record.revision_id),
            )
    }
    fn clean(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.feedback.read(cx).value().is_empty() {
            self.fail(
                "Send or clear the feedback draft before changing the workbook or revision.",
                cx,
            );
            return false;
        }
        if self.busy || self.pending.is_some() {
            return false;
        }
        if self.dirty() {
            self.fail(
                "Save or explicitly discard the draft before navigating.",
                cx,
            );
            return false;
        }
        true
    }
    fn fail(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }
    fn begin(&mut self, cx: &mut Context<Self>) {
        self.confirm = None;
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
                        s.creating = false;
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
                        s.input_dirty = false;
                        s.format_dirty = false;
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
        if !self.feedback.read(cx).value().is_empty() {
            self.fail(
                "Send or clear the feedback draft before changing the revision.",
                cx,
            );
            return;
        }
        if self.busy || self.pending.is_some() || !self.stage(cx) {
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
        if self.busy || self.pending.is_some() || !self.stage(cx) {
            return;
        }
        let Some(open) = &self.open else {
            return;
        };
        let Some((r, c)) = open.grid.rows.get(row).zip(open.grid.columns.get(column)) else {
            return;
        };
        let change = self
            .draft
            .base
            .as_ref()
            .filter(|base| *base == &open.record.revision_id)
            .and_then(|_| {
                self.draft
                    .cells
                    .get(&(r.row_id.clone(), c.column_id.clone()))
            });
        let cell = open.grid.cell(&r.row_id, &c.column_id);
        let input = match change.map(|c| &c.input) {
            Some(Patch::Set(i)) => Some(i),
            Some(Patch::Clear) => None,
            _ => cell.and_then(|c| c.input.as_ref()),
        };
        let format = match change.map(|c| &c.number_format) {
            Some(Patch::Set(s)) => Some(s.as_str()),
            Some(Patch::Clear) => None,
            _ => cell.and_then(|c| c.number_format.as_deref()),
        };
        self.kind = Kind::from(input);
        let text = input.map(Input::text).unwrap_or_default();
        let format = format.unwrap_or_default().to_owned();
        self.confirm = None;
        self.selection = Some(Selection {
            anchor: if extend {
                self.selection.map(|s| s.anchor).unwrap_or((row, column))
            } else {
                (row, column)
            },
            end: (row, column),
        });
        self.input
            .update(cx, |field, cx| field.set_value(&text, cx));
        self.number_format
            .update(cx, |field, cx| field.set_value(&format, cx));
        self.input_dirty = false;
        self.format_dirty = false;
        cx.notify();
    }
    fn stage(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.input_dirty && !self.format_dirty {
            return true;
        }
        if self.busy
            || self.pending.is_some()
            || self.input.read(cx).composing()
            || self.number_format.read(cx).composing()
        {
            return false;
        }
        let Some((open, selection)) = self.open.as_ref().zip(self.selection) else {
            return false;
        };
        if self
            .draft
            .base
            .as_ref()
            .is_some_and(|base| base != &open.record.revision_id)
        {
            self.fail("Review the newer values and explicitly reapply or discard the retained draft first.",cx);
            return false;
        }
        let (row, col) = selection.end;
        let key = (
            open.grid.rows[row].row_id.clone(),
            open.grid.columns[col].column_id.clone(),
        );
        let input = if self.input_dirty {
            match self.kind.parse(self.input.read(cx).value()) {
                Ok(v) => v,
                Err(e) => {
                    self.fail(e, cx);
                    return false;
                }
            }
        } else {
            Patch::Unchanged
        };
        let format = self.number_format.read(cx).value().to_owned();
        if format.chars().any(char::is_control) || format.chars().count() > 128 {
            self.fail(
                "Number format must contain at most 128 characters without controls.",
                cx,
            );
            return false;
        }
        self.draft
            .base
            .get_or_insert_with(|| open.record.revision_id.clone());
        self.draft
            .tab
            .get_or_insert_with(|| open.grid.tab_id.clone());
        let change = self.draft.cells.entry(key.clone()).or_insert(Change {
            row_id: key.0,
            column_id: key.1,
            input: Patch::Unchanged,
            number_format: Patch::Unchanged,
        });
        if self.input_dirty {
            change.input = input;
        }
        if self.format_dirty {
            change.number_format = if format.is_empty() {
                Patch::Clear
            } else {
                Patch::Set(format)
            };
        }
        self.input_dirty = false;
        self.format_dirty = false;
        cx.notify();
        true
    }
    fn save(&mut self, cx: &mut Context<Self>) {
        if !self.feedback.read(cx).value().is_empty() {
            self.fail(
                "Send or clear the feedback draft before changing the revision.",
                cx,
            );
            return;
        }
        if self.busy || self.pending.is_some() || !self.stage(cx) || self.draft.cells.is_empty() {
            return;
        }
        let Some((client, open)) = self.client.as_ref().zip(self.open.as_ref()) else {
            return;
        };
        let Some(base) = self.draft.base.clone() else {
            return;
        };
        if base != open.record.revision_id {
            self.fail(
                "Explicitly reapply the retained draft before saving against this revision.",
                cx,
            );
            return;
        }
        let edit = Edit::operator(
            new_id(),
            base,
            vec![Operation::SetCells {
                tab_id: open.grid.tab_id.clone(),
                cells: self.draft.cells.values().cloned().collect(),
            }],
        );
        match client.prepare_edit(&open.record.sheet_id, &edit) {
            Ok(request) => self.submit(request, cx),
            Err(e) => self.fail(e.message(), cx),
        }
    }
    fn operation(&mut self, operation: Operation, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((client, open)) = self.client.as_ref().zip(self.open.as_ref()) else {
            return;
        };
        let edit = Edit::operator(new_id(), open.record.revision_id.clone(), vec![operation]);
        match client.prepare_edit(&open.record.sheet_id, &edit) {
            Ok(request) => self.submit(request, cx),
            Err(e) => self.fail(e.message(), cx),
        }
    }
    fn submit(&mut self, request: Prepared, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        self.pending = Some(request.clone());
        self.begin(cx);
        let work = cx
            .background_executor()
            .spawn(async move { client.submit(&request) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(Accepted::Revision(record)) => {
                        s.pending = None;
                        s.pending_uncertain = false;
                        s.draft = Draft::default();
                        s.input_dirty = false;
                        s.format_dirty = false;
                        s.creating = false;
                        let (tab, row, column) = s
                            .open
                            .as_ref()
                            .map(|o| {
                                (
                                    Some(o.grid.tab_id.clone()),
                                    o.query.row_offset,
                                    o.query.column_offset,
                                )
                            })
                            .unwrap_or((None, 0, 0));
                        s.read_grid(record, tab, row, column, cx);
                    }
                    Ok(Accepted::Imported { record, warnings }) => {
                        s.pending = None;
                        s.pending_uncertain = false;
                        s.creating = false;
                        s.read_grid(record, None, 0, 0, cx);
                        s.notice = Some(if warnings.is_empty() {
                            "Imported a new workbook.".into()
                        } else {
                            format!("Imported with warnings: {}", warnings.join("; "))
                        });
                    }
                    Ok(Accepted::Feedback(receipt)) => {
                        s.pending = None;
                        s.pending_uncertain = false;
                        s.feedback.update(cx, |f, cx| f.clear(cx));
                        s.notice = Some(receipt.delivery.message().into());
                        s.feedback_receipt = Some(receipt);
                    }
                    Err(error) => {
                        s.pending_uncertain = unresolved_mutation(s.pending_uncertain, &error);
                        if !s.pending_uncertain {
                            s.pending = None;
                        }
                        s.error = Some(error.message().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
    fn reapply(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.pending.is_some() {
            return;
        }
        let Some(open) = &self.open else {
            return;
        };
        if self.draft.tab.as_ref() != Some(&open.grid.tab_id)
            || !self.draft.cells.keys().all(|(r, c)| {
                open.grid.rows.iter().any(|row| &row.row_id == r)
                    && open.grid.columns.iter().any(|col| &col.column_id == c)
            })
        {
            self.fail("Some draft cells are absent from this page or were deleted. Copy the draft before resolving those changes; it cannot be reapplied here.",cx);
            return;
        }
        self.draft.base = Some(open.record.revision_id.clone());
        self.selection = None;
        self.input_dirty = false;
        self.format_dirty = false;
        self.error = None;
        self.notice =
            Some("Draft reapplied to the displayed revision. Review it, then Save changes.".into());
        cx.notify();
    }
    fn discard(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.pending.is_some() {
            return;
        }
        self.draft = Draft::default();
        self.selection = None;
        self.input_dirty = false;
        self.format_dirty = false;
        self.error = None;
        cx.notify();
    }
}
fn draft_matches_revision(draft: &Draft, revision: Option<&Id>) -> bool {
    draft
        .base
        .as_ref()
        .is_none_or(|base| Some(base) == revision)
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
        let cell_frozen = !self.cell_editable();
        for field in [&self.input, &self.number_format] {
            field.update(cx, |field, _| field.set_read_only(cell_frozen));
        }
        for field in [&self.feedback, &self.filter, &self.title, &self.author] {
            field.update(cx, |field, _| field.set_read_only(frozen));
        }
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
            .when(self.confirm.is_some(), |e| {
                e.child(self.confirm_controls(cx))
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
    fn blank_cell_typing_is_literal_and_explicit_blank_never_discards_text() {
        assert!(
            matches!(Kind::from(None).parse("new value"), Ok(Patch::Set(Input::Text(value))) if value == "new value")
        );
        assert!(Kind::Blank.parse("keep me").is_err());
        assert!(matches!(Kind::Blank.parse(""), Ok(Patch::Clear)));
    }

    #[test]
    fn lost_reply_stays_unresolved_across_preflight_and_provider_refusals() {
        let mut uncertain = unresolved_mutation(false, &api::Error::Unknown);
        for error in [
            api::Error::ProviderChanged,
            api::Error::Unavailable,
            api::Error::Refused(401),
            api::Error::Refused(503),
        ] {
            uncertain = unresolved_mutation(uncertain, &error);
            assert!(
                uncertain,
                "A retry failure cannot resolve the earlier lost reply"
            );
        }
        assert!(!unresolved_mutation(false, &api::Error::Refused(422)));
    }

    #[test]
    fn retained_stale_draft_freezes_both_value_and_type_editing() {
        let old = Id::parse("1".repeat(32)).unwrap();
        let new = Id::parse("2".repeat(32)).unwrap();
        let mut draft = Draft {
            base: Some(old.clone()),
            ..Default::default()
        };
        assert!(draft_matches_revision(&draft, Some(&old)));
        assert!(!draft_matches_revision(&draft, Some(&new)));
        draft.base = Some(new.clone());
        assert!(draft_matches_revision(&draft, Some(&new)));
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
    fn literal_equals_text_does_not_become_a_formula() {
        assert!(matches!(
            Kind::Text.parse("=SUM(A1:A2)"),
            Ok(Patch::Set(Input::Text(_)))
        ));
        assert!(matches!(
            Kind::Formula.parse("=SUM(A1:A2)"),
            Ok(Patch::Set(Input::Formula(_)))
        ));
        assert!(Kind::Formula.parse("SUM(A1:A2)").is_err());
        assert!(Kind::Number.parse("NaN").is_err());
        assert!(Kind::Boolean.parse("yes").is_err());
    }
}
