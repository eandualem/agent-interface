//! The design language, loaded from `assets/design/tokens.toml`, and the
//! theme in use.
//!
//! Every colour, size, radius, type style and timing the chrome uses comes
//! from the current theme through the typed tokens here, so the look can
//! change without touching the views. The built-in Dark and Light themes are
//! the tokens file; other themes are JSON files that change some of those
//! values (see `themes`). `docs/DESIGN.md` describes each token.

use std::sync::{LazyLock, Mutex, RwLock};

use gpui::{FontWeight, Pixels, Rgba, px};
use toml::{Table, Value};

const SOURCE: &str = include_str!("../../assets/design/tokens.toml");

/// A token that is missing or malformed, by its dotted path.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenError {
    /// The token's path, such as `color.ground`.
    pub path: String,
    /// What is wrong with it.
    pub message: String,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.path, self.message)
    }
}

fn wrong(at: &str, key: &str, message: &str) -> TokenError {
    TokenError {
        path: if at.is_empty() {
            key.to_owned()
        } else {
            format!("{at}.{key}")
        },
        message: message.to_owned(),
    }
}

/// A token value that can be written back as JSON.
trait Json {
    fn json(&self, out: &mut String, indent: usize);
}

impl Json for Rgba {
    fn json(&self, out: &mut String, _: usize) {
        out.push('"');
        out.push_str(&to_hex(*self));
        out.push('"');
    }
}

impl Json for f32 {
    fn json(&self, out: &mut String, _: usize) {
        // The shortest text that reads back as the same number, as 0.8, not 0.800000011920929.
        out.push_str(&self.to_string());
    }
}

impl Json for String {
    fn json(&self, out: &mut String, _: usize) {
        out.push_str(&serde_json::Value::from(self.as_str()).to_string());
    }
}

impl Json for [Rgba; 16] {
    fn json(&self, out: &mut String, indent: usize) {
        out.push('[');
        for (at, colour) in self.iter().enumerate() {
            out.push_str(if at % 8 == 0 { "\n" } else { " " });
            if at % 8 == 0 {
                out.push_str(&" ".repeat(indent + 2));
            }
            colour.json(out, indent);
            if at < 15 {
                out.push(',');
            }
        }
        out.push('\n');
        out.push_str(&" ".repeat(indent));
        out.push(']');
    }
}

