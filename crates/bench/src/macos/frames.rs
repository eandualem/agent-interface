//! Watching a window's frames for a gate: each complete frame is reduced, on
//! the capture thread, to the one value the gate needs, such as a digest of a
//! region, and handed over with its display time.

use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use super::capture::{CaptureOptions, CaptureStats, WindowCapture};
use super::clock::HostTime;
use super::pixels::{Bgra, Rect};

/// One complete frame, reduced to a gate's value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen<T> {
    /// When the window server displayed the frame.
    pub time: HostTime,
    /// The gate's value for the frame.
    pub value: T,
}

/// A running capture that hands over every complete frame's value in order.
pub struct Frames<T> {
    capture: WindowCapture,
    received: Receiver<Seen<T>>,
}

impl<T: Send + 'static> Frames<T> {
    /// Starts capturing window `window`, reducing each complete frame with
    /// `reduce`, which runs on the capture thread and should be quick.
    pub fn start(
        window: u32,
        reduce: impl Fn(&Bgra<'_>) -> T + Send + 'static,
    ) -> Result<Self, String> {
        let (seen, received) = mpsc::channel();
        let capture = WindowCapture::start(window, CaptureOptions::default(), move |frame| {
            let _ = seen.send(Seen {
                time: frame.display_time,
                value: reduce(&frame.image),
            });
        })
        .map_err(|error| error.to_string())?;
        Ok(Self { capture, received })
    }

    /// Returns the next frame, waiting up to `timeout` for it.
    pub fn next(&self, timeout: Duration) -> Option<Seen<T>> {
        self.received.recv_timeout(timeout).ok()
    }

    /// Returns every frame handed over so far that has not been taken.
    pub fn drain(&self) -> Vec<Seen<T>> {
        self.received.try_iter().collect()
    }

    /// Stops the capture, waiting for its last frame callback, and returns
    /// its counts with every frame not taken yet, including those still
    /// queued when it stopped.
    pub fn stop(self) -> Result<(CaptureStats, Vec<Seen<T>>), String> {
        let stats = self.capture.stop().map_err(|error| error.to_string())?;
        Ok((stats, self.received.try_iter().collect()))
    }
}

/// The part of a frame a gate looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// The whole frame.
    Whole,
    /// The frame below its title bar: everything but the top `TITLE_BAR`
    /// points.
    Content,
    /// The top `percent` of the frame.
    Top(usize),
}

/// The height of a window's title bar, in points, with some margin: the
/// benchmark window's is 33 points.
pub const TITLE_BAR: usize = 40;

impl Region {
    /// Returns the region's rectangle in `image`.
    pub fn rect(self, image: &Bgra<'_>) -> Rect {
        let (y, height) = match self {
            Region::Whole => (0, image.height),
            Region::Content => (
                TITLE_BAR.min(image.height),
                image.height.saturating_sub(TITLE_BAR),
            ),
            Region::Top(percent) => (0, image.height * percent / 100),
        };
        Rect {
            x: 0,
            y,
            width: image.width,
            height,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_are_parts_of_the_frame() {
        let pixels = vec![0; 100 * 4 * 200];
        let image = Bgra {
            width: 100,
            height: 200,
            bytes_per_row: 400,
            pixels: &pixels,
        };
        let rect = |region: Region| {
            let rect = region.rect(&image);
            (rect.x, rect.y, rect.width, rect.height)
        };
        assert_eq!(rect(Region::Whole), (0, 0, 100, 200));
        assert_eq!(rect(Region::Content), (0, 40, 100, 160));
        assert_eq!(rect(Region::Top(45)), (0, 0, 100, 90));
    }
}
