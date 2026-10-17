//! Gate 3: typing latency
//! (`docs/decisions/0018-milestone-0-benchmark-method-revised.md`).
//!
//! The measured pane runs `cat`. Keys are posted one at a time to the
//! measured program's process, at random gaps. A key's latency runs from just
//! before it is posted to the display time of the first frame in which the
//! typing pane's region differs from the frame shown before the key.
//!
//! Every key, measured or not, is posted only while the measured window is
//! frontmost, and the window is checked to be shown between keys.

use std::fmt::Write;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::capture::CaptureStats;
use super::clock::{self, HostTime};
use super::frames::{Frames, Region, Seen};
use super::keys::{Modifiers, keycode};
use super::pixels::region_digest;
use super::report::{self, Judged, Window, ms};
use super::targets::{self, Programs, SESSION, Target};
use crate::options::Options;
use crate::payload::Rng;
use crate::stats::{self, Summary};

/// The keys typed, in turn.
const LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
/// How long a key may take to show before it counts as missed.
const KEY_TIMEOUT: Duration = Duration::from_secs(1);
/// Keys typed on one line before an unmeasured Return starts the next.
const LINE_KEYS: usize = 60;
/// The gate's limits: the app's median and 95th percentile may exceed plain
/// Ghostty's by at most these.
const MEDIAN_LIMIT: Duration = Duration::from_millis(4);
const P95_LIMIT: Duration = Duration::from_millis(8);
/// The coverage a whole-gate result needs: both conditions, each with at
/// least this many measured keys of the app and of plain Ghostty.
const REQUIRED_CONDITIONS: [Condition; 2] = [Condition::Idle, Condition::Busy];
const REQUIRED_KEYS: usize = 40;

/// What else the window is doing while keys are typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Condition {
    /// Nothing.
    Idle,
    /// A neighbouring pane below the typing pane streams output.
    Busy,
}

impl Condition {
    fn name(self) -> &'static str {
        match self {
            Condition::Idle => "idle",
            Condition::Busy => "busy",
        }
    }

    fn from_name(name: &str) -> Result<Self, String> {
        match name {
            "idle" => Ok(Condition::Idle),
            "busy" => Ok(Condition::Busy),
            _ => Err(format!("unknown condition {name}")),
        }
    }

    /// The part of the frame the typing pane is in: all of it when idle, and
    /// the top 45%, above the streaming pane, when busy.
    fn region(self) -> Region {
        match self {
            Condition::Idle => Region::Whole,
            Condition::Busy => Region::Top(45),
        }
    }
}

struct Settings {
    keys: usize,
    warmup_keys: usize,
    runs: usize,
    seed: u64,
    conditions: Vec<Condition>,
    targets: Vec<Target>,
    /// The refresh rate to use when the display does not report one.
    refresh: Option<f64>,
}

/// One run of one target in one condition.
struct Measured {
    condition: Condition,
    target: Target,
    run: usize,
    /// Each measured key's latency, or `None` for a key that never showed.
    latencies: Vec<Option<Duration>>,
    window: Window,
    /// Why the run measured nothing, if it did not.
    skipped: Option<String>,
}