/// Define a token group: a struct whose fields are read from the TOML table
/// of the same names, each with the reader that checks its kind.
macro_rules! group {
    ($(#[$doc:meta])* $name:ident { $($field:ident: $ty:ty = $read:ident),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq)]
        pub struct $name {
            $(pub $field: $ty,)*
        }

        impl $name {
            fn read(table: &Table, at: &str) -> Result<Self, TokenError> {
                Ok(Self {
                    $($field: $read(table, stringify!($field), at)?,)*
                })
            }
        }

        impl Json for $name {
            fn json(&self, out: &mut String, indent: usize) {
                let fields: &[(&str, &dyn Json)] = &[$((stringify!($field), &self.$field)),*];
                write_object(out, indent, fields);
            }
        }
    };
}

fn write_object(out: &mut String, indent: usize, fields: &[(&str, &dyn Json)]) {
    out.push_str("{\n");
    for (at, (key, value)) in fields.iter().enumerate() {
        out.push_str(&" ".repeat(indent + 2));
        out.push_str(&format!("\"{key}\": "));
        value.json(out, indent + 2);
        if at + 1 < fields.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str(&" ".repeat(indent));
    out.push('}');
}

group!(
    /// One theme's colours.
    Colors {
        ground: Rgba = color, panel: Rgba = color, surface: Rgba = color,
        field: Rgba = color, hover: Rgba = color, selected: Rgba = color,
        line: Rgba = color, line_strong: Rgba = color, edge: Rgba = color,
        text: Rgba = color, text_2: Rgba = color, muted: Rgba = color,
        faint: Rgba = color, accent: Rgba = color, accent_text: Rgba = color,
        on_accent: Rgba = color, accent_soft: Rgba = color, focus: Rgba = color,
        focus_halo: Rgba = color, attention: Rgba = color,
        attention_text: Rgba = color, on_attention: Rgba = color,
        attention_soft: Rgba = color, ok: Rgba = color, info: Rgba = color,
        violet: Rgba = color, green: Rgba = color, code: Rgba = color, shadow: Rgba = color,
        mark_from: Rgba = color, mark_to: Rgba = color, mark_edge: Rgba = color,
        mark_glyph: Rgba = color, mark_eye: Rgba = color, mark_glow: Rgba = color,
        pane_dim: f32 = number,
    }
);

group!(
    /// A theme's terminal defaults.
    Terminal {
        text: Rgba = color, background: Rgba = color,
        cursor: Rgba = color, selection: Rgba = color, selection_text: Rgba = color,
        palette: [Rgba; 16] = palette,
    }
);

group!(
    /// Font families and the default terminal size.
    Fonts { ui: String = string, mono: String = string, mono_linux: String = string, terminal_size: f32 = number }
);

group!(
    /// A size and weight.
    TextStyle { size: f32 = number, weight: f32 = number }
);

group!(
    /// Named type styles.
    Texts {
        page_title: TextStyle = style, workspace: TextStyle = style,
        row: TextStyle = style, row_selected: TextStyle = style,
        status: TextStyle = style, meta: TextStyle = style, label: TextStyle = style,
        key: TextStyle = style, empty_title: TextStyle = style, body: TextStyle = style,
        figure: TextStyle = style, group_title: TextStyle = style, doc_title: TextStyle = style,
        doc_heading: TextStyle = style, doc_body: TextStyle = style, quote: TextStyle = style,
        card_title: TextStyle = style, step_key: TextStyle = style, step_mark: TextStyle = style,
        scene_title: TextStyle = style, scene_line: TextStyle = style,
    }
);

group!(
    /// The spacing scale, on a 2 px base.
    Space {
        s2: f32 = number, s4: f32 = number, s6: f32 = number, s8: f32 = number,
        s10: f32 = number, s12: f32 = number, s14: f32 = number, s16: f32 = number,
        s18: f32 = number, s20: f32 = number, s24: f32 = number, s28: f32 = number,
    }
);

group!(
    /// Corner radii by use.
    Radius {
        key: f32 = number, icon_button: f32 = number, menu_item: f32 = number,
        row: f32 = number,
        slot: f32 = number, pane: f32 = number, popover: f32 = number, pill: f32 = number,
        bubble: f32 = number,
    }
);

group!(
    /// Component sizes.
    Size {
        titlebar: f32 = number, window_controls_x: f32 = number,
        window_controls_y: f32 = number, panel_width: f32 = number, panel_min: f32 = number,
        panel_max: f32 = number, workspace_tile: f32 = number, row: f32 = number,
        section_label: f32 = number, jump_field: f32 = number, slot: f32 = number,
        slot_row: f32 = number, icon_button: f32 = number, icon: f32 = number,
        icon_large: f32 = number, pane_header: f32 = number, pane_gap: f32 = number,
        window_inset: f32 = number, status_row: f32 = number,
        popover_min_width: f32 = number, popover_padding: f32 = number,
        popover_shadow_blur: f32 = number, popover_shadow_offset: f32 = number,
        button: f32 = number, agent_symbol: f32 = number, dot: f32 = number,
        minimap_width: f32 = number, minimap_height: f32 = number,
        focus_ring: f32 = number, focus_halo: f32 = number,
        window_width: f32 = number, window_height: f32 = number,
        window_min_width: f32 = number, window_min_height: f32 = number,
        caret: f32 = number,
        settings_width: f32 = number, settings_height: f32 = number,
        scene_mark: f32 = number, setup_column: f32 = number, setup_stage: f32 = number,
        setup_mark: f32 = number, setup_control: f32 = number, settings_nav: f32 = number, theme_card: f32 = number,
        empty_icon: f32 = number, empty_width: f32 = number,
        menu_heading: f32 = number, keycap: f32 = number, hairline: f32 = number,
        profile_popover: f32 = number, profile_mark: f32 = number,
        minimap_stroke: f32 = number, workspace_block_padding: f32 = number,
        pane_number_width: f32 = number, view_toolbar: f32 = number,
        view_footer: f32 = number, view_choice: f32 = number, view_tile: f32 = number,
        chip: f32 = number, meter: f32 = number, meter_large: f32 = number,
        fact_label: f32 = number, choice_button: f32 = number, filter: f32 = number,
        choice_row: f32 = number, hint_bar: f32 = number,
        tag_card_label: f32 = number, grouped_width: f32 = number,
        group_row: f32 = number, record_who: f32 = number, record_age: f32 = number,
        question_row: f32 = number, shelf_card: f32 = number, reader_width: f32 = number,
        reader_label: f32 = number, reference_row: f32 = number,
        conversation_rail: f32 = number, rail_button: f32 = number,
        conversation_width: f32 = number, conversation_header: f32 = number,
        composer_field: f32 = number, send_button: f32 = number, anchor: f32 = number,
        chooser_height: f32 = number,
        plan_step: f32 = number, plan_dot: f32 = number, plan_tag: f32 = number,
        plan_node_width: f32 = number, plan_node_height: f32 = number,
        plan_column: f32 = number, action: f32 = number, revision_step: f32 = number,
        sheet_tabs: f32 = number, sheet_header: f32 = number, sheet_row: f32 = number,
        sheet_number: f32 = number,
        plan_edge: f32 = number, plan_arrow: f32 = number,
        interaction_rail: f32 = number,
        interaction_gutter: f32 = number,
        interaction_gap_unit: f32 = number,
        interaction_gap_max: f32 = number,
        interaction_group_label: f32 = number,
        interaction_member_row: f32 = number,
        interaction_edge_row: f32 = number,
        interaction_time: f32 = number,
        interaction_who: f32 = number,
        interaction_status: f32 = number,
        interaction_reader: f32 = number,
        interaction_reader_chat: f32 = number,
        interaction_split: f32 = number,
        interaction_quiet_row: f32 = number,
        interaction_burst_row: f32 = number,
        interaction_lifecycle_row: f32 = number,
        interaction_lifecycle_mark: f32 = number,
        interaction_strip: f32 = number,
        interaction_band: f32 = number,
        interaction_tick: f32 = number,
    }
);

group!(
    /// Timings for the working-agent indicator and the opening scene.
    Motion {
        spinner_ms: f32 = number, scene_ms: f32 = number, scene_face_ms: f32 = number,
        scene_words_ms: f32 = number, scene_handoff_ms: f32 = number,
        scene_settle_ms: f32 = number, step_ms: f32 = number, reveal_ms: f32 = number,
    }
);

/// One theme: the whole design language as it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Design {
    /// The theme's name, as Settings shows it.
    pub name: String,
    /// Whether the theme is light, for web views and window captions.
    pub light: bool,
    pub colors: Colors,
    pub terminal: Terminal,
    pub font: Fonts,
    pub text: Texts,
    pub space: Space,
    pub radius: Radius,
    pub size: Size,
    pub motion: Motion,
}

/// The token groups a theme table holds, in the order they are written.
pub const GROUPS: [&str; 8] = [
    "color", "terminal", "font", "text", "space", "radius", "size", "motion",
];

impl Design {
    /// Read a theme from a table holding every token group.
    pub fn from_table(table: &Table, name: &str, light: bool) -> Result<Self, TokenError> {
        let group = |key: &str| -> Result<&Table, TokenError> {
            table
                .get(key)
                .and_then(Value::as_table)
                .ok_or_else(|| wrong("", key, "is missing"))
        };
        Ok(Self {
            name: name.to_owned(),
            light,
            colors: Colors::read(group("color")?, "color")?,
            terminal: Terminal::read(group("terminal")?, "terminal")?,
            font: Fonts::read(group("font")?, "font")?,
            text: Texts::read(group("text")?, "text")?,
            space: Space::read(group("space")?, "space")?,
            radius: Radius::read(group("radius")?, "radius")?,
            size: Size::read(group("size")?, "size")?,
            motion: Motion::read(group("motion")?, "motion")?,
        })
    }

    /// The theme's colours, terminal colours and fonts as JSON, for a theme
    /// file a person edits.
    pub fn colors_json(&self) -> String {
        let font = [("ui", &self.font.ui), ("mono", &self.font.mono)];
        let font: Vec<(&str, &dyn Json)> = font
            .iter()
            .map(|(key, value)| (*key, *value as &dyn Json))
            .collect();
        let mut out = String::new();
        let mut fonts = String::new();
        write_object(&mut fonts, 2, &font);
        out.push_str("  \"color\": ");
        self.colors.json(&mut out, 2);
        out.push_str(",\n  \"terminal\": ");
        self.terminal.json(&mut out, 2);
        out.push_str(",\n  \"font\": ");
        out.push_str(&fonts);
        out
    }
}

impl TextStyle {
    pub fn size(&self) -> Pixels {
        px(self.size)
    }

    pub fn weight(&self) -> FontWeight {
        FontWeight(self.weight)
    }
}

/// The tables of the built-in dark and light themes, from the tokens file.
static BASES: LazyLock<[Table; 2]> = LazyLock::new(|| {
    // The bundled file is checked by a test, so this cannot fail at runtime.
    bases(SOURCE).expect("bundled design tokens are valid")
});

fn bases(source: &str) -> Result<[Table; 2], String> {
    let root: Table = source.parse().map_err(|e| format!("tokens.toml: {e}"))?;
    let theme = |variant: &str| -> Result<Table, String> {
        let mut table = Table::new();
        for group in GROUPS {
            let value = match group {
                "color" | "terminal" => root
                    .get(group)
                    .and_then(Value::as_table)
                    .and_then(|table| table.get(variant)),
                _ => root.get(group),
            }
            .filter(|value| value.is_table())
            .ok_or_else(|| format!("tokens.toml: missing [{group}]"))?;
            table.insert(group.into(), value.clone());
        }
        if cfg!(target_os = "linux")
            && let Some(Value::Table(font)) = table.get_mut("font")
            && let Some(linux) = font.get("mono_linux").cloned()
        {
            font.insert("mono".into(), linux);
        }
        Design::from_table(&table, variant, variant == "light")
            .map_err(|e| format!("tokens.toml: {e}"))?;
        Ok(table)
    };
    Ok([theme("dark")?, theme("light")?])
}

/// The table of the built-in dark or light theme.
pub fn base(light: bool) -> &'static Table {
    &BASES[usize::from(light)]
}

