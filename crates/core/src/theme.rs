//! Validated Python-compatible theme data, independent of native rendering.

use unicode_properties::{GeneralCategory, UnicodeGeneralCategory};

/// Maximum UTF-8 theme file size.
pub const MAX_BYTES: usize = 16 * 1024;
/// Semantic roles, in the Python configuration's order.
pub const ROLE_NAMES: &[&str] = &[
    "normal", "active", "accent", "muted", "outline", "header", "danger",
];
/// Named starting palettes from the pinned Python implementation.
pub const PRESET_NAMES: &[&str] = &["default", "plain", "forest", "paper", "mono", "debug"];
const ATTRIBUTES: &[&str] = &["bold", "dim", "reverse", "standout", "underline"];
const BASIC: &[&str] = &[
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];
const PRESETS: &str = include_str!("theme/presets.toml");

/// Which ground supplies an inherited role background.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ground {
    /// The navigation panel ground.
    Panel,
    /// The content surface ground.
    Surface,
}

/// One role's configured colours and attributes; fallback order is retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Role {
    /// Canonical names, decimal indices or #rrggbb values, in fallback order.
    pub foreground: Vec<String>,
    /// Canonical background values, in fallback order.
    pub background: Vec<String>,
    /// Deduplicated attributes in the fixed configuration order.
    pub attributes: Vec<String>,
    /// None means a fixed background; an explicit override removes inheritance.
    pub background_source: Option<Ground>,
}

/// An immutable configuration snapshot. `default` remains symbolic; resolving it
/// and drawing native colours are separate product decisions. No palette is set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Theme {
    preset: String,
    panel: String,
    surface: String,
    roles: Vec<Role>,
}

impl Default for Theme {
    fn default() -> Self {
        Self::parse(b"").expect("shipped theme is valid")
    }
}

impl Theme {
    /// Parse bounded TOML 1.0 bytes. Omitted roles/fields retain their preset
    /// values. Ground overrides change inherited backgrounds before explicit
    /// role overrides are applied. Unknown fields and invalid values are errors.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err(format!("theme exceeds {MAX_BYTES} bytes"));
        }
        let text = std::str::from_utf8(bytes).map_err(|_| "theme must be UTF-8")?;
        if text.starts_with('\u{feff}') {
            return Err("theme must not begin with a UTF-8 byte order mark".into());
        }
        let data: toml::Table = toml::from_str(text).map_err(|e: toml::de::Error| {
            format!("invalid theme TOML: {}", e.message().escape_debug())
        })?;
        for name in data.keys() {
            if !ROLE_NAMES.contains(&name.as_str())
                && !matches!(name.as_str(), "preset" | "panel" | "surface")
            {
                return Err(format!("unknown theme section {name:?}"));
            }
        }
        let preset = match data.get("preset") {
            None => "default".to_owned(),
            Some(value) => {
                trim(value.as_str().ok_or("preset: expected a preset name")?).to_lowercase()
            }
        };
        if !PRESET_NAMES.contains(&preset.as_str()) {
            return Err(format!("unknown preset {preset:?}"));
        }
        let presets: toml::Table = toml::from_str(PRESETS).expect("shipped presets are valid TOML");
        let base = &presets[&preset];
        let base_panel = base["panel"].as_str().unwrap();
        let base_surface = base["surface"].as_str().unwrap();
        let panel = data
            .get("panel")
            .map(|v| ground(v, "panel"))
            .transpose()?
            .unwrap_or_else(|| base_panel.into());
        let surface = data
            .get("surface")
            .map(|v| ground(v, "surface"))
            .transpose()?
            .unwrap_or_else(|| base_surface.into());
        let mut roles = Vec::new();
        for &name in ROLE_NAMES {
            let base = base[name].as_table().unwrap();
            let mut role = Role {
                foreground: colors(&base["foreground"], name)?,
                background: colors(&base["background"], name)?,
                attributes: attributes(&base["attributes"], name)?,
                background_source: None,
            };
            let first = tmux_spelling(&role.background[0]);
            // Keep the pinned runtime's inheritance, including presets whose
            // panel and surface are both symbolic default.
            if first == base_panel {
                role.background_source = Some(Ground::Panel);
                if panel != base_panel {
                    role.background = vec![role_spelling(&panel)];
                }
            } else if first == base_surface {
                role.background_source = Some(Ground::Surface);
                if surface != base_surface {
                    role.background = vec![role_spelling(&surface)];
                }
            }
            if let Some(value) = data.get(name) {
                let table = value
                    .as_table()
                    .ok_or_else(|| format!("{name}: expected a role table"))?;
                for key in table.keys() {
                    if !matches!(key.as_str(), "foreground" | "background" | "attributes") {
                        return Err(format!("{name}: unknown option {key:?}"));
                    }
                }
                if let Some(value) = table.get("foreground") {
                    role.foreground = colors(value, &format!("{name}.foreground"))?;
                }
                if let Some(value) = table.get("background") {
                    role.background = colors(value, &format!("{name}.background"))?;
                    role.background_source = None;
                }
                if let Some(value) = table.get("attributes") {
                    role.attributes = attributes(value, &format!("{name}.attributes"))?;
                }
            }
            roles.push(role);
        }
        Ok(Self {
            preset,
            panel,
            surface,
            roles,
        })
    }

    /// The preset from which the configuration inherited its omitted fields.
    pub fn base_preset(&self) -> &str {
        &self.preset
    }
    /// Canonical ground: default, name, colourN or #rrggbb, as Python stores it.
    pub fn panel(&self) -> &str {
        &self.panel
    }
    /// Canonical content ground, retaining symbolic default when configured.
    pub fn surface(&self) -> &str {
        &self.surface
    }
    /// Find one semantic role; unknown names return None.
    pub fn role(&self, name: &str) -> Option<&Role> {
        ROLE_NAMES
            .iter()
            .position(|&n| n == name)
            .map(|i| &self.roles[i])
    }
    /// All semantic roles in their stable configuration order.
    pub fn roles(&self) -> impl Iterator<Item = (&'static str, &Role)> {
        ROLE_NAMES.iter().copied().zip(&self.roles)
    }
}

