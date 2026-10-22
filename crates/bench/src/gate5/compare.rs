//! Comparing a rebuilt pane's cells with the reference's, by the rules of
//! decision record 0021.

use std::fmt::Write as _;

use tmux_workspaces_term::{Attributes, Color, Cursor, Screen, Style, Terminal, Underline, Width};

/// Every cell of a terminal's screen, as the comparison sees it.
pub struct Grid {
    cols: u16,
    rows: u16,
    cells: Vec<View>,
    /// The cursor's position and visibility.
    pub cursor: Cursor,
}

/// One cell: what the screen copies and the details it leaves out.
#[derive(Clone, Debug, PartialEq, Eq)]
struct View {
    text: String,
    width: Width,
    style: Style,
    underline_color: Color,
    link: Option<String>,
}

/// Reads every cell of `terminal`'s screen.
pub fn grid(terminal: &mut Terminal) -> Result<Grid, String> {
    let mut screen = Screen::default();
    terminal
        .read_screen(&mut screen)
        .map_err(|error| error.to_string())?;
    let mut cells = Vec::new();
    for row in 0..screen.rows() {
        for col in 0..screen.cols() {
            let (text, cell) = screen.cell(row, col).ok_or("a cell outside the screen")?;
            let details = terminal
                .cell_details(row, col)
                .map_err(|error| error.to_string())?;
            cells.push(View {
                text: text.to_owned(),
                width: cell.width,
                style: cell.style,
                underline_color: details.underline_color,
                link: details.link,
            });
        }
    }
    Ok(Grid {
        cols: screen.cols(),
        rows: screen.rows(),
        cells,
        cursor: screen.cursor(),
    })
}

impl Grid {
    /// Returns the size in cells.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    fn get(&self, row: u16, col: u16) -> Option<&View> {
        (row < self.rows && col < self.cols)
            .then(|| &self.cells[usize::from(row) * usize::from(self.cols) + usize::from(col)])
    }
}

/// What the comparison allows for, from the tmux version and the
/// reference's terminal features.
#[derive(Clone, Copy, Debug)]
pub struct Rules {
    /// Whether hyperlinks are compared: tmux before 3.4 keeps none and sends
    /// its clients none.
    pub links: bool,
    /// Whether a palette underline colour equals the same colour given
    /// directly: tmux before 3.4 sends its clients underline colours as RGB.
    pub underline_color_by_value: bool,
}

/// A cell that differs.
#[derive(Clone, Debug)]
pub struct Difference {
    /// The cell's row in the pane, from zero.
    pub row: u16,
    /// The cell's column in the pane, from zero.
    pub col: u16,
    /// What differs.
    pub properties: Vec<Property>,
    /// The properties that differ because the candidate's cell lacks them:
    /// no hyperlink, the default colour, or no attribute or line.
    pub lost: Vec<Property>,
    /// The reference's cell.
    pub reference: String,
    /// The candidate's cell.
    pub candidate: String,
}

/// A property of a cell that can differ.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Property {
    /// The text, or whether the cell is blank.
    Text,
    /// How the text spans columns.
    Width,
    /// The text colour.
    Foreground,
    /// The background colour.
    Background,
    /// The attributes, such as bold or reverse video.
    Attributes,
    /// The underline style.
    Underline,
    /// The underline colour.
    UnderlineColor,
    /// The hyperlink.
    Link,
}

/// Compares every cell of `candidate`, a pane, with the reference's cells at
/// `origin`, the pane's column and row in the reference's window.
pub fn cells(
    reference: &Grid,
    origin: (u16, u16),
    candidate: &Grid,
    rules: Rules,
) -> Vec<Difference> {
    let mut differences = Vec::new();
    for row in 0..candidate.rows {
        for col in 0..candidate.cols {
            let ours = candidate.get(row, col);
            let theirs = reference.get(row + origin.1, col + origin.0);
            let (properties, lost) = match (theirs, ours) {
                (Some(theirs), Some(ours)) => differing(theirs, ours, rules),
                _ => (vec![Property::Text], Vec::new()),
            };
            if !properties.is_empty() {
                differences.push(Difference {
                    row,
                    col,
                    properties,
                    lost,
                    reference: describe(theirs),
                    candidate: describe(ours),
                });
            }
        }
    }
    differences
}

/// What two cells must share to look the same.
#[derive(PartialEq, Eq)]
enum Key<'a> {
    /// An empty cell or a space, which shows only its background and the
    /// lines drawn through it.
    Blank {
        background: Color,
        attributes: Attributes,
        underline: Underline,
        underline_color: Color,
        link: Option<&'a str>,
        /// The text colour, where reverse video or a line shows it.
        foreground: Option<Color>,
    },
    Cell {
        text: &'a str,
        width: Width,
        style: Style,
        underline_color: Color,
        link: Option<&'a str>,
    },
}

