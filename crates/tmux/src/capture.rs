//! Reading a pane's contents from `capture-pane`.
//!
//! A rebuild (`docs/ARCHITECTURE.md`) captures a pane
//! with `capture-pane -p -e -N -C`: one line per row, holding the row's text
//! and the escape sequences that set its style, hyperlink and character set,
//! with every byte below a space written as a backslash and three octal
//! digits and a backslash as two. [`rows`] reads those lines into [`Row`]s.
//! [`pending`] reads what `capture-pane -p -P -C` returns: the bytes the
//! pane's parser holds without having acted on them yet.
//!
//! A capture is terminal output, so it is read as a terminal reads it: the
//! style, hyperlink and character set carry on from one row into the next.
//! tmux writes it in two ways a terminal would misread. It writes each code
//! above 9 as its two digits joined by a colon, so the overline, 53, arrives
//! as `5:3`, which a terminal reads as blink. And from tmux 3.6, a tab over
//! blank cells is kept as one cell and captured as a tab character.

/// An error in what `capture-pane` returned.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Bytes that tmux does not write in a capture.
    #[error("malformed capture: {0}")]
    Malformed(&'static str),
}

/// One row of a capture.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// The row's text from its first column, split where the style,
    /// hyperlink or character set changes. Cells after the last segment were
    /// never written.
    pub segments: Vec<Segment>,
}

/// Text that shares one style, hyperlink and character set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    /// The text's colours and attributes.
    pub style: Style,
    /// The hyperlink the text belongs to, if any.
    pub link: Option<Link>,
    /// Whether tmux draws the text with the DEC line-drawing character set:
    /// tmux captures it as plain characters, such as `q` for a horizontal
    /// line, after a shift-out.
    pub line_drawing: bool,
    /// The text, one grapheme cluster per character cell. A tab is a tab
    /// cell: blank cells in this style up to the next tab stop.
    pub text: String,
}

/// A hyperlink, as an OSC 8 sequence sets it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The link's parameters, such as `id=page`, or empty.
    pub params: String,
    /// The link's target.
    pub uri: String,
}

/// A cell's colours and attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// The text colour.
    pub foreground: Color,
    /// The background colour.
    pub background: Color,
    /// The underline colour.
    pub underline_color: Color,
    /// The underline, if any.
    pub underline: Underline,
    /// Bold, italic and the other on-or-off attributes.
    pub attributes: Attributes,
}

/// A colour as the program set it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    /// The terminal's default colour.
    #[default]
    Default,
    /// An entry of the 256-colour palette. The eight standard and eight
    /// bright colours are entries 0 to 15.
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

    /// Clears every attribute in `other`.
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// Reads the reply lines of `capture-pane -p -e -N -C`, without their line
/// ends, into rows.
pub fn rows<'a>(lines: impl IntoIterator<Item = &'a [u8]>) -> Result<Vec<Row>, Error> {
    let mut reader = Reader::default();
    let mut rows = Vec::new();
    let mut line = Vec::new();
    for escaped in lines {
        unescape(escaped, &mut line)?;
        rows.push(reader.row(&line)?);
    }
    Ok(rows)
}

/// Reads the reply of `capture-pane -p -P -C` into the bytes to feed a
/// terminal so that its parser continues where the pane's stopped.
///
/// tmux has already acted on a control character inside an escape or
/// control sequence, such as a line feed after `ESC [`, so those are dropped.
/// Inside a string sequence, such as OSC, tmux ignores them, so they stay.
/// The bytes can pass through several sequences, for example an OSC that an
/// `ESC [` interrupts, so each byte is judged by the state tmux's parser was
/// in when it read it.
pub fn pending<'a>(lines: impl IntoIterator<Item = &'a [u8]>) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let mut line = Vec::new();
    for (index, escaped) in lines.into_iter().enumerate() {
        if index > 0 {
            bytes.push(b'\n');
        }
        unescape(escaped, &mut line)?;
        bytes.extend_from_slice(&line);
    }
    let mut state = Parser::Ground;
    bytes.retain(|&byte| {
        let executed = state.executes_controls() && is_executed_control(byte);
        state = state.next(byte);
        !executed
    });
    Ok(bytes)
}

/// The states of tmux's input parser (`input.c`, the same from 3.3 to 3.8)
/// that decide what it does with a control character.
#[derive(Clone, Copy)]
enum Parser {
    Ground,
    Escape,
    EscapeIntermediate,
    /// Any part of a control sequence.
    Csi,
    DcsEntry,
    DcsParameter,
    DcsIntermediate,
    DcsIgnore,
    DcsPassthrough,
    DcsEscape,
    Osc,
    /// APC, SOS, PM and tmux's `ESC k` window name.
    OtherString,
}

/// Returns whether tmux executes `byte` when it arrives in an escape or
/// control sequence. Cancel, substitute and escape end the sequence instead.
fn is_executed_control(byte: u8) -> bool {
    matches!(byte, 0x00..=0x17 | 0x19 | 0x1c..=0x1f)
}

