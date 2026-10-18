//! Rebuilding a terminal from a pane's state as tmux holds it
//! (`docs/decisions/0021-pane-state-recovery.md`).
//!
//! The types here mirror what tmux reports for a pane, in this crate's own
//! terms: the caller converts a capture into a [`Restore`], and
//! [`Terminal::restored`] writes it into a fresh terminal as terminal output.
//! After that, the pane's output continues where the capture stopped.

use libghostty_vt::terminal::Mode;

use crate::{Attributes, Color, Error, Style, Terminal, Underline};

/// A pane's state, from which [`Terminal::restored`] builds a terminal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Restore {
    /// The width in cells.
    pub width: u16,
    /// The height in cells.
    pub height: u16,
    /// The rows of history, oldest first, then the rows of the screen the
    /// pane shows.
    pub rows: Vec<Row>,
    /// How many of [`Restore::rows`] are history.
    pub history: usize,
    /// With the alternate screen on, the primary screen saved when the
    /// program switched to it; otherwise `None`.
    pub alternate: Option<Alternate>,
    /// The cursor's column and row, from zero at the top left of the screen.
    /// A column equal to the width is a cursor waiting to wrap. A column
    /// beyond it can survive a tmux resize and cannot be installed in the
    /// engine; the caller must retain that snapshot until it can rebuild.
    pub cursor: (u16, u16),
    /// Whether the program shows the cursor.
    pub cursor_visible: bool,
    /// The top and bottom rows of the scrolling region, from zero.
    pub scroll_region: (u16, u16),
    /// The columns with a tab stop, from zero.
    pub tab_stops: Vec<u16>,
    /// The terminal modes the program has set.
    pub modes: Modes,
    /// Bytes to feed the terminal last, so that its parser continues where
    /// the pane's stopped.
    pub pending: Vec<u8>,
}

/// The primary screen while the alternate screen is on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Alternate {
    /// The primary screen's rows.
    pub saved_rows: Vec<Row>,
    /// The cursor's column and row on the primary screen.
    pub saved_cursor: (u16, u16),
}

/// One row, from its first column. Cells after the last segment are blank.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// The row's text, split where the style, hyperlink or character set
    /// changes.
    pub segments: Vec<Segment>,
}

/// Text that shares one style, hyperlink and character set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Segment {
    /// The text's colours and attributes.
    pub style: Style,
    /// The underline colour.
    pub underline_color: Color,
    /// The hyperlink the text belongs to, if any.
    pub link: Option<Link>,
    /// Whether the text is drawn with the DEC line-drawing character set,
    /// such as `q` for a horizontal line.
    pub line_drawing: bool,
    /// The text, one grapheme cluster per character cell. A tab is a tab
    /// cell: blank cells in this style from its column up to the next tab
    /// stop, or to the last column if no stop follows.
    pub text: String,
}

/// A hyperlink, as an OSC 8 sequence sets it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Link {
    /// The link's parameters, such as `id=page`, or empty.
    pub params: String,
    /// The link's target.
    pub uri: String,
}

/// Terminal modes. A mode that is not known is `None`, never a false value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modes {
    /// Cursor keys send application sequences (DECCKM).
    pub cursor_keys: bool,
    /// The keypad sends application sequences (DECKPAM).
    pub keypad: bool,
    /// Insert mode (IRM).
    pub insert: bool,
    /// Text wraps at the right margin (DECAWM).
    pub wrap: bool,
    /// Cursor positions are relative to the scrolling region (DECOM).
    pub origin: bool,
    /// Mouse button presses are reported (1000).
    pub mouse_standard: bool,
    /// Mouse motion with a button down is reported (1002).
    pub mouse_button: bool,
    /// All mouse motion is reported (1003).
    pub mouse_all: bool,
    /// Mouse reports use UTF-8 coordinates (1005).
    pub mouse_utf8: bool,
    /// Mouse reports use the SGR format (1006).
    pub mouse_sgr: bool,
    /// Bracketed paste (2004).
    pub bracketed_paste: Option<bool>,
    /// The modifyOtherKeys mode.
    pub extended_keys: Option<ExtendedKeys>,
    /// Focus reporting (1004).
    pub focus_events: Option<bool>,
}

