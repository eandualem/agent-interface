//! What the display gates' reports share: the environment table, a line per
//! measured window, how results are judged against the gates' limits, and
//! writing `summary.md` and `raw.tsv`.

use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::time::Duration;

use tmux_workspaces_test_support::TmuxServer;

use super::capture::CaptureStats;
use super::displays::{self, DisplayState};
use super::targets::{Build, Programs, Running, Target};
use crate::environment;
use crate::options::Options;

/// Returns the environment as named values, in report order: what gate 1
/// reports, with Ghostty's version and the load average before the runs.
pub fn environment(programs: &Programs) -> Vec<(&'static str, String)> {
    let tmux = TmuxServer::start()
        .and_then(|server| server.run(&["display-message", "-p", "#{version}"]))
        .map(|version| version.trim().to_owned())
        .unwrap_or_else(|error| format!("unknown ({error})"));
    let mut values = environment::describe(&tmux);
    values.push(("Ghostty", programs.ghostty_version()));
    values.push(("load average before", environment::load()));
    values
}

/// Returns the report's opening: its title, the verdict and its reasons, and
/// the environment, with the load and builds measured after the runs.
pub fn opening(
    title: &str,
    verdict: &(String, Vec<String>),
    environment: &[(&str, String)],
) -> String {
    let mut out = format!("# {title}\n\n**{}**\n\n", verdict.0);
    for line in &verdict.1 {
        let _ = writeln!(out, "- {line}");
    }
    out.push_str("\n## Environment\n\n| Item | Value |\n| --- | --- |\n");
    let after = [
        ("load average after", environment::load()),
        ("other builds running after", environment::builds()),
    ];
    for (name, value) in environment.iter().chain(after.iter()) {
        let _ = writeln!(out, "| {name} | {value} |");
    }
    out
}

/// A measured window, as the reports describe it.
#[derive(Clone, Debug)]
pub struct Window {
    /// The target that drew it.
    pub target: Target,
    /// Its top left corner in points, from the top left of the main display.
    pub origin: (f64, f64),
    /// Its size in points, title bar included.
    pub size: (f64, f64),
    /// The refresh rate of its display, if known.
    pub refresh: Option<f64>,
    /// The state of its display when the run started and when it ended.
    pub display: [DisplayState; 2],
    /// How the program was built, if it is the benchmark window.
    pub build: Option<Build>,
    /// The capture's counts.
    pub stats: CaptureStats,
}

impl Window {
    /// Describes the window of `running`, drawn by `target` and captured
    /// with `stats`, whose display was in state `after` when the run ended.
    pub fn new(
        target: Target,
        running: &Running,
        stats: CaptureStats,
        after: DisplayState,
    ) -> Self {
        let bounds = running.window().bounds;
        Self {
            target,
            origin: (bounds.x, bounds.y),
            size: (bounds.width, bounds.height),
            refresh: displays::refresh_rate(bounds),
            display: [running.display_before(), after],
            build: running.build().cloned(),
            stats,
        }
    }
}

/// Returns a table of the windows measured, and of how the benchmark window
/// was built.
pub fn windows_table(windows: &[(String, Window)]) -> String {
    let mut out = String::from(
        "\n## Windows\n\n| Run | Target | Origin (points) | Size (points) | Refresh | \
         Display asleep (start, end) | Screen locked (start, end) | Complete frames | Idle | \
         Other |\n\
         | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    let yes_no = |value: Option<bool>| match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    for (run, window) in windows {
        let refresh = window
            .refresh
            .map_or("unknown".to_owned(), |hz| format!("{hz:.0} Hz"));
        let [start, end] = window.display;
        let _ = writeln!(
            out,
            "| {run} | {} | {},{} | {}×{} | {refresh} | {}, {} | {}, {} | {} | {} | {} |",
            window.target.name(),
            window.origin.0,
            window.origin.1,
            window.size.0,
            window.size.1,
            yes_no(start.asleep),
            yes_no(end.asleep),
            yes_no(start.locked),
            yes_no(end.locked),
            window.stats.complete,
            window.stats.idle,
            window.stats.other,
        );
    }
    let mut builds: Vec<&Build> = windows
        .iter()
        .filter_map(|(_, window)| window.build.as_ref())
        .collect();
    builds.dedup();
    if !builds.is_empty() {
        out.push_str(
            "\n## Benchmark window build\n\n| Revision | Profile | Engine CPU target | \
             Engine optimisation | Terminal engine |\n\
             | --- | --- | --- | --- | --- |\n",
        );
        for build in builds {
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} |",
                build.revision, build.profile, build.cpu, build.optimize, build.engine
            );
        }
    }
    out
}

