//! The original text is selectable plain text, never reconstructed from its
//! preview and never rewritten by the separately anchored discussion.
use super::*;
use crate::desktop::views::clock::Local;
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};
use gpui::{AnyElement, SharedString, div, px};

pub(super) fn body(
    view: &Interactions,
    _window: &mut Window,
    cx: &mut Context<Interactions>,
) -> AnyElement {
    let Some(item) = view.selected.clone() else {
        return div().into_any_element();
    };
    let (t, c) = (tokens(), colors());
    let agent = view.agent.as_deref().unwrap_or_default();
    let peer = counterparty(&item, agent);
    let header = header(view, &item, cx);
    let mut column = ui::reader_column().gap(px(t.space.s14));
    let in_list = view
        .filtered()
        .iter()
        .any(|i| view.items()[*i].id == item.id);
    if !in_list {
        column=column.child(ui::notice("info","This message isn’t in the list for the current window or filter. It stays open here until you choose another.",None));
    }
    if epoch(&item.accepted_at) < view.window.start && in_list {
        column = column.child(ui::notice(
            "clock",
            format!(
                "Sent {}, before this window; its recorded delivery lifecycle overlaps the window.",
                render::time(epoch(&item.accepted_at))
            ),
            None,
        ));
    }
    let same_second = view
        .items()
        .iter()
        .filter(|other| epoch(&other.accepted_at).floor() == epoch(&item.accepted_at).floor())
        .count();
    if same_second > 1 {
        column = column.child(ui::notice("clock", format!("{same_second} messages were recorded in this second. Their order here does not imply an order within the second."), None));
    }
    if item.sender.kind == "unknown" {
        column = column.child(ui::notice(
            "circle-help",
            "The sender was not recorded. No agent is inferred.",
            None,
        ));
    } else if item.sender.kind == "owner" {
        column = column.child(ui::text(
            div()
                .text_color(c.muted)
                .child("Sent by you through Agent Backbone."),
            &t.text.meta,
        ));
    }
    if let Some(error) = view.link_error {
        column = column.child(ui::notice("info", error, None));
    }
    if let Some(error) = view.detail_error {
        column = column.child(ui::notice("info", error.message(), None));
    }
    let retained = item.body.text.as_deref().unwrap_or(&item.preview);
    let message = ui::message_frame(selection::text(view, retained, &[], cx));
    column = column
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .child(ui::reader_label("ORIGINAL MESSAGE"))
                .when(item.body.level == "full", |row| {
                    row.child(ui::text(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(c.muted)
                            .child("kept in full, exactly as sent"),
                        &t.text.meta,
                    ))
                })
                .child(div().flex_1())
                .child(
                    ui::interaction_icon_button(
                        "interaction-copy",
                        "copy",
                        "Copy retained message text",
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        if let Some(text) = view
                            .selected
                            .as_ref()
                            .map(|m| m.body.text.as_deref().unwrap_or(&m.preview))
                        {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_owned()));
                        }
                    })),
                ),
        )
        .child(message);
    if item.body.level != "full" {
        column=column.child(ui::notice("link-2-off",
            if item.body.level=="excerpt" {"Full message not retained. This record kept only the opening shown above. The rest can’t be recovered."}
            else {"Full message not retained. Only the recorded preview is available."},None));
    }
    let mut delivery = div().flex().flex_col();
    for event in item
        .lifecycle
        .items
        .iter()
        .filter(|event| event.kind != "attempt")
    {
        let failed = event
            .outcome
            .as_deref()
            .is_some_and(|s| matches!(s, "failed" | "cancelled" | "not_taken"));
        let pending = event
            .outcome
            .as_deref()
            .is_some_and(|s| matches!(s, "pending" | "queued" | "offered"));
        let color = if failed { c.attention } else { c.text_2 };
        let label = match event.kind.as_str() {
            "accepted" => "Accepted",
            "queued" => "Queued",
            "delivered" => "Delivered to the runtime",
            "acknowledged" => "Acknowledged through the inbox",
            "failed" => "Failed",
            other => other,
        };
        delivery = delivery.child(ui::text(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .min_h(px(t.size.interaction_lifecycle_row))
                .child(
                    div()
                        .size(px(t.size.interaction_lifecycle_mark))
                        .rounded_full()
                        .border(px(t.size.hairline))
                        .border_color(color)
                        .when(!pending, |el| el.bg(color)),
                )
                .child(label.to_owned())
                .children(
                    event
                        .outcome
                        .clone()
                        .map(|o| div().text_color(color).child(o)),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .font_family(t.font.mono.clone())
                        .text_color(c.muted)
                        .child(format!(
                            "{}{}",
                            render::time(epoch(&event.time)),
                            if event.time_basis == "observed" {
                                " · observed"
                            } else {
                                ""
                            }
                        )),
                ),
            &t.text.meta,
        ));
    }
    if item.lifecycle.items.is_empty() {
        delivery = delivery.child("Delivery steps not recorded.");
    }
    if item.lifecycle.next_cursor.is_some() {
        delivery = delivery.child("More delivery steps exist than this view can load.");
    }
    column=column.child(section("DELIVERY", "as recorded", delivery))
        .child(ui::text(div().text_color(c.muted).child("Delivery and acknowledgement show that a message reached its destination. They do not show understanding or action."),&t.text.meta));
    let relations = recorded_relations(&item, view.items());
    if !relations.is_empty() {
        let mut links = div().flex().flex_col().gap(px(t.space.s6));
        for (n, (relation, inverse)) in relations.into_iter().enumerate() {
            let label = match (relation.relation.as_str(), inverse) {
                ("reply_to", false) => "Reply to",
                ("reply_to", true) => "Reply",
                ("corrects", false) => "Corrects",
                ("corrects", true) => "Corrected by",
                ("for_owner_request", false) => "For your request",
                ("for_owner_request", true) => "Delegated as",
                ("asked_in_conversation", _) => "Asked for in your conversation",
                ("from_report", _) => "Report",
                _ => "Recorded link",
            };
            let target = relation.target;
            let name = view
                .items()
                .iter()
                .find(|m| m.id == target.id)
                .map(|m| {
                    format!(
                        "{} → {} · {}",
                        actor_name(&m.sender),
                        actor_name(&m.recipient),
                        render::time(epoch(&m.accepted_at))
                    )
                })
                .unwrap_or_else(|| format!("{} {}", target.kind, target.id));
            let id = target.id.clone();
            let mut link = ui::interaction_chip(
                SharedString::from(format!("interaction-link-{n}")),
                format!("{label} · {name}"),
                false,
            );
            if matches!(target.kind.as_str(), "interaction" | "owner_request") {
                link =
                    link.on_click(cx.listener(move |view, _, _, cx| view.choose(id.clone(), cx)));
            } else if target.kind == "conversation"
                && target.artifact_kind.as_deref() == Some("interaction")
            {
                link = link.on_click(
                    cx.listener(move |view, _, _, cx| view.open_discussion(id.clone(), cx)),
                );
            }
            if target.kind == "conversation"
                && let Some(kind) = target
                    .artifact_kind
                    .as_deref()
                    .and_then(conversation_kind)
                    .filter(|kind| {
                        *kind != agent_interface_roster::conversations::Kind::Interaction
                    })
            {
                let id = target.id.clone();
                link = link.on_click(cx.listener(move |view, _, window, cx| {
                    view.open_reference(kind, id.clone(), window, cx)
                }));
            }
            if target.kind == "report"
                && let Ok(id) = target.id.parse::<u64>()
            {
                let response = target.artifact_kind.as_deref() == Some("response");
                link = link.on_click(
                    cx.listener(move |view, _, _, cx| view.open_report(id, response, cx)),
                );
            }
            links = links.child(link);
        }
        column = column.child(section("LINKED", "links present in the record", links));
    }
    if item.relations_truncated {
        column = column.child(ui::notice(
            "info",
            "Additional recorded links exist beyond this page’s limit.",
            None,
        ));
    }
    column = column.child(
        ui::interaction_chip(
            "interaction-details",
            if view.technical {
                "Hide technical details"
            } else {
                "Technical details"
            },
            view.technical,
        )
        .on_click(cx.listener(|view, _, _, cx| {
            view.technical = !view.technical;
            cx.notify();
        })),
    );
    if view.technical {
        let details = format!(
            "Message {}\nOperation {}\nOriginal target {}\nSender attribution {}\nRetention {}{}\nStatus evidence {}\n{} recorded attempts",
            item.id,
            item.operation_id,
            item.original_target,
            item.sender.attribution,
            item.body.level,
            item.body
                .reason
                .as_ref()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default(),
            item.status.evidence,
            item.attempt_count
        );
        column =
            column.child(selection::text(view, &details, &[], cx).font_family(t.font.mono.clone()));
        for attempt in &item.attempts.items {
            column = column.child(
                selection::text(
                    view,
                    &format!(
                        "{} · {} · {} · {}",
                        attempt.id, attempt.source, attempt.outcome, attempt.recorded_at
                    ),
                    &[],
                    cx,
                )
                .font_family(t.font.mono.clone()),
            );
        }
        if item.attempts.next_cursor.is_some() {
            column = column.child("Additional attempts were not loaded.");
        }
    }
    let mut actions = div().flex().flex_wrap().items_center().gap(px(t.space.s8));
    if item.conversation.supported {
        actions = actions.child(
            ui::interaction_primary_button("interaction-ask", "Ask about this message")
                .on_click(cx.listener(|view, _, window, cx| view.ask(window, cx))),
        );
    }
    if peer.kind == "agent" && peer.id.is_some() {
        actions = actions.child(
            ui::interaction_action(
                "interaction-pivot",
                "locate-fixed",
                format!("Center {}’s messages", actor_name(peer)),
            )
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .on_click(cx.listener(|view, _, _, cx| view.pivot(cx))),
        );
    }
    column = column.child(actions);
    div()
        .size_full()
        .min_w_0()
        .flex()
        .flex_col()
        .border_l(px(t.size.hairline))
        .border_color(c.line)
        .child(header)
        .child(
            div()
                .id("interaction-reader")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(t.space.s18))
                .child(column),
        )
        .into_any_element()
}