/// How a program has asked for keys to be encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendedKeys {
    /// Ordinary keys.
    Off,
    /// modifyOtherKeys level 1.
    Level1,
    /// modifyOtherKeys level 2.
    Level2,
}

impl Terminal {
    /// Builds a terminal holding `restore`, keeping about `scrollback` lines
    /// of history ([`Terminal::new`]).
    ///
    /// The state is written as terminal output: the rows, the tab stops, the
    /// cursor, the scrolling region, the modes and the pending bytes, in that
    /// order. Origin mode is set first, while the region is still the whole
    /// screen, so that entering the alternate screen saves it with the
    /// primary screen's cursor, as tmux keeps it on leaving. The other modes
    /// come last: with wrapping off, writing the row of a cursor waiting to
    /// wrap again would join a combining character to the wrong cell.
    ///
    /// A cursor beyond the width is rejected: it is not ordinary pending
    /// wrap. Retain that snapshot and capture again after further pane
    /// output instead of discarding its off-grid column.
    pub fn restored(restore: &Restore, scrollback: usize) -> Result<Self, Error> {
        if restore.cursor.0 > restore.width {
            return Err(Error(libghostty_vt::Error::InvalidValue));
        }
        let mut terminal = Self::new(restore.width, restore.height, scrollback)?;
        let mut out = Writer {
            terminal: &mut terminal,
            bytes: Vec::new(),
            width: restore.width,
            tab_stops: &restore.tab_stops,
        };
        let modes = &restore.modes;
        if modes.origin {
            out.put(b"\x1b[?6h");
        }
        let history = restore.history.min(restore.rows.len());
        let screen = &restore.rows[history..];
        match &restore.alternate {
            None => out.rows(&restore.rows)?,
            Some(alternate) => {
                out.rows(restore.rows[..history].iter().chain(&alternate.saved_rows))?;
                // Entering the alternate screen saves the cursor with the
                // style and character sets, which are not known: the
                // defaults. tmux moves a saved cursor past the last column
                // back onto it on leaving, as the engine does here.
                out.move_to(alternate.saved_cursor);
                out.put(b"\x1b[?1049h\x1b[H");
                out.rows(screen)?;
            }
        }
        out.put(b"\x1b[3g");
        for &stop in &restore.tab_stops {
            out.move_to((stop, 0));
            out.put(b"\x1bH");
        }
        out.place_cursor(restore.cursor, screen)?;
        // Setting the region moves the cursor, and the cursor can be outside
        // the region even in origin mode: tmux moves it to the top left of
        // the screen when the region is set. Saving and restoring it keeps
        // its place, and whether it is waiting to wrap.
        let (top, bottom) = restore.scroll_region;
        if (top, bottom) != (0, restore.height.saturating_sub(1)) {
            out.put(format!("\x1b7\x1b[{};{}r\x1b8", top + 1, bottom + 1).as_bytes());
        }
        for (on, set) in [
            (modes.cursor_keys, &b"\x1b[?1h"[..]),
            (modes.keypad, b"\x1b="),
            (modes.insert, b"\x1b[4h"),
            (!modes.wrap, b"\x1b[?7l"),
            (modes.mouse_standard, b"\x1b[?1000h"),
            (modes.mouse_button, b"\x1b[?1002h"),
            (modes.mouse_all, b"\x1b[?1003h"),
            (modes.mouse_utf8, b"\x1b[?1005h"),
            (modes.mouse_sgr, b"\x1b[?1006h"),
            (modes.bracketed_paste == Some(true), b"\x1b[?2004h"),
            (modes.focus_events == Some(true), b"\x1b[?1004h"),
            (
                modes.extended_keys == Some(ExtendedKeys::Level1),
                b"\x1b[>4;1m",
            ),
            (
                modes.extended_keys == Some(ExtendedKeys::Level2),
                b"\x1b[>4;2m",
            ),
            (!restore.cursor_visible, b"\x1b[?25l"),
        ] {
            if on {
                out.put(set);
            }
        }
        out.put(&restore.pending);
        out.flush();
        Ok(terminal)
    }

