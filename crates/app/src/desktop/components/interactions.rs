//! Message-history pieces. Shapes distinguish observation states without
//! relying on colour, and share the ordinary view's design tokens.

use super::*;
use gpui::{Bounds, canvas, fill, size};

/// The recorded runtime state, independent from message delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Observation {
    /// Work was observed, not inferred from messages.
    Working,
    /// The runtime was idle.
    Idle,
    /// Waiting or blocked.
    Blocked,
    /// The runtime was offline.
    Offline,
    /// No usable state was recorded.
    Unknown,
}

impl Observation {
    /// Preserve missing state rather than defaulting to idle.
    pub fn from_state(state: &str) -> Self {
        match state {
            "busy" | "working" | "starting" => Self::Working,
            "idle" => Self::Idle,
            "blocked" | "waiting_for_human" => Self::Blocked,
            "offline" => Self::Offline,
            _ => Self::Unknown,
        }
    }

    /// Human-readable observation.
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Blocked => "blocked",
            Self::Offline => "offline",
            Self::Unknown => "state not recorded",
        }
    }
}

/// One horizontal or vertical state segment, sized by its caller's time map.
pub fn observation_band(state: Observation, vertical: bool) -> AnyElement {
    let (t, c) = (tokens(), colors());
    let width = t.size.interaction_band;
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let length = f32::from(if vertical {
                bounds.size.height
            } else {
                bounds.size.width
            });
            let cross = f32::from(if vertical {
                bounds.size.width
            } else {
                bounds.size.height
            });
            let thickness = match state {
                Observation::Idle | Observation::Offline => t.size.hairline * 2.,
                _ => width,
            }
            .min(cross);
            let color = match state {
                Observation::Working => c.accent,
                Observation::Idle => c.line_strong,
                Observation::Blocked => c.attention,
                Observation::Offline => c.muted,
                Observation::Unknown => c.faint,
            };
            let draw =
                |start: f32, run: f32, across: f32, thick: f32, window: &mut gpui::Window| {
                    let (x, y, w, h) = if vertical {
                        (across, start, thick, run)
                    } else {
                        (start, across, run, thick)
                    };
                    window.paint_quad(fill(
                        Bounds::new(bounds.origin + point(px(x), px(y)), size(px(w), px(h))),
                        color,
                    ));
                };
            let offset = (cross - thickness) / 2.;
            match state {
                Observation::Working | Observation::Idle => {
                    draw(0., length, offset, thickness, window)
                }
                Observation::Offline | Observation::Blocked => {
                    let dash = if state == Observation::Offline {
                        t.space.s2
                    } else {
                        t.space.s4 - t.size.hairline
                    };
                    let mut at = 0.;
                    while at < length {
                        draw(at, dash.min(length - at), offset, thickness, window);
                        at += dash + t.space.s2;
                    }
                }
                Observation::Unknown => {
                    // Diagonal hatching, clipped to this band, on both axes.
                    let mut at = -thickness;
                    while at < length {
                        let mut x = 0.;
                        while x < thickness {
                            let y = at + x;
                            if y >= 0. && y < length {
                                draw(
                                    y,
                                    t.size.hairline.min(length - y),
                                    offset + x,
                                    t.size.hairline.min(thickness - x),
                                    window,
                                );
                            }
                            x += t.size.hairline;
                        }
                        at += t.space.s4;
                    }
                }
            }
        },
    )
    .size_full()
    .into_any_element()
}

/// A selectable agent card with the same outline as a current-plan card.
pub fn interaction_card(id: impl Into<SharedString>, selected: bool) -> Stateful<Div> {
    plan_card(id, selected)
        .role(gpui::accesskit::Role::Button)
        .aria_selected(selected)
        .focusable()
        .focus_visible(|el| el.border_color(colors().focus))
}

