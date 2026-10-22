//! Native preferences, saved as `tmux-workspaces/settings.toml` in the user's
//! configuration directory. A missing or invalid file leaves the defaults.

use std::{
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// Terminal text sizes the settings accept, in points.
pub const FONT_SIZES: std::ops::RangeInclusive<u32> = 9..=24;
static FONT_SIZE: AtomicU32 = AtomicU32::new(0);
/// Navigation panel widths the settings accept, from the design tokens.
fn sidebar_widths() -> std::ops::RangeInclusive<u32> {
    let size = &super::design::tokens().size;
    size.panel_min as u32..=size.panel_max as u32
}
/// The dragged panel width, or zero for the default of 22 terminal cells.
static SIDEBAR_WIDTH: AtomicU32 = AtomicU32::new(0);

/// Whether panes show their header.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneHeaders {
    #[default]
    Always,
    Hover,
    Off,
}

impl PaneHeaders {
    pub const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::Always, "always", "Always"),
        (Self::Hover, "hover", "On hover"),
        (Self::Off, "off", "Off"),
    ];

    fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(mode, ..)| *mode == self)
            .map_or("always", |(_, name, _)| name)
    }
}

static PANE_HEADERS: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// How panes show their header now.
pub fn pane_headers() -> PaneHeaders {
    match PANE_HEADERS.load(Ordering::Relaxed) {
        1 => PaneHeaders::Hover,
        2 => PaneHeaders::Off,
        _ => PaneHeaders::Always,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub light: bool,
    pub font_size: f32,
    pub shortcuts: super::shortcuts::Bindings,
    /// The navigation panel's width in pixels, when it was dragged.
    pub sidebar_width: Option<u32>,
    pub pane_headers: PaneHeaders,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            light: false,
            font_size: super::design::tokens().font.terminal_size,
            shortcuts: super::shortcuts::Bindings::default(),
            sidebar_width: None,
            pane_headers: PaneHeaders::Always,
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
        if let Some(width) = table.get("sidebar_width").and_then(toml::Value::as_integer)
            && let Ok(width) = u32::try_from(width)
            && sidebar_widths().contains(&width)
        {
            settings.sidebar_width = Some(width);
        }
        if let Some(mode) = table.get("pane_headers").and_then(toml::Value::as_str)
            && let Some((mode, ..)) = PaneHeaders::ALL.iter().find(|(_, name, _)| *name == mode)
        {
            settings.pane_headers = *mode;
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
        if self.pane_headers != PaneHeaders::Always {
            text.push_str(&format!(
                "pane_headers = \"{}\"\n",
                self.pane_headers.name()
            ));
        }
        if let Some(width) = self.sidebar_width {
            text.push_str(&format!("sidebar_width = {width}\n"));
        }
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
        super::design::set_light(self.light);
        PANE_HEADERS.store(self.pane_headers as u8, Ordering::Relaxed);
        FONT_SIZE.store(self.font_size.to_bits(), Ordering::Relaxed);
        super::shortcuts::set(self.shortcuts.clone());
        SIDEBAR_WIDTH.store(self.sidebar_width.unwrap_or(0), Ordering::Relaxed);
    }

    /// The settings windows are drawing with now.
    pub fn current() -> Self {
        Self {
            light: super::design::is_light(),
            font_size: font_size(),
            shortcuts: super::shortcuts::current(),
            sidebar_width: sidebar_width(),
            pane_headers: pane_headers(),
        }
    }
}

/// The dragged navigation panel width in pixels, if any.
pub fn sidebar_width() -> Option<u32> {
    Some(SIDEBAR_WIDTH.load(Ordering::Relaxed)).filter(|width| *width > 0)
}

/// Use a dragged panel width now, without saving it.
pub fn set_sidebar_width(width: u32) {
    SIDEBAR_WIDTH.store(
        width.clamp(*sidebar_widths().start(), *sidebar_widths().end()),
        Ordering::Relaxed,
    );
}

/// The terminal text size, in points: the saved size, else the design's.
pub fn font_size() -> f32 {
    match FONT_SIZE.load(Ordering::Relaxed) {
        0 => super::design::tokens().font.terminal_size,
        bits => f32::from_bits(bits),
    }
}

/// `$XDG_CONFIG_HOME/tmux-workspaces/settings.toml`, else under `~/.config`.
pub fn path() -> Option<PathBuf> {
    path_from(
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
        || crate::library::location::user_home(None),
    )
}

/// The settings path from these variables; the account's home directory is
/// used only when HOME is unset, as the library, theme and keymap lookups do.
fn path_from(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    account: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    let config = match xdg.filter(|path| !path.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => home
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .or_else(account)?
            .join(".config"),
    };
    Some(config.join("tmux-workspaces/settings.toml"))
}

/// Read the saved settings, or the defaults.
pub fn load() -> Settings {
    path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map_or_else(Settings::default, |text| Settings::parse(&text))
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PENDING: AtomicU32 = AtomicU32::new(0);
static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

/// Save these settings on a background thread, so slow storage never blocks
/// the windows. Saves run one at a time and an older one never replaces a
/// newer one. `done` receives the outcome.
pub fn save_in_background(settings: Settings, done: impl FnOnce(io::Result<()>) + Send + 'static) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    PENDING.fetch_add(1, Ordering::SeqCst);
    std::thread::spawn(move || {
        let _pending = Pending;
        let mut written = WRITTEN.lock().unwrap_or_else(|error| error.into_inner());
        if *written > generation {
            return done(Ok(()));
        }
        let result = path()
            .ok_or_else(|| io::Error::other("no home directory"))
            .and_then(|path| save(&settings, &path));
        if result.is_ok() {
            *written = generation;
        }
        done(result);
    });
}

struct Pending;

impl Drop for Pending {
    fn drop(&mut self) {
        PENDING.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Before the process exits, let background saves finish, waiting at most
/// `limit`; a detached save thread would otherwise end mid-write.
pub fn finish_saves(limit: std::time::Duration) {
    let deadline = std::time::Instant::now() + limit;
    while PENDING.load(Ordering::SeqCst) > 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Save atomically: write a sibling file, then rename it over the old one.
pub fn save(settings: &Settings, path: &Path) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("settings path has no directory"))?;
    std::fs::create_dir_all(directory)?;
    // Each save writes its own new file, so concurrent saves from other
    // processes never share or truncate one another's temporary file.
    static SAVES: AtomicU32 = AtomicU32::new(0);
    let temporary = directory.join(format!(
        ".settings.toml.{}.{}.new",
        std::process::id(),
        SAVES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(settings.to_toml().as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_file_follows_xdg_then_home_then_the_account() {
        let account = || Some(PathBuf::from("/account"));
        let file = "tmux-workspaces/settings.toml";
        assert_eq!(
            path_from(Some("/xdg".into()), Some("/home".into()), account),
            Some(PathBuf::from("/xdg").join(file))
        );
        assert_eq!(
            path_from(None, Some("/home".into()), account),
            Some(PathBuf::from("/home/.config").join(file))
        );
        assert_eq!(
            path_from(Some("".into()), None, account),
            Some(PathBuf::from("/account/.config").join(file))
        );
        assert_eq!(path_from(None, None, || None), None);
    }

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
            sidebar_width: Some(260),
            pane_headers: PaneHeaders::Hover,
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
        let odd =
            Settings::parse("theme = \"sepia\"\nfont_size = 99\nsidebar_width = 9000\nother = 1\n");
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
