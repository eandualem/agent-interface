//! Usage: tokens by agent, model or CLI over a recent range, as each CLI
//! recorded them. An agent row opens into its sessions.

use std::collections::BTreeMap;

use agent_interface_roster::views::{Range, Session, Tokens, Usage, epoch_seconds};
use gpui::{AnyElement, Context, Rgba, SharedString, div, prelude::*, px, relative};

use super::{UsageBy, ViewPane, count, list, mono, since};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

struct Row<'a> {
    key: String,
    runtimes: Vec<&'a str>,
    models: Vec<&'a str>,
    sessions: Vec<&'a Session>,
    tokens: Tokens,
    usd: f64,
    priced: bool,
    coverage: &'a str,
}

/// The range and grouping controls, drawn with or without data so a range
/// that fails to load can be changed.
pub(super) fn toolbar(pane: &ViewPane, cx: &mut Context<ViewPane>) -> gpui::Div {
    let t = tokens();
    let mut ranges = ui::segments();
    for choice in Range::ALL {
        ranges = ranges.child(
            ui::segment(
                SharedString::from(format!("usage-range-{}", choice.label())),
                choice.label(),
                choice == pane.range,
            )
            .on_click(cx.listener(move |pane, _, _, cx| pane.set_range(choice, cx))),
        );
    }
    let mut by = ui::segments();
    for (choice, label) in [
        (UsageBy::Agent, "Agent"),
        (UsageBy::Model, "Model"),
        (UsageBy::Cli, "CLI"),
    ] {
        by = by.child(
            ui::segment(
                SharedString::from(format!("usage-by-{label}")),
                label,
                choice == pane.usage_by,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.usage_by = choice;
                pane.expanded.clear();
                cx.notify();
            })),
        );
    }
    ui::view_toolbar()
        .gap(px(t.space.s14))
        .px(px(t.space.s18))
        .child(ranges)
        .child("by")
        .child(by)
        .child(div().flex_1())
        .child(
            div()
                .id("usage-cost")
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .cursor_pointer()
                .child("Estimated cost")
                .child(switch(pane.cost))
                .on_click(cx.listener(|pane, _, _, cx| {
                    pane.cost = !pane.cost;
                    cx.notify();
                })),
        )
}

fn switch(on: bool) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let knob = t.size.chip - t.space.s8;
    div()
        .flex()
        .flex_shrink_0()
        .w(px(knob * 2.0 + t.space.s4))
        .h(px(knob + t.space.s4))
        .p(px(t.space.s2))
        .rounded(px(knob))
        .bg(if on { c.accent } else { c.line_strong })
        .when(on, |el| el.justify_end())
        .child(
            div()
                .size(px(knob))
                .rounded_full()
                .bg(if on { c.on_accent } else { c.muted }),
        )
}

fn rank(coverage: &str) -> u8 {
    match coverage {
        "measured" => 0,
        "catching_up" => 1,
        "partial" => 2,
        _ => 3,
    }
}

fn coverage_color(coverage: &str) -> Rgba {
    let c = colors();
    match rank(coverage) {
        0 => c.ok,
        1 => c.accent_text,
        2 => c.attention_text,
        _ => c.muted,
    }
}

fn dollars(usd: f64, priced: bool) -> String {
    if priced {
        format!("${usd:.2}")
    } else {
        "–".into()
    }
}

