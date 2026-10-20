//! The milestone-0 benchmark window: the panes of one tmux window, drawn by
//! the app's terminal path. Benchmark scaffolding, not product UI
//! (`docs/decisions/0019-benchmark-window.md`).
//!
//! The benchmark harness starts it, attached to a session on a private tmux
//! server that the harness owns. Its font and colours come from the same file
//! the harness gives the Ghostty it compares with, which uses Ghostty's
//! configuration format; this window reads only the keys it needs.
//!
//! Once the window is open it prints one line, which the harness waits for:
//! `ready COLSxROWS cell WIDTHxHEIGHT`, then tab-separated fields, each a
//! name, a space and a value, saying how it was built: `revision` (the
//! revision, as the harness states its own), `profile` (Cargo's profile and
//! optimisation level, such as `release opt-level 3`), `cpu` (the CPU the terminal engine was built for),
//! `optimize` (the engine's optimisation mode, as the engine reports it) and
//! `engine` (the engine's build, described). The harness refuses a window
//! built from another revision or with another engine CPU target or
//! optimisation, or a debug window when it is a release build itself.

use std::io::Write;
use std::sync::Arc;

use gpui::{
    ActivationPolicy, App, AppContext, Bounds, Rgba, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_platform::application;
use tmux_workspaces_app::panes::Panes;
use tmux_workspaces_app::view::{self, Appearance, TerminalView};
use tmux_workspaces_test_support::ServerRef;
use tmux_workspaces_tmux::control::PaneId;

const USAGE: &str = "usage: bench-window --tmux-tmpdir DIR --tmux-socket NAME --session NAME \
--config FILE [--input-pane %N] [--scrollback LINES] [--origin X,Y]";

struct Options {
    tmpdir: String,
    socket: String,
    session: String,
    config: String,
    input: Option<PaneId>,
    scrollback: usize,
    /// Where the window's top left corner goes, in points from the top left
    /// of the main display; centred on it when not given.
    origin: Option<(f32, f32)>,
}

fn main() {
    if let Err(message) = run() {
        eprintln!("bench-window: {message}\n{USAGE}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse(std::env::args().skip(1))?;
    let appearance = appearance(
        &std::fs::read_to_string(&options.config)
            .map_err(|e| format!("reading {}: {e}", options.config))?,
    )?;
    let server =
        ServerRef::new(options.tmpdir.as_ref(), &options.socket).map_err(|e| e.to_string())?;
    let target = ["-t", options.session.as_str()];
    let window = server
        .run(
            &[
                &["display-message", "-p"][..],
                &target,
                &["#{window_width} #{window_height}"],
            ]
            .concat(),
        )
        .map_err(|e| e.to_string())?;
    let [cols, rows] = numbers(&window)?;
    let described = server
        .run(
            &[
                &["display-message", "-p"][..],
                &target,
                &["#{window_id} #{window_layout} #{window_visible_layout} #{window_raw_flags}"],
            ]
            .concat(),
        )
        .map_err(|e| e.to_string())?;
    let layout =
        tmux_workspaces_tmux::layout::parse(format!("%layout-change {described}").as_bytes())
            .map_err(str::to_owned)?
            .panes;
    let input = options
        .input
        .or_else(|| layout.first().map(|pane| pane.id))
        .ok_or("the session has no panes")?;

    let client = server
        .control(&[
            "attach-session",
            "-f",
            "no-output,ignore-size",
            "-t",
            &options.session,
        ])
        .map_err(|e| e.to_string())?;
    let (wake, woken) = futures::channel::mpsc::unbounded();
    let panes = Panes::start(client, &layout, options.scrollback, move || {
        let _ = wake.unbounded_send(());
    })
    .map_err(|e| e.to_string())?;
    let panes = Arc::new(panes);
    let origin = options.origin;

    application().run(move |cx: &mut App| {
        // An accessory app never activates itself, so its window opens on the
        // Space in front, even over full-screen windows, without changing
        // which app has focus. Keys reach it posted to this process.
        cx.set_activation_policy(ActivationPolicy::Accessory);
        let cell = view::cell_size(cx.text_system(), &appearance);
        let content = size(cell.width * f32::from(cols), cell.height * f32::from(rows));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(match origin {
                Some((x, y)) => Bounds::new(point(px(x), px(y)), content),
                None => Bounds::centered(None, content, cx),
            })),
            titlebar: Some(TitlebarOptions {
                title: Some("tmux-workspaces benchmark".into()),
                ..Default::default()
            }),
            // Output keeps arriving while another window has focus, so the
            // window draws at the display's rate whether or not it is active.
            inactive_frame_interval: None,
            is_resizable: false,
            ..Default::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| TerminalView::new(panes, layout, input, appearance, woken, window, cx))
        });
        if let Err(error) = opened {
            eprintln!("bench-window: cannot open the window: {error}");
            cx.quit();
            return;
        }
        // The harness waits for this line before it looks for the window.
        let engine = tmux_workspaces_term::engine_build();
        println!(
            "ready {}x{} cell {}x{}\trevision {}\tprofile {}\tcpu {}\toptimize {}\tengine {}",
            cols,
            rows,
            f32::from(cell.width),
            f32::from(cell.height),
            env!("BENCH_REVISION"),
            concat!(
                env!("BENCH_PROFILE"),
                " opt-level ",
                env!("BENCH_OPT_LEVEL")
            ),
            env!("BENCH_GHOSTTY_CPU"),
            engine
                .as_ref()
                .map_or("unknown", |engine| engine.optimize.as_str()),
            match &engine {
                Ok(engine) => format!(
                    "libghostty-vt {}, {}, SIMD {}",
                    engine.version, engine.optimize, engine.simd
                ),
                Err(error) => format!("unknown ({error})"),
            }
        );
        let _ = std::io::stdout().flush();
    });
    Ok(())
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        tmpdir: String::new(),
        socket: String::new(),
        session: String::new(),
        config: String::new(),
        input: None,
        scrollback: 10_000,
        origin: None,
    };
    while let Some(flag) = args.next() {
        let value = args.next().ok_or(format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--tmux-tmpdir" => options.tmpdir = value,
            "--tmux-socket" => options.socket = value,
            "--session" => options.session = value,
            "--config" => options.config = value,
            "--input-pane" => {
                let [id] = numbers(value.trim_start_matches('%'))?;
                options.input = Some(PaneId(u32::from(id)));
            }
            "--origin" => {
                let (x, y) = value.split_once(',').ok_or("--origin needs X,Y")?;
                let number =
                    |text: &str| text.trim().parse::<f32>().map_err(|_| "--origin needs X,Y");
                options.origin = Some((number(x)?, number(y)?));
            }
            "--scrollback" => {
                options.scrollback = value.parse().map_err(|_| "--scrollback needs a number")?;
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    if [
        &options.tmpdir,
        &options.socket,
        &options.session,
        &options.config,
    ]
    .iter()
    .any(|value| value.is_empty())
    {
        return Err("missing a required option".to_owned());
    }
    Ok(options)
}

/// Parses exactly `N` space-separated numbers.
fn numbers<const N: usize>(text: &str) -> Result<[u16; N], String> {
    let values: Vec<u16> = text
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| format!("expected {N} numbers, got {text:?}"))?;
    values
        .try_into()
        .map_err(|_| format!("expected {N} numbers, got {text:?}"))
}

/// Reads the font and colours from a file in Ghostty's configuration format:
/// `font-family`, `font-size`, `foreground`, `background`, `cursor-color` and
/// `palette = N=#rrggbb`. Every one of them must be set.
fn appearance(config: &str) -> Result<Appearance, String> {
    let mut font_family = None;
    let mut font_size = None;
    let (mut foreground, mut background, mut cursor) = (None, None, None);
    let mut palette = [None; 16];
    for line in config.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        match key.trim() {
            "font-family" => font_family = Some(SharedString::from(value.to_owned())),
            "font-size" => font_size = value.parse::<f32>().ok(),
            "foreground" => foreground = Some(hex(value)?),
            "background" => background = Some(hex(value)?),
            "cursor-color" => cursor = Some(hex(value)?),
            "palette" => {
                let (index, color) = value.split_once('=').ok_or("palette needs N=#rrggbb")?;
                let index: usize = index.trim().parse().map_err(|_| "bad palette index")?;
                if let Some(entry) = palette.get_mut(index) {
                    *entry = Some(hex(color)?);
                }
            }
            _ => {}
        }
    }
    let missing = |name: &str| format!("the configuration does not set {name}");
    let mut base = [Rgba::default(); 16];
    for (entry, value) in base.iter_mut().zip(palette) {
        *entry = value.ok_or_else(|| missing("all sixteen palette colours"))?;
    }
    Ok(Appearance {
        font_family: font_family.ok_or_else(|| missing("font-family"))?,
        font_size: font_size.ok_or_else(|| missing("font-size"))?,
        foreground: foreground.ok_or_else(|| missing("foreground"))?,
        background: background.ok_or_else(|| missing("background"))?,
        cursor: cursor.ok_or_else(|| missing("cursor-color"))?,
        palette: base,
    })
}

fn hex(value: &str) -> Result<Rgba, String> {
    let digits = value.trim().trim_start_matches('#');
    let value = u32::from_str_radix(digits, 16)
        .ok()
        .filter(|_| digits.len() == 6)
        .ok_or(format!("{value:?} is not a #rrggbb colour"))?;
    Ok(gpui::rgb(value))
}
