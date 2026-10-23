//! Gate 1, headless headroom: how fast the app's path
//! from control-mode output to display data runs, against tmux's own rate.
//!
//! For each payload, the gate records tmux runs on fresh private servers,
//! then replays the last recording in one child process per engine, and
//! compares the engines' final screens with each other and with tmux's
//! capture of the pane.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use tmux_workspaces_test_support::TmuxServer;
use tmux_workspaces_tmux::control::PaneId;

use crate::compare;
use crate::dump::dump;
use crate::engine::GRID;
use crate::environment;
use crate::options::Options;
use crate::payload::{self, Kind};
use crate::record::{self, Recording};
use crate::stats::{self, Summary};

/// Every engine the gate can measure, the leading one first.
const ENGINES: [&str; 3] = ["libghostty-vt", "rio-vt", "alacritty_terminal"];
/// The engine the pass or fail decision is about.
const LEADING: &str = "libghostty-vt";
/// The rate the ASCII payload must reach, in MB/s.
const ASCII_TARGET: f64 = 200.0;
/// How many times tmux's own rate every payload must reach.
const HEADROOM: f64 = 5.0;

struct Settings {
    size: usize,
    seed: u64,
    payloads: Vec<Kind>,
    engines: Vec<String>,
    tmux_warmup: usize,
    tmux_runs: usize,
    engine_warmup: usize,
    engine_runs: usize,
    chunk: usize,
    frame_rate: u32,
}

struct EngineResult {
    name: String,
    times: Vec<Duration>,
    pane_bytes: usize,
    reads: usize,
    copied: usize,
    history: usize,
    /// Whether the child reported the revision it was built from.
    revision: bool,
    peak_rss: u64,
    screen: String,
}

struct PayloadResult {
    kind: Kind,
    payload_bytes: usize,
    pane_bytes: usize,
    wire_bytes: usize,
    tmux: Vec<Duration>,
    read_every: usize,
    tmux_screen: String,
    tmux_rows: Vec<String>,
    tmux_cursor: (u16, u16),
    tmux_history: usize,
    engines: Vec<EngineResult>,
}

/// Runs gate 1 and writes `summary.md` and `raw.tsv` to `--out`.
pub fn run(options: &Options) -> Result<(), String> {
    let out = Path::new(options.required("out")?);
    let settings = Settings {
        size: options.get("size", 20_000_000)?,
        seed: options.get("seed", 1)?,
        payloads: list(
            options,
            "payloads",
            &["ascii", "unicode", "color", "scroll"],
        )
        .iter()
        .map(|name| Kind::from_name(name).ok_or(format!("unknown payload {name}")))
        .collect::<Result<_, _>>()?,
        engines: list(options, "engines", &ENGINES),
        tmux_warmup: options.get("tmux-warmup", 1)?,
        tmux_runs: options.get("tmux-runs", 10)?,
        engine_warmup: options.get("engine-warmup", 2)?,
        engine_runs: options.get("engine-runs", 10)?,
        chunk: options.get("chunk", 64 * 1024)?,
        frame_rate: options.get("frame-rate", 120)?,
    };
    if let Some(name) = settings
        .engines
        .iter()
        .find(|name| !ENGINES.contains(&name.as_str()))
    {
        return Err(format!("unknown engine {name}"));
    }
    if settings.tmux_runs == 0 || settings.engine_runs == 0 || settings.frame_rate == 0 {
        return Err("--tmux-runs, --engine-runs and --frame-rate must be above zero".to_owned());
    }
    fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
    // The payloads, recordings and final screens go to `--keep` if it is
    // given, and to a temporary directory otherwise.
    let temporary;
    let work = match options.required("keep") {
        Ok(keep) => {
            fs::create_dir_all(keep).map_err(|error| format!("{keep}: {error}"))?;
            PathBuf::from(keep)
        }
        Err(_) => {
            temporary = tempfile::Builder::new()
                .prefix("tw-bench-")
                .tempdir()
                .map_err(|error| error.to_string())?;
            temporary.path().to_owned()
        }
    };

    let tmux_version = TmuxServer::start()
        .and_then(|server| server.run(&["display-message", "-p", "#{version}"]))
        .map(|version| version.trim().to_owned())
        .unwrap_or_else(|error| format!("unknown ({error})"));
    let environment = environment::describe(&tmux_version);
    let load_before = environment::load();
    let mut results = Vec::new();
    for &kind in &settings.payloads {
        results.push(measure(kind, &settings, &work)?);
    }
    let after = [
        ("load average after", environment::load()),
        ("other builds running after", environment::builds()),
    ];

    let summary = summary(&settings, &environment, &load_before, &after, &results);
    fs::write(out.join("summary.md"), &summary).map_err(|error| error.to_string())?;
    fs::write(out.join("raw.tsv"), raw(&results)).map_err(|error| error.to_string())?;
    print!("{summary}");
    Ok(())
}

