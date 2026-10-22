//! The Python viewer's default theme and its terminal-cell grid.
//!
//! The source panel is a curses program in a tmux pane: every row is one
//! terminal cell high and every position a whole number of cells. The native
//! chrome uses the same font, grid and colors so that it lays out the same.

use gpui::{Div, Pixels, Size, div, prelude::*, px, rgb};

/// One complete set of colours: the window's chrome and the terminals'
/// defaults. Values are `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    /// The sidebar's, status row's and popups' ground.
    pub panel: u32,
    /// The terminals' ground, behind content panes and separators.
    pub surface: u32,
    /// Separator lines.
    pub outline: u32,
    /// The heading's bar and popover frames.
    pub header: u32,
    /// The selected tab, menu row and workspace slot.
    pub active: u32,
    /// Text on the selected bar.
    pub active_text: u32,
    pub normal: u32,
    pub muted: u32,
    pub accent: u32,
    pub danger: u32,
    /// The status row's saved-state light.
    pub ok: u32,
    /// Default terminal text and cursor.
    pub terminal_text: u32,
    /// The first sixteen terminal colours.
    pub terminal: [u32; 16],
}

/// The approved dark look, with Ghostty's default terminal colours.
pub const DARK: Palette = Palette {
    panel: 0x15171c,
    surface: 0x1b1e24,
    outline: 0x2a2e36,
    header: 0x1e2128,
    active: 0x262a33,
    active_text: 0xffffff,
    normal: 0xd4d6db,
    muted: 0x7d828c,
    accent: 0xe0a458,
    danger: 0xe06c75,
    ok: 0x98c379,
    terminal_text: 0xffffff,
    terminal: [
        0x1d1f21, 0xcc6666, 0xb5bd68, 0xf0c674, 0x81a2be, 0xb294bb, 0x8abeb7, 0xc5c8c6, 0x666666,
        0xd54e53, 0xb9ca4a, 0xe7c547, 0x7aa6da, 0xc397d8, 0x70c0b1, 0xeaeaea,
    ],
};

/// The light counterpart: the previous tool's paper colours, with Tomorrow's
/// light terminal colours so text stays readable on white.
pub const LIGHT: Palette = Palette {
    panel: 0xf8f8f8,
    surface: 0xffffff,
    outline: 0xe5e5e5,
    header: 0xececec,
    active: 0xe8e8e8,
    active_text: 0x1f1f1f,
    normal: 0x3b3b3b,
    muted: 0x6e6e6e,
    accent: 0xb8651b,
    danger: 0xc0392b,
    ok: 0x2e7d32,
    terminal_text: 0x4d4d4c,
    terminal: [
        0x1d1f21, 0xc82829, 0x718c00, 0xb58900, 0x4271ae, 0x8959a8, 0x3e999f, 0xd6d6d6, 0x8e908c,
        0xc82829, 0x718c00, 0xb58900, 0x4271ae, 0x8959a8, 0x3e999f, 0x4d4d4c,
    ],
};

static LIGHT_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The palette every window draws with.
pub fn current() -> &'static Palette {
    if LIGHT_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        &LIGHT
    } else {
        &DARK
    }
}

/// Switch every window's palette; windows repaint on their next frame.
pub fn set_light(light: bool) {
    LIGHT_ACTIVE.store(light, std::sync::atomic::Ordering::Relaxed);
}

macro_rules! colours {
    ($($name:ident),*) => {$(
        pub fn $name() -> u32 {
            current().$name
        }
    )*};
}

colours!(
    panel,
    surface,
    outline,
    header,
    active,
    active_text,
    normal,
    muted,
    accent,
    danger,
    ok
);

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
            .bg(rgb(surface()))
            .child(div().w(px(1.0)).h_full().bg(rgb(outline())))
    }
}
