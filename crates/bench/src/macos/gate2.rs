//! Gate 2: end-to-end bulk output, the marker probe
//! (`docs/decisions/0018-milestone-0-benchmark-method-revised.md`).
//!
//! A producer in a tmux pane waits on a FIFO, prints a 20 MB payload from
//! gate 1's generator and then fills the screen with the marker colour. The
//! time runs from the harness releasing the producer to the display time of
//! the first frame the marker colour fills, in the app and in Ghostty running
//! an ordinary tmux client.
//!
//! vtebench, the gate's other measurement, is not run yet.

use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use super::clock;
use super::frames::{Frames, Region};
use super::pixels::{Color, fraction_matching};
use super::report::{self, Judged, Window, ms};
use super::targets::{self, CHECK_EVERY, Programs, Target};
use crate::options::Options;
use crate::payload::{self, Kind};
use crate::record;
use crate::stats::{self, Summary};

/// The marker colour: palette entry 13 of the shared appearance.
const MARKER: Color = Color {
    r: 255,
    g: 0,
    b: 255,
};
/// How far each channel of a captured pixel may be from the marker colour:
/// the capture converts colours to the display's colour space.
const TOLERANCE: u8 = 64;
/// The share of the window below its title bar the marker must fill.
const FILLED: f64 = 0.9;
/// How long one payload may take to show.
const TIMEOUT: Duration = Duration::from_secs(300);
/// The gate's limit: the app's rate against Ghostty with tmux.
const RATE_LIMIT: f64 = 0.95;
/// The coverage a whole-gate result needs: every payload, each at least this
/// many bytes.
const REQUIRED_BYTES: usize = 20_000_000;

/// The producer: waits on the FIFO, prints the payload, then fills the screen
/// with palette entry 13 (bright magenta) as its background.
const PRODUCER: &str = r#"read -r _ < "$1"
cat "$2"
printf '\033[0m\033[105m\033[2J\033[H'
exec sleep 86400
"#;

struct Settings {
    /// How this program's build differs from the distributable one; any
    /// difference keeps the gate undecided (0018).
    build_issues: Vec<String>,
    size: usize,
    seed: u64,
    payloads: Vec<Kind>,
    warmup: usize,
    runs: usize,
    targets: Vec<Target>,
    /// The refresh rate to use when the display does not report one.
    refresh: Option<f64>,
}

/// One timed run.
struct Measured {
    kind: Kind,
    target: Target,
    run: usize,
    elapsed: Duration,
    window: Window,
}

/// Runs gate 2's marker probe and writes `summary.md` and `raw.tsv` to
/// `--out`.
pub fn run(options: &Options) -> Result<(), String> {
    let out = Path::new(options.required("out")?);
    let settings = Settings {
        build_issues: report::build_issues(&targets::Build::own()),
        size: options.get("size", 20_000_000)?,
        seed: options.get("seed", 1)?,
        payloads: options
            .get("payloads", "ascii,unicode,color,scroll".to_owned())?
            .split(',')
            .map(|name| Kind::from_name(name).ok_or(format!("unknown payload {name}")))
            .collect::<Result<_, _>>()?,
        warmup: options.get("warmup", 1)?,
        runs: options.get("runs", 5)?,
        targets: Target::list(options, &[Target::App, Target::GhosttyTmux])?,
        refresh: report::refresh_option(options)?,
    };
    if settings.runs == 0 || settings.targets.is_empty() {
        return Err("--runs and --targets must not be empty".to_owned());
    }
    if settings.targets.contains(&Target::Ghostty) {
        return Err("gate 2 compares with Ghostty running tmux: use ghostty-tmux".to_owned());
    }
    let programs = Programs::from_options(options)?;
    let environment = report::environment(&programs);
    let work = tempfile::Builder::new()
        .prefix("tw-gate2-")
        .tempdir()
        .map_err(|error| error.to_string())?;
    let producer = work.path().join("producer.sh");
    fs::write(&producer, PRODUCER).map_err(|error| error.to_string())?;
    let mut results = Vec::new();
    for &kind in &settings.payloads {
        let path = work.path().join(format!("payload-{}", kind.name()));
        fs::write(&path, payload::generate(kind, settings.seed, settings.size))
            .map_err(|error| error.to_string())?;
        for run in 0..settings.warmup + settings.runs {
            for target in report::order(&settings.targets, run) {
                let (elapsed, window) =
                    measure(target, &programs, &producer, &path, work.path(), run)?;
                if run >= settings.warmup {
                    results.push(Measured {
                        kind,
                        target,
                        run: run - settings.warmup + 1,
                        elapsed,
                        window,
                    });
                }
            }
        }
    }
    let payload_bytes: Vec<(Kind, usize)> = settings
        .payloads
        .iter()
        .map(|&kind| {
            let bytes = fs::metadata(work.path().join(format!("payload-{}", kind.name())))
                .map_or(0, |meta| meta.len() as usize);
            (kind, bytes)
        })
        .collect();
    let summary = summary(&settings, &environment, &results, &payload_bytes);
    report::write(out, &summary, &raw(&results, &payload_bytes))
}