    /// Returns the terminal modes the program has set. The engine does not
    /// report the modifyOtherKeys mode, so that is `None`.
    pub fn modes(&self) -> Result<Modes, Error> {
        let mode = |mode| self.engine.mode(mode).map_err(Error);
        Ok(Modes {
            cursor_keys: mode(Mode::DECCKM)?,
            keypad: mode(Mode::KEYPAD_KEYS)?,
            insert: mode(Mode::INSERT)?,
            wrap: mode(Mode::WRAPAROUND)?,
            origin: mode(Mode::ORIGIN)?,
            mouse_standard: mode(Mode::NORMAL_MOUSE)?,
            mouse_button: mode(Mode::BUTTON_MOUSE)?,
            mouse_all: mode(Mode::ANY_MOUSE)?,
            mouse_utf8: mode(Mode::UTF8_MOUSE)?,
            mouse_sgr: mode(Mode::SGR_MOUSE)?,
            bracketed_paste: Some(mode(Mode::BRACKETED_PASTE)?),
            extended_keys: None,
            focus_events: Some(mode(Mode::FOCUS_EVENT)?),
        })
    }
}

/// Writes a [`Restore`] into a terminal as terminal output.
struct Writer<'a> {
    terminal: &'a mut Terminal,
    /// Output not yet given to the terminal.
    bytes: Vec<u8>,
    width: u16,
    tab_stops: &'a [u16],
}

impl Writer<'_> {
    fn put(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    fn flush(&mut self) {
        self.terminal.write(&self.bytes);
        self.bytes.clear();
    }

    /// Moves the cursor to a column and row, from zero. Positions count
    /// from the top of the screen while the region is the whole screen,
    /// origin mode or not.
    fn move_to(&mut self, (col, row): (u16, u16)) {
        self.put(format!("\x1b[{};{}H", u32::from(row) + 1, u32::from(col) + 1).as_bytes());
    }

    /// Writes rows from the cursor's row down. Each row starts in the
    /// default style, and a row scrolled in later fills with it. The style
    /// and character sets are the defaults again afterwards.
    fn rows<'r>(&mut self, rows: impl IntoIterator<Item = &'r Row>) -> Result<(), Error> {
        // Cells drawn in line drawing are written as plain characters after
        // a shift-out, so G1 holds the line-drawing set while rows go in.
        self.put(b"\x1b)0");
        for (index, row) in rows.into_iter().enumerate() {
            if index > 0 {
                self.put(b"\x1b[0m\r\n");
            }
            self.row(row)?;
        }
        self.put(b"\x1b[0m\x1b)B");
        Ok(())
    }

    /// Moves the cursor to a column and row of the screen whose rows are
    /// `rows`. A column equal to the width is a cursor waiting to wrap:
    /// writing its row again from the first column leaves the cursor
    /// waiting to wrap, whatever the last cell's width.
    fn place_cursor(&mut self, (col, row): (u16, u16), rows: &[Row]) -> Result<(), Error> {
        if col < self.width {
            self.move_to((col, row));
            return Ok(());
        }
        self.move_to((0, row));
        if let Some(cells) = rows.get(usize::from(row)) {
            self.rows([cells])?;
        }
        self.fill_row()
    }

    /// Writes one row from the cursor, with G1 holding the line-drawing set.
    fn row(&mut self, row: &Row) -> Result<(), Error> {
        let mut link = None;
        for segment in &row.segments {
            if segment.link.as_ref() != link {
                link = segment.link.as_ref();
                match link {
                    Some(Link { params, uri }) => {
                        self.put(format!("\x1b]8;{params};{uri}\x1b\\").as_bytes());
                    }
                    None => self.put(b"\x1b]8;;\x1b\\"),
                }
            }
            self.put(&sgr(&segment.style, segment.underline_color));
            if segment.line_drawing {
                self.put(b"\x0e");
            }
            for (index, text) in segment.text.split('\t').enumerate() {
                if index > 0 {
                    self.tab()?;
                }
                self.put(text.as_bytes());
            }
            if segment.line_drawing {
                self.put(b"\x0f");
            }
        }
        if link.is_some() {
            self.put(b"\x1b]8;;\x1b\\");
        }
        Ok(())
    }

    /// Writes a tab cell as tmux draws one: blank cells in the current style
    /// from the cursor's column up to, not including, the next tab stop, or
    /// the last column if no stop follows. At the last column, or waiting to
    /// wrap, it writes nothing.
    fn tab(&mut self) -> Result<(), Error> {
        self.flush();
        let col = self.terminal.cursor()?.0;
        let last = self.width.saturating_sub(1);
        if col >= last {
            return Ok(());
        }
        let end = self
            .tab_stops
            .iter()
            .copied()
            .find(|&stop| stop > col)
            .map_or(last, |stop| stop.min(last));
        self.put(" ".repeat(usize::from(end - col)).as_bytes());
        Ok(())
    }

    /// Leaves the cursor waiting to wrap at the end of its row. The row
    /// written again usually reaches the last column; if its last cells
    /// were never written, they are written as blank cells.
    fn fill_row(&mut self) -> Result<(), Error> {
        self.flush();
        if self
            .terminal
            .engine
            .is_cursor_pending_wrap()
            .map_err(Error)?
        {
            return Ok(());
        }
        let col = self.terminal.cursor()?.0;
        self.put(
            " ".repeat(usize::from(self.width.saturating_sub(col)))
                .as_bytes(),
        );
        Ok(())
    }
}

