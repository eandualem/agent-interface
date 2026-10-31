//! Local dates and times for grouping records by day and stamping messages.

use chrono::{DateTime, Datelike, Local as Zone, NaiveDateTime, TimeZone, Timelike};

/// A moment in the local calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Local(DateTime<Zone>);

impl Local {
    pub fn at(epoch: f64) -> Option<Self> {
        let seconds = epoch.floor();
        let nanos = ((epoch - seconds) * 1e9) as u32;
        Zone.timestamp_opt(seconds as i64, nanos).single().map(Self)
    }

    pub fn now() -> Option<Self> {
        Some(Self(Zone::now()))
    }

    pub fn same_day(&self, other: &Self) -> bool {
        self.0.date_naive() == other.0.date_naive()
    }

    /// "Mon 27 Oct".
    pub fn date(&self) -> String {
        self.0.format("%a %-d %b").to_string()
    }

    /// "14:30".
    pub fn time(&self) -> String {
        format!("{:02}:{:02}", self.0.hour(), self.0.minute())
    }

    /// A precise local stamp for simultaneous messages.
    pub fn seconds(&self) -> String {
        self.0.format("%H:%M:%S").to_string()
    }

    /// The offset at this moment, including daylight saving time.
    pub fn offset(&self) -> String {
        self.0.format("UTC%:z").to_string()
    }

    /// The beginning of this local hour, preserving its actual UTC offset.
    pub fn hour_start(&self) -> f64 {
        (self.0.timestamp() - i64::from(self.0.minute() * 60 + self.0.second())) as f64
    }

    pub fn input(&self) -> String {
        self.0.format("%Y-%m-%d %H:%M").to_string()
    }

    /// Reject ambiguous and nonexistent local times instead of guessing an offset.
    pub fn parse_input(text: &str) -> Option<f64> {
        let date = NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M").ok()?;
        Zone.from_local_datetime(&date)
            .single()
            .map(|date| date.timestamp() as f64)
    }

    /// "Today", "Yesterday" or the date, seen from `now`.
    pub fn day_name(&self, now: &Self) -> String {
        let days = now.0.date_naive().num_days_from_ce() - self.0.date_naive().num_days_from_ce();
        match days {
            0 => "Today".into(),
            1 => "Yesterday".into(),
            _ => self.date(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_are_named_from_now() {
        let now = Local::at(1_793_000_000.0).expect("local time");
        let day = 86_400.0;
        assert_eq!(Local::at(1_793_000_000.0 - 60.0).unwrap().time().len(), 5);
        assert_eq!(now.day_name(&now), "Today");
        let yesterday = Local::at(1_793_000_000.0 - day).unwrap();
        assert_eq!(yesterday.day_name(&now), "Yesterday");
        let earlier = Local::at(1_793_000_000.0 - 3.0 * day).unwrap();
        assert_eq!(earlier.day_name(&now), earlier.date());
        assert!(!earlier.same_day(&now));
    }
}
