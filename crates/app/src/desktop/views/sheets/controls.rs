use super::*;
use gpui::ClipboardItem;

impl Sheets {
    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut bar = ui::view_toolbar().h_auto().flex_wrap().child(
            ui::chip("sheets-close", "Reports & updates", false).on_click(cx.listener(
                |s, _, _, cx| {
                    if s.clean(cx) {
                        cx.emit(Close);
                    }
                },
            )),
        );
        if self.open.is_none() {
            bar = bar
                .child(
                    ui::chip("sheets-refresh", "Refresh", false)
                        .on_click(cx.listener(|s, _, _, cx| s.catalog(s.cursor.clone(), cx))),
                )
                .child(
                    ui::chip("sheets-create", "New workbook", self.creating).on_click(cx.listener(
                        |s, _, _, cx| {
                            if !s.busy {
                                s.creating = !s.creating;
                                cx.notify();
                            }
                        },
                    )),
                );
        } else {
            bar = bar
                .child(
                    ui::chip("sheets-list", "All Sheets", false)
                        .on_click(cx.listener(|s, _, _, cx| s.catalog(None, cx))),
                )
                .child(
                    ui::chip("sheets-latest", "Review latest", false)
                        .on_click(cx.listener(|s, _, _, cx| s.latest(cx))),
                )
                .child(
                    ui::chip("sheets-history", "History", self.history.is_some())
                        .on_click(cx.listener(|s, _, _, cx| s.load_history(None, cx))),
                )
                .child(
                    ui::chip("sheets-save", "Save changes", self.dirty())
                        .on_click(cx.listener(|s, _, _, cx| s.save(cx))),
                )
                .child(
                    ui::chip("sheets-discard", "Discard draft", false)
                        .on_click(cx.listener(|s, _, _, cx| s.discard(cx))),
                )
                .child(
                    ui::chip("sheets-feedback", "Ask / feedback", self.feedback_open).on_click(
                        cx.listener(|s, _, _, cx| {
                            s.feedback_open = !s.feedback_open;
                            cx.notify();
                        }),
                    ),
                );
            if self
                .draft
                .base
                .as_ref()
                .zip(self.open.as_ref())
                .is_some_and(|(base, o)| base != &o.record.revision_id)
            {
                bar = bar.child(
                    ui::chip("sheets-reapply", "Reapply draft to this revision", false)
                        .on_click(cx.listener(|s, _, _, cx| s.reapply(cx))),
                );
            }
            if !self.draft.cells.is_empty() {
                bar=bar.child(ui::chip("sheets-copy-draft","Copy draft JSON",false).on_click(cx.listener(|s,_,_,cx|{
                if s.stage(cx){let text=serde_json::to_string_pretty(&serde_json::json!({"base_revision_id":s.draft.base,"tab_id":s.draft.tab,"cells":s.draft.cells.values().collect::<Vec<_>>() }));
                    if let Ok(text)=text{cx.write_to_clipboard(ClipboardItem::new_string(text));s.notice=Some("Copied the retained draft with its base revision and stable cell identifiers.".into());cx.notify();}}
            })));
            }
        }
        if self.pending.is_some() {
            bar = bar.child(
                ui::chip("sheets-retry", "Retry the same request", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        if let Some(request) = s.pending.clone() {
                            s.submit(request, cx);
                        }
                    },
                )),
            );
        }
        bar
    }
    pub(super) fn catalog_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (t, c) = (tokens(), colors());
        let mut body = div().flex_1().min_h_0().flex().flex_col();
        if self.creating {
            body = body.child(
                div()
                    .p(px(t.space.s12))
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s6))
                    .child("Workbook title")
                    .child(self.title.clone())
                    .child("Registered owning agent")
                    .child(self.author.clone())
                    .child(
                        ui::button("create-sheet", "Create workbook", ui::Button::Primary)
                            .on_click(cx.listener(|s, _, _, cx| s.create(cx))),
                    ),
            );
        }
        if self.creating {
            let mut imports = ui::view_toolbar()
                .h_auto()
                .flex_wrap()
                .child("Import as a new workbook");
            for format in [Format::Json, Format::Csv, Format::Tsv, Format::Xlsx] {
                imports = imports.child(
                    ui::chip(
                        SharedString::from(format!("import-{}", format.name())),
                        format.name(),
                        false,
                    )
                    .on_click(cx.listener(move |s, _, _, cx| s.import(format, cx))),
                );
            }
            body = body.child(imports);
        }
        let mut list = div()
            .id("sheet-catalog")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for record in &self.catalog {
            let record = record.clone();
            list = list.child(
                ui::row(
                    SharedString::from(format!("sheet-{}", record.sheet_id)),
                    false,
                )
                .h_auto()
                .min_h(px(42.))
                .child(div().flex().flex_col().child(record.title.clone()).child(
                    ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                        "{} · {} tabs · {}",
                        record.author_name,
                        record.tabs.len(),
                        record.updated_at
                    )),
                ))
                .on_click(
                    cx.listener(move |s, _, _, cx| s.open_record(record.clone(), None, 0, 0, cx)),
                ),
            );
        }
        if self.catalog.is_empty() && !self.busy {
            list = list.child(
                ui::text(div().p(px(t.space.s12)).text_color(c.muted), &t.text.body).child(
                    "No Sheets on this page. Create a workbook or ask an agent to publish one.",
                ),
            );
        }
        body.child(list)
            .child(
                ui::view_toolbar()
                    .child(
                        ui::chip("sheets-prev", "Previous", false).on_click(cx.listener(
                            |s, _, _, cx| {
                                if s.clean(cx)
                                    && let Some(cursor) = s.cursors.last().cloned()
                                {
                                    s.catalog(cursor, cx);
                                }
                            },
                        )),
                    )
                    .child(ui::chip("sheets-next", "Next", false).on_click(cx.listener(
                        |s, _, _, cx| {
                            if s.clean(cx)
                                && let Some(next) = s.next.clone()
                            {
                                s.catalog(Some(next), cx);
                            }
                        },
                    ))),
            )
            .into_any_element()
    }
    fn create(&mut self, cx: &mut Context<Self>) {
        if self.busy
            || self.pending.is_some()
            || self.title.read(cx).composing()
            || self.author.read(cx).composing()
        {
            return;
        }
        let Some(client) = self.client.as_ref() else {
            return;
        };
        let create = Create {
            author: self.author.read(cx).value().trim().into(),
            request_id: new_id(),
            title: self.title.read(cx).value().into(),
            workbook: Workbook {
                tabs: vec![NewTab {
                    tab_id: new_id(),
                    name: "Sheet 1".into(),
                    row_ids: (0..100).map(|_| new_id()).collect(),
                    column_ids: (0..12).map(|_| new_id()).collect(),
                    cells: Vec::new(),
                    view: View::default(),
                }],
            },
        };
        match client.prepare_create(&create) {
            Ok(request) => self.submit(request, cx),
            Err(e) => self.fail(e.message(), cx),
        }
    }
    pub(super) fn edit_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let Some(open) = &self.open else {
            return div();
        };
        let mut kinds = div().flex().flex_wrap().gap(px(t.space.s4));
        for kind in [
            Kind::Text,
            Kind::Number,
            Kind::Boolean,
            Kind::Date,
            Kind::Formula,
            Kind::Blank,
        ] {
            kinds = kinds.child(
                ui::chip(
                    SharedString::from(format!("sheet-kind-{}", kind.label())),
                    kind.label(),
                    self.kind == kind,
                )
                .on_click(cx.listener(move |s, _, _, cx| {
                    if s.cell_editable() {
                        s.kind = kind;
                        if kind == Kind::Blank {
                            s.input.update(cx, |f, cx| f.clear(cx));
                        }
                        s.input_dirty = true;
                        cx.notify();
                    }
                })),
            );
        }
        let selected = self
            .selection
            .map(|s| {
                format!(
                    "{}{}",
                    column_name(open.grid.columns[s.end.1].index),
                    open.grid.rows[s.end.0].index
                )
            })
            .unwrap_or_else(|| "Select a cell".into());
        let edit = div()
            .flex()
            .items_center()
            .gap(px(t.space.s8))
            .child(selected)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_h(px(76.))
                    .child(self.input.clone()),
            )
            .child("Number format")
            .child(div().w(px(140.)).child(self.number_format.clone()))
            .child(
                ui::chip("sheet-stage", "Keep in draft", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        s.stage(cx);
                    },
                )),
            )
            .child(
                ui::chip("sheet-copy", "Copy values", false)
                    .on_click(cx.listener(|s, _, _, cx| s.copy(cx))),
            )
            .child(
                ui::chip("sheet-paste", "Paste text", false)
                    .on_click(cx.listener(|s, _, _, cx| s.paste(cx))),
            );
        let view =
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(t.space.s4))
                .child(
                    ui::chip("sheet-sort-asc", "Sort view ↑", false)
                        .on_click(cx.listener(|s, _, _, cx| s.sort(Direction::Asc, cx))),
                )
                .child(
                    ui::chip("sheet-sort-desc", "Sort view ↓", false)
                        .on_click(cx.listener(|s, _, _, cx| s.sort(Direction::Desc, cx))),
                )
                .child(div().w(px(180.)).child(self.filter.clone()))
                .child(
                    ui::chip("sheet-filter", "Apply filter", false)
                        .on_click(cx.listener(|s, _, _, cx| s.filter_contains(cx))),
                )
                .child(
                    ui::chip("sheet-view-clear", "Clear sort / filter", false).on_click(
                        cx.listener(|s, _, _, cx| {
                            if let Some(o) = &s.open {
                                s.operation(
                                    Operation::SetView {
                                        tab_id: o.grid.tab_id.clone(),
                                        view: View::default(),
                                    },
                                    cx,
                                );
                            }
                        }),
                    ),
                )
                .child(ui::text(div().text_color(c.muted), &t.text.meta).child(
                    "Sort view keeps formula coordinates. Filters compare calculated values.",
                ));
        let structure = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s4))
            .child(
                ui::chip("sheet-add-row", "Insert row", false)
                    .on_click(cx.listener(|s, _, _, cx| s.insert(true, cx))),
            )
            .child(
                ui::chip("sheet-add-column", "Insert column", false)
                    .on_click(cx.listener(|s, _, _, cx| s.insert(false, cx))),
            )
            .child(div().w(px(150.)).child(self.title.clone()))
            .child(
                ui::chip("sheet-add-tab", "Add tab", false)
                    .on_click(cx.listener(|s, _, _, cx| s.add_tab(cx))),
            )
            .child(
                ui::chip("sheet-rename-tab", "Rename tab", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        if let Some(o) = &s.open {
                            s.operation(
                                Operation::RenameTab {
                                    tab_id: o.grid.tab_id.clone(),
                                    name: s.title.read(cx).value().into(),
                                },
                                cx,
                            );
                        }
                    },
                )),
            );
        let mut exports = div().flex().flex_wrap().gap(px(t.space.s4)).child("Export");
        for format in [Format::Json, Format::Xlsx, Format::Csv, Format::Tsv] {
            exports = exports.child(
                ui::chip(
                    SharedString::from(format!("sheet-export-{}", format.name())),
                    format.name(),
                    false,
                )
                .on_click(cx.listener(move |s, _, _, cx| s.export(format, cx))),
            );
        }
        div()
            .px(px(t.space.s12))
            .py(px(t.space.s6))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .child(kinds)
            .child(edit)
            .child(self.filter_controls(cx))
            .child(view)
            .child(structure)
            .child(self.structure_controls(cx))
            .child(exports)
    }
    fn sort(&mut self, direction: Direction, cx: &mut Context<Self>) {
        if let Some((open, selection)) = self.open.as_ref().zip(self.selection) {
            let mut view = open
                .record
                .tabs
                .iter()
                .find(|t| t.tab_id == open.grid.tab_id)
                .unwrap()
                .view
                .clone();
            view.sort = vec![Sort {
                column_id: open.grid.columns[selection.end.1].column_id.clone(),
                direction,
            }];
            self.operation(
                Operation::SetView {
                    tab_id: open.grid.tab_id.clone(),
                    view,
                },
                cx,
            );
        }
    }
    fn filter_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let choices = [
            Operator::Eq,
            Operator::Ne,
            Operator::Lt,
            Operator::Lte,
            Operator::Gt,
            Operator::Gte,
            Operator::Contains,
            Operator::IsBlank,
            Operator::IsNotBlank,
        ];
        let label = match self.filter_operator {
            Operator::Eq => "equals",
            Operator::Ne => "not equal",
            Operator::Lt => "less than",
            Operator::Lte => "at most",
            Operator::Gt => "greater than",
            Operator::Gte => "at least",
            Operator::Contains => "contains text",
            Operator::IsBlank => "is blank",
            Operator::IsNotBlank => "is not blank",
        };
        div()
            .flex()
            .flex_wrap()
            .gap(px(tokens().space.s4))
            .child("Filter selected column")
            .child(
                ui::chip("sheet-filter-operator", label, false).on_click(cx.listener(
                    move |s, _, _, cx| {
                        let current = choices
                            .iter()
                            .position(|v| *v == s.filter_operator)
                            .unwrap_or(0);
                        s.filter_operator = choices[(current + 1) % choices.len()];
                        cx.notify();
                    },
                )),
            )
            .child(
                ui::chip("sheet-filter-type", self.filter_kind.label(), false).on_click(
                    cx.listener(|s, _, _, cx| {
                        s.filter_kind = match s.filter_kind {
                            Kind::Text => Kind::Number,
                            Kind::Number => Kind::Boolean,
                            Kind::Boolean => Kind::Date,
                            _ => Kind::Text,
                        };
                        cx.notify();
                    }),
                ),
            )
            .child(
                ui::chip("sheet-header", "Pin selected row as header", false).on_click(
                    cx.listener(|s, _, _, cx| {
                        if let Some((open, selection)) = s.open.as_ref().zip(s.selection) {
                            let mut view = open
                                .record
                                .tabs
                                .iter()
                                .find(|t| t.tab_id == open.grid.tab_id)
                                .unwrap()
                                .view
                                .clone();
                            view.header_row_id =
                                Some(open.grid.rows[selection.end.0].row_id.clone());
                            s.operation(
                                Operation::SetView {
                                    tab_id: open.grid.tab_id.clone(),
                                    view,
                                },
                                cx,
                            );
                        }
                    }),
                ),
            )
    }
    fn filter_contains(&mut self, cx: &mut Context<Self>) {
        if self.filter.read(cx).composing() {
            return;
        }
        let value = if matches!(
            self.filter_operator,
            Operator::IsBlank | Operator::IsNotBlank
        ) {
            None
        } else {
            match self.filter_kind.parse(self.filter.read(cx).value()) {
                Ok(Patch::Set(value)) => Some(value),
                _ => {
                    self.fail("Enter a valid literal filter value.", cx);
                    return;
                }
            }
        };
        if let Some((open, selection)) = self.open.as_ref().zip(self.selection) {
            let mut view = open
                .record
                .tabs
                .iter()
                .find(|t| t.tab_id == open.grid.tab_id)
                .unwrap()
                .view
                .clone();
            let column_id = open.grid.columns[selection.end.1].column_id.clone();
            view.filters.retain(|f| f.column_id != column_id);
            view.filters.push(Filter {
                column_id,
                operator: self.filter_operator,
                value,
            });
            self.operation(
                Operation::SetView {
                    tab_id: open.grid.tab_id.clone(),
                    view,
                },
                cx,
            );
        }
    }
    fn insert(&mut self, rows: bool, cx: &mut Context<Self>) {
        if let Some(open) = &self.open {
            let tab_id = open.grid.tab_id.clone();
            let ids = vec![new_id()];
            let before_id = self.selection.map(|s| {
                if rows {
                    open.grid.rows[s.end.0].row_id.clone()
                } else {
                    open.grid.columns[s.end.1].column_id.clone()
                }
            });
            self.operation(
                if rows {
                    Operation::InsertRows {
                        tab_id,
                        before_id,
                        ids,
                    }
                } else {
                    Operation::InsertColumns {
                        tab_id,
                        before_id,
                        ids,
                    }
                },
                cx,
            );
        }
    }
    fn add_tab(&mut self, cx: &mut Context<Self>) {
        let tab = NewTab {
            tab_id: new_id(),
            name: self.title.read(cx).value().into(),
            row_ids: (0..100).map(|_| new_id()).collect(),
            column_ids: (0..12).map(|_| new_id()).collect(),
            cells: Vec::new(),
            view: View::default(),
        };
        self.operation(Operation::AddTab { tab }, cx);
    }
    fn load_history(&mut self, cursor: Option<String>, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((client, open)) = self.client.clone().zip(self.open.as_ref()) else {
            return;
        };
        let sheet = open.record.sheet_id.clone();
        let requested = cursor.clone();
        self.begin(cx);
        let work = cx
            .background_executor()
            .spawn(async move { client.history(&sheet, requested.as_deref()) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(page) => {
                        commit_cursor(&mut s.history_cursors, &s.history_cursor, &cursor);
                        s.history = Some(page);
                        s.history_cursor = cursor;
                    }
                    Err(e) => s.error = Some(e.message().into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn history_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        ui::view_toolbar()
            .child(
                ui::chip("sheet-history-back", "Back to cells", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        s.history = None;
                        cx.notify();
                    },
                )),
            )
            .child(
                ui::chip("sheet-history-prev", "Previous", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        if s.clean(cx)
                            && let Some(cursor) = s.history_cursors.last().cloned()
                        {
                            s.load_history(cursor, cx);
                        }
                    },
                )),
            )
            .child(
                ui::chip("sheet-history-next", "Next", false).on_click(cx.listener(
                    |s, _, _, cx| {
                        if s.clean(cx)
                            && let Some(next) =
                                s.history.as_ref().and_then(|p| p.next_cursor.clone())
                        {
                            s.load_history(Some(next), cx);
                        }
                    },
                )),
            )
    }
    pub(super) fn feedback_receipt_controls(
        &self,
        receipt: &FeedbackReceipt,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let receipt = receipt.clone();
        ui::view_toolbar()
            .h_auto()
            .flex_wrap()
            .child(format!("Last feedback: {}", receipt.delivery.message()))
            .child(
                ui::chip("sheet-feedback-receipt", "Copy feedback receipt", false).on_click(
                    cx.listener(move |_, _, _, cx| {
                        if let Ok(text) = serde_json::to_string_pretty(&receipt) {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    }),
                ),
            )
    }
    pub(super) fn feedback_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        div()
            .p(px(t.space.s10))
            .border_t_1()
            .border_color(c.line)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .child("Ask about the selected cells at the displayed revision (up to 100 cells).")
            .child(self.feedback.clone())
            .child(
                ui::chip("sheet-clear-feedback", "Clear feedback draft", false).on_click(
                    cx.listener(|s, _, _, cx| {
                        if !s.busy && s.pending.is_none() {
                            s.feedback.update(cx, |f, cx| f.clear(cx));
                            cx.notify();
                        }
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .gap(px(t.space.s6))
                    .child(
                        ui::chip(
                            "sheet-question",
                            "Question",
                            matches!(self.feedback_kind, FeedbackKind::Question),
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.feedback_kind = FeedbackKind::Question;
                            cx.notify();
                        })),
                    )
                    .child(
                        ui::chip(
                            "sheet-feedback-kind",
                            "Feedback",
                            matches!(self.feedback_kind, FeedbackKind::Feedback),
                        )
                        .on_click(cx.listener(|s, _, _, cx| {
                            s.feedback_kind = FeedbackKind::Feedback;
                            cx.notify();
                        })),
                    )
                    .child(
                        ui::button("sheet-send-feedback", "Send to owner", ui::Button::Primary)
                            .on_click(cx.listener(|s, _, _, cx| s.send_feedback(cx))),
                    ),
            )
    }
    fn send_feedback(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.pending.is_some() || self.feedback.read(cx).composing() {
            return;
        }
        if self.dirty() {
            self.fail(
                "Save or discard cell edits before sending feedback about committed values.",
                cx,
            );
            return;
        }
        let Some((client, open)) = self.client.as_ref().zip(self.open.as_ref()) else {
            return;
        };
        let anchor = self.selection.map(|selection| {
            let (rows, columns) = selection.bounds();
            Anchor {
                tab_id: open.grid.tab_id.clone(),
                row_ids: rows.map(|r| open.grid.rows[r].row_id.clone()).collect(),
                column_ids: columns
                    .map(|c| open.grid.columns[c].column_id.clone())
                    .collect(),
            }
        });
        let feedback = api::Feedback::operator(
            new_id(),
            open.record.revision_id.clone(),
            self.feedback_kind,
            self.feedback.read(cx).value().into(),
            anchor,
        );
        match client.prepare_feedback(&open.record.sheet_id, &feedback) {
            Ok(request) => self.submit(request, cx),
            Err(e) => self.fail(e.message(), cx),
        }
    }
    fn import(&mut self, format: Format, cx: &mut Context<Self>) {
        if !self.clean(cx) || self.title.read(cx).composing() || self.author.read(cx).composing() {
            return;
        }
        let Some(client) = self.client.clone() else {
            return;
        };
        let author = self.author.read(cx).value().trim().to_owned();
        let title = self.title.read(cx).value().to_owned();
        if author.is_empty() || title.trim().is_empty() {
            self.fail(
                "Enter a workbook title and its registered owning agent first.",
                cx,
            );
            return;
        }
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("Import {} as a new workbook", format.name()).into()),
        });
        self.begin(cx);
        cx.spawn(async move |this, cx| {
            let path = match picker.await {
                Ok(Ok(Some(paths))) if paths.len() == 1 => paths[0].clone(),
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |s, cx| {
                        s.busy = false;
                        cx.notify();
                    });
                    return;
                }
                _ => {
                    let _ = this.update(cx, |s, cx| {
                        s.busy = false;
                        s.fail("Could not choose an import file.", cx);
                    });
                    return;
                }
            };
            let work = cx.background_executor().spawn(async move {
                use base64::Engine;
                use std::{io::Read, os::unix::fs::OpenOptionsExt};
                let limit = if matches!(format, Format::Xlsx) {
                    8 * 1024 * 1024
                } else {
                    16 * 1024 * 1024
                };
                let file = std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(path)
                    .map_err(|_| "Could not open the import file.")?;
                if !file
                    .metadata()
                    .map_err(|_| "Could not inspect the import file.")?
                    .is_file()
                {
                    return Err("Choose a regular file.");
                }
                let mut bytes = Vec::new();
                file.take(limit + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "Could not read the import file.")?;
                if bytes.len() as u64 > limit {
                    return Err("Import file exceeds the supported size.");
                }
                let content = if matches!(format, Format::Xlsx) {
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                } else {
                    String::from_utf8(bytes).map_err(|_| "Choose a UTF-8 file for this format.")?
                };
                client
                    .prepare_import(&Import {
                        author,
                        request_id: new_id(),
                        title,
                        format,
                        content,
                    })
                    .map_err(|e| e.message())
            });
            let result = work.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(request) => s.submit(request, cx),
                    Err(error) => s.fail(error, cx),
                }
            });
        })
        .detach();
    }
    fn export(&mut self, format: Format, cx: &mut Context<Self>) {
        if !self.clean(cx) {
            return;
        }
        let Some((client, open)) = self.client.clone().zip(self.open.as_ref()) else {
            return;
        };
        let sheet = open.record.sheet_id.clone();
        let revision = open.record.revision_id.clone();
        let tab = open.grid.tab_id.clone();
        let directory = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let picker =
            cx.prompt_for_new_path(&directory, Some(&format!("workbook.{}", format.name())));
        self.begin(cx);
        cx.spawn(async move |this, cx| {
            let path = match picker.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |s, cx| {
                        s.busy = false;
                        cx.notify();
                    });
                    return;
                }
                _ => {
                    let _ = this.update(cx, |s, cx| {
                        s.busy = false;
                        s.fail("Could not open the save dialog.", cx);
                    });
                    return;
                }
            };
            let worker = cx.background_executor().spawn(async move {
                use std::io::Write;
                let bytes = client
                    .export(&sheet, &revision, format, Some(&tab))
                    .map_err(|e| e.message())?;
                let mut file =
                    tempfile::NamedTempFile::new_in(path.parent().ok_or("Invalid destination.")?)
                        .map_err(|_| "Could not create export file.")?;
                file.write_all(&bytes.bytes)
                    .map_err(|_| "Could not write export file.")?;
                file.persist(&path)
                    .map_err(|_| "Could not finish saving export file.")?;
                Ok::<_, &str>(bytes.warnings)
            });
            let result = worker.await;
            let _ = this.update(cx, |s, cx| {
                s.busy = false;
                match result {
                    Ok(warnings) => {
                        s.notice = Some(if !warnings.is_empty() {
                            format!("Exported with warnings: {}", warnings.join("; "))
                        } else if matches!(format, Format::Csv | Format::Tsv) {
                            "Exported one tab as values only.".into()
                        } else {
                            "Exported the displayed immutable revision.".into()
                        });
                    }
                    Err(e) => s.error = Some(e.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }
}
