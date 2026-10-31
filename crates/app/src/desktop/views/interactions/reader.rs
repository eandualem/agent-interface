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
    let sent = outgoing(&item, agent);
    let peer = counterparty(&item, agent);
    let mut actions = ui::view_toolbar()
        .h_auto()
        .min_h(px(t.size.view_toolbar))
        .flex_wrap()
        .py(px(t.space.s6))
        .child(ui::direction_tag(sent))
        .child(div().flex_1())
        .child(
            ui::interaction_icon_button("interaction-copy", "copy", "Copy retained message text")
                .aria_label("Copy retained message text")
                .on_click(cx.listener(|view, _, _, cx| {
                    if let Some(text) = view
                        .selected
                        .as_ref()
                        .map(|m| m.body.text.as_deref().unwrap_or(&m.preview))
                    {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_owned()));
                    }
                })),
        );
    if view.narrow_reader(cx) {
        actions = actions.child(
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
    for (id, icon, delta) in [
        ("interaction-previous", "chevron-left", -1),
        ("interaction-next", "chevron-right", 1),
    ] {
        actions = actions.child(
            ui::interaction_icon_button(
                id,
                icon,
                if delta < 0 {
                    "Previous message"
                } else {
                    "Next message"
                },
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                let indexes = view.filtered();
                let position = indexes.iter().position(|i| {
                    view.selected
                        .as_ref()
                        .is_some_and(|s| s.id == view.items()[*i].id)
                });
                if let Some(position) = position {
                    let next = position as isize + delta;
                    if next >= 0
                        && let Some(index) = indexes.get(next as usize)
                    {
                        view.choose(view.items()[*index].id.clone(), cx);
                    }
                }
            })),
        );
    }
    let when = Local::at(epoch(&item.accepted_at))
        .map(|date| format!("{}, {} · {}", date.date(), date.seconds(), date.offset()))
        .unwrap_or_default();
    let mut column = ui::reader_column()
        .gap(px(t.space.s14))
        .child(ui::text(
            div().text_color(c.text).child(format!(
                "{} → {}",
                actor_name(&item.sender),
                actor_name(&item.recipient)
            )),
            &t.text.doc_heading,
        ))
        .child(ui::text(
            div()
                .text_color(c.muted)
                .child(format!("{} {when}", if sent { "Sent" } else { "Received" })),
            &t.text.meta,
        ));
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
        .child(ui::reader_label("ORIGINAL MESSAGE"))
        .child(message);
    if item.body.level != "full" {
        column=column.child(ui::notice("link-2-off",
            if item.body.level=="excerpt" {"Full message not retained. This record kept only the opening shown above. The rest can’t be recovered."}
            else {"Full message not retained. Only the recorded preview is available."},None));
    }
    let mut delivery = div().flex().flex_col();
    for event in &item.lifecycle.items {
        let attempt = event.kind == "attempt";
        let failed = event
            .outcome
            .as_deref()
            .is_some_and(|s| matches!(s, "failed" | "cancelled" | "not_taken"));
        let pending = event
            .outcome
            .as_deref()
            .is_some_and(|s| matches!(s, "pending" | "queued" | "offered"));
        let color = if failed {
            c.attention
        } else if attempt {
            c.faint
        } else {
            c.text_2
        };
        let label = match event.kind.as_str() {
            "accepted" => "Accepted",
            "queued" => "Queued",
            "delivered" => "Delivered to the runtime",
            "acknowledged" => "Acknowledged through the inbox",
            "attempt" => "Delivery attempt",
            "failed" => "Failed",
            other => other,
        };
        delivery = delivery.child(ui::text(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .min_h(px(t.size.interaction_lifecycle_row))
                .when(attempt, |el| el.pl(px(t.space.s20 + t.space.s2)))
                .child(
                    div()
                        .size(px(t.size.interaction_lifecycle_mark))
                        .rounded_full()
                        .border(px(t.size.hairline))
                        .border_color(color)
                        .when(!attempt && !pending, |el| el.bg(color)),
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
    column=column.child(ui::reader_section("DELIVERY STEPS",delivery))
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
        column = column.child(ui::reader_section("RECORDED LINKS", links));
    }
    if item.relations_truncated {
        column = column.child(ui::notice(
            "info",
            "Additional recorded links exist beyond this page’s limit.",
            None,
        ));
    }
    if peer.kind == "agent" && peer.id.is_some() {
        column = column.child(
            ui::interaction_action(
                "interaction-pivot",
                "locate-fixed",
                format!("Center {}’s messages", actor_name(peer)),
            )
            .on_click(cx.listener(|view, _, _, cx| view.pivot(cx))),
        );
    }
    if item.conversation.supported {
        column = column.child(
            ui::interaction_action(
                "interaction-ask",
                "message-square",
                "Ask about this message",
            )
            .on_click(cx.listener(|view, _, window, cx| view.ask(window, cx))),
        );
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
    div()
        .size_full()
        .min_w_0()
        .flex()
        .flex_col()
        .border_l(px(t.size.hairline))
        .border_color(c.line)
        .child(actions)
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
