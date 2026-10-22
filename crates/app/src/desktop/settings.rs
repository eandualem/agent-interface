//! Native preferences, saved as `tmux-workspaces/settings.toml` in the user's
//! configuration directory. A missing or invalid file leaves the defaults.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// Terminal text sizes the settings accept, in points.
pub const FONT_SIZES: std::ops::RangeInclusive<u32> = 9..=24;
const DEFAULT_FONT_SIZE: f32 = 13.0;

static FONT_SIZE: AtomicU32 = AtomicU32::new(DEFAULT_FONT_SIZE.to_bits());

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub font_size: f32,
    pub shortcuts: super::shortcuts::Bindings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            light: false,
            font_size: DEFAULT_FONT_SIZE,
            shortcuts: super::shortcuts::Bindings::default(),
        }
    }
}

impl Settings {
    /// Read a settings file's text. Unknown keys are ignored; invalid values
    /// keep their defaults.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        let Ok(table) = text.parse::<toml::Table>() else {
            return settings;
        };
        if let Some(theme) = table.get("theme").and_then(toml::Value::as_str) {
            settings.light = theme == "light";
        }
        if let Some(size) = table.get("font_size").and_then(toml::Value::as_integer)
            && u32::try_from(size).is_ok_and(|size| FONT_SIZES.contains(&size))
        {
            settings.font_size = size as f32;
        }
        if let Some(shortcuts) = table.get("shortcuts").and_then(toml::Value::as_table) {
            settings.shortcuts = super::shortcuts::Bindings::from_entries(
                shortcuts
                    .iter()
                    .filter_map(|(id, value)| Some((id.as_str(), value.as_str()?))),
            );
        }
        settings
    }

    pub fn to_toml(&self) -> String {
        let mut text = format!(
            "theme = \"{}\"\nfont_size = {}\n",
            if self.light { "light" } else { "dark" },
            self.font_size as u32
        );
        let changed = self.shortcuts.changed();
        if !changed.is_empty() {
            text.push_str("\n[shortcuts]\n");
            for (id, chord) in changed {
                text.push_str(&format!("{id} = {}\n", toml::Value::String(chord)));
            }
        }
        text
    }

    /// Make these the settings every window draws with.
    pub fn apply(&self) {
        super::theme::set_light(self.light);
        FONT_SIZE.store(self.font_size.to_bits(), Ordering::Relaxed);
        super::shortcuts::set(self.shortcuts.clone());
    }

    /// The settings windows are drawing with now.
    pub fn current() -> Self {
        Self {
            light: super::theme::is_light(),
            font_size: font_size(),
            shortcuts: super::shortcuts::current(),
        }
    }
}

/// The terminal text size, in points.
pub fn font_size() -> f32 {
    f32::from_bits(FONT_SIZE.load(Ordering::Relaxed))
}

/// `$XDG_CONFIG_HOME/tmux-workspaces/settings.toml`, else under `~/.config`.
pub fn path() -> Option<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|path| !path.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => crate::library::location::user_home(None)?.join(".config"),
    };
    Some(config.join("tmux-workspaces/settings.toml"))
}

/// Read the saved settings, or the defaults.
pub fn load() -> Settings {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map_or_else(Settings::default, |text| Settings::parse(&text))
}

/// Save atomically: write a sibling file, then rename it over the old one.
pub fn save(settings: &Settings, path: &Path) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("settings path has no directory"))?;
    std::fs::create_dir_all(directory)?;
    let temporary = directory.join(".settings.toml.new");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(settings.to_toml().as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_invalid_values_keep_defaults() {
        let mut shortcuts = super::super::shortcuts::Bindings::default();
        shortcuts
            .assign(
                super::super::shortcuts::Command::NewTab,
                super::super::shortcuts::Chord::parse("ctrl-alt-t").unwrap(),
            )
            .unwrap();
        let light = Settings {
            light: true,
            font_size: 16.0,
            shortcuts,
        };
        assert_eq!(Settings::parse(&light.to_toml()), light);
        // Shortcuts are global; other tests rely on the defaults.
        let applied = Settings {
            shortcuts: super::super::shortcuts::Bindings::default(),
            ..light.clone()
        };
        applied.apply();
        assert_eq!(Settings::current(), applied);
        Settings::default().apply();
        assert_eq!(Settings::current(), Settings::default());
        assert_eq!(Settings::parse("not toml ["), Settings::default());
        let odd = Settings::parse("theme = \"sepia\"\nfont_size = 99\nother = 1\n");
        assert_eq!(odd, Settings::default());
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/settings.toml");
        save(&light, &path).unwrap();
        assert_eq!(
            Settings::parse(&std::fs::read_to_string(&path).unwrap()),
            light
        );
    }
}
