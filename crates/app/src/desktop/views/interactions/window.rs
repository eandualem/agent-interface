//! Requested time window. Relative windows advance only on an explicit refresh
//! acceptance; a poll must not move the ledger under the person reading it.

use super::super::clock::Local;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Preset {
    Hour,
    #[default]
    SevenHours,
    Day,
    Custom,
}

impl Preset {
    pub const ALL: [Self; 4] = [Self::Hour, Self::SevenHours, Self::Day, Self::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hour => "Last hour",
            Self::SevenHours => "7 hours",
            Self::Day => "24 hours",
            Self::Custom => "Custom…",
        }
    }

    pub fn seconds(self) -> Option<f64> {
        match self {
            Self::Hour => Some(3600.),
            Self::SevenHours => Some(25200.),
            Self::Day => Some(86400.),
            Self::Custom => None,
        }
    }

    pub fn quiet_threshold(self) -> f64 {
        60. * match self {
            Self::Hour => 15.,
            Self::Day => 90.,
            _ => 45.,
        }
    }

    pub fn gap_threshold(self) -> f64 {
        60. * match self {
            Self::Hour => 2.,
            Self::Day => 10.,
            _ => 5.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TimeWindow {
    pub preset: Preset,
    pub start: f64,
    pub end: f64,
}

impl TimeWindow {
    pub fn new(preset: Preset, end: f64) -> Self {
        Self {
            preset,
            start: end - preset.seconds().unwrap_or(25200.),
            end,
        }
    }

    pub fn at(self, end: f64) -> Self {
        if self.preset == Preset::Custom {
            self
        } else {
            Self::new(self.preset, end)
        }
    }

    pub fn custom(start: &str, end: &str, history: Option<f64>) -> Result<Self, &'static str> {
        let start = Local::parse_input(start)
            .ok_or("Enter a valid, unambiguous local start: YYYY-MM-DD HH:MM.")?;
        let end = Local::parse_input(end)
            .ok_or("Enter a valid, unambiguous local end: YYYY-MM-DD HH:MM.")?;
        let start = history.map_or(start, |history| start.max(history));
        if start >= end {
            return Err("The end must follow the start of retained history and the chosen start.");
        }
        Ok(Self {
            preset: Preset::Custom,
            start,
            end,
        })
    }

    pub fn label(self) -> String {
        match Local::at(self.start).zip(Local::at(self.end)) {
            Some((start, end)) => format!(
                "{}, {} – {}{} · {}{}",
                start.date(),
                start.time(),
                if start.same_day(&end) {
                    String::new()
                } else {
                    format!("{}, ", end.date())
                },
                end.time(),
                start.offset(),
                if start.offset() == end.offset() {
                    String::new()
                } else {
                    format!(" → {}", end.offset())
                }
            ),
            None => "Time window unavailable".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_windows_advance_explicitly_and_custom_keeps_its_bounds() {
        let first = TimeWindow::new(Preset::SevenHours, 100000.);
        assert_eq!(first.start, 74800.);
        assert_eq!(first.at(100100.).start, 74900.);
        let custom = TimeWindow {
            preset: Preset::Custom,
            ..first
        };
        assert_eq!(custom.at(100100.), custom);
        assert!(TimeWindow::custom("invalid", "2026-10-31 12:00", None).is_err());
        assert!(TimeWindow::custom("2026-10-31 12:00", "2026-10-31 11:00", None).is_err());
    }
}
