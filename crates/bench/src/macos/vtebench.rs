//! vtebench, gate 2's second measurement
//! (`docs/decisions/0022-vtebench-in-gate-2.md`).
//!
//! Each run starts a new window of one target, whose tmux pane runs vtebench
//! from a checkout at a pinned commit once the harness releases it. For each
//! sample, vtebench times how long its writes of one benchmark to the pane
//! block, and it writes every sample to a `.dat` file the harness reads.
//!
//! These are not display times. tmux keeps reading a pane that an ordinary
//! client shows, and discards the output that client cannot keep up with, but
//! a control client that falls behind stops tmux reading the pane. So with
//! Ghostty running tmux the times are mostly tmux's, and with the app they are
//! the slower of tmux and the app.

use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use super::capture::CaptureStats;
use super::report::{Window, ms};
use super::targets::{self, CHECK_EVERY, Programs, Target};
use crate::options::Options;
use crate::record;
use crate::stats::{self, Summary};

/// The vtebench commit the gate runs: the newest on its default branch, from
/// January 2025.
pub const COMMIT: &str = "ead80032e57dee2e75f0b51f2ea67528647d9944";
/// The benchmarks vtebench has at [`COMMIT`], in the order it runs them: the
/// gate's vtebench cases.
pub const CASES: [&str; 12] = [
    "cursor_motion",
    "dense_cells",
    "light_cells",
    "medium_cells",
    "scrolling",
    "scrolling_bottom_region",
    "scrolling_bottom_small_region",
    "scrolling_fullscreen",
    "scrolling_top_region",
    "scrolling_top_small_region",
    "sync_medium_cells",
    "unicode",
];
/// vtebench's release build, in the checkout.
const BINARY: &str = "target/release/vtebench";
/// The coverage a whole-gate result needs: runs per target, so that each
/// target runs both first and second.
const REQUIRED_RUNS: usize = 2;
/// The coverage a whole-gate result needs: bytes per sample. vtebench
/// truncates each sample to whole milliseconds, and a sample this large
/// takes at least 50 ms below 167 MB/s, which keeps that error under 2%.
const REQUIRED_BYTES: usize = 8_388_608;
/// The coverage a whole-gate result needs: seconds of samples per case.
const REQUIRED_SECS: u64 = 10;
/// How much longer than its seconds of samples one case may take: loading
/// its scripts, its warm-up sample and the sample that runs past the end.
const CASE_MARGIN: Duration = Duration::from_secs(60);

/// The pane's script. It waits on the FIFO `$1`, then runs vtebench `$2` on
/// the benchmarks of the checkout `$3`, with `$5` bytes per sample and `$6`
/// seconds per case, writing its samples to `$4.part` and its errors to
/// `$4.err`. It then writes vtebench's exit status to `$4.status` and renames
/// the samples to `$4`, the file the harness waits for. The part file is made
/// first, so `$4` appears even if vtebench writes nothing. vtebench runs in
/// the checkout with a relative benchmark path because some of its scripts
/// find their data with an unquoted `dirname $0`.
const SCRIPT: &str = r#"read -r _ < "$1"
: > "$4.part"
cd "$3" && "$2" -b benchmarks --dat "$4.part" --silent --min-bytes "$5" --max-secs "$6" 2> "$4.err"
echo "$?" > "$4.status"
mv "$4.part" "$4"
exec sleep 86400
"#;

/// How vtebench runs.
pub struct Settings {
    /// The checkout, as an absolute path: the pane does not run in this
    /// program's working directory.
    pub dir: PathBuf,
    /// Runs per target.
    pub runs: usize,
    /// vtebench's `--min-bytes`: each sample writes at least this many bytes.
    pub bytes: usize,
    /// vtebench's `--max-secs`: how long each case takes samples.
    pub secs: u64,
}

