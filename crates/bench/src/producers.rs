//! Output that the display gates run inside the measured terminal
//! (`docs/decisions/0018-milestone-0-benchmark-method-revised.md`): a fire
//! animation and a scroll test, each written to standard output as fast as
//! the terminal accepts it, for a fixed time.
//!
//! Both are seeded, so every run draws the same frames in the same order.
//! With `--ready FILE`, each creates FILE once its first output has been
//! written, so a harness can tell the producer is really running.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::options::Options;
use crate::payload::{self, Kind, Rng};

/// The heat values of the fire, from cold to hottest.
const HEAT_LEVELS: usize = 37;

/// The 256-colour palette entry for each heat value: black through dark red,
/// red, orange and yellow to white.
const FIRE_COLORS: [u8; HEAT_LEVELS] = [
    16, 52, 52, 88, 88, 124, 124, 160, 160, 196, 196, 202, 202, 202, 208, 208, 208, 214, 214, 214,
    220, 220, 220, 226, 226, 226, 227, 227, 228, 228, 229, 229, 230, 230, 231, 231, 231,
];

/// `fire`: draws a fire animation that fills `--cols` by `--rows` cells, the
/// terminal's size by default, redrawing every cell each frame, for
/// `--seconds`, and creates `--ready` after the first frame.
pub fn fire(options: &Options) -> Result<(), String> {
    let (default_cols, default_rows) = terminal_size().unwrap_or((200, 50));
    let cols: usize = options.get("cols", default_cols)?;
    let rows: usize = options.get("rows", default_rows)?;
    let seconds: f64 = options.get("seconds", 30.0)?;
    let mut rng = Rng(options.get("seed", 1)?);
    let mut ready = Ready::from_options(options);
    let mut fire = Fire::new(cols, rows);
    let mut frame = Vec::with_capacity(cols * rows * 12);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    // Hide the cursor, so only the fire changes on screen.
    out.write_all(b"\x1b[?25l\x1b[2J")
        .map_err(|error| error.to_string())?;
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < end {
        fire.step(&mut rng);
        fire.draw(&mut frame);
        out.write_all(&frame).map_err(|error| error.to_string())?;
        out.flush().map_err(|error| error.to_string())?;
        ready.written(&mut out)?;
    }
    out.write_all(b"\x1b[0m\x1b[?25h")
        .map_err(|error| error.to_string())
}

/// `scroll-test`: prints the `ascii` payload's log lines over and over for
/// `--seconds`, so the screen scrolls continuously, and creates `--ready`
/// after the first chunk.
pub fn scroll_test(options: &Options) -> Result<(), String> {
    let seconds: f64 = options.get("seconds", 30.0)?;
    let lines = payload::generate(Kind::Ascii, options.get("seed", 1)?, 1 << 20);
    let end = Instant::now() + Duration::from_secs_f64(seconds);
    scroll(
        &mut io::stdout().lock(),
        &lines,
        end,
        Ready::from_options(options),
    )
}

/// Writes `lines` to `out` over and over until `end`, creating `ready`'s file
/// after the first chunk is written.
fn scroll(
    out: &mut impl Write,
    lines: &[u8],
    end: Instant,
    mut ready: Ready,
) -> Result<(), String> {
    while Instant::now() < end {
        // A chunk at a time, so the time limit holds within a few
        // milliseconds.
        for chunk in lines.chunks(16 * 1024) {
            out.write_all(chunk).map_err(|error| error.to_string())?;
            ready.written(out)?;
            if Instant::now() >= end {
                break;
            }
        }
    }
    out.flush().map_err(|error| error.to_string())
}

/// The file a producer creates once its first output is written: `--ready`.
struct Ready(Option<PathBuf>);

impl Ready {
    fn from_options(options: &Options) -> Self {
        Self(options.required("ready").ok().map(PathBuf::from))
    }

    /// Notes that output was written to `out`: the first time, flushes it and
    /// then creates the file.
    fn written(&mut self, out: &mut impl Write) -> Result<(), String> {
        if let Some(path) = self.0.take() {
            out.flush().map_err(|error| error.to_string())?;
            create(&path)?;
        }
        Ok(())
    }
}

/// Creates an empty file at `path`.
fn create(path: &Path) -> Result<(), String> {
    fs::write(path, b"").map_err(|error| format!("{}: {error}", path.display()))
}

