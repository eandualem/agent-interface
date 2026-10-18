//! `probe-display`: a manual check that a process's window can be found and
//! captured, how regularly its frames arrive and, optionally, how long typed
//! text takes to appear.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::capture::{CaptureOptions, WindowCapture};
use super::clock::HostTime;
use super::keys::{self, Modifiers, keycode};
use super::pixels::{Color, Rect, fraction_matching, region_digest};
use super::windows::windows_of;
use crate::options::Options;
use crate::stats::summarize;

/// How far each channel may be from `--color` and still count.
const COLOR_TOLERANCE: u8 = 16;

/// One complete frame, as the probe records it.
struct Seen {
    display_time: HostTime,
    digest: u64,
    /// The fraction of the frame near `--color`, if one was given.
    color: Option<f64>,
}

/// Lists the windows of `--pid`, then captures one of them for `--seconds`
/// and prints how many frames arrived and the intervals between them.
///
/// The window is `--window`, which must belong to `--pid`, or else the
/// process's first ordinary window that is on screen. `--type TEXT` types
/// TEXT and Return into `--pid`, and only `--pid`, one second into the
/// capture, if the window is then frontmost, and reports when the window
/// first changed after that.
/// `--color RRGGBB` reports the largest share of a frame showing that colour.
pub fn run(options: &Options) -> Result<(), String> {
    let pid: i32 = options
        .required("pid")?
        .parse()
        .map_err(|_| "--pid must be a process ID".to_owned())?;
    let seconds: f64 = options.get("seconds", 5.0)?;
    let chosen: u32 = options.get("window", 0)?;
    let text: String = options.get("type", String::new())?;
    let color = match options.get("color", String::new())?.as_str() {
        "" => None,
        hex => Some(parse_color(hex)?),
    };

    let windows = windows_of(pid).map_err(|error| error.to_string())?;
    println!("windows of process {pid}:");
    for window in &windows {
        println!(
            "  {} layer {} on screen {} at {},{} size {}x{} {:?}",
            window.id,
            window.layer,
            window.on_screen,
            window.bounds.x,
            window.bounds.y,
            window.bounds.width,
            window.bounds.height,
            window.title.as_deref().unwrap_or(""),
        );
    }
    let window = if chosen != 0 {
        windows
            .iter()
            .find(|window| window.id == chosen)
            .ok_or_else(|| format!("window {chosen} does not belong to process {pid}"))?
    } else {
        match windows
            .iter()
            .find(|window| window.layer == 0 && window.on_screen)
        {
            Some(window) => window,
            None => {
                println!("no ordinary window of process {pid} is on screen: nothing captured");
                return Ok(());
            }
        }
    };

    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let recorded = Arc::clone(&seen);
    let capture = WindowCapture::start(window.id, CaptureOptions::default(), move |frame| {
        let image = &frame.image;
        let whole = Rect {
            x: 0,
            y: 0,
            width: image.width,
            height: image.height,
        };
        let seen = Seen {
            display_time: frame.display_time,
            digest: region_digest(image, whole),
            color: color.map(|target| {
                fraction_matching(image, whole, |pixel| pixel.near(target, COLOR_TOLERANCE))
            }),
        };
        if let Ok(mut recorded) = recorded.lock() {
            recorded.push(seen);
        }
    })
    .map_err(|error| error.to_string())?;
    let mut typed = None;
    if !text.is_empty() {
        std::thread::sleep(Duration::from_secs(1));
        typed = Some(keys::post_text(pid, window, &text).map_err(|error| error.to_string())?);
        keys::post_key(pid, window, keycode::RETURN, Modifiers::default())
            .map_err(|error| error.to_string())?;
    }
    std::thread::sleep(Duration::from_secs_f64(seconds));
    let stats = capture.stop().map_err(|error| error.to_string())?;

    println!(
        "window {}: {} samples, {} complete, {} idle, {} other",
        window.id, stats.delivered, stats.complete, stats.idle, stats.other
    );
    let seen = seen.lock().map_err(|_| "a frame callback panicked")?;
    let changes = seen
        .windows(2)
        .filter(|pair| pair[0].digest != pair[1].digest)
        .count();
    println!("complete frames whose pixels changed: {changes}");
    let intervals: Vec<Duration> = seen
        .windows(2)
        .map(|pair| pair[1].display_time.since(pair[0].display_time))
        .collect();
    if intervals.is_empty() {
        println!("fewer than two complete frames: no intervals");
    } else {
        let summary = summarize(&intervals);
        println!(
            "intervals between complete frames: median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
            ms(summary.median),
            ms(summary.p95),
            ms(summary.max),
        );
    }
    if let Some(typed) = typed {
        println!("typed at host time {} ns", typed.nanos());
        match first_change_after(&seen, typed) {
            Some(shown) => println!(
                "first changed frame after typing: {:.2} ms later",
                ms(shown.since(typed))
            ),
            None => println!("no frame changed after typing"),
        }
    }
    if color.is_some() {
        let most = seen
            .iter()
            .filter_map(|frame| frame.color)
            .fold(0.0, f64::max);
        println!("largest share of a frame near the colour: {most:.3}");
    }
    Ok(())
}

/// Returns when the first frame displayed after `time` differed from the
/// frame before it.
fn first_change_after(seen: &[Seen], time: HostTime) -> Option<HostTime> {
    let first_after = seen.iter().position(|frame| frame.display_time > time)?;
    let before = first_after.checked_sub(1).map(|index| seen[index].digest);
    let mut previous = before;
    for frame in &seen[first_after..] {
        if previous.is_some_and(|digest| digest != frame.digest) {
            return Some(frame.display_time);
        }
        previous = Some(frame.digest);
    }
    None
}

fn parse_color(hex: &str) -> Result<Color, String> {
    let channel = |at: usize| {
        hex.get(at..at + 2)
            .and_then(|digits| u8::from_str_radix(digits, 16).ok())
    };
    match (hex.len(), channel(0), channel(2), channel(4)) {
        (6, Some(r), Some(g), Some(b)) => Ok(Color { r, g, b }),
        _ => Err(format!("--color: {hex:?} is not RRGGBB")),
    }
}

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1e3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(nanos: u64, digest: u64) -> Seen {
        Seen {
            display_time: HostTime::from_nanos(nanos),
            digest,
            color: None,
        }
    }

    #[test]
    fn finds_the_first_change_after_typing() {
        let frames = [
            seen(10, 1),
            seen(20, 1),
            seen(30, 1),
            seen(40, 2),
            seen(50, 3),
        ];
        let at = |nanos| HostTime::from_nanos(nanos);
        assert_eq!(first_change_after(&frames, at(25)), Some(at(40)));
        assert_eq!(first_change_after(&frames, at(35)), Some(at(40)));
        // The first frame after typing already differs from the one before.
        assert_eq!(first_change_after(&frames, at(45)), Some(at(50)));
        assert_eq!(first_change_after(&frames, at(55)), None);
        // With no frame before typing, the first frame is the baseline.
        assert_eq!(first_change_after(&frames, at(5)), Some(at(40)));
    }

    #[test]
    fn parses_colours() {
        assert_eq!(
            parse_color("ff00a0"),
            Ok(Color {
                r: 255,
                g: 0,
                b: 160
            })
        );
        assert!(parse_color("ff00").is_err());
        assert!(parse_color("gg0000").is_err());
    }
}
