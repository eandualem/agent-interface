//! Gate 5, control-mode fidelity
//! (`docs/decisions/0018-milestone-0-benchmark-method-revised.md`), by the
//! harness decision record 0021 describes: each scenario rebuilds a pane with
//! the app's recovery code on a control connection and compares it, cell by
//! cell, with an ordinary tmux client attached to the same pane.
//!
//! The gate runs on the tmux found first on `PATH`, and reports for that
//! version. Every server is private, through `tmux-workspaces-test-support`.

mod candidate;
#[cfg(test)]
mod coexistence;
mod compare;
mod connection;
mod fixtures;
mod pty;
mod scenarios;
mod session;

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use tmux_workspaces_test_support::TmuxServer;

use crate::environment;
use crate::options::Options;
use compare::Difference;
use fixtures::{After, FIXTURES, Fixture, Why};
use scenarios::Rebuild;
use session::Layout;

/// The result of comparing a rebuilt pane with the reference.
#[derive(Debug, Default)]
pub struct Comparison {
    /// The pane's width in cells.
    pub width: u16,
    /// The cells that differ.
    pub cells: Vec<Difference>,
    /// How the cursor's position differs, if it does.
    pub cursor: Option<String>,
    /// Other differences: the cursor's visibility, the modes, the pane's
    /// size.
    pub notes: Vec<String>,
    /// Modes this tmux version does not report, which were not compared.
    pub unknown: Vec<&'static str>,
}

impl Comparison {
    fn matches(&self) -> bool {
        self.cells.is_empty() && self.cursor.is_none() && self.notes.is_empty()
    }
}

/// One comparison of a rebuilt pane with the reference.
#[derive(Debug)]
pub struct Compared {
    /// When it was made.
    pub after: After,
    /// The window's size, where the scenario resized it.
    pub at: Option<(u16, u16)>,
    /// What it found.
    pub comparison: Comparison,
}

impl Compared {
    /// A comparison right after a rebuild.
    pub fn rebuilt(at: Option<(u16, u16)>, comparison: Comparison) -> Self {
        Self {
            after: After::Rebuild,
            at,
            comparison,
        }
    }

    /// A comparison after more output and input.
    pub fn more(comparison: Comparison) -> Self {
        Self {
            after: After::More,
            at: None,
            comparison,
        }
    }

    /// The two comparisons most scenarios make: after the rebuild, then
    /// after more output and input.
    fn pair([rebuilt, more]: [Comparison; 2]) -> Vec<Self> {
        vec![Self::rebuilt(None, rebuilt), Self::more(more)]
    }

    fn label(&self) -> String {
        let when = match self.after {
            After::Rebuild => "after the rebuild",
            _ => "after more output and input",
        };
        match self.at {
            Some((cols, rows)) => format!("{when} at {cols}x{rows}"),
            None => when.to_owned(),
        }
    }
}

/// A check that passes or fails on its own, such as a rebuild continuing a
/// count.
#[derive(Debug)]
pub struct Check {
    /// What was checked.
    pub name: String,
    /// Whether it held, and why not.
    pub result: Result<(), String>,
}

/// Every scenario, in report order.
const SCENARIOS: [&str; 9] = [
    "attach",
    "pause",
    "split-attach",
    "split-pause",
    "resize",
    "flow",
    "own-pause",
    "exit",
    "hooks",
];

/// One scenario's comparisons of one fixture.
struct Entry {
    scenario: &'static str,
    fixture: Option<&'static Fixture>,
    name: &'static str,
    result: Result<Vec<Compared>, String>,
}

