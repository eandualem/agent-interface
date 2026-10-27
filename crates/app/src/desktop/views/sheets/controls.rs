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
            bar = bar.child(
                ui::chip("sheets-refresh", "Refresh", false)
                    .on_click(cx.listener(|s, _, _, cx| s.catalog(s.cursor.clone(), cx))),
            );
        } else {
            bar = bar
                .child(
                    ui::chip("sheets-list", "All Sheets", false)
                        .on_click(cx.listener(|s, _, _, cx| s.catalog(None, cx))),
                )
                .child(
                    ui::chip("sheets-latest", "Refresh", false)
                        .on_click(cx.listener(|s, _, _, cx| s.latest(cx))),
                )
                .child(
                    ui::chip("sheets-history", "History", self.history.is_some())
                        .on_click(cx.listener(|s, _, _, cx| s.load_history(None, cx))),
                )
                .child(
                    ui::chip("sheets-copy", "Copy values", false)
                        .on_click(cx.listener(|s, _, _, cx| s.copy(cx))),
                )
                .child(
                    ui::chip("sheets-export", "Export", self.export_open).on_click(cx.listener(
                        |s, _, _, cx| {
                            s.export_open = !s.export_open;
                            cx.notify();
                        },
                    )),
                );
            if self.export_open {
                for format in [Format::Json, Format::Xlsx, Format::Csv, Format::Tsv] {
                    bar = bar.child(
                        ui::chip(
                            SharedString::from(format!("sheet-export-{}", format.name())),
                            format.name(),
                            false,
                        )
                        .on_click(cx.listener(move |s, _, _, cx| s.export(format, cx))),
                    );
                }
            }
        }
        bar
    }
    pub(super) fn catalog_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (t, c) = (tokens(), colors());
        let body = div().flex_1().min_h_0().flex().flex_col();
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
                ui::text(div().p(px(t.space.s12)).text_color(c.muted), &t.text.body)
                    .child("No Sheets on this page. Ask an agent to publish a workbook."),
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
                        if !s.busy {
                            s.history = None;
                            cx.notify();
                        }
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
    pub(super) fn send_feedback(&mut self, cx: &mut Context<Self>) {
        if self.busy || self.pending.is_some() || self.feedback.read(cx).value(cx).trim().is_empty()
        {
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
            FeedbackKind::Feedback,
            self.feedback.read(cx).value(cx).into(),
            anchor,
        );
        match client.prepare_feedback(&open.record.sheet_id, &feedback) {
            Ok(request) => self.submit(request, cx),
            Err(e) => self.feedback.update(cx, |f, cx| {
                f.complete(Outcome::Refused(e.message().into()), cx)
            }),
        }
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
