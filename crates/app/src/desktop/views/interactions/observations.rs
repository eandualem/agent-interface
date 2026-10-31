//! Normalize explicit observed spans without filling a gap with a guessed state.
//! Conflicting overlaps are missing evidence, never a last-writer inference.
use crate::desktop::components::Observation;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Span {
    pub start: f64,
    pub end: f64,
    pub state: Observation,
}

pub(super) fn normalize(spans: &[Span], start: f64, end: f64) -> Vec<Span> {
    if !start.is_finite() || !end.is_finite() || start >= end {
        return vec![];
    }
    let mut edges = vec![
        (start, Observation::Unknown, 0),
        (end, Observation::Unknown, 0),
    ];
    for span in spans {
        if !span.start.is_finite() || !span.end.is_finite() {
            continue;
        }
        let a = span.start.max(start);
        let b = span.end.min(end);
        if a < b {
            edges.push((a, span.state, 1));
            edges.push((b, span.state, -1));
        }
    }
    edges.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut active = BTreeMap::<Observation, usize>::new();
    let mut result: Vec<Span> = vec![];
    let mut index = 0;
    while index < edges.len() {
        let at = edges[index].0;
        while index < edges.len() && edges[index].0 == at {
            let (_, state, delta) = edges[index];
            if delta > 0 {
                *active.entry(state).or_default() += 1;
            } else if delta < 0
                && let Some(count) = active.get_mut(&state)
            {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    active.remove(&state);
                }
            }
            index += 1;
        }
        let Some(next) = edges.get(index).map(|e| e.0) else {
            break;
        };
        let state = if active.len() == 1 {
            *active.keys().next().expect("one state")
        } else {
            Observation::Unknown
        };
        if let Some(previous) = result.last_mut()
            && previous.state == state
            && previous.end == at
        {
            previous.end = next;
        } else {
            result.push(Span {
                start: at,
                end: next,
                state,
            });
        }
    }
    result
}

pub(super) fn describe(spans: &[Span], start: f64, end: f64) -> (Observation, String) {
    let mut seconds = BTreeMap::<Observation, f64>::new();
    for span in normalize(spans, start, end) {
        *seconds.entry(span.state).or_default() += span.end - span.start;
    }
    let mut parts: Vec<_> = seconds.into_iter().collect();
    parts.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let dominant = parts.first().map_or(Observation::Unknown, |part| part.0);
    let label = parts
        .iter()
        .map(|(state, seconds)| format!("{} {}", state.label(), super::render::duration(*seconds)))
        .collect::<Vec<_>>()
        .join(" · ");
    (dominant, label)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gaps_and_conflicting_evidence_stay_unknown_and_nothing_precedes_the_edge() {
        use Observation::*;
        let spans = [
            Span {
                start: 0.,
                end: 20.,
                state: Idle,
            },
            Span {
                start: 15.,
                end: 30.,
                state: Working,
            },
            Span {
                start: 40.,
                end: 50.,
                state: Offline,
            },
        ];
        let result = normalize(&spans, 10., 60.);
        assert_eq!(
            result,
            vec![
                Span {
                    start: 10.,
                    end: 15.,
                    state: Idle
                },
                Span {
                    start: 15.,
                    end: 20.,
                    state: Unknown
                },
                Span {
                    start: 20.,
                    end: 30.,
                    state: Working
                },
                Span {
                    start: 30.,
                    end: 40.,
                    state: Unknown
                },
                Span {
                    start: 40.,
                    end: 50.,
                    state: Offline
                },
                Span {
                    start: 50.,
                    end: 60.,
                    state: Unknown
                }
            ]
        );
        assert_eq!(
            normalize(&[], 10., 20.),
            vec![Span {
                start: 10.,
                end: 20.,
                state: Unknown
            }]
        );
    }
}
