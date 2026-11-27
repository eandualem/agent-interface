//! The peer list: a row per connection and request, the All and Waiting on
//! you choice, a filter, and this profile pinned at the bottom. Minimised, it
//! is a strip of marks; at [`Width::Narrow`] a chooser in the thread header
//! stands in for it.
use super::live::{Form, Tip, peer_mark};
use super::rules;
use super::*;
use crate::desktop::peer_inbox::{Selection, Summary};
use agent_interface_roster::profiles::peers as api;
use api::Direction;

/// How much room the pane has, which decides how the peer list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Width {
    /// The list shows open or minimised, as saved for the workspace.
    Wide,
    /// The list starts minimised, and a choice lasts for this pane only.
    Medium,
    /// A chooser in the thread header stands in for the list.
    Narrow,
}

impl Width {
    pub(super) fn of(width: f32) -> Self {
        let t = tokens();
        if width < t.size.connections_narrow {
            Self::Narrow
        } else if width < t.size.connections_split {
            Self::Medium
        } else {
            Self::Wide
        }
    }
}

/// How the thread header shows the two gates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Gates {
    /// Each gate's word, Incoming or Outgoing, beside its segments.
    Words,
    /// Only each gate's direction icon beside its segments; the word is
    /// its tooltip and accessible name.
    Icons,
    /// One button that opens both gates.
    Folded,
}

/// What the thread header shows at its width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct HeaderFit {
    pub gates: Gates,
    pub fingerprint: bool,
}

// Widths in px, measured from the Linux check's 1280 px capture of a
// connected thread's header (a 790 px thread column).
/// Left padding, the 24 px mark, a name allowance of 120 px (longer
/// names give way), the Connected tag (81 px) and the gaps between them.
const HEADER_NAMED: f32 = 20.0 + 24.0 + 10.0 + 120.0 + 10.0 + 81.0 + 10.0;
/// The fingerprint (82 px), the copy button and their gaps.
const HEADER_FINGERPRINT: f32 = 82.0 + 10.0 + 24.0 + 10.0;
/// Both gates with their words, from the Incoming icon to the end of ⋯.
const HEADER_WORDS: f32 = 433.0;
/// The same without the two words and their 6 px gaps (52 and 53 px).
const HEADER_ICONS: f32 = HEADER_WORDS - (6.0 + 52.0) - (6.0 + 53.0);
/// The folded gates button (147 px), a gap and ⋯.
const HEADER_FOLDED: f32 = 147.0 + 6.0 + 28.0;
/// The right padding.
const HEADER_END: f32 = 14.0;

/// The header's fit for a thread column `thread` px wide: the widest gates
/// that fit beside the name, then the fingerprint if it still has room.
pub(super) fn header_fit(width: Width, thread: f32) -> HeaderFit {
    let fits = |controls: f32| thread >= HEADER_NAMED + controls + HEADER_END;
    let (gates, controls) = if width == Width::Narrow {
        (Gates::Folded, HEADER_FOLDED)
    } else if fits(HEADER_WORDS) {
        (Gates::Words, HEADER_WORDS)
    } else if fits(HEADER_ICONS) {
        (Gates::Icons, HEADER_ICONS)
    } else {
        (Gates::Folded, HEADER_FOLDED)
    };
    HeaderFit {
        gates,
        fingerprint: width != Width::Narrow && fits(controls + HEADER_FINGERPRINT),
    }
}

/// The key a workspace's minimised peer list is saved under: its profile
/// and its workspace, or the workspace alone when it has no profile.
pub(super) fn list_key(profile: Option<&str>, workspace: &str) -> String {
    match profile {
        Some(profile) => format!("{profile}/{workspace}"),
        None => workspace.to_owned(),
    }
}

/// Set a pane's peer list key and profile name from where it is, and say
/// whether either changed.
pub(super) fn place(
    (key, profile_name): (&mut Option<String>, &mut Option<String>),
    profile: Option<&str>,
    workspace: &str,
    name: Option<&str>,
) -> bool {
    let next = (Some(list_key(profile, workspace)), name.map(str::to_owned));
    let changed = (&*key, &*profile_name) != (&next.0, &next.1);
    (*key, *profile_name) = next;
    changed
}