/// A fixed-height ledger row. Callers supply the independently styled columns.
pub fn interaction_row(id: impl Into<SharedString>, selected: bool, member: bool) -> Stateful<Div> {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .id(id.into())
            .relative()
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .h(px(if member {
                t.size.interaction_member_row
            } else {
                t.size.group_row
            }))
            .px(px(t.space.s14))
            .gap(px(t.space.s10))
            .whitespace_nowrap()
            .overflow_hidden()
            .cursor_pointer()
            .bg(c.panel)
            .text_color(c.text_2)
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .role(gpui::accesskit::Role::Button)
            .aria_selected(selected)
            .focusable()
            .focus_visible(|el| {
                el.border(px(t.size.focus_ring))
                    .border_color(colors().focus)
            })
            .when(member, |el| {
                el.pl(px(t.size.interaction_burst_row)).child(
                    div()
                        .absolute()
                        .left(px(t.space.s20 + t.space.s2))
                        .top_0()
                        .bottom_0()
                        .w(px(t.size.hairline))
                        .bg(c.line_strong),
                )
            })
            .when(selected, card_selected)
            .when(!selected, |el| el.hover(|el| el.bg(colors().hover))),
        &t.text.row,
    )
}

/// The untouched message body has its own frame, separate from commentary.
pub fn message_frame(body: impl IntoElement) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .min_w_0()
            .p(px(t.space.s14))
            .bg(c.panel)
            .border(px(t.size.hairline))
            .border_color(c.line)
            .rounded(px(t.radius.slot))
            .line_height(relative(1.55))
            .text_color(c.text)
            .child(body),
        &t.text.doc_body,
    )
}

/// Direction is written as well as coloured.
pub fn direction_tag(sent: bool) -> Div {
    let (t, c) = (tokens(), colors());
    text(
        div()
            .flex()
            .items_center()
            .gap(px(t.space.s4))
            .px(px(t.space.s6))
            .border(px(t.size.hairline))
            .rounded(px(t.radius.pill))
            .border_color(if sent { c.line_strong } else { c.info })
            .text_color(if sent { c.text_2 } else { c.info })
            .child(if sent { "Sent" } else { "Received" }),
        &t.text.meta,
    )
}

/// A delivery mark; failed handoff is dotted rather than relying on colour.
pub fn message_tick(color: Rgba, dotted: bool) -> AnyElement {
    let t = tokens();
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            if !dotted {
                window.paint_quad(fill(bounds, color));
                return;
            }
            let vertical = bounds.size.height > bounds.size.width;
            let length = f32::from(if vertical {
                bounds.size.height
            } else {
                bounds.size.width
            });
            let cross = if vertical {
                bounds.size.width
            } else {
                bounds.size.height
            };
            let mut at = 0.;
            while at < length {
                let run = px(t.space.s2.min(length - at));
                let (offset, dimensions) = if vertical {
                    (point(px(0.), px(at)), size(cross, run))
                } else {
                    (point(px(at), px(0.)), size(run, cross))
                };
                window.paint_quad(fill(Bounds::new(bounds.origin + offset, dimensions), color));
                at += t.space.s4;
            }
        },
    )
    .size_full()
    .into_any_element()
}

/// A later explanation stays visibly separate from the captured original.
pub fn interaction_after_note() -> Div {
    let (t, c) = (tokens(), colors());
    super::text(
        div()
            .flex()
            .items_start()
            .gap(px(t.space.s6))
            .min_w_0()
            .text_color(c.muted)
            .child(super::icon("history", t.size.icon, c.muted))
            .child(div().flex_1().min_w_0().whitespace_normal().child(
                "Explained now, in reply to you. No reason was recorded when the message was sent.",
            )),
        &t.text.meta,
    )
}

fn interaction_button(button: Stateful<Div>, label: SharedString) -> Stateful<Div> {
    button
        .role(gpui::accesskit::Role::Button)
        .aria_label(label)
        .focusable()
        .focus_visible(|el| {
            el.border(px(tokens().size.focus_ring))
                .border_color(colors().focus)
        })
}

/// Named keyboard-focusable control using the shared chip treatment.
pub fn interaction_chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    selected: bool,
) -> Stateful<Div> {
    let label = label.into();
    interaction_button(super::chip(id, label.clone(), selected), label)
}

/// Named keyboard-focusable back control using the shared treatment.
pub fn interaction_back_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let label = label.into();
    interaction_button(super::back_button(id, label.clone()), label)
}

/// Named keyboard-focusable reader action.
pub fn interaction_action(
    id: impl Into<SharedString>,
    icon: &str,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    let label = label.into();
    interaction_button(super::action(id, icon, label.clone()), label)
}

/// A pictorial control still has a spoken name and visible focus ring.
pub fn interaction_icon_button(
    id: impl Into<SharedString>,
    icon: &str,
    label: impl Into<SharedString>,
) -> Stateful<Div> {
    interaction_button(super::icon_button(id, icon), label.into())
}
