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
    thread_scroll: gpui::ScrollHandle,
    /// The open menu's frame, and the ⋯ and folded gates buttons that open
    /// a menu; the chooser and ＋ use `entry`.
    menu_focus: gpui::FocusHandle,
    more_focus: gpui::FocusHandle,
    gates_focus: gpui::FocusHandle,
    /// A confirmation's Cancel, focused when it opens, and the Decline it
    /// came from, by message, to return to.
    confirm_focus: gpui::FocusHandle,
    decline_focus: gpui::FocusHandle,
    decline_opener: Option<String>,
    /// The menu a press outside just closed, so the press on its own button
    /// does not open it again.
    dismissed: Option<(Menu, std::time::Instant)>,
    /// The next drawn thread scrolls to what waits on the person.
    scroll_on_open: std::cell::Cell<bool>,
    selection: RefCell<Selection>,
    needs_you: bool,
    filter: Entity<NameField>,
    input: Entity<NameField>,
    width: peers::Width,
    /// The pane's width in px, for what the thread header has room for.
    pane_width: f32,
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
    _subscriptions: [Subscription; 3],
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
    /// The connect popover, which is a form rather than a menu; named only
    /// when a press outside closed it.
    Connect,
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
        let editor_keys = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &NameEvent, window, cx| match event {
                NameEvent::Cancel => {
                    let connect = this.connecting();
                    this.live.form = None;
                    if connect {
                        this.focus_after_connect(window, cx);
                    }
                    cx.notify();
                }
                NameEvent::Save => {
                    let connect = this.connecting();
                    this.submit_live_form(cx);
                    if connect {
                        this.focus_after_connect(window, cx);
                    }
                }
                NameEvent::Changed(_) => cx.notify(),
            },
        );
        let menu_focus = cx.focus_handle();
        // A menu closes when focus leaves it, as by Tab.
        let menu_left = cx.on_focus_out(&menu_focus, window, |pane, _, _, cx| {
            if !matches!(pane.menu, Menu::None) {
                pane.dismissed = Some((pane.menu, std::time::Instant::now()));
                pane.menu = Menu::None;
                cx.notify();
            }
        });
        Self {
            live: live::Live::new(connection, cx),
            preview: None,
            entry: cx.focus_handle().tab_stop(true),
            thread_focus: cx.focus_handle(),
            thread_scroll: gpui::ScrollHandle::new(),
            menu_focus,
            more_focus: cx.focus_handle().tab_stop(true),
            gates_focus: cx.focus_handle().tab_stop(true),
            confirm_focus: cx.focus_handle().tab_stop(true),
            decline_focus: cx.focus_handle().tab_stop(true),
            decline_opener: None,
            dismissed: None,
            scroll_on_open: std::cell::Cell::new(false),
            selection: RefCell::default(),
            needs_you: false,
            filter,
            input,
            width: peers::Width::Wide,
            pane_width: 0.0,
            list_key: None,
            list_local: None,
            profile_name: None,
            menu: Menu::None,
            copied: None,
            expanded: HashSet::new(),
            notes: HashMap::new(),
            _subscriptions: [search, editor_keys, menu_left],
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
        // The preview's gate changes go with its records; the live ones stay.
        self.live.events.retain(|e| !e.preview);
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

    /// Whether a press outside `menu` closed it a moment ago, which the same
    /// press then reaches its button as a click.
    fn just_dismissed(&mut self, menu: Menu) -> bool {
        self.dismissed
            .take()
            .is_some_and(|(closed, at)| closed == menu && at.elapsed() < Duration::from_millis(400))
    }

    /// A menu's button: open the menu with focus in it, or close it.
    fn toggle_menu(&mut self, menu: Menu, window: &mut Window, cx: &mut Context<Self>) {
        // One popover at a time.
        if self.connecting() {
            self.live.form = None;
        }
        if self.menu == menu || self.just_dismissed(menu) {
            self.menu = Menu::None;
        } else {
            self.menu = menu;
            window.focus(&self.menu_focus, cx);
        }
        cx.notify();
    }

    /// A menu's frame: it takes focus when it opens, Escape closes it and
    /// returns focus to its button, and a press outside closes it.
    fn menu_frame(&self, cx: &Context<Self>) -> Div {
        ui::menu()
            .track_focus(&self.menu_focus)
            .on_key_down(cx.listener(|pane, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    let button = match pane.menu {
                        Menu::More => &pane.more_focus,
                        Menu::Gates => &pane.gates_focus,
                        _ => &pane.entry,
                    };
                    window.focus(button, cx);
                    pane.menu = Menu::None;
                    cx.notify();
                }
            }))
            .on_mouse_down_out(cx.listener(|pane, _, _, cx| {
                pane.dismissed = Some((pane.menu, std::time::Instant::now()));
                pane.menu = Menu::None;
                cx.notify();
            }))
    }

    /// Close a confirmation or note and give focus back to what opened it,
    /// where that is still drawn.
    fn cancel_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.live.form.take() {
            Some(live::Form::Disconnect(_)) => window.focus(&self.more_focus, cx),
            Some(live::Form::Decline(_)) => window.focus(&self.decline_focus, cx),
            _ => {}
        }
        cx.notify();
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
        ring(ui::text_action(
            id.clone(),
            if self.copied.as_ref() == Some(&id) {
                "Copied"
            } else {
                label
            },
        ))
        .focusable()
        .tab_stop(true)
        .on_click(cx.listener(move |pane, _, _, cx| pane.copy(key.clone(), text.clone(), cx)))
    }
}

use crate::desktop::peer_inbox;

/// The focus ring the shared interaction controls draw, for this pane's own
/// focusable controls.
fn ring(control: gpui::Stateful<Div>) -> gpui::Stateful<Div> {
    control.focus_visible(|el| {
        el.border(px(tokens().size.focus_ring))
            .border_color(colors().focus)
    })
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
            move |bounds, window, cx| {
                let pixels = f32::from(bounds.size.width);
                let width = peers::Width::of(pixels);
                let changed = entity
                    .update(cx, |this, _| {
                        let mut changed = false;
                        if (this.pane_width - pixels).abs() >= 1.0 {
                            this.pane_width = pixels;
                            changed = true;
                        }
                        if this.width != width {
                            // A narrower list starts minimised again.
                            this.width = width;
                            this.list_local = None;
                            this.menu = Menu::None;
                            changed = true;
                        }
                        changed
                    })
                    .unwrap_or(false);
                // A view notified while the window draws gets no next frame
                // of its own, so the pane would keep its old layout until
                // some other event. A new width schedules the frame that
                // shows it; an unchanged one schedules nothing.
                if changed {
                    let entity = entity.clone();
                    window.on_next_frame(move |_, cx| {
                        let _ = entity.update(cx, |_, cx| cx.notify());
                    });
                }
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
                            .focusable()
                            .tab_stop(true)
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