/// Whether the list is minimised: as saved when wide, else as chosen in
/// this pane, and minimised until something is chosen.
pub(super) fn minimised(width: Width, saved: bool, local: Option<bool>) -> bool {
    match width {
        Width::Wide => saved,
        _ => local.unwrap_or(true),
    }
}

/// A toggle: what to save for the workspace, if anything, and the pane's
/// own choice. Only a choice made wide is saved, so a narrow pane beside
/// another never changes how the wide list opens.
pub(super) fn toggle(
    width: Width,
    saved: bool,
    local: Option<bool>,
) -> (Option<bool>, Option<bool>) {
    let next = !minimised(width, saved, local);
    match width {
        Width::Wide => (Some(next), None),
        _ => (None, Some(next)),
    }
}

/// How a row's trailing word reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::desktop) enum Badge {
    /// Waiting on the person: a review or a request.
    Waiting,
    Quiet,
    /// A refusal, such as a declined request.
    Failed,
}

/// One row of the list.
#[derive(Debug, PartialEq)]
pub(in crate::desktop) struct Entry {
    pub selection: Selection,
    pub name: Option<String>,
    pub peer_id: String,
    pub connected: bool,
    pub badge: Option<(String, Badge)>,
    /// Whether it waits on the person: a review or a request, never a
    /// refusal, which only takes the attention colour.
    pub needs: bool,
}

impl Entry {
    pub(super) fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        rules::display_name(self.name.as_deref())
            .to_lowercase()
            .contains(&query)
            || self.peer_id.to_lowercase().contains(&query)
            || rules::fingerprint(&self.peer_id).contains(&query)
    }
}

/// The row the waiting count opens: the first that waits on the person
/// other than the one shown, else the one shown.
pub(super) fn next_waiting(rows: &[Entry], shown: Option<&Selection>) -> Option<Selection> {
    let waiting = || rows.iter().filter(|e| e.needs);
    waiting()
        .find(|e| Some(&e.selection) != shown)
        .or_else(|| waiting().next())
        .map(|e| e.selection.clone())
}

/// The rows for `summary`: requests not yet accepted, then connections.
pub(in crate::desktop) fn entries(summary: &Summary) -> Vec<Entry> {
    let overview = &summary.overview;
    let requests = overview
        .invitations
        .iter()
        .filter(|r| r.state != "accepted")
        .map(|r| {
            let badge = match (r.direction, r.state.as_str()) {
                (Direction::Incoming, "pending") => ("request".to_owned(), Badge::Waiting),
                (Direction::Outgoing, "pending") => ("pending".to_owned(), Badge::Quiet),
                (Direction::Outgoing, "declined") => ("declined".to_owned(), Badge::Failed),
                (_, state) => (state.replace('_', " "), Badge::Quiet),
            };
            Entry {
                selection: Selection::Request(r.request_id.clone()),
                name: r.display_name.clone(),
                peer_id: r.peer_id.clone(),
                connected: false,
                needs: badge.1 == Badge::Waiting,
                badge: Some(badge),
            }
        });
    let connections = overview.connections.iter().map(|p| {
        let count = summary.count(&p.connection.generation);
        let badge = if count > 0 {
            Some((format!("{count} to review"), Badge::Waiting))
        } else if p.state == "connected" {
            None
        } else if p.state == "revocation_pending" {
            // A disconnect the relay refused for good is a refusal.
            let refused = p.revocation_failure.is_some();
            Some((
                "stopped".to_owned(),
                if refused { Badge::Failed } else { Badge::Quiet },
            ))
        } else {
            Some((p.state.replace('_', " "), Badge::Quiet))
        };
        Entry {
            selection: Selection::Peer(p.connection.clone()),
            name: p.display_name.clone(),
            peer_id: p.connection.peer_id.clone(),
            connected: p.state == "connected",
            badge,
            needs: count > 0,
        }
    });
    requests.chain(connections).collect()
}