/// Returns the SGR sequences that set exactly `style` and
/// `underline_color`. The colours go in sequences of their own, as the
/// engine reads at most 24 parameters in one.
fn sgr(style: &Style, underline_color: Color) -> Vec<u8> {
    let mut sgr = String::from("\x1b[0");
    for (attribute, code) in [
        (Attributes::BOLD, "1"),
        (Attributes::FAINT, "2"),
        (Attributes::ITALIC, "3"),
        (Attributes::BLINK, "5"),
        (Attributes::INVERSE, "7"),
        (Attributes::INVISIBLE, "8"),
        (Attributes::STRIKETHROUGH, "9"),
        (Attributes::OVERLINE, "53"),
    ] {
        if style.attributes.contains(attribute) {
            sgr.push(';');
            sgr.push_str(code);
        }
    }
    match style.underline {
        Underline::None => {}
        Underline::Single => sgr.push_str(";4"),
        Underline::Double => sgr.push_str(";4:2"),
        Underline::Curly => sgr.push_str(";4:3"),
        Underline::Dotted => sgr.push_str(";4:4"),
        Underline::Dashed => sgr.push_str(";4:5"),
    }
    sgr.push('m');
    for (color, code) in [
        (style.foreground, 38),
        (style.background, 48),
        (underline_color, 58),
    ] {
        match color {
            Color::Default => {}
            Color::Palette(index) => sgr.push_str(&format!("\x1b[{code};5;{index}m")),
            Color::Rgb(r, g, b) => sgr.push_str(&format!("\x1b[{code};2;{r};{g};{b}m")),
        }
    }
    sgr.into_bytes()
}

#[cfg(test)]
mod tests {
    use libghostty_vt::key::{Action, Encoder, Event, Key, Mods};

    use super::*;
    use crate::{CellDetails, Screen, Width};

    fn text(text: &str) -> Segment {
        Segment {
            text: text.to_owned(),
            ..Segment::default()
        }
    }

    fn row(segments: Vec<Segment>) -> Row {
        Row { segments }
    }

    /// A pane of plain rows, all of them on the screen, with tmux's default
    /// tab stops and modes.
    fn pane(width: u16, height: u16, rows: &[&str]) -> Restore {
        Restore {
            width,
            height,
            rows: rows.iter().map(|line| row(vec![text(line)])).collect(),
            cursor_visible: true,
            scroll_region: (0, height - 1),
            tab_stops: (8..width).step_by(8).collect(),
            modes: Modes {
                wrap: true,
                ..Modes::default()
            },
            ..Restore::default()
        }
    }

    fn read(terminal: &mut Terminal) -> Screen {
        let mut screen = Screen::default();
        terminal.read_screen(&mut screen).unwrap();
        screen
    }