/// Runs one payload through `target` and returns the time until the marker
/// filled the window, with the window's description.
fn measure(
    target: Target,
    programs: &Programs,
    producer: &Path,
    payload: &Path,
    work: &Path,
    run: usize,
) -> Result<(Duration, Window), String> {
    let fifo = work.join(format!("gate-{}-{run}", target.name()));
    let _ = fs::remove_file(&fifo);
    let made = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .map_err(|error| error.to_string())?;
    if !made.success() {
        return Err("mkfifo failed".to_owned());
    }
    let command = format!(
        "exec sh '{}' '{}' '{}'",
        producer.display(),
        fifo.display(),
        payload.display()
    );
    let server = targets::session(&command).map_err(|error| error.to_string())?;
    let running = targets::start(target, programs, Some(&server), "", work)?;
    let frames = Frames::start(running.window().id, |image| {
        fraction_matching(image, Region::Content.rect(image), |pixel| {
            pixel.near(MARKER, TOLERANCE)
        })
    })?;
    thread::sleep(Duration::from_millis(300));
    running.check_visible()?;

    let released = clock::now();
    record::release(&fifo).map_err(|error| error.to_string())?;
    let deadline = Instant::now() + TIMEOUT;
    let mut next_check = Instant::now() + CHECK_EVERY;
    let shown = loop {
        // A window that stops being visible stops drawing, so it is checked
        // while the output runs too: the run is then void, not slow.
        if Instant::now() >= next_check {
            running.check_visible()?;
            next_check = Instant::now() + CHECK_EVERY;
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(format!(
                "{}: the marker did not fill the window within {} s",
                target.name(),
                TIMEOUT.as_secs()
            ));
        }
        let Some(seen) = frames.next(deadline.min(next_check) - now) else {
            continue;
        };
        if seen.time > released && seen.value >= FILLED {
            break seen.time;
        }
    };
    let after = running.check_visible()?;
    let (stats, _) = frames.stop()?;
    let window = Window::new(target, &running, stats, after);
    running.stop();
    drop(server);
    Ok((shown.since(released), window))
}

fn raw(results: &[Measured], payload_bytes: &[(Kind, usize)]) -> String {
    let mut out = String::from("payload\ttarget\trun\telapsed_ns\tmegabytes_per_second\n");
    for result in results {
        let _ = writeln!(
            out,
            "{}\t{}\t{}\t{}\t{:.1}",
            result.kind.name(),
            result.target.name(),
            result.run,
            result.elapsed.as_nanos(),
            stats::rate(bytes_of(payload_bytes, result.kind), result.elapsed)
        );
    }
    out
}

fn bytes_of(payload_bytes: &[(Kind, usize)], kind: Kind) -> usize {
    payload_bytes
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or(0, |(_, bytes)| *bytes)
}

fn times(results: &[Measured], kind: Kind, target: Target) -> Option<Summary> {
    let times: Vec<Duration> = results
        .iter()
        .filter(|result| result.kind == kind && result.target == target)
        .map(|result| result.elapsed)
        .collect();
    (!times.is_empty()).then(|| stats::summarize(&times))
}

