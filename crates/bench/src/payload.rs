//! Seeded benchmark payloads: the terminal output the milestone-0 gates send
//! through tmux and replay (`docs/decisions/0013-milestone-0-benchmark-method.md`).
//!
//! Each [`Kind`] imitates one sort of real output, written for a 200×50
//! terminal. [`generate`] is a pure function of its arguments, built on a
//! local SplitMix64 generator rather than a dependency, so a kind, seed and
//! size give the same bytes on every machine and in later versions of this
//! harness. That is what keeps results from different runs comparable.
//!
//! Every payload:
//!
//! - ends each line with LF alone, because the pane's terminal driver adds
//!   the CR;
//! - holds no tabs, queries, OSC or DCS strings, or mode changes, so it needs
//!   no replies;
//! - uses only these sequences: CSI with a final `m`, `K`, `J`, `H`, `r`,
//!   `L` or `M`, and ESC `M`, ESC `7` and ESC `8`;
//! - never contains `~` or `tw-bench`, which the harness's markers use;
//! - ends with [`TAIL`].

/// The end of every payload: default style, the whole screen as the scroll
/// region and the cursor on the bottom row, then a line end, so text printed
/// afterwards lands on a fresh bottom line in the default style.
pub const TAIL: &[u8] = b"\x1b[0m\x1b[r\x1b[50;1H\n";

/// The kind of terminal output a payload imitates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Log-like lines of printable ASCII.
    Ascii,
    /// Mixed multilingual text: accents and combining marks, several
    /// scripts, wide characters and emoji sequences.
    Unicode,
    /// Escape-heavy coloured output, like a compiler's or a log colouriser's.
    Color,
    /// A full-screen program that scrolls regions, inserts and deletes lines
    /// and redraws status lines.
    Scroll,
}

impl Kind {
    /// Every kind, in the order results report them.
    pub const ALL: [Kind; 4] = [Kind::Ascii, Kind::Unicode, Kind::Color, Kind::Scroll];

    /// Returns the kind's name, as the harness's command line and results
    /// use it.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Ascii => "ascii",
            Kind::Unicode => "unicode",
            Kind::Color => "color",
            Kind::Scroll => "scroll",
        }
    }

    /// Returns the kind called `name`, if there is one.
    pub fn from_name(name: &str) -> Option<Kind> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }
}

/// Returns a payload of `kind` generated from `seed`.
///
/// Lines are added until the payload holds at least `size` bytes, so it stops
/// at the first line end at or after `size`; [`TAIL`] follows.
pub fn generate(kind: Kind, seed: u64, size: usize) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut out = Vec::with_capacity(size + 8192);
    let mut region = Region::default();
    while out.len() < size {
        match kind {
            Kind::Ascii => ascii_line(&mut rng, &mut out),
            Kind::Unicode => unicode_line(&mut rng, &mut out),
            Kind::Color => color_line(&mut rng, &mut out),
            Kind::Scroll => region.step(&mut rng, &mut out),
        }
    }
    out.extend_from_slice(TAIL);
    out
}

/// SplitMix64 (Steele, Lea and Flood, 2014): small, fast and fully
/// specified, so its sequence never changes. The display gates use it too,
/// for their animations and the gaps between typed keys.
pub(crate) struct Rng(pub(crate) u64);

impl Rng {
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a number in `0..n`, for `n` below 2³².
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (((self.next_u64() >> 32) * n as u64) >> 32) as usize
    }

    /// Returns a number in `low..=high`.
    pub(crate) fn range(&mut self, low: usize, high: usize) -> usize {
        low + self.below(high - low + 1)
    }

    /// Returns true `percent` times in a hundred.
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Returns a line's width in columns: a few empty lines, most within the
/// 200-column screen, and some long enough to wrap.
fn line_columns(rng: &mut Rng) -> usize {
    match rng.below(100) {
        0..=4 => 0,
        5..=69 => rng.range(20, 120),
        70..=91 => rng.range(121, 200),
        _ => rng.range(201, 420),
    }
}

fn push_number(out: &mut Vec<u8>, mut n: usize) {
    let mut digits = [0; 20];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[start..]);
}

fn push_char(out: &mut Vec<u8>, c: char) {
    let mut buffer = [0; 4];
    out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes());
}