fn key(view: &View, rules: Rules) -> Key<'_> {
    let link = view.link.as_deref().filter(|_| rules.links);
    let underline_color = if rules.underline_color_by_value {
        by_value(view.underline_color)
    } else {
        view.underline_color
    };
    let style = view.style;
    if view.width == Width::Narrow && (view.text.is_empty() || view.text == " ") {
        let attributes = style.attributes;
        let shows_foreground = attributes.contains(Attributes::INVERSE)
            || attributes.contains(Attributes::OVERLINE)
            || attributes.contains(Attributes::STRIKETHROUGH)
            || style.underline != Underline::None;
        return Key::Blank {
            background: style.background,
            attributes,
            underline: style.underline,
            underline_color,
            link,
            foreground: shows_foreground.then_some(style.foreground),
        };
    }
    Key::Cell {
        text: &view.text,
        width: view.width,
        style,
        underline_color,
        link,
    }
}

/// Returns the properties in which two cells' keys differ, and those of
/// them the candidate's cell lacks. A blank cell and one with text differ in
/// their text, in the colours and lines both show, and in the text's colour
/// against the colour the blank cell holds.
fn differing(theirs: &View, ours: &View, rules: Rules) -> (Vec<Property>, Vec<Property>) {
    let (their_key, our_key) = (key(theirs, rules), key(ours, rules));
    let (their_parts, our_parts) = (Parts::of(&their_key), Parts::of(&our_key));
    let foreground = match (their_parts.foreground, our_parts.foreground) {
        (Some(their_color), Some(our_color)) => their_color != our_color,
        (None, Some(_)) | (Some(_), None)
            if their_parts.text.is_some() != our_parts.text.is_some() =>
        {
            theirs.style.foreground != ours.style.foreground
        }
        // Only one shows its colour: their attributes or lines differ.
        _ => false,
    };
    let compared = [
        (Property::Text, their_parts.text != our_parts.text, false),
        (Property::Width, their_parts.width != our_parts.width, false),
        (
            Property::Foreground,
            foreground,
            ours.style.foreground == Color::Default,
        ),
        (
            Property::Background,
            their_parts.background != our_parts.background,
            our_parts.background == Color::Default,
        ),
        (
            Property::Attributes,
            their_parts.attributes != our_parts.attributes,
            our_parts.attributes == Attributes::default(),
        ),
        (
            Property::Underline,
            their_parts.underline != our_parts.underline,
            our_parts.underline == Underline::None,
        ),
        (
            Property::UnderlineColor,
            their_parts.underline_color != our_parts.underline_color,
            our_parts.underline_color == Color::Default,
        ),
        (
            Property::Link,
            their_parts.link != our_parts.link,
            our_parts.link.is_none(),
        ),
    ];
    let properties = compared
        .iter()
        .filter_map(|&(property, differs, _)| differs.then_some(property))
        .collect();
    let lost = compared
        .iter()
        .filter_map(|&(property, differs, lacks)| (differs && lacks).then_some(property))
        .collect();
    (properties, lost)
}

/// A key's properties side by side: a blank cell has no text and shows its
/// text colour only under reverse video or a line.
struct Parts<'a> {
    text: Option<&'a str>,
    width: Width,
    foreground: Option<Color>,
    background: Color,
    attributes: Attributes,
    underline: Underline,
    underline_color: Color,
    link: Option<&'a str>,
}

impl<'a> Parts<'a> {
    fn of(key: &Key<'a>) -> Self {
        match *key {
            Key::Blank {
                background,
                attributes,
                underline,
                underline_color,
                link,
                foreground,
            } => Self {
                text: None,
                width: Width::Narrow,
                foreground,
                background,
                attributes,
                underline,
                underline_color,
                link,
            },
            Key::Cell {
                text,
                width,
                style,
                underline_color,
                link,
            } => Self {
                text: Some(text),
                width,
                foreground: Some(style.foreground),
                background: style.background,
                attributes: style.attributes,
                underline: style.underline,
                underline_color,
                link,
            },
        }
    }
}

