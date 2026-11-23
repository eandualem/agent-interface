//! Operations the native terminal grid needs from its attached session.

use super::{Input, InputReservation, PaneId, Panes, Rgba, Screen, Scroll};
use crate::panes::Update;
use std::{io, ops::Range, sync::Arc};

pub(crate) trait Backend: Send + Sync {
    fn set_visible(&self, _visible: bool) {}
    fn set_agent_default(&self, _size: Option<(u16, u16)>) -> bool {
        false
    }
    fn take_update(&self) -> Option<Update>;
    fn recycle(&self, screens: Vec<Screen>);
    fn send_reserved_input(
        &self,
        pane: PaneId,
        input: Input,
        reservation: Option<Arc<InputReservation>>,
    ) -> io::Result<()>;
    fn read_rows(&self, pane: PaneId, rows: Range<usize>) -> io::Result<Screen>;
    fn set_size(&self, cols: u16, rows: u16) -> io::Result<()>;
    fn set_colors(&self, foreground: Rgba, background: Rgba);
    fn ignore_size(&self) -> io::Result<()>;
    fn failure(&self) -> Option<String>;
    fn is_finished(&self) -> bool;
    fn take_windows_changed(&self) -> bool;
    fn shutdown(&self);
}

impl Backend for Panes {
    fn set_agent_default(&self, size: Option<(u16, u16)>) -> bool {
        self.set_agent_default(size)
    }
    fn take_update(&self) -> Option<Update> {
        self.take_update()
    }
    fn recycle(&self, screens: Vec<Screen>) {
        self.recycle(screens)
    }
    fn send_reserved_input(
        &self,
        pane: PaneId,
        input: Input,
        reservation: Option<Arc<InputReservation>>,
    ) -> io::Result<()> {
        self.send_reserved_input(pane, input, reservation)
    }
    fn read_rows(&self, pane: PaneId, rows: Range<usize>) -> io::Result<Screen> {
        self.read_rows(pane, rows)
    }
    fn set_size(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.set_size(cols, rows)
    }
    fn set_colors(&self, fg: Rgba, bg: Rgba) {
        self.set_colors(fg, bg)
    }
    fn ignore_size(&self) -> io::Result<()> {
        self.ignore_size()
    }
    fn failure(&self) -> Option<String> {
        self.failure()
    }
    fn is_finished(&self) -> bool {
        self.is_finished()
    }
    fn take_windows_changed(&self) -> bool {
        self.take_windows_changed()
    }
    fn shutdown(&self) {
        self.shutdown()
    }
}

/// A session shown, never typed into, such as a run's view: keys, pastes
/// and clicks are refused, and the wheel scrolls its history.
pub(crate) struct ReadOnly(pub Arc<Panes>);

/// What a refused key or paste says.
pub(crate) const READ_ONLY: &str = "This view is read-only.";

impl Backend for ReadOnly {
    fn take_update(&self) -> Option<Update> {
        self.0.take_update()
    }
    fn recycle(&self, screens: Vec<Screen>) {
        self.0.recycle(screens)
    }
    fn send_reserved_input(
        &self,
        pane: PaneId,
        input: Input,
        reservation: Option<Arc<InputReservation>>,
    ) -> io::Result<()> {
        let scroll = match input {
            Input::Scroll(scroll) => scroll,
            Input::Wheel(_, rows) => Scroll::Lines(rows),
            Input::Mouse(..) | Input::Click(..) => return Ok(()),
            Input::Key(_) | Input::Paste(_) => return Err(io::Error::other(READ_ONLY)),
        };
        self.0
            .send_reserved_input(pane, Input::Scroll(scroll), reservation)
    }
    fn read_rows(&self, pane: PaneId, rows: Range<usize>) -> io::Result<Screen> {
        self.0.read_rows(pane, rows)
    }
    fn set_size(&self, cols: u16, rows: u16) -> io::Result<()> {
        self.0.set_size(cols, rows)
    }
    fn set_colors(&self, fg: Rgba, bg: Rgba) {
        self.0.set_colors(fg, bg)
    }
    fn ignore_size(&self) -> io::Result<()> {
        self.0.ignore_size()
    }
    fn failure(&self) -> Option<String> {
        self.0.failure()
    }
    fn is_finished(&self) -> bool {
        self.0.is_finished()
    }
    fn take_windows_changed(&self) -> bool {
        self.0.take_windows_changed()
    }
    fn shutdown(&self) {
        self.0.shutdown()
    }
}