impl Settings {
    /// Reads `--vtebench DIR`, `--vtebench-runs N` (2), `--vtebench-bytes
    /// BYTES` (8388608) and `--vtebench-secs S` (10), and checks the
    /// checkout. Returns `None` without `--vtebench`.
    pub fn from_options(options: &Options) -> Result<Option<Self>, String> {
        let Ok(dir) = options.required("vtebench") else {
            return Ok(None);
        };
        let settings = Self {
            dir: checkout(Path::new(dir))?,
            runs: options.get("vtebench-runs", REQUIRED_RUNS)?,
            bytes: options.get("vtebench-bytes", REQUIRED_BYTES)?,
            secs: options.get("vtebench-secs", REQUIRED_SECS)?,
        };
        if settings.runs == 0 || settings.bytes == 0 || settings.secs == 0 {
            return Err(
                "--vtebench-runs, --vtebench-bytes and --vtebench-secs must not be 0".to_owned(),
            );
        }
        Ok(Some(settings))
    }
}

/// Checks that `dir` is a vtebench checkout at [`COMMIT`], with no changes
/// and with its release build, and returns its absolute path.
fn checkout(dir: &Path) -> Result<PathBuf, String> {
    let how = format!(
        "check out vtebench commit {COMMIT} and build it with `cargo build --release --locked`, \
         as docs/benchmarks/README.md describes"
    );
    let dir = fs::canonicalize(dir)
        .map_err(|error| format!("--vtebench {}: {error}; {how}", dir.display()))?;
    let git = |args: &[&str]| -> Result<String, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args(args)
            .output()
            .map_err(|error| format!("git: {error}"))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(format!(
                "--vtebench {}: git {} failed: {}; {how}",
                dir.display(),
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    };
    let head = git(&["rev-parse", "HEAD"])?;
    if head != COMMIT {
        return Err(format!(
            "--vtebench {}: the checkout is at {head}, not {COMMIT}; {how}",
            dir.display()
        ));
    }
    if !git(&["status", "--porcelain"])?.is_empty() {
        return Err(format!(
            "--vtebench {}: the checkout has changes; {how}",
            dir.display()
        ));
    }
    if !dir.join(BINARY).is_file() {
        return Err(format!(
            "--vtebench {}: there is no {BINARY}; {how}",
            dir.display()
        ));
    }
    Ok(dir)
}

/// A case's name and its samples, in milliseconds.
pub type Case = (String, Vec<u64>);

/// One vtebench run in one target.
pub struct Run {
    /// The target that drew the pane.
    pub target: Target,
    /// The run's number, from 1.
    pub run: usize,
    /// The samples of each case vtebench ran, in its order.
    pub cases: Vec<Case>,
    /// The window, which is not captured.
    pub window: Window,
}

/// Runs vtebench once in a new window of `target`, and returns its samples.
/// `work` is a private directory, and `run` counts from 1.
pub fn measure(
    target: Target,
    programs: &Programs,
    settings: &Settings,
    work: &Path,
    run: usize,
) -> Result<Run, String> {
    let script = work.join("vtebench.sh");
    fs::write(&script, SCRIPT).map_err(|error| error.to_string())?;
    let name = format!("vtebench-{}-{run}", target.name());
    let fifo = work.join(&name);
    let _ = fs::remove_file(&fifo);
    let made = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map_err(|error| error.to_string())?;
    if !made.success() {
        return Err("mkfifo failed".to_owned());
    }
    let dat = work.join(format!("{name}.dat"));
    // A run of the same name, with a target listed twice, must not leave its
    // samples for this one.
    for path in [dat.clone(), beside(&dat, ".status"), beside(&dat, ".err")] {
        let _ = fs::remove_file(path);
    }
    let command = format!(
        "exec sh {} {} {} {} {} {} {}",
        targets::shell_word(&script),
        targets::shell_word(&fifo),
        targets::shell_word(&settings.dir.join(BINARY)),
        targets::shell_word(&settings.dir),
        targets::shell_word(&dat),
        settings.bytes,
        settings.secs
    );
    let server = targets::session(&command, work).map_err(|error| error.to_string())?;
    let running = targets::start(target, programs, Some(&server), "", work)?;
    // As in the marker probe, the window settles before the release.
    running.wait_visible(Duration::from_millis(300))?;

    record::release(&fifo).map_err(|error| error.to_string())?;
    let timeout = (Duration::from_secs(settings.secs) + CASE_MARGIN) * CASES.len() as u32;
    let deadline = Instant::now() + timeout;
    // A window that stops being visible stops drawing, which voids the run,
    // so it is checked until vtebench has finished.
    while !dat.exists() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!(
                "{}: vtebench did not finish within {} s",
                target.name(),
                timeout.as_secs()
            ));
        }
        thread::sleep(left.min(CHECK_EVERY));
        running.check_visible()?;
    }
    let after = running.check_visible()?;
    let window = Window::new(target, &running, CaptureStats::default(), after);
    running.stop();
    drop(server);
    let cases = results(&dat).map_err(|error| format!("{}: {error}", target.name()))?;
    Ok(Run {
        target,
        run,
        cases,
        window,
    })
}