/// Pushes a random character from `first..=last`, a range that holds only
/// assigned characters.
fn push_char_in(rng: &mut Rng, out: &mut Vec<u8>, first: u32, last: u32) {
    let code = first + rng.below((last - first + 1) as usize) as u32;
    push_char(
        out,
        char::from_u32(code).expect("the range holds no surrogates"),
    );
}

/// Pushes CSI `row` ; `col` H, counted from one.
fn push_cursor_position(out: &mut Vec<u8>, row: usize, col: usize) {
    out.extend_from_slice(b"\x1b[");
    push_number(out, row);
    out.push(b';');
    push_number(out, col);
    out.push(b'H');
}

// ASCII text, shared by every kind. Words never contain `-`, so no output can
// spell `tw-bench`; `-` appears only between the digits of a date.

const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const HEX: &[u8] = b"0123456789abcdef";

/// Pushes a word of 1 to 12 letters and returns its length.
fn push_word(rng: &mut Rng, out: &mut Vec<u8>) -> usize {
    let len = rng.range(1, 12);
    let capital = rng.chance(10);
    for i in 0..len {
        let letter = LOWER[rng.below(LOWER.len())];
        out.push(if i == 0 && capital {
            letter.to_ascii_uppercase()
        } else {
            letter
        });
    }
    len
}

/// Pushes one token of log-like text: mostly words, with numbers, settings,
/// paths, quotes, brackets, operators and hexadecimal values.
fn push_ascii_token(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(20) {
        0..=10 => {
            push_word(rng, out);
        }
        11 | 12 => {
            push_number(out, rng.below(100_000));
            if rng.chance(30) {
                out.push(b'.');
                push_number(out, rng.below(1000));
            }
        }
        13 => {
            push_word(rng, out);
            out.push(b'=');
            if rng.chance(50) {
                push_number(out, rng.below(10_000));
            } else {
                push_word(rng, out);
            }
        }
        14 => {
            for _ in 0..rng.range(1, 4) {
                out.push(b'/');
                push_word(rng, out);
            }
            if rng.chance(50) {
                out.extend_from_slice(rng.pick(&[&b".rs"[..], b".so", b".log", b".json"]));
            }
        }
        15 => {
            out.push(b'"');
            push_word(rng, out);
            out.push(b' ');
            push_word(rng, out);
            out.push(b'"');
        }
        16 => match rng.below(3) {
            0 => {
                out.push(b'(');
                push_word(rng, out);
                out.push(b')');
            }
            1 => {
                out.push(b'[');
                push_number(out, rng.below(1000));
                out.push(b']');
            }
            _ => {
                out.push(b'{');
                push_word(rng, out);
                out.extend_from_slice(b": ");
                push_number(out, rng.below(1000));
                out.push(b'}');
            }
        },
        17 => {
            push_word(rng, out);
            out.push(*rng.pick(b",.:;!?'"));
        }
        18 => out.extend_from_slice(rng.pick(&[
            &b"=="[..],
            b"!=",
            b"<=",
            b">=",
            b"&&",
            b"||",
            b"+=",
            b"*",
            b"%",
            b"@",
            b"#",
            b"$",
            b"^",
            b"|",
            b"\\",
            b"<>",
        ])),
        _ => {
            out.extend_from_slice(b"0x");
            for _ in 0..8 {
                out.push(HEX[rng.below(HEX.len())]);
            }
        }
    }
}

/// Pushes a log line's prefix, such as
/// `2026-10-17T08:14:03.512Z INFO  [pool.worker] `.
fn push_log_prefix(rng: &mut Rng, out: &mut Vec<u8>) {
    out.extend_from_slice(b"2026-10-");
    push_two_digits(out, rng.range(10, 28));
    out.push(b'T');
    push_two_digits(out, rng.below(24));
    out.push(b':');
    push_two_digits(out, rng.below(60));
    out.push(b':');
    push_two_digits(out, rng.below(60));
    out.push(b'.');
    let millis = rng.below(1000);
    push_two_digits(out, millis / 10);
    push_number(out, millis % 10);
    out.extend_from_slice(b"Z ");
    out.extend_from_slice(rng.pick(&[
        &b"TRACE"[..],
        b"DEBUG",
        b"INFO ",
        b"INFO ",
        b"INFO ",
        b"WARN ",
        b"ERROR",
    ]));
    out.extend_from_slice(b" [");
    push_word(rng, out);
    out.push(b'.');
    push_word(rng, out);
    out.extend_from_slice(b"] ");
}