    /// The screen's rows as text, with blank cells as spaces.
    fn lines(terminal: &mut Terminal) -> Vec<String> {
        let screen = read(terminal);
        (0..screen.rows())
            .map(|row| {
                let line: String = (0..screen.cols())
                    .filter_map(|col| match screen.cell(row, col).unwrap() {
                        (_, cell) if cell.width == Width::Spacer => None,
                        ("", _) => Some(" "),
                        (text, _) => Some(text),
                    })
                    .collect();
                line.trim_end().to_owned()
            })
            .collect()
    }

    #[test]
    fn off_grid_is_not_silently_restored_as_pending_wrap() {
        let mut restore = pane(36, 9, &[""]);
        restore.cursor = (40, 5);
        assert!(Terminal::restored(&restore, 0).is_err());
        restore.cursor = (36, 5);
        let terminal = Terminal::restored(&restore, 0).unwrap();
        assert_eq!(terminal.logical_cursor().unwrap(), (36, 5));
        assert_eq!(terminal.cursor().unwrap(), (35, 5));
        assert!(terminal.cursor_enabled().unwrap());
    }

    #[test]
    fn rows_fill_the_history_then_the_screen() {
        let mut restore = pane(4, 2, &["h1", "h2", "s1", "s2"]);
        restore.history = 2;
        restore.rows[3].segments[0].style.background = Color::Palette(4);
        restore.cursor = (2, 1);
        let mut terminal = Terminal::restored(&restore, 100).unwrap();

        assert_eq!(terminal.history().unwrap(), 2);
        assert_eq!(lines(&mut terminal), ["s1", "s2"]);
        assert_eq!(terminal.cursor().unwrap(), (2, 1));
        // The style ends with the rows: a row scrolled in fills with the
        // default colours.
        terminal.write(b"\r\n");
        let screen = read(&mut terminal);
        assert_eq!(
            screen.cell(0, 0).unwrap().1.style.background,
            Color::Palette(4)
        );
        assert_eq!(
            screen.cell(1, 3).unwrap().1.style.background,
            Color::Default
        );
    }

    #[test]
    fn segments_keep_their_style_link_and_character_set() {
        let link = Link {
            params: "id=x".to_owned(),
            uri: "https://example.com".to_owned(),
        };
        let mut bold = text("ab");
        bold.style.attributes.insert(Attributes::BOLD);
        bold.style.foreground = Color::Palette(1);
        bold.style.underline = Underline::Single;
        bold.underline_color = Color::Palette(5);
        bold.link = Some(link.clone());
        let mut lined = text("c");
        lined.style.attributes.insert(Attributes::OVERLINE);
        lined.style.underline = Underline::Curly;
        lined.underline_color = Color::Rgb(1, 2, 3);
        lined.link = Some(link);
        let mut drawing = text("qx");
        drawing.line_drawing = true;
        let mut restore = pane(10, 2, &[]);
        restore.rows = vec![
            row(vec![bold, lined, drawing, text("q")]),
            row(vec![text("z")]),
        ];
        restore.cursor = (6, 0);
        let mut terminal = Terminal::restored(&restore, 0).unwrap();
        // G1 is plain text again once the rows are in.
        terminal.write(b"\x0eq\x0f");

        assert_eq!(lines(&mut terminal), ["abc─│qq", "z"]);
        let screen = read(&mut terminal);
        let style = screen.cell(0, 0).unwrap().1.style;
        assert!(style.attributes.contains(Attributes::BOLD));
        assert_eq!(style.foreground, Color::Palette(1));
        let style = screen.cell(0, 2).unwrap().1.style;
        assert!(style.attributes.contains(Attributes::OVERLINE));
        assert!(!style.attributes.contains(Attributes::BLINK));
        assert_eq!(style.underline, Underline::Curly);
        assert_eq!(style.foreground, Color::Default);
        let uri = Some("https://example.com".to_owned());
        let details = terminal.cell_details(0, 0).unwrap();
        assert_eq!(
            (details.underline_color, details.link),
            (Color::Palette(5), uri.clone())
        );
        let details = terminal.cell_details(0, 2).unwrap();
        assert_eq!(
            (details.underline_color, details.link),
            (Color::Rgb(1, 2, 3), uri)
        );
        assert_eq!(terminal.cell_details(0, 3).unwrap(), CellDetails::default());
        // A link is closed at the end of its row.
        assert_eq!(terminal.cell_details(1, 0).unwrap().link, None);
    }

