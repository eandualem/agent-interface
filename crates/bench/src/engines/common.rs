//! Output the engines agree on, and the comparison the cross-engine tests
//! use. `alacritty_terminal` is the reference for `rio-vt`, because `rio-vt`
//! cannot share a program with `libghostty-vt`; `tmux-workspaces-bench-rio`
//! includes this file for its tests.

use crate::display::Screen;
use crate::engine::Engine;

/// Inputs on which the engines agree, cell by cell, at 20 by 6.
pub const INPUTS: &[(&str, &[u8])] = &[
    (
        "ASCII with wrapping",
        b"hello world\r\nthis line is longer than twenty columns and wraps\r\nend",
    ),
    (
        "16, 256 and direct colours with a background-colour erase",
        b"\x1b[31mred\x1b[0m \x1b[92;44mbright\x1b[0m \x1b[38;5;200mpink\x1b[0m\r\n\
          \x1b[38;2;10;20;30;48;2;200;100;50mdirect\x1b[0m\r\n\
          \x1b[48;5;22m\x1b[Kerased\x1b[0m\r\n\
          \x1b[1;2;3;4;7;8;9mattrs\x1b[0m \x1b[4:3mcurly\x1b[0m \x1b[4:2mdouble\x1b[0m",
    ),
    (
        "wide CJK, including one that wraps",
        "漢字かなカナ한국어\r\n0123456789012345678漢".as_bytes(),
    ),
    ("a combining mark", "cafe\u{301} na\u{308}ive".as_bytes()),
    (
        "a scroll region with line feeds and reverse index",
        b"top\r\n1\r\n2\r\n3\r\n4\r\nbottom\x1b[2;5r\x1b[5;1Ha\nb\nc\x1b[2;1H\x1bMup\x1b[r\x1b[6;3H",
    ),
];

/// Feeds `bytes` to a new 20 by 6 terminal and reads its screen.
pub fn read<E: Engine>(bytes: &[u8]) -> Screen {
    let mut engine = E::new(20, 6, 100);
    engine.write(bytes);
    let mut screen = Screen::default();
    engine.read_screen(&mut screen);
    screen
}

/// Asserts two screens match cell by cell, treating "" and " " as the same
/// text, since some engines store a space in blank cells.
pub fn assert_same_screen(input: &str, expected: &Screen, actual: &Screen) {
    assert_eq!(
        (actual.cols(), actual.rows()),
        (expected.cols(), expected.rows())
    );
    for row in 0..expected.rows() {
        for col in 0..expected.cols() {
            let (want_text, want) = expected.cell(row, col).unwrap();
            let (got_text, got) = actual.cell(row, col).unwrap();
            assert_eq!(
                (blank(got_text), got.width, got.style),
                (blank(want_text), want.width, want.style),
                "row {row}, column {col} of {input}"
            );
        }
    }
    assert_eq!(actual.cursor(), expected.cursor(), "cursor of {input}");
}

fn blank(text: &str) -> &str {
    if text == " " { "" } else { text }
}
