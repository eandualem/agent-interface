//! Gate 4: frame pacing
//! (`docs/BENCHMARKING.md`).
//!
//! The measured terminal runs the fire animation or the scroll test, the app
//! inside a tmux pane and plain Ghostty directly. Frames are counted only
//! when their pixels changed: ScreenCaptureKit also delivers complete frames
//! that repeat the previous one.

use std::fmt::Write;
use std::path::Path;
use std::time::Duration;

use super::clock::{self, HostTime};
use super::frames::{Frames, Region, Seen};
use super::pixels::region_digest;
use super::report::{self, Window, ms};
use super::targets::{self, Programs, Target};
use crate::options::Options;

/// What the measured terminal draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scenario {
    /// The fire animation, redrawing the whole pane every frame.
    Fire,
    /// Log lines scrolling continuously.
    Scroll,
}

impl Scenario {
    fn name(self) -> &'static str {
        match self {
            Scenario::Fire => "fire",
            Scenario::Scroll => "scroll",
        }
    }

    fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "fire" => Ok(Scenario::Fire),
            "scroll" => Ok(Scenario::Scroll),
            _ => Err(format!("unknown scenario {name}")),
        }
    }

    /// The harness subcommand that draws it.
    fn producer(self) -> &'static str {
        match self {
            Scenario::Fire => "fire",
            Scenario::Scroll => "scroll-test",
        }
    }
}

/// The gate's limits: the app's frame rate against plain Ghostty's, and the
/// share of the app's frames that must follow the previous one within 1.5
/// refresh intervals.
const RATE_LIMIT: f64 = 0.95;
const ON_TIME_LIMIT: f64 = 0.99;
/// The capture's first second is left out.
const SETTLE: Duration = Duration::from_secs(1);
/// The capture runs this much longer than the measured span, so that frames
/// displayed at its very end have arrived when it stops.
const TAIL: Duration = Duration::from_millis(250);
/// The coverage a whole-gate result needs: both scenarios, each measured for
/// at least this long.
const REQUIRED_SCENARIOS: [Scenario; 2] = [Scenario::Fire, Scenario::Scroll];
const REQUIRED_SECONDS: f64 = 30.0;

struct Settings {
    /// How this program's build differs from the distributable one; any
    /// difference keeps the gate undecided (0018).
    build_issues: Vec<String>,
    seconds: f64,
    runs: usize,
    scenarios: Vec<Scenario>,
    targets: Vec<Target>,
    /// The refresh rate to use when the display does not report one.
    refresh: Option<f64>,
}

/// One run of one target in one scenario.
struct Measured {
    scenario: Scenario,
    target: Target,
    run: usize,
    /// The display times of the changed frames in the measured span, from
    /// its start.
    frames: Vec<Duration>,
    /// The refresh rate the intervals are judged against.
    refresh: f64,
    window: Window,
}

/// Runs gate 4 and writes `summary.md` and `raw.tsv` to `--out`.
pub fn run(options: &Options) -> Result<(), String> {
    let out = Path::new(options.required("out")?);
    let settings = Settings {
        build_issues: report::build_issues(&targets::Build::own()),
        seconds: options.get("seconds", 30.0)?,
        runs: options.get("runs", 1)?,
        scenarios: options
            .get("scenarios", "fire,scroll".to_owned())?
            .split(',')
            .map(Scenario::from_name)
            .collect::<Result<_, _>>()?,
        targets: Target::list(options, &[Target::App, Target::Ghostty])?,
        refresh: report::refresh_option(options)?,
    };
    if settings.seconds <= 0.0 || settings.runs == 0 || settings.targets.is_empty() {
        return Err("--seconds, --runs and --targets must not be empty".to_owned());
    }
    let programs = Programs::from_options(options)?;
    let environment = report::environment(&programs);
    let work = tempfile::Builder::new()
        .prefix("tw-gate4-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    for &scenario in &settings.scenarios {
        for run in 0..settings.runs {
            for target in report::order(&settings.targets, run) {
                results.push(measure(
                    target,
                    scenario,
                    run + 1,
                    &settings,
                    &programs,
                    work.path(),
                )?);
            }
        }
    }
    let summary = summary(&settings, &environment, &results);
    report::write(out, &summary, &raw(&results))
}