/// Runs a replay child for the engine this program measures, as `run`
/// starts it.
pub fn replay_child(options: &Options) -> Result<(), String> {
    use crate::child;
    use crate::engines::Ghostty;
    use crate::engines::alacritty::Alacritty;

    match options.required("engine")? {
        "libghostty-vt" => child::run::<Ghostty>(options),
        "alacritty_terminal" => child::run::<Alacritty>(options),
        other => Err(format!("this program does not measure {other}")),
    }
}

fn list(options: &Options, name: &str, default: &[&str]) -> Vec<String> {
    match options.required(name) {
        Ok(value) => value.split(',').map(str::to_owned).collect(),
        Err(_) => default.iter().map(|&item| item.to_owned()).collect(),
    }
}

fn measure(kind: Kind, settings: &Settings, work: &Path) -> Result<PayloadResult, String> {
    let name = kind.name();
    let payload = payload::generate(kind, settings.seed, settings.size);
    if payload.contains(&b'~') {
        return Err(format!("{name}: the payload holds a marker character"));
    }
    let payload_path = work.join(format!("{name}.payload"));
    fs::write(&payload_path, &payload).map_err(|error| error.to_string())?;

    let mut times = Vec::new();
    let mut last: Option<Recording> = None;
    for run in 0..settings.tmux_warmup + settings.tmux_runs {
        eprintln!("{name}: tmux run {}", run + 1);
        let run_dir = work.join(format!("{name}-tmux-{run}"));
        fs::create_dir(&run_dir).map_err(|error| error.to_string())?;
        let recording = record::record(&payload_path, GRID, &run_dir)
            .map_err(|error| format!("{name}: tmux run {}: {error}", run + 1))?;
        let output = record::pane_output(&recording.wire, recording.pane)
            .map_err(|error| error.to_string())?;
        // Every run must deliver exactly the payload.
        record::check_output(&output, &payload)
            .map_err(|error| format!("{name}: tmux run {}: {error}", run + 1))?;
        if run >= settings.tmux_warmup {
            times.push(recording.elapsed);
        }
        last = Some(recording);
        let _ = fs::remove_dir_all(&run_dir);
    }
    let recording = last.expect("at least one tmux run");
    let pane_bytes = record::pane_output(&recording.wire, recording.pane)
        .map_err(|error| error.to_string())?
        .len();

    // A display refreshing `frame_rate` times a second, while tmux delivers
    // this payload at its median rate, reads the screen out once every this
    // many pane bytes.
    let tmux = stats::summarize(&times);
    let read_every = (stats::rate(payload.len(), tmux.median) * 1e6
        / f64::from(settings.frame_rate))
    .max(1.0) as usize;
    let wire_path = work.join(format!("{name}.wire"));
    fs::write(&wire_path, &recording.wire).map_err(|error| error.to_string())?;
    let tmux_screen = dump(&compare::screen_from_capture(&recording.capture, GRID));
    let tmux_rows = compare::capture_rows(&recording.capture);
    fs::write(work.join(format!("{name}.tmux.screen")), &tmux_screen)
        .map_err(|error| error.to_string())?;
    fs::write(work.join(format!("{name}.tmux.rows")), tmux_rows.join("\n"))
        .map_err(|error| error.to_string())?;

    let mut engines = Vec::new();
    for engine in &settings.engines {
        eprintln!("{name}: {engine}");
        let screen_path = work.join(format!("{name}.{engine}.screen"));
        engines.push(replay(
            engine,
            &wire_path,
            recording.pane,
            read_every,
            settings,
            &screen_path,
        )?);
    }
    Ok(PayloadResult {
        kind,
        payload_bytes: payload.len(),
        pane_bytes,
        wire_bytes: recording.wire.len(),
        tmux: times,
        read_every,
        tmux_screen,
        tmux_rows,
        tmux_cursor: recording.capture.cursor,
        tmux_history: recording.capture.history,
        engines,
    })
}

