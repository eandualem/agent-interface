//! The native peer list and agent-to-agent thread. Sample records are loaded
//! only after the person explicitly opens the preview and live in this pane.
use super::{
    NameEvent, NameField, colors,
    selection::{self, Selectable, Selection},
    tokens, ui,
};
use gpui::{
    ClipboardItem, Context, Div, Entity, Render, SharedString, Subscription, Window, canvas, div,
    prelude::*, px,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    time::Duration,
};
mod card;
mod live;
mod model;
// The ID check and fingerprint are drawn from in the next steps.
#[allow(dead_code)]
mod rules;

pub(super) struct Pane {
    live: live::Live,
    /// The sample records while the preview is open.
    preview: Option<model::Sample>,
    entry: gpui::FocusHandle,
    thread_focus: gpui::FocusHandle,
    selection: RefCell<Selection>,
    needs_you: bool,
    filter: Entity<NameField>,
    input: Entity<NameField>,
    compact: bool,
    /// The pane's width, for the cards' share of the column.
    narrow: bool,
    show_thread: bool,
    /// The thread header's ⋯ menu is open.
    menu_open: bool,
    /// What was copied last, shown as Copied for a moment.
    copied: Option<SharedString>,
    /// Messages whose whole body is open.
    expanded: HashSet<String>,
    /// The notes this pane sent back with a message, and when, by message.
    notes: HashMap<String, (String, String)>,
    _subscriptions: [Subscription; 2],
}

impl Selectable for Pane {
    fn selection(&self) -> &RefCell<Selection> {
        &self.selection
    }
}

impl Pane {
    pub(super) fn new(
        connection: super::Connection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            let mut field = NameField::bare_filter(window, cx);
            field.focus = field.focus.clone().tab_stop(true);
            field
        });
        let input = cx.new(|cx| {
            let mut field = NameField::feedback(window, cx);
            field.focus = field.focus.clone().tab_stop(true);
            field
        });
        let search = cx.subscribe(&filter, |_, _, event: &NameEvent, cx| {
            if matches!(event, NameEvent::Changed(_)) {
                cx.notify();
            }
        });
        let editor_keys =
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &NameEvent, _, cx| match event {
                    NameEvent::Cancel => {
                        this.live.form = None;
                        cx.notify();
                    }
                    NameEvent::Save => this.submit_live_form(cx),
                    NameEvent::Changed(_) => cx.notify(),
                },
            );
        Self {
            live: live::Live::new(connection, cx),
            preview: None,
            entry: cx.focus_handle().tab_stop(true),
            thread_focus: cx.focus_handle(),
            selection: RefCell::default(),
            needs_you: false,
            filter,
            input,
            compact: false,
            narrow: false,
            show_thread: false,
            menu_open: false,
            copied: None,
            expanded: HashSet::new(),
            notes: HashMap::new(),
            _subscriptions: [search, editor_keys],
        }
    }

    pub(super) fn is_tab(key: &gpui::Keystroke) -> bool {
        key.key == "tab" && key.modifiers.number_of_modifiers() == u8::from(key.modifiers.shift)
    }

    pub(super) fn focus_entry(&self, window: &mut Window, cx: &mut gpui::App) {
        window.focus(&self.entry, cx);
    }

    /// Open or close the preview. Its records are new each time it opens.
    fn preview(&mut self, open: bool, cx: &mut Context<Self>) {
        self.preview = open.then(model::Sample::new);
        self.needs_you = false;
        self.expanded.clear();
        self.notes.clear();
        self.filter.update(cx, |field, cx| field.clear(cx));
        let first = self.preview.as_ref().and_then(|sample| {
            sample
                .overview
                .connections
                .first()
                .map(|p| peer_inbox::Selection::Peer(p.connection.clone()))
        });
        self.pick(first, cx);
    }

    /// Copy `text` and show Copied on the control `key` for a moment.
    fn copy(&mut self, key: SharedString, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(key.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            let _ = this.update(cx, |pane, cx| {
                if pane.copied.as_ref() == Some(&key) {
                    pane.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// A quiet text control that copies `text`.
    fn copy_text(
        &self,
        id: impl Into<SharedString>,
        label: &'static str,
        text: String,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let id = id.into();
        let key = id.clone();
        ui::text_action(
            id.clone(),
            if self.copied.as_ref() == Some(&id) {
                "Copied"
            } else {
                label
            },
        )
        .focusable()
        .tab_stop(true)
        .on_click(cx.listener(move |pane, _, _, cx| pane.copy(key.clone(), text.clone(), cx)))
    }
}

use crate::desktop::peer_inbox;

impl Pane {
    /// The peer column: its title and ＋, the All / Needs you choice, the
    /// search field and the rows.
    fn rail_frame(
        &self,
        rows: gpui::Stateful<Div>,
        entry: bool,
        cx: &Context<Self>,
        connect: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let plus = ui::interaction_icon_button("connect-peer", "plus", "Connect a profile")
            .tab_stop(true)
            .on_click(cx.listener(move |this, _, window, cx| connect(this, window, cx)));
        let plus = if entry {
            plus.track_focus(&self.entry)
        } else {
            plus
        };
        let mut segments = ui::segments();
        for (needs, label) in [(false, "All"), (true, "Needs you")] {
            segments = segments.child(
                ui::segment(format!("filter-{needs}"), label, self.needs_you == needs)
                    .flex_1()
                    .justify_center()
                    .focusable()
                    .tab_stop(true)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.needs_you = needs;
                        cx.notify();
                    })),
            );
        }
        div()
            .min_h_0()
            .bg(c.panel)
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .p(px(t.space.s8))
            .when(!self.compact, |el| {
                el.w(px(t.size.connections_rail))
                    .flex_shrink_0()
                    .border_r(px(t.size.hairline))
                    .border_color(c.line)
            })
            .when(self.compact, |el| el.flex_1())
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .pl(px(t.space.s4))
                    .child(ui::text(div().text_color(c.muted), &t.text.label).child("PEERS"))
                    .child(plus),
            )
            .child(segments)
            .child(ui::filter_frame().child(hinted(&self.filter, "Filter peers", cx)))
            .child(rows)
    }
}