impl Parser {
    fn executes_controls(self) -> bool {
        matches!(self, Self::Escape | Self::EscapeIntermediate | Self::Csi)
    }

    fn next(self, byte: u8) -> Self {
        match (self, byte) {
            // A DCS's data ends only at its string terminator.
            (Self::DcsPassthrough, ESC) => Self::DcsEscape,
            (Self::DcsPassthrough, _) => Self::DcsPassthrough,
            (Self::DcsEscape, b'\\') => Self::Ground,
            (Self::DcsEscape, _) => Self::DcsPassthrough,
            (_, CAN | SUB) => Self::Ground,
            (_, ESC) => Self::Escape,
            (Self::Ground, _) => Self::Ground,
            (Self::Escape, 0x20..=0x2f) => Self::EscapeIntermediate,
            (Self::Escape, b'P') => Self::DcsEntry,
            (Self::Escape, b'[') => Self::Csi,
            (Self::Escape, b']') => Self::Osc,
            (Self::Escape, b'X' | b'^' | b'_' | b'k') => Self::OtherString,
            (Self::Escape | Self::EscapeIntermediate, 0x30..=0x7e) => Self::Ground,
            (Self::Csi, 0x40..=0x7e) => Self::Ground,
            (Self::DcsEntry | Self::DcsParameter | Self::DcsIntermediate, 0x40..=0x7e) => {
                Self::DcsPassthrough
            }
            (Self::DcsEntry | Self::DcsParameter, 0x20..=0x2f) => Self::DcsIntermediate,
            (Self::DcsEntry, b':') | (Self::DcsParameter, b':' | 0x3c..=0x3f) => Self::DcsIgnore,
            (Self::DcsEntry, 0x30..=0x3f) => Self::DcsParameter,
            (Self::DcsIntermediate, 0x30..=0x3f) => Self::DcsIgnore,
            (Self::Osc, 0x07) => Self::Ground,
            (state, _) => state,
        }
    }
}

const ESC: u8 = 0x1b;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;
const SHIFT_OUT: u8 = 0x0e;
const SHIFT_IN: u8 = 0x0f;

/// The state a capture carries from one row into the next.
#[derive(Default)]
struct Reader {
    style: Style,
    link: Option<Link>,
    line_drawing: bool,
}

impl Reader {
    fn row(&mut self, mut line: &[u8]) -> Result<Row, Error> {
        let mut row = Row::default();
        let mut text = Vec::new();
        while let Some(&byte) = line.first() {
            let (style, link, line_drawing) = (self.style, self.link.clone(), self.line_drawing);
            line = match byte {
                ESC => self.escape(line)?,
                SHIFT_OUT | SHIFT_IN => {
                    self.line_drawing = byte == SHIFT_OUT;
                    &line[1..]
                }
                b'\t' | b' '.. => {
                    let end = line
                        .iter()
                        .position(|&byte| byte < b' ' && byte != b'\t')
                        .unwrap_or(line.len());
                    text.extend_from_slice(&line[..end]);
                    &line[end..]
                }
                _ => return Err(Error::Malformed("a control character in a row")),
            };
            let changed =
                (style, &link, line_drawing) != (self.style, &self.link, self.line_drawing);
            if changed && !text.is_empty() {
                row.segments.push(Segment {
                    style,
                    link,
                    line_drawing,
                    text: String::from_utf8_lossy(&text).into_owned(),
                });
                text.clear();
            }
        }
        if !text.is_empty() {
            row.segments.push(Segment {
                style: self.style,
                link: self.link.clone(),
                line_drawing: self.line_drawing,
                text: String::from_utf8_lossy(&text).into_owned(),
            });
        }
        Ok(row)
    }