/// Direction and participants share one header row; the timestamp belongs
/// immediately beneath it, while navigation remains independent of scrolling.
fn header(view: &Interactions, item: &Interaction, cx: &mut Context<Interactions>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let sent = outgoing(item, view.agent.as_deref().unwrap_or_default());
    let indexes = view.filtered();
    let position = indexes
        .iter()
        .position(|index| view.items()[*index].id == item.id);
    let mut navigation = div()
        .flex()
        .items_center()
        .gap(px(t.space.s6))
        .flex_shrink_0();
    if let Some(position) = position {
        navigation = navigation.child(ui::text(
            div()
                .font_family(t.font.mono.clone())
                .text_color(c.muted)
                .child(format!("{} of {}", position + 1, indexes.len())),
            &t.text.meta,
        ));
    }
    for (id, icon, delta) in [
        ("interaction-previous", "chevron-up", -1),
        ("interaction-next", "chevron-down", 1),
    ] {
        let next = position.and_then(|position| {
            usize::try_from(position as isize + delta)
                .ok()
                .and_then(|next| indexes.get(next))
                .map(|index| view.items()[*index].id.clone())
        });
        navigation = navigation.child(
            ui::interaction_icon_button(
                id,
                icon,
                if delta < 0 {
                    "Previous message"
                } else {
                    "Next message"
                },
            )
            .when(next.is_none(), |button| {
                button
                    .opacity(0.35)
                    .aria_description("No message in this direction")
            })
            .on_click(cx.listener(move |view, _, _, cx| {
                if let Some(id) = &next {
                    view.choose(id.clone(), cx);
                }
            })),
        );
    }
    let when = Local::at(epoch(&item.accepted_at))
        .map(|date| format!("{}, {} · {}", date.date(), date.seconds(), date.offset()))
        .unwrap_or_default();
    let mut header = div()
        .flex_shrink_0()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s8))
        .p(px(t.space.s18))
        .border_b(px(t.size.hairline))
        .border_color(c.line);
    if view.narrow_reader(cx) {
        header = header.child(
            ui::interaction_back_button("interaction-reader-back", "Messages").on_click(
                cx.listener(|view, _, _, cx| {
                    view.detail_generation += 1;
                    view.detail_dirty = None;
                    view.selected = None;
                    view.close_conversation(cx);
                    cx.notify();
                }),
            ),
        );
    }
    header
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s10))
                .child(ui::direction_tag(sent).flex_shrink_0())
                .child(ui::text(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_family(t.font.mono.clone())
                        .text_color(c.text)
                        .child(format!(
                            "{} → {}",
                            actor_name(&item.sender),
                            actor_name(&item.recipient)
                        )),
                    &t.text.row,
                ))
                .child(navigation),
        )
        .child(ui::text(
            div()
                .text_color(c.muted)
                .child(format!("{} {when}", if sent { "Sent" } else { "Received" })),
            &t.text.meta,
        ))
}

