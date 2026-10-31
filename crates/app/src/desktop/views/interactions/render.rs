//! Native composition of the agent list, ledger and its aligned rail.

use super::*;
use crate::desktop::views::clock::Local;
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};
use gpui::{AnyElement, SharedString, div, point, px, relative};
use ledger::Row;

pub(super) fn ordered(view: &Interactions) -> Vec<&api::AgentSummary> {
    let mut agents: Vec<_> = view
        .snapshot
        .as_ref()
        .map(|s| s.summary.items.iter().collect())
        .unwrap_or_default();
    agents.sort_by(|a, b| {
        b.latest
            .as_ref()
            .map(|l| epoch(&l.time))
            .unwrap_or(0.)
            .total_cmp(&a.latest.as_ref().map(|l| epoch(&l.time)).unwrap_or(0.))
            .then(a.agent.name.cmp(&b.agent.name))
    });
    agents
}
pub(super) fn time(at: f64) -> String {
    Local::at(at)
        .map(|t| t.time())
        .unwrap_or_else(|| "—".into())
}
pub(super) fn duration(seconds: f64) -> String {
    if seconds > 0. && seconds < 60. {
        return "<1 min".into();
    }
    let minutes = (seconds.max(0.) / 60.).round() as u64;
    if minutes >= 60 {
        format!("{} h {} min", minutes / 60, minutes % 60)
    } else {
        format!("{minutes} min")
    }
}
fn radio(id: String, label: &str, on: bool) -> gpui::Stateful<gpui::Div> {
    let t = tokens();
    ui::segment(
        SharedString::from(id),
        SharedString::from(label.to_owned()),
        on,
    )
    .role(gpui::accesskit::Role::RadioButton)
    .aria_selected(on)
    .aria_label(label.to_owned())
    .focusable()
    .focus_visible(|el| {
        el.border(px(t.size.focus_ring))
            .border_color(colors().focus)
    })
}
fn actor(actor: &Actor) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .min_w_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(c.text)
            .when(actor.kind == "agent", |el| {
                el.font_family(t.font.mono.clone())
            })
            .when(actor.kind == "owner", |el| {
                el.font_weight(gpui::FontWeight::SEMIBOLD)
            })
            .when(
                matches!(actor.kind.as_str(), "unknown" | "external"),
                |el| el.text_color(c.muted).italic(),
            )
            .child(actor_name(actor).to_owned()),
        &t.text.meta,
    )
}
fn new_activity(view: &Interactions, cx: &mut Context<Interactions>) -> gpui::Stateful<gpui::Div> {
    let count = view.pending.as_ref().map_or(0, |(_, _, count)| *count);
    ui::interaction_chip(
        "interactions-new",
        format!("{count} new since {}", time(view.window.end)),
        false,
    )
    .on_click(cx.listener(|view, _, _, cx| view.accept_new(cx)))
}

fn list_frame(view: &Interactions, list: AnyElement, cx: &mut Context<Interactions>) -> AnyElement {
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .child(list)
        .when(view.pending.is_some(), |el| {
            el.child(
                div()
                    .absolute()
                    .top(px(tokens().space.s10))
                    .left_0()
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(new_activity(view, cx)),
            )
        })
        .into_any_element()
}