    /// Applies the escape sequence at the start of `line` and returns the
    /// rest of the line.
    fn escape<'a>(&mut self, line: &'a [u8]) -> Result<&'a [u8], Error> {
        match line.get(1) {
            Some(b'[') => {
                let end = line[2..]
                    .iter()
                    .position(|byte| (0x40..=0x7e).contains(byte))
                    .ok_or(Error::Malformed("an unfinished control sequence"))?
                    + 2;
                if line[end] != b'm' {
                    return Err(Error::Malformed("a control sequence other than SGR"));
                }
                self.select_graphic_rendition(&line[2..end]);
                Ok(&line[end + 1..])
            }
            Some(b']') => {
                let end = line
                    .windows(2)
                    .position(|pair| pair == [ESC, b'\\'])
                    .ok_or(Error::Malformed("an unfinished OSC sequence"))?;
                let link = line[2..end]
                    .strip_prefix(b"8;")
                    .ok_or(Error::Malformed("an OSC sequence other than a hyperlink"))?;
                let separator = link
                    .iter()
                    .position(|&byte| byte == b';')
                    .ok_or(Error::Malformed("a hyperlink without a target"))?;
                let uri = &link[separator + 1..];
                self.link = (!uri.is_empty()).then(|| Link {
                    params: String::from_utf8_lossy(&link[..separator]).into_owned(),
                    uri: String::from_utf8_lossy(uri).into_owned(),
                });
                Ok(&line[end + 2..])
            }
            _ => Err(Error::Malformed("an escape sequence tmux does not capture")),
        }
    }

    /// Applies an SGR sequence's parameters, ignoring any it does not know as
    /// a terminal does.
    fn select_graphic_rendition(&mut self, params: &[u8]) {
        let style = &mut self.style;
        let mut fields = params.split(|&byte| byte == b';');
        while let Some(field) = fields.next() {
            let mut parts = field.split(|&byte| byte == b':').map(number);
            let code = parts.next().flatten().unwrap_or(0);
            let sub = parts.next().flatten();
            let set = |style: &mut Style, attribute, on| {
                if on {
                    style.attributes.insert(attribute);
                } else {
                    style.attributes.remove(attribute);
                }
            };
            match (code, sub) {
                (0, _) => *style = Style::default(),
                (1, _) => set(style, Attributes::BOLD, true),
                (2, _) => set(style, Attributes::FAINT, true),
                (3, _) => set(style, Attributes::ITALIC, true),
                (4, None) => style.underline = Underline::Single,
                (4, Some(kind)) => {
                    style.underline = match kind {
                        0 => Underline::None,
                        2 => Underline::Double,
                        3 => Underline::Curly,
                        4 => Underline::Dotted,
                        5 => Underline::Dashed,
                        _ => Underline::Single,
                    };
                }
                // tmux's way of writing 53.
                (5, Some(3)) | (53, _) => set(style, Attributes::OVERLINE, true),
                (5, _) => set(style, Attributes::BLINK, true),
                (7, _) => set(style, Attributes::INVERSE, true),
                (8, _) => set(style, Attributes::INVISIBLE, true),
                (9, _) => set(style, Attributes::STRIKETHROUGH, true),
                (22, _) => {
                    set(style, Attributes::BOLD, false);
                    set(style, Attributes::FAINT, false);
                }
                (23, _) => set(style, Attributes::ITALIC, false),
                (24, _) => style.underline = Underline::None,
                (25, _) => set(style, Attributes::BLINK, false),
                (27, _) => set(style, Attributes::INVERSE, false),
                (28, _) => set(style, Attributes::INVISIBLE, false),
                (29, _) => set(style, Attributes::STRIKETHROUGH, false),
                (55, _) => set(style, Attributes::OVERLINE, false),
                (30..=37, _) => style.foreground = Color::Palette(code as u8 - 30),
                (90..=97, _) => style.foreground = Color::Palette(code as u8 - 90 + 8),
                (39, _) => style.foreground = Color::Default,
                (40..=47, _) => style.background = Color::Palette(code as u8 - 40),
                (100..=107, _) => style.background = Color::Palette(code as u8 - 100 + 8),
                (49, _) => style.background = Color::Default,
                (59, _) => style.underline_color = Color::Default,
                (38 | 48 | 58, None) => {
                    let Some(color) = extended_color(&mut fields) else {
                        continue;
                    };
                    match code {
                        38 => style.foreground = color,
                        48 => style.background = color,
                        _ => style.underline_color = color,
                    }
                }
                _ => {}
            }
        }
    }
}

/// Reads the rest of an extended colour, `5;N` or `2;R;G;B`, from the fields
/// after `38`, `48` or `58`.
fn extended_color<'a>(fields: &mut impl Iterator<Item = &'a [u8]>) -> Option<Color> {
    let mut next = || {
        fields
            .next()
            .and_then(number)
            .and_then(|n| u8::try_from(n).ok())
    };
    match next()? {
        5 => Some(Color::Palette(next()?)),
        2 => Some(Color::Rgb(next()?, next()?, next()?)),
        _ => None,
    }
}

fn number(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() || digits.len() > 9 || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    Some(
        digits
            .iter()
            .fold(0, |value, digit| value * 10 + u32::from(digit - b'0')),
    )
}

/// Replaces `out` with `escaped` with `capture-pane -C`'s escapes decoded:
/// a backslash and three octal digits, or two backslashes. A backslash
/// before a byte above ASCII is a backslash.
fn unescape(escaped: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    out.clear();
    let mut rest = escaped;
    while let Some(backslash) = rest.iter().position(|&byte| byte == b'\\') {
        out.extend_from_slice(&rest[..backslash]);
        rest = &rest[backslash + 1..];
        match rest {
            [b'\\', ..] => {
                out.push(b'\\');
                rest = &rest[1..];
            }
            [a @ b'0'..=b'3', b @ b'0'..=b'7', c @ b'0'..=b'7', ..] => {
                out.push((a - b'0') << 6 | (b - b'0') << 3 | (c - b'0'));
                rest = &rest[3..];
            }
            // tmux doubles a backslash only in a cell of one byte, so a
            // backslash with a combining mark is written as it is.
            [0x80..=0xff, ..] => out.push(b'\\'),
            _ => return Err(Error::Malformed("a backslash that is not an escape")),
        }
    }
    out.extend_from_slice(rest);
    Ok(())
}
