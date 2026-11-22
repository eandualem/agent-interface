use super::*;
use gpui::{Rgba, Stateful, relative};
use model::{Direction, Message};

pub(super) fn row_status(p: &Peer) -> (&'static str, String, Rgba) {
    let c = colors();
    if p.waiting() > 0 {
        return ("!", format!("{} review", p.waiting()), c.attention_text);
    }
    match p.link {
        Link::IncomingRequest => ("!", "request".into(), c.attention_text),
        Link::Refused => ("!", "declined".into(), c.attention_text),
        Link::Pending => ("◷", "pending".into(), c.accent_text),
        Link::Offline => (
            "◌",
            format!(
                "off · {}",
                p.messages
                    .iter()
                    .filter(|m| m.receipt == Receipt::Queued)
                    .count()
            ),
            c.accent_text,
        ),
        Link::StoppedHere => ("⊘", "stopped".into(), c.muted),
        Link::Connected => {
            let (i, o) = p.gates.labels();
            ("●", format!("{}/{}", &i[..1], &o[..1]), c.ok)
        }
    }
}

impl Pane {
    /// Keep delayed clicks attached to the peer whose conversation drew the button.
    fn button(
        &self,
        p: &Peer,
        id: &str,
        label: &str,
        style: ui::Button,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, usize, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let peer = p.id.clone();
        ui::button(format!("connection-{peer}-{id}"), label.to_owned(), style)
            .focusable()
            .tab_stop(true)
            .on_click(cx.listener(move |this, _, window, cx| {
                if this.preview
                    && this.selected.as_ref() == Some(&peer)
                    && let Some(index) = this.peers.iter().position(|p| p.id == peer)
                {
                    click(this, index, window, cx);
                    this.reconcile(cx);
                    cx.notify();
                }
            }))
    }