fn push_two_digits(out: &mut Vec<u8>, n: usize) {
    out.push(b'0' + (n / 10) as u8);
    out.push(b'0' + (n % 10) as u8);
}

/// Pushes ASCII tokens separated by spaces until the text since `start` is
/// at least `columns` wide, without a trailing space.
fn push_ascii_text(rng: &mut Rng, out: &mut Vec<u8>, start: usize, columns: usize) {
    while out.len() - start < columns {
        push_ascii_token(rng, out);
        out.push(b' ');
    }
    if out.len() > start && out.last() == Some(&b' ') {
        out.pop();
    }
}

fn ascii_line(rng: &mut Rng, out: &mut Vec<u8>) {
    let columns = line_columns(rng);
    if columns > 0 {
        let start = out.len();
        if rng.chance(70) {
            push_log_prefix(rng, out);
        }
        push_ascii_text(rng, out, start, columns);
    }
    out.push(b'\n');
}

// Multilingual text.

const PRECOMPOSED: &[char] = &[
    'é', 'è', 'ê', 'ë', 'à', 'á', 'â', 'ä', 'å', 'ç', 'ñ', 'ö', 'ø', 'ü', 'ß', 'ł', 'ő', 'ű', 'ą',
    'ę', 'ś', 'ž', 'č', 'ř', 'ğ', 'ş', 'ı', 'ā', 'ē', 'ī',
];

const COMBINING_MARKS: &[char] = &[
    '\u{0300}', '\u{0301}', '\u{0302}', '\u{0303}', '\u{0304}', '\u{0306}', '\u{0307}', '\u{0308}',
    '\u{030A}', '\u{030C}', '\u{0323}', '\u{0327}', '\u{0328}',
];

/// Sequences joined with ZERO WIDTH JOINER, each drawn as one emoji.
const ZWJ_SEQUENCES: &[&str] = &[
    "\u{1F469}\u{200D}\u{1F4BB}",
    "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
    "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}",
    "\u{1F9D1}\u{200D}\u{1F680}",
    "\u{1F469}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F468}",
    "\u{1F415}\u{200D}\u{1F9BA}",
];

/// Emoji that take a skin-tone modifier.
const MODIFIER_BASES: &[char] = &[
    '\u{1F44D}',
    '\u{1F44B}',
    '\u{1F64C}',
    '\u{1F44F}',
    '\u{1F9D1}',
];

/// Region codes drawn as flags from pairs of regional indicators.
const FLAG_REGIONS: &[&[u8; 2]] = &[
    b"JP", b"US", b"DE", b"FR", b"ET", b"BR", b"IN", b"KR", b"GB", b"CA",
];

/// Characters whose text presentation VARIATION SELECTOR-16 turns into emoji.
const VS16_BASES: &[char] = &[
    '\u{2764}', '\u{2600}', '\u{2714}', '\u{26A0}', '\u{2708}', '\u{260E}',
];

/// Pushes a word of 2 to 10 Latin letters, some accented, some carrying one
/// to three combining marks, and returns its width.
fn push_latin_word(rng: &mut Rng, out: &mut Vec<u8>) -> usize {
    let len = rng.range(2, 10);
    for _ in 0..len {
        match rng.below(100) {
            0..=64 => out.push(LOWER[rng.below(LOWER.len())]),
            65..=84 => push_char(out, *rng.pick(PRECOMPOSED)),
            _ => {
                out.push(LOWER[rng.below(LOWER.len())]);
                let marks = match rng.below(10) {
                    0..=6 => 1,
                    7 | 8 => 2,
                    _ => 3,
                };
                for _ in 0..marks {
                    push_char(out, *rng.pick(COMBINING_MARKS));
                }
            }
        }
    }
    len
}

/// Pushes `len` characters from `first..=last` and returns `len`.
fn push_run(rng: &mut Rng, out: &mut Vec<u8>, first: u32, last: u32, len: usize) -> usize {
    for _ in 0..len {
        push_char_in(rng, out, first, last);
    }
    len
}