/// Runs gate 3 and writes `summary.md` and `raw.tsv` to `--out`.
pub fn run(options: &Options) -> Result<(), String> {
    let out = Path::new(options.required("out")?);
    let settings = Settings {
        keys: options.get("keys", 40)?,
        warmup_keys: options.get("warmup-keys", 5)?,
        runs: options.get("runs", 1)?,
        seed: options.get("seed", 1)?,
        conditions: options
            .get("conditions", "idle,busy".to_owned())?
            .split(',')
            .map(Condition::from_name)
            .collect::<Result<_, _>>()?,
        targets: Target::list(
            options,
            &[Target::App, Target::Ghostty, Target::GhosttyTmux],
        )?,
        refresh: report::refresh_option(options)?,
    };
    if settings.keys == 0 || settings.runs == 0 || settings.targets.is_empty() {
        return Err("--keys, --runs and --targets must not be empty".to_owned());
    }
    let programs = Programs::from_options(options)?;
    let environment = report::environment(&programs);
    let work = tempfile::Builder::new()
        .prefix("tw-gate3-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let mut rng = Rng(settings.seed);
    let mut results = Vec::new();
    for &condition in &settings.conditions {
        for run in 0..settings.runs {
            for target in report::order(&settings.targets, run) {
                let measured = measure(
                    target,
                    condition,
                    run + 1,
                    &settings,
                    &programs,
                    work.path(),
                    &mut rng,
                )?;
                results.push(measured);
            }
        }
    }
    let summary = summary(&settings, &environment, &results);
    report::write(out, &summary, &raw(&results))
}

fn measure(
    target: Target,
    condition: Condition,
    run: usize,
    settings: &Settings,
    programs: &Programs,
    work: &Path,
    rng: &mut Rng,
) -> Result<Measured, String> {
    let bench = std::env::current_exe().map_err(|error| error.to_string())?;
    let stream = format!("exec '{}' scroll-test --seconds 3600", bench.display());
    let server = if target.uses_tmux() {
        let server = targets::session("exec cat").map_err(|error| error.to_string())?;
        if condition == Condition::Busy {
            let split = ["split-window", "-v", "-t", SESSION, "sh", "-c", &stream];
            let top = format!("{SESSION}:0.0");
            for command in [&split[..], &["select-pane", "-t", &top]] {
                server.run(command).map_err(|error| error.to_string())?;
            }
        }
        Some(server)
    } else {
        None
    };
    // Plain Ghostty runs this in its first split, and again in the split
    // opened below it, which streams instead.
    let marker = work.join(format!("{}-{run}.split", target.name()));
    // The second split leaves its own file, so its existence shows the
    // streaming split really started.
    let streaming = work.join(format!("{}-{run}.streaming", target.name()));
    let plain = match condition {
        Condition::Idle => "exec cat".to_owned(),
        Condition::Busy => format!(
            "if [ -e '{0}' ]; then : > '{1}'; {stream}; fi; : > '{0}'; exec cat",
            marker.display(),
            streaming.display()
        ),
    };
    let running = targets::start(target, programs, server.as_ref(), &plain, work)?;
    if condition == Condition::Busy && target == Target::Ghostty {
        // Ghostty's default keys: Command-Shift-D splits downwards, and
        // Command-Option-Up returns to the split above.
        let split = Modifiers {
            command: true,
            shift: true,
            ..Modifiers::default()
        };
        let up = Modifiers {
            command: true,
            option: true,
            ..Modifiers::default()
        };
        running.post_key(keycode::D, split)?;
        thread::sleep(Duration::from_secs(1));
        running.post_key(keycode::UP, up)?;
        thread::sleep(Duration::from_millis(500));
        if !streaming.exists() {
            // Not measured rather than measured without its neighbour.
            let after = running.check_visible()?;
            let window = Window::new(target, &running, CaptureStats::default(), after);
            return Ok(Measured {
                condition,
                target,
                run,
                latencies: Vec::new(),
                window,
                skipped: Some("plain Ghostty did not open the streaming split".to_owned()),
            });
        }
    }
    let region = condition.region();
    let frames = Frames::start(running.window().id, move |image| {
        region_digest(image, region.rect(image))
    })?;
    thread::sleep(Duration::from_millis(500));
    running.check_visible()?;

    let mut watch = ChangeWatch::default();
    let mut latencies = Vec::with_capacity(settings.keys);
    for index in 0..settings.warmup_keys + settings.keys {
        if index > 0 && index % LINE_KEYS == 0 {
            running.post_key(keycode::RETURN, Modifiers::default())?;
            thread::sleep(Duration::from_millis(300));
        }
        for seen in frames.drain() {
            watch.before(seen);
        }
        let letter = char::from(LETTERS[index % LETTERS.len()]);
        let posted = running.post_text(&letter.to_string())?;
        let shown = wait_for_change(&frames, &mut watch, posted);
        if index >= settings.warmup_keys {
            latencies.push(shown.map(|time| time.since(posted)));
        }
        // A window that stopped being visible stopped drawing: the run is
        // void. Checking here, after the key showed, keeps the check out of
        // the measured time.
        running.check_visible()?;
        let gap = Duration::from_millis(rng.range(150, 350) as u64);
        thread::sleep(gap.saturating_sub(clock::now().since(posted)));
    }
    let after = running.check_visible()?;
    let (stats, _) = frames.stop()?;
    let window = Window::new(target, &running, stats, after);
    running.stop();
    drop(server);
    Ok(Measured {
        condition,
        target,
        run,
        latencies,
        window,
        skipped: None,
    })
}

/// Waits up to [`KEY_TIMEOUT`] for the first frame after `posted` that
/// differs from the frame shown before it.
fn wait_for_change(
    frames: &Frames<u64>,
    watch: &mut ChangeWatch,
    posted: HostTime,
) -> Option<HostTime> {
    let deadline = Instant::now() + KEY_TIMEOUT;
    loop {
        let seen = frames.next(deadline.saturating_duration_since(Instant::now()))?;
        if let Some(shown) = watch.after(seen, posted) {
            return Some(shown);
        }
    }
}

/// Follows the digests of successive frames to find the first change after a
/// key.
#[derive(Default)]
struct ChangeWatch {
    /// The digest of the latest frame seen.
    last: Option<u64>,
}

impl ChangeWatch {
    /// Notes a frame shown before the key was posted.
    fn before(&mut self, seen: Seen<u64>) {
        self.last = Some(seen.value);
    }

    /// Notes a frame that arrived after the key was posted at `posted`, and
    /// returns its display time if it is the first to show a change. A frame
    /// displayed before `posted` only updates what the screen showed.
    fn after(&mut self, seen: Seen<u64>, posted: HostTime) -> Option<HostTime> {
        let changed = seen.time > posted && self.last.is_some_and(|last| last != seen.value);
        self.last = Some(seen.value);
        changed.then_some(seen.time)
    }
}

fn raw(results: &[Measured]) -> String {
    let mut out = String::from("condition\ttarget\trun\tkey\tlatency_ns\n");
    for result in results {
        for (key, latency) in result.latencies.iter().enumerate() {
            let latency = latency.map_or("missed".to_owned(), |time| time.as_nanos().to_string());
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{}\t{latency}",
                result.condition.name(),
                result.target.name(),
                result.run,
                key + 1
            );
        }
    }
    out
}

