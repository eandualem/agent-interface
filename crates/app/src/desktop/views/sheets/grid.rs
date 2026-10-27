use super::*;
use gpui::{ClipboardItem, uniform_list};

impl Sheets {
    pub(super) fn workbook(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(open) = &self.open else {
            return div().into_any_element();
        };
        let (t, c) = (tokens(), colors());
        if let Some(history) = &self.history {
            let mut body = div()
                .id("sheet-history")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .flex()
                .flex_col();
            for record in &history.items {
                let record = record.clone();
                body =
                    body.child(
                        ui::row(
                            SharedString::from(format!("revision-{}", record.revision_id)),
                            record.revision_id == open.record.revision_id,
                        )
                        .child(format!(
                            "{} · {} · {}",
                            record.updated_at,
                            record.editor_name.as_deref().unwrap_or(&record.editor_kind),
                            record.revision_id
                        ))
                        .on_click(cx.listener(move |s, _, _, cx| {
                            s.open_record(record.clone(), None, 0, 0, cx)
                        })),
                    );
            }
            return body.child(self.history_controls(cx)).into_any_element();
        }
        let mut tabs = ui::view_toolbar().gap(px(t.space.s6)).flex_wrap().h_auto();
        for tab in &open.record.tabs {
            let id = tab.tab_id.clone();
            tabs = tabs.child(
                ui::chip(
                    SharedString::from(format!("sheet-tab-{id}")),
                    SharedString::from(tab.name.clone()),
                    id == open.grid.tab_id,
                )
                .on_click(cx.listener(move |s, _, _, cx| {
                    if let Some(open) = &s.open {
                        s.open_record(open.record.clone(), Some(id.clone()), 0, 0, cx);
                    }
                })),
            );
        }
        let width = 54. + open.grid.columns.len() as f32 * 160.;
        let mut header = div()
            .h(px(28.))
            .flex_shrink_0()
            .flex()
            .bg(c.selected)
            .child(div().w(px(54.)).flex_shrink_0().child("Row"));
        for column in &open.grid.columns {
            header = header.child(
                div()
                    .w(px(160.))
                    .flex_shrink_0()
                    .px(px(8.))
                    .border_l_1()
                    .border_color(c.line)
                    .child(column_name(column.index)),
            );
        }
        let table = div()
            .id("sheet-grid-horizontal")
            .flex_1()
            .min_h_0()
            .overflow_x_scroll()
            .child(
                div()
                    .w(px(width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(
                        uniform_list(
                            "sheet-grid-rows",
                            open.grid.rows.len(),
                            cx.processor(|s, range: std::ops::Range<usize>, _, cx| {
                                range.map(|row| s.row(row, cx)).collect::<Vec<_>>()
                            }),
                        )
                        .flex_1()
                        .min_h_0(),
                    ),
            );
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                ui::text(
                    div()
                        .px(px(t.space.s12))
                        .py(px(t.space.s4))
                        .text_color(c.muted),
                    &t.text.meta,
                )
                .child(format!(
                    "{} · {} · {} · canonical A1 coordinates",
                    open.record.title,
                    open.record.author_name,
                    if open.record.current_revision_id == open.record.revision_id {
                        "current revision"
                    } else {
                        "historical revision"
                    }
                )),
            )
            .child(tabs)
            .child(self.edit_controls(cx))
            .child(table)
            .child(self.page_controls(cx))
            .when(self.feedback_open, |e| e.child(self.feedback_controls(cx)))
            .into_any_element()
    }
    fn row(&self, row: usize, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let Some(open) = &self.open else {
            return div();
        };
        let r = &open.grid.rows[row];
        let mut line = ui::text(
            div().h(px(28.)).flex().border_b_1().border_color(c.line),
            &t.text.meta,
        )
        .child(
            div()
                .w(px(54.))
                .h_full()
                .flex_shrink_0()
                .px(px(8.))
                .text_color(c.muted)
                .child(r.index.to_string()),
        );
        for (column, col) in open.grid.columns.iter().enumerate() {
            let cell = open.grid.cell(&r.row_id, &col.column_id);
            let staged = self
                .draft
                .base
                .as_ref()
                .filter(|b| *b == &open.record.revision_id)
                .and_then(|_| {
                    self.draft
                        .cells
                        .get(&(r.row_id.clone(), col.column_id.clone()))
                });
            let display = match staged.map(|s| &s.input) {
                Some(Patch::Set(input)) => input.text(),
                Some(Patch::Clear) => String::new(),
                _ => cell.map(|c| c.display.clone()).unwrap_or_default(),
            };
            let display: String = display
                .chars()
                .take(200)
                .map(|ch| match ch {
                    '\n' => '↵',
                    '\t' => '⇥',
                    ch if ch.is_control() => ' ',
                    ch => ch,
                })
                .collect();
            let selected = self.selection.is_some_and(|s| s.contains(row, column));
            line = line.child(
                div()
                    .id(SharedString::from(format!("sheet-cell-{row}-{column}")))
                    .w(px(160.))
                    .h_full()
                    .flex_shrink_0()
                    .px(px(8.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .border_l_1()
                    .border_color(c.line)
                    .cursor_pointer()
                    .when(selected, |e| e.bg(c.selected))
                    .when(staged.is_some(), |e| e.text_color(c.accent))
                    .when(
                        cell.is_some_and(|c| matches!(c.value, Value::Error(_))),
                        |e| e.text_color(c.attention_text),
                    )
                    .child(display)
                    .on_click(cx.listener(move |s, event: &gpui::ClickEvent, window, cx| {
                        s.select(row, column, event.modifiers().shift, cx);
                        window.focus(&s.focus, cx);
                    })),
            );
        }
        line
    }
    pub(super) fn page_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(open) = &self.open else {
            return div();
        };
        let row = open.query.row_offset;
        let col = open.query.column_offset;
        let next_row = open.grid.next_row_offset;
        let next_col = col + open.grid.columns.len();
        let previous_row = open.pages.rows.last().copied().unwrap_or(0);
        let previous_col = open.pages.columns.last().copied().unwrap_or(0);
        ui::view_toolbar()
            .h_auto()
            .flex_wrap()
            .child(
                ui::chip("sheet-rows-prev", "Previous rows", false).on_click(cx.listener(
                    move |s, _, _, cx| {
                        if row > 0 {
                            s.navigate(previous_row, col, cx);
                        }
                    },
                )),
            )
            .child(
                ui::chip("sheet-rows-next", "Next rows", false).on_click(cx.listener(
                    move |s, _, _, cx| {
                        if let Some(row) = next_row {
                            s.navigate(row, col, cx);
                        }
                    },
                )),
            )
            .child(
                ui::chip("sheet-cols-prev", "Previous columns", false).on_click(cx.listener(
                    move |s, _, _, cx| {
                        if col > 0 {
                            s.navigate(row, previous_col, cx);
                        }
                    },
                )),
            )
            .child(
                ui::chip("sheet-cols-next", "Next columns", false).on_click(cx.listener(
                    move |s, _, _, cx| {
                        if s.open
                            .as_ref()
                            .is_some_and(|o| next_col < o.grid.total_columns)
                        {
                            s.navigate(row, next_col, cx);
                        }
                    },
                )),
            )
            .child(format!(
                "{} rows · {} columns · Shift-click selects a range",
                open.grid.total_rows, open.grid.total_columns
            ))
    }
    fn navigate(&mut self, row: usize, col: usize, cx: &mut Context<Self>) {
        if let Some(open) = &self.open {
            self.open_record(
                open.record.clone(),
                Some(open.grid.tab_id.clone()),
                row,
                col,
                cx,
            );
        }
    }
    pub(super) fn copy(&mut self, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((open, selection)) = self.open.as_ref().zip(self.selection) else {
            return;
        };
        let (rows, cols) = selection.bounds();
        let text = rows
            .map(|r| {
                cols.clone()
                    .map(|c| {
                        let row = &open.grid.rows[r].row_id;
                        let col = &open.grid.columns[c].column_id;
                        quote(
                            open.grid
                                .cell(row, col)
                                .map(|c| c.display.as_str())
                                .unwrap_or_default(),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notice = Some(
            "Copied displayed values as tab-separated text. Paste treats them as literal text."
                .into(),
        );
        cx.notify();
    }
    pub(super) fn paste(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.pending.is_some() || !self.stage(cx) {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let values = match parse_tsv(&text) {
            Ok(v) => v,
            Err(e) => {
                self.fail(e, cx);
                return;
            }
        };
        let Some((open, selection)) = self.open.as_ref().zip(self.selection) else {
            return;
        };
        if self
            .draft
            .base
            .as_ref()
            .is_some_and(|base| base != &open.record.revision_id)
        {
            self.fail(
                "Resolve the retained draft before pasting into this revision.",
                cx,
            );
            return;
        }
        let (rows, columns) = selection.bounds();
        let (row, col) = (*rows.start(), *columns.start());
        if row + values.len() > open.grid.rows.len()
            || values
                .iter()
                .any(|r| col + r.len() > open.grid.columns.len())
        {
            self.fail("The paste extends beyond this loaded page. Choose a smaller range; nothing was pasted.",cx);
            return;
        }
        let mut changes = self.draft.cells.clone();
        for (ri, values) in values.into_iter().enumerate() {
            for (ci, value) in values.into_iter().enumerate() {
                let r = open.grid.rows[row + ri].row_id.clone();
                let c = open.grid.columns[col + ci].column_id.clone();
                changes
                    .entry((r.clone(), c.clone()))
                    .and_modify(|change| change.input = Patch::Set(Input::Text(value.clone())))
                    .or_insert(Change {
                        row_id: r,
                        column_id: c,
                        input: Patch::Set(Input::Text(value)),
                        number_format: Patch::Unchanged,
                    });
            }
        }
        if changes.len() > 1000 {
            self.fail(
                "Paste would exceed 1,000 changed cells. Nothing was pasted.",
                cx,
            );
            return;
        }
        self.draft.cells = changes;
        self.draft.base = Some(open.record.revision_id.clone());
        self.draft.tab = Some(open.grid.tab_id.clone());
        self.selection = None;
        self.notice = Some("Pasted literal text into the draft. Save changes to commit it.".into());
        cx.notify();
    }
}
fn quote(s: &str) -> String {
    if s.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.into()
    }
}
fn parse_tsv(s: &str) -> Result<Vec<Vec<String>>, &'static str> {
    if s.len() > 1024 * 1024 {
        return Err("Clipboard text exceeds 1 MiB.");
    }
    let mut rows = vec![Vec::new()];
    let mut value = String::new();
    let mut chars = s.chars().peekable();
    let mut quoted = false;
    let mut closed = false;
    let mut count = 0;
    while let Some(ch) = chars.next() {
        if quoted {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    value.push('"');
                } else {
                    quoted = false;
                    closed = true;
                }
            } else {
                value.push(ch);
            }
        } else if ch == '"' && value.is_empty() && !closed {
            quoted = true;
        } else if ch == '\t' || ch == '\n' || ch == '\r' {
            if value.len() > 16384 {
                return Err("A pasted cell exceeds 16 KiB.");
            }
            rows.last_mut().unwrap().push(std::mem::take(&mut value));
            count += 1;
            closed = false;
            if ch != '\t' {
                if ch == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                rows.push(Vec::new());
            }
        } else if closed {
            return Err("Clipboard text has invalid quoted cells.");
        } else {
            value.push(ch);
        }
        if count > 1000 || value.len() > 16384 {
            return Err("Paste is limited to 1,000 cells and 16 KiB per cell.");
        }
    }
    if quoted {
        return Err("Clipboard text has an unfinished quoted cell.");
    }
    if !value.is_empty() || closed || rows.last().is_some_and(|r| !r.is_empty()) || rows.len() == 1
    {
        rows.last_mut().unwrap().push(value);
    } else {
        rows.pop();
    }
    let width = rows.first().map(Vec::len).unwrap_or(0);
    if rows.iter().any(|r| r.len() != width) || rows.iter().map(Vec::len).sum::<usize>() > 1000 {
        return Err("Paste a rectangular range of at most 1,000 cells.");
    }
    Ok(rows)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_roundtrips_quotes_newlines_and_formula_like_literal_text() {
        let values = ["=literal", "two\tcolumns", "two\nlines", "a\"b", ""];
        let wire = values
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join("\t");
        assert_eq!(
            parse_tsv(&wire).unwrap(),
            vec![values.map(str::to_owned).to_vec()]
        );
        assert!(parse_tsv("one\ttwo\nthree").is_err());
        assert!(parse_tsv("\"unfinished").is_err());
    }
}
