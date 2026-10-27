//! Reads the captures recorded from each tmux version in `tests/captures`
//! (recorded by `examples/record-captures.rs`).

use std::path::{Path, PathBuf};

use agent_interface_tmux::capture::{self, Attributes, Color, Link, Row, Style, Underline};

/// Every recorded version's directory and its version string.
fn versions() -> Vec<(String, PathBuf)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/captures");
    let mut versions: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                path,
            )
        })
        .collect();
    versions.sort();
    assert!(versions.len() >= 2, "captures from at least 3.3 and 3.8");
    versions
}

fn read(directory: &Path, name: &str) -> Vec<u8> {
    std::fs::read(directory.join(format!("{name}.txt"))).unwrap()
}

/// The recorded reply lines: one per row of the 20x5 fixture pane, each
/// ended by a line feed in the file.
fn rows(directory: &Path, name: &str) -> Vec<Row> {
    let capture = read(directory, name);
    let lines = capture.strip_suffix(b"\n").unwrap_or(&capture);
    let rows = capture::rows(lines.split(|&byte| byte == b'\n')).unwrap();
    assert_eq!(rows.len(), 5, "{}/{name}", directory.display());
    rows
}

/// Whether the version is at least `major.minor`.
fn at_least(version: &str, major: u32, minor: u32) -> bool {
    let mut numbers = version
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse::<u32>().ok());
    (numbers.next().unwrap(), numbers.next().unwrap()) >= (major, minor)
}

/// A row's cells as (character, style, link, line drawing), with blank
/// default cells at the end left out: later versions capture them.
fn cells(row: &Row) -> Vec<(char, Style, Option<Link>, bool)> {
    let mut cells: Vec<_> = row
        .segments
        .iter()
        .flat_map(|segment| {
            segment
                .text
                .chars()
                .map(|c| (c, segment.style, segment.link.clone(), segment.line_drawing))
        })
        .collect();
    while cells.last().is_some_and(|(c, style, link, _)| {
        *c == ' ' && *style == Style::default() && link.is_none()
    }) {
        cells.pop();
    }
    cells
}

fn text(row: &Row) -> String {
    cells(row).into_iter().map(|(c, ..)| c).collect()
}

fn style(attributes: &[Attributes]) -> Style {
    let mut style = Style::default();
    for &attribute in attributes {
        style.attributes.insert(attribute);
    }
    style
}

#[test]
fn reads_every_attribute_underline_and_colour() {
    let underline = |kind, color| Style {
        underline: kind,
        underline_color: color,
        ..Style::default()
    };
    let expected = [
        ('B', style(&[Attributes::BOLD])),
        ('D', style(&[Attributes::FAINT])),
        ('I', style(&[Attributes::ITALIC])),
        ('U', underline(Underline::Single, Color::Default)),
        ('W', underline(Underline::Double, Color::Default)),
        ('C', underline(Underline::Curly, Color::Default)),
        ('T', underline(Underline::Dotted, Color::Default)),
        ('H', underline(Underline::Dashed, Color::Default)),
        ('K', style(&[Attributes::BLINK])),
        ('V', style(&[Attributes::INVERSE])),
        ('X', style(&[Attributes::INVISIBLE])),
        ('S', style(&[Attributes::STRIKETHROUGH])),
        // tmux writes the overline as 5:3, which is not blink.
        ('O', style(&[Attributes::OVERLINE])),
        ('L', underline(Underline::Single, Color::Palette(196))),
        ('R', underline(Underline::Single, Color::Rgb(1, 2, 3))),
    ];
    let fg = |color| Style {
        foreground: color,
        ..Style::default()
    };
    let bg = |color| Style {
        background: color,
        ..Style::default()
    };
    let colours = [
        ('a', fg(Color::Palette(1))),
        ('b', fg(Color::Palette(9))),
        ('c', fg(Color::Palette(123))),
        ('d', fg(Color::Rgb(10, 20, 30))),
        ('e', bg(Color::Palette(4))),
        ('f', bg(Color::Palette(12))),
        ('g', bg(Color::Palette(200))),
        ('h', bg(Color::Rgb(4, 5, 6))),
    ];
    for (version, directory) in versions() {
        let rows = rows(&directory, "styles");
        for (row, expected) in [(0, &expected[..]), (1, &colours[..])] {
            let got: Vec<_> = cells(&rows[row])
                .into_iter()
                .map(|(c, s, ..)| (c, s))
                .collect();
            assert_eq!(got, expected, "tmux {version}, row {row}");
        }
    }
}

