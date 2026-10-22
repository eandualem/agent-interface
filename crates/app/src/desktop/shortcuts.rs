//! Configurable keyboard shortcuts for workspace commands.
//!
//! Each command has a default chord; the settings file can replace it. Keys
//! the terminal or the application menu own (copy, paste, numbered selection,
//! Settings, New Window, Open Library, Close Window, Quit) stay fixed.

use std::{
    collections::HashMap,
    sync::{LazyLock, RwLock},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    NewTab,
    SplitRight,
    SplitBelow,
    Attach,
    RenameTab,
    NextPane,
    PreviousPane,
    NextTab,
    PreviousTab,
    FocusPane,
    NewWorkspace,
    RenameWorkspace,
    NextWorkspace,
    PreviousWorkspace,
    FocusPanel,
}

/// Every configurable command: its settings key, label, group and default.
pub const COMMANDS: &[(Command, &str, &str, &str, &str)] = &[
    (
        Command::NewTab,
        "new-tab",
        "New tab",
        "Tabs and panes",
        "cmd-t",
    ),
    (
        Command::SplitRight,
        "split-right",
        "Split right",
        "Tabs and panes",
        "cmd-d",
    ),
    (
        Command::SplitBelow,
        "split-below",
        "Split below",
        "Tabs and panes",
        "cmd-shift-d",
    ),
    (
        Command::Attach,
        "attach",
        "Attach session",
        "Tabs and panes",
        "cmd-shift-a",
    ),
    (
        Command::RenameTab,
        "rename-tab",
        "Rename tab",
        "Tabs and panes",
        "cmd-r",
    ),
    (
        Command::NextPane,
        "next-pane",
        "Next pane",
        "Tabs and panes",
        "cmd-]",
    ),
    (
        Command::PreviousPane,
        "previous-pane",
        "Previous pane",
        "Tabs and panes",
        "cmd-[",
    ),
    (
        Command::NextTab,
        "next-tab",
        "Next tab",
        "Tabs and panes",
        "cmd-shift-]",
    ),
    (
        Command::PreviousTab,
        "previous-tab",
        "Previous tab",
        "Tabs and panes",
        "cmd-shift-[",
    ),
    (
        Command::FocusPane,
        "focus-pane",
        "Focus one pane / restore",
        "Tabs and panes",
        "cmd-shift-enter",
    ),
    (
        Command::NewWorkspace,
        "new-workspace",
        "New workspace",
        "Workspaces",
        "cmd-shift-n",
    ),
    (
        Command::RenameWorkspace,
        "rename-workspace",
        "Rename workspace",
        "Workspaces",
        "cmd-shift-r",
    ),
    (
        Command::NextWorkspace,
        "next-workspace",
        "Next workspace",
        "Workspaces",
        "cmd-alt-right",
    ),
    (
        Command::PreviousWorkspace,
        "previous-workspace",
        "Previous workspace",
        "Workspaces",
        "cmd-alt-left",
    ),
    (
        Command::FocusPanel,
        "focus-panel",
        "Focus the panel",
        "Workspaces",
        "cmd-b",
    ),
];

/// Keys a command may not take, because something else always owns them.
const RESERVED: &[&str] = &[
    "cmd-c",
    "cmd-v",
    "cmd-q",
    "cmd-n",
    "cmd-o",
    "cmd-w",
    "cmd-,",
    "ctrl-shift-c",
    "ctrl-shift-v",
];

/// One key with its modifiers, independent of keyboard-layout shifting.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub key: String,
}

impl Chord {
    /// Parse `cmd-shift-]` style text. The last part is the key.
    pub fn parse(text: &str) -> Option<Self> {
        let (modifiers, key) = match text.strip_suffix("--") {
            Some(rest) => (rest, "-"),
            None => text.rsplit_once('-').unwrap_or(("", text)),
        };
        let mut chord = Self::from_key(key)?;
        for modifier in modifiers.split('-').filter(|part| !part.is_empty()) {
            match modifier {
                "cmd" => chord.cmd = true,
                "ctrl" => chord.ctrl = true,
                "alt" => chord.alt = true,
                "shift" => chord.shift = true,
                _ => return None,
            }
        }
        Some(chord)
    }