/// How one comparison came out.
enum Verdict {
    Match,
    /// Matched where a difference was expected.
    MatchedDespite(Why),
    /// Differed as a limit of record 0021 explains.
    Explained(&'static str),
    /// Differed in a way nothing expected explains, beyond the expected
    /// difference if there was one.
    Unexplained(Option<Why>),
}

impl Verdict {
    fn passes(&self) -> bool {
        matches!(
            self,
            Self::Match | Self::MatchedDespite(_) | Self::Explained(_)
        )
    }
}

/// Runs gate 5 on the tmux first on `PATH`, writing `summary.md` and
/// `raw.tsv` to `--out`.
pub fn run(options: &Options) -> Result<(), String> {
    let out = Path::new(options.required("out")?);
    let scenarios = list(options, "scenarios", &SCENARIOS)?;
    let names: Vec<&str> = FIXTURES.iter().map(|fixture| fixture.name).collect();
    let fixture_names = list(options, "fixtures", &names)?;
    let flow_seconds: f64 = options.get("flow-seconds", 6.0)?;
    let stalls: usize = options.get("pauses", 20)?;
    if flow_seconds < 3.0 {
        return Err("--flow-seconds must be at least 3".to_owned());
    }
    fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    let version_text = TmuxServer::start()
        .and_then(|server| server.run(&["display-message", "-p", "#{version}"]))
        .map(|version| version.trim().to_owned())
        .map_err(|error| format!("starting tmux: {error}"))?;
    let version = fixtures::version(&version_text)
        .ok_or_else(|| format!("cannot read tmux version {version_text:?}"))?;
    let environment = environment::describe(&version_text);
    let selected: Vec<&'static Fixture> = FIXTURES
        .iter()
        .filter(|fixture| fixture_names.contains(&fixture.name))
        .collect();

    let mut entries = Vec::new();
    let mut checks: Vec<(&str, Check)> = Vec::new();
    let mut own_pauses = None;
    for &scenario in &scenarios {
        eprintln!("gate 5: {scenario}");
        let run_fixtures = |layout: Layout, rebuild: Rebuild, entries: &mut Vec<Entry>| {
            for &fixture in &selected {
                if scenarios::runs(fixture, layout) {
                    entries.push(Entry {
                        scenario,
                        fixture: Some(fixture),
                        name: fixture.name,
                        result: scenarios::fixture(version, fixture, layout, rebuild)
                            .map(Compared::pair),
                    });
                }
            }
        };
        match scenario {
            "attach" => run_fixtures(Layout::One(40, 8), Rebuild::Attach, &mut entries),
            "pause" => run_fixtures(Layout::One(40, 8), Rebuild::Pause, &mut entries),
            "split-attach" => run_fixtures(Layout::Split, Rebuild::Attach, &mut entries),
            "split-pause" => run_fixtures(Layout::Split, Rebuild::Pause, &mut entries),
            "resize" => {
                for &fixture in &selected {
                    if matches!(
                        fixture.runs,
                        fixtures::Runs::Resize | fixtures::Runs::ResizeWhileDrawing
                    ) {
                        entries.push(Entry {
                            scenario,
                            fixture: Some(fixture),
                            name: fixture.name,
                            result: scenarios::resize(
                                version,
                                fixture,
                                &out.join("resize").join(fixture.name),
                            ),
                        });
                    }
                }
            }
            "flow" => {
                let result = scenarios::flow(version, flow_seconds).map(|(found, comparisons)| {
                    checks.extend(found.into_iter().map(|check| (scenario, check)));
                    Compared::pair(comparisons)
                });
                entries.push(Entry {
                    scenario,
                    fixture: None,
                    name: "count",
                    result,
                });
            }
            "own-pause" => match scenarios::own_pause(version, stalls) {
                Ok(found) => {
                    checks.extend(found.checks.into_iter().map(|check| (scenario, check)));
                    own_pauses = Some(found.paused);
                    if let Some(comparisons) = found.comparisons {
                        entries.push(Entry {
                            scenario,
                            fixture: None,
                            name: "count",
                            result: Ok(Compared::pair(comparisons)),
                        });
                    }
                }
                Err(error) => checks.push((
                    scenario,
                    Check {
                        name: "the scenario ran".to_owned(),
                        result: Err(error),
                    },
                )),
            },
            "exit" => {
                let after_pause =
                    scenarios::exit_after_pause(version).unwrap_or_else(|error| Check {
                        name: "a pane that exits after its pause fails the rebuild".to_owned(),
                        result: Err(error),
                    });
                checks.push((scenario, after_pause));
                let result = scenarios::exit_during_attach(version).map(|(check, comparisons)| {
                    checks.push((scenario, check));
                    Compared::pair(comparisons)
                });
                entries.push(Entry {
                    scenario,
                    fixture: None,
                    name: "remaining pane",
                    result,
                });
            }
            "hooks" => match scenarios::hooks(version) {
                Ok(found) => checks.extend(found.into_iter().map(|check| (scenario, check))),
                Err(error) => checks.push((
                    scenario,
                    Check {
                        name: "the scenario ran".to_owned(),
                        result: Err(error),
                    },
                )),
            },
            _ => unreachable!("scenario names are checked"),
        }
    }

    // A gate result needs every scenario and fixture, and tmux's own pause
    // seen at least once.
    let mut incomplete = Vec::new();
    if scenarios.len() < SCENARIOS.len() {
        incomplete.push(format!("only the scenarios {}", scenarios.join(", ")));
    }
    if fixture_names.len() < names.len() {
        incomplete.push(format!("only the fixtures {}", fixture_names.join(", ")));
    }
    if own_pauses == Some(0) {
        incomplete.push(format!(
            "none of the {stalls} stalls ended in tmux's own pause"
        ));
    }
    let report = Report {
        version_text: &version_text,
        version,
        environment: &environment,
        entries: &entries,
        checks: &checks,
        own_pauses: own_pauses.map(|paused| (paused, stalls)),
        incomplete: &incomplete,
    };
    fs::write(out.join("summary.md"), report.summary())
        .and_then(|()| fs::write(out.join("raw.tsv"), report.raw()))
        .map_err(|error| format!("{}: {error}", out.display()))?;
    eprintln!(
        "gate 5 on tmux {version_text}: {}; see {}",
        match report.outcome() {
            Outcome::Pass => "PASS",
            Outcome::Fail => "FAIL",
            Outcome::Incomplete => "INCOMPLETE",
        },
        out.join("summary.md").display()
    );
    Ok(())
}

/// The gate's result on one tmux version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Pass,
    Fail,
    /// Nothing failed, but the run left something out.
    Incomplete,
}