#[test]
fn the_style_carries_into_the_next_row() {
    let green = Style {
        foreground: Color::Palette(2),
        ..style(&[Attributes::BOLD])
    };
    for (version, directory) in versions() {
        let rows = rows(&directory, "carry");
        let row: Vec<_> = cells(&rows[1])
            .into_iter()
            .map(|(c, s, ..)| (c, s))
            .collect();
        let mut expected: Vec<_> = "still".chars().map(|c| (c, green)).collect();
        expected.extend(" plain".chars().map(|c| (c, Style::default())));
        assert_eq!(row, expected, "tmux {version}");
    }
}

#[test]
fn reads_line_drawing_across_rows() {
    for (version, directory) in versions() {
        let rows = rows(&directory, "line-drawing");
        let drawn = |row: &Row| -> Vec<(char, bool)> {
            cells(row).into_iter().map(|(c, _, _, d)| (c, d)).collect()
        };
        assert_eq!(
            drawn(&rows[0]),
            "lqqk".chars().map(|c| (c, true)).collect::<Vec<_>>()
        );
        let mut expected: Vec<_> = "x  x".chars().map(|c| (c, true)).collect();
        expected.extend(" ok".chars().map(|c| (c, false)));
        assert_eq!(drawn(&rows[1]), expected, "tmux {version}");
    }
}

#[test]
fn reads_hyperlinks_and_loses_a_link_that_continues_onto_the_next_row() {
    let a = Link {
        params: "".into(),
        uri: "http://a.example/".into(),
    };
    let shared = a.clone();
    assert!(std::sync::Arc::ptr_eq(&a.uri, &shared.uri));
    let b = Link {
        params: "id=k".into(),
        uri: "http://b.example/".into(),
    };
    for (version, directory) in versions() {
        let rows = rows(&directory, "links");
        let links: Vec<_> = cells(&rows[0])
            .into_iter()
            .map(|(_, _, link, _)| link)
            .collect();
        assert_eq!(text(&rows[0]), "one two 012345678901", "tmux {version}");
        let mut expected = vec![None; 20];
        if at_least(&version, 3, 4) {
            expected[..3].fill(Some(a.clone()));
            expected[8..].fill(Some(b.clone()));
        }
        assert_eq!(links, expected, "tmux {version}");
        // tmux closes the link at the end of the row and does not open it
        // again where it continues.
        assert!(cells(&rows[1]).iter().all(|(_, _, link, _)| link.is_none()));
        assert_eq!(text(&rows[1]), "2345678901234 end", "tmux {version}");
    }
}

#[test]
fn keeps_grapheme_clusters_and_text_that_looks_like_protocol() {
    for (version, directory) in versions() {
        let wide = text(&rows(&directory, "wide")[0]).replace(' ', "");
        assert_eq!(
            wide, "\u{754c}e\u{301}\u{1f469}\u{200d}\u{1f4bb}\u{26a0}\u{fe0f}x",
            "tmux {version}"
        );
        let rows = rows(&directory, "text");
        assert_eq!(text(&rows[0]), "%begin 1 2 3");
        assert_eq!(text(&rows[1]), "back\\slash 100%", "tmux {version}");
    }
}