    fn from_key(key: &str) -> Option<Self> {
        if key.is_empty() || key.chars().any(char::is_whitespace) {
            return None;
        }
        // Shifted punctuation arrives as the character it produces.
        let unshifted = match key {
            "{" => Some("["),
            "}" => Some("]"),
            "!" => Some("1"),
            "@" => Some("2"),
            "#" => Some("3"),
            "$" => Some("4"),
            "%" => Some("5"),
            "^" => Some("6"),
            "&" => Some("7"),
            "*" => Some("8"),
            "(" => Some("9"),
            ")" => Some("0"),
            "_" => Some("-"),
            "+" => Some("="),
            "|" => Some("\\"),
            ":" => Some(";"),
            "\"" => Some("'"),
            "<" => Some(","),
            ">" => Some("."),
            "?" => Some("/"),
            "~" => Some("`"),
            _ => None,
        };
        Some(Self {
            cmd: false,
            ctrl: false,
            alt: false,
            shift: unshifted.is_some(),
            // Only ASCII letters fold: other layouts can report a shifted and
            // an unshifted key as distinct characters without a Shift flag.
            key: unshifted.unwrap_or(key).to_ascii_lowercase(),
        })
    }

    /// The chord a key press stands for. Fn combinations have none: they
    /// cannot be saved, so they never run a command.
    pub fn from_keystroke(keystroke: &gpui::Keystroke) -> Option<Self> {
        let modifiers = keystroke.modifiers;
        if modifiers.function {
            return None;
        }
        let mut chord = Self::from_key(&keystroke.key)?;
        chord.cmd = modifiers.platform;
        chord.ctrl = modifiers.control;
        chord.alt = modifiers.alt;
        chord.shift |= modifiers.shift;
        Some(chord)
    }

    /// The settings-file spelling, which [`Chord::parse`] reads back.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for (on, name) in [
            (self.cmd, "cmd-"),
            (self.ctrl, "ctrl-"),
            (self.alt, "alt-"),
            (self.shift, "shift-"),
        ] {
            if on {
                text.push_str(name);
            }
        }
        text.push_str(&self.key);
        text
    }

    /// The menu spelling, such as `⌘⇧]`.
    pub fn label(&self) -> String {
        let mut label = String::new();
        for (on, glyph) in [
            (self.ctrl, "⌃"),
            (self.alt, "⌥"),
            (self.shift, "⇧"),
            (self.cmd, "⌘"),
        ] {
            if on {
                label.push_str(glyph);
            }
        }
        label.push_str(match self.key.as_str() {
            "enter" => "↩",
            "left" => "←",
            "right" => "→",
            "up" => "↑",
            "down" => "↓",
            "escape" => "⎋",
            "tab" => "⇥",
            "space" => "Space",
            "backspace" => "⌫",
            "delete" => "⌦",
            key => return label + &key.to_ascii_uppercase(),
        });
        label
    }

    /// Why this chord cannot be a command's shortcut, if it cannot.
    pub fn unusable(&self) -> Option<&'static str> {
        if !self.cmd && !self.ctrl && !self.alt {
            return Some("Use ⌘, ⌃ or ⌥ so typing in terminals is not affected");
        }
        if RESERVED.contains(&self.to_text().as_str()) {
            return Some("That key belongs to copy, paste or the application menu");
        }
        if self.cmd
            && !self.ctrl
            && !self.shift
            && self.key.len() == 1
            && self.key.as_bytes()[0].is_ascii_digit()
        {
            return Some("⌘ and ⌘⌥ with a number select tabs and workspaces");
        }
        None
    }
}

/// The chord assigned to every command.
#[derive(Clone, Debug, PartialEq)]
pub struct Bindings(HashMap<Command, Chord>);

impl Default for Bindings {
    fn default() -> Self {
        Self(
            COMMANDS
                .iter()
                .map(|(command, _, _, _, default)| {
                    (
                        *command,
                        Chord::parse(default).expect("valid default shortcut"),
                    )
                })
                .collect(),
        )
    }
}

impl Bindings {
    pub fn chord(&self, command: Command) -> &Chord {
        &self.0[&command]
    }

    /// The command a key press runs.
    pub fn command_for(&self, keystroke: &gpui::Keystroke) -> Option<Command> {
        let chord = Chord::from_keystroke(keystroke)?;
        self.0
            .iter()
            .find(|(_, bound)| **bound == chord)
            .map(|(command, _)| *command)
    }

    /// Give `command` a new chord, refusing one another command already uses.
    pub fn assign(&mut self, command: Command, chord: Chord) -> Result<(), String> {
        if let Some(reason) = chord.unusable() {
            return Err(reason.into());
        }
        if let Some((other, ..)) = COMMANDS
            .iter()
            .find(|(other, ..)| *other != command && self.0.get(other) == Some(&chord))
        {
            let label = COMMANDS
                .iter()
                .find(|(candidate, ..)| candidate == other)
                .map_or("another command", |(_, _, label, ..)| label);
            return Err(format!("{} is already used by {label}", chord.label()));
        }
        self.0.insert(command, chord);
        Ok(())
    }

