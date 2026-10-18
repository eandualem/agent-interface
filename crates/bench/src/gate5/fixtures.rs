//! The programs gate 5 rebuilds panes from, and the differences each may show
//! (`docs/decisions/0021-pane-state-recovery.md`).
//!
//! Each fixture is two pieces of shell: `draw` leaves the pane in the state to
//! rebuild, and `more` writes further output after the rebuild, so that state
//! a capture leaves out shows up. Fixtures that draw in a 40-column pane say
//! so in their escape sequences.

use super::Comparison;
use super::compare::Difference;
use super::compare::Property::{self, Background, Foreground, Link, Text, Width};

/// A fixture.
pub struct Fixture {
    /// The fixture's name in reports.
    pub name: &'static str,
    /// Shell commands that draw the state to rebuild.
    pub draw: &'static str,
    /// Shell commands that write more output after the rebuild.
    pub more: &'static str,
    /// Which scenarios run it.
    pub runs: Runs,
    /// The differences it is expected to show, each explained.
    pub expected: &'static [Expected],
}

/// Which scenarios run a fixture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    /// Attach and pause, in windows with one pane and with several.
    Everywhere,
    /// Attach and pause in windows with one pane only: characters it draws
    /// grow after they are drawn, which tmux 3.7 does not redraw for an
    /// ordinary client of a pane narrower than its window.
    OnePane,
    /// The resize scenario only, drawing before the harness resizes.
    Resize,
    /// The resize scenario only, drawing while the harness resizes.
    ResizeWhileDrawing,
}

/// A difference a fixture is expected to show.
#[derive(Clone, Copy, Debug)]
pub struct Expected {
    /// When: after the rebuild, after more output, or both.
    pub after: After,
    /// The tmux versions it applies to, from `from` up to but not including
    /// `until`, as (major, minor).
    pub from: (u32, u32),
    /// See `from`.
    pub until: (u32, u32),
    /// What may differ: each cell that differs must be one of these allows.
    pub allows: &'static [Allows],
    /// What explains it.
    pub why: Why,
}

/// What an expected difference may change: any other difference in the
/// same comparison is unexplained.
#[derive(Clone, Copy, Debug)]
pub struct Allows {
    /// The properties of a cell that may differ.
    pub properties: &'static [Property],
    /// The cells that may differ, in reading order from the first (row,
    /// column) up to but not including the second; `None` for any cell.
    pub cells: Option<((u16, u16), (u16, u16))>,
    /// Whether the differing cells of each row must reach its last column,
    /// as when a capture ends a row early. Later output may have written
    /// over cells in that tail.
    pub tail: bool,
    /// Whether the cursor's position may differ.
    pub cursor: bool,
    /// Whether the properties may differ only by the candidate's cell
    /// lacking them, as when a limit leaves out a link or a colour.
    pub lost_only: bool,
}

/// When a comparison is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum After {
    /// Right after the rebuild.
    Rebuild,
    /// After the fixture's further output and the input sent to it.
    More,
    /// Both.
    Both,
}