fn summary(
    settings: &Settings,
    environment: &[(&str, String)],
    results: &[Measured],
    payload_bytes: &[(Kind, usize)],
) -> String {
    let verdict = verdict(settings, results, payload_bytes);
    let mut out = report::opening("Gate 2: end-to-end bulk output", &verdict, environment);
    let _ = write!(
        out,
        "\n## Settings\n\n\
         - Payloads from seed {}, at least {} bytes each; {} warm-up and {} timed run(s) per \
           target, each on a new private server and a new window.\n\
         - The time runs from releasing the producer to the display time of the first frame \
           at least {:.0}% filled with the marker colour, below the title bar.\n\
         - vtebench is not run yet.\n",
        settings.seed,
        settings.size,
        settings.warmup,
        settings.runs,
        FILLED * 100.0
    );
    for &kind in &settings.payloads {
        let bytes = bytes_of(payload_bytes, kind);
        let _ = write!(
            out,
            "\n## {}\n\nPayload {bytes} bytes.\n\n\
             | Target | Median MB/s | p95 MB/s (slow end) | Median ms | p95 ms | Fastest ms | Slowest ms |\n\
             | --- | --- | --- | --- | --- | --- | --- |\n",
            kind.name()
        );
        for &target in &settings.targets {
            let Some(s) = times(results, kind, target) else {
                continue;
            };
            let _ = writeln!(
                out,
                "| {} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} | {:.1} |",
                target.name(),
                stats::rate(bytes, s.median),
                stats::rate(bytes, s.p95),
                ms(s.median),
                ms(s.p95),
                ms(s.min),
                ms(s.max)
            );
        }
    }
    let windows: Vec<(String, Window)> = results
        .iter()
        .map(|result| {
            (
                format!("{} {}", result.kind.name(), result.run),
                result.window.clone(),
            )
        })
        .collect();
    out.push_str(&report::windows_table(&windows));
    out
}

fn verdict(
    settings: &Settings,
    results: &[Measured],
    payload_bytes: &[(Kind, usize)],
) -> (String, Vec<String>) {
    let mut reasons = Vec::new();
    let mut failed = false;
    for &kind in &settings.payloads {
        let (Some(app), Some(ghostty)) = (
            times(results, kind, Target::App),
            times(results, kind, Target::GhosttyTmux),
        ) else {
            reasons.push(format!(
                "{}: not decided, the app and Ghostty with tmux were not both measured",
                kind.name()
            ));
            continue;
        };
        let Some(refresh) = results
            .iter()
            .find(|result| result.kind == kind && result.target == Target::App)
            .and_then(|result| result.window.refresh)
            .or(settings.refresh)
        else {
            reasons.push(format!(
                "{}: not decided, the display reports no refresh rate: give --refresh HZ",
                kind.name()
            ));
            continue;
        };
        let bytes = bytes_of(payload_bytes, kind);
        let ratio = stats::rate(bytes, app.median) / stats::rate(bytes, ghostty.median);
        let later = app.median.saturating_sub(ghostty.median);
        let interval = Duration::from_secs_f64(1.0 / refresh);
        // Both limits as times: the rate's is Ghostty's time divided by the
        // ratio needed. A clear failure of either fails, whatever the other
        // shows.
        let judged = Judged::of(app.median, ghostty.median.div_f64(RATE_LIMIT), interval)
            .max(Judged::of(app.median, ghostty.median + interval, interval));
        failed |= judged == Judged::Fails;
        reasons.push(format!(
            "{}: the app's median rate is {:.2} times Ghostty with tmux's (needs {RATE_LIMIT}), \
             and its final frame {:.1} ms later (allowed {:.1} ms, one refresh interval): {}",
            kind.name(),
            ratio,
            ms(later),
            ms(interval),
            judged.describe(),
        ));
    }
    reasons.push("vtebench is not run yet, so the gate cannot pass on this report.".to_owned());
    reasons.push(
        "An acceptance result needs three independent runs of a clean build on a quiet \
         machine (decision record 0018); this report alone does not make one."
            .to_owned(),
    );
    // Without vtebench, the gate can fail here but never pass.
    (
        report::verdict(
            2,
            failed,
            true,
            &[
                missing(settings, payload_bytes),
                settings.build_issues.clone(),
            ]
            .concat(),
        ),
        reasons,
    )
}

