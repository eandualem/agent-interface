//! Native preferences, saved as `agent-interface/settings.toml` in the user's
//! configuration directory. A missing or invalid file leaves the defaults.

use std::{
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
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
/// Conversation panel widths the settings accept, from the design's minimum.
fn conversation_widths() -> std::ops::RangeInclusive<u32> {
    super::design::tokens().size.conversation_width as u32..=4000
}
/// The dragged conversation width, or zero for a share of the reader.
static CONVERSATION_WIDTH: AtomicU32 = AtomicU32::new(0);

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
static INTRO_SEEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SETUP_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
    /// Persistent, explicit local diagnostic verbosity.
    pub logging: crate::diagnostics::Level,
    pub light: bool,
    pub font_size: f32,
    /// The interface size in percent, one of [`super::zoom::SIZES`].
    pub interface_size: u32,
    pub shortcuts: super::shortcuts::Bindings,
    /// The navigation panel's width in pixels, when it was dragged.
    pub sidebar_width: Option<u32>,
    /// The conversation panel's width in pixels, when it was dragged.
    pub conversation_width: Option<u32>,
    pub pane_headers: PaneHeaders,
    /// The first-run introduction has played.
    pub intro_seen: bool,
    /// First-run setup was finished.
    pub setup_done: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            logging: crate::diagnostics::Level::Warnings,
            light: false,
            font_size: super::design::tokens().font.terminal_size,
            interface_size: 100,
            shortcuts: super::shortcuts::Bindings::default(),
            sidebar_width: None,
            conversation_width: None,
            pane_headers: PaneHeaders::Always,
            intro_seen: false,
            setup_done: false,
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
        if let Some(value) = table.get("logging").and_then(toml::Value::as_str)
            && let Some((level, ..)) = crate::diagnostics::Level::ALL
                .iter()
                .find(|(_, name, _)| *name == value)
        {
            settings.logging = *level;
        }
        if let Some(theme) = table.get("theme").and_then(toml::Value::as_str) {
            settings.light = theme == "light";
        }
        if let Some(size) = table.get("font_size").and_then(toml::Value::as_integer)
            && u32::try_from(size).is_ok_and(|size| FONT_SIZES.contains(&size))
        {
            settings.font_size = size as f32;
        }
        if let Some(size) = table
            .get("interface_size")
            .and_then(toml::Value::as_integer)
            && let Ok(size) = u32::try_from(size)
            && super::zoom::SIZES.contains(&size)
        {
            settings.interface_size = size;
        }
        if let Some(width) = table.get("sidebar_width").and_then(toml::Value::as_integer)
            && let Ok(width) = u32::try_from(width)
            && sidebar_widths().contains(&width)
        {
            settings.sidebar_width = Some(width);
        }
        if let Some(width) = table
            .get("conversation_width")
            .and_then(toml::Value::as_integer)
            && let Ok(width) = u32::try_from(width)
            && conversation_widths().contains(&width)
        {
            settings.conversation_width = Some(width);
        }
        if let Some(mode) = table.get("pane_headers").and_then(toml::Value::as_str)
            && let Some((mode, ..)) = PaneHeaders::ALL.iter().find(|(_, name, _)| *name == mode)
        {
            settings.pane_headers = *mode;
        }
        if let Some(setup) = table.get("setup").and_then(toml::Value::as_table) {
            let flag = |key| setup.get(key).and_then(toml::Value::as_bool) == Some(true);
            settings.intro_seen = flag("intro_seen");
            settings.setup_done = flag("done");
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
        text.push_str(&format!("logging = \"{}\"\n", self.logging.name()));
        if self.interface_size != 100 {
            text.push_str(&format!("interface_size = {}\n", self.interface_size));
        }
        if self.pane_headers != PaneHeaders::Always {
            text.push_str(&format!(
                "pane_headers = \"{}\"\n",
                self.pane_headers.name()
            ));
        }
        if let Some(width) = self.sidebar_width {
            text.push_str(&format!("sidebar_width = {width}\n"));
        }
        if let Some(width) = self.conversation_width {
            text.push_str(&format!("conversation_width = {width}\n"));
        }
        if self.intro_seen || self.setup_done {
            text.push_str(&format!(
                "\n[setup]\nintro_seen = {}\ndone = {}\n",
                self.intro_seen, self.setup_done
            ));
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
        crate::diagnostics::set_level(self.logging);
        super::design::set_light(self.light);
        PANE_HEADERS.store(self.pane_headers as u8, Ordering::Relaxed);
        FONT_SIZE.store(self.font_size.to_bits(), Ordering::Relaxed);
        super::zoom::set(self.interface_size);
        super::shortcuts::set(self.shortcuts.clone());
        SIDEBAR_WIDTH.store(self.sidebar_width.unwrap_or(0), Ordering::Relaxed);
        CONVERSATION_WIDTH.store(self.conversation_width.unwrap_or(0), Ordering::Relaxed);
        INTRO_SEEN.store(self.intro_seen, Ordering::Relaxed);
        SETUP_DONE.store(self.setup_done, Ordering::Relaxed);
    }

    /// The settings windows are drawing with now.
    pub fn current() -> Self {
        Self {
            logging: crate::diagnostics::level(),
            light: super::design::is_light(),
            font_size: font_size(),
            interface_size: (super::zoom::factor() * 100.0).round() as u32,
            shortcuts: super::shortcuts::current(),
            sidebar_width: sidebar_width(),
            conversation_width: conversation_width(),
            pane_headers: pane_headers(),
            intro_seen: INTRO_SEEN.load(Ordering::Relaxed),
            setup_done: SETUP_DONE.load(Ordering::Relaxed),
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

/// The dragged conversation panel width in pixels, if any.
pub fn conversation_width() -> Option<u32> {
    Some(CONVERSATION_WIDTH.load(Ordering::Relaxed)).filter(|width| *width > 0)
}

/// Use a dragged conversation width now, without saving it.
pub fn set_conversation_width(width: u32) {
    CONVERSATION_WIDTH.store(
        width.clamp(*conversation_widths().start(), *conversation_widths().end()),
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

/// `$XDG_CONFIG_HOME/agent-interface/settings.toml`, else under `~/.config`.
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
    Some(config.join("agent-interface/settings.toml"))
}

/// Read the saved settings, or the defaults.
pub fn load() -> Settings {
    path()
        .and_then(|path| {
            let config = path.parent()?.parent()?;
            load_from(&crate::compat::config_file(config, "settings.toml")).ok()
        })
        .unwrap_or_default()
}

const MAX_SETTINGS_BYTES: usize = 64 * 1024;

fn load_from(path: &Path) -> io::Result<Settings> {
    // Check the opened handle so a special file or a replaced symlink cannot
    // leave startup waiting for a writer. Ordinary dotfile symlinks still work.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("settings is not a regular file"));
    }
    let mut text = String::new();
    file.take((MAX_SETTINGS_BYTES + 1) as u64)
        .read_to_string(&mut text)?;
    if text.len() > MAX_SETTINGS_BYTES {
        return Err(io::Error::other("settings exceeds 64 KiB"));
    }
    Ok(Settings::parse(&text))
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static PENDING: AtomicU32 = AtomicU32::new(0);
static WRITTEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);

/// Save these settings on a background thread, so slow storage never blocks
/// the windows. Saves run one at a time and an older one never replaces a
/// newer one. `done` receives the outcome.
pub fn save_in_background(settings: Settings, done: impl FnOnce(io::Result<()>) + Send + 'static) {
    spawn_save(settings, done, |task| {
        std::thread::Builder::new().spawn(task).map(|_| ())
    });
}

fn spawn_save(
    settings: Settings,
    done: impl FnOnce(io::Result<()>) + Send + 'static,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> io::Result<()>,
) {
    let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    PENDING.fetch_add(1, Ordering::SeqCst);
    // Capturing the guard also balances accounting if spawning drops the task.
    let pending = Pending;
    let done = std::sync::Arc::new(std::sync::Mutex::new(Some(done)));
    let callback = std::sync::Arc::clone(&done);
    let result = spawn(Box::new(move || {
        let _pending = pending;
        let done = callback.lock().unwrap().take().unwrap();
        let mut written = WRITTEN.lock().unwrap_or_else(|error| error.into_inner());
        if *written > generation {
            return done(Ok(()));
        }
        let result = path()
            .ok_or_else(|| io::Error::other("no home directory"))
            .and_then(|path| save(&settings, &path));
        if result.is_ok() {
            *written = generation;
            tracing::debug!(event = "settings_saved", generation, "Settings saved");
        } else if let Err(error) = &result {
            tracing::warn!(event = "settings_save_failed", generation, error_kind = ?error.kind(), os_error = error.raw_os_error(), "Settings save failed");
        }
        done(result);
    }));
    if let Err(error) = result {
        let done = done.lock().unwrap().take().unwrap();
        done(Err(error));
    }
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
    save_to_temporary(settings, path, &temporary)
}

fn save_to_temporary(settings: &Settings, path: &Path, temporary: &Path) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    let written = file
        .write_all(settings.to_toml().as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(temporary, path));
    if written.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_levels_are_explicit_and_survive_normal_settings_load() {
        assert_eq!(
            Settings::parse("").logging,
            crate::diagnostics::Level::Warnings
        );
        assert_eq!(
            Settings::parse("logging = 'invalid'").logging,
            crate::diagnostics::Level::Warnings
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        for (level, ..) in crate::diagnostics::Level::ALL {
            let settings = Settings {
                logging: level,
                ..Settings::default()
            };
            save(&settings, &path).unwrap();
            assert_eq!(load_from(&path).unwrap(), settings);
        }
    }

    #[test]
    fn save_cleanup_only_removes_its_own_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        let temporary = directory.path().join("temporary");
        std::fs::write(&path, "original").unwrap();
        std::fs::write(&temporary, "other save").unwrap();
        assert!(save_to_temporary(&Settings::default(), &path, &temporary).is_err());
        assert_eq!(std::fs::read_to_string(&temporary).unwrap(), "other save");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        std::fs::remove_file(&temporary).unwrap();
        let destination = directory.path().join("directory");
        std::fs::create_dir(&destination).unwrap();
        assert!(save_to_temporary(&Settings::default(), &destination, &temporary).is_err());
        assert!(!temporary.exists());
        save_to_temporary(&Settings::default(), &path, &temporary).unwrap();
        assert_eq!(load_from(&path).unwrap(), Settings::default());
    }

    #[test]
    fn failed_save_thread_reports_once_and_balances_pending() {
        let before = PENDING.load(Ordering::SeqCst);
        let (sender, receiver) = std::sync::mpsc::channel();
        spawn_save(
            Settings::default(),
            move |result| sender.send(result.unwrap_err().kind()).unwrap(),
            |_task| Err(io::Error::from(io::ErrorKind::WouldBlock)),
        );
        assert_eq!(receiver.recv().unwrap(), io::ErrorKind::WouldBlock);
        assert!(receiver.recv().is_err());
        assert_eq!(PENDING.load(Ordering::SeqCst), before);
    }

    #[test]
    fn settings_reads_are_bounded_and_reject_special_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.toml");
        let text = "theme = 'light'\n";
        std::fs::write(&path, text).unwrap();
        let alias = directory.path().join("alias.toml");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert_eq!(load_from(&alias).unwrap(), Settings::parse(text));

        let boundary = format!("{text}#{}", " ".repeat(MAX_SETTINGS_BYTES - text.len() - 1));
        std::fs::write(&path, &boundary).unwrap();
        assert_eq!(load_from(&path).unwrap(), Settings::parse(text));
        std::fs::write(&path, format!("{boundary} ")).unwrap();
        assert!(load_from(&path).is_err());

        std::fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(load_from(&alias).is_err());
    }

    #[test]
    fn the_settings_file_follows_xdg_then_home_then_the_account() {
        let account = || Some(PathBuf::from("/account"));
        let file = "agent-interface/settings.toml";
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
            logging: crate::diagnostics::Level::Warnings,
            light: true,
            font_size: 16.0,
            interface_size: 150,
            shortcuts,
            sidebar_width: Some(260),
            conversation_width: Some(640),
            pane_headers: PaneHeaders::Hover,
            intro_seen: true,
            setup_done: false,
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
        let odd = Settings::parse(
            "theme = \"sepia\"\nfont_size = 99\ninterface_size = 130\nsidebar_width = 9000\nconversation_width = 10\nother = 1\n",
        );
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