/// Runs one engine's replay child and reads its results.
fn replay(
    engine: &str,
    wire: &Path,
    pane: PaneId,
    read_every: usize,
    settings: &Settings,
    screen: &Path,
) -> Result<EngineResult, String> {
    let this = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command;
    if engine == "rio-vt" {
        let program = this.with_file_name(format!(
            "tmux-workspaces-bench-rio{}",
            std::env::consts::EXE_SUFFIX
        ));
        command = Command::new(&program);
        if !program.exists() {
            return Err(format!(
                "{} is missing: build it with the same profile, \
                 cargo build -p tmux-workspaces-bench-rio",
                program.display()
            ));
        }
    } else {
        command = Command::new(&this);
        command.args(["replay", "--engine", engine]);
    }
    let output = command
        .arg("--wire")
        .arg(wire)
        .args(["--pane", &pane.0.to_string()])
        .args(["--chunk", &settings.chunk.to_string()])
        .args(["--read-every", &read_every.to_string()])
        .args(["--warmup", &settings.engine_warmup.to_string()])
        .args(["--runs", &settings.engine_runs.to_string()])
        .arg("--screen")
        .arg(screen)
        .output()
        .map_err(|error| format!("{engine}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{engine}: the replay failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let mut result = EngineResult {
        name: engine.to_owned(),
        times: Vec::new(),
        pane_bytes: 0,
        reads: 0,
        copied: 0,
        history: 0,
        revision: false,
        peak_rss: 0,
        screen: fs::read_to_string(screen).map_err(|error| error.to_string())?,
    };
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        let number = || {
            value
                .parse::<u64>()
                .map_err(|_| format!("{engine}: unexpected line {line:?}"))
        };
        match key {
            "engine" => {}
            // A replay program built from other code would measure that code.
            "revision" if value != env!("BENCH_REVISION") => {
                return Err(format!(
                    "{engine}: built from {value}, but this program from {}; \
                     build both programs together",
                    env!("BENCH_REVISION")
                ));
            }
            "revision" => result.revision = true,
            "history" => result.history = number()? as usize,
            "run" => result.times.push(Duration::from_nanos(number()?)),
            "pane_bytes" => result.pane_bytes = number()? as usize,
            "reads" => result.reads = number()? as usize,
            "copied" => result.copied = number()? as usize,
            "peak_rss" => result.peak_rss = number()?,
            _ => return Err(format!("{engine}: unexpected line {line:?}")),
        }
    }
    if !result.revision {
        return Err(format!(
            "{engine}: the replay program does not say what it was built from; \
             build both programs together"
        ));
    }
    Ok(result)
}

fn raw(results: &[PayloadResult]) -> String {
    let mut out = String::from("payload\tpath\trun\telapsed_ns\tmegabytes_per_second\n");
    for result in results {
        let paths = std::iter::once(("tmux", &result.tmux)).chain(
            result
                .engines
                .iter()
                .map(|engine| (engine.name.as_str(), &engine.times)),
        );
        for (path, times) in paths {
            for (run, time) in times.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "{}\t{path}\t{}\t{}\t{:.1}",
                    result.kind.name(),
                    run + 1,
                    time.as_nanos(),
                    stats::rate(result.payload_bytes, *time)
                );
            }
        }
    }
    out
}