    #[test]
    fn a_tab_cell_spans_to_the_next_tab_stop() {
        let mut tabbed = text("\tb\t");
        tabbed.style.background = Color::Palette(2);
        let mut restore = pane(12, 1, &[]);
        restore.tab_stops = vec![4];
        // A tab at the last column, or waiting to wrap, takes no cells.
        restore.rows = vec![row(vec![text("a"), tabbed, text("x\t")])];
        let mut terminal = Terminal::restored(&restore, 0).unwrap();

        let screen = read(&mut terminal);
        let cell = |col| screen.cell(0, col).unwrap();
        for col in (1..4).chain(5..11) {
            assert_eq!(cell(col).0, " ", "column {col}");
            assert_eq!(cell(col).1.style.background, Color::Palette(2));
        }
        assert_eq!((cell(0).0, cell(4).0, cell(11).0), ("a", "b", "x"));
        assert_eq!(cell(11).1.style.background, Color::Default);
    }

    #[test]
    fn the_alternate_screen_keeps_the_primary_screen_and_its_cursor() {
        let mut restore = pane(4, 2, &["h", "a1", "a2"]);
        restore.history = 1;
        restore.alternate = Some(Alternate {
            saved_rows: vec![row(vec![text("p1")]), row(vec![text("p2")])],
            saved_cursor: (1, 1),
        });
        restore.cursor = (2, 0);
        let mut terminal = Terminal::restored(&restore, 100).unwrap();

        assert_eq!(lines(&mut terminal), ["a1", "a2"]);
        assert_eq!(terminal.cursor().unwrap(), (2, 0));
        terminal.write(b"\x1b[?1049l");
        assert_eq!(lines(&mut terminal), ["p1", "p2"]);
        assert_eq!(terminal.cursor().unwrap(), (1, 1));
        assert_eq!(terminal.history().unwrap(), 1);
    }

    #[test]
    fn leaving_the_alternate_screen_keeps_the_default_style_and_origin_mode() {
        let mut red = text("qq");
        red.style.foreground = Color::Palette(1);
        red.line_drawing = true;
        let mut restore = pane(4, 4, &["a1", "", "", ""]);
        restore.alternate = Some(Alternate {
            saved_rows: vec![
                row(vec![text("p1")]),
                row(vec![]),
                row(vec![]),
                row(vec![red]),
            ],
            saved_cursor: (0, 3),
        });
        restore.scroll_region = (1, 2);
        restore.modes.origin = true;
        restore.cursor = (2, 0);
        let mut terminal = Terminal::restored(&restore, 0).unwrap();
        terminal.write(b"\x1b[?1049l");
        assert_eq!(terminal.cursor().unwrap(), (0, 3));
        // The program's style and character sets are not known: plain.
        terminal.write(b"x\x0eq\x0f");
        // Origin mode stays on, as tmux keeps it.
        terminal.write(b"\x1b[Hy");

        assert_eq!(lines(&mut terminal), ["p1", "y", "", "xq"]);
        let screen = read(&mut terminal);
        assert_eq!(screen.cell(3, 0).unwrap().1.style, Style::default());
    }

    #[test]
    fn leaving_the_alternate_screen_puts_a_cursor_past_the_last_column_on_it() {
        let mut restore = pane(4, 1, &[""]);
        restore.alternate = Some(Alternate {
            saved_rows: vec![row(vec![text("abcd")])],
            saved_cursor: (4, 0),
        });
        let mut terminal = Terminal::restored(&restore, 0).unwrap();
        terminal.write(b"\x1b[?1049lX");

        assert_eq!(lines(&mut terminal), ["abcX"]);
    }

