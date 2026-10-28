use super::*;
use gpui::{ClipboardItem, uniform_list};

/// The width of a column of cells.
pub(super) const CELL: f32 = 160.;

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
        let selected_columns = self.selection.map(|selection| selection.bounds().1);
        let width = t.size.sheet_number + open.grid.columns.len() as f32 * CELL;
        // Column letters over the grid; the selected ones in the accent.
        let mut header = ui::text(
            div()
                .h(px(t.size.sheet_header))
                .flex_shrink_0()
                .flex()
                .bg(c.panel)
                .border_b(px(t.size.hairline))
                .border_color(c.line_strong)
                .font_family(t.font.mono.clone())
                .text_color(c.muted),
            &t.text.key,
        )
        .font_weight(gpui::FontWeight::NORMAL)
        .child(
            div()
                .w(px(t.size.sheet_number))
                .flex_shrink_0()
                .border_r(px(t.size.hairline))
                .border_color(c.line_strong),
        );
        for (index, column) in open.grid.columns.iter().enumerate() {
            let on = selected_columns
                .as_ref()
                .is_some_and(|columns| columns.contains(&index));
            header = header.child(
                div()
                    .w(px(CELL))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_r(px(t.size.hairline))
                    .border_color(c.line)
                    .when(on, |el| el.bg(c.selected).text_color(c.accent_text))
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
            .child(self.selection_bar(open))
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

    /// Over the grid: the selected cells as a label and how many there are,
    /// and that the workbook is read-only.
    fn selection_bar(&self, open: &Open) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let (label, count) = match (self.selection, self.anchor(open)) {
            (Some(selection), Some(anchor)) => {
                let (rows, columns) = selection.bounds();
                let cells = rows.count() * columns.count();
                let label = anchor
                    .rsplit_once('!')
                    .map_or(anchor.clone(), |(_, cells)| cells.to_owned());
                let count = if cells == 1 {
                    "1 cell selected".to_owned()
                } else {
                    format!("{cells} cells selected")
                };
                (label, count)
            }
            _ => (
                String::new(),
                "Select cells to copy them or talk about them".to_owned(),
            ),
        };
        ui::text(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s10))
                .h(px(t.size.view_toolbar - t.space.s4))
                .px(px(t.space.s12))
                .border_b(px(t.size.hairline))
                .border_color(c.line)
                .text_color(c.muted),
            &t.text.status,
        )
        .when(!label.is_empty(), |el| {
            el.child(
                ui::text(
                    div()
                        .flex()
                        .items_center()
                        .h(px(t.size.chip))
                        .min_w(px(t.space.s28 * 2.0 + t.space.s8))
                        .px(px(t.space.s8))
                        .rounded(px(t.radius.key))
                        .bg(c.field)
                        .border(px(t.size.hairline))
                        .border_color(c.line_strong)
                        .font_family(t.font.mono.clone())
                        .text_color(c.text),
                    &t.text.meta,
                )
                .child(label),
            )
        })
        .child(count)
        .child(div().flex_1())
        .child(ui::icon("lock", t.size.icon - 1.0, c.muted))
        .child("Read-only")
    }

    fn row(&self, row: usize, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let Some(open) = &self.open else {
            return div();
        };
        let r = &open.grid.rows[row];
        let selected_row = self
            .selection
            .is_some_and(|selection| selection.bounds().0.contains(&row));
        // The first row holds the column names.
        let heading = r.index == 1;
        let mut line = ui::text(
            div().h(px(t.size.sheet_row)).flex().text_color(c.text_2),
            &t.text.body,
        )
        .child(
            ui::text(
                div()
                    .w(px(t.size.sheet_number))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_end()
                    .pr(px(t.space.s8))
                    .bg(c.panel)
                    .border_r(px(t.size.hairline))
                    .border_b(px(t.size.hairline))
                    .border_r(px(t.size.hairline))
                    .border_color(c.line_strong)
                    .font_family(t.font.mono.clone())
                    .text_color(if selected_row { c.accent_text } else { c.muted }),
                &t.text.key,
            )
            .font_weight(gpui::FontWeight::NORMAL)
            .when(selected_row, |el| el.bg(c.selected))
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
            let number = cell.is_some_and(|c| matches!(c.value, Value::Number(_)));
            let selected = self.selection.is_some_and(|s| s.contains(row, column));
            let anchor = self.selection.is_some_and(|s| s.anchor == (row, column));
            line = line.child(
                div()
                    .id(SharedString::from(format!("sheet-cell-{row}-{column}")))
                    .w(px(CELL))
                    .h_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .px(px(t.space.s8))
                    .overflow_hidden()
                    .border_r(px(t.size.hairline))
                    .border_b(px(t.size.hairline))
                    .border_color(c.line)
                    .cursor_pointer()
                    .when(number, |el| {
                        el.justify_end()
                            .font_family(t.font.mono.clone())
                            .text_size(t.text.status.size())
                    })
                    .when(heading, |el| {
                        el.text_color(c.text).font_weight(t.text.workspace.weight())
                    })
                    .when(selected, |el| el.bg(c.accent_soft).text_color(c.text))
                    .when(anchor, |el| {
                        el.border(px(t.size.focus_ring)).border_color(c.accent)
                    })
                    .when(
                        cell.is_some_and(|c| matches!(c.value, Value::Error(_))),
                        |e| e.text_color(c.attention_text),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(display),
                    )
                    .on_click(cx.listener(move |s, event: &gpui::ClickEvent, window, cx| {
                        s.select(row, column, event.modifiers().shift, cx);
                        window.focus(&s.focus, cx);
                    })),
            );
        }
        line
    }

    /// Under the grid: the workbook's tabs, which rows and columns show, and
    /// the arrows to the rows before and after them.
    pub(super) fn page_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some(open) = &self.open else {
            return div();
        };
        let (t, c) = (tokens(), colors());
        let row = open.query.row_offset;
        let col = open.query.column_offset;
        let next_row = open.grid.next_row_offset;
        let next_col = col + open.grid.columns.len();
        let more_columns = next_col < open.grid.total_columns;
        let previous_row = open.pages.rows.last().copied().unwrap_or(0);
        let previous_col = open.pages.columns.last().copied().unwrap_or(0);
        // Tabs that do not fit scroll sideways rather than hide.
        let mut tabs = div()
            .id("sheet-tabs")
            .flex()
            .flex_1()
            .gap(px(t.space.s4))
            .min_w_0()
            .overflow_x_scroll();
        for tab in &open.record.tabs {
            let id = tab.tab_id.clone();
            let on = id == open.grid.tab_id;
            tabs = tabs.child(
                ui::text(
                    div()
                        .id(SharedString::from(format!("sheet-tab-{id}")))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .h(px(t.size.action))
                        .px(px(t.space.s12))
                        .rounded(px(t.radius.icon_button))
                        .cursor_pointer()
                        .whitespace_nowrap(),
                    &t.text.status,
                )
                .when(on, |el| {
                    el.bg(c.surface)
                        .text_color(c.text)
                        .border(px(t.size.hairline))
                        .border_color(c.line_strong)
                })
                .when(!on, |el| {
                    el.text_color(c.muted)
                        .hover(|el| el.text_color(colors().text))
                })
                .child(tab.name.clone())
                .on_click(cx.listener(move |s, _, _, cx| {
                    if let Some(open) = &s.open {
                        s.open_record(open.record.clone(), Some(id.clone()), 0, 0, cx);
                    }
                })),
            );
        }
        let first = open.grid.rows.first().map_or(0, |row| row.index);
        let last = open.grid.rows.last().map_or(0, |row| row.index);
        let columns = match (open.grid.columns.first(), open.grid.columns.last()) {
            (Some(a), Some(b)) => format!("{}–{}", column_name(a.index), column_name(b.index)),
            _ => String::new(),
        };
        ui::text(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s4))
                .h(px(t.size.sheet_tabs))
                .px(px(t.space.s10))
                .bg(c.panel)
                .border_t(px(t.size.hairline))
                .border_color(c.line),
            &t.text.status,
        )
        .child(tabs)
        .child(
            ui::text(
                div()
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .font_family(t.font.mono.clone())
                    .text_color(c.muted),
                &t.text.key,
            )
            .font_weight(gpui::FontWeight::NORMAL)
            .child(format!(
                "rows {first}–{last} of {} · {columns}",
                open.grid.total_rows
            )),
        )
        .child(
            ui::icon_button("sheet-rows-prev", "chevron-left").on_click(cx.listener(
                move |s, _, _, cx| {
                    if row > 0 {
                        s.navigate(previous_row, col, cx);
                    }
                },
            )),
        )
        .child(
            ui::icon_button("sheet-rows-next", "chevron-right").on_click(cx.listener(
                move |s, _, _, cx| {
                    if let Some(row) = next_row {
                        s.navigate(row, col, cx);
                    }
                },
            )),
        )
        // Columns page only when the tab has more than one page of them.
        .when(col > 0, |el| {
            el.child(
                ui::action("sheet-cols-prev", "chevron-left", "Earlier columns")
                    .on_click(cx.listener(move |s, _, _, cx| s.navigate(row, previous_col, cx))),
            )
        })
        .when(more_columns, |el| {
            el.child(
                ui::action("sheet-cols-next", "chevron-right", "More columns")
                    .on_click(cx.listener(move |s, _, _, cx| s.navigate(row, next_col, cx))),
            )
        })
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