fn summary(
    settings: &Settings,
    environment: &[(&str, String)],
    load_before: &str,
    after: &[(&str, String)],
    results: &[PayloadResult],
) -> String {
    let mut out = String::from("# Gate 1: headless headroom\n\n");
    let verdict = verdict(results);
    let _ = writeln!(out, "**{}**\n", verdict.0);
    for line in &verdict.1 {
        let _ = writeln!(out, "- {line}");
    }

    out.push_str("\n## Environment\n\n| Item | Value |\n| --- | --- |\n");
    for (name, value) in environment
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .chain([("load average before", load_before)])
        .chain(after.iter().map(|(name, value)| (*name, value.as_str())))
    {
        let _ = writeln!(out, "| {name} | {value} |");
    }

    let _ = write!(
        out,
        "\n## Settings\n\n\
         - Grid {}×{} and a history limit of {} lines, for tmux and every engine.\n\
         - Payloads from seed {}, at least {} bytes each.\n\
         - tmux: {} warm-up and {} timed runs per payload, each on a new private server.\n\
         - Engines: {} warm-up and {} timed replays each, in a child process per engine and payload.\n\
         - Replay: {} control-wire bytes per decode call; a read-out of the whole screen \
           every N pane bytes, where N is what a {} Hz display would see at tmux's median \
           rate, and a forced read-out after the last chunk.\n\
         - Rates divide the payload's bytes, as generated, by the time.\n",
        GRID.cols,
        GRID.rows,
        GRID.scrollback,
        settings.seed,
        settings.size,
        settings.tmux_warmup,
        settings.tmux_runs,
        settings.engine_warmup,
        settings.engine_runs,
        settings.chunk,
        settings.frame_rate,
    );

    for result in results {
        let tmux = stats::summarize(&result.tmux);
        let tmux_rate = stats::rate(result.payload_bytes, tmux.median);
        let _ = write!(
            out,
            "\n## {}\n\nPayload {} bytes; pane output {} bytes; control wire {} bytes. \
             A read-out every {} pane bytes.\n\n\
             | Path | Median MB/s | p95 MB/s (slow end) | Fastest MB/s | Slowest MB/s | p95 × tmux median | Read-outs | Copied per replay | History lines kept | Peak memory |\n\
             | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |\n",
            result.kind.name(),
            result.payload_bytes,
            result.pane_bytes,
            result.wire_bytes,
            result.read_every,
        );
        rate_row(
            &mut out,
            "tmux, drained control-mode client",
            result.payload_bytes,
            &tmux,
            tmux_rate,
            Err(result.tmux_history),
        );
        for engine in &result.engines {
            let times = stats::summarize(&engine.times);
            rate_row(
                &mut out,
                &engine.name,
                result.payload_bytes,
                &times,
                tmux_rate,
                Ok(engine),
            );
        }

        out.push_str(
            "\n| Final screen | Against libghostty-vt | Text and cursor against tmux | \
             Cells against tmux's capture, as libghostty-vt reads it |\n| --- | --- | --- | --- |\n",
        );
        let leading = result.engines.iter().find(|engine| engine.name == LEADING);
        let mut examples = String::new();
        for engine in &result.engines {
            let against_leading = match leading {
                Some(leading) if leading.name != engine.name => describe(
                    &compare::compare(&leading.screen, &engine.screen),
                    &format!("{} against {}", engine.name, LEADING),
                    &mut examples,
                ),
                _ => "-".to_owned(),
            };
            let rows =
                compare::differing_rows(&result.tmux_rows, &compare::dump_rows(&engine.screen));
            let cursor = compare::dump_cursor(&engine.screen) == Some(result.tmux_cursor);
            let text_against_tmux = match (rows, cursor) {
                (0, true) => "same".to_owned(),
                (0, false) => "same rows; cursor differs".to_owned(),
                (rows, true) => format!("{rows} rows differ"),
                (rows, false) => format!("{rows} rows differ; cursor differs"),
            };
            let cells_against_tmux = describe(
                &compare::compare(&result.tmux_screen, &engine.screen),
                &format!("{} against tmux's capture (tmux first)", engine.name),
                &mut examples,
            );
            let _ = writeln!(
                out,
                "| {} | {against_leading} | {text_against_tmux} | {cells_against_tmux} |",
                engine.name
            );
        }
        if !examples.is_empty() {
            let _ = write!(out, "\nFirst differences:\n\n```text\n{examples}```\n");
        }
    }
    out
}

