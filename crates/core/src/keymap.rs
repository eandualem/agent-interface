//! Immutable, validated Python-compatible keymaps. Keys are data, never commands.

use std::collections::HashMap;

mod keys;

/// Maximum accepted UTF-8 file size, matching the Python reader.
pub const MAX_BYTES: usize = 64 * 1024;
const DEFAULTS: &str = include_str!("keymap/defaults.toml");
const YIELDING: &[&str] = &[
    "tab-options",
    "workspace-options",
    "refresh-viewer",
    "copy-selection",
    "show-agents",
];

/// One action's effective prefix and direct keys, in declaration order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    /// Stable identifier accepted by the Python keymap format.
    pub action: &'static str,
    /// Human-readable action name.
    pub label: &'static str,
    /// Keys pressed after the viewer prefix; an empty list disables this route.
    pub keys: Vec<String>,
    /// Native direct shortcut notation; an empty list disables this route.
    pub direct: Vec<String>,
}

/// An effective keymap, validated once and retained for a window's lifetime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keymap {
    prefix: String,
    actions: Vec<Bindings>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::parse(b"").expect("shipped keymap is valid")
    }
}

impl Keymap {
    /// Parse bounded TOML 1.0 bytes. Omitted actions keep their defaults; each
    /// supplied array replaces that action's keys in that section, including [].
    /// Unknown options/actions, unsafe keys, aliases that collide and direct keys
    /// shadowing prefix input are errors. This never reads or writes files.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err(format!("keymap exceeds {MAX_BYTES} bytes"));
        }
        let source = std::str::from_utf8(bytes).map_err(|_| "keymap must be UTF-8".to_owned())?;
        if source.starts_with('\u{feff}') {
            return Err("keymap must not begin with a UTF-8 byte order mark".into());
        }
        let table: toml::Table = toml::from_str(source).map_err(|e: toml::de::Error| {
            // Display includes raw source, which may contain terminal controls.
            format!("invalid keymap TOML: {}", e.message().escape_debug())
        })?;
        for key in table.keys() {
            if !matches!(key.as_str(), "prefix" | "bindings" | "direct") {
                return Err(format!("unknown keymap option: {key:?}"));
            }
        }
        let prefix = match table.get("prefix") {
            None => "C-g",
            Some(value) => value.as_str().ok_or("prefix: expected a key string")?,
        };
        let prefix = keys::prefix(prefix).map_err(|e| format!("prefix: {e}"))?;
        if prefix == "Escape" {
            return Err("prefix: Escape is reserved for cancellation".into());
        }
        let defaults: toml::Table = toml::from_str(DEFAULTS).expect("shipped TOML is valid");
        let mut actions: Vec<_> = ACTIONS
            .iter()
            .map(|&(action, label)| Bindings {
                action,
                label,
                keys: Vec::new(),
                direct: Vec::new(),
            })
            .collect();
        for section in ["bindings", "direct"] {
            let empty = toml::Table::new();
            let configured = match table.get(section) {
                None => &empty,
                Some(value) => value
                    .as_table()
                    .ok_or_else(|| format!("{section}: expected a table of action = [keys]"))?,
            };
            for action in configured.keys() {
                if !ACTIONS.iter().any(|&(name, _)| name == action) {
                    return Err(format!("{section}: unknown action {action:?}"));
                }
            }
            let mut parsed = Vec::new();
            for entry in &actions {
                let value = configured
                    .get(entry.action)
                    .unwrap_or(&defaults[section][entry.action]);
                let list = value
                    .as_array()
                    .filter(|keys| keys.len() <= 32)
                    .ok_or_else(|| {
                        format!(
                            "{section}.{}: expected a list of at most 32 keys",
                            entry.action
                        )
                    })?;
                let mut canonical = Vec::new();
                for value in list {
                    let key = value.as_str().ok_or_else(|| {
                        format!("{section}.{}: expected a key string", entry.action)
                    })?;
                    canonical.push(
                        if section == "bindings" {
                            keys::prefix(key)
                        } else {
                            keys::direct(key)
                        }
                        .map_err(|e| format!("{section}.{}: {e}", entry.action))?,
                    );
                }
                parsed.push(canonical);
            }
            let mut claimed: Vec<_> = actions
                .iter()
                .zip(&parsed)
                .filter(|(entry, _)| configured.contains_key(entry.action))
                .flat_map(|(_, keys)| keys.iter().cloned())
                .collect();
            if section == "bindings" {
                claimed.extend([prefix.clone(), "Escape".into()]);
            }
            let mut seen = HashMap::new();
            for (entry, mut keys) in actions.iter_mut().zip(parsed) {
                if YIELDING.contains(&entry.action) && !configured.contains_key(entry.action) {
                    keys.retain(|key| !claimed.contains(key));
                }
                for key in &keys {
                    if section == "bindings" && (key == &prefix || key == "Escape") {
                        return Err(format!(
                            "bindings.{}: {key:?} is reserved for prefix/cancel",
                            entry.action
                        ));
                    }
                    if let Some(previous) = seen.insert(key.clone(), entry.action) {
                        return Err(format!(
                            "{section}: duplicate key {key:?} for {previous:?} and {:?}",
                            entry.action
                        ));
                    }
                }
                if section == "bindings" {
                    entry.keys = keys;
                } else {
                    entry.direct = keys;
                }
            }
        }
        let mut physical: HashMap<_, _> = actions
            .iter()
            .flat_map(|entry| {
                entry
                    .keys
                    .iter()
                    .map(move |key| (key.clone(), format!("bindings.{}", entry.action)))
            })
            .collect();
        physical.insert(prefix.clone(), "prefix".into());
        physical.insert("Escape".into(), "Escape cancellation".into());
        for entry in &actions {
            for trigger in &entry.direct {
                if let Some(owner) = keys::physical(trigger).and_then(|key| physical.get(&key)) {
                    return Err(format!(
                        "direct.{}: {trigger:?} shadows {owner}; choose a different direct trigger or prefix binding",
                        entry.action
                    ));
                }
            }
        }
        Ok(Self { prefix, actions })
    }

    /// The canonical prefix key.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The complete effective action list, including explicitly disabled actions.
    pub fn actions(&self) -> &[Bindings] {
        &self.actions
    }

    /// Print a complete configuration that reads back without restoring defaults
    /// for any disabled action. No file is written.
    pub fn to_toml(&self) -> String {
        let mut text = format!(
            "prefix = {}\n",
            serde_json::to_string(&self.prefix).unwrap()
        );
        for section in ["bindings", "direct"] {
            text.push_str(&format!("\n[{section}]\n"));
            for entry in &self.actions {
                let keys = if section == "bindings" {
                    &entry.keys
                } else {
                    &entry.direct
                };
                text.push_str(&format!(
                    "{} = {}\n",
                    entry.action,
                    serde_json::to_string(keys).unwrap()
                ));
            }
        }
        text
    }

    /// Describe the effective map. Native event dispatch is a separate integration.
    pub fn help(&self) -> String {
        let mut text = format!("Viewer prefix: {}\nPrefix, then key:\n", self.prefix);
        for entry in &self.actions {
            if !entry.keys.is_empty() {
                let keys = entry
                    .keys
                    .iter()
                    .map(|k| keys::prefix_label(k))
                    .collect::<Vec<_>>()
                    .join(" / ");
                text.push_str(&format!("  {keys:<20} {}\n", entry.label));
            }
        }
        text.push_str(&format!("  Escape               Cancel prefix\n  {:<20} Send literal prefix\n\nNative direct shortcuts:\n", self.prefix));
        for entry in &self.actions {
            if !entry.direct.is_empty() {
                let keys = entry
                    .direct
                    .iter()
                    .map(|k| keys::direct_label(k))
                    .collect::<Vec<_>>()
                    .join(" / ");
                text.push_str(&format!("  {keys:<20} {}\n", entry.label));
            }
        }
        text
    }
}