/// Lists what the run left out of the coverage a whole-gate result needs.
fn missing(settings: &Settings, payload_bytes: &[(Kind, usize)]) -> Vec<String> {
    Kind::ALL
        .iter()
        .filter_map(|&kind| {
            if !settings.payloads.contains(&kind) {
                Some(format!("{} not run", kind.name()))
            } else {
                let bytes = bytes_of(payload_bytes, kind);
                (bytes < REQUIRED_BYTES)
                    .then(|| format!("{}: {bytes} bytes, needs {REQUIRED_BYTES}", kind.name()))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(payloads: &[Kind]) -> Settings {
        Settings {
            build_issues: Vec::new(),
            size: REQUIRED_BYTES,
            seed: 1,
            payloads: payloads.to_vec(),
            warmup: 1,
            runs: 1,
            targets: vec![Target::App, Target::GhosttyTmux],
            refresh: None,
        }
    }

    /// A run of `target` on the ASCII payload that took `elapsed` ms, on a
    /// display of `refresh` hertz.
    fn measured(target: Target, elapsed: u64, refresh: Option<f64>) -> Measured {
        Measured {
            kind: Kind::Ascii,
            target,
            run: 1,
            elapsed: Duration::from_millis(elapsed),
            window: Window {
                target,
                origin: (40.0, 60.0),
                size: (1600.0, 850.0),
                refresh,
                display: Default::default(),
                build: None,
                stats: Default::default(),
            },
        }
    }

    #[test]
    fn final_frames_within_one_refresh_of_a_limit_are_not_decided() {
        let bytes = [(Kind::Ascii, REQUIRED_BYTES)];
        let judge = |app: u64, refresh: Option<f64>, settings: &Settings| {
            let results = [
                measured(Target::App, app, refresh),
                measured(Target::GhosttyTmux, 1000, refresh),
            ];
            verdict(settings, &results, &bytes)
        };
        let ascii = settings(&[Kind::Ascii]);
        // At 60 Hz, the final frame may come 16.7 ms after Ghostty's, and
        // results within another 16.7 ms of that need a camera.
        let (_, reasons) = judge(1005, Some(60.0), &ascii);
        assert!(
            reasons[0].ends_with("needs camera confirmation (0018)"),
            "{reasons:?}"
        );
        let (verdict, reasons) = judge(980, Some(60.0), &ascii);
        assert!(reasons[0].ends_with(": passes"), "{reasons:?}");
        assert!(verdict.starts_with("Gate 2 not decided"), "{verdict}");
        // 1100 ms is over both limits, Ghostty's time / 0.95 (1052.6 ms)
        // and one refresh interval later (1016.7 ms).
        let (verdict, reasons) = judge(1100, Some(60.0), &ascii);
        assert!(reasons[0].ends_with(": fails"), "{reasons:?}");
        assert!(verdict.starts_with("Gate 2 failed"), "{verdict}");
        // Without a refresh rate, nothing is decided, unless one is given.
        let (verdict, reasons) = judge(1100, None, &ascii);
        assert!(reasons[0].contains("give --refresh HZ"), "{reasons:?}");
        assert!(verdict.starts_with("Gate 2 not decided"), "{verdict}");
        let given = Settings {
            refresh: Some(60.0),
            ..settings(&[Kind::Ascii])
        };
        assert!(judge(1100, None, &given).0.starts_with("Gate 2 failed"));
    }

    #[test]
    fn a_subset_of_payloads_is_exploratory() {
        let small = [(Kind::Ascii, 1_000_000)];
        assert_eq!(
            missing(&settings(&[Kind::Ascii]), &small),
            vec![
                "ascii: 1000000 bytes, needs 20000000".to_owned(),
                "unicode not run".to_owned(),
                "color not run".to_owned(),
                "scroll not run".to_owned(),
            ]
        );
        let full: Vec<(Kind, usize)> = Kind::ALL
            .iter()
            .map(|&kind| (kind, REQUIRED_BYTES))
            .collect();
        assert!(missing(&settings(&Kind::ALL), &full).is_empty());
    }

    #[test]
    fn the_producer_prints_the_payload_then_the_marker() {
        let work = tempfile::tempdir().unwrap();
        let producer = work.path().join("producer.sh");
        fs::write(&producer, PRODUCER).unwrap();
        let fifo = work.path().join("gate");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let payload = work.path().join("payload");
        fs::write(&payload, "text\n").unwrap();
        // Replace the final sleep so the script ends.
        let script = fs::read_to_string(&producer)
            .unwrap()
            .replace("exec sleep 86400", "exit 0");
        fs::write(&producer, script).unwrap();
        let child = Command::new("sh")
            .arg(&producer)
            .arg(&fifo)
            .arg(&payload)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        record::release(&fifo).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.stdout, b"text\n\x1b[0m\x1b[105m\x1b[2J\x1b[H");
    }
}