/// What explains an expected difference.
#[derive(Clone, Copy, Debug)]
pub enum Why {
    /// A limit decision record 0021 lists: the gate passes.
    Limit(&'static str),
}

const ALL: ((u32, u32), (u32, u32)) = ((0, 0), (u32::MAX, 0));

const fn expect(
    after: After,
    versions: ((u32, u32), (u32, u32)),
    allows: &'static [Allows],
    why: Why,
) -> Expected {
    Expected {
        after,
        from: versions.0,
        until: versions.1,
        allows,
        why,
    }
}

/// The cells from `from` up to but not including `until`, each a (row,
/// column).
type Cells = Option<((u16, u16), (u16, u16))>;

/// Allows `properties` of `cells` to differ, and the cursor's position if
/// `cursor`.
const fn allows(properties: &'static [Property], cells: Cells, cursor: bool) -> Allows {
    Allows {
        properties,
        cells,
        tail: false,
        cursor,
        lost_only: false,
    }
}

/// Allows the candidate's `cells` to lack `properties`.
const fn lacks(properties: &'static [Property], cells: Cells) -> Allows {
    Allows {
        properties,
        cells,
        tail: false,
        cursor: false,
        lost_only: true,
    }
}

impl Allows {
    /// Requires the differing cells of each row to reach its end.
    const fn tail(self) -> Self {
        Self { tail: true, ..self }
    }
}

/// Every fixture.
pub const FIXTURES: &[Fixture] = &[
    Fixture {
        name: "styles",
        draw: r"printf '\033[1mB\033[22m \033[2mD\033[22m \033[3mI\033[23m \033[4mU\033[4:3mC\033[24m \033[5mK\033[25m \033[7mV\033[27m \033[8mH\033[28m \033[9mS\033[29m \033[53mO\033[55m \033[58;5;196;4mL\033[59;24m \033[91mb\033[38;5;123mx\033[38;2;1;2;3mr\033[0m\n'",
        more: r"printf 'after\n'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "background-erase",
        draw: r"printf 'a\033[44m\033[K\033[0m\nb\033[41m   \033[0m\n\033[42m\033[2K\033[0mc   \n'",
        more: r"printf '\033[1;3Hz'",
        runs: Runs::Everywhere,
        expected: &[expect(
            After::Both,
            ((0, 0), (3, 4)),
            &[lacks(&[Background], Some(((0, 0), (3, 0)))).tail()],
            Why::Limit("tmux 3.3 captures a row only up to its last written cell"),
        )],
    },
    Fixture {
        name: "wide",
        draw: r"printf '\347\225\214e\314\201\360\237\221\251\342\200\215\360\237\222\273x\n'",
        more: r"printf '\342\232\240\357\270\217y\n'",
        runs: Runs::OnePane,
        expected: &[],
    },
    Fixture {
        name: "line-drawing",
        draw: r"printf '\033(0lqk\033(B \033)0\016x\017\n'",
        more: r"printf 'q\n'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "history",
        draw: "seq 1 30",
        more: r"printf 'more\n'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "alternate",
        draw: r"printf 'P1\nP2\nP3'; printf '\033[?1049h\033[2;5r\033[?1h\033=\033[?2004h\033[4h\033[3g\033[1;6H\033H\033[1;20H\033H'; printf '\033[1;1HALT\033[5;1Hbottom\033[3;3H'",
        more: r"printf 'ins\033[5;1H\nscrolled\ta\tb'; sleep 0.2; printf '\033[?1049lback'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "pending-csi",
        draw: r"printf 'pre\033[3'",
        more: r"printf '1mRED'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "pending-csi-line-feed",
        draw: r"printf 'top\n\033[\n'",
        more: r"printf '31mX'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "pending-osc",
        draw: r"printf 'pre\033]0;tit'",
        more: r"printf 'le\033\\post'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "pending-wrap-backspace",
        draw: r"printf '\033[1;40HW'",
        more: r"printf '\b\b\b\b\bX'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "pending-wrap-left",
        draw: r"printf '\033[1;40HW'",
        more: r"printf '\033[5DX'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "pending-wrap-nowrap",
        draw: r"printf '\033[1;40HW'",
        more: r"printf '\033[?7lx'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "pending-wrap",
        draw: r"printf '\033[1;40HW'",
        more: r"printf 'N'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "wide-pending-wrap",
        draw: r"printf '\033[1;39H\347\225\214'",
        more: r"printf 'N'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "origin-no-wrap",
        draw: r"printf '\033[3;6r\033[?6h\033[?7l\033[2;38Habcdef'",
        more: r"printf '\033[1;1Ho\033[4;1H\nscroll'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "links",
        draw: r"printf 'a\033]8;id=k;http://e.x/1\007LINK\033]8;;\007 b\033]8;;http://e.x/2\007two\033]8;;\007\n'",
        more: r"printf '\033]8;;http://e.x/3\007three\033]8;;\007\n'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "link-wrap",
        draw: r"printf '\033]8;;http://e.x/w\007abcdefghijabcdefghijabcdefghijabcdefghijabcdefghij\033]8;;\007\n'",
        more: r"printf 'x\n'",
        runs: Runs::Everywhere,
        expected: &[expect(
            After::Both,
            ((3, 4), (u32::MAX, 0)),
            &[lacks(&[Link], Some(((1, 0), (2, 0))))],
            Why::Limit("a capture does not reopen a link that continues onto the next row"),
        )],
    },
    Fixture {
        name: "tabs",
        draw: r"printf '\033[44m\033[2K\ta\tb\033[0m\n\033[41m\t\033[0mc\n'",
        more: r"printf '\tx\n'",
        runs: Runs::Everywhere,
        // The row is erased in blue so that tab cells show; tmux 3.3 drops
        // the erased cells after the last one written.
        expected: &[expect(
            After::Both,
            ((0, 0), (3, 4)),
            &[lacks(&[Background], Some(((0, 17), (1, 0)))).tail()],
            Why::Limit("tmux 3.3 captures a row only up to its last written cell"),
        )],
    },
    Fixture {
        name: "tab-moved",
        draw: r"printf '\033[44m\033[2K\tX\033[0m\033[1;1H\033[2@'",
        more: r"printf '\033[3;1Hz'",
        runs: Runs::Everywhere,
        expected: &[expect(
            After::Both,
            ((3, 6), (u32::MAX, 0)),
            &[
                allows(&[Text], Some(((0, 8), (0, 9))), false),
                allows(&[Text], Some(((0, 10), (0, 11))), false),
                lacks(&[Background], Some(((0, 38), (1, 0)))),
            ],
            Why::Limit(
                "a tab cell moved since it was written is measured against the current tab stops",
            ),
        )],
    },
    Fixture {
        name: "underline-colours",
        draw: r"printf '\033[4;58;5;196mA\033[58;2;1;2;3mB\033[59mC\033[0m\n'",
        more: r"printf 'x\n'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "current-style",
        draw: r"printf 'x\033[31;44m'",
        more: r"printf 'after'",
        runs: Runs::Everywhere,
        expected: &[expect(
            After::More,
            ALL,
            &[lacks(&[Foreground, Background], Some(((0, 1), (2, 0))))],
            Why::Limit("the current text style is not exported"),
        )],
    },
    Fixture {
        name: "partial-utf8",
        draw: r"printf 'pre\347\225'",
        more: r"printf '\214post'",
        runs: Runs::Everywhere,
        expected: &[expect(
            After::More,
            ALL,
            // The wide character, then "post" and the echoed "input" it
            // shifts.
            &[
                allows(&[Text, Width], Some(((0, 3), (0, 5))), false),
                allows(&[Text], Some(((0, 5), (0, 14))), false),
            ],
            Why::Limit("a partly received UTF-8 character is not exported"),
        )],
    },
    Fixture {
        name: "saved-cursor",
        draw: r"printf '\033[2;3H\033[32m\0337\033[0m\033[6;6Hz'",
        more: r"printf '\0338X'",
        runs: Runs::Everywhere,
        expected: &[expect(
            After::More,
            ALL,
            // The candidate writes "X" and the echoed "input" from the top
            // left in the default colour; the reference writes them at the
            // saved position in green, then "<input>" on the next row.
            &[
                allows(&[Text], Some(((0, 0), (0, 6))), true),
                allows(&[Text], Some(((1, 0), (1, 2))), false),
                allows(&[Text, Foreground], Some(((1, 2), (1, 8))), false),
                allows(&[Text, Foreground], Some(((2, 0), (2, 7))), false),
            ],
            Why::Limit("the general saved cursor (DECSC) is not exported"),
        )],
    },
    Fixture {
        name: "still-line-drawing",
        draw: r"printf '\033(0lq'",
        more: r"printf 'qk\033(B\n'",
        runs: Runs::Everywhere,
        expected: &[expect(
            After::More,
            ALL,
            &[allows(&[Text], Some(((0, 2), (0, 4))), false)],
            Why::Limit("the character set in use is not exported"),
        )],
    },
    Fixture {
        name: "screen-title",
        draw: r"printf 'a'",
        more: r"printf '\033kname\033\\b'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "screen-title-pending",
        draw: r"printf 'a\033kna'",
        more: r"printf 'me\033\\b'",
        runs: Runs::Everywhere,
        expected: &[],
    },
    Fixture {
        name: "reflow",
        draw: r"seq 1 12; printf 'a long line of text that is longer than thirty columns\n'; printf '\033[32mgreen and also long enough to wrap at thirty\033[0m\n$ partial'",
        more: r"printf ' more\nafter\n'",
        runs: Runs::Resize,
        expected: &[],
    },
    Fixture {
        name: "alternate-resize",
        draw: r"printf 'P1 0123456789012345678901234567890123456\nP2\033[?1049h\033[HALT'",
        more: r"printf '\033[?1049lback\n'",
        runs: Runs::Resize,
        expected: &[expect(
            After::More,
            ALL,
            &[allows(&[Text], None, true)],
            Why::Limit("a capture returns the saved primary screen at the current width"),
        )],
    },
    Fixture {
        name: "nowrap-margin",
        draw: r"printf '\033[?1049h\033[?7l\033[6;39H'",
        more: r"printf 'AB'",
        runs: Runs::Resize,
        expected: &[],
    },
    Fixture {
        name: "offgrid-print",
        draw: r"printf '\033[?1049h\033[6;40HR'",
        more: r"printf 'x'",
        runs: Runs::Resize,
        expected: &[],
    },
    Fixture {
        name: "offgrid-backspace",
        draw: r"printf '\033[?1049h\033[6;40HR'",
        more: r"printf '\b\b\b\b\bX'",
        runs: Runs::Resize,
        expected: &[],
    },
    Fixture {
        name: "offgrid-left",
        draw: r"printf '\033[?1049h\033[6;40HR'",
        more: r"printf '\033[5DX'",
        runs: Runs::Resize,
        expected: &[],
    },
    Fixture {
        name: "offgrid-nowrap",
        draw: r"printf '\033[?1049h\033[6;40HR'",
        more: r"printf '\033[?7lx'",
        runs: Runs::Resize,
        expected: &[],
    },
    Fixture {
        name: "redraw",
        draw: r#"draw() { set -- $(stty size); printf '\033[2J\033[H+%s+' "$2x$1"; printf '\033[%d;1H-bottom-\033[%d;%dHR' "$1" "$1" "$2"; }; printf '\033[?1049h'; trap draw WINCH; draw; while [ ! -e "$stop" ]; do sleep 0.05; done"#,
        more: r"printf 'x'",
        runs: Runs::ResizeWhileDrawing,
        expected: &[],
    },
    Fixture {
        name: "stream",
        draw: r#"i=0; while [ $i -lt 300 ]; do echo "line $i with some text to wrap around"; i=$((i+1)); sleep 0.005; done"#,
        more: r"printf 'end\n'",
        runs: Runs::ResizeWhileDrawing,
        expected: &[],
    },
];

impl Fixture {
    /// Returns the difference this fixture is expected to show `after` a
    /// comparison on tmux `version`, if one is.
    pub fn expected(&self, after: After, version: (u32, u32)) -> Option<&Expected> {
        self.expected.iter().find(|expected| {
            (expected.after == after || expected.after == After::Both)
                && expected.from <= version
                && version < expected.until
        })
    }
}

impl Expected {
    /// Whether this explains every difference of `comparison`.
    pub fn explains(&self, comparison: &Comparison) -> bool {
        comparison.notes.is_empty()
            && (comparison.cursor.is_none() || self.allows.iter().any(|allows| allows.cursor))
            && comparison.cells.iter().all(|cell| {
                self.allows.iter().any(|allows| {
                    allows.covers(cell) && (!allows.tail || ends_row(comparison, cell.row))
                })
            })
    }
}

impl Allows {
    /// Whether `cell` differs only as this allows.
    fn covers(&self, cell: &Difference) -> bool {
        self.cells
            .is_none_or(|(from, until)| (from..until).contains(&(cell.row, cell.col)))
            && cell.properties.iter().all(|property| {
                self.properties.contains(property)
                    && (!self.lost_only || cell.lost.contains(property))
            })
    }
}

/// Whether `row` differs in its last column.
fn ends_row(comparison: &Comparison, row: u16) -> bool {
    let last = comparison.width.checked_sub(1);
    comparison
        .cells
        .iter()
        .any(|cell| cell.row == row && Some(cell.col) == last)
}

/// Reads a tmux version such as `3.3a`, `3.8` or `next-3.9` as (major,
/// minor).
pub fn version(text: &str) -> Option<(u32, u32)> {
    let digits = text
        .trim()
        .trim_start_matches(|c: char| !c.is_ascii_digit());
    let (major, rest) = digits.split_once('.')?;
    let minor: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some((major.parse().ok()?, minor.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_tmux_versions() {
        assert_eq!(version("3.3a"), Some((3, 3)));
        assert_eq!(version("3.8\n"), Some((3, 8)));
        assert_eq!(version("next-3.9"), Some((3, 9)));
        assert_eq!(version("master"), None);
    }

    #[test]
    fn expected_differences_hold_for_their_versions_and_comparisons() {
        let find = |name| {
            FIXTURES
                .iter()
                .find(|fixture| fixture.name == name)
                .unwrap()
        };
        let erase = find("background-erase");
        assert!(erase.expected(After::Rebuild, (3, 3)).is_some());
        assert!(erase.expected(After::More, (3, 3)).is_some());
        assert!(erase.expected(After::Rebuild, (3, 4)).is_none());
        let style = find("current-style");
        assert!(style.expected(After::Rebuild, (3, 8)).is_none());
        assert!(style.expected(After::More, (3, 8)).is_some());
        assert!(find("styles").expected(After::More, (3, 8)).is_none());
        let names: Vec<_> = FIXTURES.iter().map(|fixture| fixture.name).collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(names.len(), unique.len());
    }

    #[test]
    fn an_expected_difference_explains_only_what_it_allows() {
        let link = FIXTURES
            .iter()
            .find(|fixture| fixture.name == "link-wrap")
            .unwrap()
            .expected(After::Rebuild, (3, 8))
            .unwrap();
        let cell = |row, properties: &[Property]| super::super::compare::Difference {
            row,
            col: 0,
            properties: properties.to_vec(),
            lost: properties.to_vec(),
            reference: String::new(),
            candidate: String::new(),
        };
        let with = |cells| Comparison {
            cells,
            ..Comparison::default()
        };
        assert!(link.explains(&with(vec![cell(1, &[Link])])));
        assert!(!link.explains(&with(vec![cell(1, &[Link, Text])])));
        assert!(!link.explains(&with(vec![cell(0, &[Link])])));
        // A different link is not a missing one.
        let mut other_link = cell(1, &[Link]);
        other_link.lost.clear();
        assert!(!link.explains(&with(vec![other_link])));
        let mut moved = with(vec![cell(1, &[Link])]);
        moved.cursor = Some("moved".to_owned());
        assert!(!link.explains(&moved));
        let mut resized = with(Vec::new());
        resized.notes.push("resized".to_owned());
        assert!(!link.explains(&resized));
        // tmux 3.3 drops a row's erased tail, not cells inside it.
        let erase = FIXTURES
            .iter()
            .find(|fixture| fixture.name == "background-erase")
            .unwrap()
            .expected(After::Rebuild, (3, 3))
            .unwrap();
        let at = |col| {
            let mut cell = cell(0, &[Background]);
            cell.col = col;
            cell
        };
        let mut tail = with((30..40).map(at).collect());
        tail.width = 40;
        assert!(erase.explains(&tail));
        let mut inside = with((1..4).map(at).collect());
        inside.width = 40;
        assert!(!erase.explains(&inside));
        // Nor the colour of a written character beside a dropped tail.
        let tabs = FIXTURES
            .iter()
            .find(|fixture| fixture.name == "tabs")
            .unwrap()
            .expected(After::Rebuild, (3, 3))
            .unwrap();
        let mut beside = with((17..40).map(at).collect());
        beside.width = 40;
        assert!(tabs.explains(&beside));
        beside.cells.push(at(16));
        assert!(!tabs.explains(&beside));
        // A limit with two regions explains each only with its own
        // properties.
        let moved = FIXTURES
            .iter()
            .find(|fixture| fixture.name == "tab-moved")
            .unwrap()
            .expected(After::Rebuild, (3, 8))
            .unwrap();
        let mut text = cell(0, &[Text]);
        text.col = 8;
        assert!(moved.explains(&with(vec![text.clone(), at(38)])));
        text.col = 9;
        assert!(!moved.explains(&with(vec![text.clone()])));
        text.col = 25;
        assert!(!moved.explains(&with(vec![text.clone()])));
        // Nor text past the shifted output of a partly received character.
        let utf8 = FIXTURES
            .iter()
            .find(|fixture| fixture.name == "partial-utf8")
            .unwrap()
            .expected(After::More, (3, 8))
            .unwrap();
        text.col = 13;
        assert!(utf8.explains(&with(vec![text.clone()])));
        text.col = 14;
        assert!(!utf8.explains(&with(vec![text])));
        // Nor a colour where only the saved cursor's text moved.
        let saved = FIXTURES
            .iter()
            .find(|fixture| fixture.name == "saved-cursor")
            .unwrap()
            .expected(After::More, (3, 8))
            .unwrap();
        assert!(saved.explains(&with(vec![cell(1, &[Text])])));
        assert!(!saved.explains(&with(vec![cell(1, &[Text, Foreground])])));
    }
}
