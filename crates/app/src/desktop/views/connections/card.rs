//! One message in a thread: a card at about nine tenths of the column, the
//! sender's at its side, with a header, a Markdown body cut at six lines and a
//! line saying where it stands. A message waiting for review adds a strip
//! and the person's choices.
use super::live::{Change, DecisionKind, Form};
use super::rules::{self, Action, Tone};
use super::*;
use crate::desktop::views::markdown;
use agent_interface_roster::profiles::peers as api;
use api::Direction;
use gpui::relative;

/// The local agent the app writes for: Agent Backbone does not name it.
pub(super) const LOCAL_AGENT: &str = "main";

impl Pane {
    /// `peer` is the connection's shown name; `actions` whether a decision
    /// can be sent now; `held` whether an open note or confirmation stays
    /// usable, which it does while only a read is in flight; `revision`
    /// which resubmission this is, when known.
    pub(super) fn message_card(
        &self,
        peer: &str,
        m: &api::Message,
        revision: Option<u32>,
        actions: bool,
        held: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let outgoing = m.direction == Direction::Outgoing;
        let waiting = m.state == "pending_review";
        let key = m.message_id.clone();
        let (sender, recipient) = if outgoing {
            (LOCAL_AGENT, peer)
        } else {
            (peer, LOCAL_AGENT)
        };
        let meta =
            |text: String, color| ui::text(div().text_color(color), &t.text.meta).child(text);
        let mut card = div()
            .flex_shrink_0()
            .min_w_0()
            .w(relative(if self.width == peers::Width::Narrow {
                0.92
            } else {
                0.9
            }))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(t.radius.card))
            .border(px(t.size.hairline))
            .bg(if outgoing {
                c.outgoing_surface
            } else {
                c.panel
            })
            .border_color(match (waiting, outgoing) {
                (true, _) => c.focus,
                (false, true) => c.outgoing_border,
                (false, false) => c.line,
            })
            .map(|el| {
                if outgoing {
                    el.self_end()
                } else {
                    el.self_start()
                }
            });
        if waiting {
            card = card.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s8))
                    .h(px(t.size.review_strip))
                    .px(px(t.space.s14))
                    .bg(c.accent_soft)
                    .whitespace_nowrap()
                    .child(ui::icon("clock", t.size.icon - 1.0, c.accent_text))
                    .child(
                        ui::text(div().text_color(c.accent_text), &t.text.status)
                            .font_weight(t.text.row_selected.weight())
                            .child("Waiting for your review"),
                    )
                    .child(div().flex_1())
                    .when(outgoing, |el| {
                        el.child(
                            meta(
                                "Nothing is sent until you pass it through.".into(),
                                c.text_2,
                            )
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis(),
                        )
                    }),
            );
        }
        let name = |text: &str, agent: bool, color| {
            let el = div()
                .flex_shrink_0()
                .text_color(color)
                .child(text.to_owned());
            if agent {
                ui::text(el.font_family(t.font.mono.clone()), &t.text.status)
            } else {
                ui::text(el, &t.text.status)
            }
        };
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .pt(px(t.space.s10))
                .px(px(t.space.s14))
                .whitespace_nowrap()
                .child(ui::conversation_avatar(sender, t.size.message_avatar, true))
                .child(name(sender, outgoing, c.text))
                .child(ui::icon("arrow-right", t.size.icon - 2.0, c.muted))
                .child(name(recipient, !outgoing, c.text_2))
                .children(revision.map(|n| ui::pill(format!("revision {n}"))))
                .child(div().flex_1())
                .child(meta(rules::clock(&m.submitted_at), c.muted)),
        );
        let cut = rules::cut(&m.text);
        let open = self.expanded.contains(&key);
        let shown = match &cut {
            Some((start, _)) if !open => start.as_str(),
            _ => m.text.as_str(),
        };
        let look = markdown::Look {
            style: &t.text.row,
            color: c.text,
            line_height: 1.55,
            gap: t.space.s8,
            compact: true,
        };
        card = card.child(
            div()
                .pt(px(t.space.s8))
                .px(px(t.space.s14))
                .min_w_0()
                .child(markdown::render_in(&markdown::authored(shown), &look, self, cx).min_w_0()),
        );
        let toggle = cut.as_ref().map(|(_, more)| {
            let key = key.clone();
            ui::text(
                div()
                    .id(SharedString::from(format!("cut-{}", m.message_id)))
                    .flex_shrink_0()
                    .focusable()
                    .tab_stop(true)
                    .cursor_pointer()
                    .text_color(c.accent_text)
                    .hover(|el| el.underline()),
                &t.text.meta,
            )
            .child(rules::cut_label(*more, open))
            .on_click(cx.listener(move |pane, _, _, cx| {
                if !pane.expanded.remove(&key) {
                    pane.expanded.insert(key.clone());
                }
                cx.notify();
            }))
        });
        if waiting {
            return card
                .children(toggle.map(|toggle| {
                    div()
                        .flex()
                        .pt(px(t.space.s6))
                        .px(px(t.space.s14))
                        .child(toggle)
                }))
                .child(self.review_bar(peer, m, actions, held, cx));
        }
        let mut row = ui::message_meta()
            .pt(px(t.space.s8))
            .pb(px(t.space.s10))
            .px(px(t.space.s14))
            .flex_nowrap();
        if let Some(state) = rules::state_line(m, LOCAL_AGENT) {
            let (icon, text) = match state.tone {
                Tone::Muted => (c.muted, c.muted),
                Tone::Ok => (c.ok, c.muted),
                Tone::Attention => (c.attention, c.attention_text),
            };
            row = row
                .child(ui::icon(state.icon, t.size.icon - 2.0, icon))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .text_color(text)
                        .child(state.text),
                );
        }
        card.child(
            row.child(div().flex_1())
                .children(toggle)
                .child(self.copy_text(format!("copy-{key}"), "Copy", m.text.clone(), cx)),
        )
    }

    /// Under a waiting message: the decisions it offers, a confirmation, or
    /// the note that sends it back.
    fn review_bar(
        &self,
        peer: &str,
        m: &api::Message,
        actions: bool,
        held: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let bar = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(t.space.s8))
            .mt(px(t.space.s10))
            .px(px(t.space.s14))
            .py(px(t.space.s10))
            .border_t(px(t.size.hairline))
            .border_color(if m.direction == Direction::Outgoing {
                c.outgoing_border
            } else {
                c.line
            });
        let meta = |text: String| ui::text(div().text_color(c.muted), &t.text.meta).child(text);
        let form = match &self.live.form {
            Some(Form::Decline(x)) if x.message_id == m.message_id => Some(true),
            Some(Form::Revise(x)) if x.message_id == m.message_id => Some(false),
            _ => None,
        };
        // A decision whose reply was lost is sent again from its own card.
        if let Some((error, can_retry)) = self.lost_decision(&m.message_id) {
            return bar
                .child(ui::icon("triangle-alert", t.size.icon - 2.0, c.attention))
                .child(
                    ui::text(
                        div().flex_1().min_w_0().text_color(c.attention_text),
                        &t.text.meta,
                    )
                    .child(error),
                )
                .when(can_retry, |el| {
                    el.child(self.live_button(
                        format!("retry-{}", m.message_id),
                        "Retry",
                        ui::Button::Secondary,
                        cx,
                        |p, _, cx| p.send_live(cx),
                    ))
                });
        }
        // An open note or confirmation stays while a read is in flight, so
        // typing is not lost; the choices themselves wait for the read.
        match form {
            Some(_) if !held => return div(),
            None if !actions => return div(),
            _ => {}
        }
        match form {
            Some(true) => {
                let decline = m.clone();
                bar.child(meta("Decline this message? It will not be delivered.".into()).flex_1())
                    .child(self.live_button(
                        "confirm-decline",
                        "Decline",
                        ui::Button::Danger,
                        cx,
                        move |p, _, cx| {
                            p.change(
                                Change::Review(Box::new(decline.clone()), DecisionKind::Decline),
                                cx,
                            )
                        },
                    ))
                    .child(self.cancel_form(cx))
            }
            Some(false) => {
                let note = self.input.read(cx).value().trim().to_owned();
                let send = if note.is_empty() {
                    ui::button(
                        "send-revise",
                        format!("Send back to {LOCAL_AGENT}"),
                        ui::Button::Secondary,
                    )
                    .opacity(0.35)
                } else {
                    self.live_button(
                        "send-revise",
                        format!("Send back to {LOCAL_AGENT}"),
                        ui::Button::Secondary,
                        cx,
                        |p, _, cx| p.submit_live_form(cx),
                    )
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s6))
                    .mt(px(t.space.s10))
                    .px(px(t.space.s14))
                    .py(px(t.space.s10))
                    .border_t(px(t.size.hairline))
                    .border_color(c.outgoing_border)
                    .child(
                        ui::text(div().text_color(c.text_2), &t.text.meta)
                            .child(format!("What should {LOCAL_AGENT} change?")),
                    )
                    .child(
                        field_box(hinted(&self.input, "Your note", cx))
                            .min_h(px(t.size.button * 2.0))
                            .items_start()
                            .border_color(c.focus),
                    )
                    .child(meta(format!(
                        "The message stays here. {LOCAL_AGENT} gets your note and sends a new version for review."
                    )))
                    .child(div().flex().flex_wrap().gap(px(t.space.s8)).child(send).child(self.cancel_form(cx)))
            }
            None => {
                let mut bar = bar;
                for action in rules::review_actions(m.direction) {
                    let msg = m.clone();
                    bar = bar.child(match action {
                        Action::Pass => self.live_button(
                            format!("pass-{}", m.message_id),
                            "Pass through",
                            ui::Button::Primary,
                            cx,
                            move |p, _, cx| {
                                p.change(
                                    Change::Review(Box::new(msg.clone()), DecisionKind::Pass),
                                    cx,
                                )
                            },
                        ),
                        Action::Revise => self.live_button(
                            format!("revise-{}", m.message_id),
                            "Ask to revise",
                            ui::Button::Secondary,
                            cx,
                            move |p, w, cx| p.show_form(Form::Revise(msg.clone()), w, cx),
                        ),
                        Action::Decline => {
                            ui::text_action(format!("decline-{}", m.message_id), "Decline")
                                .text_color(c.text_2)
                                .h(px(t.size.button))
                                .px(px(t.space.s10))
                                .focusable()
                                .tab_stop(true)
                                .on_click(cx.listener(move |p, _, w, cx| {
                                    p.show_form(Form::Decline(msg.clone()), w, cx)
                                }))
                        }
                    });
                }
                bar.child(div().flex_1()).child(meta(rules::pass_note(
                    m.direction,
                    LOCAL_AGENT,
                    peer,
                )))
            }
        }
    }

    fn cancel_form(&self, cx: &Context<Self>) -> gpui::Stateful<Div> {
        self.live_button(
            "cancel-form",
            "Cancel",
            ui::Button::Secondary,
            cx,
            |pane, _, cx| {
                pane.live.form = None;
                cx.notify();
            },
        )
    }
}
