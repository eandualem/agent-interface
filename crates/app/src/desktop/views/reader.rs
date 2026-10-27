//! The reader every view opens a skill or a policy in: the list is replaced
//! by the document, with a back button saying where it returns to, the
//! document's place in that list, Rendered or Source, a summary card and the
//! text. Esc, `[` `]` and S do the same from the keyboard.

use gpui::{AnyElement, Context, Div, SharedString, div, prelude::*, px};

use super::{ViewPane, markdown, mono, selection};
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

/// A document's text, as far as it has been read.
pub(super) enum Text<'a> {
    Loading,
    Failed(&'static str),
    Ready { text: &'a str, cut: bool },
}

/// One open document.
pub(super) struct Page<'a> {
    /// Where the back button returns to, such as "Skills".
    pub back: String,
    /// The document's name, beside the back button.
    pub name: Option<String>,
    /// Its place in the list that opened it, and that list's length.
    pub at: Option<usize>,
    pub count: usize,
    /// What it is and who receives it.
    pub card: Div,
    pub text: Text<'a>,
}

/// The view's header, then the reader in place of its list.
pub(super) fn render(
    pane: &ViewPane,
    header: Div,
    page: Page<'_>,
    cx: &mut Context<ViewPane>,
) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let mut display = ui::segments();
    for (raw, label) in [(false, "Rendered"), (true, "Source")] {
        display = display.child(
            ui::segment(
                SharedString::from(format!("reader-display-{label}")),
                label,
                pane.raw == raw,
            )
            .on_click(cx.listener(move |pane, _, _, cx| {
                pane.raw = raw;
                cx.notify();
            })),
        );
    }
    let step = |id: &'static str, icon: &'static str, by: isize, cx: &mut Context<ViewPane>| {
        ui::icon_button(id, icon).on_click(cx.listener(move |pane, _, _, cx| pane.step(by, cx)))
    };
    let bar = div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(t.space.s8))
        .h(px(t.size.row + t.space.s10))
        .px(px(t.space.s10))
        .border_b(px(t.size.hairline))
        .border_color(c.line)
        .child(
            ui::back_button("reader-back", page.back).on_click(cx.listener(|pane, _, _, cx| {
                pane.close(cx);
            })),
        )
        .children(
            page.name
                .map(|name| mono(name).min_w_0().text_color(c.text)),
        )
        .child(div().flex_1())
        .child(step("reader-previous", "chevron-up", -1, cx))
        .child(
            ui::text(div().flex_shrink_0().text_color(c.muted), &t.text.status).child(
                match page.at {
                    Some(at) => format!("{} of {}", at + 1, page.count),
                    None => "not in this list".into(),
                },
            ),
        )
        .child(step("reader-next", "chevron-down", 1, cx))
        .child(display);
    let content: AnyElement = match page.text {
        Text::Loading => ui::text(div().text_color(c.muted), &t.text.body)
            .child("Reading…")
            .into_any_element(),
        Text::Failed(error) => ui::text(div().text_color(c.attention_text), &t.text.body)
            .child(error)
            .into_any_element(),
        Text::Ready { text, cut } => div()
            .flex()
            .flex_col()
            .gap(px(t.space.s12))
            .child(if pane.raw {
                ui::text(
                    div().font_family(t.font.mono.clone()).text_color(c.text_2),
                    &t.text.status,
                )
                .child(selection::text(pane, text, &[], cx))
            } else {
                markdown::render(&markdown::parse(text), pane, cx)
            })
            .when(cut, |el| {
                el.child(
                    ui::text(div().text_color(c.muted), &t.text.meta)
                        .child("The rest of this file is longer than this view reads."),
                )
            })
            .into_any_element(),
    };
    let body = div()
        .id("reader-page")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(t.space.s16))
        .overflow_y_scroll()
        .px(px(t.space.s24))
        .py(px(t.space.s20))
        .child(page.card)
        .child(content);
    let mut hints = vec![("esc", "back"), ("[ ]", "previous, next"), ("S", "source")];
    if pane.conversation.is_some() {
        hints.push(("C", "conversation"));
    }
    let footer = ui::view_footer(&hints, "read-only");
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header)
        .child(bar)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(body)
                        .child(footer),
                )
                .children(pane.conversation.clone()),
        )
        .into_any_element()
}
