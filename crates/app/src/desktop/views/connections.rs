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
mod peers;
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
    width: peers::Width,
    /// The key this workspace's peer list choice is saved under.
    list_key: Option<String>,
    /// This pane's own choice, made below the wide width.
    list_local: Option<bool>,
    /// This profile's name, for the bottom of the peer list.
    profile_name: Option<String>,
    menu: Menu,
    /// What was copied last, shown as Copied for a moment.
    copied: Option<SharedString>,
    /// Messages whose whole body is open.
    expanded: HashSet<String>,
    /// The notes this pane sent back with a message, and when, by message.
    notes: HashMap<String, (String, String)>,
    _subscriptions: [Subscription; 2],
}

/// Which of the pane's small menus is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    None,
    /// The thread header's ⋯.
    More,
    /// The narrow header's folded gates.
    Gates,
    /// The narrow header's peer chooser.
    Peers,
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
            width: peers::Width::Wide,
            list_key: None,
            list_local: None,
            profile_name: None,
            menu: Menu::None,
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

    /// Where this pane is: its workspace, that workspace's profile and the
    /// profile's name, which decide where its peer list choice is saved.
    pub(super) fn place(&mut self, profile: Option<&str>, workspace: &str, name: Option<&str>) {
        self.list_key = Some(peers::list_key(profile, workspace));
        self.profile_name = name.map(str::to_owned);
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
                let width = peers::Width::of(f32::from(bounds.size.width));
                let _ = entity.update(cx, |this, cx| {
                    if this.width != width {
                        // A narrower list starts minimised again.
                        this.width = width;
                        this.list_local = None;
                        this.menu = Menu::None;
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