/// Reads a finished run's samples from `dat`, once the pane's script has
/// renamed them to it, after checking the exit status and the errors it wrote
/// beside them. vtebench exits successfully even when it cannot write its
/// samples, so anything it wrote to standard error fails the run.
fn results(dat: &Path) -> Result<Vec<Case>, String> {
    let status = fs::read_to_string(beside(dat, ".status"))
        .map_err(|error| format!("vtebench's exit status: {error}"))?;
    if status.trim() != "0" {
        return Err(format!("vtebench exited with status {}", status.trim()));
    }
    let errors = fs::read_to_string(beside(dat, ".err"))
        .map_err(|error| format!("vtebench's errors: {error}"))?;
    if !errors.trim().is_empty() {
        return Err(format!("vtebench reported: {}", errors.trim()));
    }
    parse(&fs::read_to_string(dat).map_err(|error| format!("{}: {error}", dat.display()))?)
}

/// Returns the path of the file beside `dat` whose name adds `suffix`.
fn beside(dat: &Path, suffix: &str) -> PathBuf {
    let mut path = dat.as_os_str().to_owned();
    path.push(suffix);
    PathBuf::from(path)
}

/// Parses vtebench's `.dat` file: a line of benchmark names, then a line per
/// sample with a column per benchmark, holding the sample in whole
/// milliseconds, or `_` once that benchmark has no more samples. Each name
/// and value is followed by a space.
fn parse(text: &str) -> Result<Vec<Case>, String> {
    let mut lines = text.lines();
    let mut cases: Vec<Case> = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .map(|name| (name.to_owned(), Vec::new()))
        .collect();
    if cases.is_empty() {
        return Err("vtebench's .dat file names no benchmark".to_owned());
    }
    let mut ended = vec![false; cases.len()];
    for (index, line) in lines.enumerate() {
        let number = index + 2;
        let values: Vec<&str> = line.split_whitespace().collect();
        if values.len() != cases.len() {
            return Err(format!(
                "vtebench's .dat file, line {number}: {} values for {} benchmarks",
                values.len(),
                cases.len()
            ));
        }
        for (column, value) in values.into_iter().enumerate() {
            if value == "_" {
                ended[column] = true;
                continue;
            }
            let (name, samples) = &mut cases[column];
            let sample = value
                .parse()
                .ok()
                .filter(|_| !ended[column])
                .ok_or_else(|| {
                    format!("vtebench's .dat file, line {number}: cannot use {value:?} for {name}")
                })?;
            samples.push(sample);
        }
    }
    Ok(cases)
}

/// Returns every sample of `case` in `target`'s runs, summarised, with how
/// many there are.
fn pooled(runs: &[Run], case: &str, target: Target) -> Option<(usize, Summary)> {
    let samples: Vec<Duration> = runs
        .iter()
        .filter(|run| run.target == target)
        .flat_map(|run| run.cases.iter().filter(|(name, _)| name == case))
        .flat_map(|(_, samples)| samples.iter().copied().map(Duration::from_millis))
        .collect();
    (!samples.is_empty()).then(|| (samples.len(), stats::summarize(&samples)))
}