/// Message sections keep the label above their content, matching the original
/// and leaving the full reader width available for delivery evidence and links.
fn section(label: &'static str, note: &'static str, body: impl IntoElement) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .flex_col()
        .gap(px(t.space.s14))
        .py(px(t.space.s14))
        .border_t(px(t.size.hairline))
        .border_color(c.line)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .child(ui::reader_label(label))
                .child(ui::text(
                    div().text_color(c.muted).child(note),
                    &t.text.meta,
                )),
        )
        .child(body)
}

/// A provider may project an actual incoming link onto its target, including
/// when its source lies outside this window. Preserve its source and direction.
fn recorded_relations(item: &Interaction, visible: &[Interaction]) -> Vec<(api::Relation, bool)> {
    let mut links = Vec::new();
    let mut seen = HashSet::new();
    for record in std::iter::once(item).chain(visible) {
        for relation in &record.relations {
            let source = relation
                .source_interaction_id
                .as_deref()
                .unwrap_or(&record.id);
            let incoming = source != item.id
                && matches!(
                    relation.target.kind.as_str(),
                    "interaction" | "owner_request"
                )
                && relation.target.id == item.id;
            if source != item.id && !incoming {
                continue;
            }
            if !seen.insert((
                source.to_owned(),
                relation.relation.clone(),
                relation.target.kind.clone(),
                relation.target.id.clone(),
            )) {
                continue;
            }
            let mut link = relation.clone();
            if incoming {
                link.target.kind = "interaction".into();
                link.target.id = source.into();
            }
            links.push((link, incoming));
        }
    }
    links
}