/// The built-in dark or light theme.
pub fn builtin(light: bool) -> &'static Design {
    static BUILTIN: LazyLock<[&'static Design; 2]> = LazyLock::new(|| {
        [false, true].map(|light| {
            let name = if light { "Light" } else { "Dark" };
            intern(
                Design::from_table(base(light), name, light)
                    .expect("bundled design tokens are valid"),
            )
        })
    });
    BUILTIN[usize::from(light)]
}

/// Every theme applied so far. A theme is kept for the life of the process,
/// so the tokens can be handed out as plain references; applying the same
/// theme again reuses its copy.
static INTERNED: Mutex<Vec<&'static Design>> = Mutex::new(Vec::new());
static CURRENT: RwLock<Option<&'static Design>> = RwLock::new(None);

fn intern(design: Design) -> &'static Design {
    let mut interned = INTERNED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(known) = interned.iter().find(|known| ***known == design) {
        return known;
    }
    let design = Box::leak(Box::new(design));
    interned.push(design);
    design
}

/// The theme in use.
pub fn tokens() -> &'static Design {
    let current = *CURRENT
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    current.unwrap_or_else(|| builtin(false))
}

/// The current theme's colours.
pub fn colors() -> &'static Colors {
    &tokens().colors
}

/// The current theme's terminal defaults.
pub fn terminal() -> &'static Terminal {
    &tokens().terminal
}

