//! One geometry for the ledger and its time rail. Labels hold time still;
//! message times land at row centres, and long gaps take compressed space.

/// The normal split also needs enough space for the actual sibling panels.
pub(super) fn split_fits(
    width: f32,
    threshold: f32,
    reader: f32,
    discussion: f32,
    ledger: f32,
) -> bool {
    width >= threshold && width - reader - discussion >= ledger
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Time {
    Label,
    Point(f64),
    Span { newest: f64, oldest: f64 },
}

impl Time {
    fn ends(self) -> Option<(f64, f64)> {
        match self {
            Self::Label => None,
            Self::Point(at) => Some((at, at)),
            Self::Span { newest, oldest } => Some((newest, oldest)),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Line<T> {
    pub value: T,
    pub time: Time,
    pub height: f32,
    pub y: f32,
    pub newest: f64,
    pub oldest: f64,
}

impl<T> Line<T> {
    pub fn new(value: T, time: Time, height: f32) -> Self {
        Self {
            value,
            time,
            height,
            y: 0.,
            newest: 0.,
            oldest: 0.,
        }
    }

    /// Several messages share a collapsed row. Keep each mark inside it.
    pub fn mark(&self, at: f64) -> f32 {
        if self.newest == self.oldest {
            return self.height / 2.;
        }
        3. + ((self.newest - at) / (self.newest - self.oldest)).clamp(0., 1.) as f32
            * (self.height - 6.).max(0.)
    }
}

pub(super) struct Rail<T> {
    pub lines: Vec<Line<T>>,
    anchors: Vec<(f32, f64)>,
    pub height: f32,
}

impl<T> Rail<T> {
    pub fn new(mut lines: Vec<Line<T>>, end: f64) -> Self {
        let timed: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, line)| line.time.ends().map(|_| i))
            .collect();
        for (i, &index) in timed.iter().enumerate() {
            let (top, bottom) = lines[index].time.ends().expect("timed line");
            lines[index].oldest = bottom;
            if i == 0 {
                lines[index].newest = end;
            } else {
                let prev = timed[i - 1];
                let upper = lines[prev].time.ends().expect("timed line").1;
                let edge = if upper == top {
                    top
                } else if matches!(lines[prev].time, Time::Point(_))
                    && matches!(lines[index].time, Time::Point(_))
                {
                    (upper + top) / 2.
                } else if matches!(lines[prev].time, Time::Span { .. }) {
                    upper
                } else {
                    top
                };
                lines[prev].oldest = edge;
                lines[index].newest = edge;
            }
        }
        let mut anchors = Vec::new();
        let mut y = 0.;
        let mut at = end;
        for line in &mut lines {
            line.y = y;
            if line.time == Time::Label {
                line.newest = at;
                line.oldest = at;
            }
            anchors.push((y, line.newest));
            if let Time::Point(moment) = line.time {
                anchors.push((y + line.height / 2., moment));
            }
            y += line.height;
            anchors.push((y, line.oldest));
            at = line.oldest;
        }
        Self {
            lines,
            anchors,
            height: y,
        }
    }

    /// Upper edge of a time, before any labels or same-second rows.
    pub fn upper(&self, at: f64) -> f32 {
        for (i, &(y, time)) in self.anchors.iter().enumerate() {
            if time <= at {
                return if i == 0 {
                    y
                } else {
                    interpolate(self.anchors[i - 1], (y, time), at)
                };
            }
        }
        self.height
    }

    /// Lower edge includes stationary labels so adjacent bands stay continuous.
    pub fn lower(&self, at: f64) -> f32 {
        for (i, &(y, time)) in self.anchors.iter().enumerate().rev() {
            if time >= at {
                return if i + 1 == self.anchors.len() {
                    y
                } else {
                    interpolate((y, time), self.anchors[i + 1], at)
                };
            }
        }
        0.
    }
}

fn interpolate(a: (f32, f64), b: (f32, f64), at: f64) -> f32 {
    if a.1 == b.1 {
        return a.0;
    }
    a.0 + ((a.1 - at) / (a.1 - b.1)).clamp(0., 1.) as f32 * (b.0 - a.0)
}

pub(super) fn gap_height(seconds: f64, unit: f32, max: f32) -> f32 {
    (unit * (seconds.max(0.) / 60.).sqrt() as f32)
        .round()
        .min(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_centres_and_labels_share_one_continuous_clock() {
        let rail = Rail::new(
            vec![
                Line::new((), Time::Label, 34.),
                Line::new((), Time::Point(600.), 36.),
                Line::new((), Time::Point(480.), 36.),
                Line::new((), Time::Label, 34.),
                Line::new(
                    (),
                    Time::Span {
                        newest: 400.,
                        oldest: 100.,
                    },
                    80.,
                ),
                Line::new((), Time::Point(0.), 40.),
            ],
            700.,
        );
        assert_eq!(rail.upper(600.), 52.);
        assert_eq!(rail.upper(480.), 88.);
        assert_eq!(rail.lines[1].oldest, rail.lines[2].newest);
        assert_eq!(rail.lines[3].newest, rail.lines[3].oldest);
        assert_eq!(rail.upper(400.), 106.);
        assert_eq!(rail.lower(400.), 140.);
        assert_eq!(rail.height, 260.);
        assert_eq!(gap_height(28. * 60., 6., 160.), 32.);
        assert_eq!(gap_height(577. * 60., 6., 160.), 144.);
        assert_eq!(gap_height(86400., 6., 160.), 160.);
    }

    #[test]
    fn same_second_and_collapsed_rows_do_not_divide_by_zero() {
        let rail = Rail::new(
            vec![
                Line::new((), Time::Point(100.), 36.),
                Line::new((), Time::Point(100.), 36.),
                Line::new(
                    (),
                    Time::Span {
                        newest: 90.,
                        oldest: 20.,
                    },
                    40.,
                ),
            ],
            110.,
        );
        assert!(rail.lower(100.) > rail.upper(100.));
        let row = &rail.lines[2];
        assert_eq!(row.mark(90.), 3.);
        assert_eq!(row.mark(20.), 37.);
    }
}

#[cfg(test)]
mod responsive_tests {
    use super::split_fits;
    #[test]
    fn discussion_and_resize_preserve_a_usable_ledger_or_use_back_navigation() {
        assert!(!split_fits(1040., 1100., 560., 0., 700.));
        assert!(!split_fits(1140., 1100., 460., 433.2, 700.));
        assert!(split_fits(1728., 1100., 460., 360., 700.));
        assert!(split_fits(1400., 1100., 560., 44., 700.));
        assert!(!split_fits(1400., 1100., 460., 532., 700.));
    }
}
