//! Operations the native terminal grid needs from its attached session.

use super::{Input, InputReservation, PaneId, Panes, Rgba, Screen};
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
