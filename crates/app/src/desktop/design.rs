//! The design language, loaded from `assets/design/tokens.toml`.
//!
//! Every colour, size, radius, type style and timing the chrome uses comes
//! from that file through the typed tokens here, so the look can change
//! without touching the views. `docs/DESIGN.md` describes each token.

use std::sync::{
    LazyLock,
    atomic::{AtomicBool, Ordering},
};

use gpui::{FontWeight, Pixels, Rgba, px};
use toml::{Table, Value};

const SOURCE: &str = include_str!("../../assets/design/tokens.toml");

/// Define a token group: a struct whose fields are read from the TOML table
/// of the same names, each with the reader that checks its kind.
macro_rules! group {
    ($(#[$doc:meta])* $name:ident { $($field:ident: $ty:ty = $read:ident),* $(,)? }) => {
        $(#[$doc])*
        #[derive(Clone, Debug)]
        pub struct $name {
            $(pub $field: $ty,)*
        }

        impl $name {
            fn read(table: &Table, at: &str) -> Result<Self, String> {
                Ok(Self {
                    $($field: $read(table, stringify!($field), at)?,)*
                })
            }
        }
    };
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
        pane_dim: f32 = number,
    }
);

group!(
    /// A theme's terminal defaults.
    Terminal {
        text: Rgba = color, background: Rgba = color,
        cursor: Rgba = color, palette: [Rgba; 16] = palette,
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
        settings_nav: f32 = number, theme_card: f32 = number,
        empty_icon: f32 = number, empty_width: f32 = number,
        menu_heading: f32 = number, keycap: f32 = number, hairline: f32 = number,
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
    }
);

group!(
    /// Timings for visible working-agent indicators.
    Motion { spinner_ms: f32 = number }
);

/// The whole design language.
#[derive(Clone, Debug)]
pub struct Design {
    pub dark: Colors,
    pub light: Colors,
    pub terminal_dark: Terminal,
    pub terminal_light: Terminal,
    pub font: Fonts,
    pub text: Texts,
    pub space: Space,
    pub radius: Radius,
    pub size: Size,
    pub motion: Motion,
}

impl Design {
    /// Parse and check a tokens file.
    pub fn parse(source: &str) -> Result<Self, String> {
        let root: Table = source.parse().map_err(|e| format!("tokens.toml: {e}"))?;
        let group = |path: &[&str]| -> Result<&Table, String> {
            let mut table = &root;
            for key in path {
                table = table
                    .get(*key)
                    .and_then(Value::as_table)
                    .ok_or_else(|| format!("tokens.toml: missing [{}]", path.join(".")))?;
            }
            Ok(table)
        };
        Ok(Self {
            dark: Colors::read(group(&["color", "dark"])?, "color.dark")?,
            light: Colors::read(group(&["color", "light"])?, "color.light")?,
            terminal_dark: Terminal::read(group(&["terminal", "dark"])?, "terminal.dark")?,
            terminal_light: Terminal::read(group(&["terminal", "light"])?, "terminal.light")?,
            font: Fonts::read(group(&["font"])?, "font")?,
            text: Texts::read(group(&["text"])?, "text")?,
            space: Space::read(group(&["space"])?, "space")?,
            radius: Radius::read(group(&["radius"])?, "radius")?,
            size: Size::read(group(&["size"])?, "size")?,
            motion: Motion::read(group(&["motion"])?, "motion")?,
        })
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

static DESIGN: LazyLock<Design> = LazyLock::new(|| {
    // The bundled file is checked by a test, so this cannot fail at runtime.
    let mut design = Design::parse(SOURCE).expect("bundled design tokens are valid");
    if cfg!(target_os = "linux") {
        design.font.mono.clone_from(&design.font.mono_linux);
    }
    design
});
static LIGHT: AtomicBool = AtomicBool::new(false);

/// The design language.
pub fn tokens() -> &'static Design {
    &DESIGN
}

/// The current theme's colours.
pub fn colors() -> &'static Colors {
    if is_light() {
        &DESIGN.light
    } else {
        &DESIGN.dark
    }
}

/// The current theme's terminal defaults.
pub fn terminal() -> &'static Terminal {
    if is_light() {
        &DESIGN.terminal_light
    } else {
        &DESIGN.terminal_dark
    }
}

/// Whether the light theme is in use.
pub fn is_light() -> bool {
    LIGHT.load(Ordering::Relaxed)
}

/// Switch every window's theme; windows repaint on their next frame.
pub fn set_light(light: bool) {
    LIGHT.store(light, Ordering::Relaxed);
}

fn value<'a>(table: &'a Table, key: &str, at: &str) -> Result<&'a Value, String> {
    table
        .get(key)
        .ok_or_else(|| format!("tokens.toml: missing {at}.{key}"))
}

fn number(table: &Table, key: &str, at: &str) -> Result<f32, String> {
    match value(table, key, at)? {
        Value::Float(n) => Ok(*n as f32),
        Value::Integer(n) => Ok(*n as f32),
        _ => Err(format!("tokens.toml: {at}.{key} must be a number")),
    }
}

fn string(table: &Table, key: &str, at: &str) -> Result<String, String> {
    value(table, key, at)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("tokens.toml: {at}.{key} must be text"))
}

fn style(table: &Table, key: &str, at: &str) -> Result<TextStyle, String> {
    let style = value(table, key, at)?
        .as_table()
        .ok_or_else(|| format!("tokens.toml: [{at}.{key}] must be a table"))?;
    TextStyle::read(style, &format!("{at}.{key}"))
}

fn color(table: &Table, key: &str, at: &str) -> Result<Rgba, String> {
    let text = value(table, key, at)?
        .as_str()
        .ok_or_else(|| format!("tokens.toml: {at}.{key} must be a colour"))?;
    hex(text).ok_or_else(|| format!("tokens.toml: {at}.{key} is not #rrggbb or #rrggbbaa"))
}

fn palette(table: &Table, key: &str, at: &str) -> Result<[Rgba; 16], String> {
    let colours = value(table, key, at)?
        .as_array()
        .filter(|list| list.len() == 16)
        .ok_or_else(|| format!("tokens.toml: {at}.{key} must list 16 colours"))?;
    let mut palette = [Rgba::default(); 16];
    for (slot, colour) in palette.iter_mut().zip(colours) {
        *slot = colour
            .as_str()
            .and_then(hex)
            .ok_or_else(|| format!("tokens.toml: {at}.{key} has an invalid colour"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_tokens_are_complete_and_well_formed() {
        let design = Design::parse(SOURCE).unwrap();
        assert_eq!(design.dark.ground, hex("#0f1012").unwrap());
        assert!((design.dark.accent_soft.a - 0x24 as f32 / 255.0).abs() < 1e-6);
        assert_eq!(design.size.panel_width, 232.0);
        assert_eq!(design.text.row_selected.weight, 500.0);
        let missing = SOURCE.replace("ground = \"#0f1012\"", "");
        assert!(
            Design::parse(&missing)
                .unwrap_err()
                .contains("color.dark.ground")
        );
        let malformed = SOURCE.replace("\"#0f1012\"", "\"#0f10\"");
        assert!(Design::parse(&malformed).is_err());
        assert_eq!(hex("#e8a85f24").unwrap().a, 0x24 as f32 / 255.0);
        assert!(hex("e8a85f").is_none());
    }
}