fn trim(text: &str) -> &str {
    text.trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}'))
}

fn decimal(text: &str) -> Option<u32> {
    if text.is_empty() {
        return None;
    }
    let mut number = 0;
    for ch in text.chars() {
        if ch.general_category() != GeneralCategory::DecimalNumber {
            return None;
        }
        // Decimal digits form ascending groups of ten; adjacent groups exist.
        let mut start = ch as u32;
        while char::from_u32(start - 1)
            .is_some_and(|c| c.general_category() == GeneralCategory::DecimalNumber)
        {
            start -= 1;
        }
        number = (number * 10 + (ch as u32 - start) % 10).min(256);
    }
    Some(number)
}

fn indexed(number: i64) -> Result<String, String> {
    if (0..=255).contains(&number) {
        Ok(number.to_string())
    } else {
        Err("color index is outside 0-255".into())
    }
}

fn rgb(text: &str) -> bool {
    text.len() == 7
        && text.starts_with('#')
        && text.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
}

fn color(value: &toml::Value) -> Result<String, String> {
    if let Some(number) = value.as_integer() {
        return indexed(number);
    }
    let raw = value
        .as_str()
        .filter(|v| !v.is_empty() && v.chars().count() <= 32)
        .ok_or("expected a color name, default, 0-255 or #rrggbb")?;
    let text = trim(raw).to_lowercase().replace('_', "-");
    if text == "default"
        || BASIC.contains(&text.as_str())
        || text
            .strip_prefix("bright-")
            .is_some_and(|name| BASIC.contains(&name))
        || rgb(&text)
    {
        return Ok(text);
    }
    if let Some(number) = decimal(&text) {
        return indexed(number.into());
    }
    Err(format!("unsupported color {raw:?}"))
}

fn ground(value: &toml::Value, name: &str) -> Result<String, String> {
    let result = (|| {
        if let Some(number) = value.as_integer() {
            return indexed(number).map(|n| format!("colour{n}"));
        }
        let raw = value.as_str().ok_or("expected a ground color")?;
        let text = trim(raw).to_lowercase().replace('_', "-");
        if text == "default"
            || rgb(&text)
            || BASIC.contains(&text.as_str())
            || text
                .strip_prefix("bright")
                .is_some_and(|n| BASIC.contains(&n))
        {
            return Ok(text);
        }
        if text
            .strip_prefix("bright-")
            .is_some_and(|n| BASIC.contains(&n))
        {
            return Ok(text.replace("bright-", "bright"));
        }
        if let Some(number) = decimal(text.strip_prefix("colour").unwrap_or(&text)) {
            return indexed(number.into()).map(|n| format!("colour{n}"));
        }
        Err(format!("unsupported color {raw:?}"))
    })();
    result.map_err(|e| format!("{name}: {e}"))
}