/// Reads `--refresh HZ`, the refresh rate to use when the display reports
/// none.
pub fn refresh_option(options: &Options) -> Result<Option<f64>, String> {
    if options.required("refresh").is_err() {
        return Ok(None);
    }
    let hz: f64 = options.get("refresh", 0.0)?;
    if hz > 0.0 && hz.is_finite() {
        Ok(Some(hz))
    } else {
        Err(format!("--refresh: cannot use {hz}"))
    }
}

/// Where a result lies against a limit it must not exceed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Judged {
    /// Below the limit by more than the margin.
    Passes,
    /// Within the margin of the limit, on either side: the capture leaves
    /// out the display's scan-out, so decision record 0018 has a camera
    /// confirm such a result before the gate is decided.
    Near,
    /// Above the limit by more than the margin.
    Fails,
}

impl Judged {
    /// Judges `value` against `limit`, with results within `margin` of it,
    /// one refresh interval, left to a camera.
    pub fn of(value: Duration, limit: Duration, margin: Duration) -> Self {
        if value.abs_diff(limit) <= margin {
            Judged::Near
        } else if value < limit {
            Judged::Passes
        } else {
            Judged::Fails
        }
    }

    /// Describes the outcome for a report line.
    pub fn describe(self) -> &'static str {
        match self {
            Judged::Passes => "passes",
            Judged::Near => {
                "not decided: within one refresh interval of the limit, needs camera \
                 confirmation (0018)"
            }
            Judged::Fails => "fails",
        }
    }
}

/// Returns a gate's verdict, `Gate N passed`, `failed` or `not decided`.
///
/// `missing` lists what the run left out of the gate's required coverage.
/// Without it the gate cannot pass: the verdict is at most not decided, and
/// says what is missing.
pub fn verdict(
    gate: u8,
    failed: bool,
    undecided: bool,
    missing: &[String],
    build_issues: &[String],
) -> String {
    // Only the distributable build decides a gate, either way (0018).
    let outcome = if !build_issues.is_empty() {
        "not decided"
    } else if failed {
        "failed"
    } else if undecided || !missing.is_empty() {
        "not decided"
    } else {
        "passed"
    };
    let mut notes = Vec::new();
    if !missing.is_empty() {
        notes.push(format!("exploratory subset: {}", missing.join("; ")));
    }
    if !build_issues.is_empty() {
        notes.push(format!(
            "not a distributable build: {}",
            build_issues.join("; ")
        ));
    }
    if notes.is_empty() {
        format!("Gate {gate} {outcome}")
    } else {
        format!("Gate {gate} {outcome} ({})", notes.join("; "))
    }
}

/// Returns how `build` differs from the distributable configuration: a
/// release build of the engine in `ReleaseFast` mode for the `baseline` CPU.
/// Only that configuration decides a gate (0018); `check_build` has made
/// sure the window matches the harness.
pub fn build_issues(build: &super::targets::Build) -> Vec<String> {
    let mut issues = Vec::new();
    if build.profile != "release opt-level 3" {
        issues.push(format!(
            "a {} build, not release opt-level 3",
            build.profile
        ));
    }
    if build.cpu != "baseline" {
        issues.push(format!("engine CPU target {}, not baseline", build.cpu));
    }
    if build.optimize != "ReleaseFast" {
        issues.push(format!(
            "engine optimisation {}, not ReleaseFast",
            build.optimize
        ));
    }
    issues
}

