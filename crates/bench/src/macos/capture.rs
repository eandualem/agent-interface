//! Capturing one window's frames with ScreenCaptureKit.
//!
//! A [`WindowCapture`] streams the frames the window server composites for
//! one window, each with the time it was displayed. The window may be
//! covered by others: ScreenCaptureKit captures the window's own content.
//! Capturing needs the host's screen-recording permission, which this module
//! checks but never requests.

#![allow(unsafe_code)]

use std::io;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send, sel};
use objc2_core_foundation::{CFArray, CGRect};
use objc2_core_graphics::CGPreflightScreenCaptureAccess;
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
    kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCFrameStatus, SCShareableContent, SCStream, SCStreamConfiguration,
    SCStreamFrameInfoDisplayTime, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType,
};

use super::clock::HostTime;
use super::pixels::Bgra;

/// How long to wait for ScreenCaptureKit to answer a request.
const TIMEOUT: Duration = Duration::from_secs(10);

/// How a window is captured.
#[derive(Clone, Copy, Debug)]
pub struct CaptureOptions {
    /// Pixels per point in the captured image: 1 gives one pixel per point,
    /// 2 the full detail of a Retina display.
    pub scale: f64,
    /// The shortest time between two frames. Zero asks for every frame the
    /// window server composites.
    pub minimum_frame_interval: Duration,
    /// How many frames ScreenCaptureKit may have in use at once.
    pub queue_depth: usize,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            scale: 1.0,
            minimum_frame_interval: Duration::ZERO,
            queue_depth: 8,
        }
    }
}

/// One complete frame of the window, lent to the frame callback.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// When the window server displayed the frame.
    pub display_time: HostTime,
    /// The frame's pixels, valid only during the callback.
    pub image: Bgra<'a>,
}

/// What a capture delivered, counted when it stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureStats {
    /// Every sample ScreenCaptureKit delivered.
    pub delivered: u64,
    /// Complete frames, the ones passed to the callback.
    pub complete: u64,
    /// Samples saying the window had not changed since the last frame.
    pub idle: u64,
    /// Any other sample: blank, suspended, started or stopped, or one that
    /// could not be read.
    pub other: u64,
    /// When the first complete frame was displayed.
    pub first_display: Option<HostTime>,
    /// When the last complete frame was displayed.
    pub last_display: Option<HostTime>,
}

/// The callback that receives each complete frame.
type OnFrame = Box<dyn FnMut(&Frame<'_>) + Send>;

/// The state the stream output shares with [`WindowCapture`]. ScreenCaptureKit
/// calls the output on one serial queue, so the locks are never contended
/// while frames arrive.
struct OutputState {
    on_frame: Mutex<Option<OnFrame>>,
    stats: Mutex<CaptureStats>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and `Output` does not
    // implement `Drop`.
    #[unsafe(super(NSObject))]
    #[ivars = OutputState]
    struct Output;

    unsafe impl NSObjectProtocol for Output {}

    unsafe impl SCStreamOutput for Output {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output(
            &self,
            _stream: &SCStream,
            sample: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind == SCStreamOutputType::Screen {
                self.receive(sample);
            }
        }
    }
);

impl Output {
    fn new(on_frame: OnFrame) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputState {
            on_frame: Mutex::new(Some(on_frame)),
            stats: Mutex::new(CaptureStats::default()),
        });
        // SAFETY: `init` is NSObject's designated initialiser.
        unsafe { msg_send![super(this), init] }
    }

    fn receive(&self, sample: &CMSampleBuffer) {
        let mut stats = lock(&self.ivars().stats);
        stats.delivered += 1;
        let (status, display_time) = frame_info(sample);
        match (status, display_time) {
            (Some(SCFrameStatus::Complete), Some(_)) => {}
            (Some(SCFrameStatus::Idle), _) => {
                stats.idle += 1;
                return;
            }
            _ => {
                stats.other += 1;
                return;
            }
        }
        let display_time = display_time.expect("matched above");
        // SAFETY: a screen sample from ScreenCaptureKit is backed by an
        // image buffer, retained here for the rest of this call.
        let Some(buffer) = (unsafe { sample.image_buffer() }) else {
            stats.other += 1;
            return;
        };
        if CVPixelBufferGetPixelFormatType(&buffer) != kCVPixelFormatType_32BGRA {
            stats.other += 1;
            return;
        }
        // SAFETY: `buffer` is a live pixel buffer; it is unlocked below with
        // the same flags.
        let locked =
            unsafe { CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly) };
        if locked != kCVReturnSuccess {
            stats.other += 1;
            return;
        }
        stats.complete += 1;
        stats.first_display.get_or_insert(display_time);
        stats.last_display = Some(display_time);
        drop(stats);

        let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
        let bytes_per_row = CVPixelBufferGetBytesPerRow(&buffer);
        let height = CVPixelBufferGetHeight(&buffer);
        if !base.is_null() {
            // SAFETY: while the buffer is locked, its base address points at
            // `height` rows of `bytes_per_row` bytes, which nothing writes.
            let pixels = unsafe { std::slice::from_raw_parts(base, bytes_per_row * height) };
            let frame = Frame {
                display_time,
                image: Bgra {
                    width: CVPixelBufferGetWidth(&buffer),
                    height,
                    bytes_per_row,
                    pixels,
                },
            };
            if let Some(on_frame) = lock(&self.ivars().on_frame).as_mut() {
                on_frame(&frame);
            }
        }
        // SAFETY: the buffer was locked above with these flags.
        unsafe { CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::ReadOnly) };
    }
}