fn measure(
    target: Target,
    scenario: Scenario,
    run: usize,
    settings: &Settings,
    programs: &Programs,
    work: &Path,
) -> Result<Measured, String> {
    let bench = std::env::current_exe().map_err(|error| error.to_string())?;
    // The producer outlasts the capture, whatever the program's start-up
    // takes.
    let command = format!(
        "exec {} {} --seconds {}",
        targets::shell_word(&bench),
        scenario.producer(),
        settings.seconds + 60.0
    );
    let server = if target.uses_tmux() {
        Some(targets::session(&command, work).map_err(|error| error.to_string())?)
    } else {
        None
    };
    let running = targets::start(target, programs, server.as_ref(), &command, work)?;
    let frames = Frames::start(running.window().id, |image| {
        region_digest(image, Region::Whole.rect(image))
    })?;
    let started = clock::now();
    let span = Duration::from_secs_f64(settings.seconds);
    // A window that stopped being visible stopped drawing: the run is void.
    let after = running.wait_visible(SETTLE + span + TAIL)?;
    // Stopping first lets the frames still queued arrive; the measured span
    // is cut out of all of them below.
    let (stats, seen) = frames.stop()?;
    let window = Window::new(target, &running, stats, after);
    running.stop();
    drop(server);
    let refresh = window
        .refresh
        .or(settings.refresh)
        .ok_or("the display reports no refresh rate: give --refresh HZ")?;
    let from = started.after(SETTLE);
    let to = from.after(span);
    Ok(Measured {
        scenario,
        target,
        run,
        frames: changed_frames(&seen, from, to),
        refresh,
        window,
    })
}

/// Returns the display times, from `from`, of the frames between `from` and
/// `to` whose pixels differ from the frame before them.
fn changed_frames(seen: &[Seen<u64>], from: HostTime, to: HostTime) -> Vec<Duration> {
    seen.windows(2)
        .filter(|pair| pair[0].value != pair[1].value)
        .map(|pair| pair[1].time)
        .filter(|&time| time >= from && time <= to)
        .map(|time| time.since(from))
        .collect()
}

/// A target's frames in one scenario, over every run.
struct Pacing {
    /// Changed frames per second.
    rate: f64,
    /// The share of intervals between changed frames within 1.5 refresh
    /// intervals.
    on_time: f64,
    /// Every interval between changed frames.
    intervals: Vec<Duration>,
}

fn pacing(
    results: &[Measured],
    scenario: Scenario,
    target: Target,
    seconds: f64,
) -> Option<Pacing> {
    let runs: Vec<&Measured> = results
        .iter()
        .filter(|result| result.scenario == scenario && result.target == target)
        .collect();
    if runs.is_empty() {
        return None;
    }
    let frames: usize = runs.iter().map(|result| result.frames.len()).sum();
    let mut intervals = Vec::new();
    let mut on_time = 0;
    for result in &runs {
        let limit = Duration::from_secs_f64(1.5 / result.refresh);
        for pair in result.frames.windows(2) {
            let interval = pair[1] - pair[0];
            on_time += usize::from(interval <= limit);
            intervals.push(interval);
        }
    }
    Some(Pacing {
        rate: frames as f64 / (seconds * runs.len() as f64),
        on_time: if intervals.is_empty() {
            0.0
        } else {
            on_time as f64 / intervals.len() as f64
        },
        intervals,
    })
}

fn raw(results: &[Measured]) -> String {
    let mut out = String::from("scenario\ttarget\trun\tframe\tdisplay_ns_from_start\n");
    for result in results {
        for (frame, time) in result.frames.iter().enumerate() {
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}",
                result.scenario.name(),
                result.target.name(),
                result.run,
                frame + 1,
                time.as_nanos()
            );
        }
    }
    out
}

fn summary(settings: &Settings, environment: &[(&str, String)], results: &[Measured]) -> String {
    let verdict = verdict(settings, results);
    let mut out = report::opening("Gate 4: frame pacing", &verdict, environment);
    let _ = write!(
        out,
        "\n## Settings\n\n\
         - {} s measured per run after {} s left out, {} run(s) per target and scenario.\n\
         - Only frames whose pixels differ from the frame before count.\n\
         - An interval is on time within 1.5 refresh intervals of the display the window is on.\n",
        settings.seconds,
        SETTLE.as_secs(),
        settings.runs,
    );
    for &scenario in &settings.scenarios {
        let _ = write!(
            out,
            "\n## {}\n\n| Target | Frames per second | On time | Median interval ms | p95 interval ms | Longest ms |\n\
             | --- | --- | --- | --- | --- | --- |\n",
            scenario.name()
        );
        for &target in &settings.targets {
            let Some(pacing) = pacing(results, scenario, target, settings.seconds) else {
                continue;
            };
            let intervals = if pacing.intervals.is_empty() {
                "- | - | -".to_owned()
            } else {
                let s = crate::stats::summarize(&pacing.intervals);
                format!("{:.2} | {:.2} | {:.2}", ms(s.median), ms(s.p95), ms(s.max))
            };
            let _ = writeln!(
                out,
                "| {} | {:.1} | {:.2}% | {intervals} |",
                target.name(),
                pacing.rate,
                pacing.on_time * 100.0
            );
        }
    }
    let windows: Vec<(String, Window)> = results
        .iter()
        .map(|result| {
            (
                format!("{} {}", result.scenario.name(), result.run),
                result.window.clone(),
            )
        })
        .collect();
    out.push_str(&report::windows_table(&windows));
    out
}