/// Writes `summary.md` and `raw.tsv` to `out` and prints the summary.
pub fn write(out: &Path, summary: &str, raw: &str) -> Result<(), String> {
    fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    fs::write(out.join("summary.md"), summary).map_err(|error| error.to_string())?;
    fs::write(out.join("raw.tsv"), raw).map_err(|error| error.to_string())?;
    print!("{summary}");
    Ok(())
}

/// Returns `targets` in the order run `run` measures them: which target goes
/// first alternates, run by run.
pub fn order(targets: &[Target], run: usize) -> Vec<Target> {
    let mut order = targets.to_vec();
    if !order.is_empty() {
        let first = run % order.len();
        order.rotate_left(first);
    }
    order
}

/// Returns `time` in milliseconds.
pub fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1e3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_within_one_refresh_of_a_limit_need_a_camera() {
        let ms = Duration::from_millis;
        let interval = Duration::from_micros(16_667);
        assert_eq!(Judged::of(ms(10), ms(30), interval), Judged::Passes);
        assert_eq!(Judged::of(ms(20), ms(30), interval), Judged::Near);
        assert_eq!(Judged::of(ms(40), ms(30), interval), Judged::Near);
        assert_eq!(Judged::of(ms(50), ms(30), interval), Judged::Fails);
        // The worst of several results decides.
        assert_eq!(Judged::Passes.max(Judged::Near), Judged::Near);
        assert_eq!(Judged::Near.max(Judged::Fails), Judged::Fails);
    }

    #[test]
    fn only_the_distributable_build_decides() {
        let build = |profile: &str, cpu: &str, optimize: &str| super::super::targets::Build {
            revision: "abc".to_owned(),
            profile: profile.to_owned(),
            cpu: cpu.to_owned(),
            optimize: optimize.to_owned(),
            engine: String::new(),
        };
        assert!(build_issues(&build("release opt-level 3", "baseline", "ReleaseFast")).is_empty());
        let debug = build_issues(&build("debug opt-level 0", "baseline", "ReleaseFast"));
        assert_eq!(debug.len(), 1);
        assert_eq!(
            verdict(4, true, false, &[], &debug),
            "Gate 4 not decided (not a distributable build: a debug opt-level 0 build, \
             not release opt-level 3)"
        );
        assert_eq!(build_issues(&build("debug", "native", "Debug")).len(), 3);
    }

    #[test]
    fn a_subset_never_passes_the_gate() {
        let missing = ["busy not run".to_owned()];
        assert_eq!(verdict(3, false, false, &[], &[]), "Gate 3 passed");
        assert_eq!(verdict(3, false, true, &[], &[]), "Gate 3 not decided");
        assert_eq!(verdict(3, true, false, &[], &[]), "Gate 3 failed");
        assert_eq!(
            verdict(3, false, false, &missing, &[]),
            "Gate 3 not decided (exploratory subset: busy not run)"
        );
        assert_eq!(
            verdict(3, true, false, &missing, &[]),
            "Gate 3 failed (exploratory subset: busy not run)"
        );
    }

    #[test]
    fn the_refresh_option_takes_a_positive_rate() {
        let options = |args: &[&str]| {
            Options::parse(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>()).unwrap()
        };
        assert_eq!(refresh_option(&options(&[])), Ok(None));
        assert_eq!(
            refresh_option(&options(&["--refresh", "120"])),
            Ok(Some(120.0))
        );
        assert!(refresh_option(&options(&["--refresh", "-60"])).is_err());
        assert!(refresh_option(&options(&["--refresh", "fast"])).is_err());
    }
}