/// Whether the theme in use is light.
pub fn is_light() -> bool {
    tokens().light
}

/// Use `design` in every window; windows repaint on their next frame.
pub fn set_theme(design: Design) {
    let design = intern(design);
    *CURRENT
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(design);
}

fn value<'a>(table: &'a Table, key: &str, at: &str) -> Result<&'a Value, TokenError> {
    table.get(key).ok_or_else(|| wrong(at, key, "is missing"))
}

fn number(table: &Table, key: &str, at: &str) -> Result<f32, TokenError> {
    let number = match value(table, key, at)? {
        Value::Float(n) => *n as f32,
        Value::Integer(n) => *n as f32,
        _ => return Err(wrong(at, key, "must be a number")),
    };
    if number.is_finite() {
        Ok(number)
    } else {
        Err(wrong(at, key, "must be a finite number"))
    }
}

fn string(table: &Table, key: &str, at: &str) -> Result<String, TokenError> {
    value(table, key, at)?
        .as_str()
        .filter(|text| !text.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| wrong(at, key, "must be text"))
}

fn style(table: &Table, key: &str, at: &str) -> Result<TextStyle, TokenError> {
    let style = value(table, key, at)?
        .as_table()
        .ok_or_else(|| wrong(at, key, "must be an object with size and weight"))?;
    TextStyle::read(style, &format!("{at}.{key}"))
}