/// Returns a palette colour as the RGB colour tmux gives it.
fn by_value(color: Color) -> Color {
    const STANDARD: [(u8, u8, u8); 16] = [
        (0x00, 0x00, 0x00),
        (0x80, 0x00, 0x00),
        (0x00, 0x80, 0x00),
        (0x80, 0x80, 0x00),
        (0x00, 0x00, 0x80),
        (0x80, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0xc0, 0xc0, 0xc0),
        (0x80, 0x80, 0x80),
        (0xff, 0x00, 0x00),
        (0x00, 0xff, 0x00),
        (0xff, 0xff, 0x00),
        (0x00, 0x00, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0xff, 0xff),
        (0xff, 0xff, 0xff),
    ];
    const STEPS: [u8; 6] = [0x00, 0x5f, 0x87, 0xaf, 0xd7, 0xff];
    let Color::Palette(index) = color else {
        return color;
    };
    let (r, g, b) = match index {
        0..=15 => STANDARD[usize::from(index)],
        16..=231 => {
            let cube = usize::from(index - 16);
            (STEPS[cube / 36], STEPS[cube / 6 % 6], STEPS[cube % 6])
        }
        _ => {
            let grey = 8 + (index - 232) * 10;
            (grey, grey, grey)
        }
    };
    Color::Rgb(r, g, b)
}

fn describe(view: Option<&View>) -> String {
    let Some(view) = view else {
        return "outside".to_owned();
    };
    let mut text = format!("{:?} {:?}", view.text, view.width);
    let style = view.style;
    if style != Style::default() {
        let _ = write!(text, " {style:?}");
    }
    if view.underline_color != Color::Default {
        let _ = write!(text, " underline colour {:?}", view.underline_color);
    }
    if let Some(link) = &view.link {
        let _ = write!(text, " link {link}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text: &str) -> View {
        View {
            text: text.to_owned(),
            width: Width::Narrow,
            style: Style::default(),
            underline_color: Color::Default,
            link: None,
        }
    }

    const STRICT: Rules = Rules {
        links: true,
        underline_color_by_value: false,
    };

    #[test]
    fn blank_cells_differ_only_in_what_shows() {
        let mut red = view(" ");
        red.style.foreground = Color::Palette(1);
        // An empty cell and a space look the same, whatever the text colour.
        assert!(key(&view(""), STRICT) == key(&red, STRICT));
        let mut reversed = red.clone();
        reversed.style.attributes.insert(Attributes::INVERSE);
        let mut plain_reversed = view("");
        plain_reversed.style.attributes.insert(Attributes::INVERSE);
        assert!(key(&reversed, STRICT) != key(&plain_reversed, STRICT));
        let mut blue = view("");
        blue.style.background = Color::Palette(4);
        assert!(key(&blue, STRICT) != key(&view(" "), STRICT));
        assert!(key(&view("x"), STRICT) != key(&view(" "), STRICT));
    }

    #[test]
    fn a_difference_names_the_properties_that_differ() {
        let mut blue = view(" ");
        blue.style.background = Color::Palette(4);
        let mut red_x = view("x");
        red_x.style.foreground = Color::Palette(1);
        let differ = |a: &View, b: &View| differing(a, b, STRICT);
        // A red x where a blank cell holds the default colour shows red.
        assert_eq!(
            differ(&blue, &red_x).0,
            [Property::Text, Property::Foreground, Property::Background]
        );
        assert_eq!(
            differ(&blue, &view("x")).0,
            [Property::Text, Property::Background]
        );
        assert_eq!(differ(&blue, &view("x")).1, [Property::Background]);
        assert_eq!(
            differ(&red_x, &view("x")),
            (vec![Property::Foreground], vec![Property::Foreground])
        );
        assert_eq!(differ(&view("x"), &red_x).1, []);
        assert!(differ(&view(""), &view(" ")).0.is_empty());
    }

    #[test]
    fn the_rules_drop_links_and_compare_underline_colours_by_value() {
        let mut linked = view("a");
        linked.link = Some("http://e.x".to_owned());
        assert!(key(&linked, STRICT) != key(&view("a"), STRICT));
        let lenient = Rules {
            links: false,
            underline_color_by_value: true,
        };
        assert!(key(&linked, lenient) == key(&view("a"), lenient));
        let mut palette = view("a");
        palette.underline_color = Color::Palette(196);
        let mut rgb = view("a");
        rgb.underline_color = Color::Rgb(0xff, 0, 0);
        assert!(key(&palette, STRICT) != key(&rgb, STRICT));
        assert!(key(&palette, lenient) == key(&rgb, lenient));
        assert_eq!(by_value(Color::Palette(244)), Color::Rgb(128, 128, 128));
        assert_eq!(by_value(Color::Palette(9)), Color::Rgb(255, 0, 0));
    }
}