fn toolbar(view: &Interactions, cx: &mut Context<Interactions>) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let mut ranges = ui::segments()
        .id("interaction-ranges")
        .role(gpui::accesskit::Role::RadioGroup)
        .aria_label("Time window");
    for (i, preset) in Preset::ALL.into_iter().enumerate() {
        ranges = ranges.child(
            radio(
                format!("interaction-window-{i}"),
                preset.label(),
                view.window.preset == preset,
            )
            .on_click(cx.listener(move |view, event: &gpui::ClickEvent, _, cx| {
                view.menu_at = Some(event.position());
                view.change_window(preset, cx);
            })),
        );
    }
    let mut bar = ui::view_toolbar()
        .h_auto()
        .min_h(px(t.size.view_toolbar))
        .py(px(t.space.s6))
        .flex_wrap();
    if let Some(agent) = &view.agent {
        let name = view
            .back
            .last()
            .and_then(|(id, _)| {
                view.snapshot
                    .as_ref()?
                    .summary
                    .items
                    .iter()
                    .find(|a| a.agent.id.as_ref() == Some(id))
            })
            .map(|a| a.agent.name.as_str())
            .unwrap_or("All agents");
        bar = bar.child(
            ui::interaction_back_button("interactions-back", SharedString::from(name.to_owned()))
                .on_click(cx.listener(|view, _, _, cx| view.back_agent(cx))),
        );
        let name = view
            .snapshot
            .as_ref()
            .and_then(|s| {
                s.summary
                    .items
                    .iter()
                    .find(|a| a.agent.id.as_ref() == Some(agent))
            })
            .map(|a| a.agent.name.clone())
            .unwrap_or_default();
        bar = bar.child(ui::text(
            div().font_family(t.font.mono.clone()).child(name),
            &t.text.meta,
        ));
    }
    bar = bar.child(ranges).child(ui::text(
        div().text_color(c.muted).child(view.window.label()),
        &t.text.meta,
    ));
    if view.agent.is_some() {
        let mut directions = ui::segments()
            .id("interaction-directions")
            .role(gpui::accesskit::Role::RadioGroup)
            .aria_label("Message direction");
        for (i, label, dir) in [
            (0, "Both", Direction::Both),
            (1, "Sent", Direction::Sent),
            (2, "Received", Direction::Received),
        ] {
            directions = directions.child(
                radio(
                    format!("interaction-direction-{i}"),
                    label,
                    view.direction == dir,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.direction = dir;
                    cx.notify();
                })),
            );
        }
        bar = bar.child(directions).child(
            ui::interaction_chip(
                "interaction-peers",
                view.peer
                    .as_ref()
                    .and_then(|key| {
                        view.items()
                            .iter()
                            .flat_map(|m| [&m.sender, &m.recipient])
                            .find(|a| actor_key(a) == *key)
                    })
                    .map(|a| actor_name(a).to_owned())
                    .unwrap_or_else(|| "All counterparties".into()),
                view.peer.is_some(),
            )
            .on_click(cx.listener(|view, event: &gpui::ClickEvent, _, cx| {
                view.menu_at = Some(event.position());
                view.custom = false;
                view.peer_menu = !view.peer_menu;
                cx.notify();
            })),
        );
    }
    if view.pending.is_some() && view.selected.is_some() && view.narrow_reader(cx) {
        bar = bar.child(new_activity(view, cx));
    }
    bar.child(div().flex_1()).child(
        ui::interaction_icon_button("interaction-refresh", "refresh-cw", "Refresh interactions")
            .aria_label("Refresh interactions")
            .on_click(cx.listener(|view, _, _, cx| view.refresh(cx))),
    )
}
fn menus(
    view: &Interactions,
    window: &Window,
    cx: &mut Context<Interactions>,
) -> Option<AnyElement> {
    if !view.custom && !view.peer_menu {
        return None;
    }
    let (t, c) = (tokens(), colors());
    let mut menus = ui::menu();
    if view.custom {
        menus = menus.child(
            div()
                .flex()
                .flex_col()
                .gap(px(t.space.s8))
                .p(px(t.space.s12))
                .child(
                    div()
                        .w(px(t.size.popover_min_width))
                        .child("Start · local time")
                        .child(view.custom_start.clone()),
                )
                .child("to")
                .child(
                    div()
                        .w(px(t.size.popover_min_width))
                        .child("End · local time")
                        .child(view.custom_end.clone()),
                )
                .child("Local time · YYYY-MM-DD HH:MM")
                .child(
                    ui::interaction_chip("interaction-custom-apply", "Apply", true)
                        .on_click(cx.listener(|view, _, _, cx| view.apply_custom(cx))),
                )
                .children(
                    view.custom_error
                        .map(|e| div().text_color(c.attention_text).child(e)),
                ),
        );
    }
    if view.peer_menu {
        let mut peers: Vec<_> = view
            .items()
            .iter()
            .filter_map(|m| view.agent.as_ref().map(|a| counterparty(m, a)))
            .collect();
        peers.sort_by_key(|a| {
            (
                a.kind != "owner",
                a.kind == "unknown",
                actor_name(a).to_owned(),
            )
        });
        peers.dedup_by_key(|a| actor_key(a));
        let mut choices = div()
            .id("interaction-counterparties")
            .role(gpui::accesskit::Role::Menu)
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .p(px(t.space.s8))
            .child(
                ui::interaction_chip(
                    "interaction-peer-all",
                    "All counterparties",
                    view.peer.is_none(),
                )
                .role(gpui::accesskit::Role::MenuItemRadio)
                .aria_selected(view.peer.is_none())
                .on_click(cx.listener(|v, _, _, cx| {
                    v.peer = None;
                    v.peer_menu = false;
                    cx.notify();
                })),
            );
        for peer in peers {
            let key = actor_key(peer);
            choices = choices.child(
                ui::interaction_chip(
                    SharedString::from(format!("interaction-peer-{key}")),
                    actor_name(peer).to_owned(),
                    view.peer.as_ref() == Some(&key),
                )
                .role(gpui::accesskit::Role::MenuItemRadio)
                .aria_selected(view.peer.as_ref() == Some(&key))
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.peer = Some(key.clone());
                    v.peer_menu = false;
                    cx.notify();
                })),
            );
        }
        menus = menus.child(choices);
    }
    let at = view
        .menu_at
        .unwrap_or(view.origin.get() + point(px(t.space.s14), px(t.size.view_toolbar)));
    Some(
        gpui::deferred(
            gpui::anchored()
                .position(at)
                .snap_to_window_with_margin(px(t.space.s8))
                .child(
                    menus
                        .id("interaction-menu")
                        .max_h(window.viewport_size().height - px(t.space.s16))
                        .overflow_y_scroll()
                        .occlude()
                        .on_mouse_down_out(cx.listener(|v, _, _, cx| {
                            v.custom = false;
                            v.peer_menu = false;
                            cx.notify();
                        })),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}
fn strip(
    agent: &api::AgentSummary,
    view: &Interactions,
    window: TimeWindow,
) -> gpui::Stateful<gpui::Div> {
    let (t, c) = (tokens(), colors());
    let span = (window.end - window.start).max(1.);
    let mut strip = div()
        .id(SharedString::from(format!(
            "interaction-strip-{}",
            agent.agent.id.as_deref().unwrap_or_default()
        )))
        .relative()
        .h(px(t.size.interaction_strip))
        .w_full()
        .role(gpui::accesskit::Role::Image)
        .aria_label(format!(
            "Recorded messages for {}. State history is partial.",
            agent.agent.name
        ));
    let start = view
        .snapshot
        .as_ref()
        .and_then(|s| s.summary.coverage.retained_from.as_deref())
        .map_or(window.start, |at| epoch(at).max(window.start));
    for band in observations::normalize(
        &view.spans(agent.agent.id.as_deref().unwrap_or_default()),
        start,
        window.end,
    ) {
        strip = strip.child(
            div()
                .absolute()
                .left(relative(((band.start - window.start) / span) as f32))
                .w(relative(((band.end - band.start) / span) as f32))
                .top(px(t.space.s8))
                .h(px(t.size.interaction_band))
                .child(ui::observation_band(band.state, false)),
        );
    }
    for tick in &agent.ticks {
        let fraction = ((epoch(&tick.time) - window.start) / span).clamp(0., 1.) as f32;
        strip = strip.child(
            div()
                .absolute()
                .left(relative(fraction))
                .top(px(if tick.direction == "sent" {
                    t.size.hairline
                } else {
                    t.space.s14
                }))
                .w(px(t.size.hairline * 2.))
                .h(px(t.space.s6))
                .child(ui::message_tick(
                    if !tick.delivered {
                        c.attention
                    } else if tick.direction == "sent" {
                        c.text_2
                    } else {
                        c.info
                    },
                    !tick.delivered,
                )),
        );
    }
    strip
}
fn agents(view: &Interactions, cx: &mut Context<Interactions>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let mut column = div()
        .flex()
        .flex_col()
        .gap(px(t.space.s10))
        .p(px(t.space.s14))
        .w_full();
    for item in ordered(view) {
        let Some(id) = item.agent.id.clone() else {
            continue;
        };
        let mut title = div()
            .flex()
            .items_center()
            .gap(px(t.space.s8))
            .child(ui::agent_symbol(ui::AgentState::from_report(&item.state)))
            .child(actor(&item.agent))
            .child(ui::badge(item.runtime.clone()))
            .child(ui::text(
                div().text_color(c.muted).child(item.state.clone()),
                &t.text.meta,
            ));
        if item.latest.is_none() {
            title = title.child(ui::text(
                div()
                    .min_w_0()
                    .text_ellipsis()
                    .text_color(c.muted)
                    .child(empty_agent(item)),
                &t.text.meta,
            ));
        }
        title = title.child(div().flex_1());
        if item.latest.is_some()
            && view.snapshot.as_ref().is_some_and(|s| {
                s.summary.coverage.completeness != "complete_within_declared_scope"
            })
        {
            title = title.child(ui::tag("partial history", ui::Tone::Quiet).border_dashed());
        }
        if let Some(latest) = &item.latest {
            title = title.child(format!("latest {}", time(epoch(&latest.time))));
        }
        title = title.child(ui::icon("chevron-right", t.size.icon, c.faint));
        let card = ui::interaction_card(
            SharedString::from(format!("interaction-card-{id}")),
            view.card.as_ref() == Some(&id),
        )
        .aria_label(format!(
            "{}: {} sent, {} received, {} not delivered",
            item.agent.name, item.counts.sent, item.counts.received, item.counts.not_delivered
        ))
        .child(title);
        let card = if item.latest.is_some() {
            let mut peers: Vec<_> = item.counterparties.iter().collect();
            peers.sort_by_key(|a| (a.kind != "owner", a.kind == "unknown", actor_name(a)));
            let peers = peers
                .into_iter()
                .map(actor_name)
                .collect::<Vec<_>>()
                .join(", ");
            card.child(ui::text(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(t.space.s6))
                    .text_color(c.muted)
                    .child(format!(
                        "{} sent · {} received",
                        item.counts.sent, item.counts.received
                    ))
                    .when(item.counts.not_delivered > 0, |el| {
                        el.child(
                            div()
                                .text_color(c.attention_text)
                                .child(format!("· {} not delivered", item.counts.not_delivered)),
                        )
                    })
                    .child(format!("with {peers}")),
                &t.text.meta,
            ))
        } else {
            card
        };
        let card = if item.latest.is_some() {
            card.child(strip(item, view, view.window))
        } else {
            card
        };
        column = column.child(
            card.on_click(cx.listener(move |view, _, _, cx| view.open_agent(id.clone(), cx))),
        );
    }
    let mut legend = div().flex().flex_wrap().gap(px(t.space.s14));
    for state in [
        ui::Observation::Working,
        ui::Observation::Idle,
        ui::Observation::Blocked,
        ui::Observation::Offline,
        ui::Observation::Unknown,
    ] {
        legend = legend.child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s6))
                .child(
                    div()
                        .w(px(t.space.s18))
                        .h(px(t.space.s8))
                        .child(ui::observation_band(state, false)),
                )
                .child(state.label()),
        );
    }
    column = column.child(ui::text(legend.text_color(c.muted), &t.text.meta));
    column = column.child(ui::text(
        div()
            .text_color(c.muted)
            .child("Sent marks above the line · received below · states are sampled observations"),
        &t.text.meta,
    ));
    if let Some(s) = &view.snapshot {
        if let Some(at) = s
            .summary
            .coverage
            .body_retained_from
            .as_deref()
            .and_then(|at| Local::at(epoch(at)))
        {
            column = column.child(ui::text(
                div().text_color(c.muted).child(format!(
                    "Messages before {}, {} may retain only a preview.",
                    at.date(),
                    at.time()
                )),
                &t.text.meta,
            ));
        }
        if s.summary.items.iter().any(|agent| agent.ticks_truncated) {
            column = column.child(ui::text(
                div().text_color(c.muted).child(
                    "Activity marks are limited. Open an agent to read its retained history.",
                ),
                &t.text.meta,
            ));
        }
    }

    div()
        .id("interaction-agents")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&view.scroll)
        .child(column)
        .into_any_element()
}

