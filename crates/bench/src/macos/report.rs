//! What the display gates' reports share: the environment table, a line per
//! measured window, and writing `summary.md` and `raw.tsv`.

use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::time::Duration;

use tmux_workspaces_test_support::TmuxServer;

use super::capture::CaptureStats;
use super::displays;
use super::targets::{Programs, Target};
use super::windows::WindowInfo;
use crate::environment;

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
    verdict: &(&str, Vec<String>),
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
    /// Its size in points, title bar included.
    pub size: (f64, f64),
    /// The refresh rate of its display, if known.
    pub refresh: Option<f64>,
    /// The capture's counts.
    pub stats: CaptureStats,
}

impl Window {
    /// Describes `window`, drawn by `target`, captured with `stats`.
    pub fn new(target: Target, window: &WindowInfo, stats: CaptureStats) -> Self {
        Self {
            target,
            size: (window.bounds.width, window.bounds.height),
            refresh: displays::refresh_rate(window.bounds),
            stats,
        }
    }
}

/// Returns a table of the windows measured.
pub fn windows_table(windows: &[(String, Window)]) -> String {
    let mut out = String::from(
        "\n## Windows\n\n| Run | Target | Size (points) | Refresh | Complete frames | Idle | Other |\n\
         | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for (run, window) in windows {
        let refresh = window
            .refresh
            .map_or("unknown".to_owned(), |hz| format!("{hz:.0} Hz"));
        let _ = writeln!(
            out,
            "| {run} | {} | {}×{} | {refresh} | {} | {} | {} |",
            window.target.name(),
            window.size.0,
            window.size.1,
            window.stats.complete,
            window.stats.idle,
            window.stats.other,
        );
    }
    out
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
