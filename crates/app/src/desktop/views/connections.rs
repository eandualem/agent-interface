//! The native peer list and asymmetric conversation. Sample records are loaded
//! only after the person explicitly opens the preview and live in this pane.
use super::{NameEvent, NameField, colors, tokens, ui};
use gpui::{
    ClipboardItem, Context, Div, Entity, Render, Subscription, Window, canvas, div, prelude::*, px,
};
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
    preview: bool,
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
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| NameField::filter(window, cx));
        let input = cx.new(|cx| NameField::feedback(window, cx));
        let search = cx.subscribe(&filter, |this, _, event: &NameEvent, cx| {
            if matches!(event, NameEvent::Changed(_)) {
                this.reconcile(cx);
                cx.notify();
            }
        });
        let editor_keys = cx.subscribe_in(
            &input,
            window,
            |this, _, event: &NameEvent, window, cx| match event {
                NameEvent::Cancel => this.edit(Editor::None, window, cx),
                NameEvent::Save if this.editor == Editor::Connect => this.connect(cx),
                NameEvent::Save => this.submit_revision(cx),
                NameEvent::Changed(_) => {}
            },
        );
        Self {
            preview: false,
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
        let (t, c) = (tokens(), colors());
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
                ui::row(format!("peer-{id}"), self.selected.as_ref() == Some(&id))
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
                            .child(peer.name.clone()),
                    )
                    .child(ui::text(div().text_color(color), &t.text.meta).child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select(Some(id.clone()), cx);
                        this.show_thread = true;
                        cx.notify();
                    })),
            );
        }
        if count == 0 {
            rows = rows.child(
                div()
                    .p(px(t.space.s12))
                    .text_color(c.muted)
                    .child("No matching peers"),
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
                    .child("Peers")
                    .child(
                        ui::button("connect-peer", "＋", ui::Button::Secondary)
                            .focusable()
                            .tab_stop(true)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit(Editor::Connect, window, cx)
                            })),
                    ),
            )
            .child(div().flex().gap(px(t.space.s4)).children(
                [(false, "All"), (true, "Needs you")].map(|(needs, label)| {
                    ui::row(format!("filter-{needs}"), self.needs_you == needs)
                        .focusable()
                        .tab_stop(true)
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.needs_you = needs;
                            this.reconcile(cx);
                            cx.notify();
                        }))
                }),
            ))
            .child(ui::text(div().text_color(c.muted), &t.text.meta).child("Search peers"))
            .child(self.filter.clone())
            .child(rows)
    }
}

impl Render for Pane {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, c) = (tokens(), colors());
        let frame = div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(c.surface)
            .child(ui::view_header("link-2", "Connections"));
        if !self.preview {
            return frame.child(
                ui::empty_state(
                    ui::empty_tile("unplug", c.selected, c.muted),
                    "Connections unavailable",
                    "This version cannot read profile connections from Agent Backbone yet.",
                )
                .child(
                    ui::button(
                        "preview-connections",
                        "Preview with sample data",
                        ui::Button::Secondary,
                    )
                    .focusable()
                    .tab_stop(true)
                    .on_click(cx.listener(|this, _, _, cx| this.preview(true, cx))),
                ),
            );
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
                        ui::text(div().min_w_0().text_color(c.muted), &t.text.meta)
                            .child("Sample data · actions apply only to this preview"),
                    )
                    .child(
                        ui::button("exit-preview", "Exit preview", ui::Button::Secondary)
                            .focusable()
                            .tab_stop(true)
                            .on_click(cx.listener(|this, _, _, cx| this.preview(false, cx))),
                    ),
            )
            .when(self.editor == Editor::Connect, |el| {
                el.child(self.connect_form(cx))
            })
            .child(content)
    }
}