fn rate_row(
    out: &mut String,
    name: &str,
    bytes: usize,
    times: &Summary,
    tmux_rate: f64,
    engine: Result<&EngineResult, usize>,
) {
    let _ = write!(
        out,
        "| {name} | {:.1} | {:.1} | {:.1} | {:.1} | ",
        stats::rate(bytes, times.median),
        stats::rate(bytes, times.p95),
        stats::rate(bytes, times.min),
        stats::rate(bytes, times.max),
    );
    match engine {
        Ok(engine) => {
            let _ = writeln!(
                out,
                "{:.1} | {} | {:.1} MB | {} | {:.1} MB |",
                stats::rate(bytes, times.p95) / tmux_rate,
                engine.reads,
                engine.copied as f64 / 1e6,
                engine.history,
                engine.peak_rss as f64 / 1e6,
            );
        }
        Err(tmux_history) => {
            let _ = writeln!(out, "- | - | - | {tmux_history} | - |");
        }
    }
}

fn describe(difference: &compare::Difference, label: &str, examples: &mut String) -> String {
    if difference.is_none() {
        return "same".to_owned();
    }
    for (a, b) in &difference.examples {
        let _ = writeln!(examples, "{label}:\n  {a}\n  {b}");
    }
    let mut parts = Vec::new();
    if difference.cells > 0 {
        parts.push(format!(
            "{} cells in {} rows from row {}; text differs in {} rows",
            difference.cells,
            difference.rows,
            difference.first_row.unwrap_or_default(),
            difference.text_rows
        ));
    }
    if difference.header {
        parts.push("cursor or size differs".to_owned());
    }
    parts.join(", ")
}

/// The gate's decision on the leading engine, and the reasons. Its timing
/// on a payload counts only if its final screen matches every other
/// engine's, before comparing processing rates.
fn verdict(results: &[PayloadResult]) -> (&'static str, Vec<String>) {
    let mut reasons = Vec::new();
    let mut pass = true;
    let mut ascii_measured = false;
    for result in results {
        let name = result.kind.name();
        let Some(engine) = result.engines.iter().find(|engine| engine.name == LEADING) else {
            continue;
        };
        let others = result.engines.iter().filter(|other| other.name != LEADING);
        let mut disqualified = Vec::new();
        for other in others {
            if !compare::compare(&engine.screen, &other.screen).is_none() {
                disqualified.push(format!("its final screen differs from {}'s", other.name));
            }
        }
        if disqualified.is_empty() {
            reasons.push(format!("{name}: {LEADING}'s timing counts"));
        } else {
            pass = false;
            reasons.push(format!(
                "{name}: {LEADING}'s timing does not count: {}",
                disqualified.join("; ")
            ));
        }

        let tmux_rate = stats::rate(result.payload_bytes, stats::summarize(&result.tmux).median);
        let rate = stats::rate(result.payload_bytes, stats::summarize(&engine.times).p95);
        let ratio = rate / tmux_rate;
        let headroom_ok = ratio >= HEADROOM;
        pass &= headroom_ok;
        reasons.push(format!(
            "{name}: {LEADING} {rate:.1} MB/s at its slow end, {ratio:.1} times tmux's median {tmux_rate:.1} MB/s (needs {HEADROOM}): {}",
            if headroom_ok { "met" } else { "not met" }
        ));
        if result.kind == Kind::Ascii {
            ascii_measured = true;
            let ascii_ok = rate >= ASCII_TARGET;
            pass &= ascii_ok;
            reasons.push(format!(
                "ascii: {LEADING} {rate:.1} MB/s at its slow end (needs {ASCII_TARGET}): {}",
                if ascii_ok { "met" } else { "not met" }
            ));
        }
    }
    if !ascii_measured {
        pass = false;
        reasons.push(format!("ascii was not measured with {LEADING}"));
    }
    (if pass { "Gate 1: PASS" } else { "Gate 1: FAIL" }, reasons)
}