fn color(table: &Table, key: &str, at: &str) -> Result<Rgba, TokenError> {
    value(table, key, at)?
        .as_str()
        .and_then(hex)
        .ok_or_else(|| wrong(at, key, "must be a colour, #rrggbb or #rrggbbaa"))
}

fn palette(table: &Table, key: &str, at: &str) -> Result<[Rgba; 16], TokenError> {
    let colours = value(table, key, at)?
        .as_array()
        .filter(|list| list.len() == 16)
        .ok_or_else(|| wrong(at, key, "must list 16 colours"))?;
    let mut palette = [Rgba::default(); 16];
    for (slot, colour) in palette.iter_mut().zip(colours) {
        *slot = colour
            .as_str()
            .and_then(hex)
            .ok_or_else(|| wrong(at, key, "has a colour that is not #rrggbb or #rrggbbaa"))?;
    }
    Ok(palette)
}

/// `#rrggbb` or `#rrggbbaa`.
pub fn hex(text: &str) -> Option<Rgba> {
    let digits = text.strip_prefix('#')?;
    if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let channel = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    let alpha = if digits.len() == 8 { channel(6)? } else { 255 };
    Some(Rgba {
        r: f32::from(channel(0)?) / 255.0,
        g: f32::from(channel(2)?) / 255.0,
        b: f32::from(channel(4)?) / 255.0,
        a: f32::from(alpha) / 255.0,
    })
}

/// A colour as `#rrggbb`, with `aa` added when it is not opaque.
pub fn to_hex(colour: Rgba) -> String {
    let byte = |channel: f32| (channel.clamp(0.0, 1.0) * 255.0).round() as u8;
    let mut text = format!(
        "#{:02x}{:02x}{:02x}",
        byte(colour.r),
        byte(colour.g),
        byte(colour.b)
    );
    if byte(colour.a) != 255 {
        text.push_str(&format!("{:02x}", byte(colour.a)));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_tokens_are_complete_and_well_formed() {
        let dark = builtin(false);
        assert_eq!(dark.colors.ground, hex("#0f1012").unwrap());
        assert!((dark.colors.accent_soft.a - 0x24 as f32 / 255.0).abs() < 1e-6);
        assert_eq!(dark.size.panel_width, 232.0);
        assert_eq!(dark.text.row_selected.weight, 500.0);
        assert!(builtin(true).light && !dark.light);
        let missing = SOURCE.replace("ground = \"#0f1012\"", "");
        assert!(bases(&missing).unwrap_err().contains("color.ground"));
        let malformed = SOURCE.replace("\"#0f1012\"", "\"#0f10\"");
        assert!(bases(&malformed).is_err());
        assert_eq!(hex("#e8a85f24").unwrap().a, 0x24 as f32 / 255.0);
        assert!(hex("e8a85f").is_none());
        assert_eq!(to_hex(hex("#e8a85f24").unwrap()), "#e8a85f24");
        assert_eq!(to_hex(hex("#0F1012").unwrap()), "#0f1012");
    }

    #[test]
    fn applying_a_theme_reuses_an_equal_copy() {
        let mut design = builtin(false).clone();
        design.colors.accent = hex("#123456").unwrap();
        let first = intern(design.clone()) as *const Design;
        assert_eq!(intern(design) as *const Design, first);
        assert!(std::ptr::eq(intern(builtin(true).clone()), builtin(true)));
    }

    #[test]
    fn a_theme_writes_its_colours_as_json_that_reads_back() {
        let design = builtin(true);
        let text = format!("{{\n{}\n}}", design.colors_json());
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["color"]["accent"], "#a35a16");
        assert_eq!(value["color"]["pane_dim"], 0.8);
        assert_eq!(value["terminal"]["palette"].as_array().unwrap().len(), 16);
        assert_eq!(value["font"]["mono"], design.font.mono.as_str());
    }
}