/// Returns the app's median rate in `case` as a multiple of Ghostty with
/// tmux's, or why it cannot be had. Both write the same bytes per sample, so
/// it is Ghostty with tmux's median time divided by the app's.
fn ratio(runs: &[Run], case: &str) -> Result<f64, &'static str> {
    let (Some((_, app)), Some((_, ghostty))) = (
        pooled(runs, case, Target::App),
        pooled(runs, case, Target::GhosttyTmux),
    ) else {
        return Err("the app and Ghostty with tmux were not both measured");
    };
    if app.median.is_zero() || ghostty.median.is_zero() {
        return Err("a median of 0 ms is below vtebench's resolution");
    }
    Ok(ghostty.median.as_nanos() as f64 / app.median.as_nanos() as f64)
}

/// How a vtebench case compares with the gate's limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The app's rate is at least the limit.
    Passes,
    /// The app's rate is below the limit.
    Fails,
    /// The case cannot be judged from these runs.
    Undecided,
}

/// Judges every case: the app's median rate must be at least `limit` times
/// Ghostty with tmux's. vtebench's times are not display times, so the
/// camera rule of the marker probe does not apply. Returns each case's
/// outcome with its line for the report.
pub fn judge(runs: &[Run], limit: f64) -> Vec<(Outcome, String)> {
    CASES
        .iter()
        .map(|&case| match ratio(runs, case) {
            Ok(ratio) => {
                let (outcome, word) = if ratio >= limit {
                    (Outcome::Passes, "passes")
                } else {
                    (Outcome::Fails, "fails")
                };
                let line = format!(
                    "vtebench {case}: the app's median rate is {ratio:.2} times Ghostty with \
                     tmux's (needs {limit}): {word}"
                );
                (outcome, line)
            }
            Err(why) => (
                Outcome::Undecided,
                format!("vtebench {case}: not decided, {why}"),
            ),
        })
        .collect()
}

/// Lists what the vtebench runs left out of the coverage a whole-gate result
/// needs: any run at all without `settings`, fewer runs, bytes or seconds
/// than the gate's, and every case missing from a run's output, as vtebench
/// leaves out a benchmark whose script printed nothing.
pub fn missing(settings: Option<&Settings>, runs: &[Run]) -> Vec<String> {
    let Some(settings) = settings else {
        return vec!["vtebench not run".to_owned()];
    };
    let mut missing = Vec::new();
    if settings.runs < REQUIRED_RUNS {
        missing.push(format!(
            "vtebench: {} run(s) per target, needs {REQUIRED_RUNS}",
            settings.runs
        ));
    }
    if settings.bytes < REQUIRED_BYTES {
        missing.push(format!(
            "vtebench: {} bytes per sample, needs {REQUIRED_BYTES}",
            settings.bytes
        ));
    }
    if settings.secs < REQUIRED_SECS {
        missing.push(format!(
            "vtebench: {} s per case, needs {REQUIRED_SECS}",
            settings.secs
        ));
    }
    for case in CASES {
        let without: Vec<String> = runs
            .iter()
            .filter(|run| {
                !run.cases
                    .iter()
                    .any(|(name, samples)| name == case && !samples.is_empty())
            })
            .map(|run| format!("{} run {}", run.target.name(), run.run))
            .collect();
        if !without.is_empty() {
            missing.push(format!(
                "vtebench {case} missing from {}",
                without.join(", ")
            ));
        }
    }
    missing
}

