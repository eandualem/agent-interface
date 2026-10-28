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
            .track_scroll(&self.columns_scroll)
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
                        .track_scroll(&self.rows_scroll)
                        .flex_1()
                        .min_h_0(),
                    ),
            );
        let sheet = div()
            .flex_1()
            .min_w_0()
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
                    "{} · {} · {}",
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
            .child(table)
            .child(self.page_controls(cx));
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .child(sheet)
            .child(self.conversation.clone())
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
            let display = cell.map(|c| c.display.clone()).unwrap_or_default();
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
                        let cell = open.grid.cell(row, col);
                        clipboard_value(
                            cell.map(|c| c.display.as_str()).unwrap_or_default(),
                            cell.is_some_and(|c| matches!(c.value, Value::Text(_))),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.notice = Some("Copied displayed values as tab-separated text.".into());
        cx.notify();
    }
}
fn quote(s: &str) -> String {
    if s.is_empty() || s.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.into()
    }
}

// Keep text results literal when pasted into a spreadsheet, including formula
// results whose calculated type is text. Numeric results retain their type.
fn clipboard_value(display: &str, text: bool) -> String {
    if text && display.trim_start().starts_with(['=', '+', '-', '@']) {
        quote(&format!("'{display}"))
    } else {
        quote(display)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_text_stays_literal_without_changing_numeric_results() {
        for text in ["=1+1", "+cmd", "-5", "@SUM(A1)", "  =1+1"] {
            assert_eq!(clipboard_value(text, true), format!("'{text}"));
        }
        assert_eq!(clipboard_value("-5", false), "-5");
        assert_eq!(clipboard_value("ordinary text", true), "ordinary text");
        assert_eq!(clipboard_value("\t=1+1", true), "\"'\t=1+1\"");
        assert_eq!(clipboard_value("a\tb", true), "\"a\tb\"");
    }
}