pub(super) fn body(pane: &ViewPane, usage: &Usage, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let header = ui::view_header("chart-line", "Usage").child("tokens as each CLI recorded them");

    let mut groups: BTreeMap<String, Row<'_>> = BTreeMap::new();
    for session in &usage.sessions {
        let keys: Vec<String> = match pane.usage_by {
            UsageBy::Agent => vec![session.agent.clone()],
            UsageBy::Cli => vec![session.runtime.clone()],
            UsageBy::Model if session.models.is_empty() => vec!["unknown model".into()],
            // Backbone counts a session's tokens as a whole, so a session that
            // used several models is its own group rather than counted twice.
            UsageBy::Model => vec![session.models.join(" + ")],
        };
        for key in keys {
            let row = groups.entry(key.clone()).or_insert_with(|| Row {
                key,
                runtimes: Vec::new(),
                models: Vec::new(),
                sessions: Vec::new(),
                tokens: Tokens::default(),
                usd: 0.0,
                priced: false,
                coverage: &session.coverage,
            });
            row.sessions.push(session);
            row.tokens += session.tokens;
            if let Some(usd) = session
                .estimated_usd
                .as_deref()
                .and_then(|usd| usd.parse::<f64>().ok())
            {
                row.usd += usd;
                row.priced = true;
            }
            if !row.runtimes.contains(&session.runtime.as_str()) {
                row.runtimes.push(&session.runtime);
            }
            for model in &session.models {
                if !row.models.contains(&model.as_str()) {
                    row.models.push(model);
                }
            }
            // The weakest coverage of a group's sessions is its coverage.
            if rank(&session.coverage) > rank(row.coverage) {
                row.coverage = &session.coverage;
            }
        }
    }
    let mut rows: Vec<_> = groups.into_values().collect();
    rows.sort_by(|a, b| b.tokens.total.cmp(&a.tokens.total).then(a.key.cmp(&b.key)));
    let largest = rows.first().map_or(1, |row| row.tokens.total.max(1));
    let agents = usage
        .sessions
        .iter()
        .map(|s| s.agent.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    let totals = usage.totals;
    let total_usd = usage
        .estimated_usd
        .as_deref()
        .and_then(|usd| usd.parse::<f64>().ok());
    let headline = div()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s10))
        .px(px(t.space.s18))
        .py(px(t.space.s16))
        .border_b(px(t.size.hairline))
        .border_color(c.line)
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(t.space.s12))
                .child(ui::text(div(), &t.text.figure).child(if pane.cost {
                    dollars(total_usd.unwrap_or(0.0), total_usd.is_some())
                } else {
                    count(totals.total)
                }))
                .child(
                    ui::text(div().text_color(c.muted), &t.text.body).child(format!(
                        "{} in the last {}, {agents} agents, {} CLI sessions{}",
                        if pane.cost { "estimated" } else { "tokens" },
                        pane.range.label(),
                        usage.sessions.len(),
                        if usage.more { " (first 1000)" } else { "" }
                    )),
                ),
        )
        .child(breakdown(totals));

    let first = match pane.usage_by {
        UsageBy::Agent => "AGENT",
        UsageBy::Model => "MODEL",
        UsageBy::Cli => "CLI",
    };
    let mut table = list("usage-rows").px(px(t.space.s10)).child(columns(
        div().h(px(t.size.pane_header)).text_color(c.muted),
        [
            first,
            "CLI",
            "SESSIONS",
            "MODELS SEEN",
            if pane.cost { "COST" } else { "TOKENS" },
            "COVERAGE",
        ]
        .map(|text| {
            ui::text(div(), &t.text.label)
                .child(text)
                .into_any_element()
        }),
    ));
    for row in &rows {
        let open = pane.expanded.contains(&row.key);
        let key = row.key.clone();
        let fraction = row.tokens.total as f32 / largest as f32;
        let mark = if open { "▾ " } else { "▸ " };
        table = table.child(
            columns(
                div()
                    .h(px(t.size.row + t.space.s2))
                    .rounded(px(t.radius.icon_button))
                    .when(open, |el| el.bg(c.hover)),
                [
                    mono(format!("{mark}{}", row.key))
                        .text_color(c.text)
                        .into_any_element(),
                    mono(row.runtimes.join(", "))
                        .text_color(c.muted)
                        .into_any_element(),
                    div()
                        .text_color(c.muted)
                        .child(row.sessions.len().to_string())
                        .into_any_element(),
                    div()
                        .text_color(c.muted)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(row.models.join(", "))
                        .into_any_element(),
                    amount(
                        fraction,
                        if pane.cost {
                            dollars(row.usd, row.priced)
                        } else {
                            count(row.tokens.total)
                        },
                        c.accent,
                    ),
                    coverage(row.coverage),
                ],
            )
            .id(SharedString::from(format!("usage-row-{key}")))
            .cursor_pointer()
            .on_click(cx.listener(move |pane, _, _, cx| {
                if !pane.expanded.remove(&key) {
                    pane.expanded.insert(key.clone());
                }
                cx.notify();
            }))
            .into_any_element(),
        );
        if open {
            let mut sessions = row.sessions.clone();
            sessions.sort_by_key(|session| std::cmp::Reverse(session.tokens.total));
            for session in sessions {
                let short: String = session.id.chars().take(6).collect();
                let label = match &session.parent {
                    Some(_) => format!("   ↳ subagent {short}"),
                    None => format!(
                        "   {short} · {}{}",
                        since(epoch_seconds(&session.first_seen)),
                        if session.current { " · current" } else { "" }
                    ),
                };
                let usd = session
                    .estimated_usd
                    .as_deref()
                    .and_then(|usd| usd.parse::<f64>().ok());
                table = table.child(
                    columns(
                        div()
                            .h(px(t.size.row + t.space.s2))
                            .bg(c.panel)
                            .rounded(px(t.radius.icon_button)),
                        [
                            mono(label).text_color(c.muted).into_any_element(),
                            mono(session.agent.clone())
                                .text_color(c.muted)
                                .into_any_element(),
                            div().into_any_element(),
                            div()
                                .text_color(c.muted)
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(session.models.join(", "))
                                .into_any_element(),
                            amount(
                                session.tokens.total as f32 / largest as f32,
                                if pane.cost {
                                    dollars(usd.unwrap_or(0.0), usd.is_some())
                                } else {
                                    count(session.tokens.total)
                                },
                                c.faint,
                            ),
                            coverage(&session.coverage),
                        ],
                    )
                    .into_any_element(),
                );
            }
        }
    }
    if pane.usage_by == UsageBy::Agent {
        for agent in &usage.unmeasured {
            table = table.child(
                columns(
                    div().h(px(t.size.row + t.space.s2)).text_color(c.muted),
                    [
                        mono(format!("▸ {}", agent.agent)).into_any_element(),
                        mono(agent.runtime.clone()).into_any_element(),
                        div().child("–").into_any_element(),
                        div().child("–").into_any_element(),
                        div().child("–").into_any_element(),
                        coverage("unavailable"),
                    ],
                )
                .into_any_element(),
            );
        }
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(toolbar(pane, cx))
        .child(headline)
        .child(table)
        .child(ui::view_footer(
            &[("click", "sessions of a row")],
            "unavailable is not zero: no readable record was found",
        ))
        .into_any_element()
}