    fn gates(&self, p: &Peer, draft: Gates, cx: &Context<Self>) -> Div {
        let t = tokens();
        let mut menu = ui::menu()
            .min_w_0()
            .max_w_full()
            .child(ui::menu_heading("Delivery gate"));
        for (incoming, label, auto) in [
            (true, "Incoming", draft.incoming_auto),
            (false, "Outgoing", draft.outgoing_auto),
        ] {
            let mut choices = div()
                .flex()
                .flex_wrap()
                .gap(px(t.space.s6))
                .items_center()
                .child(label);
            for (value, text) in [(false, "Review"), (true, "Auto")] {
                choices = choices.child(self.button(
                    p,
                    &format!("gate-{incoming}-{value}"),
                    text,
                    if auto == value {
                        ui::Button::Primary
                    } else {
                        ui::Button::Secondary
                    },
                    cx,
                    move |this, _, _, _| {
                        if let Editor::Gates(g) = &mut this.editor {
                            if incoming {
                                g.incoming_auto = value;
                            } else {
                                g.outgoing_auto = value;
                            }
                        }
                    },
                ));
            }
            menu = menu.child(choices);
        }
        let apply = self.button(
            p,
            "apply-gates",
            "Apply gate",
            ui::Button::Primary,
            cx,
            |this, index, _, _| {
                if let Editor::Gates(gates) = this.editor
                    && this.peers[index].can_review()
                {
                    this.peers[index].gates = gates;
                }
                this.editor = Editor::None;
            },
        );
        let consequence = "Future messages on an Auto direction skip review. Already-waiting messages stay in review.";
        menu.when(draft.incoming_auto || draft.outgoing_auto, |el| {
            el.child(consequence)
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .child(apply)
                .child(self.cancel(p, cx)),
        )
    }

    fn cancel(&self, p: &Peer, cx: &Context<Self>) -> Stateful<Div> {
        self.button(
            p,
            "cancel-editor",
            "Cancel",
            ui::Button::Secondary,
            cx,
            |this, _, window, cx| this.edit(Editor::None, window, cx),
        )
    }

    fn card(&self, p: &Peer, n: usize, m: &Message, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let outgoing = m.direction == Direction::Outgoing;
        let pending = m.receipt == Receipt::Waiting && p.can_review();
        let tint = [c.info, c.violet, c.green, c.accent][p
            .id
            .bytes()
            .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
            % 4];
        let mut card = div()
            .min_w_0()
            .max_w(relative(if pending { 0.88 } else { 0.82 }))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(t.space.s6))
            .rounded(px(t.radius.card))
            .p(px(t.space.s10))
            .border(px(t.size.hairline))
            .bg(if outgoing && !pending {
                c.outgoing_surface
            } else {
                c.panel
            })
            .border_color(if outgoing { c.outgoing_border } else { c.line })
            .when(outgoing, |el| el.self_end().rounded_tr(px(t.radius.key)))
            .when(!outgoing, |el| {
                el.self_start()
                    .rounded_tl(px(t.radius.key))
                    .border_l(px(t.space.s2))
                    .border_color(tint)
            })
            .when(pending, |el| {
                el.border(px(t.size.hairline))
                    .border_dashed()
                    .border_color(c.attention)
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_between()
                    .gap(px(t.space.s12))
                    .child(
                        ui::mono(if outgoing {
                            "main".to_owned()
                        } else {
                            p.name.clone()
                        })
                        .text_color(if outgoing {
                            c.text_2
                        } else {
                            tint
                        }),
                    )
                    .child(
                        ui::text(
                            div().text_color(if pending { c.attention_text } else { c.muted }),
                            &t.text.meta,
                        )
                        .child(if pending {
                            format!("review · {}", m.time)
                        } else {
                            m.time.to_owned()
                        }),
                    ),
            )
            .child(ui::text(div().text_color(c.text), &t.text.body).child(m.body.clone()));
        if pending {
            let mut actions = div().flex().flex_wrap().gap(px(t.space.s6));
            if self.editor == Editor::Decline(n) {
                actions = actions
                    .child(self.button(
                        p,
                        &format!("confirm-decline-{n}"),
                        "Confirm decline",
                        ui::Button::Danger,
                        cx,
                        move |this, i, _, _| {
                            if this.editor == Editor::Decline(n) {
                                this.peers[i].review(n, Decision::Decline);
                                this.editor = Editor::None;
                            }
                        },
                    ))
                    .child(self.cancel(p, cx));
            } else if self.editor == Editor::Revise(n) && outgoing {
                card = card
                    .child("Revision note for main")
                    .child(self.input.clone())
                    .children(self.error);
                actions = actions
                    .child(self.button(
                        p,
                        &format!("send-note-{n}"),
                        "Send note to main",
                        ui::Button::Primary,
                        cx,
                        move |this, _, _, cx| {
                            if this.editor == Editor::Revise(n) {
                                this.submit_revision(cx);
                            }
                        },
                    ))
                    .child(self.cancel(p, cx));
            } else {
                actions = actions.child(self.button(
                    p,
                    &format!("pass-{n}"),
                    "Pass through",
                    ui::Button::Primary,
                    cx,
                    move |this, i, _, _| {
                        this.peers[i].review(n, Decision::Pass);
                        this.editor = Editor::None;
                    },
                ));
                if outgoing {
                    actions = actions.child(self.button(
                        p,
                        &format!("revise-{n}"),
                        "Ask to revise",
                        ui::Button::Secondary,
                        cx,
                        move |this, _, window, cx| this.edit(Editor::Revise(n), window, cx),
                    ));
                }
                actions = actions.child(self.button(
                    p,
                    &format!("decline-{n}"),
                    "Decline",
                    ui::Button::Danger,
                    cx,
                    move |this, _, window, cx| this.edit(Editor::Decline(n), window, cx),
                ));
            }
            let consequence = match self.editor {
                Editor::Decline(index) if index == n => "Declines this message.".to_owned(),
                Editor::Revise(index) if index == n => {
                    "Returns the revision note to main.".to_owned()
                }
                _ if outgoing => format!("Sends unchanged to {}", p.name),
                _ => "Makes available & notifies main".to_owned(),
            };
            card = card
                .child(actions)
                .child(ui::text(div().text_color(c.muted), &t.text.meta).child(consequence));
        } else {
            let label = match m.receipt {
                Receipt::Delivered => "Delivered",
                Receipt::Notified => "main notified",
                Receipt::Passed => "Passed · awaiting receipt",
                Receipt::Missing => "Receipt missing · delivery unknown",
                Receipt::Uncertain => "Delivery uncertain · held for confirmation",
                Receipt::Declined => "Declined",
                Receipt::Revision => "Sent back to main to revise",
                Receipt::Queued if p.link == Link::StoppedHere => {
                    "Stopped here · remote outcome unknown"
                }
                Receipt::Queued => "Queued · peer offline",
                Receipt::Waiting => "Review stopped",
            };
            card = card
                .child(ui::text(div().text_color(c.muted), &t.text.meta).child(label))
                .children(
                    m.revision_note
                        .as_ref()
                        .map(|note| div().text_color(c.text_2).child(note.clone())),
                );
        }
        card
    }

    pub(super) fn thread(&self, p: &Peer, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let mut header = div()
            .p(px(t.space.s10))
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(t.space.s8))
            .when(self.compact, |el| {
                el.child(
                    ui::back_button("back-to-peers", "All")
                        .focusable()
                        .tab_stop(true)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.show_thread = false;
                            cx.notify();
                        })),
                )
            })
            .child(ui::profile_mark(&p.name, t.size.workspace_tile))
            .child(p.name.clone());
        if p.can_review() {
            let (i, o) = p.gates.labels();
            header = header
                .child(self.button(
                    p,
                    "gate",
                    &format!("↙ {i} · ↗ {o} ⌄"),
                    ui::Button::Secondary,
                    cx,
                    |this, n, window, cx| this.edit(Editor::Gates(this.peers[n].gates), window, cx),
                ))
                .child(self.button(
                    p,
                    "disconnect",
                    "Disconnect",
                    ui::Button::Danger,
                    cx,
                    |this, _, window, cx| this.edit(Editor::Disconnect, window, cx),
                ));
        }
        let mut body = div()
            .id(format!("thread-{}", p.id))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(t.space.s10))
            .p(px(t.space.s12));
        match p.link {
            Link::IncomingRequest => {
                let accept = self.button(
                    p,
                    "accept-request",
                    "Accept",
                    ui::Button::Primary,
                    cx,
                    |this, n, _, _| {
                        if this.peers[n].link == Link::IncomingRequest {
                            this.peers[n].link = Link::Connected;
                            this.peers[n].gates = Gates::default();
                            this.needs_you = false;
                        }
                    },
                );
                let decline = self.button(
                    p,
                    "decline-request",
                    "Decline",
                    ui::Button::Danger,
                    cx,
                    |this, n, _, cx| {
                        if this.peers[n].link == Link::IncomingRequest {
                            this.peers.remove(n);
                            this.select(None, cx);
                        }
                    },
                );
                let note = format!(
                    "Accepting lets {}’s agent send messages to your profile. Incoming messages become available to your agent after review. Both directions start on Review.",
                    p.name
                );
                body = body
                    .child("Connection request")
                    .child(format!("Name provided by requester: {}", p.name))
                    .child(div().font_family(t.font.mono.clone()).child(p.id.clone()))
                    .child(note)
                    .child(
                        div()
                            .flex()
                            .gap(px(t.space.s8))
                            .flex_wrap()
                            .child(accept)
                            .child(decline),
                    );
            }
            Link::Pending => {
                body = body
                    .child("Connection request pending")
                    .child(div().font_family(t.font.mono.clone()).child(p.id.clone()))
            }
            Link::Refused => {
                body = body
                    .child("Connection request declined")
                    .child(
                        div()
                            .font_family(t.font.mono.clone())
                            .child("request_declined"),
                    )
                    .child(self.button(
                        p,
                        "dismiss-request",
                        "Dismiss",
                        ui::Button::Secondary,
                        cx,
                        |this, n, _, cx| {
                            if this.peers[n].link == Link::Refused {
                                this.peers.remove(n);
                                this.select(None, cx);
                            }
                        },
                    ))
            }
            Link::Offline | Link::Connected | Link::StoppedHere => {
                if let Editor::Gates(draft) = self.editor {
                    body = body.child(self.gates(p, draft, cx));
                }
                if self.editor == Editor::Disconnect {
                    body = body.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .child(format!("Disconnect from {}?", p.name))
                            .child(self.button(
                                p,
                                "confirm-disconnect",
                                "Confirm",
                                ui::Button::Danger,
                                cx,
                                |this, n, _, _| {
                                    if this.editor == Editor::Disconnect {
                                        this.peers[n].link = Link::StoppedHere;
                                        this.editor = Editor::None;
                                    }
                                },
                            ))
                            .child(self.cancel(p, cx)),
                    );
                }
                if p.link == Link::Offline {
                    let note = format!(
                        "{} is offline. Outgoing messages are queued by Agent Backbone until reconnect.",
                        p.name
                    );
                    body = body.child(note);
                }
                if p.link == Link::StoppedHere {
                    body = body.child("Stopped here, remote confirmation pending");
                }
                body = body
                    .child(
                        ui::text(div().text_color(c.muted), &t.text.meta).child(format!(
                            "Left: {} (external · available to main) · Right: main",
                            p.name
                        )),
                    )
                    .children(
                        p.messages
                            .iter()
                            .enumerate()
                            .map(|(n, m)| self.card(p, n, m, cx)),
                    );
            }
        }
        let mut footer = div()
            .p(px(t.space.s8))
            .flex()
            .flex_wrap()
            .justify_between()
            .child(
                ui::text(div().text_color(c.muted), &t.text.meta)
                    .child("Sample read 14:10 · states may lag ~1 min"),
            );
        if let Some(next) = self
            .peers
            .iter()
            .find(|peer| peer.id != p.id && peer.needs_you() && self.matches(peer, cx))
        {
            let id = next.id.clone();
            footer = footer.child(self.button(
                p,
                "next-waiting",
                &format!("Next waiting: {} →", next.name),
                ui::Button::Secondary,
                cx,
                move |this, _, _, cx| {
                    this.select(Some(id.clone()), cx);
                    this.show_thread = true;
                },
            ));
        }
        div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .child(footer)
    }
}