/// A peer's row: its status mark, its name and a short trailing label.
fn peer_row(
    id: String,
    selected: bool,
    name: String,
    mark: &'static str,
    label: String,
    color: gpui::Rgba,
) -> gpui::Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::row(id, selected)
        .focusable()
        .tab_stop(true)
        .child(
            div()
                .flex_shrink_0()
                .w(px(t.size.icon))
                .flex()
                .justify_center()
                .child(
                    div()
                        .text_color(color)
                        .when(mark == "!", |el| {
                            el.rounded_full()
                                .bg(c.attention)
                                .text_color(c.on_attention)
                                .size(px(t.size.icon))
                                .flex()
                                .justify_center()
                                .items_center()
                        })
                        .child(mark),
                ),
        )
        .child(
            div()
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .text_ellipsis()
                .child(name),
        )
        .child(ui::text(div().text_color(color), &t.text.meta).child(label))
}

/// The quiet line a list shows when nothing is in it.
fn no_rows(text: &'static str) -> Div {
    let (t, c) = (tokens(), colors());
    ui::text(div().p(px(t.space.s10)).text_color(c.muted), &t.text.meta).child(text)
}

/// A field's frame, for a bare field or a value to select and copy.
fn field_box(content: impl IntoElement) -> Div {
    let (t, c) = (tokens(), colors());
    div()
        .flex()
        .items_center()
        .min_w_0()
        .px(px(t.space.s8))
        .py(px(t.space.s6))
        .rounded(px(t.radius.row))
        .bg(c.panel)
        .border(px(t.size.hairline))
        .border_color(c.line_strong)
        .child(content)
}

/// A bare field with a muted hint while it is empty.
fn hinted(field: &Entity<NameField>, hint: &'static str, cx: &gpui::App) -> Div {
    let c = colors();
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .when(
            field.read(cx).value().is_empty() && !field.read(cx).composing(),
            |el| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .text_color(c.muted)
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(hint),
                )
            },
        )
        .child(field.clone())
}

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let entity = cx.entity().downgrade();
        let tracker = canvas(
            move |bounds, _, cx| {
                let width = f32::from(bounds.size.width);
                let compact = width < tokens().size.connections_split;
                let narrow = width < tokens().size.connections_narrow;
                let _ = entity.update(cx, |this, cx| {
                    if this.compact != compact || this.narrow != narrow {
                        this.compact = compact;
                        this.narrow = narrow;
                        cx.notify();
                    }
                });
            },
            |_, (), _, _| (),
        )
        .absolute()
        .size_full();
        let frame = div()
            .tab_group()
            .capture_key_down(|event: &gpui::KeyDownEvent, window, cx| {
                if Self::is_tab(&event.keystroke) {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            })
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(tracker)
            .child(
                ui::view_header("link-2", "Connections")
                    .child(div().flex_1())
                    .children(self.read_at().map(|read| {
                        ui::text(
                            div().flex().items_center().gap(px(t.space.s6)),
                            &t.text.meta,
                        )
                        .child(ui::dot(c.ok))
                        .child(read)
                    })),
            );
        let frame = frame.when(self.preview.is_some(), |el| {
            el.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(px(t.space.s8))
                    .px(px(t.space.s12))
                    .py(px(t.space.s4))
                    .bg(c.field)
                    .child(
                        ui::button("exit-preview", "Exit preview", ui::Button::Secondary)
                            .track_focus(&self.entry)
                            .on_click(cx.listener(|this, _, _, cx| this.preview(false, cx))),
                    )
                    .child(
                        ui::text(div().min_w_0().text_color(c.muted), &t.text.meta)
                            .child("Sample data · actions apply only to this preview"),
                    ),
            )
        });
        let view = self.live_view(cx);
        frame.child(view)
    }
}