fn conversation_kind(kind: &str) -> Option<agent_interface_roster::conversations::Kind> {
    use agent_interface_roster::conversations::Kind;
    Some(match kind {
        "interaction" => Kind::Interaction,
        "document" => Kind::Document,
        "sheet" => Kind::Sheet,
        "report" => Kind::Report,
        "response" => Kind::Response,
        "plan" => Kind::Plan,
        "skill" => Kind::Skill,
        "policy" => Kind::Policy,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_reverse_links_keep_their_source_without_duplicates() {
        let mut original: Interaction = serde_json::from_str(include_str!(
            "../../../../../roster/tests/fixtures/interactions/detail.json"
        ))
        .unwrap();
        original.relations.clear();
        let mut child = original.clone();
        child.id = "child".into();
        let relation = api::Relation {
            source_interaction_id: Some("child".into()),
            relation: "for_owner_request".into(),
            target: api::RelationTarget {
                kind: "owner_request".into(),
                id: original.id.clone(),
                artifact_kind: None,
                entry_id: None,
            },
            recorded_at: original.accepted_at.clone(),
            provenance: "recorded".into(),
        };
        child.relations.push(relation.clone());
        original.relations.push(relation);
        let links = recorded_relations(&original, &[child]);
        assert_eq!(links.len(), 1);
        assert!(links[0].1);
        assert_eq!(links[0].0.target.id, "child");
        assert_eq!(links[0].0.source_interaction_id.as_deref(), Some("child"));
    }
}
