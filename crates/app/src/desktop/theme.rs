//! The Python viewer's default theme and its terminal-cell grid.
//!
//! The source panel is a curses program in a tmux pane: every row is one
//! terminal cell high and every position a whole number of cells. The native
//! chrome uses the same font, grid and colors so that it lays out the same.

use gpui::{Div, Pixels, Size, div, prelude::*, px, rgb};

/// The sidebar's, status row's and popups' ground.
pub const PANEL: u32 = 0x15171c;
/// The terminals' ground, behind content panes and separators.
pub const SURFACE: u32 = 0x1b1e24;
/// Separator lines.
pub const OUTLINE: u32 = 0x2a2e36;
/// The heading's bar.
pub const HEADER: u32 = 0x1e2128;
/// The selected tab, menu row and workspace slot.
pub const ACTIVE: u32 = 0x262a33;
/// Text on the selected bar.
pub const ACTIVE_TEXT: u32 = 0xffffff;
pub const NORMAL: u32 = 0xd4d6db;
pub const MUTED: u32 = 0x7d828c;
pub const ACCENT: u32 = 0xe0a458;
pub const DANGER: u32 = 0xe06c75;
/// The status row's saved-state light.
pub const OK: u32 = 0x98c379;

/// The panel's width, in terminal cells, as in the source layout.
pub const SIDEBAR_COLUMNS: usize = 22;

/// One terminal cell of the chrome's font.
#[derive(Clone, Copy)]
pub struct Cells(pub Size<Pixels>);

impl Cells {
    pub fn width(self, columns: usize) -> Pixels {
        self.0.width * columns as f32
    }

    /// A full-width row, one cell high, whose text starts one cell in.
    pub fn row(self) -> Div {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(self.0.height)
            .px(self.0.width)
            .overflow_hidden()
            .whitespace_nowrap()
    }

    /// A blank row.
    pub fn blank(self) -> Div {
        div().flex_shrink_0().h(self.0.height)
    }

    /// The tmux separator between panes: one cell of surface with a thin line
    /// in the outline color down its middle.
    pub fn separator(self) -> Div {
        div()
            .flex_shrink_0()
            .w(self.0.width)
            .h_full()
            .flex()
            .justify_center()
            .bg(rgb(SURFACE))
            .child(div().w(px(1.0)).h_full().bg(rgb(OUTLINE)))
    }
}