/// Pushes one space-free piece of multilingual text and returns its width.
fn push_unicode_segment(rng: &mut Rng, out: &mut Vec<u8>) -> usize {
    match rng.below(100) {
        0..=24 => push_word(rng, out),
        25..=37 => push_latin_word(rng, out),
        38..=44 => {
            let len = rng.range(2, 10);
            push_run(rng, out, 0x03B1, 0x03C9, len)
        }
        45..=51 => {
            let len = rng.range(2, 10);
            push_run(rng, out, 0x0430, 0x044F, len)
        }
        52 | 53 => {
            let len = rng.range(2, 8);
            push_run(rng, out, 0x05D0, 0x05EA, len)
        }
        54 | 55 => {
            let len = rng.range(2, 8);
            push_run(rng, out, 0x0641, 0x064A, len)
        }
        56..=65 => {
            let len = rng.range(1, 6);
            2 * push_run(rng, out, 0x4E00, 0x9FFF, len)
        }
        66..=71 => {
            let len = rng.range(1, 5);
            2 * push_run(rng, out, 0xAC00, 0xD7A3, len)
        }
        72..=77 => {
            let len = rng.range(1, 6);
            if rng.chance(50) {
                2 * push_run(rng, out, 0x3041, 0x3096, len)
            } else {
                2 * push_run(rng, out, 0x30A1, 0x30FA, len)
            }
        }
        78..=84 => 2 * push_run(rng, out, 0x1F600, 0x1F64F, 1),
        85..=88 => {
            out.extend_from_slice(rng.pick(ZWJ_SEQUENCES).as_bytes());
            2
        }
        89..=91 => {
            push_char(out, *rng.pick(MODIFIER_BASES));
            push_char_in(rng, out, 0x1F3FB, 0x1F3FF);
            2
        }
        92..=94 => {
            for &letter in rng.pick(FLAG_REGIONS).iter() {
                let indicator = 0x1F1E6 + u32::from(letter - b'A');
                push_char(
                    out,
                    char::from_u32(indicator).expect("a regional indicator"),
                );
            }
            2
        }
        _ => {
            push_char(out, *rng.pick(VS16_BASES));
            push_char(out, '\u{FE0F}');
            2
        }
    }
}

fn unicode_line(rng: &mut Rng, out: &mut Vec<u8>) {
    let target = line_columns(rng);
    let mut columns = 0;
    while columns < target {
        if columns > 0 {
            out.push(b' ');
            columns += 1;
        }
        columns += push_unicode_segment(rng, out);
    }
    out.push(b'\n');
}

// Coloured text.

/// Pushes one SGR parameter that sets a colour or an attribute.
fn push_style_parameter(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(9) {
        0 => push_number(out, *rng.pick(&[30, 90]) + rng.below(8)),
        1 => push_number(out, *rng.pick(&[40, 100]) + rng.below(8)),
        2 | 3 => {
            out.extend_from_slice(if rng.chance(50) { b"38;5;" } else { b"48;5;" });
            push_number(out, rng.below(256));
        }
        4 | 5 => {
            out.extend_from_slice(if rng.chance(50) { b"38;2;" } else { b"48;2;" });
            push_rgb(rng, out);
        }
        _ => push_number(out, *rng.pick(&[1, 2, 3, 4, 5, 7, 9])),
    }
}

fn push_rgb(rng: &mut Rng, out: &mut Vec<u8>) {
    push_number(out, rng.below(256));
    out.push(b';');
    push_number(out, rng.below(256));
    out.push(b';');
    push_number(out, rng.below(256));
}