fn colors(value: &toml::Value, name: &str) -> Result<Vec<String>, String> {
    let values = value
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_else(|| std::slice::from_ref(value));
    if values.is_empty() || values.len() > 8 {
        return Err(format!("{name}: expected 1 to 8 colors"));
    }
    values
        .iter()
        .map(|v| color(v).map_err(|e| format!("{name}: {e}")))
        .collect()
}

fn attributes(value: &toml::Value, name: &str) -> Result<Vec<String>, String> {
    let values = value
        .as_array()
        .filter(|values| values.len() <= 5)
        .ok_or_else(|| format!("{name}: expected a list of attribute names"))?;
    let mut given = Vec::new();
    for value in values {
        let attribute = value
            .as_str()
            .map(|s| trim(s).to_lowercase())
            .ok_or_else(|| format!("{name}: expected an attribute name"))?;
        if !ATTRIBUTES.contains(&attribute.as_str()) {
            return Err(format!("{name}: unknown attribute {attribute:?}"));
        }
        given.push(attribute);
    }
    Ok(ATTRIBUTES
        .iter()
        .filter(|&&a| given.iter().any(|s| s == a))
        .map(|&a| a.to_owned())
        .collect())
}

fn tmux_spelling(color: &str) -> String {
    if color.as_bytes()[0].is_ascii_digit() {
        format!("colour{color}")
    } else {
        color.replace("bright-", "bright")
    }
}

fn role_spelling(ground: &str) -> String {
    if let Some(index) = ground.strip_prefix("colour") {
        index.to_owned()
    } else if let Some(name) = ground.strip_prefix("bright") {
        format!("bright-{name}")
    } else {
        ground.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_overrides_preserve_fallbacks_and_inheritance() {
        for preset in PRESET_NAMES {
            let theme = Theme::parse(format!("preset = '{preset}'").as_bytes()).unwrap();
            assert_eq!(theme.base_preset(), *preset);
            assert_eq!(theme.roles().count(), 7);
        }
        let base = Theme::default();
        assert_eq!(base.panel(), "#15171c");
        assert_eq!(base.surface(), "#1b1e24");
        assert_eq!(
            base.role("normal").unwrap().foreground,
            ["#d4d6db", "252", "white"]
        );
        assert_eq!(Theme::parse(b"panel = '#15171c'").unwrap(), base);
        let theme = Theme::parse(b"preset = ' PLAIN '\npanel = 'bright_blue'\nsurface = 12\n[normal]\nforeground = [' RED ', 42]\nbackground = 'default'\nattributes = ['underline', 'Bold', 'bold']").unwrap();
        assert_eq!(theme.panel(), "brightblue");
        assert_eq!(theme.surface(), "colour12");
        let role = theme.role("normal").unwrap();
        assert_eq!(role.foreground, ["red", "42"]);
        assert_eq!(role.background, ["default"]);
        assert_eq!(role.attributes, ["bold", "underline"]);
        assert_eq!(role.background_source, None);
        let outline = theme.role("outline").unwrap();
        assert_eq!(outline.background, ["bright-blue"]);
        assert_eq!(outline.background_source, Some(Ground::Panel));
        assert_eq!(
            Theme::parse(b"preset = 'paper'")
                .unwrap()
                .role("accent")
                .unwrap()
                .foreground[0],
            "#b8651b"
        );
        let unicode =
            Theme::parse("panel = 'colour٢٥٥'\n[normal]\nforeground = '４２'".as_bytes()).unwrap();
        assert_eq!(unicode.panel(), "colour255");
        assert_eq!(unicode.role("normal").unwrap().foreground, ["42"]);
    }

    #[test]
    fn invalid_configuration_is_rejected_without_echoing_controls() {
        for input in [
            "preset = true",
            "preset = 'missing'",
            "unknown = 1",
            "normal = 1",
            "panel = 256",
            "surface = false",
            "[normal]\nunknown = 1",
            "[normal]\nforeground = []",
            "[normal]\nbackground = 'brightblue'",
            "[normal]\nforeground = -1",
            "[normal]\nforeground = true",
            "[normal]\nattributes = ['italic']",
            "[normal]\nattributes = 'bold'",
            "[normal]\nattributes = ['bold', 'bold', 'bold', 'bold', 'bold', 'bold']",
            "\u{feff}preset = 'plain'",
            "preset = '\u{1b}[31m' trailing",
        ] {
            let error = Theme::parse(input.as_bytes()).unwrap_err();
            assert!(!error.contains('\u{1b}'), "{error:?}");
        }
        assert!(Theme::parse(&[0xff]).is_err());
        assert!(Theme::parse(&vec![b' '; MAX_BYTES]).is_ok());
        assert!(Theme::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
}
