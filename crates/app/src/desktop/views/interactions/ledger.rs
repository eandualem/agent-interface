//! Build rows before drawing. The rows and rail consume the same geometry;
//! grouping never manufactures message relations or runtime observations.

use std::collections::{HashMap, HashSet};

use super::{
    layout::{self, Line, Rail, Time},
    window::TimeWindow,
};
use crate::desktop::{design::tokens, views::clock::Local};

pub(super) struct Event {
    pub index: usize,
    pub id: String,
    pub at: f64,
    pub peer: String,
    pub conversation: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum Row {
    Message(usize),
    Burst {
        key: String,
        members: Vec<usize>,
        expanded: bool,
    },
    Member(usize),
    Conversation {
        anchor: String,
        members: Vec<usize>,
    },
    Quiet {
        start: f64,
        end: f64,
    },
    Group {
        label: String,
        count: usize,
        same_second: Option<(f64, usize)>,
    },
    Gap {
        start: f64,
        end: f64,
        inside: bool,
    },
    Edge,
}

struct Unit {
    row: Row,
    top: f64,
    bottom: f64,
    members: Vec<usize>,
}

/// Only visible messages are supplied. Quiet rows require an unfiltered,
/// complete message page: hidden or unretained messages are not silence.
pub(super) fn build(
    events: &[Event],
    window: TimeWindow,
    history: Option<f64>,
    quiet_intervals: &[(f64, f64)],
    expanded: &HashSet<String>,
) -> Rail<Row> {
    let t = tokens();
    let lo = history
        .map_or(window.start, |at| at.max(window.start))
        .min(window.end);
    let gap = |seconds| {
        layout::gap_height(
            seconds,
            t.size.interaction_gap_unit,
            t.size.interaction_gap_max,
        )
    };
    let mut seconds = HashMap::<i64, usize>::new();
    for event in events {
        *seconds.entry(event.at.floor() as i64).or_default() += 1;
    }
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by(|a, b| b.at.total_cmp(&a.at).then(a.index.cmp(&b.index)));
    let mut units = Vec::new();
    let by_index: HashMap<_, _> = events.iter().map(|event| (event.index, event)).collect();
    let mut cursor = 0;
    while cursor < ordered.len() {
        let first = ordered[cursor];
        let mut end = cursor + 1;
        if let Some(anchor) = &first.conversation {
            // Interleaved conversations form separate contiguous runs. A run
            // never absorbs an unrelated message or makes the rail go backwards.
            while end < ordered.len() && ordered[end].conversation.as_ref() == Some(anchor) {
                end += 1;
            }
            let members: Vec<_> = ordered[cursor..end]
                .iter()
                .map(|event| event.index)
                .collect();
            units.push(Unit {
                row: Row::Conversation {
                    anchor: anchor.clone(),
                    members: members.clone(),
                },
                top: first.at.max(lo),
                bottom: ordered[end - 1].at.max(lo),
                members,
            });
        } else {
            while end < ordered.len()
                && ordered[end].conversation.is_none()
                && ordered[end].peer == first.peer
                && ordered[end - 1].at - ordered[end].at <= 900.
                && ordered[end - 1].at - ordered[end].at < window.preset.quiet_threshold()
            {
                end += 1;
            }
            // A consecutive exchange is useful even when it contains only
            // two messages. Direction does not split the same counterparty.
            if end - cursor >= 2 {
                let members: Vec<_> = ordered[cursor..end]
                    .iter()
                    .map(|event| event.index)
                    .collect();
                let key = ordered[end - 1].id.clone();
                units.push(Unit {
                    row: Row::Burst {
                        key: key.clone(),
                        members: members.clone(),
                        expanded: expanded.contains(&key),
                    },
                    top: first.at.max(lo),
                    bottom: ordered[end - 1].at.max(lo),
                    members,
                });
            } else {
                for event in &ordered[cursor..end] {
                    units.push(Unit {
                        row: Row::Message(event.index),
                        top: event.at.max(lo),
                        bottom: event.at.max(lo),
                        members: vec![event.index],
                    });
                }
            }
        }
        cursor = end;
    }
    if !quiet_intervals.is_empty() && !ordered.is_empty() {
        let mut intervals: Vec<_> = quiet_intervals
            .iter()
            .copied()
            .filter(|(a, b)| a.is_finite() && b.is_finite() && a < b)
            .collect();
        intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut covered: Vec<(f64, f64)> = vec![];
        for (a, b) in intervals {
            if let Some(last) = covered.last_mut()
                && a <= last.1
            {
                last.1 = last.1.max(b);
            } else {
                covered.push((a, b));
            }
        }
        let mut points = vec![window.end];
        points.extend(ordered.iter().map(|event| event.at.clamp(lo, window.end)));
        points.push(lo);
        for pair in points.windows(2) {
            for &(a, b) in &covered {
                let start = pair[1].max(a);
                let end = pair[0].min(b);
                if end - start >= window.preset.quiet_threshold() {
                    units.push(Unit {
                        row: Row::Quiet { start, end },
                        top: end,
                        bottom: start,
                        members: vec![],
                    });
                }
            }
        }
    }
    units.sort_by(|a, b| {
        b.top
            .total_cmp(&a.top)
            .then(a.members.is_empty().cmp(&b.members.is_empty()))
    });
    let mut flat = Vec::new();
    let mut hour = None;
    for unit in units {
        let local = Local::at(unit.top);
        let next_hour = local.map(|local| local.hour_start() as i64);
        if next_hour != hour {
            hour = next_hour;
            let start = (next_hour.unwrap_or(0) as f64).max(lo);
            let end = ((next_hour.unwrap_or(0) as f64) + 3599.).min(window.end);
            let label = Local::at(start)
                .zip(Local::at(end))
                .map(|(a, b)| {
                    let date = if Local::now().is_some_and(|now| now.same_day(&a)) {
                        String::new()
                    } else {
                        format!("{} · ", a.date())
                    };
                    format!("{date}{} – {}", a.time(), b.time())
                })
                .unwrap_or_else(|| "Time not recorded".into());
            flat.push(Line::new(
                Row::Group {
                    label,
                    count: 0,
                    same_second: None,
                },
                Time::Label,
                t.size.interaction_group_label,
            ));
        }
        if let Some(label) = flat
            .iter_mut()
            .rev()
            .find(|line| matches!(line.value, Row::Group { .. }))
            && let Row::Group {
                count, same_second, ..
            } = &mut label.value
        {
            *count += unit.members.len();
            if same_second.is_none() {
                *same_second = unit.members.iter().find_map(|i| {
                    let at = by_index[i].at.floor() as i64;
                    seconds
                        .get(&at)
                        .filter(|n| **n > 1)
                        .map(|n| (at as f64, *n))
                });
            }
        }
        match &unit.row {
            Row::Burst {
                expanded: true,
                members,
                ..
            } => {
                flat.push(Line::new(
                    unit.row.clone(),
                    Time::Label,
                    t.size.interaction_burst_row,
                ));
                for index in members {
                    let at = by_index[index].at.max(lo);
                    flat.push(Line::new(
                        Row::Member(*index),
                        Time::Point(at),
                        t.size.interaction_member_row,
                    ));
                }
            }
            Row::Message(_) => {
                flat.push(Line::new(unit.row, Time::Point(unit.top), t.size.group_row))
            }
            _ => {
                let height = match unit.row {
                    Row::Burst { .. } => t.size.interaction_burst_row,
                    Row::Quiet { .. } => {
                        gap(unit.top - unit.bottom).max(t.size.interaction_quiet_row)
                    }
                    _ => t.size.group_row,
                };
                flat.push(Line::new(
                    unit.row,
                    Time::Span {
                        newest: unit.top,
                        oldest: unit.bottom,
                    },
                    height,
                ));
            }
        }
    }
    flat.push(Line::new(
        Row::Edge,
        Time::Point(lo),
        t.size.interaction_edge_row,
    ));
    let mut lines = Vec::new();
    let mut pending = Vec::new();
    let mut previous = window.end;
    for line in flat {
        let (top, bottom) = match line.time {
            Time::Label => {
                pending.push(line);
                continue;
            }
            Time::Point(at) => (at, at),
            Time::Span { newest, oldest } => (newest, oldest),
        };
        if previous - top >= window.preset.gap_threshold() {
            let group = pending
                .iter()
                .position(|line| matches!(line.value, Row::Group { .. }));
            let inside = group.is_none() && !matches!(line.value, Row::Edge);
            pending.insert(
                group.unwrap_or(0),
                Line::new(
                    Row::Gap {
                        start: top,
                        end: previous,
                        inside,
                    },
                    Time::Span {
                        newest: previous,
                        oldest: top,
                    },
                    gap(previous - top),
                ),
            );
        }
        lines.append(&mut pending);
        lines.push(line);
        previous = bottom;
    }
    Rail::new(lines, window.end)
}

/// Quiet assertions exclude the whole recorded message lifecycle, including a
/// pending message which began before the window. Coverage alone is not silence.
pub(super) fn unoccupied(covered: &[(f64, f64)], occupied: &[(f64, f64)]) -> Vec<(f64, f64)> {
    fn merged(values: &[(f64, f64)]) -> Vec<(f64, f64)> {
        let mut values: Vec<_> = values
            .iter()
            .copied()
            .filter(|(a, b)| a.is_finite() && b.is_finite() && a < b)
            .collect();
        values.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut result: Vec<(f64, f64)> = vec![];
        for (a, b) in values {
            if let Some(last) = result.last_mut()
                && a <= last.1
            {
                last.1 = last.1.max(b);
            } else {
                result.push((a, b));
            }
        }
        result
    }
    let occupied = merged(occupied);
    let mut result = vec![];
    let mut cursor = 0;
    for (start, end) in merged(covered) {
        let mut at = start;
        while cursor < occupied.len() && occupied[cursor].1 <= at {
            cursor += 1;
        }
        let mut next = cursor;
        while next < occupied.len() && occupied[next].0 < end {
            let (a, b) = occupied[next];
            if a > at {
                result.push((at, a.min(end)));
            }
            at = at.max(b);
            if at >= end {
                break;
            }
            next += 1;
        }
        if at < end {
            result.push((at, end));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::super::window::Preset;
    use super::*;

    fn event(index: usize, at: f64, peer: &str, conversation: Option<&str>) -> Event {
        Event {
            index,
            at,
            id: format!("message-{index}"),
            peer: peer.into(),
            conversation: conversation.map(str::to_owned),
        }
    }
    fn window() -> TimeWindow {
        TimeWindow::new(Preset::SevenHours, 100_000.)
    }

    #[test]
    fn quiet_coverage_excludes_lifecycle_overlap_and_a_pending_message() {
        assert_eq!(
            unoccupied(
                &[(0., 100.), (100., 200.)],
                &[(-20., 40.), (80., 90.), (85., 120.)]
            ),
            vec![(40., 80.), (120., 200.)]
        );
        assert!(unoccupied(&[(0., 200.)], &[(-20., 200.)]).is_empty());
        assert_eq!(
            unoccupied(&[(0., 50.), (70., 100.)], &[(40., 80.)]),
            vec![(0., 40.), (80., 100.)]
        );
    }

    #[test]
    fn interleaved_conversation_runs_preserve_the_chronological_rail() {
        let events = [
            event(0, 99_900., "owner", Some("anchor")),
            event(1, 99_890., "other", None),
            event(2, 99_880., "owner", Some("anchor")),
        ];
        let rail = build(&events, window(), None, &[], &HashSet::new());
        let messages: Vec<_> = rail
            .lines
            .iter()
            .filter(|l| matches!(l.value, Row::Conversation { .. } | Row::Message(_)))
            .collect();
        assert_eq!(messages.len(), 3);
        assert!(
            matches!(&messages[0].value,Row::Conversation{anchor,members} if anchor=="anchor" && members==&vec![0])
        );
        assert!(matches!(messages[1].value, Row::Message(1)));
        assert!(
            matches!(&messages[2].value,Row::Conversation{anchor,members} if anchor=="anchor" && members==&vec![2])
        );
        assert!(rail.lines.iter().all(|l| l.newest >= l.oldest));
        assert!(
            rail.lines
                .windows(2)
                .all(|pair| pair[0].oldest == pair[1].newest)
        );
    }

    #[test]
    fn bursts_expand_to_actual_rows_and_keep_a_stable_key_on_new_arrivals() {
        let mut events: Vec<_> = (0..6)
            .map(|i| event(i, 99_900. - i as f64 * 60., "peer", None))
            .collect();
        let rail = build(&events, window(), None, &[], &HashSet::new());
        assert!(rail.lines.iter().any(|l| matches!(&l.value,Row::Burst{key,members,expanded:false} if key=="message-5" && members.len()==6)));
        events.push(event(6, 99_960., "peer", None));
        let rail = build(
            &events,
            window(),
            None,
            &[],
            &HashSet::from(["message-5".into()]),
        );
        assert_eq!(
            rail.lines
                .iter()
                .filter(|l| matches!(l.value, Row::Member(_)))
                .count(),
            7
        );
        for line in rail
            .lines
            .iter()
            .filter(|l| matches!(l.value, Row::Member(_)))
        {
            assert!(matches!(line.time, Time::Point(_)));
        }
    }

    #[test]
    fn consecutive_pairs_group_without_absorbing_other_peers_or_discussions() {
        let events = [
            event(0, 99_900., "peer", None),
            event(1, 99_840., "peer", None),
            event(2, 99_780., "other", None),
            event(3, 99_720., "peer", None),
            event(4, 99_660., "peer", Some("discussion")),
            event(5, 99_600., "peer", None),
            event(6, 99_540., "peer", None),
        ];
        let rail = build(&events, window(), None, &[], &HashSet::new());
        let groups: Vec<_> = rail
            .lines
            .iter()
            .filter_map(|line| match &line.value {
                Row::Burst { members, .. } => Some(members.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(groups, vec![vec![0, 1], vec![5, 6]]);
        assert!(
            rail.lines
                .iter()
                .any(|line| matches!(line.value, Row::Message(3)))
        );
        assert!(
            rail.lines
                .windows(2)
                .all(|pair| pair[0].oldest == pair[1].newest)
        );
    }

    #[test]
    fn a_quiet_interval_keeps_consecutive_messages_separate_on_the_rail() {
        let events = [
            event(0, 99_900., "peer", None),
            event(1, 99_000., "peer", None),
        ];
        let rail = build(
            &events,
            TimeWindow::new(Preset::Hour, 100_000.),
            None,
            &[(96_400., 100_000.)],
            &HashSet::new(),
        );
        assert!(
            !rail
                .lines
                .iter()
                .any(|line| matches!(line.value, Row::Burst { .. }))
        );
        assert!(
            rail.lines
                .iter()
                .any(|line| matches!(line.value, Row::Quiet { .. }))
        );
        assert!(rail.lines.iter().all(|line| line.newest >= line.oldest));
    }

    #[test]
    fn quiet_rows_require_unfiltered_complete_history_and_stop_at_retention() {
        let events = [
            event(0, 99_000., "peer", None),
            event(1, 75_000., "other", None),
        ];
        let complete = build(
            &events,
            window(),
            Some(80_000.),
            &[(80_000., 100_000.)],
            &HashSet::new(),
        );
        assert!(
            complete
                .lines
                .iter()
                .any(|l| matches!(l.value, Row::Quiet { .. }))
        );
        assert!(complete.lines.iter().all(|l| l.oldest >= 80_000.));
        let filtered = build(&events, window(), Some(80_000.), &[], &HashSet::new());
        assert!(
            !filtered
                .lines
                .iter()
                .any(|l| matches!(l.value, Row::Quiet { .. }))
        );
        assert!(
            filtered
                .lines
                .iter()
                .any(|l| matches!(l.value, Row::Gap { .. }))
        );
    }
}
