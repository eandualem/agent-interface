//! The reader every view opens a skill or a policy in: the list is replaced
//! by the document, with a back button saying where it returns to, the
//! document's place in that list, a button that copies its Markdown, a
//! summary card and the rendered text. Esc and `[` `]` do the same from the
//! keyboard.

use gpui::{AnyElement, Context, Div, div, prelude::*, px};

use super::{ViewPane, markdown, mono};
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
    // The document's Markdown, whole, for pasting elsewhere. The files are
    // Backbone's, so the reader only reads them.
    let source = match page.text {
        Text::Ready { text, .. } => Some(text.to_owned()),
        _ => None,
    };
    let copy = source.map(|source| {
        ui::button(
            "reader-copy",
            if pane.copied {
                "Copied"
            } else {
                "Copy Markdown"
            },
            ui::Button::Secondary,
        )
        .child(ui::icon(
            if pane.copied { "check" } else { "copy" },
            t.size.icon,
            c.muted,
        ))
        .on_click(cx.listener(move |pane, _, _, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(source.clone()));
            pane.copied = true;
            cx.notify();
            cx.spawn(async |pane, cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(1500))
                    .await;
                let _ = pane.update(cx, |pane, cx| {
                    pane.copied = false;
                    cx.notify();
                });
            })
            .detach();
        }))
    });
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
        .children(copy);
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
            .child(markdown::render(&markdown::parse(text), pane, cx))
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
    let mut hints = vec![("esc", "back"), ("[ ]", "previous, next")];
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