/// Returns the report's vtebench section: the settings, and for each case
/// each target's samples, pooled over its runs, and the app's rate against
/// Ghostty with tmux's.
pub fn section(settings: &Settings, targets: &[Target], runs: &[Run]) -> String {
    let mut out = format!(
        "\n## vtebench\n\n\
         vtebench at commit `{COMMIT}`, run {} time(s) per target with `--min-bytes {} \
         --max-secs {}`, each time on a new private server and in a new window. A sample is \
         how long one case's writes to the pane blocked, in whole milliseconds; each target's \
         samples are pooled over its runs. With Ghostty running tmux, tmux keeps reading the \
         pane however far Ghostty falls behind, so those times are mostly tmux's own \
         (docs/benchmarks/README.md). The ratio is Ghostty with tmux's median time divided by \
         the app's: the app's rate as a multiple of Ghostty with tmux's. These windows are not \
         captured, so the windows table shows no frames for them.\n\n| Case |",
        settings.runs, settings.bytes, settings.secs
    );
    for target in targets {
        let _ = write!(
            out,
            " {0} samples | {0} median ms | {0} p95 ms |",
            target.name()
        );
    }
    out.push_str(" Ratio |\n| --- |");
    for _ in targets {
        out.push_str(" --- | --- | --- |");
    }
    out.push_str(" --- |\n");
    for case in CASES {
        let _ = write!(out, "| {case} |");
        for &target in targets {
            let _ = match pooled(runs, case, target) {
                Some((count, s)) => {
                    write!(out, " {count} | {:.1} | {:.1} |", ms(s.median), ms(s.p95))
                }
                None => write!(out, " 0 | - | - |"),
            };
        }
        let ratio = ratio(runs, case).map_or("-".to_owned(), |ratio| format!("{ratio:.2}"));
        let _ = writeln!(out, " {ratio} |");
    }
    out
}