/// Returns option `name` as a comma-separated list of names from `known`,
/// or all of `known` if it is missing.
fn list<'a>(options: &Options, name: &str, known: &[&'a str]) -> Result<Vec<&'a str>, String> {
    let Ok(value) = options.required(name) else {
        return Ok(known.to_vec());
    };
    let mut items: Vec<&str> = Vec::new();
    for item in value.split(',') {
        let known = known
            .iter()
            .find(|known| **known == item)
            .copied()
            .ok_or_else(|| format!("--{name}: unknown {item}"))?;
        if items.contains(&known) {
            return Err(format!("--{name}: {item} given twice"));
        }
        items.push(known);
    }
    Ok(items)
}

struct Report<'a> {
    version_text: &'a str,
    version: (u32, u32),
    environment: &'a [(&'static str, String)],
    entries: &'a [Entry],
    checks: &'a [(&'a str, Check)],
    own_pauses: Option<(usize, usize)>,
    /// What the run left out, if anything.
    incomplete: &'a [String],
}

impl Report<'_> {
    fn verdict(&self, entry: &Entry, after: After, comparison: &Comparison) -> Verdict {
        let expected = entry
            .fixture
            .and_then(|fixture| fixture.expected(after, self.version));
        match expected {
            None if comparison.matches() => Verdict::Match,
            None => Verdict::Unexplained(None),
            Some(expected) if comparison.matches() => Verdict::MatchedDespite(expected.why),
            Some(expected) if !expected.explains(comparison) => {
                Verdict::Unexplained(Some(expected.why))
            }
            Some(expected) => match expected.why {
                Why::Limit(limit) => Verdict::Explained(limit),
            },
        }
    }

    fn verdicts(&self) -> impl Iterator<Item = (&Entry, &Compared, &Comparison, Verdict)> {
        self.entries.iter().flat_map(move |entry| {
            entry.result.iter().flatten().map(move |compared| {
                (
                    entry,
                    compared,
                    &compared.comparison,
                    self.verdict(entry, compared.after, &compared.comparison),
                )
            })
        })
    }

    fn outcome(&self) -> Outcome {
        let passed = self.entries.iter().all(|entry| entry.result.is_ok())
            && self.verdicts().all(|(.., verdict)| verdict.passes())
            && self.checks.iter().all(|(_, check)| check.result.is_ok());
        if !passed {
            Outcome::Fail
        } else if self.incomplete.is_empty() {
            Outcome::Pass
        } else {
            Outcome::Incomplete
        }
    }

    fn summary(&self) -> String {
        let mut text = String::new();
        let _ = writeln!(
            text,
            "# Gate 5: control-mode fidelity on tmux {}\n",
            self.version_text
        );
        let mut counts = [0usize; 4];
        for (.., verdict) in self.verdicts() {
            counts[match verdict {
                Verdict::Match | Verdict::MatchedDespite(_) => 0,
                Verdict::Explained(_) => 1,
                Verdict::Unexplained(_) => 2,
            }] += 1;
        }
        counts[3] = self
            .entries
            .iter()
            .filter(|entry| entry.result.is_err())
            .count();
        let failed_checks = self
            .checks
            .iter()
            .filter(|(_, check)| check.result.is_err())
            .count();
        let _ = writeln!(
            text,
            "**{}.** Of {} comparisons, {} match, {} differ as a limit of decision record 0021 explains and {} differ unexplained. {} fixture runs failed to complete. {} of {} checks failed.\n",
            match self.outcome() {
                Outcome::Pass => "Passed".to_owned(),
                Outcome::Fail => "Failed".to_owned(),
                Outcome::Incomplete => format!(
                    "Incomplete, not a gate result: {}",
                    self.incomplete.join("; ")
                ),
            },
            counts[..3].iter().sum::<usize>(),
            counts[0],
            counts[1],
            counts[2],
            counts[3],
            failed_checks,
            self.checks.len(),
        );
        text.push_str("## Environment\n\n| | |\n| --- | --- |\n");
        for (name, value) in self.environment {
            let _ = writeln!(text, "| {name} | {value} |");
        }
        text.push_str("\n## Comparisons\n\n| Scenario | Fixture | Compared | Result |\n| --- | --- | --- | --- |\n");
        for entry in self.entries {
            match &entry.result {
                Ok(compared) => {
                    for compared in compared {
                        let verdict = self.verdict(entry, compared.after, &compared.comparison);
                        let _ = writeln!(
                            text,
                            "| {} | {} | {} | {} |",
                            entry.scenario,
                            entry.name,
                            compared.label(),
                            describe(&verdict, &compared.comparison)
                        );
                    }
                }
                Err(error) => {
                    let _ = writeln!(
                        text,
                        "| {} | {} | | did not complete: {error} |",
                        entry.scenario, entry.name
                    );
                }
            }
        }
        if !self.checks.is_empty() || self.own_pauses.is_some() {
            text.push_str("\n## Checks\n\n");
            if let Some((paused, stalls)) = self.own_pauses {
                let _ = writeln!(
                    text,
                    "tmux's own pause: {paused} of {stalls} stalls ended in a pause.\n"
                );
            }
            text.push_str("| Scenario | Check | Result |\n| --- | --- | --- |\n");
            for (scenario, check) in self.checks {
                let result = match &check.result {
                    Ok(()) => "passed".to_owned(),
                    Err(error) => format!("**failed**: {error}"),
                };
                let _ = writeln!(text, "| {scenario} | {} | {result} |", check.name);
            }
        }
        let unknown: BTreeSet<&str> = self
            .verdicts()
            .flat_map(|(_, _, comparison, _)| comparison.unknown.iter().copied())
            .collect();
        text.push_str("\n## Not compared\n\n");
        if !unknown.is_empty() {
            let _ = writeln!(
                text,
                "- Modes tmux {} does not report: {}.",
                self.version_text,
                unknown.into_iter().collect::<Vec<_>>().join(", ")
            );
        }
        text.push_str("- The extended-keys mode: the terminal engine does not report it.\n");
        let details: Vec<_> = self
            .verdicts()
            .filter(|(_, _, comparison, _)| !comparison.matches())
            .collect();
        if !details.is_empty() {
            text.push_str("\n## Differences\n");
            for (entry, compared, comparison, verdict) in details {
                let _ = writeln!(
                    text,
                    "\n### {} / {}, {}: {}\n",
                    entry.scenario,
                    entry.name,
                    compared.label(),
                    describe(&verdict, comparison)
                );
                for note in comparison.cursor.iter().chain(&comparison.notes) {
                    let _ = writeln!(text, "- {note}");
                }
                for cell in comparison.cells.iter().take(6) {
                    let _ = writeln!(
                        text,
                        "- row {}, column {}, {:?}: reference {}; rebuilt {}",
                        cell.row, cell.col, cell.properties, cell.reference, cell.candidate
                    );
                }
                if comparison.cells.len() > 6 {
                    let _ = writeln!(text, "- and {} more cells", comparison.cells.len() - 6);
                }
            }
        }
        text
    }

    fn raw(&self) -> String {
        let mut text = String::from("scenario\tfixture\tafter\tverdict\tcells\tnotes\n");
        for (entry, compared, comparison, verdict) in self.verdicts() {
            let _ = writeln!(
                text,
                "{}\t{}\t{}\t{}\t{}\t{}",
                entry.scenario,
                entry.name,
                match (compared.after, compared.at) {
                    (After::Rebuild, Some((cols, rows))) => format!("rebuild-{cols}x{rows}"),
                    (After::Rebuild, None) => "rebuild".to_owned(),
                    _ => "more".to_owned(),
                },
                match verdict {
                    Verdict::Match => "match".to_owned(),
                    Verdict::MatchedDespite(_) => "match-despite-limit".to_owned(),
                    Verdict::Explained(_) => "limit".to_owned(),
                    Verdict::Unexplained(_) => "unexplained".to_owned(),
                },
                comparison.cells.len(),
                comparison
                    .cursor
                    .iter()
                    .chain(&comparison.notes)
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            );
        }
        for entry in self.entries {
            if let Err(error) = &entry.result {
                let _ = writeln!(
                    text,
                    "{}\t{}\t-\terror\t-\t{error}",
                    entry.scenario, entry.name
                );
            }
        }
        for (scenario, check) in self.checks {
            let _ = writeln!(
                text,
                "{scenario}\t{}\t-\t{}\t-\t{}",
                check.name,
                if check.result.is_ok() { "pass" } else { "fail" },
                check.result.as_ref().err().map_or("", String::as_str)
            );
        }
        text
    }
}

fn describe(verdict: &Verdict, comparison: &Comparison) -> String {
    let what = || {
        let mut parts = Vec::new();
        if !comparison.cells.is_empty() {
            parts.push(format!("{} cells", comparison.cells.len()));
        }
        if comparison.cursor.is_some() {
            parts.push("cursor".to_owned());
        }
        if !comparison.notes.is_empty() {
            parts.push(format!("{} other", comparison.notes.len()));
        }
        parts.join(", ")
    };
    match verdict {
        Verdict::Match => "match".to_owned(),
        Verdict::MatchedDespite(Why::Limit(why)) => {
            format!("match, although expected to differ: {why}")
        }
        Verdict::Explained(limit) => format!("differs ({}): {limit}", what()),
        Verdict::Unexplained(None) => format!("**differs** ({}): unexplained", what()),
        Verdict::Unexplained(Some(Why::Limit(why))) => format!(
            "**differs** ({}): unexplained, beyond what is expected ({why})",
            what()
        ),
    }
}