    #[test]
    fn origin_mode_keeps_a_cursor_outside_the_region() {
        // tmux's DECSTBM homes the cursor to the top left of the screen,
        // even in origin mode.
        let mut restore = pane(10, 4, &["", "", "", ""]);
        restore.scroll_region = (1, 2);
        restore.modes.origin = true;
        restore.cursor = (4, 0);
        let mut terminal = Terminal::restored(&restore, 0).unwrap();

        assert_eq!(terminal.cursor().unwrap(), (4, 0));
        terminal.write(b"\x1b[H");
        assert_eq!(terminal.cursor().unwrap(), (0, 1));
    }

    #[test]
    fn every_attribute_and_colour_together_is_kept() {
        let mut all = text("a");
        for attribute in [
            Attributes::BOLD,
            Attributes::FAINT,
            Attributes::ITALIC,
            Attributes::BLINK,
            Attributes::INVERSE,
            Attributes::INVISIBLE,
            Attributes::STRIKETHROUGH,
            Attributes::OVERLINE,
        ] {
            all.style.attributes.insert(attribute);
        }
        all.style.underline = Underline::Curly;
        all.style.foreground = Color::Rgb(1, 2, 3);
        all.style.background = Color::Rgb(4, 5, 6);
        all.underline_color = Color::Rgb(7, 8, 9);
        let mut restore = pane(4, 1, &[]);
        restore.rows = vec![row(vec![all.clone()])];
        let mut terminal = Terminal::restored(&restore, 0).unwrap();

        assert_eq!(read(&mut terminal).cell(0, 0).unwrap().1.style, all.style);
        assert_eq!(
            terminal.cell_details(0, 0).unwrap().underline_color,
            Color::Rgb(7, 8, 9)
        );
    }

    #[test]
    fn the_region_tab_stops_and_origin_mode_hold() {
        let mut restore = pane(10, 4, &["", "", "", ""]);
        restore.scroll_region = (1, 2);
        restore.tab_stops = vec![3, 7];
        restore.modes.origin = true;
        restore.cursor = (5, 2);
        let mut terminal = Terminal::restored(&restore, 0).unwrap();

        assert_eq!(terminal.cursor().unwrap(), (5, 2));
        assert!(terminal.modes().unwrap().origin);
        terminal.write(b"\x1b[H");
        assert_eq!(terminal.cursor().unwrap(), (0, 1));
        terminal.write(b"\t");
        assert_eq!(terminal.cursor().unwrap(), (3, 1));
        terminal.write(b"\t");
        assert_eq!(terminal.cursor().unwrap(), (7, 1));
        terminal.write(b"\x1b[9;1H");
        assert_eq!(terminal.cursor().unwrap(), (0, 2));
    }

    /// Every mode tmux reports set, other than wrapping off.
    fn every_mode() -> Modes {
        Modes {
            cursor_keys: true,
            keypad: true,
            insert: true,
            wrap: true,
            origin: true,
            mouse_standard: true,
            mouse_button: true,
            mouse_all: true,
            mouse_utf8: true,
            mouse_sgr: true,
            bracketed_paste: Some(true),
            extended_keys: Some(ExtendedKeys::Level2),
            focus_events: Some(true),
        }
    }

