use super::*;
use gpui::ClipboardItem;

impl Sheets {
    /// An open workbook's bar: back to the shelf, its title, the revision
    /// being read, and Copy values and Export on the right. The shelf has
    /// none; the section bar serves it.
    pub(super) fn toolbar(&self, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let open = self.open.as_ref()?;
        let (t, c) = (tokens(), colors());
        let latest = open.record.current_revision_id == open.record.revision_id;
        let revision = open.record.revision_id.to_string();
        let label = format!("rev {}", &revision[..revision.len().min(8)]);
        let mut export = ui::action("sheet-export", "download", "Export")
            .child(ui::icon("chevron-down", t.size.icon - 2.0, c.muted))
            .on_click(cx.listener(|s, _, _, cx| {
                s.export_open = !s.export_open;
                cx.notify();
            }));
        if self.export_open {
            let mut menu = ui::menu()
                .absolute()
                .top(px(t.size.action + t.space.s4))
                .right_0()
                .id("sheet-export-menu")
                .occlude()
                .on_mouse_down_out(cx.listener(|s, _, _, cx| {
                    s.export_open = false;
                    cx.notify();
                }));
            for format in [Format::Json, Format::Xlsx, Format::Csv, Format::Tsv] {
                menu = menu.child(
                    ui::menu_item(
                        SharedString::from(format!("sheet-export-{}", format.name())),
                        None,
                        format.name(),
                        None,
                    )
                    .on_click(cx.listener(move |s, _, _, cx| {
                        cx.stop_propagation();
                        s.export_open = false;
                        s.export(format, cx);
                        cx.notify();
                    })),
                );
            }
            export = export
                .relative()
                .child(gpui::deferred(menu).with_priority(1));
        }
        Some(
            ui::view_toolbar()
                .child(
                    ui::back_button("sheets-back", "Sheets").on_click(cx.listener(
                        |s, _, _, cx| {
                            s.export_open = false;
                            if s.clean(cx) {
                                s.catalog(cx);
                            }
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
                            .text_color(c.text)
                            .font_weight(t.text.row_selected.weight()),
                        &t.text.row,
                    )
                    .child(open.record.title.clone()),
                )
                .child(ui::revision_stepper(
                    ui::revision_step("sheet-older", "chevron-left", true)
                        .on_click(cx.listener(|s, _, _, cx| s.load_history(None, cx))),
                    label,
                    latest,
                    ui::revision_step("sheet-newer", "chevron-right", !latest).on_click(
                        cx.listener(move |s, _, _, cx| {
                            if !latest {
                                s.latest(cx);
                            }
                        }),
                    ),
                ))
                .child(div().flex_1())
                .child(
                    ui::action(
                        "sheet-copy",
                        "copy",
                        if self.selection.is_some() {
                            "Copy selected values"
                        } else {
                            "Copy displayed values"
                        },
                    )
                    .on_click(cx.listener(|s, _, _, cx| s.copy(cx))),
                )
                .child(export),
        )
    }

    /// Bring the shelf or the open workbook up to date.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.open.is_some() {
            self.latest(cx);
        } else {
            self.catalog(cx);
        }
    }

    /// Whether a workbook is open rather than the shelf.
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub(super) fn catalog_body(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let (t, c) = (tokens(), colors());
        let body = div().flex_1().min_h_0().flex().flex_col();
        let mut shelf = ui::shelf();
        let mut records: Vec<_> = self.catalog.iter().collect();
        records.sort_by_cached_key(|record| {
            let [revision, _] = super::seen_keys(record);
            shelf_order(
                super::super::seen::is_seen_key(&revision, &record.updated_at),
                &record.updated_at,
                &record.sheet_id.to_string(),
            )
        });
        for record in records {
            let [revision, thing] = super::seen_keys(record);
            let mut tags = Vec::new();
            if !super::super::seen::is_seen_key(&revision, &record.updated_at) {
                let label = if super::super::seen::known(&thing) {
                    "updated"
                } else {
                    "new"
                };
                tags.push(ui::tag(label, ui::Tone::Tag));
            }
            let tabs = record.tabs.len();
            let record = record.clone();
            shelf = shelf.child(
                ui::shelf_card(
                    SharedString::from(format!("sheet-{}", record.sheet_id)),
                    "sheet",
                    false,
                    tags,
                    record.title.clone(),
                    record
                        .agent_name
                        .clone()
                        .unwrap_or_else(|| record.author_name.clone()),
                    format!(
                        "{} ago",
                        super::super::since(agent_interface_roster::views::epoch_seconds(
                            &record.updated_at
                        ))
                    ),
                    format!("{tabs} {}", if tabs == 1 { "tab" } else { "tabs" }),
                )
                .on_click(
                    cx.listener(move |s, _, _, cx| s.open_record(record.clone(), None, 0, 0, cx)),
                ),
            );
        }
        let mut list = ui::grouped_page(
            "sheet-catalog",
            div()
                .child(ui::group_label(
                    div()
                        .text_color(c.text_2)
                        .font_weight(t.text.row_selected.weight())
                        .child("Workbooks"),
                    "unread first · newest updates first · read-only",
                ))
                .child(shelf),
        );
        if self.catalog.is_empty() && !self.busy {
            list = list.child(
                ui::text(div().p(px(t.space.s12)).text_color(c.muted), &t.text.body)
                    .child("No Sheets yet. Ask an agent to publish a workbook."),
            );
        }
        body.child(list).into_any_element()
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
    pub(super) fn send_feedback(
        &mut self,
        conversation: Entity<Conversation>,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.pending.is_some() {
            conversation.update(cx, |c, cx| {
                c.complete(
                    index,
                    Outcome::Refused("Wait for the earlier message to finish.".into()),
                    cx,
                )
            });
            return;
        }
        let Some((client, open)) = self.client.as_ref().zip(self.open.as_ref()) else {
            conversation.update(cx, |c, cx| {
                c.complete(index, Outcome::Refused("No workbook is open.".into()), cx)
            });
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
            conversation.read(cx).text(index).into(),
            anchor,
        );
        match client.prepare_feedback(&open.record.sheet_id, &feedback) {
            Ok(request) => self.submit(request, conversation.clone(), index, cx),
            Err(e) => conversation.update(cx, |c, cx| {
                c.complete(index, Outcome::Refused(e.message().into()), cx)
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

/// Unread revisions lead, with the newest update first in each group.
fn shelf_order(seen: bool, updated: &str, id: &str) -> (bool, std::cmp::Reverse<i64>, String) {
    (
        seen,
        std::cmp::Reverse(
            agent_interface_roster::views::epoch_seconds(updated).unwrap_or(0.0) as i64,
        ),
        id.into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_workbooks_lead_and_each_group_is_newest_first() {
        let mut rows = [
            shelf_order(true, "2026-10-28T12:00:00Z", "seen"),
            shelf_order(false, "2026-10-28T10:00:00Z", "older unread"),
            shelf_order(false, "2026-10-28T11:00:00Z", "newer unread"),
        ];
        rows.sort();
        assert_eq!(
            rows.map(|row| row.2),
            ["newer unread", "older unread", "seen"]
        );
    }
}
