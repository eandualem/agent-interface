//! The native peer list and asymmetric conversation. Sample records are loaded
//! only after the person explicitly opens the preview and live in this pane.
use super::{NameEvent, NameField, colors, tokens, ui};
use gpui::{
    ClipboardItem, Context, Div, Entity, Render, Subscription, Window, canvas, div, prelude::*, px,
};
mod live;
mod model;
mod thread;
use model::{Decision, Gates, Link, Peer, Receipt};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Editor {
    None,
    Connect,
    Gates(Gates),
    Disconnect,
    Decline(usize),
    Revise(usize),
}

pub(super) struct Pane {
    live: live::Live,
    preview: bool,
    entry: gpui::FocusHandle,
    peers: Vec<Peer>,
    selected: Option<String>,
    needs_you: bool,
    filter: Entity<NameField>,
    input: Entity<NameField>,
    editor: Editor,
    compact: bool,
    show_thread: bool,
    copied: bool,
    error: Option<&'static str>,
    _subscriptions: [Subscription; 2],
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
        let search = cx.subscribe(&filter, |this, _, event: &NameEvent, cx| {
            if matches!(event, NameEvent::Changed(_)) {
                if this.preview {
                    this.reconcile(cx);
                }
                cx.notify();
            }
        });
        let editor_keys = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &NameEvent, window, cx| match event {
                NameEvent::Cancel if !this.preview => {
                    this.live.form = None;
                    cx.notify();
                }
                NameEvent::Save if !this.preview => this.submit_live_form(cx),
                NameEvent::Cancel => this.edit(Editor::None, window, cx),
                NameEvent::Save if this.editor == Editor::Connect => this.connect(cx),
                NameEvent::Save => this.submit_revision(cx),
                NameEvent::Changed(_) => {}
            },
        );
        Self {
            live: live::Live::new(connection, cx),
            preview: false,
            entry: cx.focus_handle().tab_stop(true),
            peers: Vec::new(),
            selected: None,
            needs_you: false,
            filter,
            input,
            editor: Editor::None,
            compact: false,
            show_thread: false,
            copied: false,
            error: None,
            _subscriptions: [search, editor_keys],
        }
    }

    pub(super) fn is_tab(key: &gpui::Keystroke) -> bool {
        key.key == "tab" && key.modifiers.number_of_modifiers() == u8::from(key.modifiers.shift)
    }

    pub(super) fn focus_entry(&self, window: &mut Window, cx: &mut gpui::App) {
        window.focus(&self.entry, cx);
    }

    fn matches(&self, peer: &Peer, cx: &gpui::App) -> bool {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        (!self.needs_you || peer.needs_you())
            && (peer.name.to_lowercase().contains(&query)
                || peer.id.to_lowercase().contains(&query))
    }

    fn select(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.selected = id;
        self.editor = Editor::None;
        self.error = None;
        self.copied = false;
        self.input.update(cx, |field, cx| field.clear(cx));
    }

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        if self
            .peers
            .iter()
            .any(|peer| Some(&peer.id) == self.selected.as_ref() && self.matches(peer, cx))
        {
            return;
        }
        let next = self
            .peers
            .iter()
            .find(|p| self.matches(p, cx))
            .map(|p| p.id.clone());
        self.select(next, cx);
        self.show_thread = false;
    }

    fn preview(&mut self, open: bool, cx: &mut Context<Self>) {
        self.preview = open;
        self.peers = if open { model::sample() } else { Vec::new() };
        self.needs_you = false;
        self.show_thread = false;
        self.filter.update(cx, |field, cx| field.clear(cx));
        self.select(self.peers.first().map(|p| p.id.clone()), cx);
        cx.notify();
    }

    fn edit(&mut self, editor: Editor, window: &mut Window, cx: &mut Context<Self>) {
        self.editor = editor;
        self.error = None;
        self.copied = false;
        self.input.update(cx, |field, cx| {
            field.clear(cx);
            if matches!(editor, Editor::Connect | Editor::Revise(_)) {
                window.focus(&field.focus, cx);
            }
        });
        cx.notify();
    }

    fn submit_revision(&mut self, cx: &mut Context<Self>) {
        let Editor::Revise(n) = self.editor else {
            return;
        };
        let Some(peer) = self
            .peers
            .iter_mut()
            .find(|p| Some(&p.id) == self.selected.as_ref())
        else {
            return;
        };
        let note = self.input.read(cx).value().to_owned();
        if note.trim().is_empty() {
            self.error = Some("Enter a revision note.");
        } else {
            peer.review(n, Decision::Revise(note));
            self.error = None;
            self.editor = Editor::None;
            self.reconcile(cx);
        }
        cx.notify();
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let id = self.input.read(cx).value().trim().to_owned();
        if id.is_empty()
            || id.chars().any(char::is_whitespace)
            || id == "sample-local"
            || self.peers.iter().any(|p| p.id == id)
        {
            self.error = Some("Enter a different peer ID.");
        } else {
            self.peers.push(Peer {
                name: id.clone(),
                id: id.clone(),
                link: Link::Pending,
                gates: Gates::default(),
                messages: Vec::new(),
            });
            self.needs_you = false;
            self.filter.update(cx, |field, cx| field.clear(cx));
            self.select(Some(id), cx);
            self.show_thread = true;
        }
        cx.notify();
    }

    fn connect_form(&self, cx: &mut Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        div()
            .p(px(t.space.s12))
            .bg(c.panel)
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .child("My connection ID")
            .child(ui::mono("sample-local"))
            .child(
                ui::button(
                    "copy-sample-id",
                    if self.copied { "Copied" } else { "Copy ID" },
                    ui::Button::Secondary,
                )
                .focusable()
                .tab_stop(true)
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string("sample-local".into()));
                    this.copied = true;
                    cx.notify();
                })),
            )
            .child("Peer connection ID")
            .child(self.input.clone())
            .children(
                self.error
                    .map(|error| div().text_color(c.attention_text).child(error)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(t.space.s8))
                    .child(
                        ui::button("request-peer", "Send", ui::Button::Primary)
                            .focusable()
                            .tab_stop(true)
                            .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                    )
                    .child(
                        ui::button("close-connect", "Cancel", ui::Button::Secondary)
                            .focusable()
                            .tab_stop(true)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit(Editor::None, window, cx)
                            })),
                    ),
            )
    }

    fn rail(&self, cx: &mut Context<Self>) -> Div {
        let mut rows = div()
            .id("connections-peers")
            .min_h_0()
            .flex_1()
            .overflow_y_scroll();
        let mut count = 0;
        for peer in self.peers.iter().filter(|p| self.matches(p, cx)) {
            count += 1;
            let id = peer.id.clone();
            let (mark, label, color) = thread::row_status(peer);
            rows = rows.child(
                peer_row(
                    format!("peer-{id}"),
                    self.selected.as_ref() == Some(&id),
                    peer.name.clone(),
                    mark,
                    label,
                    color,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select(Some(id.clone()), cx);
                    this.show_thread = true;
                    cx.notify();
                })),
            );
        }
        if count == 0 {
            rows = rows.child(no_rows("No matching peers"));
        }
        self.rail_frame(rows, false, cx, |this, window, cx| {
            this.edit(Editor::Connect, window, cx)
        })
    }

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
        let plus = ui::icon_button("connect-peer", "plus")
            .focusable()
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
                        if this.preview {
                            this.reconcile(cx);
                        }
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
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(ui::view_header("link-2", "Connections"));
        if !self.preview {
            return frame.child(self.live_view(cx));
        }
        let wide = !self.compact;
        let selected = self
            .peers
            .iter()
            .find(|p| Some(&p.id) == self.selected.as_ref());
        let entity = cx.entity().downgrade();
        let tracker = canvas(
            move |bounds, _, cx| {
                let compact = f32::from(bounds.size.width) < tokens().size.connections_split;
                let _ = entity.update(cx, |this, cx| {
                    if this.compact != compact {
                        this.compact = compact;
                        cx.notify();
                    }
                });
            },
            |_, (), _, _| (),
        )
        .absolute()
        .size_full();
        let mut content = div().relative().min_h_0().flex_1().flex().child(tracker);
        if wide || !self.show_thread {
            content = content.child(self.rail(cx));
        }
        if wide || self.show_thread {
            content = content.child(match selected {
                Some(peer) => self.thread(peer, cx),
                None => ui::empty_state(
                    ui::empty_tile("link-2", c.selected, c.muted),
                    "No peer selected",
                    "Choose a peer to see its conversation.",
                ),
            });
        }
        frame
            .child(
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
            .when(self.editor == Editor::Connect, |el| {
                el.child(self.connect_form(cx))
            })
            .child(content)
    }
}