fn message_row(
    view: &Interactions,
    index: usize,
    member: bool,
    same: bool,
    cx: &mut Context<Interactions>,
) -> AnyElement {
    let item = &view.items()[index];
    let agent = view.agent.as_deref().unwrap_or_default();
    let sent = outgoing(item, agent);
    let peer = counterparty(item, agent);
    let selected = view.selected.as_ref().is_some_and(|s| s.id == item.id);
    let (t, c) = (tokens(), colors());
    let id = item.id.clone();
    let hover_id = id.clone();
    let linked = view
        .selected
        .as_ref()
        .is_some_and(|selected| recorded_link(selected, item) || recorded_link(item, selected));
    let row = ui::interaction_row(
        SharedString::from(format!("interaction-message-{id}")),
        selected,
        member,
    )
    .aria_label(format!(
        "{} {} at {}, {}. {}",
        if sent { "Sent to" } else { "Received from" },
        actor_name(peer),
        time(epoch(&item.accepted_at)),
        item.status.value,
        item.preview
    ))
    .when(linked && !selected, |el| {
        el.child(
            div()
                .absolute()
                .inset(px(t.space.s4))
                .border(px(t.size.hairline))
                .border_dashed()
                .border_color(c.focus),
        )
    })
    .on_hover(cx.listener(move |view, hovered, _, cx| {
        if *hovered {
            view.hovered = Some(hover_id.clone());
        } else if view.hovered.as_ref() == Some(&hover_id) {
            view.hovered = None;
        }
        cx.notify();
    }))
    .when(same, |el| {
        el.child(
            div()
                .absolute()
                .left(px(t.space.s4))
                .top(px(t.space.s6))
                .bottom(px(t.space.s6))
                .w(px(t.space.s2))
                .bg(c.line_strong),
        )
    })
    .child(ui::icon(
        if sent {
            "arrow-up-right"
        } else {
            "arrow-down-left"
        },
        t.size.icon,
        if sent { c.text_2 } else { c.info },
    ))
    .child(ui::text(
        div()
            .w(px(t.size.interaction_time))
            .flex_shrink_0()
            .font_family(t.font.mono.clone())
            .text_color(c.muted)
            .child(time(epoch(&item.accepted_at))),
        &t.text.key,
    ))
    .child(
        div()
            .w(px(t.size.interaction_who))
            .flex_shrink_0()
            .flex()
            .gap(px(t.space.s4))
            .child(if sent { "→" } else { "←" })
            .child(actor(peer)),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .text_ellipsis()
            .child(item.preview.clone()),
    )
    .when(item.body.level != "full", |el| {
        el.child(ui::tag("preview only", ui::Tone::Quiet).border_dashed())
    })
    .when(item.sender.kind == "unknown", |el| {
        el.child(ui::tag("sender not recorded", ui::Tone::Quiet).border_dashed())
    })
    .children(wait_label(item).map(|label| ui::tag(label, ui::Tone::Quiet)))
    .child(ui::text(
        div()
            .w(px(t.size.interaction_status))
            .flex_shrink_0()
            .text_right()
            .text_color(if item.status.value == "failed" {
                c.attention_text
            } else {
                c.muted
            })
            .child(item.status.value.clone()),
        &t.text.meta,
    ))
    .child(ui::icon("chevron-right", t.size.icon, c.faint))
    .on_click(cx.listener(move |view, _, _, cx| view.choose(id.clone(), cx)));
    row.into_any_element()
}