/// The latencies of one target in one condition, over every run.
struct Pooled {
    shown: Vec<Duration>,
    missed: usize,
}

fn pooled(results: &[Measured], condition: Condition, target: Target) -> Option<Pooled> {
    let runs: Vec<&Measured> = results
        .iter()
        .filter(|result| result.condition == condition && result.target == target)
        .collect();
    if runs.is_empty() {
        return None;
    }
    let all = runs.iter().flat_map(|result| result.latencies.iter());
    Some(Pooled {
        shown: all.clone().flatten().copied().collect(),
        missed: all.filter(|latency| latency.is_none()).count(),
    })
}

fn summary(settings: &Settings, environment: &[(&str, String)], results: &[Measured]) -> String {
    let verdict = verdict(settings, results);
    let mut out = report::opening("Gate 3: typing latency", &verdict, environment);
    let _ = write!(
        out,
        "\n## Settings\n\n\
         - {} measured keys per run after {} unmeasured ones, {} run(s) per target and \
           condition, gaps of 150 to 350 ms from seed {}; Return after every {} keys.\n\
         - The pane runs `cat`. Busy: a pane below it streams `scroll-test`, and only the \
           top 45% of the window is compared.\n\
         - A key is missed if no changed frame shows within {} ms.\n",
        settings.keys,
        settings.warmup_keys,
        settings.runs,
        settings.seed,
        LINE_KEYS,
        KEY_TIMEOUT.as_millis(),
    );
    for &condition in &settings.conditions {
        let _ = write!(
            out,
            "\n## {}\n\n| Target | Keys shown | Missed | Median ms | p95 ms | Fastest ms | Slowest ms |\n\
             | --- | --- | --- | --- | --- | --- | --- |\n",
            condition.name()
        );
        for &target in &settings.targets {
            let Some(pooled) = pooled(results, condition, target) else {
                continue;
            };
            let row = summarize(&pooled.shown).map_or("- | - | - | -".to_owned(), |s| {
                format!(
                    "{:.2} | {:.2} | {:.2} | {:.2}",
                    ms(s.median),
                    ms(s.p95),
                    ms(s.min),
                    ms(s.max)
                )
            });
            let _ = writeln!(
                out,
                "| {} | {} | {} | {row} |",
                target.name(),
                pooled.shown.len(),
                pooled.missed
            );
        }
    }
    let windows: Vec<(String, Window)> = results
        .iter()
        .map(|result| {
            (
                format!("{} {}", result.condition.name(), result.run),
                result.window.clone(),
            )
        })
        .collect();
    out.push_str(&report::windows_table(&windows));
    out
}