/// The four kinds of tokens as a stacked bar, with a legend under it.
pub(super) fn breakdown(totals: Tokens) -> gpui::Div {
    let (t, c) = (tokens(), colors());
    let parts = [
        (totals.cache_read, c.info, "cache reads"),
        (totals.input, c.accent, "uncached input"),
        (totals.cache_write, c.violet, "cache writes"),
        (totals.output, c.green, "output"),
    ];
    let mut legend = ui::text(
        div().flex().flex_wrap().gap(px(t.space.s20)),
        &t.text.status,
    );
    for (tokens, color, label) in parts {
        legend = legend.child(ui::legend(color, label, count(tokens)));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(t.space.s10))
        .child(ui::stacked_meter(
            &parts.map(|(tokens, color, _)| (tokens as f32, color)),
        ))
        .child(legend)
}

fn amount(fraction: f32, label: String, color: Rgba) -> AnyElement {
    let t = tokens();
    div()
        .flex()
        .items_center()
        .gap(px(t.space.s10))
        .child(ui::meter(fraction, color))
        .child(
            mono(label)
                .flex_shrink_0()
                .w(px(t.size.view_tile * 2.0))
                .flex()
                .justify_end(),
        )
        .into_any_element()
}

pub(super) fn coverage(coverage: &str) -> AnyElement {
    let color = coverage_color(coverage);
    div()
        .flex()
        .items_center()
        .gap(px(tokens().space.s6))
        .text_color(color)
        .child(div().size(px(tokens().size.dot)).rounded_full().bg(color))
        .child(coverage.replace('_', " "))
        .into_any_element()
}

// The table's six columns, as in the design: name, CLI, sessions, models,
// a flexible tokens bar, coverage.
fn columns(row: gpui::Div, cells: [AnyElement; 6]) -> gpui::Div {
    let t = tokens();
    let widths = [0.22, 0.09, 0.09, 0.17, 0.0, 0.12];
    let mut row = ui::text(
        row.flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(t.space.s14))
            .px(px(t.space.s8)),
        &t.text.status,
    );
    for (cell, width) in cells.into_iter().zip(widths) {
        let frame = div().min_w_0().overflow_hidden();
        row = row.child(if width == 0.0 {
            frame.flex_1().child(cell)
        } else {
            frame.flex_shrink_0().w(relative(width)).child(cell)
        });
    }
    row
}