/// Reads a sample's frame status and display time from its attachments.
fn frame_info(sample: &CMSampleBuffer) -> (Option<SCFrameStatus>, Option<HostTime>) {
    // SAFETY: reading the attachments without creating them has no
    // preconditions.
    let Some(attachments) = (unsafe { sample.sample_attachments_array(false) }) else {
        return (None, None);
    };
    // SAFETY: a CFArray is toll-free bridged with NSArray, and
    // ScreenCaptureKit's attachments are dictionaries keyed by strings.
    let attachments: &NSArray<NSDictionary<NSString, AnyObject>> =
        unsafe { &*(&*attachments as *const CFArray).cast() };
    let Some(info) = attachments.firstObject() else {
        return (None, None);
    };
    // SAFETY: the keys are constant strings that ScreenCaptureKit exports.
    let (status_key, time_key) = unsafe { (SCStreamFrameInfoStatus, SCStreamFrameInfoDisplayTime) };
    let number = |key: &NSString| {
        info.objectForKey(key)
            .and_then(|value| value.downcast::<NSNumber>().ok())
    };
    let status = number(status_key).map(|status| SCFrameStatus(status.integerValue()));
    let time = number(time_key).map(|time| HostTime::from_ticks(time.unsignedLongLongValue()));
    (status, time)
}

/// A running capture of one window. Stop it with [`WindowCapture::stop`];
/// dropping it stops it too, without waiting.
pub struct WindowCapture {
    stream: Retained<SCStream>,
    output: Retained<Output>,
    queue: DispatchRetained<DispatchQueue>,
    stopped: bool,
}

/// An Objective-C object handed from a completion handler to the thread
/// waiting for it.
struct Handed<T>(T);

// SAFETY: ScreenCaptureKit's content, window and filter objects are
// immutable once created and have no thread affinity; each is moved to the
// waiting thread only after the completion handler has finished with it.
unsafe impl<T> Send for Handed<T> {}

impl WindowCapture {
    /// Starts capturing window `window_id`, calling `on_frame` on a capture
    /// thread for every complete frame. `on_frame` should return quickly:
    /// ScreenCaptureKit drops frames while earlier ones are still in use.
    pub fn start(
        window_id: u32,
        options: CaptureOptions,
        on_frame: impl FnMut(&Frame<'_>) + Send + 'static,
    ) -> io::Result<Self> {
        if !CGPreflightScreenCaptureAccess() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "this process may not capture the screen: grant the program that runs \
                 it Screen Recording access in System Settings",
            ));
        }
        let (filter, frame) = window_filter(window_id)?;
        let config = configuration(frame, options)?;
        // SAFETY: the filter and configuration are fully initialised; no
        // delegate is needed because errors also end the frames.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            )
        };
        let output = Output::new(Box::new(on_frame));
        let queue = DispatchQueue::new("tmux-workspaces-bench.capture", None);
        // SAFETY: the queue is a serial queue this capture owns, which
        // `Output` needs because its callback is not reentrant.
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                Some(&queue),
            )
        }
        .map_err(|error| ns_error("could not add the capture output", &error))?;
        let mut capture = Self {
            stream,
            output,
            queue,
            stopped: false,
        };
        let started = wait_for_completion(|handler| {
            // SAFETY: the handler is a valid block that outlives the call.
            unsafe {
                capture
                    .stream
                    .startCaptureWithCompletionHandler(Some(handler))
            }
        });
        if let Err(error) = started {
            capture.stopped = true;
            return Err(io::Error::other(format!(
                "could not start capturing: {error}"
            )));
        }
        Ok(capture)
    }

    /// Stops capturing, waits for the last frame callback to return, and
    /// returns what the capture delivered.
    pub fn stop(mut self) -> io::Result<CaptureStats> {
        self.stopped = true;
        let stopped = wait_for_completion(|handler| {
            // SAFETY: the handler is a valid block that outlives the call.
            unsafe { self.stream.stopCaptureWithCompletionHandler(Some(handler)) }
        });
        // Removing the output and running an empty task on its queue makes
        // sure no callback is still running once this returns.
        // SAFETY: `output` was added to this stream for screen samples.
        let _ = unsafe {
            self.stream.removeStreamOutput_type_error(
                ProtocolObject::from_ref(&*self.output),
                SCStreamOutputType::Screen,
            )
        };
        self.queue.exec_sync(|| {});
        lock(&self.output.ivars().on_frame).take();
        stopped.map_err(|error| io::Error::other(format!("could not stop capturing: {error}")))?;
        Ok(*lock(&self.output.ivars().stats))
    }
}