/// Returns the columns and rows of the terminal on standard input, as `stty`
/// reports them.
fn terminal_size() -> Option<(usize, usize)> {
    let output = Command::new("stty")
        .arg("size")
        .stdin(Stdio::inherit())
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let mut numbers = text.split_whitespace().map(str::parse);
    let rows = numbers.next()?.ok()?;
    let cols = numbers.next()?.ok()?;
    Some((cols, rows))
}

/// The classic fire effect: heat rises from a hot bottom row, drifting and
/// cooling at random.
struct Fire {
    cols: usize,
    rows: usize,
    /// Heat from 0 to `HEAT_LEVELS - 1`, row by row from the top.
    heat: Vec<u8>,
}

impl Fire {
    fn new(cols: usize, rows: usize) -> Self {
        let mut heat = vec![0; cols * rows];
        if let Some(bottom) = heat.chunks_mut(cols).last() {
            bottom.fill((HEAT_LEVELS - 1) as u8);
        }
        Self { cols, rows, heat }
    }

    /// Moves the fire on by one frame.
    fn step(&mut self, rng: &mut Rng) {
        for row in 1..self.rows {
            for col in 0..self.cols {
                let random = rng.below(4);
                let below = self.heat[row * self.cols + col];
                let cooled = below.saturating_sub((random & 1) as u8);
                let to = (col + self.cols + 1 - random) % self.cols;
                self.heat[(row - 1) * self.cols + to] = cooled;
            }
        }
    }

    /// Writes the whole frame into `out`, replacing what it held: every cell
    /// as a space with its heat's background colour.
    fn draw(&self, out: &mut Vec<u8>) {
        out.clear();
        out.extend_from_slice(b"\x1b[H");
        for (index, row) in self.heat.chunks(self.cols).enumerate() {
            let mut current = None;
            for &heat in row {
                let color = FIRE_COLORS[usize::from(heat)];
                if current != Some(color) {
                    let _ = write!(out, "\x1b[48;5;{color}m");
                    current = Some(color);
                }
                out.push(b' ');
            }
            if index + 1 < self.rows {
                out.extend_from_slice(b"\r\n");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fire_rises_from_a_hot_bottom_row() {
        let mut fire = Fire::new(20, 10);
        let mut rng = Rng(1);
        for _ in 0..20 {
            fire.step(&mut rng);
        }
        let row_heat = |row: usize| -> u32 {
            fire.heat[row * 20..(row + 1) * 20]
                .iter()
                .map(|&heat| u32::from(heat))
                .sum()
        };
        assert_eq!(row_heat(9), 20 * 36);
        assert!(row_heat(8) > row_heat(1));
    }

    #[test]
    fn a_frame_draws_every_cell_once() {
        let fire = Fire::new(7, 3);
        let mut frame = Vec::new();
        fire.draw(&mut frame);
        assert!(frame.starts_with(b"\x1b[H"));
        assert_eq!(frame.iter().filter(|&&byte| byte == b' ').count(), 21);
        assert_eq!(frame.windows(2).filter(|pair| pair == b"\r\n").count(), 2);
    }

    /// A writer that fails every write.
    struct Broken;

    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("broken pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_ready_file_appears_only_after_output_was_written() {
        let work = tempfile::tempdir().unwrap();
        let lines = b"line\n".repeat(10);
        let soon = || Instant::now() + Duration::from_millis(20);

        let ready = work.path().join("written");
        let mut out = Vec::new();
        scroll(&mut out, &lines, soon(), Ready(Some(ready.clone()))).unwrap();
        assert!(out.starts_with(b"line\n"));
        assert!(ready.exists());

        let never = work.path().join("never");
        assert!(scroll(&mut Broken, &lines, soon(), Ready(Some(never.clone()))).is_err());
        assert!(!never.exists());
    }

    #[test]
    fn the_same_seed_draws_the_same_frames() {
        let frames = |seed| {
            let mut fire = Fire::new(30, 8);
            let mut rng = Rng(seed);
            let mut frame = Vec::new();
            for _ in 0..5 {
                fire.step(&mut rng);
            }
            fire.draw(&mut frame);
            frame
        };
        assert_eq!(frames(3), frames(3));
        assert_ne!(frames(3), frames(4));
    }
}