pub(in crate::desktop) fn badge_color(badge: Badge) -> gpui::Rgba {
    let c = colors();
    match badge {
        Badge::Waiting => c.accent_text,
        Badge::Quiet => c.muted,
        Badge::Failed => c.attention_text,
    }
}

impl Pane {
    /// Whether the list is minimised now.
    pub(super) fn list_minimised(&self) -> bool {
        let saved = self
            .list_key
            .as_deref()
            .is_some_and(crate::desktop::settings::peer_list_minimised);
        minimised(self.width, saved, self.list_local)
    }

    /// Minimise or show the list. Its minimise and show buttons share one
    /// focus handle, so focus moves to the button that takes the other's
    /// place.
    fn toggle_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.keep_focus(&self.list_focus.clone(), window, cx);
        let saved = self
            .list_key
            .as_deref()
            .is_some_and(crate::desktop::settings::peer_list_minimised);
        let (save, local) = toggle(self.width, saved, self.list_local);
        self.list_local = local;
        if let (Some(minimised), Some(key)) = (save, &self.list_key) {
            use crate::desktop::settings;
            settings::set_peer_list_minimised(key, minimised);
            settings::save_in_background(settings::Settings::current(), |result| {
                if let Err(error) = result {
                    tracing::warn!(%error, "Peer list choice not saved");
                }
            });
        }
        cx.notify();
    }

    fn plus(&self, cx: &Context<Self>) -> gpui::Stateful<Div> {
        ui::interaction_icon_button("connect-peer", "plus", "Connect a profile")
            .tab_stop(true)
            .track_focus(&self.entry)
            .on_click(cx.listener(|p, _, w, cx| {
                // ＋ closes its own popover, also when a press outside the
                // popover has just closed it.
                if p.connecting() || p.just_dismissed(Menu::Connect) {
                    p.live.form = None;
                    cx.notify();
                } else {
                    p.show_form(Form::Connect, w, cx)
                }
            }))
    }

    /// The open list, 248 px wide.
    pub(super) fn peer_list(
        &self,
        s: &Summary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let all = entries(s);
        // The same count as the footer's: requests and messages to review.
        let waiting = s.requests() + s.overview.pending_reviews;
        let query = self.filter.read(cx).value().to_owned();
        let scroll = self.list_scroll("live-peers-scroll", window, cx);
        let mut rows = div()
            .id("live-peers")
            .track_scroll(&scroll)
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(t.size.hairline));
        let mut shown = 0;
        for entry in all
            .into_iter()
            .filter(|e| (!self.needs_you || e.needs) && e.matches(&query))
        {
            shown += 1;
            let row = self.peer_row(entry, cx);
            rows = rows.child(self.in_view(row, None, &scroll, window, cx));
        }
        if shown == 0 {
            rows = rows.child(no_rows(
                if s.overview.connections.is_empty() && s.overview.invitations.is_empty() {
                    "No connections yet."
                } else {
                    "No matching peers"
                },
            ));
        }
        let mut segments = ui::segments()
            .border(px(t.size.hairline))
            .border_color(c.line_strong);
        for (needs, label) in [
            (false, "All".to_owned()),
            (true, format!("Waiting on you · {waiting}")),
        ] {
            segments = segments.child(
                ui::segment(format!("filter-{needs}"), label, self.needs_you == needs)
                    .focus_visible(|el| {
                        el.border(px(tokens().size.focus_ring))
                            .border_color(colors().focus)
                    })
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
            .w(px(t.size.peer_list))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .p(px(t.space.s8))
            .border_r(px(t.size.hairline))
            .border_color(c.line)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s2))
                    .h(px(t.size.row))
                    .pl(px(t.space.s8))
                    .child(
                        ui::text(div().flex_1().text_color(c.muted), &t.text.label).child("PEERS"),
                    )
                    .child(self.plus(cx))
                    .child(
                        ui::interaction_icon_button(
                            "minimise-peers",
                            "panel-left-close",
                            "Minimise the peer list",
                        )
                        .track_focus(&self.list_focus)
                        .on_click(cx.listener(|p, _, w, cx| p.toggle_list(w, cx))),
                    ),
            )
            .child(segments.mt(px(t.space.s2)).mb(px(t.space.s8)))
            .child(ui::filter_frame().mb(px(t.space.s8)).child(hinted(
                &self.filter,
                "Filter peers",
                cx,
            )))
            .child(rows)
            .child(self.this_profile(&s.overview.local_id, cx))
    }

    fn peer_row(&self, entry: Entry, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let (t, c) = (tokens(), colors());
        let selected = self.live.selected.as_ref() == Some(&entry.selection);
        let id = match &entry.selection {
            Selection::Peer(g) => format!("peer-{}", g.generation),
            Selection::Request(r) => format!("invite-{r}"),
        };
        let named = entry.name.as_deref().is_some_and(|n| !n.trim().is_empty());
        let second = if named && entry.connected {
            ui::text(
                div().flex().items_center().gap(px(t.space.s4)),
                &t.text.meta,
            )
            .text_color(c.muted)
            .child(ui::dot(c.ok))
            .child("connected")
        } else {
            ui::text(div().font_family(t.font.mono.clone()), &t.text.key)
                .text_color(c.muted)
                .child(rules::fingerprint(&entry.peer_id))
        };
        let selection = entry.selection.clone();
        div()
            .id(id)
            .focus_visible(|el| {
                el.border(px(tokens().size.focus_ring))
                    .border_color(colors().focus)
            })
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(t.space.s10))
            .h(px(t.size.peer_row))
            .px(px(t.space.s8))
            .rounded(px(t.radius.row))
            .whitespace_nowrap()
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .when(selected, |el| el.bg(c.selected))
            .when(!selected, |el| el.hover(|el| el.bg(colors().hover)))
            .child(peer_mark(
                entry.name.as_deref(),
                t.size.view_tile - t.space.s4,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(t.space.s2))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(if named { c.text } else { c.text_2 })
                            .when(named, |el| el.font_weight(t.text.row_selected.weight()))
                            .child(rules::display_name(entry.name.as_deref()).to_owned()),
                    )
                    .child(second),
            )
            .children(entry.badge.map(|(label, badge)| {
                ui::text(
                    div().flex_shrink_0().text_color(badge_color(badge)),
                    &t.text.meta,
                )
                .child(label)
            }))
            .on_click(cx.listener(move |p, _, _, cx| p.pick(Some(selection.clone()), cx)))
    }

    /// This profile's mark, name and short fingerprint, with Copy ID.
    fn this_profile(&self, local: &str, cx: &Context<Self>) -> Div {
        let (t, c) = (tokens(), colors());
        let name = self.profile_name.as_deref().unwrap_or("This profile");
        let copied = self.copied.as_deref() == Some("copy-local-id");
        let local = local.to_owned();
        div()
            .flex()
            .flex_col()
            .gap(px(t.space.s8))
            .mx(px(-t.space.s8))
            .px(px(t.space.s16))
            .pt(px(t.space.s10))
            .pb(px(t.space.s4))
            .border_t(px(t.size.hairline))
            .border_color(c.line)
            .child(ui::text(div().text_color(c.muted), &t.text.label).child("THIS PROFILE"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(t.space.s10))
                    .whitespace_nowrap()
                    .child(ui::profile_mark(name, t.size.view_tile - t.space.s4))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(t.space.s2))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_color(c.text)
                                    .child(name.to_owned()),
                            )
                            .child(
                                ui::text(div().font_family(t.font.mono.clone()), &t.text.key)
                                    .text_color(c.muted)
                                    .child(rules::fingerprint(&local)),
                            ),
                    )
                    .child(
                        small_button("copy-local-id", if copied { "Copied" } else { "Copy ID" })
                            .on_click(cx.listener(move |p, _, _, cx| {
                                p.copy("copy-local-id".into(), local.clone(), cx)
                            })),
                    ),
            )
    }

    /// The minimised list: a 48 px strip of marks.
    pub(super) fn peer_strip(
        &self,
        s: &Summary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let strip = div()
            .w(px(t.size.peer_strip))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(t.space.s6))
            .py(px(t.space.s8))
            .border_r(px(t.size.hairline))
            .border_color(c.line)
            .child(
                ui::interaction_icon_button(
                    "expand-peers",
                    "panel-left-open",
                    "Show the peer list",
                )
                .track_focus(&self.list_focus)
                .on_click(cx.listener(|p, _, w, cx| p.toggle_list(w, cx))),
            )
            .child(self.plus(cx))
            .child(
                div()
                    .w(px(t.size.view_tile - t.space.s4))
                    .h(px(t.size.hairline))
                    .my(px(t.space.s2))
                    .bg(c.line),
            );
        let scroll = self.list_scroll("peer-strip-scroll", window, cx);
        let mut marks = div()
            .id("peer-strip")
            .track_scroll(&scroll)
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(t.space.s6));
        for entry in entries(s) {
            let selected = self.live.selected.as_ref() == Some(&entry.selection);
            let id = match &entry.selection {
                Selection::Peer(g) => format!("strip-{}", g.generation),
                Selection::Request(r) => format!("strip-{r}"),
            };
            let selection = entry.selection.clone();
            // The strip shows only marks, so each says whose it is on hover.
            let mut tip = rules::display_name(entry.name.as_deref()).to_owned();
            if entry.name.as_deref().is_none_or(|n| n.trim().is_empty()) {
                tip = format!("{tip} {}", rules::fingerprint(&entry.peer_id));
            }
            if let Some((label, _)) = &entry.badge {
                tip = format!("{tip} · {label}");
            }
            let mark = mark_button(id, selected)
                .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone().into())).into())
                .child(peer_mark(
                    entry.name.as_deref(),
                    t.size.view_tile - t.space.s4,
                ))
                .children(
                    entry
                        .badge
                        .as_ref()
                        .and_then(|(_, badge)| match badge {
                            Badge::Waiting => Some(c.accent),
                            Badge::Failed => Some(c.attention),
                            Badge::Quiet => None,
                        })
                        .map(|dot| {
                            div()
                                .absolute()
                                .top(px(t.space.s2))
                                .right(px(t.space.s2))
                                .size(px(t.space.s8 + t.space.s4))
                                .rounded_full()
                                .bg(dot)
                                .border(px(t.space.s2))
                                .border_color(if selected { c.selected } else { c.surface })
                        }),
                )
                .on_click(cx.listener(move |p, _, _, cx| p.pick(Some(selection.clone()), cx)));
            marks = marks.child(self.in_view(mark, None, &scroll, window, cx));
        }
        let name = self
            .profile_name
            .as_deref()
            .unwrap_or("This profile")
            .to_owned();
        let local = s.overview.local_id.clone();
        let copied = self.copied.as_deref() == Some("strip-local");
        let tip: SharedString = format!("{name}: copy this profile's connection ID").into();
        strip.child(marks).child(
            mark_button("strip-local", false)
                .tooltip(move |_, cx| cx.new(|_| Tip(tip.clone())).into())
                .map(|el| {
                    if copied {
                        el.child(ui::icon("check", t.size.icon, c.ok))
                    } else {
                        el.child(ui::profile_mark(&name, t.size.view_tile - t.space.s4))
                    }
                })
                .on_click(
                    cx.listener(move |p, _, _, cx| p.copy("strip-local".into(), local.clone(), cx)),
                ),
        )
    }

    /// Under the narrow width: the chosen peer as a button whose menu lists
    /// every peer and the way to connect another.
    pub(super) fn peer_chooser(
        &self,
        s: &Summary,
        name: Option<&str>,
        label: &str,
        peer_id: Option<&str>,
        cx: &Context<Self>,
    ) -> Div {
        let (t, c) = (tokens(), colors());
        let waiting = entries(s)
            .iter()
            .any(|e| e.needs && Some(&e.selection) != self.live.selected.as_ref());
        let mut chooser = div().relative().min_w_0().child(
            div()
                .id("peer-chooser")
                .focus_visible(|el| {
                    el.border(px(tokens().size.focus_ring))
                        .border_color(colors().focus)
                })
                .flex()
                .items_center()
                .gap(px(t.space.s8))
                .min_w_0()
                .h(px(t.size.icon_button + t.space.s4))
                .pl(px(t.space.s6))
                .pr(px(t.space.s8))
                .rounded(px(t.radius.row))
                .border(px(t.size.hairline))
                .border_color(c.line_strong)
                .bg(c.field)
                .cursor_pointer()
                // ＋ is not drawn this narrow, so the chooser takes the
                // pane's first focus and gets it back when a menu closes.
                .track_focus(&self.entry)
                .hover(|el| el.bg(colors().hover))
                .child(peer_mark(name, t.size.message_avatar))
                .child(
                    ui::text(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(c.text),
                        &t.text.row_selected,
                    )
                    .child(label.to_owned()),
                )
                // An unnamed connection is told apart by its fingerprint.
                .children(
                    peer_id
                        .filter(|_| rules::display_name(name) == "Unnamed profile")
                        .map(|id| {
                            ui::text(
                                div().flex_shrink_0().font_family(t.font.mono.clone()),
                                &t.text.key,
                            )
                            .text_color(c.muted)
                            .child(rules::fingerprint(id))
                        }),
                )
                .when(waiting, |el| el.child(ui::dot(c.accent)))
                .child(ui::icon("chevron-down", t.size.icon - 2.0, c.muted))
                .on_click(cx.listener(|p, _, w, cx| p.toggle_menu(Menu::Peers, w, cx))),
        );
        if self.menu == Menu::Peers {
            let mut menu = self
                .menu_frame(cx)
                .absolute()
                .top(px(t.size.button + t.space.s4))
                .left_0()
                .w(px(t.size.peer_list));
            for entry in entries(s) {
                let selection = entry.selection.clone();
                let id = match &entry.selection {
                    Selection::Peer(g) => format!("choose-{}", g.generation),
                    Selection::Request(r) => format!("choose-{r}"),
                };
                menu = menu.child(
                    ui::menu_item(
                        id,
                        None,
                        rules::display_name(entry.name.as_deref()).to_owned(),
                        None,
                    )
                    .focus_visible(|el| {
                        el.border(px(tokens().size.focus_ring))
                            .border_color(colors().focus)
                    })
                    // An unnamed connection is told apart by its fingerprint.
                    .when(
                        entry.name.as_deref().is_none_or(|n| n.trim().is_empty()),
                        |el| {
                            el.child(
                                ui::text(
                                    div().flex_shrink_0().font_family(t.font.mono.clone()),
                                    &t.text.key,
                                )
                                .text_color(c.muted)
                                .child(rules::fingerprint(&entry.peer_id)),
                            )
                        },
                    )
                    .children(entry.badge.map(|(label, badge)| {
                        ui::text(
                            div().flex_shrink_0().text_color(badge_color(badge)),
                            &t.text.meta,
                        )
                        .child(label)
                    }))
                    .focusable()
                    .tab_stop(true)
                    .on_click(cx.listener(move |p, _, w, cx| {
                        p.keep_focus(&p.entry.clone(), w, cx);
                        p.pick(Some(selection.clone()), cx)
                    })),
                );
            }
            menu = menu.child(ui::menu_rule()).child(
                ui::menu_item(
                    "choose-connect",
                    Some(ui::icon("plus", t.size.icon, c.muted)),
                    "Connect a profile",
                    None,
                )
                .focus_visible(|el| {
                    el.border(px(tokens().size.focus_ring))
                        .border_color(colors().focus)
                })
                .focusable()
                .tab_stop(true)
                .on_click(cx.listener(|p, _, w, cx| {
                    p.menu = Menu::None;
                    p.show_form(Form::Connect, w, cx)
                })),
            );
            chooser = chooser.child(gpui::deferred(menu).with_priority(1));
        }
        chooser
    }
}