impl Drop for WindowCapture {
    fn drop(&mut self) {
        if !self.stopped {
            // SAFETY: stopping without a completion handler is allowed.
            unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
        }
        lock(&self.output.ivars().on_frame).take();
    }
}

/// Finds window `window_id` among the windows ScreenCaptureKit may capture,
/// and returns a filter that captures it alone, with its frame in points.
fn window_filter(window_id: u32) -> io::Result<(Retained<SCContentFilter>, CGRect)> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            // SAFETY: ScreenCaptureKit passes either valid objects or null.
            let (content, error) = unsafe { (Retained::retain(content), error.as_ref()) };
            let result = match (content, error) {
                (_, Some(error)) => Err(ns_error_text(error)),
                (None, None) => Err("no shareable content".to_owned()),
                (Some(content), None) => {
                    // SAFETY: reading the windows of shareable content has no
                    // preconditions.
                    let windows = unsafe { content.windows() };
                    // SAFETY: as above, for each window's identifier.
                    match windows
                        .iter()
                        .find(|window| unsafe { window.windowID() } == window_id)
                    {
                        None => Err(format!(
                            "window {window_id} cannot be captured or does not exist"
                        )),
                        Some(window) => {
                            // SAFETY: as above, for the window's frame; the
                            // filter initialiser takes a valid window.
                            let frame = unsafe { window.frame() };
                            let filter = unsafe {
                                SCContentFilter::initWithDesktopIndependentWindow(
                                    SCContentFilter::alloc(),
                                    &window,
                                )
                            };
                            Ok(Handed((filter, frame)))
                        }
                    }
                }
            };
            let _ = sender.send(result);
        },
    );
    // SAFETY: the handler is a valid block; ScreenCaptureKit keeps it alive
    // until it has been called. Off-screen windows are included, so that a
    // covered window can still be found.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, false, &handler,
        );
    }
    let result = receiver.recv_timeout(TIMEOUT).map_err(|_| {
        io::Error::other("ScreenCaptureKit did not list the shareable windows in time")
    })?;
    result.map(|Handed(found)| found).map_err(io::Error::other)
}

/// Builds the stream configuration for a window of `frame` points.
fn configuration(
    frame: CGRect,
    options: CaptureOptions,
) -> io::Result<Retained<SCStreamConfiguration>> {
    let pixels = |points: f64| (points * options.scale).round().max(1.0) as usize;
    let interval = i64::try_from(options.minimum_frame_interval.as_nanos())
        .map_err(|_| io::Error::other("the minimum frame interval is too long"))?;
    let queue_depth = isize::try_from(options.queue_depth)
        .map_err(|_| io::Error::other("the queue depth is too large"))?;
    // SAFETY: a new configuration is set up before any stream uses it, with
    // values in the ranges ScreenCaptureKit documents.
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setWidth(pixels(frame.size.width));
        config.setHeight(pixels(frame.size.height));
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        config.setShowsCursor(false);
        // ScreenCaptureKit itself needs macOS 12.3, but these two settings
        // came later, audio in macOS 13 and shadows in macOS 14; older
        // systems capture no audio anyway, and keep the shadow.
        if config.respondsToSelector(sel!(setCapturesAudio:)) {
            config.setCapturesAudio(false);
        }
        if config.respondsToSelector(sel!(setIgnoreShadowsSingleWindow:)) {
            config.setIgnoreShadowsSingleWindow(true);
        }
        config.setMinimumFrameInterval(CMTime::new(interval, 1_000_000_000));
        config.setQueueDepth(queue_depth);
        Ok(config)
    }
}

/// Calls `request` with a completion handler, and waits for ScreenCaptureKit
/// to call it.
fn wait_for_completion(
    request: impl FnOnce(&block2::DynBlock<dyn Fn(*mut NSError)>),
) -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: ScreenCaptureKit passes a valid error or null.
        let _ = sender.send(unsafe { error.as_ref() }.map(ns_error_text));
    });
    request(&handler);
    match receiver.recv_timeout(TIMEOUT) {
        Ok(None) => Ok(()),
        Ok(Some(error)) => Err(error),
        Err(_) => Err("ScreenCaptureKit did not answer in time".to_owned()),
    }
}

fn ns_error_text(error: &NSError) -> String {
    error.localizedDescription().to_string()
}

fn ns_error(context: &str, error: &NSError) -> io::Error {
    io::Error::other(format!("{context}: {}", ns_error_text(error)))
}

/// Locks `mutex`, carrying on if a callback panicked while holding it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