/// Returns `vtebench.tsv`: every sample of every run, one per line.
pub fn tsv(runs: &[Run]) -> String {
    let mut out = String::from("case\ttarget\trun\tsample\tmilliseconds\n");
    for run in runs {
        for (case, samples) in &run.cases {
            for (index, sample) in samples.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "{case}\t{}\t{}\t{}\t{sample}",
                    run.target.name(),
                    run.run,
                    index + 1
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    /// A run of `target` whose every case has `samples`.
    fn run(target: Target, run: usize, samples: &[u64]) -> Run {
        Run {
            target,
            run,
            cases: CASES
                .iter()
                .map(|case| ((*case).to_owned(), samples.to_vec()))
                .collect(),
            window: Window {
                target,
                origin: (40.0, 60.0),
                size: (1600.0, 850.0),
                refresh: None,
                display: Default::default(),
                build: None,
                stats: CaptureStats::default(),
            },
        }
    }

    #[test]
    fn the_dat_file_is_parsed_with_its_gaps() {
        assert_eq!(
            parse("dense_cells unicode \n31 40 \n29 _ \n30 _ \n"),
            Ok(vec![
                ("dense_cells".to_owned(), vec![31, 29, 30]),
                ("unicode".to_owned(), vec![40]),
            ])
        );
        assert_eq!(
            parse("scrolling \n"),
            Ok(vec![("scrolling".to_owned(), Vec::new())])
        );
    }

    #[test]
    fn a_malformed_dat_file_is_refused() {
        for text in [
            "",
            " \n1 \n",
            "a b \n1 \n",
            "a b \n1 2 3 \n",
            "a \n1.5 \n",
            "a \n-1 \n",
            "a b \n1 _ \n2 3 \n",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn each_case_is_judged_on_its_pooled_medians() {
        let both = |app: &[u64], ghostty: &[u64]| {
            judge(
                &[
                    run(Target::App, 1, app),
                    run(Target::GhosttyTmux, 1, ghostty),
                ],
                0.95,
            )
        };
        // 95 / 100 is exactly the limit.
        let judged = both(&[100, 100, 300], &[95, 95, 10]);
        assert_eq!(judged.len(), CASES.len());
        assert_eq!(judged[0].0, Outcome::Passes, "{judged:?}");
        assert!(
            judged[0]
                .1
                .ends_with("0.95 times Ghostty with tmux's (needs 0.95): passes")
        );
        assert_eq!(both(&[100], &[94])[0].0, Outcome::Fails);
        assert_eq!(both(&[0], &[94])[0].0, Outcome::Undecided);
        let alone = judge(&[run(Target::App, 1, &[100])], 0.95);
        assert!(alone[0].1.contains("not both measured"), "{alone:?}");
    }

    #[test]
    fn fewer_runs_bytes_seconds_or_cases_are_exploratory() {
        let full = Settings {
            dir: PathBuf::new(),
            runs: REQUIRED_RUNS,
            bytes: REQUIRED_BYTES,
            secs: REQUIRED_SECS,
        };
        let mut runs = vec![
            run(Target::App, 1, &[50]),
            run(Target::GhosttyTmux, 1, &[50]),
        ];
        assert!(missing(Some(&full), &runs).is_empty());
        assert_eq!(missing(None, &runs), vec!["vtebench not run".to_owned()]);
        runs[1].cases.retain(|(case, _)| case != "unicode");
        let short = Settings {
            runs: 1,
            bytes: 1_048_576,
            secs: 5,
            ..full
        };
        assert_eq!(
            missing(Some(&short), &runs),
            vec![
                "vtebench: 1 run(s) per target, needs 2".to_owned(),
                "vtebench: 1048576 bytes per sample, needs 8388608".to_owned(),
                "vtebench: 5 s per case, needs 10".to_owned(),
                "vtebench unicode missing from ghostty-tmux run 1".to_owned(),
            ]
        );
    }

    #[test]
    fn a_directory_that_is_not_a_checkout_is_refused() {
        let work = tempfile::tempdir().unwrap();
        let error = checkout(work.path()).unwrap_err();
        assert!(error.contains(COMMIT), "{error}");
    }

    #[test]
    fn the_script_runs_vtebench_and_leaves_its_samples() {
        let work = tempfile::tempdir().unwrap();
        // A stand-in for vtebench that notes its arguments in the directory
        // it runs in, the checkout, and writes a small `.dat` file to the
        // path after `--dat`.
        let fake = work.path().join("vtebench");
        fs::write(
            &fake,
            "#!/bin/sh\n\
             echo \"$@\" > arguments\n\
             while [ $# -gt 0 ]; do\n\
             if [ \"$1\" = --dat ]; then printf 'dense_cells unicode \\n31 40 \\n29 _ \\n' > \"$2\"; fi\n\
             shift\n\
             done\n",
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let script = work.path().join("vtebench.sh");
        // Replace the final sleep so the script ends.
        fs::write(&script, SCRIPT.replace("exec sleep 86400", "exit 0")).unwrap();
        let fifo = work.path().join("gate");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let dat = work.path().join("samples.dat");
        let run_script = || {
            let mut child = Command::new("sh")
                .arg(&script)
                .arg(&fifo)
                .arg(&fake)
                .arg(work.path())
                .arg(&dat)
                .args(["1024", "2"])
                .spawn()
                .unwrap();
            record::release(&fifo).unwrap();
            assert!(child.wait().unwrap().success());
        };
        run_script();
        let arguments = fs::read_to_string(work.path().join("arguments")).unwrap();
        assert_eq!(
            arguments.trim(),
            format!(
                "-b benchmarks --dat {}.part --silent --min-bytes 1024 --max-secs 2",
                dat.display()
            )
        );
        assert_eq!(
            results(&dat),
            Ok(vec![
                ("dense_cells".to_owned(), vec![31, 29]),
                ("unicode".to_owned(), vec![40]),
            ])
        );
        // A vtebench that fails is reported, not read.
        fs::write(&fake, "#!/bin/sh\nexit 3\n").unwrap();
        run_script();
        assert_eq!(
            results(&dat),
            Err("vtebench exited with status 3".to_owned())
        );
        // So is one that cannot write its samples, though it exits with 0.
        fs::write(
            &fake,
            "#!/bin/sh\necho 'Unable to write DAT file: disk full' >&2\n",
        )
        .unwrap();
        run_script();
        assert_eq!(
            results(&dat),
            Err("vtebench reported: Unable to write DAT file: disk full".to_owned())
        );
    }
}