fn summarize(times: &[Duration]) -> Option<Summary> {
    (!times.is_empty()).then(|| stats::summarize(times))
}

fn verdict(settings: &Settings, results: &[Measured]) -> (String, Vec<String>) {
    let mut reasons = Vec::new();
    let mut failed = false;
    let mut undecided = false;
    for &condition in &settings.conditions {
        let summary =
            |target| pooled(results, condition, target).and_then(|pooled| summarize(&pooled.shown));
        let (Some(app), Some(ghostty)) = (summary(Target::App), summary(Target::Ghostty)) else {
            undecided = true;
            reasons.push(format!(
                "{}: not decided, the app and plain Ghostty were not both measured",
                condition.name()
            ));
            continue;
        };
        // A missed key's latency is unknown and over a second, so the
        // percentiles of the keys shown cannot decide the gate.
        let missed = |target| pooled(results, condition, target).map_or(0, |pooled| pooled.missed);
        let (app_missed, ghostty_missed) = (missed(Target::App), missed(Target::Ghostty));
        if app_missed + ghostty_missed > 0 {
            undecided = true;
            reasons.push(format!(
                "{}: not decided, {app_missed} key(s) of the app and {ghostty_missed} of plain \
                 Ghostty were missed",
                condition.name()
            ));
            continue;
        }
        let Some(refresh) = results
            .iter()
            .find(|result| result.condition == condition && result.target == Target::App)
            .and_then(|result| result.window.refresh)
            .or(settings.refresh)
        else {
            undecided = true;
            reasons.push(format!(
                "{}: not decided, the display reports no refresh rate: give --refresh HZ",
                condition.name()
            ));
            continue;
        };
        let interval = Duration::from_secs_f64(1.0 / refresh);
        let over = |app: Duration, ghostty: Duration| ms(app) - ms(ghostty);
        // A clear failure of either limit fails, whatever the other shows.
        let judged = Judged::of(app.median, ghostty.median + MEDIAN_LIMIT, interval)
            .max(Judged::of(app.p95, ghostty.p95 + P95_LIMIT, interval));
        failed |= judged == Judged::Fails;
        undecided |= judged == Judged::Near;
        reasons.push(format!(
            "{}: the app's median is {:+.2} ms and its p95 {:+.2} ms from plain Ghostty's \
             (limits +{} and +{} ms, one refresh interval {:.2} ms): {}",
            condition.name(),
            over(app.median, ghostty.median),
            over(app.p95, ghostty.p95),
            MEDIAN_LIMIT.as_millis(),
            P95_LIMIT.as_millis(),
            ms(interval),
            judged.describe(),
        ));
        if let Some(tmux) = summary(Target::GhosttyTmux) {
            reasons.push(format!(
                "{}: Ghostty with tmux's median is {:+.2} ms from plain Ghostty's, tmux's own share",
                condition.name(),
                over(tmux.median, ghostty.median),
            ));
        }
    }
    for result in results {
        if let Some(why) = &result.skipped {
            reasons.push(format!(
                "{} run {} of {}: not measured, {why}",
                result.condition.name(),
                result.run,
                result.target.name()
            ));
        }
    }
    let missed: usize = results
        .iter()
        .map(|result| result.latencies.iter().filter(|l| l.is_none()).count())
        .sum();
    if missed > 0 {
        reasons.push(format!(
            "{missed} key(s) never showed within the time limit"
        ));
    }
    reasons.push(
        "An acceptance result needs three independent runs of a clean build on a quiet \
         machine (decision record 0018); this report alone does not make one."
            .to_owned(),
    );
    (
        report::verdict(3, failed, undecided, &missing(settings, results)),
        reasons,
    )
}