fn ledger(view: &mut Interactions, cx: &mut Context<Interactions>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    if view.snapshot.is_none() {
        return div()
            .flex_1()
            .child(ui::notice(
                "clock",
                view.error.map_or("Loading this window…", Error::message),
                None,
            ))
            .into_any_element();
    }
    if view.items().is_empty() {
        return div()
            .flex_1()
            .child(ui::empty_state(
                ui::empty_tile("arrow-left-right", c.selected, c.text_2),
                "No interactions in this window",
                "Choose a longer window to see earlier recorded messages.",
            ))
            .child(
                ui::interaction_chip("interactions-show-day", "Show 24 hours", false)
                    .on_click(cx.listener(|v, _, _, cx| v.change_window(Preset::Day, cx))),
            )
            .into_any_element();
    }
    let agent = view.agent.as_deref().unwrap_or_default();
    let filtered = view.filtered();
    let visible_ids: HashSet<_> = filtered.iter().copied().collect();
    let mut seconds = HashMap::<i64, usize>::new();
    for item in view.items() {
        *seconds
            .entry(message_time(item).floor() as i64)
            .or_default() += 1;
    }
    let events: Vec<_> = filtered
        .iter()
        .map(|i| {
            let item = &view.items()[*i];
            ledger::Event {
                index: *i,
                id: item.id.clone(),
                at: message_time(item),
                peer: {
                    let peer = counterparty(item, agent);
                    if peer.kind == "owner" || (peer.kind == "agent" && peer.id.is_some()) {
                        actor_key(peer)
                    } else {
                        format!("unattributed-{}", item.id)
                    }
                },
                conversation: item
                    .conversation_entry
                    .as_ref()
                    .filter(|entry| entry.kind == "interaction" && entry.entry_kind == "message")
                    .and_then(|entry| entry.anchored_interaction_id.clone()),
            }
        })
        .collect();
    let history = view
        .snapshot
        .as_ref()
        .and_then(|s| s.summary.coverage.retained_from.as_deref())
        .map(epoch);
    let quiet_intervals = if view.direction == Direction::Both && view.peer.is_none() {
        view.snapshot
            .as_ref()
            .and_then(|s| s.history.as_ref())
            .filter(|page| !page.coverage.truncated && page.next_cursor.is_none())
            .map(|page| {
                page.coverage
                    .message_intervals
                    .iter()
                    .filter(|interval| interval.completeness == "complete_within_declared_scope")
                    .map(|interval| (epoch(&interval.since), epoch(&interval.until)))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        vec![]
    };
    let occupied: Vec<_> = view
        .items()
        .iter()
        .map(|item| {
            let settled = item.status.delivered
                || matches!(
                    item.status.value.as_str(),
                    "failed" | "cancelled" | "expired" | "not_taken" | "registration_retired"
                );
            (
                epoch(&item.accepted_at),
                if settled {
                    epoch(&item.last_event_at)
                } else {
                    view.window.end
                },
            )
        })
        .collect();
    let quiet_intervals = ledger::unoccupied(&quiet_intervals, &occupied);
    let rail = ledger::build(
        &events,
        view.window,
        history,
        &quiet_intervals,
        &view.expanded,
    );
    let spans = view.spans(agent);
    let mut rows = div()
        .relative()
        .w_full()
        .h(px(rail.height))
        .pl(px(t.size.interaction_rail + t.size.interaction_gutter));
    let mut rail_layer=div().id("interaction-time-rail").absolute().left_0().top_0().w(px(t.size.interaction_rail)).h(px(rail.height))
        .border_r(px(t.size.hairline)).border_color(c.line)
        .role(gpui::accesskit::Role::Image).aria_label("Recorded state beside the list. Sent marks are on the right, received marks on the left. Time between messages takes compressed space. Unrecorded state is hatched.");
    let lo = history.unwrap_or(view.window.start).max(view.window.start);
    for band in observations::normalize(&spans, lo, view.window.end) {
        let top = rail.upper(band.end);
        let bottom = rail.lower(band.start);
        rail_layer = rail_layer.child(
            div()
                .absolute()
                .left(px(t.size.interaction_rail - t.space.s28))
                .top(px(top))
                .h(px((bottom - top).max(0.)))
                .w(px(t.size.interaction_band))
                .child(ui::observation_band(band.state, true)),
        );
    }
    let mut labels = vec![
        (t.space.s8, view.window.end),
        (
            rail.height - t.size.interaction_edge_row / 2.,
            history.unwrap_or(view.window.start).max(view.window.start),
        ),
    ];
    let start = history.unwrap_or(view.window.start).max(view.window.start);
    let mut hour = Local::at(start)
        .map(|t| t.hour_start() + 3600.)
        .unwrap_or(start);
    while hour < view.window.end {
        let y = rail.upper(hour);
        if labels
            .iter()
            .all(|(other, at)| (other - y).abs() >= t.space.s20 && time(*at) != time(hour))
        {
            labels.push((y, hour));
        }
        hour += 3600.;
    }
    for (y, at) in labels {
        rail_layer = rail_layer.child(ui::text(
            div()
                .absolute()
                .left(px(t.space.s6))
                .top(px(y - t.space.s6))
                .text_color(c.muted)
                .font_family(t.font.mono.clone())
                .child(time(at)),
            &t.text.key,
        ));
    }
    for (i, item) in view
        .items()
        .iter()
        .enumerate()
        .filter(|(i, _)| !visible_ids.contains(i))
    {
        let _ = i;
        rail_layer = rail_layer.child(
            div()
                .absolute()
                .top(px(rail.upper(epoch(&item.accepted_at).max(start))))
                .left(px(if outgoing(item, agent) {
                    t.size.interaction_rail - t.space.s18
                } else {
                    t.size.interaction_rail - t.space.s28 - t.space.s12
                }))
                .w(px(t.size.interaction_tick))
                .h(px(t.space.s2))
                .bg(c.muted)
                .opacity(0.3),
        );
    }
    let mut selected_y = None;
    let selected_index = view
        .selected
        .as_ref()
        .and_then(|selected| view.items().iter().position(|item| item.id == selected.id));
    let top = -f32::from(view.scroll.offset().y);
    let viewport = f32::from(view.scroll.bounds().size.height).max(t.size.window_height);
    let margin = t.size.interaction_gap_max;
    for (n, line) in rail.lines.iter().enumerate() {
        if selected_index.is_some_and(|index| match &line.value {
            Row::Message(i) | Row::Member(i) => *i == index,
            Row::Burst {
                members,
                expanded: false,
                ..
            }
            | Row::Conversation { members, .. } => members.contains(&index),
            _ => false,
        }) {
            selected_y = Some(line.y);
        }
        // Geometry is retained for the whole history; only near-visible rows
        // become elements, keeping large retained histories cheap to scroll.
        if line.y + line.height < top - margin || line.y > top + viewport + margin {
            continue;
        }
        let same = |i: usize| {
            seconds
                .get(&(message_time(&view.items()[i]).floor() as i64))
                .copied()
                .unwrap_or(0)
                > 1
        };
        let body =
            match &line.value {
                Row::Message(i) => message_row(view, *i, false, same(*i), cx),
                Row::Member(i) => message_row(view, *i, true, same(*i), cx),
                Row::Group {
                    label,
                    count,
                    same_second,
                } => ui::text(
                    div()
                        .h_full()
                        .flex()
                        .items_end()
                        .pb(px(t.space.s6))
                        .gap(px(t.space.s8))
                        .text_color(c.muted)
                        .child(label.clone())
                        .child(format!(
                            "{count} {}",
                            if *count == 1 { "message" } else { "messages" }
                        ))
                        .children(same_second.map(|(at, n)| {
                            div().child(format!(
                                "· {n} in the same second, {}",
                                Local::at(at).map(|l| l.seconds()).unwrap_or_default()
                            ))
                        })),
                    &t.text.meta,
                )
                .into_any_element(),
                Row::Gap { start, end, inside } => ui::text(
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .px(px(t.space.s14))
                        .when(*inside, |el| {
                            el.bg(c.surface)
                                .border_t(px(t.size.hairline))
                                .border_l(px(t.size.hairline))
                                .border_r(px(t.size.hairline))
                                .border_dashed()
                                .border_color(c.line)
                        })
                        .text_color(c.faint)
                        .font_family(t.font.mono.clone())
                        .when(line.height >= t.space.s20 + t.space.s2, |el| {
                            el.child(format!(
                                "{} · {}",
                                duration(end - start),
                                observations::describe(&spans, *start, *end).0.label()
                            ))
                        }),
                    &t.text.key,
                )
                .into_any_element(),
                Row::Quiet { start, end } => ui::text(
                    div()
                        .id(SharedString::from(format!("interaction-quiet-{n}")))
                        .role(gpui::accesskit::Role::ListItem)
                        .aria_label(format!(
                            "{} to {}: {} without captured Backbone messages; observed {}",
                            time(*start),
                            time(*end),
                            duration(end - start),
                            observations::describe(&spans, *start, *end).1
                        ))
                        .h_full()
                        .flex()
                        .items_center()
                        .gap(px(t.space.s10))
                        .px(px(t.space.s14))
                        .bg(c.panel)
                        .text_color(c.muted)
                        .child(div().w(px(t.space.s18)).h(px(t.space.s10)).child(
                            ui::observation_band(
                                observations::describe(&spans, *start, *end).0,
                                false,
                            ),
                        ))
                        .child(format!(
                            "{}–{} · {} without captured Backbone messages · {}",
                            time(*start),
                            time(*end),
                            duration(end - start),
                            observations::describe(&spans, *start, *end).1
                        )),
                    &t.text.meta,
                )
                .into_any_element(),
                Row::Burst {
                    key,
                    members,
                    expanded,
                } => {
                    let key = key.clone();
                    let peer = counterparty(&view.items()[members[0]], agent);
                    let sent = members
                        .iter()
                        .filter(|i| outgoing(&view.items()[**i], agent))
                        .count();
                    let newest = epoch(&view.items()[members[0]].accepted_at);
                    let oldest =
                        epoch(&view.items()[*members.last().expect("burst members")].accepted_at);
                    let selected = members.iter().any(|index| {
                        view.selected
                            .as_ref()
                            .is_some_and(|item| item.id == view.items()[*index].id)
                    });
                    ui::interaction_row(SharedString::from(format!("burst-{key}")), selected, false)
                        .h(px(t.size.interaction_burst_row))
                        .aria_expanded(*expanded)
                        .aria_label(format!(
                            "{} {} messages with {} from {} to {}",
                            if *expanded { "Collapse" } else { "Expand" },
                            members.len(),
                            actor_name(peer),
                            time(oldest),
                            time(newest)
                        ))
                        .child(ui::icon("arrow-left-right", t.size.icon, c.muted))
                        .child(format!("{}–{}", time(oldest), time(newest)))
                        .child(actor(peer))
                        .child(format!(
                            "{} messages · {} received · {} sent",
                            members.len(),
                            members.len() - sent,
                            sent
                        ))
                        .child(div().flex_1())
                        .child(if *expanded { "Collapse" } else { "Expand" })
                        .on_click(cx.listener(move |v, _, _, cx| {
                            if !v.expanded.remove(&key) {
                                v.expanded.insert(key.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element()
                }
                Row::Conversation { anchor, members } => {
                    let anchor = anchor.clone();
                    let context = view
                        .items()
                        .iter()
                        .find(|item| item.id == anchor)
                        .map(|original| {
                            format!(
                                "the {} message to {}",
                                time(epoch(&original.accepted_at)),
                                actor_name(&original.recipient)
                            )
                        })
                        .unwrap_or_else(|| "a retained message".into());
                    let newest = message_time(&view.items()[members[0]]);
                    let oldest =
                        message_time(&view.items()[*members.last().expect("conversation run")]);
                    let label = format!(
                        "Your conversation about {context} · {} {}, {}–{}",
                        members.len(),
                        if members.len() == 1 {
                            "message"
                        } else {
                            "messages"
                        },
                        time(oldest),
                        time(newest)
                    );
                    ui::interaction_row(
                        SharedString::from(format!(
                            "interaction-conversation-{anchor}-{}",
                            view.items()[members[0]].id
                        )),
                        false,
                        false,
                    )
                    .aria_label(label.clone())
                    .child(ui::icon("message-square", t.size.icon, c.muted))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("⇄ You"))
                    .child(div().min_w_0().text_ellipsis().child(label))
                    .on_click(cx.listener(move |v, _, _, cx| v.open_discussion(anchor.clone(), cx)))
                    .into_any_element()
                }
                Row::Edge => {
                    let history_edge = history.is_some_and(|h| h > view.window.start);
                    let at = if history_edge {
                        history.unwrap_or(view.window.start)
                    } else {
                        view.window.start
                    };
                    ui::text(
                        div()
                            .h_full()
                            .flex()
                            .items_center()
                            .text_color(c.muted)
                            .child(format!(
                                "{} {} · {}",
                                if history_edge {
                                    "History starts"
                                } else {
                                    "Window starts"
                                },
                                Local::at(at)
                                    .map(|l| format!("{}, {}", l.date(), l.time()))
                                    .unwrap_or_default(),
                                if history_edge {
                                    "nothing earlier was recorded"
                                } else {
                                    "choose 24 hours to see earlier messages"
                                }
                            )),
                        &t.text.meta,
                    )
                    .into_any_element()
                }
            };
        let marks: Vec<_> = match &line.value {
            Row::Message(i) | Row::Member(i) => vec![(*i, line.height / 2.)],
            Row::Burst {
                members,
                expanded: false,
                ..
            }
            | Row::Conversation { members, .. } => members
                .iter()
                .map(|i| (*i, line.mark(message_time(&view.items()[*i]))))
                .collect(),
            _ => vec![],
        };
        for (i, y) in &marks {
            let item = &view.items()[*i];
            let selected = view.selected.as_ref().is_some_and(|s| s.id == item.id);
            if selected {
                selected_y = Some(line.y);
            }
            let sent = outgoing(item, agent);
            let color = if selected {
                c.accent
            } else if item.status.value == "failed" {
                c.attention
            } else if sent {
                c.text_2
            } else {
                c.info
            };
            rail_layer = rail_layer.child(
                div()
                    .absolute()
                    .left(px(if sent {
                        t.size.interaction_rail - t.space.s18
                    } else {
                        t.size.interaction_rail - t.space.s28 - t.space.s12
                    }))
                    .top(px(line.y + y - t.size.hairline))
                    .w(px(if selected {
                        t.space.s14
                    } else {
                        t.size.interaction_tick
                    }))
                    .h(px(if selected {
                        t.size.hairline * 3.
                    } else {
                        t.space.s2
                    }))
                    .child(ui::message_tick(color, !selected && !item.status.delivered)),
            );
            let hovered = view.hovered.as_ref() == Some(&item.id);
            if selected || hovered {
                rows = rows.child(
                    div()
                        .absolute()
                        .left(px(t.size.interaction_rail - t.space.s18))
                        .top(px(line.y + y))
                        .w(px(t.space.s18 + t.size.interaction_gutter))
                        .h(px(t.size.hairline))
                        .bg(if selected { c.accent } else { c.muted })
                        .opacity(if selected { 1. } else { 0.5 }),
                );
            }
        }
        if marks.len() > 1 {
            let min = marks.iter().map(|(_, y)| *y).fold(f32::INFINITY, f32::min);
            let max = marks.iter().map(|(_, y)| *y).fold(0., f32::max);
            rail_layer = rail_layer.child(
                div()
                    .absolute()
                    .left(px(t.size.interaction_rail - t.space.s8))
                    .top(px(line.y + min - t.space.s2))
                    .w(px(t.space.s4))
                    .h(px(max - min + t.space.s4))
                    .border_l(px(t.size.hairline))
                    .border_t(px(t.size.hairline))
                    .border_b(px(t.size.hairline))
                    .border_color(c.muted),
            );
        }
        let card = is_card(&line.value);
        let first = card && (n == 0 || !is_card(&rail.lines[n - 1].value));
        let last = card && (n + 1 == rail.lines.len() || !is_card(&rail.lines[n + 1].value));
        rows = rows.child(
            div()
                .id(SharedString::from(format!("interaction-line-{n}")))
                .absolute()
                .left(px(t.size.interaction_rail + t.size.interaction_gutter))
                .right_0()
                .top(px(line.y))
                .h(px(line.height))
                .when(card, |el| {
                    el.border_l(px(t.size.hairline))
                        .border_r(px(t.size.hairline))
                        .border_color(c.line)
                })
                .when(first, |el| {
                    el.rounded_t(px(t.radius.popover)).overflow_hidden()
                })
                .when(last, |el| {
                    el.rounded_b(px(t.radius.popover))
                        .border_b(px(t.size.hairline))
                        .border_color(c.line)
                        .overflow_hidden()
                })
                .child(body),
        );
    }
    if view.reveal && view.scroll.bounds().size.height > px(0.) {
        view.reveal = false;
        if let Some(y) = selected_y
            && (px(y) < -view.scroll.offset().y
                || px(y + t.size.group_row)
                    > -view.scroll.offset().y + view.scroll.bounds().size.height)
        {
            view.scroll
                .set_offset(point(px(0.), px(-(y - t.size.interaction_gap_max).max(0.))));
            cx.notify();
        }
    }
    // Layout can clamp a wheel offset after this render (especially near the
    // bottom or after collapsing rows). Reconcile virtualization against the
    // final offset so clamped scrolling never leaves visible rows unpainted.
    let scroll = view.scroll.clone();
    let entity = cx.entity().downgrade();
    rows = rows.child(rail_layer).child(
        gpui::canvas(
            move |_, _, cx| {
                if (-f32::from(scroll.offset().y) - top).abs() > 1. {
                    let _ = entity.update(cx, |_, cx| cx.notify());
                }
            },
            |_, (), _, _| (),
        )
        .absolute()
        .size_full(),
    );
    div()
        .id("interaction-ledger")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .track_scroll(&view.scroll)
        .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
        .px(px(t.space.s14))
        .child(rows)
        .into_any_element()
}

impl Render for Interactions {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        if self
            .ask_after_read
            .as_ref()
            .is_some_and(|id| self.selected.as_ref().is_some_and(|m| m.id == *id))
        {
            self.ask_after_read = None;
            self.ask(window, cx);
        }
        let width = self.width.clone();
        let origin = self.origin.clone();
        let entity = cx.entity().downgrade();
        let tracker = gpui::canvas(
            move |bounds, _, cx| {
                origin.set(bounds.origin);
                let next = f32::from(bounds.size.width);
                if (width.get() - next).abs() > 1. {
                    width.set(next);
                    let _ = entity.update(cx, |_, cx| cx.notify());
                }
            },
            |_, (), _, _| (),
        )
        .absolute()
        .size_full();
        let mut frame = div()
            .relative()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(toolbar(self, cx));
        if self.error == Some(Error::Unsupported) {
            frame = frame.child(
                ui::interaction_chip("interactions-open-activity", "Open Activity", false)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(Event::Activity))),
            );
        }
        let main = if self.snapshot.is_none() && self.selected.is_none() {
            match self.error {
                Some(error) => div()
                    .flex_1()
                    .child(ui::empty_state(
                        ui::empty_tile("arrow-left-right", c.selected, c.muted),
                        if error == Error::Unsupported {
                            "Interaction history isn’t available"
                        } else {
                            "Interaction history unavailable"
                        },
                        error.message(),
                    ))
                    .into_any_element(),
                None => {
                    let mut rows = div()
                        .flex_1()
                        .p(px(t.space.s14))
                        .flex()
                        .flex_col()
                        .gap(px(t.space.s10));
                    for i in 0..5 {
                        rows = rows.child(
                            div()
                                .id(i)
                                .h(px(t.size.group_row * 2.))
                                .bg(c.panel)
                                .rounded(px(t.radius.popover)),
                        );
                    }
                    rows.child("Loading interaction history…")
                        .into_any_element()
                }
            }
        } else if self.agent.is_none() {
            let list = agents(self, cx);
            list_frame(self, list, cx)
        } else {
            let narrow = self.narrow_reader(cx);
            let mut content = div().flex_1().min_h_0().min_w_0().flex();
            if !narrow || self.selected.is_none() {
                let list = ledger(self, cx);
                content = content.child(list_frame(self, list, cx));
            }
            if self.selected.is_some() {
                self.selection.borrow_mut().begin();
                let reader = reader::body(self, window, cx);
                self.selection.borrow_mut().finish();
                content = content.child(
                    div()
                        .h_full()
                        .min_w_0()
                        .when(!narrow, |el| {
                            el.w(px(if self.discussion_open(cx) {
                                t.size.interaction_reader_chat
                            } else {
                                t.size.interaction_reader
                            }))
                            .flex_shrink_0()
                        })
                        .when(narrow, |el| el.flex_1())
                        .child(reader),
                );
                if let Some(conversation) = self.conversation.clone() {
                    content =
                        content.child(conversation.update(cx, |c, cx| c.panel(None, window, cx)));
                } else if self
                    .selected
                    .as_ref()
                    .is_some_and(|item| item.conversation.supported)
                {
                    // The discussion entry point exists on first selection,
                    // before a thread has ever been opened or instantiated.
                    content = content.child(ui::conversation_rail(
                        div()
                            .id("interaction-discussion-open")
                            .role(gpui::accesskit::Role::Button)
                            .aria_label("Discuss this message")
                            .focusable()
                            .focus_visible(|el| {
                                el.border(px(t.size.focus_ring)).border_color(c.focus)
                            })
                            .on_click(cx.listener(|view, _, window, cx| view.ask(window, cx))),
                        None,
                        false,
                    ));
                }
            }
            content.into_any_element()
        };
        frame = frame.child(div().relative().flex_1().min_h_0().flex().child(main));
        if let Some(error) = self.detail_error.filter(|_| self.selected.is_none()) {
            frame = frame.child(ui::notice("info", error.message(), None));
        }
        if self
            .snapshot
            .as_ref()
            .is_some_and(|s| s.state_error.is_some())
        {
            frame=frame.child(ui::notice("info",if self.snapshot.as_ref().is_some_and(|s|s.states.is_some()) {"State history stopped updating. The last observations remain; uncovered time is not recorded."} else {"Observed state history is unavailable. Uncovered time stays marked as not recorded."},None));
        }
        if let Some(error) = self.error.filter(|_| self.snapshot.is_some()) {
            frame = frame.child(ui::notice("unplug", error.message(), None));
        }
        frame
            .child(ui::view_footer(
                &[
                    ("↑↓", "move"),
                    ("↵", "read"),
                    ("⌥↵", "ask"),
                    ("P", "center peer"),
                    ("Esc", "back"),
                ],
                if self.width.get() < t.size.interaction_split {
                    "Recorded communication"
                } else {
                    "Recorded states are context, not causes"
                },
            ))
            .child(tracker)
            .children(menus(self, window, cx))
            .child(selection::events(cx))
            .on_key_down(cx.listener(|view, event: &gpui::KeyDownEvent, window, cx| {
                if view.key(&event.keystroke, window, cx) {
                    cx.stop_propagation();
                }
            }))
    }
}

fn recorded_link(from: &Interaction, to: &Interaction) -> bool {
    from.relations.iter().any(|r| {
        let source = r.source_interaction_id.as_deref().unwrap_or(&from.id);
        matches!(r.target.kind.as_str(), "interaction" | "owner_request")
            && ((source == from.id && r.target.id == to.id)
                || (source == to.id && r.target.id == from.id))
    })
}

fn wait_label(item: &Interaction) -> Option<String> {
    let queued = item
        .lifecycle
        .items
        .iter()
        .filter(|e| e.kind == "queued")
        .map(|e| epoch(&e.time))
        .min_by(f64::total_cmp);
    let delivered = item
        .lifecycle
        .items
        .iter()
        .filter(|e| {
            e.kind == "delivered"
                || e.outcome.as_deref().is_some_and(|outcome| {
                    matches!(outcome, "delivered" | "acknowledged" | "handed_off")
                })
        })
        .map(|e| epoch(&e.time))
        .min_by(f64::total_cmp);
    let waited = queued
        .zip(delivered)
        .map(|(a, b)| b - a)
        .filter(|s| *s >= 60.);
    match (waited, item.attempt_count > 1) {
        (Some(seconds), true) => Some(format!(
            "waited {} · {} attempts",
            duration(seconds),
            item.attempt_count
        )),
        (Some(seconds), false) => Some(format!("waited {}", duration(seconds))),
        (None, true) => Some(format!("{} attempts", item.attempt_count)),
        _ => None,
    }
}

fn is_card(row: &Row) -> bool {
    !matches!(
        row,
        Row::Group { .. } | Row::Edge | Row::Gap { inside: false, .. }
    )
}

fn empty_agent(item: &api::AgentSummary) -> String {
    item.previous
        .as_ref()
        .map(|previous| {
            format!(
                "No interactions in this window · last recorded {}",
                Local::at(epoch(&previous.time))
                    .map(|at| format!("{}, {}", at.date(), at.time()))
                    .unwrap_or_default()
            )
        })
        .unwrap_or_else(|| "No interactions recorded in this window".into())
}
