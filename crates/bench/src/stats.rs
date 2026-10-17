//! Summaries of repeated timings.

use std::time::Duration;

/// The spread of a set of timings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Summary {
    /// The median.
    pub median: Duration,
    /// The nearest-rank 95th percentile: the slow end.
    pub p95: Duration,
    /// The fastest timing.
    pub min: Duration,
    /// The slowest timing.
    pub max: Duration,
}

/// Summarises `times`, which must not be empty.
pub fn summarize(times: &[Duration]) -> Summary {
    assert!(!times.is_empty(), "no timings to summarise");
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    let n = sorted.len();
    let median = if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2
    };
    // Nearest rank: the smallest value with at least 95% of values at or
    // below it.
    let rank = (95 * n).div_ceil(100);
    Summary {
        median,
        p95: sorted[rank - 1],
        min: sorted[0],
        max: sorted[n - 1],
    }
}

/// Returns `bytes` per `time` in megabytes (10^6 bytes) per second.
pub fn rate(bytes: usize, time: Duration) -> f64 {
    bytes as f64 / time.as_secs_f64() / 1e6
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(values: &[u64]) -> Vec<Duration> {
        values.iter().copied().map(Duration::from_millis).collect()
    }

    #[test]
    fn summarises_odd_and_even_counts() {
        let odd = summarize(&ms(&[5, 1, 3]));
        assert_eq!(odd.median, Duration::from_millis(3));
        assert_eq!(
            (odd.min, odd.max, odd.p95),
            (ms(&[1])[0], ms(&[5])[0], ms(&[5])[0])
        );
        let even = summarize(&ms(&[4, 1, 3, 2]));
        assert_eq!(even.median, Duration::from_micros(2500));
    }

    #[test]
    fn the_95th_percentile_is_a_nearest_rank() {
        let twenty: Vec<u64> = (1..=20).collect();
        assert_eq!(summarize(&ms(&twenty)).p95, Duration::from_millis(19));
        let ten: Vec<u64> = (1..=10).collect();
        assert_eq!(summarize(&ms(&ten)).p95, Duration::from_millis(10));
    }
}
