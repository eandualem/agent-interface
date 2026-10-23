//! Usage: tokens by agent over a recent range, as each CLI recorded them.

use std::collections::BTreeMap;

use gpui::{AnyElement, Context, Rgba, SharedString, div, prelude::*, px, relative};
use tmux_workspaces_roster::views::{Range, Tokens, Usage};

use super::{ViewPane, count, list, mono};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

struct Row<'a> {
    agent: &'a str,
    runtime: &'a str,
    sessions: usize,
    models: Vec<&'a str>,
    tokens: Tokens,
    coverage: &'a str,
}

pub(super) fn body(usage: &Usage, range: Range, cx: &mut Context<ViewPane>) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let mut ranges = ui::segments();
    for choice in Range::ALL {
        ranges = ranges.child(
            ui::segment(
                SharedString::from(format!("usage-range-{}", choice.label())),
                choice.label(),
                choice == range,
            )
            .on_click(cx.listener(move |pane, _, _, cx| pane.set_range(choice, cx))),
        );
    }
    let toolbar = ui::view_toolbar()
        .child(ranges)
        .child(div().flex_1())
        .child(format!("coverage {}", usage.coverage.replace('_', " ")));

    let mut agents: BTreeMap<&str, Row<'_>> = BTreeMap::new();
    for session in &usage.sessions {
        let row = agents.entry(&session.agent).or_insert_with(|| Row {
            agent: &session.agent,
            runtime: &session.runtime,
            sessions: 0,
            models: Vec::new(),
            tokens: Tokens::default(),
            coverage: &session.coverage,
        });
        row.sessions += 1;
        row.tokens += session.tokens;
        for model in &session.models {
            if !row.models.contains(&model.as_str()) {
                row.models.push(model);
            }
        }
        // The weakest coverage of an agent's sessions is its coverage.
        if rank(&session.coverage) > rank(row.coverage) {
            row.coverage = &session.coverage;
        }
    }
    let mut rows: Vec<_> = agents.into_values().collect();
    rows.sort_by(|a, b| {
        b.tokens
            .total
            .cmp(&a.tokens.total)
            .then(a.agent.cmp(b.agent))
    });
    let largest = rows.first().map_or(1, |row| row.tokens.total.max(1));

    let totals = usage.totals;
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
                .child(ui::text(div(), &t.text.figure).child(count(totals.total)))
                .child(
                    ui::text(div().text_color(c.muted), &t.text.body).child(format!(
                        "tokens in the last {}, {} agents, {} CLI sessions{}",
                        range.label(),
                        rows.len(),
                        usage.sessions.len(),
                        if usage.more { " (first 1000)" } else { "" }
                    )),
                ),
        )
        .child(ui::stacked_meter(
            &parts.map(|(tokens, color, _)| (tokens as f32, color)),
        ))
        .child(legend);

    let mut table = list("usage-rows").child(columns(
        div().h(px(t.size.row)).text_color(c.muted),
        [
            "AGENT",
            "CLI",
            "SESSIONS",
            "MODELS SEEN",
            "TOKENS",
            "COVERAGE",
        ]
        .map(|text| {
            ui::text(div(), &t.text.label)
                .child(text)
                .into_any_element()
        }),
    ));
    for row in &rows {
        let fraction = row.tokens.total as f32 / largest as f32;
        table = table.child(columns(
            div().h(px(t.size.row + t.space.s2)),
            [
                mono(row.agent.to_owned()).into_any_element(),
                mono(row.runtime.to_owned())
                    .text_color(c.muted)
                    .into_any_element(),
                div()
                    .text_color(c.muted)
                    .child(row.sessions.to_string())
                    .into_any_element(),
                div()
                    .text_color(c.muted)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(row.models.join(", "))
                    .into_any_element(),
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .child(ui::meter(fraction, c.accent))
                    .child(mono(count(row.tokens.total)).flex_shrink_0())
                    .into_any_element(),
                coverage(row.coverage).into_any_element(),
            ],
        ));
    }
    for agent in &usage.unmeasured {
        table = table.child(columns(
            div().h(px(t.size.row + t.space.s2)).text_color(c.muted),
            [
                mono(agent.agent.clone()).into_any_element(),
                mono(agent.runtime.clone()).into_any_element(),
                div().child("–").into_any_element(),
                div()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(agent.reason.clone())
                    .into_any_element(),
                div().child("–").into_any_element(),
                coverage("unavailable").into_any_element(),
            ],
        ));
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(toolbar)
        .child(headline)
        .child(table)
        .child(ui::view_footer(
            &[],
            "unavailable is not zero: no readable record was found",
        ))
        .into_any_element()
}

// Lower is better measured.
fn rank(coverage: &str) -> u8 {
    match coverage {
        "measured" => 0,
        "catching_up" => 1,
        "partial" => 2,
        _ => 3,
    }
}

fn coverage(coverage: &str) -> gpui::Div {
    let c = colors();
    let color: Rgba = match rank(coverage) {
        0 => c.ok,
        1 => c.accent_text,
        2 => c.attention_text,
        _ => c.muted,
    };
    div()
        .flex()
        .items_center()
        .gap(px(tokens().space.s6))
        .text_color(color)
        .child(ui::dot(color))
        .child(coverage.replace('_', " "))
}

// The table's six columns, as shares of its width.
fn columns(row: gpui::Div, cells: [AnyElement; 6]) -> gpui::Div {
    let t = tokens();
    let widths = [0.22, 0.09, 0.09, 0.17, 0.0, 0.13];
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