const ACTIONS: &[(&str, &str)] = &[
    ("new-tab", "New tab"),
    ("split-right", "Split right"),
    ("split-below", "Split below"),
    ("attach", "Attach session"),
    ("rename-tab", "Rename tab"),
    ("tab-options", "Tab options"),
    ("next-tab", "Next tab"),
    ("previous-tab", "Previous tab"),
    ("next-pane", "Next pane"),
    ("previous-pane", "Previous pane"),
    ("focus", "Focus / restore layout"),
    ("workspaces", "Workspace menu"),
    ("workspace-options", "Workspace options"),
    ("new-workspace", "New workspace"),
    ("rename-workspace", "Rename workspace"),
    ("next-workspace", "Next workspace"),
    ("previous-workspace", "Previous workspace"),
    ("close-pane", "Close pane"),
    ("close-tab", "Close tab"),
    ("copy-selection", "Copy selection"),
    ("sidebar", "Focus navigation"),
    ("refresh-viewer", "Refresh viewer"),
    ("show-agents", "Show / hide agents"),
    ("quit", "Detach viewer"),
    ("select-tab-1", "Select tab 1"),
    ("select-tab-2", "Select tab 2"),
    ("select-tab-3", "Select tab 3"),
    ("select-tab-4", "Select tab 4"),
    ("select-tab-5", "Select tab 5"),
    ("select-tab-6", "Select tab 6"),
    ("select-tab-7", "Select tab 7"),
    ("select-tab-8", "Select tab 8"),
    ("select-tab-9", "Select tab 9"),
    ("select-workspace-1", "Select workspace 1"),
    ("select-workspace-2", "Select workspace 2"),
    ("select-workspace-3", "Select workspace 3"),
    ("select-workspace-4", "Select workspace 4"),
    ("select-workspace-5", "Select workspace 5"),
    ("select-workspace-6", "Select workspace 6"),
    ("select-workspace-7", "Select workspace 7"),
    ("select-workspace-8", "Select workspace 8"),
    ("select-workspace-9", "Select workspace 9"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaps_unbinding_and_yielding_defaults_round_trip_without_restoring_keys() {
        let map = Keymap::parse(
            br#"
prefix = "C-b"
[bindings]
new-tab = ["m"]
close-tab = []
[direct]
new-tab = ["ctrl+alt+t", "super+e"]
close-pane = []
quit = ["super+t"]
"#,
        )
        .unwrap();
        let action = |name| {
            map.actions()
                .iter()
                .find(|entry| entry.action == name)
                .unwrap()
        };
        assert_eq!(map.prefix(), "C-b");
        assert_eq!(action("new-tab").keys, ["m"]);
        assert!(action("tab-options").keys.is_empty());
        assert!(action("close-tab").keys.is_empty());
        assert_eq!(action("split-right").keys, ["v", "%"]);
        assert_eq!(action("workspace-options").keys, ["M"]);
        assert_eq!(action("quit").direct, ["super+t"]);
        assert_eq!(Keymap::parse(map.to_toml().as_bytes()).unwrap(), map);
        let help = map.help();
        assert!(help.contains("⌃⌥T / ⌘E"));
        assert!(
            !help
                .lines()
                .any(|line| line.ends_with("Close pane") && line.contains('⌘'))
        );
        assert!(!help.lines().any(|line| line.ends_with("Tab options")));
        assert_eq!(Keymap::default().actions().len(), 42);
    }

    #[test]
    fn key_aliases_keep_case_and_modifier_meaning() {
        for (raw, canonical) in [
            ("C-I", "Tab"),
            ("C-M", "Enter"),
            ("C-[", "Escape"),
            ("C-?", "BSpace"),
            ("C-@", "C-Space"),
            ("C-Space", "C-Space"),
            ("C-H", "C-h"),
            ("C-M-X", "M-C-x"),
            ("M-C-X", "M-C-x"),
            ("M-X", "M-X"),
            ("X", "X"),
            ("backspace", "BSpace"),
            ("bacKspace", "BSpace"),
            ("return", "Enter"),
            ("esc", "Escape"),
            ("f2", "F2"),
        ] {
            assert_eq!(keys::prefix(raw).unwrap(), canonical);
            assert_eq!(keys::prefix(canonical).unwrap(), canonical);
        }
        assert_eq!(keys::direct("shift+super+t").unwrap(), "super+shift+t");
        for key in [
            "\n",
            "C-C-t",
            "C-1",
            "$(id)",
            "User1",
            "MouseDown1Pane",
            ";",
            "\\",
            "é",
            "",
        ] {
            assert!(keys::prefix(key).is_err(), "{key}");
        }
        for key in [
            "t",
            "shift+t",
            "super+super+t",
            "global:super+t",
            "super+t=quit",
            "cmd+t",
            "super+T",
        ] {
            assert!(keys::direct(key).is_err(), "{key}");
        }
    }

    #[test]
    fn invalid_configuration_and_colliding_aliases_are_reported() {
        assert!(Keymap::parse(b"\xef\xbb\xbfprefix = 'C-b'\n").is_err());
        let diagnostic = Keymap::parse(b"#\x1b[2J\n").unwrap_err();
        assert!(!diagnostic.contains('\x1b'));
        for (text, error) in [
            ("unknown = 1", "unknown keymap option"),
            ("prefix = true", "prefix"),
            ("prefix = 'C-['", "cancellation"),
            ("bindings = []", "bindings"),
            ("[bindings]\nnot-an-action = []", "unknown action"),
            ("[bindings]\nnew-tab = 't'", "bindings.new-tab"),
            ("[bindings]\nnew-tab = ['t','t']", "duplicate"),
            (
                "[bindings]\nnew-tab = ['Tab']\nattach = ['C-i']",
                "duplicate",
            ),
            ("[bindings]\nnew-tab = ['C-G']", "reserved"),
            (
                "[bindings]\nnew-tab = ['m']\ntab-options = ['m']",
                "duplicate",
            ),
            ("[direct]\nnew-tab = ['shift+super+w']", "duplicate"),
            ("prefix = 'C-g'\nprefix = 'C-a'", "TOML"),
        ] {
            assert!(
                Keymap::parse(text.as_bytes()).unwrap_err().contains(error),
                "{text}"
            );
        }
        let too_many = format!("[bindings]\nnew-tab = [{}]", vec!["'F2'"; 33].join(","));
        assert!(
            Keymap::parse(too_many.as_bytes())
                .unwrap_err()
                .contains("at most 32")
        );
        for (binding, trigger) in [
            ("M-t", "alt+t"),
            ("M-T", "alt+shift+t"),
            ("M-C-T", "ctrl+alt+t"),
            ("Tab", "ctrl+i"),
            ("M-Left", "alt+left"),
            ("C-Space", "ctrl+shift+2"),
            ("M-BTab", "alt+shift+tab"),
            ("M-{", "alt+shift+bracket_left"),
        ] {
            let text =
                format!("[bindings]\nnew-tab = ['{binding}']\n[direct]\nquit = ['{trigger}']");
            assert!(
                Keymap::parse(text.as_bytes())
                    .unwrap_err()
                    .contains("shadows"),
                "{text}"
            );
        }
        assert!(
            Keymap::parse(b"[direct]\nquit = ['ctrl+g']")
                .unwrap_err()
                .contains("shadows")
        );
        assert!(
            Keymap::parse(b"[direct]\nquit = ['ctrl+bracket_left']")
                .unwrap_err()
                .contains("shadows")
        );
        assert!(
            Keymap::parse(
                b"[bindings]\nnew-tab = ['M-T']\n[direct]\nquit = ['alt+t','super+ctrl+g']"
            )
            .is_ok()
        );
    }
}
