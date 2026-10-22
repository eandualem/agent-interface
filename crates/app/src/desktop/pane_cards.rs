//! Panes as cards: each with an optional header, a focus ring on the focused
//! pane, dimmed content elsewhere, and the gap between them for resizing.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, Bounds, BoxShadow, Context, Entity, Pixels, SharedString, div, point, prelude::*,
    px,
};
use tmux_workspaces_core::{Direction, Leaf, Node};

use super::{
    Attaching, Browser, components as ui,
    design::{colors, tokens},
    settings::{self, PaneHeaders},
    shortcuts::{self, Command},
    terminal::LiveTerminal,
};
use ui::AgentState;

/// What every card in one tree needs to know.
pub(super) struct Cards<'a> {
    pub focus: &'a str,
    pub numbers: &'a [String],
    pub terminals: &'a HashMap<String, Entity<LiveTerminal>>,
    pub chooser: Option<&'a Attaching>,
    pub split_bounds: &'a Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    /// The agent state shown in each pane, by leaf.
    pub leaf_states: HashMap<String, AgentState>,
}

impl Browser {
    pub(super) fn tree(node: &Node, cards: &Cards<'_>, cx: &mut Context<Self>) -> AnyElement {
        match node {
            Node::Leaf(leaf) => Self::card(leaf, cards, cx),
            Node::Split {
                id,
                direction,
                ratio,
                first,
                second,
                ..
            } => {
                let t = tokens();
                div()
                    .relative()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .flex()
                    .when(*direction == Direction::Below, |el| el.flex_col())
                    .child(
                        div()
                            .flex_basis(px(0.0))
                            .flex_grow(*ratio as f32)
                            .min_w_0()
                            .min_h_0()
                            .child(Self::tree(first, cards, cx)),
                    )
                    .child(
                        // The gap between panes is where they resize.
                        div()
                            .id(SharedString::from(format!("divider-{id}")))
                            .flex_shrink_0()
                            .when(*direction == Direction::Right, |el| {
                                el.w(px(t.size.pane_gap))
                                    .h_full()
                                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                            })
                            .when(*direction == Direction::Below, |el| {
                                el.h(px(t.size.pane_gap))
                                    .w_full()
                                    .cursor(gpui::CursorStyle::ResizeUpDown)
                            })
                            .on_mouse_down(
                                gpui::MouseButton::Left,
                                cx.listener({
                                    let id = id.clone();
                                    move |browser, event: &gpui::MouseDownEvent, window, cx| {
                                        browser.begin_resize(&id, event.position, window, cx);
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex_basis(px(0.0))
                            .flex_grow((1.0 - ratio) as f32)
                            .min_w_0()
                            .min_h_0()
                            .child(Self::tree(second, cards, cx)),
                    )
                    .child(
                        gpui::canvas(|_, _, _| (), {
                            let id = id.clone();
                            let bounds = Rc::clone(cards.split_bounds);
                            move |area, (), _, _| {
                                bounds.borrow_mut().insert(id.clone(), area);
                            }
                        })
                        .absolute()
                        .size_full(),
                    )
                    .into_any_element()
            }
        }
    }

    fn card(leaf: &Leaf, cards: &Cards<'_>, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let id = leaf.id.clone();
        let focused = leaf.id == cards.focus;
        let state = cards.leaf_states.get(&leaf.id).copied();
        let group: SharedString = format!("pane-group-{id}").into();
        let border = if state == Some(AgentState::NeedsYou) {
            c.attention
        } else if focused {
            c.focus
        } else {
            c.edge
        };
        let header = Self::pane_header(leaf, focused, state, cards, cx);
        let (inline, overlay) = match settings::pane_headers() {
            PaneHeaders::Always => (Some(header), None),
            PaneHeaders::Hover => (None, Some(header)),
            PaneHeaders::Off => (None, None),
        };
        let body = if let Some(chooser) = cards
            .chooser
            .filter(|chooser| chooser.destination.pane == leaf.id)
        {
            chooser.view.clone().into_any_element()
        } else if let Some(terminal) = cards.terminals.get(&leaf.id) {
            terminal.clone().into_any_element()
        } else {
            Self::new_pane(&leaf.id, cx)
        };
        div()
            .id(SharedString::from(format!("pane-{id}")))
            .group(group.clone())
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(c.surface)
            .rounded(px(t.radius.pane))
            .border(px(t.size.hairline))
            .border_color(border)
            .when(focused, |el| {
                el.shadow(vec![BoxShadow {
                    color: c.focus_halo.into(),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: px(t.size.focus_halo),
                    inset: false,
                }])
            })
            // A left click selects the pane. A right or middle click selects it
            // only if the terminal took keyboard focus (a mouse-enabled program
            // accepts those clicks), so pane commands follow the typing.
            .capture_any_mouse_down(cx.listener({
                let id = id.clone();
                move |browser, event: &gpui::MouseDownEvent, window, cx| {
                    let leaf = id.clone();
                    let id = id.clone();
                    let select = move |browser: &mut Self, cx: &mut Context<Self>| {
                        if let Some(layout) = &mut browser.layout
                            && layout.model().tab().is_some_and(|tab| tab.focus != id)
                        {
                            layout.model_mut().choose_pane(&id);
                            cx.notify();
                        }
                    };
                    if event.button == gpui::MouseButton::Left {
                        select(browser, cx);
                    } else {
                        cx.defer_in(window, move |browser, window, cx| {
                            if browser
                                .terminals
                                .get(&leaf)
                                .is_some_and(|terminal| terminal.read(cx).has_focus(window, cx))
                            {
                                select(browser, cx);
                            }
                        });
                    }
                }
            }))
            .children(inline)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .when(!focused, |el| el.opacity(c.pane_dim))
                    .child(body),
            )
            // In hover mode the header overlays the top of the pane while it is
            // hovered. The pointer is always over the pane when it is over the
            // header, so the header is visible whenever it takes the pointer.
            // Presses and scrolling there stay with the header; releases and
            // moves pass on, so a gesture begun in the terminal still ends.
            .children(overlay.map(|header| {
                div()
                    .id(SharedString::from(format!("pane-hover-header-{id}")))
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .opacity(0.0)
                    .group_hover(group, |style| style.opacity(1.0))
                    .on_any_mouse_down(cx.listener(
                        |browser, event: &gpui::MouseDownEvent, window, cx| {
                            if event.button == gpui::MouseButton::Left
                                && let Some(focus) = &browser.navigation
                            {
                                window.focus(focus, cx);
                            }
                            cx.stop_propagation();
                        },
                    ))
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(header)
            }))
            .into_any_element()
    }

    /// The 28 px header: what runs there, where, and the pane's number.
    fn pane_header(
        leaf: &Leaf,
        focused: bool,
        state: Option<AgentState>,
        cards: &Cards<'_>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (t, c) = (tokens(), colors());
        let number = cards
            .numbers
            .iter()
            .position(|id| id == &leaf.id)
            .map_or(0, |n| n + 1);
        let terminal = cards.terminals.get(&leaf.id);
        let mut row = ui::text(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(px(t.space.s8))
                .h(px(t.size.pane_header))
                .px(px(t.space.s12))
                .bg(c.surface)
                .border_b(px(t.size.hairline))
                .border_color(c.line)
                .text_color(c.muted)
                .overflow_hidden()
                .whitespace_nowrap(),
            &t.text.status,
        );
        let icon_color = if focused { c.accent } else { c.muted };
        let title_color = if focused { c.text } else { c.text_2 };
        let windows = terminal
            .map(|terminal| terminal.read(cx).windows_for_header())
            .unwrap_or_default();
        let several = windows.len() > 1;
        if several {
            // The session's windows scroll sideways when they outgrow the pane.
            let mut choices = div()
                .id(SharedString::from(format!("tmux-windows-{}", leaf.id)))
                .flex_1()
                .min_w_0()
                .flex()
                .gap(px(t.space.s8))
                .overflow_x_scroll();
            for (window, current) in windows {
                let terminal = terminal.cloned();
                choices = choices.child(
                    div()
                        .id(SharedString::from(format!(
                            "tmux-window-{}-{}",
                            leaf.id, window.id
                        )))
                        .cursor_pointer()
                        .text_color(if current { title_color } else { c.muted })
                        .hover(|el| el.text_color(colors().text))
                        .child(
                            window
                                .name
                                .chars()
                                .map(|ch| if ch.is_control() { ' ' } else { ch })
                                .collect::<String>(),
                        )
                        .on_click(move |_, _, cx| {
                            if let Some(terminal) = &terminal {
                                terminal.update(cx, |terminal, cx| {
                                    terminal.select_window(window.clone(), cx)
                                });
                            }
                        }),
                );
            }
            row = row.child(choices);
        }
        match &leaf.attachment {
            Some(attachment) => {
                // An attached session: its tmux windows, its name on the right.
                if !several {
                    row = row
                        .child(ui::icon(
                            if state.is_some() { "bot" } else { "link-2" },
                            t.size.icon - t.space.s2 / 2.0,
                            icon_color,
                        ))
                        .child(
                            div()
                                .text_color(title_color)
                                .font_weight(t.text.row_selected.weight())
                                .child(attachment.session.clone()),
                        );
                }
                row = match state {
                    Some(AgentState::Working) => {
                        row.child(Self::chip("working", c.accent_soft, c.accent_text))
                    }
                    Some(AgentState::NeedsYou) => {
                        row.child(Self::chip("needs you", c.attention_soft, c.attention_text))
                    }
                    _ => row,
                };
                if !several {
                    row = row.child(div().flex_1());
                }
                if several {
                    row = row
                        .child(ui::icon(
                            "link-2",
                            t.size.icon - t.space.s2 / 2.0,
                            icon_color,
                        ))
                        .child(
                            div()
                                .text_color(title_color)
                                .child(attachment.session.clone()),
                        );
                }
            }
            None => {
                let cwd = leaf
                    .cwd
                    .as_deref()
                    .map(|path| home_relative(path, std::env::var("HOME").ok().as_deref()))
                    .unwrap_or_default();
                row = row
                    .child(ui::icon(
                        "terminal",
                        t.size.icon - t.space.s2 / 2.0,
                        icon_color,
                    ))
                    .child(div().text_color(title_color).child(if leaf.empty {
                        "New pane"
                    } else {
                        "Terminal"
                    }))
                    .child(
                        div()
                            .min_w_0()
                            // Share remaining space with the window choices;
                            // a long directory must not collapse their width.
                            .when(several, |el| el.flex_1())
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(cwd),
                    );
                if !several {
                    row = row.child(div().flex_1());
                }
            }
        }
        row.child(
            div()
                .w(px(t.size.pane_number_width))
                .flex_shrink_0()
                .font_family(t.font.mono.clone())
                .text_size(t.text.key.size())
                .child(number.to_string()),
        )
    }

    fn chip(label: &'static str, fill: gpui::Rgba, color: gpui::Rgba) -> gpui::Div {
        let t = tokens();
        ui::text(
            div()
                .flex()
                .items_center()
                .px(px(t.space.s6))
                .rounded(px(t.radius.popover))
                .bg(fill)
                .text_color(color),
            &t.text.meta,
        )
        .child(label)
    }

    /// An empty pane asks what it should run; nothing is created until then.
    fn new_pane(pane: &str, cx: &mut Context<Self>) -> AnyElement {
        let (t, c) = (tokens(), colors());
        let terminal = pane.to_owned();
        let attach = pane.to_owned();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(t.space.s14))
                    .p(px(t.space.s24))
                    .child(
                        ui::text(div().text_color(c.text), &t.text.empty_title)
                            .child("What should it run?"),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s8))
                            .w(px(t.size.popover_min_width))
                            .child(
                                ui::button(
                                    SharedString::from(format!("open-terminal-{pane}")),
                                    "Open terminal",
                                    ui::Button::Primary,
                                )
                                .child(div().flex_1())
                                .child(ui::key_hint("↵").text_color(c.on_accent))
                                .on_click(cx.listener(
                                    move |browser, _, window, cx| {
                                        cx.stop_propagation();
                                        browser.open_terminal(&terminal, window, cx);
                                    },
                                )),
                            )
                            .child(
                                ui::button(
                                    SharedString::from(format!("attach-{pane}")),
                                    "Attach a session",
                                    ui::Button::Secondary,
                                )
                                .child(div().flex_1())
                                .child(ui::key_hint(shortcuts::label(Command::Attach)))
                                .on_click(cx.listener(
                                    move |browser, _, window, cx| {
                                        cx.stop_propagation();
                                        browser.open_attach(&attach, window, cx);
                                    },
                                )),
                            ),
                    ),
            )
            .child(
                ui::text(
                    div()
                        .h(px(t.size.button + t.space.s4))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_t(px(t.size.hairline))
                        .border_color(c.line)
                        .text_color(c.muted),
                    &t.text.status,
                )
                .child("Nothing is created until you choose."),
            )
            .into_any_element()
    }
}

/// `path` with the home directory shown as `~`.
fn home_relative(path: &str, home: Option<&str>) -> String {
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return path.to_owned();
    };
    match std::path::Path::new(path).strip_prefix(home) {
        Ok(relative) if relative.as_os_str().is_empty() => "~".to_owned(),
        Ok(relative) => format!("~/{}", relative.display()),
        Err(_) => path.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::home_relative;

    #[test]
    fn home_abbreviation_respects_directory_boundaries() {
        let home = Some("/home/user");
        assert_eq!(home_relative("/home/user", home), "~");
        assert_eq!(home_relative("/home/user/project", home), "~/project");
        assert_eq!(
            home_relative("/home/user2/project", home),
            "/home/user2/project"
        );
        assert_eq!(home_relative("/work", Some("")), "/work");
    }
}
