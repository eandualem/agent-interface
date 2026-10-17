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
use super::report::{self, Window, ms};
use super::targets::{self, Programs, Target};
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

/// The producer: waits on the FIFO, prints the payload, then fills the screen
/// with palette entry 13 (bright magenta) as its background.
const PRODUCER: &str = r#"read -r _ < "$1"
cat "$2"
printf '\033[0m\033[105m\033[2J\033[H'
exec sleep 86400
"#;

struct Settings {
    size: usize,
    seed: u64,
    payloads: Vec<Kind>,
    warmup: usize,
    runs: usize,
    targets: Vec<Target>,
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
    let shown = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let Some(seen) = frames.next(left) else {
            return Err(format!(
                "{}: the marker did not fill the window within {} s",
                target.name(),
                TIMEOUT.as_secs()
            ));
        };
        if seen.time > released && seen.value >= FILLED {
            break seen.time;
        }
    };
    running.check_visible()?;
    let stats = frames.stop()?;
    let window = Window::new(target, running.window(), stats);
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
) -> (&'static str, Vec<String>) {
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
        let refresh = results
            .iter()
            .find(|result| result.kind == kind && result.target == Target::App)
            .and_then(|result| result.window.refresh)
            .unwrap_or(60.0);
        let bytes = bytes_of(payload_bytes, kind);
        let ratio = stats::rate(bytes, app.median) / stats::rate(bytes, ghostty.median);
        let later = app.median.saturating_sub(ghostty.median);
        let interval = Duration::from_secs_f64(1.0 / refresh);
        let passes = ratio >= RATE_LIMIT && later <= interval;
        failed |= !passes;
        reasons.push(format!(
            "{}: the app's median rate is {:.2} times Ghostty with tmux's (needs {RATE_LIMIT}), \
             and its final frame {:.1} ms later (allowed {:.1} ms): {}",
            kind.name(),
            ratio,
            ms(later),
            ms(interval),
            if passes { "passes" } else { "fails" },
        ));
    }
    reasons.push("vtebench is not run yet, so the gate cannot pass on this report.".to_owned());
    reasons.push(
        "An acceptance result needs three independent runs of a clean build on a quiet \
         machine (decision record 0018); this report alone does not make one."
            .to_owned(),
    );
    // Without vtebench, the gate can fail here but never pass.
    let verdict = if failed {
        "Gate 2 failed"
    } else {
        "Gate 2 not decided"
    };
    (verdict, reasons)
}

#[cfg(test)]
mod tests {
    use super::*;

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
