//! Explicit structural actions use the same revision compare-and-swap boundary.
use super::*;

pub(super) struct Copied {
    sheet: Id,
    revision: Id,
    tab: Id,
    rows: Vec<Id>,
    columns: Vec<Id>,
    row_indices: Vec<usize>,
    column_indices: Vec<usize>,
}
impl Sheets {
    pub(super) fn structure_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut row = div()
            .flex()
            .flex_wrap()
            .gap(px(tokens().space.s4))
            .child(
                ui::chip("sheet-copy-cells", "Copy cells / formulas", false)
                    .on_click(cx.listener(|s, _, _, cx| s.remember_copy(cx))),
            )
            .child(
                ui::chip("sheet-paste-cells", "Paste copied cells", false)
                    .on_click(cx.listener(|s, _, _, cx| s.paste_cells(cx))),
            );
        for (kind, label) in [
            (0, "Delete selected rows"),
            (1, "Delete selected columns"),
            (2, "Delete tab"),
        ] {
            row = row.child(
                ui::chip(
                    SharedString::from(format!("sheet-delete-{kind}")),
                    label,
                    false,
                )
                .on_click(cx.listener(move |s, _, _, cx| s.confirm_delete(kind, cx))),
            );
        }
        for (rows, label) in [
            (true, "Move copied rows here"),
            (false, "Move copied columns here"),
        ] {
            row = row.child(
                ui::chip(
                    SharedString::from(format!("sheet-move-{rows}")),
                    label,
                    false,
                )
                .on_click(cx.listener(move |s, _, _, cx| s.move_axes(rows, cx))),
            );
        }
        row
    }
    fn remember_copy(&mut self, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((open, selection)) = self.open.as_ref().zip(self.selection) else {
            return;
        };
        let (rows, cols) = selection.bounds();
        self.copied = Some(Copied {
            sheet: open.record.sheet_id.clone(),
            revision: open.record.revision_id.clone(),
            tab: open.grid.tab_id.clone(),
            rows: rows
                .clone()
                .map(|i| open.grid.rows[i].row_id.clone())
                .collect(),
            row_indices: rows.map(|i| open.grid.rows[i].index).collect(),
            columns: cols
                .clone()
                .map(|i| open.grid.columns[i].column_id.clone())
                .collect(),
            column_indices: cols.map(|i| open.grid.columns[i].index).collect(),
        });
        self.notice=Some("Cell references copied within this workbook revision. Select a target cell, then Paste copied cells; formulas adjust on the provider.".into());
        cx.notify();
    }
    fn paste_cells(&mut self, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((open, selection)) = self.open.as_ref().zip(self.selection) else {
            return;
        };
        let Some(copied) = &self.copied else {
            self.fail("Copy a cell range first.", cx);
            return;
        };
        if copied.sheet != open.record.sheet_id || copied.revision != open.record.revision_id {
            self.fail(
                "Copy the source again from this workbook revision before pasting formulas.",
                cx,
            );
            return;
        }
        let (rows, cols) = selection.bounds();
        let (row, col) = (*rows.start(), *cols.start());
        let Some(rows) = open.grid.rows.get(row..row + copied.rows.len()) else {
            self.fail("The paste extends beyond this loaded page.", cx);
            return;
        };
        let Some(cols) = open.grid.columns.get(col..col + copied.columns.len()) else {
            self.fail("The paste extends beyond this loaded page.", cx);
            return;
        };
        let op = Operation::CopyCells {
            source_tab_id: copied.tab.clone(),
            source_row_ids: copied.rows.clone(),
            source_column_ids: copied.columns.clone(),
            target_tab_id: open.grid.tab_id.clone(),
            target_row_ids: rows.iter().map(|r| r.row_id.clone()).collect(),
            target_column_ids: cols.iter().map(|c| c.column_id.clone()).collect(),
        };
        self.operation(op, cx);
    }
    fn move_axes(&mut self, rows: bool, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((open, selection)) = self.open.as_ref().zip(self.selection) else {
            return;
        };
        let Some(copied) = &self.copied else {
            self.fail("Copy a contiguous range first.", cx);
            return;
        };
        if copied.sheet != open.record.sheet_id
            || copied.revision != open.record.revision_id
            || copied.tab != open.grid.tab_id
        {
            self.fail(
                "A physical move requires a source copied from this tab and revision.",
                cx,
            );
            return;
        }
        let indices = if rows {
            &copied.row_indices
        } else {
            &copied.column_indices
        };
        if indices.windows(2).any(|w| w[1] != w[0] + 1) {
            self.fail("Physical moves require contiguous canonical rows or columns. Clear the saved sort/filter view first.",cx);
            return;
        }
        let (ids, before_id) = if rows {
            (
                copied.rows.clone(),
                open.grid.rows[selection.end.0].row_id.clone(),
            )
        } else {
            (
                copied.columns.clone(),
                open.grid.columns[selection.end.1].column_id.clone(),
            )
        };
        if ids.contains(&before_id) {
            self.fail("Choose a destination outside the copied range.", cx);
            return;
        }
        let tab_id = open.grid.tab_id.clone();
        let before_id = Some(before_id);
        self.operation(
            if rows {
                Operation::MoveRows {
                    tab_id,
                    ids,
                    before_id,
                }
            } else {
                Operation::MoveColumns {
                    tab_id,
                    ids,
                    before_id,
                }
            },
            cx,
        );
    }
    fn confirm_delete(&mut self, kind: u8, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((client, open)) = self.client.as_ref().zip(self.open.as_ref()) else {
            return;
        };
        let tab_id = open.grid.tab_id.clone();
        let (operation, label) = if kind == 2 {
            (
                Operation::DeleteTab { tab_id },
                "Delete this tab and its cells".to_owned(),
            )
        } else {
            let Some(selection) = self.selection else {
                return;
            };
            let (rows, cols) = selection.bounds();
            if kind == 0 {
                let ids: Vec<_> = rows.map(|r| open.grid.rows[r].row_id.clone()).collect();
                let label = format!("Delete {} selected rows and their cells", ids.len());
                (Operation::DeleteRows { tab_id, ids }, label)
            } else {
                let ids: Vec<_> = cols
                    .map(|c| open.grid.columns[c].column_id.clone())
                    .collect();
                let label = format!("Delete {} selected columns and their cells", ids.len());
                (Operation::DeleteColumns { tab_id, ids }, label)
            }
        };
        let edit = Edit::operator(new_id(), open.record.revision_id.clone(), vec![operation]);
        match client.prepare_edit(&open.record.sheet_id, &edit) {
            Ok(request) => {
                self.confirm = Some((label, request));
                cx.notify();
            }
            Err(e) => self.fail(e.message(), cx),
        }
    }
    pub(super) fn confirm_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some((label, _)) = &self.confirm else {
            return div();
        };
        ui::view_toolbar()
            .h_auto()
            .flex_wrap()
            .child(format!("{label}? Historical revisions remain available."))
            .child(
                ui::button(
                    "sheet-confirm-delete",
                    "Confirm deletion",
                    ui::Button::Danger,
                )
                .on_click(cx.listener(|s, _, _, cx| {
                    if s.clean(cx)
                        && let Some((_, request)) = s.confirm.take()
                    {
                        s.submit(request, cx);
                    }
                })),
            )
            .child(
                ui::chip("sheet-cancel-delete", "Cancel", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        s.confirm = None;
                        cx.notify();
                    },
                )),
            )
    }
}