    #[test]
    fn a_cursor_waiting_to_wrap_stays_waiting_under_every_mode() {
        let base = pane(1, 1, &[]).modes;
        let cases = [
            Modes {
                cursor_keys: true,
                ..base
            },
            Modes {
                keypad: true,
                ..base
            },
            Modes {
                insert: true,
                ..base
            },
            Modes {
                wrap: false,
                ..base
            },
            Modes {
                origin: true,
                ..base
            },
            Modes {
                mouse_standard: true,
                ..base
            },
            Modes {
                mouse_button: true,
                ..base
            },
            Modes {
                mouse_all: true,
                ..base
            },
            Modes {
                mouse_utf8: true,
                ..base
            },
            Modes {
                mouse_sgr: true,
                ..base
            },
            Modes {
                bracketed_paste: Some(true),
                ..base
            },
            Modes {
                focus_events: Some(true),
                ..base
            },
            Modes {
                extended_keys: Some(ExtendedKeys::Level1),
                ..base
            },
            Modes {
                extended_keys: Some(ExtendedKeys::Level2),
                ..base
            },
            every_mode(),
            Modes {
                wrap: false,
                ..every_mode()
            },
        ];
        for (modes, cursor_visible) in cases
            .into_iter()
            .map(|modes| (modes, true))
            .chain([(base, false)])
        {
            let mut drawing = text("qq");
            drawing.line_drawing = true;
            let mut restore = pane(4, 2, &[]);
            restore.rows = vec![row(vec![drawing, text("ab")]), row(vec![])];
            restore.cursor = (4, 0);
            restore.modes = modes;
            restore.cursor_visible = cursor_visible;
            let mut terminal = Terminal::restored(&restore, 0).unwrap();

            assert!(
                terminal.engine.is_cursor_pending_wrap().unwrap(),
                "{modes:?}"
            );
            assert_eq!(terminal.cursor().unwrap(), (3, 0), "{modes:?}");
            assert_eq!(lines(&mut terminal), ["──ab", ""], "{modes:?}");
            let expected = Modes {
                bracketed_paste: Some(modes.bracketed_paste.unwrap_or(false)),
                extended_keys: None,
                focus_events: Some(modes.focus_events.unwrap_or(false)),
                ..modes
            };
            assert_eq!(terminal.modes().unwrap(), expected);
            assert_eq!(terminal.engine.is_cursor_visible().unwrap(), cursor_visible);
        }
    }

    #[test]
    fn a_cursor_waiting_to_wrap_after_any_row_wraps_the_next_character() {
        // After a wide character, and in a row whose last cells were cleared.
        for line in ["ab界", "ab"] {
            for modes in [pane(1, 1, &[]).modes, every_mode()] {
                let mut restore = pane(4, 2, &[line, ""]);
                restore.cursor = (4, 0);
                restore.modes = modes;
                let mut terminal = Terminal::restored(&restore, 0).unwrap();
                assert!(terminal.engine.is_cursor_pending_wrap().unwrap(), "{line}");

                terminal.write(b"x");
                assert_eq!(lines(&mut terminal), [line, "x"], "{modes:?}");
                assert_eq!(terminal.cursor().unwrap(), (1, 1));
            }
        }
    }

    #[test]
    fn a_cursor_waiting_to_wrap_with_wrapping_off_keeps_combining_characters() {
        let mut restore = pane(4, 1, &["abe\u{301}x"]);
        restore.cursor = (4, 0);
        restore.modes.wrap = false;
        let mut terminal = Terminal::restored(&restore, 0).unwrap();

        assert_eq!(lines(&mut terminal), ["abe\u{301}x"]);
        assert!(terminal.engine.is_cursor_pending_wrap().unwrap());
    }

    #[test]
    fn the_extended_keys_mode_reaches_the_key_encoder() {
        let mut event = Event::new().unwrap();
        event
            .set_action(Action::Press)
            .set_key(Key::H)
            .set_mods(Mods::CTRL | Mods::SHIFT)
            .set_utf8(Some("H"));
        let encode = |keys| {
            let mut restore = pane(4, 1, &[]);
            restore.modes.extended_keys = keys;
            let terminal = Terminal::restored(&restore, 0).unwrap();
            let mut encoder = Encoder::new().unwrap();
            encoder.set_options_from_terminal(&terminal.engine);
            let mut bytes = Vec::with_capacity(32);
            encoder.encode_to_vec(&event, &mut bytes).unwrap();
            bytes
        };
        assert_eq!(encode(Some(ExtendedKeys::Level2)), b"\x1b[27;6;72~");
        for keys in [None, Some(ExtendedKeys::Off), Some(ExtendedKeys::Level1)] {
            assert_ne!(encode(keys), b"\x1b[27;6;72~", "{keys:?}");
        }
    }

    #[test]
    fn pending_bytes_continue_the_parser() {
        let mut restore = pane(4, 1, &[]);
        restore.pending = b"\x1b[3".to_vec();
        let mut terminal = Terminal::restored(&restore, 0).unwrap();
        terminal.write(b"1mx");

        let screen = read(&mut terminal);
        let (text, cell) = screen.cell(0, 0).unwrap();
        assert_eq!((text, cell.style.foreground), ("x", Color::Palette(1)));
    }
}
