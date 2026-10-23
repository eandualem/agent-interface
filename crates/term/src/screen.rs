//! Owned display data: a copy of a terminal's visible screen that a renderer
//! can draw without access to the engine.
//!
//! This file uses nothing but the standard library, so that the benchmark's
//! `rio-vt` program, which cannot link the engine, can include it.

use std::mem::size_of;

/// The visible screen of a terminal, copied out of the engine.
///
/// A `Screen` holds no engine handle, so the thread that owns the terminal can
/// hand it to the user-interface thread
/// (`docs/ARCHITECTURE.md`). Reading
/// into the same `Screen` again reuses its allocations.
///
/// `Terminal::read_screen` fills a screen from the engine. Building one cell
/// by cell with [`Screen::clear`] and [`Screen::push`] is for comparisons and
/// tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    /// Every cell's text, concatenated in row-major order.
    text: String,
    cursor: Cursor,
    wrapped: Vec<bool>,
    pub(crate) mouse_input: bool,
}

/// One cell of a [`Screen`]. Its text is read through [`Screen::cell`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cell {
    text_start: u32,
    text_len: u32,
    /// How the cell's text spans columns.
    pub width: Width,
    /// The cell's colours and attributes.
    pub style: Style,
}

/// How a cell's text spans columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Width {
    /// The text takes one column.
    #[default]
    Narrow,
    /// The text takes two columns: this cell and the [`Width::Spacer`] after
    /// it.
    Wide,
    /// The second column of a wide character. It has no text of its own.
    Spacer,
    /// The last column of a row, left empty because the next character was
    /// wide and wrapped to the next row.
    WrapSpacer,
}

/// A cell's colours and attributes, as the program set them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// The text colour.
    pub foreground: Color,
    /// The background colour.
    pub background: Color,
    /// The underline, if any.
    pub underline: Underline,
    /// Bold, italic and the other on-or-off attributes.
    pub attributes: Attributes,
}

/// A colour as the program set it, before any palette is applied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    /// The terminal's default colour.
    #[default]
    Default,
    /// An entry of the 256-colour palette.
    Palette(u8),
    /// A direct colour: red, green and blue.
    Rgb(u8, u8, u8),
}

/// The kind of underline under a cell's text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Underline {
    /// No underline.
    #[default]
    None,
    /// A single line.
    Single,
    /// A double line.
    Double,
    /// A wavy line.
    Curly,
    /// A dotted line.
    Dotted,
    /// A dashed line.
    Dashed,
}

/// A set of on-or-off text attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attributes(u8);

impl Attributes {
    /// Bold text.
    pub const BOLD: Self = Self(1);
    /// Faint, or dim, text.
    pub const FAINT: Self = Self(1 << 1);
    /// Italic text.
    pub const ITALIC: Self = Self(1 << 2);
    /// Blinking text.
    pub const BLINK: Self = Self(1 << 3);
    /// Foreground and background swapped.
    pub const INVERSE: Self = Self(1 << 4);
    /// Hidden text.
    pub const INVISIBLE: Self = Self(1 << 5);
    /// Text with a line through it.
    pub const STRIKETHROUGH: Self = Self(1 << 6);
    /// Text with a line above it.
    pub const OVERLINE: Self = Self(1 << 7);

    /// Returns whether every attribute in `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Sets every attribute in `other`.
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

/// Where the cursor is and whether it is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    /// The column, counted from zero at the left.
    pub col: u16,
    /// The row, counted from zero at the top of the screen.
    pub row: u16,
    /// Whether the cursor is shown on this screen.
    pub visible: bool,
}

impl Screen {
    /// Returns the number of columns.
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Returns the number of rows.
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Returns the cursor.
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Whether the displayed live program owns unmodified mouse clicks.
    pub fn mouse_input(&self) -> bool {
        self.mouse_input
    }

    /// Returns the text and the cell at `row` and `col`, counted from zero at
    /// the top left, or `None` outside the screen. The text is a whole
    /// grapheme cluster, and empty when the cell has none.
    pub fn cell(&self, row: u16, col: u16) -> Option<(&str, Cell)> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        let cell = *self
            .cells
            .get(usize::from(row) * usize::from(self.cols) + usize::from(col))?;
        let start = cell.text_start as usize;
        let text = self.text.get(start..start + cell.text_len as usize)?;
        Some((text, cell))
    }

    /// Returns the number of bytes this screen's display data takes: what a
    /// copy of it hands to the user interface.
    pub fn size_in_bytes(&self) -> usize {
        self.cells.len() * size_of::<Cell>() + self.text.len() + self.wrapped.len()
    }

    /// Empties the screen and sets its size, keeping its allocations. Then
    /// [`Screen::push`] every cell, row by row from the top left.
    pub fn clear(&mut self, cols: u16, rows: u16) {
        self.cols = cols;
        self.rows = rows;
        self.cells.clear();
        self.text.clear();
        self.cursor = Cursor::default();
        self.mouse_input = false;
        self.wrapped.clear();
        self.wrapped.resize(usize::from(rows), false);
    }

    /// Adds the next cell, after the cells already pushed.
    pub fn push(&mut self, text: &str, width: Width, style: Style) {
        // A screen's text is a few bytes per cell, far below `u32::MAX`.
        self.cells.push(Cell {
            text_start: self.text.len() as u32,
            text_len: text.len() as u32,
            width,
            style,
        });
        self.text.push_str(text);
    }

    /// Whether a row continues directly onto the next row without a line break.
    pub fn row_wrapped(&self, row: u16) -> bool {
        self.wrapped.get(usize::from(row)).copied().unwrap_or(false)
    }

    /// Records a row's soft-wrap flag when copying terminal display data.
    pub fn set_row_wrapped(&mut self, row: u16, wrapped: bool) {
        if let Some(value) = self.wrapped.get_mut(usize::from(row)) {
            *value = wrapped;
        }
    }

    /// Sets the cursor.
    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.cursor = cursor;
    }
}