fn verdict(settings: &Settings, results: &[Measured]) -> (String, Vec<String>) {
    let mut reasons = Vec::new();
    let (mut failed, mut undecided) = (false, false);
    for &scenario in &settings.scenarios {
        let of = |target| pacing(results, scenario, target, settings.seconds);
        let (Some(app), Some(ghostty)) = (of(Target::App), of(Target::Ghostty)) else {
            undecided = true;
            reasons.push(format!(
                "{}: not decided, the app and plain Ghostty were not both measured",
                scenario.name()
            ));
            continue;
        };
        let ratio = if ghostty.rate > 0.0 {
            app.rate / ghostty.rate
        } else {
            0.0
        };
        let passes = ratio >= RATE_LIMIT && app.on_time >= ON_TIME_LIMIT;
        failed |= !passes;
        reasons.push(format!(
            "{}: the app draws {:.1} frames per second, {:.2} times plain Ghostty's {:.1} \
             (needs {RATE_LIMIT}), with {:.2}% on time (needs {}%): {}",
            scenario.name(),
            app.rate,
            ratio,
            ghostty.rate,
            app.on_time * 100.0,
            ON_TIME_LIMIT * 100.0,
            if passes { "passes" } else { "fails" },
        ));
    }
    reasons.push(
        "An acceptance result needs three independent runs of a clean build on a quiet \
         machine; this report alone does not make one."
            .to_owned(),
    );
    (
        report::verdict(
            4,
            failed,
            undecided,
            &missing(settings),
            &settings.build_issues,
        ),
        reasons,
    )
}

/// Lists what the run left out of the coverage a whole-gate result needs.
fn missing(settings: &Settings) -> Vec<String> {
    let mut missing: Vec<String> = REQUIRED_SCENARIOS
        .iter()
        .filter(|scenario| !settings.scenarios.contains(scenario))
        .map(|scenario| format!("{} not run", scenario.name()))
        .collect();
    if settings.seconds < REQUIRED_SECONDS {
        missing.push(format!(
            "{} s measured per run, needs {REQUIRED_SECONDS} s",
            settings.seconds
        ));
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(nanos: u64, value: u64) -> Seen<u64> {
        Seen {
            time: HostTime::from_nanos(nanos),
            value,
        }
    }

    #[test]
    fn only_changed_frames_in_the_span_count() {
        let frames = [
            seen(5, 1),
            seen(10, 2),
            seen(20, 2),
            seen(30, 3),
            seen(40, 3),
            seen(50, 4),
            seen(60, 5),
        ];
        let from = HostTime::from_nanos(15);
        let to = HostTime::from_nanos(55);
        let changed = changed_frames(&frames, from, to);
        assert_eq!(
            changed,
            vec![Duration::from_nanos(15), Duration::from_nanos(35)]
        );
    }

    #[test]
    fn only_both_scenarios_over_thirty_seconds_can_pass() {
        let settings = |scenarios: &[Scenario], seconds| Settings {
            build_issues: Vec::new(),
            seconds,
            runs: 1,
            scenarios: scenarios.to_vec(),
            targets: vec![Target::App, Target::Ghostty],
            refresh: None,
        };
        assert!(missing(&settings(&REQUIRED_SCENARIOS, 30.0)).is_empty());
        assert_eq!(
            missing(&settings(&[Scenario::Fire], 5.0)),
            vec![
                "scroll not run".to_owned(),
                "5 s measured per run, needs 30 s".to_owned()
            ]
        );
    }

    #[test]
    fn pacing_counts_intervals_within_one_and_a_half_refreshes() {
        let measured = |frames: &[u64]| Measured {
            scenario: Scenario::Fire,
            target: Target::App,
            run: 1,
            frames: frames.iter().map(|&ms| Duration::from_millis(ms)).collect(),
            refresh: 60.0,
            window: Window {
                target: Target::App,
                origin: (0.0, 0.0),
                size: (0.0, 0.0),
                refresh: Some(60.0),
                display: Default::default(),
                build: None,
                stats: Default::default(),
            },
        };
        // Intervals 17, 16, 30 and 17 ms; 25 ms is the limit at 60 Hz.
        let results = [measured(&[0, 17, 33, 63, 80])];
        let pacing = pacing(&results, Scenario::Fire, Target::App, 1.0).unwrap();
        assert_eq!(pacing.rate, 5.0);
        assert_eq!(pacing.on_time, 0.75);
    }
}