    /// Commands whose chord differs from the default, as settings entries.
    pub fn changed(&self) -> Vec<(&'static str, String)> {
        let defaults = Self::default();
        COMMANDS
            .iter()
            .filter(|(command, ..)| self.0[command] != defaults.0[command])
            .map(|(command, id, ..)| (*id, self.0[command].to_text()))
            .collect()
    }

    /// Apply saved entries together, so two commands may swap keys. Unknown
    /// commands and unusable chords are ignored; a saved chord that clashes
    /// with another command's returns that command to its default.
    pub fn from_entries<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let defaults = Self::default();
        let mut bindings = defaults.clone();
        for (id, text) in entries {
            if let (Some((command, ..)), Some(chord)) = (
                COMMANDS.iter().find(|(_, candidate, ..)| *candidate == id),
                Chord::parse(text),
            ) && chord.unusable().is_none()
            {
                bindings.0.insert(*command, chord);
            }
        }
        while let Some(command) = COMMANDS
            .iter()
            .map(|(command, ..)| *command)
            .find(|command| {
                bindings.0[command] != defaults.0[command]
                    && bindings
                        .0
                        .iter()
                        .any(|(other, chord)| other != command && *chord == bindings.0[command])
            })
        {
            bindings.0.insert(command, defaults.0[&command].clone());
        }
        bindings
    }
}

static CURRENT: LazyLock<RwLock<Bindings>> = LazyLock::new(|| RwLock::new(Bindings::default()));

/// The bindings every window uses now.
pub fn current() -> Bindings {
    CURRENT
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

pub fn set(bindings: Bindings) {
    *CURRENT.write().unwrap_or_else(|error| error.into_inner()) = bindings;
}

/// The command a key press runs with the current bindings.
pub fn command_for(keystroke: &gpui::Keystroke) -> Option<Command> {
    CURRENT
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .command_for(keystroke)
}

/// A command's current key, as menus show it.
pub fn label(command: Command) -> String {
    CURRENT
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .chord(command)
        .label()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_round_trip_and_match_layout_shifted_keys() {
        for (_, _, _, _, default) in COMMANDS {
            let chord = Chord::parse(default).unwrap();
            assert_eq!(Chord::parse(&chord.to_text()), Some(chord));
        }
        assert_eq!(Chord::parse("cmd-shift-]").unwrap().label(), "⇧⌘]");
        assert_eq!(Chord::parse("cmd--").unwrap().key, "-");
        assert!(Chord::parse("hyper-x").is_none());
        let bindings = Bindings::default();
        // macOS reports Command-Shift-] as `}` with Shift held.
        let key = gpui::Keystroke::parse("cmd-shift-}").unwrap();
        assert_eq!(bindings.command_for(&key), Some(Command::NextTab));
        let key = gpui::Keystroke::parse("cmd-t").unwrap();
        assert_eq!(bindings.command_for(&key), Some(Command::NewTab));
        let key = gpui::Keystroke::parse("cmd-fn-t").unwrap();
        assert_eq!(bindings.command_for(&key), None);
        // A layout's shifted letter stays distinct from its unshifted one.
        assert_ne!(Chord::parse("cmd-Ö"), Chord::parse("cmd-ö"));
    }

    #[test]
    fn assignments_refuse_clashes_and_reserved_keys_and_save_only_changes() {
        let mut bindings = Bindings::default();
        let chord = |text| Chord::parse(text).unwrap();
        assert!(bindings.assign(Command::NewTab, chord("cmd-d")).is_err());
        assert!(bindings.assign(Command::NewTab, chord("cmd-c")).is_err());
        assert!(bindings.assign(Command::NewTab, chord("x")).is_err());
        assert!(bindings.assign(Command::NewTab, chord("cmd-3")).is_err());
        assert!(
            bindings
                .assign(Command::NewTab, chord("ctrl-alt-t"))
                .is_ok()
        );
        assert_eq!(
            bindings.changed(),
            vec![("new-tab", "ctrl-alt-t".to_owned())]
        );
        let restored = Bindings::from_entries([("new-tab", "ctrl-alt-t"), ("unknown", "cmd-k")]);
        assert_eq!(restored, bindings);
        assert_eq!(
            Bindings::from_entries([("split-right", "cmd-t")]),
            Bindings::default()
        );
        let swapped = Bindings::from_entries([("new-tab", "cmd-d"), ("split-right", "cmd-t")]);
        assert_eq!(swapped.chord(Command::NewTab), &chord("cmd-d"));
        assert_eq!(swapped.chord(Command::SplitRight), &chord("cmd-t"));
    }
}