#[test]
fn reads_tab_cells_from_3_6_and_background_erases() {
    let blue = Style {
        background: Color::Palette(4),
        ..Style::default()
    };
    for (version, directory) in versions() {
        let tabs = &rows(&directory, "tabs")[0];
        let got: Vec<_> = cells(tabs).into_iter().map(|(c, s, ..)| (c, s)).collect();
        // A tab over blank cells takes the style of the cells it covers.
        let expected: Vec<_> = if at_least(&version, 3, 6) {
            vec![
                ('a', Style::default()),
                ('\t', blue),
                ('b', Style::default()),
                ('\t', blue),
                ('c', Style::default()),
                (' ', blue),
                (' ', blue),
                (' ', blue),
            ]
        } else {
            let mut cells = vec![('a', Style::default())];
            cells.extend([(' ', blue); 7]);
            cells.push(('b', Style::default()));
            cells.extend([(' ', blue); 7]);
            cells.push(('c', Style::default()));
            if at_least(&version, 3, 4) {
                cells.extend([(' ', blue); 3]);
            }
            cells
        };
        assert_eq!(got, expected, "tmux {version}");

        let erase = rows(&directory, "background-erase");
        // tmux 3.3 captures a row only up to its last written cell.
        let tail = if at_least(&version, 3, 4) { 19 } else { 0 };
        assert_eq!(cells(&erase[0]).len(), 1 + tail, "tmux {version}");
        assert!(
            cells(&erase[0])[1..]
                .iter()
                .all(|(c, s, ..)| *c == ' ' && *s == blue)
        );
        let red = Style {
            background: Color::Palette(1),
            ..Style::default()
        };
        assert_eq!(
            &cells(&erase[1])[1..]
                .iter()
                .map(|(_, s, ..)| *s)
                .collect::<Vec<_>>(),
            &[red; 3]
        );
    }
}

#[test]
fn reads_the_alternate_and_saved_screens() {
    let red = Style {
        foreground: Color::Palette(1),
        ..style(&[Attributes::BOLD])
    };
    for (version, directory) in versions() {
        let alternate = rows(&directory, "alternate");
        assert_eq!(text(&alternate[0]), "ALT", "tmux {version}");
        assert_eq!(cells(&alternate[0])[0].1, red);
        let saved = rows(&directory, "alternate-saved");
        assert_eq!(
            saved[..3].iter().map(text).collect::<Vec<_>>(),
            ["P1", "P2", "P3"],
            "tmux {version}"
        );
        assert_eq!(cells(&saved[2])[0].1, red);
    }
}

#[test]
fn pending_bytes_drop_controls_tmux_has_already_acted_on() {
    for (version, directory) in versions() {
        let pending = |name| {
            let capture = read(&directory, name);
            let lines = capture.strip_suffix(b"\n").unwrap_or(&capture);
            capture::pending(lines.split(|&byte| byte == b'\n')).unwrap()
        };
        assert_eq!(pending("pending-csi"), b"\x1b[3", "tmux {version}");
        assert_eq!(
            pending("pending-csi-line-feed"),
            b"\x1b[12",
            "tmux {version}"
        );
        assert_eq!(
            pending("pending-osc"),
            b"\x1b]0;ti\r\ntle",
            "tmux {version}"
        );
        // The line end came after `ESC [` interrupted the OSC.
        assert_eq!(
            pending("pending-osc-csi"),
            b"\x1b]0;ti\x1b[12",
            "tmux {version}"
        );
    }
}

#[test]
fn pending_bytes_follow_the_parser_from_one_sequence_into_the_next() {
    let pending = |line: &[u8]| capture::pending([line]).unwrap();
    // A control sequence that a string sequence interrupts: tmux ignored the
    // line feed inside the OSC.
    assert_eq!(pending(b"\\033[1\\033]0;a\\012b"), b"\x1b[1\x1b]0;a\nb");
    // A DCS's data keeps every byte, escape included, until its terminator.
    assert_eq!(
        pending(b"\\033P1$rx\\012\\033y\\012"),
        b"\x1bP1$rx\n\x1by\n"
    );
    // An escape sequence with an intermediate byte executes controls.
    assert_eq!(pending(b"\\033(\\012"), b"\x1b(");
    // Nothing pending.
    assert_eq!(pending(b""), b"");
}

#[test]
fn reads_a_backslash_with_a_combining_mark_as_it_is() {
    for (version, directory) in versions() {
        let rows = rows(&directory, "backslash-combining");
        assert_eq!(text(&rows[0]), "\\\u{301}\\x", "tmux {version}");
    }
}

#[test]
fn rejects_what_tmux_does_not_capture() {
    for line in [
        &b"a\\x"[..],
        b"a\\01",
        b"\\033[2J",
        b"\\033]0;title\\033\\\\",
        b"\\033(0",
        b"\\033[1",
        b"bell\\007",
    ] {
        assert!(
            capture::rows([line]).is_err(),
            "{}",
            String::from_utf8_lossy(line)
        );
    }
}
