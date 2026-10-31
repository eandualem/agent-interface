//! Bundled vector icons: a curated subset of Lucide 0.460.0 (ISC licence,
//! `assets/icons/LICENSE-lucide`), plus this project's own `agent-*`,
//! `step-check` and `arrow-head` shapes.
//! GPUI draws each as a mask in the text color.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        const ICONS: &[(&str, &[u8])] = &[
            $(($name, include_bytes!(concat!("../../assets/icons/", $name, ".svg")))),*
        ];
    };
}

icons!(
    "locate-fixed",
    "link-2-off",
    "clock",
    "info",
    "circle-help",
    "arrow-up",
    "arrow-left-right",
    "arrow-down-left",
    "arrow-up-right",
    "chevron-down",
    "chevron-up",
    "hash",
    "plus",
    "minus",
    "ellipsis",
    "settings",
    "x",
    "terminal",
    "bot",
    "circle",
    "briefcase",
    "code",
    "rocket",
    "star",
    "heart",
    "house",
    "folder",
    "globe",
    "book-open",
    "graduation-cap",
    "flask-conical",
    "palette",
    "music",
    "camera",
    "film",
    "gamepad-2",
    "coffee",
    "flame",
    "zap",
    "sparkles",
    "gem",
    "diamond",
    "triangle",
    "square",
    "flag",
    "pencil",
    "brain",
    "cpu",
    "server",
    "database",
    "cloud",
    "shield",
    "lock",
    "mail",
    "message-square",
    "calendar",
    "chart-line",
    "dollar-sign",
    "leaf",
    "sun",
    "moon",
    "mountain",
    "plane",
    "user",
    "users",
    "building-2",
    "wrench",
    "bug",
    "git-branch",
    "package",
    "layers",
    "chevrons-up-down",
    "search",
    "sliders-horizontal",
    "link-2",
    "chevron-right",
    "check",
    "unplug",
    "square-plus",
    "folder-open",
    "keyboard",
    "circle-check",
    "agent-arc",
    "agent-play",
    "step-check",
    "arrow-head",
    "copy",
    "download",
    "activity",
    "chevron-left",
    "layout-grid",
    "align-left",
    "send",
    "file",
    "file-text",
    "refresh-cw",
    "history",
    "corner-down-right",
    "panel-right-close",
    "sheet",
);

/// Icons a workspace can show, in the order the picker offers them.
pub const WORKSPACE: &[&str] = &[
    "briefcase",
    "code",
    "terminal",
    "rocket",
    "star",
    "heart",
    "house",
    "folder",
    "globe",
    "book-open",
    "graduation-cap",
    "flask-conical",
    "palette",
    "music",
    "camera",
    "film",
    "gamepad-2",
    "coffee",
    "flame",
    "zap",
    "sparkles",
    "gem",
    "diamond",
    "flag",
    "pencil",
    "brain",
    "bot",
    "cpu",
    "server",
    "database",
    "cloud",
    "shield",
    "lock",
    "mail",
    "message-square",
    "calendar",
    "chart-line",
    "dollar-sign",
    "leaf",
    "sun",
    "moon",
    "mountain",
    "plane",
    "user",
    "users",
    "building-2",
    "wrench",
    "bug",
    "git-branch",
    "package",
    "layers",
    "circle",
    "triangle",
    "square",
];

/// The asset path GPUI loads an icon from.
pub fn path(name: &str) -> SharedString {
    format!("icons/{name}.svg").into()
}

/// A workspace's icon: its chosen symbol, else the native counterpart of an
/// older one-glyph icon, else none (the workspace shows its number).
pub fn workspace(symbol: Option<&str>, glyph: Option<&str>) -> Option<&'static str> {
    if let Some(symbol) = symbol
        && let Some(name) = WORKSPACE.iter().find(|name| **name == symbol)
    {
        return Some(name);
    }
    Some(match glyph? {
        "◆" => "diamond",
        "●" => "circle",
        "▲" => "triangle",
        "■" => "square",
        "★" => "star",
        "✦" => "sparkles",
        "◈" => "gem",
        "⚑" => "flag",
        "⌂" => "house",
        "✎" => "pencil",
        "⚙" => "settings",
        "♪" => "music",
        _ => return None,
    })
}

/// Serves the bundled icons to GPUI.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(path
            .strip_prefix("icons/")
            .and_then(|name| name.strip_suffix(".svg"))
            .and_then(|name| ICONS.iter().find(|(icon, _)| *icon == name))
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .map(|(name, _)| self::path(name))
            .filter(|asset| asset.starts_with(path))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_icon_is_bundled_and_older_glyphs_map_to_icons() {
        for name in WORKSPACE {
            assert!(
                Assets.load(&path(name)).unwrap().is_some(),
                "{name} is not bundled"
            );
        }
        assert_eq!(workspace(Some("rocket"), Some("★")), Some("rocket"));
        assert_eq!(workspace(Some("unknown"), Some("★")), Some("star"));
        assert_eq!(workspace(None, Some("⌂")), Some("house"));
        assert_eq!(workspace(None, Some("Z")), None);
        assert_eq!(workspace(None, None), None);
    }
}