/// A 36 px square holding a mark, `selected` filled.
fn mark_button(id: impl Into<SharedString>, selected: bool) -> gpui::Stateful<Div> {
    let (t, c) = (tokens(), colors());
    let size = t.size.view_tile + t.space.s8;
    div()
        .id(id.into())
        .focus_visible(|el| {
            el.border(px(tokens().size.focus_ring))
                .border_color(colors().focus)
        })
        .relative()
        .flex_shrink_0()
        .size(px(size))
        .rounded(px(t.radius.row))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .focusable()
        .tab_stop(true)
        .when(selected, |el| el.bg(c.selected))
        .when(!selected, |el| el.hover(|el| el.bg(colors().hover)))
}

/// A small bordered text button, such as Copy ID.
pub(super) fn small_button(id: &'static str, label: &'static str) -> gpui::Stateful<Div> {
    let (t, c) = (tokens(), colors());
    ui::text(
        div()
            .id(id)
            .focus_visible(|el| {
                el.border(px(tokens().size.focus_ring))
                    .border_color(colors().focus)
            })
            .flex_shrink_0()
            .flex()
            .items_center()
            .h(px(t.size.icon_button))
            .px(px(t.space.s8))
            .rounded(px(t.radius.icon_button))
            .border(px(t.size.hairline))
            .border_color(c.line_strong)
            .bg(c.field)
            .text_color(c.text_2)
            .cursor_pointer()
            .focusable()
            .tab_stop(true)
            .hover(|el| el.bg(colors().hover)),
        &t.text.meta,
    )
    .child(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_list_is_saved_by_profile_and_workspace() {
        assert_eq!(
            list_key(Some("0123abcd"), "4c1f0e9a2b3d"),
            "0123abcd/4c1f0e9a2b3d"
        );
        assert_eq!(list_key(None, "4c1f0e9a2b3d"), "4c1f0e9a2b3d");
        // A profile name that loads after the pane was placed still reaches it.
        let (mut key, mut name) = (None, None);
        assert!(place(
            (&mut key, &mut name),
            Some("0123abcd"),
            "4c1f0e9a2b3d",
            None
        ));
        assert!(!place(
            (&mut key, &mut name),
            Some("0123abcd"),
            "4c1f0e9a2b3d",
            None
        ));
        assert!(place(
            (&mut key, &mut name),
            Some("0123abcd"),
            "4c1f0e9a2b3d",
            Some("Work")
        ));
        assert_eq!(
            (key.as_deref(), name.as_deref()),
            (Some("0123abcd/4c1f0e9a2b3d"), Some("Work"))
        );
    }

    #[test]
    fn only_a_wide_choice_is_saved_and_narrower_lists_start_minimised() {
        use Width::{Medium, Narrow, Wide};
        assert!(!minimised(Wide, false, None));
        assert!(minimised(Wide, true, Some(false)));
        assert!(minimised(Medium, false, None));
        assert!(minimised(Narrow, false, None));
        // Wide: the choice is saved and the pane keeps none of its own.
        assert_eq!(toggle(Wide, false, None), (Some(true), None));
        assert_eq!(toggle(Wide, true, None), (Some(false), None));
        // Medium: the pane opens its own list; nothing is saved.
        assert_eq!(toggle(Medium, true, None), (None, Some(false)));
        assert_eq!(toggle(Medium, false, Some(false)), (None, Some(true)));
    }

    #[test]
    fn rows_say_what_waits_and_never_show_a_raw_id_as_a_name() {
        let sample = model::Sample::new();
        let summary = Summary::with_counts(sample.overview.clone(), sample.counts());
        let rows = entries(&summary);
        let badge = |name: Option<&str>, state: &str| {
            rows.iter()
                .find(|e| {
                    e.name.as_deref() == name
                        && e.badge.as_ref().map(|b| b.0.as_str()) == Some(state)
                })
                .map(|e| (e.badge.clone().unwrap().1, e.needs))
        };
        assert_eq!(badge(None, "request"), Some((Badge::Waiting, true)));
        assert_eq!(badge(None, "pending"), Some((Badge::Quiet, false)));
        // A refusal takes the attention colour but does not wait on the person.
        assert_eq!(
            badge(Some("Staging"), "declined"),
            Some((Badge::Failed, false))
        );
        assert_eq!(
            badge(Some("Harbor"), "1 to review"),
            Some((Badge::Waiting, true))
        );
        let unnamed = rows.iter().find(|e| e.name.is_none()).unwrap();
        assert!(unnamed.matches(&rules::fingerprint(&unnamed.peer_id)[..8]));
        assert!(rows.iter().any(|e| e.matches("harbor")));
        assert_eq!(rows.iter().filter(|e| e.needs).count(), 3);
        assert_eq!(badge(None, "stopped"), Some((Badge::Quiet, false)));
        let mut refused = sample.overview.clone();
        for p in &mut refused.connections {
            if p.state == "revocation_pending" {
                p.revocation_failure = Some(api::RevocationFailure {
                    code: "generation_mismatch".into(),
                });
            }
        }
        let rows = entries(&Summary::with_counts(refused, sample.counts()));
        assert!(
            rows.iter()
                .any(|e| e.badge == Some(("stopped".into(), Badge::Failed)))
        );
    }

    #[test]
    fn the_waiting_count_opens_a_review_or_request_never_a_refusal() {
        let sample = model::Sample::new();
        let rows = entries(&Summary::with_counts(
            sample.overview.clone(),
            sample.counts(),
        ));
        let first = next_waiting(&rows, None).unwrap();
        assert!(
            rows.iter()
                .any(|e| e.selection == first && e.badge.as_ref().unwrap().1 == Badge::Waiting)
        );
        // From the first waiting row, it moves on to the next one.
        let second = next_waiting(&rows, Some(&first)).unwrap();
        assert_ne!(second, first);
        let declined = rows
            .iter()
            .find(|e| e.badge.as_ref().is_some_and(|b| b.1 == Badge::Failed))
            .unwrap();
        for shown in rows.iter().map(|e| Some(&e.selection)) {
            assert_ne!(next_waiting(&rows, shown), Some(declined.selection.clone()));
        }
        // Nothing waiting, nothing to open.
        let quiet: Vec<_> = rows.into_iter().filter(|e| !e.needs).collect();
        assert_eq!(next_waiting(&quiet, None), None);
    }

    #[test]
    fn the_header_drops_words_then_folds_its_gates_as_it_narrows() {
        use Gates::{Folded, Icons, Words};
        let fit = |width, thread| {
            let HeaderFit { gates, fingerprint } = header_fit(width, thread);
            (gates, fingerprint)
        };
        // Words need 722 px, icons 605 px, the fingerprint 126 px more.
        assert_eq!(fit(Width::Wide, 1100.0), (Words, true));
        // A 1280 px window with the list open leaves about 790 px.
        assert_eq!(fit(Width::Wide, 790.0), (Words, false));
        assert_eq!(fit(Width::Wide, 722.0), (Words, false));
        assert_eq!(fit(Width::Wide, 721.0), (Icons, false));
        assert_eq!(fit(Width::Medium, 606.0), (Icons, false));
        // A 1040 px window with the list open leaves about 560 px.
        assert_eq!(fit(Width::Wide, 560.0), (Folded, false));
        assert_eq!(fit(Width::Narrow, 540.0), (Folded, false));
    }

    #[test]
    fn the_width_decides_how_the_list_shows() {
        use Width::{Medium, Narrow, Wide};
        assert_eq!(
            [559.0, 560.0, 719.0, 720.0].map(Width::of),
            [Narrow, Medium, Medium, Wide]
        );
    }
}