/// Pushes an SGR sequence with one to three parameters.
fn push_sgr(rng: &mut Rng, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b[");
    for i in 0..rng.range(1, 3) {
        if i > 0 {
            out.push(b';');
        }
        push_style_parameter(rng, out);
    }
    out.push(b'm');
}

/// Pushes a full or partial reset, as colourisers mix both.
fn push_reset(rng: &mut Rng, out: &mut Vec<u8>) {
    let code = *rng.pick(&[0, 0, 22, 23, 24, 25, 27, 29, 39, 49]);
    if code == 0 && rng.chance(50) {
        out.extend_from_slice(b"\x1b[m");
    } else {
        out.extend_from_slice(b"\x1b[");
        push_number(out, code);
        out.push(b'm');
    }
}

/// Pushes an SGR sequence that sets only the background.
fn push_background(rng: &mut Rng, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b[");
    match rng.below(3) {
        0 => push_number(out, *rng.pick(&[40, 100]) + rng.below(8)),
        1 => {
            out.extend_from_slice(b"48;5;");
            push_number(out, rng.below(256));
        }
        _ => {
            out.extend_from_slice(b"48;2;");
            push_rgb(rng, out);
        }
    }
    out.push(b'm');
}

fn color_line(rng: &mut Rng, out: &mut Vec<u8>) {
    let target = line_columns(rng);
    let mut columns = 0;
    while columns < target {
        if columns > 0 {
            out.push(b' ');
            columns += 1;
        }
        if rng.chance(70) {
            push_sgr(rng, out);
        }
        let start = out.len();
        push_ascii_token(rng, out);
        columns += out.len() - start;
        if rng.chance(40) {
            push_reset(rng, out);
        }
    }
    if rng.chance(15) {
        // Erase the rest of the line in a background colour.
        push_background(rng, out);
        out.extend_from_slice(b"\x1b[K");
    }
    // Otherwise the style carries into the next line.
    if rng.chance(70) {
        out.extend_from_slice(b"\x1b[0m");
    }
    out.push(b'\n');
}

// A full-screen program.

/// The scroll payload's scroll region, as rows counted from one.
struct Region {
    top: usize,
    bottom: usize,
}

impl Default for Region {
    fn default() -> Self {
        Self { top: 1, bottom: 50 }
    }
}

impl Region {
    /// Pushes one step of screen updates, ending with its only line end.
    fn step(&mut self, rng: &mut Rng, out: &mut Vec<u8>) {
        match rng.below(100) {
            0..=7 => {
                self.set(rng, out);
                push_cursor_position(out, self.top, 1);
                self.text_line(rng, out);
            }
            8..=39 => {
                // A line end on the region's bottom row scrolls the region up.
                push_cursor_position(out, self.bottom, 1);
                self.text_line(rng, out);
            }
            40..=51 => {
                // A reverse index on the region's top row scrolls it down.
                push_cursor_position(out, self.top, 1);
                out.extend_from_slice(b"\x1bM");
                self.text_line(rng, out);
            }
            52..=63 => {
                push_cursor_position(out, rng.range(self.top, self.bottom), 1);
                out.extend_from_slice(b"\x1b[");
                push_number(out, rng.range(1, 5));
                out.push(if rng.chance(50) { b'L' } else { b'M' });
                self.text_line(rng, out);
            }
            64..=71 => {
                push_cursor_position(out, rng.range(self.top, self.bottom), rng.range(1, 200));
                out.extend_from_slice(rng.pick(&[
                    &b"\x1b[K"[..],
                    b"\x1b[K",
                    b"\x1b[1K",
                    b"\x1b[2K",
                    b"\x1b[2K",
                    b"\x1b[J",
                    b"\x1b[1J",
                ]));
                if rng.chance(5) {
                    out.extend_from_slice(b"\x1b[2J");
                }
                self.text_line(rng, out);
            }
            72..=79 => {
                out.extend_from_slice(b"\x1b7");
                for row in [1, 50] {
                    push_cursor_position(out, row, 1);
                    out.extend_from_slice(b"\x1b[7m");
                    let start = out.len();
                    push_ascii_text(rng, out, start, 200);
                    out.truncate(start + 200);
                    out.resize(start + 200, b' ');
                    out.extend_from_slice(b"\x1b[0m");
                }
                out.extend_from_slice(b"\x1b8");
                self.text_line(rng, out);
            }
            // A line where the last one left the cursor: after a step on the
            // bottom row, this keeps the region scrolling.
            _ => self.text_line(rng, out),
        }
    }

    /// Sets a new scroll region, which also moves the cursor home.
    fn set(&mut self, rng: &mut Rng, out: &mut Vec<u8>) {
        match rng.below(20) {
            0 => {
                (self.top, self.bottom) = (1, 50);
                out.extend_from_slice(b"\x1b[r");
                return;
            }
            1..=11 => (self.top, self.bottom) = (2, 49),
            _ => {
                self.top = rng.range(2, 25);
                self.bottom = rng.range(self.top + 4, 49);
            }
        }
        out.extend_from_slice(b"\x1b[");
        push_number(out, self.top);
        out.push(b';');
        push_number(out, self.bottom);
        out.push(b'r');
    }

    /// Pushes a line of text short enough not to wrap, sometimes with a
    /// coloured word, then a line end.
    fn text_line(&self, rng: &mut Rng, out: &mut Vec<u8>) {
        let start = out.len();
        if rng.chance(20) {
            out.extend_from_slice(b"\x1b[");
            push_number(out, 31 + rng.below(6));
            out.push(b'm');
            push_word(rng, out);
            out.extend_from_slice(b"\x1b[39m ");
        }
        let columns = rng.range(10, 150);
        push_ascii_text(rng, out, start, columns);
        out.push(b'\n');
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_payloads(seed: u64, size: usize) -> Vec<(Kind, Vec<u8>)> {
        Kind::ALL
            .into_iter()
            .map(|kind| (kind, generate(kind, seed, size)))
            .collect()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    /// Returns whether `payload` has a CSI sequence with digit parameters
    /// that ends in `last` and has `parameters` of them.
    fn has_csi(payload: &[u8], parameters: usize, last: u8) -> bool {
        payload.windows(2).enumerate().any(|(i, window)| {
            if window != b"\x1b[" {
                return false;
            }
            let rest = &payload[i + 2..];
            let end = rest
                .iter()
                .position(|b| !(b.is_ascii_digit() || *b == b';'))
                .unwrap_or(rest.len());
            let params = &rest[..end];
            rest.get(end) == Some(&last)
                && !params.is_empty()
                && params.split(|b| *b == b';').count() == parameters
                && params.split(|b| *b == b';').all(|p| !p.is_empty())
        })
    }

    fn fnv1a(bytes: &[u8]) -> u64 {
        bytes.iter().fold(0xCBF2_9CE4_8422_2325, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01B3)
        })
    }

    #[test]
    fn names_round_trip() {
        for kind in Kind::ALL {
            assert_eq!(Kind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(Kind::from_name("other"), None);
    }

    #[test]
    fn the_same_inputs_give_the_same_payload() {
        for kind in Kind::ALL {
            assert_eq!(generate(kind, 7, 100_000), generate(kind, 7, 100_000));
        }
    }

    #[test]
    fn different_seeds_give_different_payloads() {
        for kind in Kind::ALL {
            assert_ne!(generate(kind, 1, 100_000), generate(kind, 2, 100_000));
        }
    }

    #[test]
    fn a_payload_stops_at_the_first_line_end_after_its_size() {
        for seed in 0..20 {
            for (kind, payload) in all_payloads(seed, 50_000) {
                let body = &payload[..payload.len() - TAIL.len()];
                assert!(body.len() >= 50_000, "{kind:?}");
                assert!(body.len() < 50_000 + 8192, "{kind:?}: {}", body.len());
                assert_eq!(body.last(), Some(&b'\n'), "{kind:?}");
                let last_line_start = body[..body.len() - 1]
                    .iter()
                    .rposition(|b| *b == b'\n')
                    .map_or(0, |i| i + 1);
                assert!(last_line_start < 50_000, "{kind:?}");
            }
        }
        assert_eq!(generate(Kind::Ascii, 1, 0), TAIL);
    }

    #[test]
    fn every_payload_keeps_the_harness_rules() {
        for seed in 0..5 {
            for (kind, payload) in all_payloads(seed, 200_000) {
                assert!(std::str::from_utf8(&payload).is_ok(), "{kind:?}");
                assert!(payload.ends_with(TAIL), "{kind:?}");
                assert!(!payload.contains(&b'~'), "{kind:?}");
                assert!(!payload.contains(&b'\r'), "{kind:?}");
                assert!(!payload.contains(&b'\t'), "{kind:?}");
                assert!(!contains(&payload, b"tw-bench"), "{kind:?}");
                assert_only_allowed_sequences(kind, &payload);
            }
        }
    }

    fn assert_only_allowed_sequences(kind: Kind, payload: &[u8]) {
        let mut i = 0;
        while let Some(offset) = payload[i..].iter().position(|b| *b == 0x1b) {
            let start = i + offset;
            match payload.get(start + 1) {
                Some(b'M' | b'7' | b'8') => i = start + 2,
                Some(b'[') => {
                    let params = payload[start + 2..]
                        .iter()
                        .position(|b| !(b.is_ascii_digit() || *b == b';'))
                        .expect("a CSI sequence ends");
                    let last = payload[start + 2 + params];
                    assert!(
                        b"mKJHrLM".contains(&last),
                        "{kind:?}: CSI ending in {:?}",
                        char::from(last)
                    );
                    i = start + 3 + params;
                }
                other => panic!("{kind:?}: ESC followed by {other:?}"),
            }
        }
        for byte in payload {
            assert!(
                *byte >= 0x20 || *byte == b'\n' || *byte == 0x1b,
                "{kind:?}: control byte {byte:#04x}"
            );
        }
    }

    #[test]
    fn ascii_is_printable_text_with_empty_and_wrapping_lines() {
        let payload = generate(Kind::Ascii, 1, 200_000);
        let body = &payload[..payload.len() - TAIL.len()];
        assert!(
            body.iter()
                .all(|b| (0x20..=0x7D).contains(b) || *b == b'\n')
        );
        let lines: Vec<&[u8]> = body.split(|b| *b == b'\n').collect();
        assert!(lines.iter().any(|line| line.is_empty()));
        assert!(lines.iter().any(|line| line.len() > 200));
    }

    #[test]
    fn unicode_mixes_scripts_wide_characters_and_emoji_sequences() {
        let payload = generate(Kind::Unicode, 1, 65_536);
        let text = std::str::from_utf8(&payload).unwrap();
        let has = |test: fn(char) -> bool| text.chars().any(test);
        assert!(has(|c| ('\u{4E00}'..='\u{9FFF}').contains(&c)), "CJK");
        assert!(has(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)), "Hangul");
        assert!(
            has(|c| ('\u{0300}'..='\u{036F}').contains(&c)),
            "combining mark"
        );
        assert!(has(|c| ('\u{03B1}'..='\u{03C9}').contains(&c)), "Greek");
        assert!(has(|c| ('\u{0430}'..='\u{044F}').contains(&c)), "Cyrillic");
        assert!(
            has(|c| ('\u{05D0}'..='\u{064A}').contains(&c)),
            "Hebrew or Arabic"
        );
        assert!(has(|c| c == '\u{200D}'), "ZWJ");
        assert!(has(|c| c == '\u{FE0F}'), "VS16");
        assert!(
            has(|c| ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)),
            "skin tone"
        );
        assert!(has(|c| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)), "flag");
        let chars: Vec<char> = text.chars().collect();
        let mark = |c: char| ('\u{0300}'..='\u{036F}').contains(&c);
        assert!(
            chars
                .windows(3)
                .any(|w| w[0].is_ascii_lowercase() && mark(w[1]) && mark(w[2])),
            "stacked combining marks"
        );
    }

    #[test]
    fn color_is_escape_heavy() {
        let payload = generate(Kind::Color, 1, 65_536);
        assert!(contains(&payload, b"38;2;"));
        assert!(contains(&payload, b"48;2;"));
        assert!(contains(&payload, b"38;5;"));
        assert!(contains(&payload, b"48;5;"));
        assert!(contains(&payload, b"\x1b[K"));
        assert!(contains(&payload, b"\x1b[m"));
        assert!(contains(&payload, b"\x1b[39m"));
        assert!(contains(&payload, b"\x1b[49m"));
        let escapes = payload.iter().filter(|b| **b == 0x1b).count();
        assert!(escapes * 20 > payload.len(), "{escapes} escapes");
    }

    #[test]
    fn scroll_uses_regions_and_line_editing() {
        let payload = generate(Kind::Scroll, 1, 65_536);
        assert!(has_csi(&payload, 2, b'r'), "scroll region");
        assert!(has_csi(&payload, 2, b'H'), "cursor position");
        assert!(has_csi(&payload, 1, b'L'), "insert lines");
        assert!(has_csi(&payload, 1, b'M'), "delete lines");
        assert!(contains(&payload, b"\x1bM"), "reverse index");
        assert!(contains(&payload, b"\x1b7"), "save cursor");
        assert!(contains(&payload, b"\x1b8"), "restore cursor");
        assert!(contains(&payload, b"\x1b[7m"), "status line");
        assert!(contains(&payload, b"\x1b[2K"), "erase line");
    }

    /// These hashes pin the generator's output. A change to them changes
    /// every payload, so results measured before the change can no longer be
    /// compared with results measured after it: record the change with the
    /// results.
    #[test]
    fn the_payloads_are_pinned() {
        let hashes: Vec<(Kind, u64)> = Kind::ALL
            .into_iter()
            .map(|kind| (kind, fnv1a(&generate(kind, 1, 65_536))))
            .collect();
        assert_eq!(
            hashes,
            [
                (Kind::Ascii, 0xE68D_4570_1323_7B23),
                (Kind::Unicode, 0x5B05_4FFB_FBDE_7440),
                (Kind::Color, 0xECAD_6EC9_E765_B153),
                (Kind::Scroll, 0xA68C_650E_A5DE_CFD4),
            ]
        );
    }
}