/// Lists what the run left out of the coverage a whole-gate result needs.
fn missing(settings: &Settings, results: &[Measured]) -> Vec<String> {
    let mut missing = Vec::new();
    for condition in REQUIRED_CONDITIONS {
        if !settings.conditions.contains(&condition) {
            missing.push(format!("{} not run", condition.name()));
            continue;
        }
        for target in [Target::App, Target::Ghostty] {
            let keys = pooled(results, condition, target)
                .map_or(0, |pooled| pooled.shown.len() + pooled.missed);
            if keys < REQUIRED_KEYS {
                missing.push(format!(
                    "{}: {keys} measured keys of {}, needs {REQUIRED_KEYS}",
                    condition.name(),
                    target.name()
                ));
            }
        }
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
    fn the_first_changed_frame_after_the_key_counts() {
        let posted = HostTime::from_nanos(100);
        let mut watch = ChangeWatch::default();
        watch.before(seen(90, 1));
        // Displayed before the key: it only updates what the screen showed.
        assert_eq!(watch.after(seen(95, 2), posted), None);
        assert_eq!(watch.after(seen(110, 2), posted), None);
        assert_eq!(
            watch.after(seen(120, 3), posted),
            Some(HostTime::from_nanos(120))
        );
    }

    fn settings(conditions: &[Condition]) -> Settings {
        Settings {
            keys: 40,
            warmup_keys: 5,
            runs: 1,
            seed: 1,
            conditions: conditions.to_vec(),
            targets: vec![Target::App, Target::Ghostty],
            refresh: None,
        }
    }

    /// A run of `target` in `condition` whose keys all took `latency` ms.
    fn measured(condition: Condition, target: Target, keys: usize, latency: f64) -> Measured {
        Measured {
            condition,
            target,
            run: 1,
            latencies: vec![Some(Duration::from_secs_f64(latency / 1e3)); keys],
            window: Window {
                target,
                origin: (40.0, 60.0),
                size: (1600.0, 850.0),
                refresh: Some(60.0),
                display: Default::default(),
                build: None,
                stats: CaptureStats::default(),
            },
            skipped: None,
        }
    }

    #[test]
    fn a_result_within_one_refresh_of_a_limit_is_not_decided() {
        let all = [Condition::Idle, Condition::Busy];
        let gate = |app: f64| {
            let results: Vec<Measured> = all
                .iter()
                .flat_map(|&condition| {
                    [
                        measured(condition, Target::App, 40, app),
                        measured(condition, Target::Ghostty, 40, 20.0),
                    ]
                })
                .collect();
            verdict(&settings(&all), &results).0
        };
        // The limits are +4 ms (median) and +8 ms (p95); at 60 Hz one
        // refresh interval is 16.7 ms.
        assert_eq!(gate(2.0), "Gate 3 passed");
        assert_eq!(gate(30.0), "Gate 3 not decided");
        assert_eq!(gate(20.0), "Gate 3 not decided");
        assert_eq!(gate(60.0), "Gate 3 failed");
    }

    #[test]
    fn a_subset_of_conditions_or_keys_cannot_pass() {
        let results = [
            measured(Condition::Idle, Target::App, 8, 2.0),
            measured(Condition::Idle, Target::Ghostty, 8, 20.0),
        ];
        let (verdict, _) = verdict(&settings(&[Condition::Idle]), &results);
        assert_eq!(
            verdict,
            "Gate 3 not decided (exploratory subset: idle: 8 measured keys of app, needs 40; \
             idle: 8 measured keys of ghostty, needs 40; busy not run)"
        );
    }

    #[test]
    fn with_no_frame_before_the_key_the_first_one_is_the_baseline() {
        let posted = HostTime::from_nanos(100);
        let mut watch = ChangeWatch::default();
        assert_eq!(watch.after(seen(110, 7), posted), None);
        assert_eq!(
            watch.after(seen(120, 8), posted),
            Some(HostTime::from_nanos(120))
        );
    }
}
