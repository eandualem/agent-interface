//! Import a VS Code colour theme: its `colors` map, read onto the app's
//! tokens. A colour the theme leaves out keeps the built-in Dark or Light
//! theme's value; tints the app needs and VS Code has no key for are made
//! from the theme's own colours. Syntax colours (`tokenColors`) have no
//! place in the app and are left out. A theme file's `include`d parents are
//! read first, and the theme chosen in VS Code on this machine can be found
//! without asking for its file.

use std::path::{Path, PathBuf};

use gpui::Rgba;
use serde_json::{Map, Value};

use super::{Error, jsonc};
use crate::desktop::design::{self, Design};

/// The largest VS Code file read: a theme, a settings file or an
/// extension's manifest.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// How many parents a theme file may include in a chain.
const MAX_INCLUDES: usize = 8;

/// The theme `text` describes, named from the file when it has no name, as
/// a full theme file of the app's own.
#[cfg(test)]
fn import(text: &str, fallback_name: &str) -> Result<(String, String), Error> {
    let value = jsonc::parse(text)?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::new("A VS Code theme is a JSON object, { … }"))?;
    let colors = match object.get("colors") {
        Some(Value::Object(colors)) => colors,
        _ => {
            return Err(Error::new(
                "This file has no \"colors\" map, so it is not a VS Code colour theme",
            ));
        }
    };
    let light = object.get("type").and_then(Value::as_str).map(is_light);
    let name = object.get("name").and_then(Value::as_str);
    Ok(build(colors, light, name.unwrap_or(fallback_name)))
}

/// The theme in the file at `path`, with the parents it includes. `light`
/// says which kind it is when the file does not.
pub fn import_file(
    path: &Path,
    light: Option<bool>,
    name: Option<&str>,
) -> Result<(String, String), Error> {
    let mut colors = Map::new();
    let mut found_light = None;
    let mut found_name = None;
    let mut found_colors = false;
    let mut next = Some(path.to_path_buf());
    let mut layers = Vec::new();
    while let Some(file) = next.take() {
        if layers.len() > MAX_INCLUDES {
            return Err(Error::new("The theme includes too many parent themes"));
        }
        let text = read(&file)?;
        let value = jsonc::parse(&text)
            .map_err(|error| Error::new(format!("{}: {error}", file.display())))?;
        let object = value.as_object().cloned().unwrap_or_default();
        if let Some(include) = object.get("include").and_then(Value::as_str) {
            next = file.parent().map(|folder| folder.join(include));
        }
        layers.push(object);
    }
    // Parents first, so each theme's own colours win.
    for object in layers.iter().rev() {
        if let Some(Value::Object(own)) = object.get("colors") {
            found_colors = true;
            colors.extend(own.clone());
        }
        if let Some(kind) = object.get("type").and_then(Value::as_str) {
            found_light = Some(is_light(kind));
        }
        if let Some(own) = object.get("name").and_then(Value::as_str) {
            found_name = Some(own.to_owned());
        }
    }
    if !found_colors {
        return Err(Error::new(
            "This file has no \"colors\" map, so it is not a VS Code colour theme",
        ));
    }
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| {
            stem.trim_end_matches("-color-theme")
                .replace(['-', '_'], " ")
        })
        .unwrap_or_else(|| "Imported".into());
    let name = name.map(str::to_owned).or(found_name).unwrap_or(stem);
    Ok(build(&colors, found_light.or(light), &name))
}

fn is_light(kind: &str) -> bool {
    matches!(kind, "light" | "vs" | "hc-light")
}

/// The app's theme file for a VS Code `colors` map. A theme that does not
/// say whether it is light is judged by its editor background.
fn build(colors: &Map<String, Value>, light: Option<bool>, name: &str) -> (String, String) {
    let light = light.unwrap_or_else(|| {
        colors
            .get("editor.background")
            .and_then(Value::as_str)
            .and_then(colour)
            .is_some_and(|background| luminance(background) > 0.5)
    });
    let name = name.trim();
    let name = if name.is_empty() { "Imported" } else { name };
    let mut design = design::builtin(light).clone();
    design.name = name.to_owned();
    map(&mut design, colors);
    (name.to_owned(), super::full_json(&design, name))
}

/// Read one of VS Code's files, bounded, without waiting on a pipe.
fn read(path: &Path) -> Result<String, Error> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let fail = |error: std::io::Error| Error::new(format!("{}: {error}", path.display()));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(fail)?;
    if !file.metadata().map_err(fail)?.is_file() {
        return Err(Error::new(format!("{} is not a file", path.display())));
    }
    let mut text = String::new();
    file.take(MAX_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(fail)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(Error::new(format!(
            "{} is larger than 1 MiB",
            path.display()
        )));
    }
    Ok(text)
}

/// The colour theme chosen in VS Code on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    /// The theme's name as VS Code's settings give it.
    pub name: String,
    /// The theme's file inside the extension that provides it.
    pub path: PathBuf,
    /// Whether the extension lists it as a light theme.
    pub light: Option<bool>,
}

/// Find the colour theme chosen in VS Code: `workbench.colorTheme` in its
/// user settings, looked up among the installed and built-in extensions.
pub fn chosen() -> Option<Chosen> {
    let home = PathBuf::from(std::env::var_os("HOME").filter(|home| !home.is_empty())?);
    let (settings, builtin) = if cfg!(target_os = "macos") {
        (
            home.join("Library/Application Support/Code/User/settings.json"),
            vec![
                PathBuf::from(
                    "/Applications/Visual Studio Code.app/Contents/Resources/app/extensions",
                ),
                home.join("Applications/Visual Studio Code.app/Contents/Resources/app/extensions"),
            ],
        )
    } else {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|config| !config.is_empty())
            .map_or_else(|| home.join(".config"), PathBuf::from);
        (
            config.join("Code/User/settings.json"),
            vec![
                PathBuf::from("/usr/share/code/resources/app/extensions"),
                PathBuf::from("/usr/lib/code/extensions"),
            ],
        )
    };
    let mut folders = vec![home.join(".vscode/extensions")];
    folders.extend(builtin);
    chosen_in(&settings, &folders)
}

fn chosen_in(settings: &Path, folders: &[PathBuf]) -> Option<Chosen> {
    let settings = jsonc::parse(&read(settings).ok()?).ok()?;
    let name = settings.get("workbench.colorTheme")?.as_str()?.to_owned();
    for folder in folders {
        let Ok(extensions) = std::fs::read_dir(folder) else {
            continue;
        };
        for extension in extensions.flatten() {
            let root = extension.path();
            let Some(manifest) = read(&root.join("package.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            else {
                continue;
            };
            let Some(themes) = manifest
                .pointer("/contributes/themes")
                .and_then(Value::as_array)
            else {
                continue;
            };
            // Built-in themes label themselves through a translation file.
            let mut labels: Option<Value> = None;
            for theme in themes {
                let text = |key: &str| theme.get(key).and_then(Value::as_str);
                let label = text("label").map(|label| {
                    match label
                        .strip_prefix('%')
                        .and_then(|key| key.strip_suffix('%'))
                    {
                        Some(key) => labels
                            .get_or_insert_with(|| {
                                read(&root.join("package.nls.json"))
                                    .ok()
                                    .and_then(|text| serde_json::from_str(&text).ok())
                                    .unwrap_or(Value::Null)
                            })
                            .get(key)
                            .and_then(Value::as_str)
                            .unwrap_or(label)
                            .to_owned(),
                        None => label.to_owned(),
                    }
                });
                if text("id") != Some(name.as_str()) && label.as_deref() != Some(name.as_str()) {
                    continue;
                }
                let Some(path) = text("path") else {
                    continue;
                };
                return Some(Chosen {
                    name,
                    path: root.join(path),
                    light: text("uiTheme").map(is_light),
                });
            }
        }
    }
    None
}

/// A VS Code colour: `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
fn colour(text: &str) -> Option<Rgba> {
    let digits = text.trim().strip_prefix('#')?;
    let long: String = match digits.len() {
        3 | 4 => digits.chars().flat_map(|ch| [ch, ch]).collect(),
        6 | 8 => digits.to_owned(),
        _ => return None,
    };
    design::hex(&format!("#{long}"))
}

fn alpha(colour: Rgba, a: f32) -> Rgba {
    Rgba { a, ..colour }
}

/// `top` laid over `under`, as it is seen.
fn over(top: Rgba, under: Rgba) -> Rgba {
    let mix = |t: f32, u: f32| t * top.a + u * (1.0 - top.a);
    Rgba {
        r: mix(top.r, under.r),
        g: mix(top.g, under.g),
        b: mix(top.b, under.b),
        a: 1.0,
    }
}

/// `t` of the way from `from` to `to`.
fn mix(from: Rgba, to: Rgba, t: f32) -> Rgba {
    let at = |a: f32, b: f32| a + (b - a) * t;
    Rgba {
        r: at(from.r, to.r),
        g: at(from.g, to.g),
        b: at(from.b, to.b),
        a: 1.0,
    }
}

fn luminance(colour: Rgba) -> f32 {
    let channel = |c: f32| {
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(colour.r) + 0.7152 * channel(colour.g) + 0.0722 * channel(colour.b)
}

/// The WCAG contrast ratio of two opaque colours.
fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Whichever of `choices` reads best on `fill`.
fn on(fill: Rgba, choices: &[Rgba]) -> Rgba {
    choices
        .iter()
        .copied()
        .max_by(|a, b| contrast(*a, fill).total_cmp(&contrast(*b, fill)))
        .unwrap_or(fill)
}

/// Read the theme's colours onto `design`.
fn map(design: &mut Design, colors: &Map<String, Value>) {
    let get = |key: &str| colors.get(key).and_then(Value::as_str).and_then(colour);
    let first = |keys: &[&str]| keys.iter().find_map(|key| get(key));
    // A border or fill only counts when it can be seen.
    let solid = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| get(key).filter(|colour| colour.a >= 0.5))
    };
    let base = design.colors.clone();
    let c = &mut design.colors;
    let surface = first(&["editor.background"]).map(|colour| over(colour, c.surface));
    if let Some(surface) = surface {
        c.surface = surface;
    }
    let surface = c.surface;
    c.panel = first(&["sideBar.background", "editor.background"])
        .map_or(c.panel, |colour| over(colour, surface));
    c.ground = first(&[
        "titleBar.activeBackground",
        "activityBar.background",
        "sideBar.background",
        "editor.background",
    ])
    .map_or(c.ground, |colour| over(colour, surface));
    if let Some(text) = first(&["foreground", "editor.foreground"]) {
        c.text = over(text, surface);
    }
    let text = c.text;
    // Colours made from the surface and text only when the theme sets them.
    let restyled = c.surface != base.surface || c.text != base.text;
    let made = |current: Rgba, made: Rgba| if restyled { made } else { current };
    c.text_2 = first(&["sideBar.foreground"])
        .map_or(made(c.text_2, mix(text, surface, 0.15)), |colour| {
            over(colour, surface)
        });
    c.muted = first(&["descriptionForeground"])
        .map(|colour| over(colour, surface))
        .filter(|colour| contrast(*colour, surface) >= 3.0)
        .unwrap_or(made(c.muted, mix(text, surface, 0.4)));
    c.faint = first(&["editorLineNumber.foreground", "disabledForeground"])
        .map_or(made(c.faint, mix(text, surface, 0.6)), |colour| {
            over(colour, surface)
        });
    c.field = first(&[
        "input.background",
        "dropdown.background",
        "editorWidget.background",
    ])
    .map_or(made(c.field, mix(surface, text, 0.04)), |colour| {
        over(colour, surface)
    });
    c.hover = first(&["list.hoverBackground"])
        .map_or(made(c.hover, mix(c.panel, text, 0.05)), |colour| {
            over(colour, c.panel)
        });
    // Some themes fill the active row with the accent; the app keeps its
    // text colour on a selected row, so the quieter selection comes first.
    c.selected = [
        "list.inactiveSelectionBackground",
        "list.activeSelectionBackground",
    ]
    .iter()
    .find_map(|key| get(key))
    .map(|colour| over(colour, c.panel))
    .filter(|colour| contrast(text, *colour) >= 4.5)
    .unwrap_or(made(c.selected, mix(c.panel, text, 0.09)));
    c.line = solid(&["sideBar.border", "panel.border", "editorGroup.border"])
        .map(|colour| over(colour, surface))
        .unwrap_or(made(c.line, mix(surface, text, 0.09)));
    c.line_strong = solid(&["input.border", "contrastBorder"])
        .map(|colour| over(colour, surface))
        .filter(|colour| contrast(*colour, surface) > 1.1)
        .unwrap_or(made(c.line_strong, mix(surface, text, 0.16)));
    c.edge = solid(&["editorGroup.border", "panel.border"])
        .map(|colour| over(colour, c.ground))
        .unwrap_or(made(c.edge, mix(surface, text, 0.11)));
    // The accent is the first of the theme's highlights that stands out.
    let accent = [
        "activityBarBadge.background",
        "progressBar.background",
        "button.background",
        "focusBorder",
        "badge.background",
        "textLink.foreground",
    ]
    .iter()
    .filter_map(|key| get(key).filter(|colour| colour.a >= 0.5))
    .map(|colour| over(colour, surface))
    .find(|colour| contrast(*colour, surface) >= 2.0);
    if let Some(accent) = accent {
        c.accent = accent;
        c.accent_text = first(&["textLink.foreground"])
            .map(|colour| over(colour, surface))
            .filter(|colour| contrast(*colour, surface) >= 3.0)
            .unwrap_or(accent);
        c.on_accent = on(accent, &[surface, text]);
        c.accent_soft = alpha(accent, c.accent_soft.a);
        c.focus = alpha(accent, c.focus.a);
        c.focus_halo = alpha(accent, c.focus_halo.a);
    }
    let attention = first(&[
        "errorForeground",
        "editorError.foreground",
        "terminal.ansiRed",
    ])
    .map(|colour| over(colour, surface))
    .filter(|colour| contrast(*colour, surface) >= 3.0);
    if let Some(attention) = attention {
        c.attention = attention;
        c.attention_text = attention;
        c.on_attention = on(attention, &[surface, text]);
        c.attention_soft = alpha(attention, c.attention_soft.a);
    }
    let readable = |keys: &[&str], slot: &mut Rgba| {
        if let Some(colour) = first(keys)
            .map(|colour| over(colour, surface))
            .filter(|colour| contrast(*colour, surface) >= 3.0)
        {
            *slot = colour;
        }
    };
    readable(
        &[
            "gitDecoration.addedResourceForeground",
            "terminal.ansiGreen",
        ],
        &mut c.ok,
    );
    readable(&["editorInfo.foreground", "terminal.ansiBlue"], &mut c.info);
    readable(&["terminal.ansiMagenta"], &mut c.violet);
    readable(&["terminal.ansiGreen"], &mut c.green);
    readable(
        &["textPreformat.foreground", "terminal.ansiYellow"],
        &mut c.code,
    );
    if let Some(shadow) = first(&["widget.shadow"]).filter(|colour| colour.a > 0.0) {
        c.shadow = shadow;
    }

    let t = &mut design.terminal;
    let background = first(&["terminal.background", "editor.background"])
        .map_or(t.background, |colour| over(colour, surface));
    t.background = background;
    t.text = first(&["terminal.foreground", "editor.foreground", "foreground"])
        .map_or(t.text, |colour| over(colour, background));
    t.cursor = first(&["terminalCursor.foreground", "editorCursor.foreground"])
        .map_or(design.colors.accent, |colour| over(colour, background));
    if let Some(selection) = first(&["terminal.selectionBackground", "editor.selectionBackground"])
    {
        t.selection = over(selection, background);
        t.selection_text = first(&["terminal.selectionForeground"])
            .map_or(t.text, |colour| over(colour, t.selection));
    }
    const ANSI: [&str; 16] = [
        "terminal.ansiBlack",
        "terminal.ansiRed",
        "terminal.ansiGreen",
        "terminal.ansiYellow",
        "terminal.ansiBlue",
        "terminal.ansiMagenta",
        "terminal.ansiCyan",
        "terminal.ansiWhite",
        "terminal.ansiBrightBlack",
        "terminal.ansiBrightRed",
        "terminal.ansiBrightGreen",
        "terminal.ansiBrightYellow",
        "terminal.ansiBrightBlue",
        "terminal.ansiBrightMagenta",
        "terminal.ansiBrightCyan",
        "terminal.ansiBrightWhite",
    ];
    for (slot, key) in t.palette.iter_mut().zip(ANSI) {
        if let Some(colour) = get(key) {
            *slot = over(colour, background);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vs_code_theme_maps_onto_the_app_and_reads_back() {
        let text = r##"{
            // VS Code writes comments and trailing commas.
            "name": "Sample Night",
            "type": "dark",
            "colors": {
                "editor.background": "#1a1b26",
                "editor.foreground": "#a9b1d6",
                "sideBar.background": "#16161e",
                "list.activeSelectionBackground": "#202330",
                "focusBorder": "#545c7e33",
                "button.background": "#3d59a1dd",
                "textLink.foreground": "#6183bb",
                "terminal.ansiRed": "#f7768e",
                "terminal.ansiBrightWhite": "#acb",
            },
            "tokenColors": [],
        }"##;
        let (name, json) = import(text, "file").unwrap();
        assert_eq!(name, "Sample Night");
        let design = super::super::resolve_text("sample", &json).unwrap();
        assert!(!design.light);
        assert_eq!(design.colors.surface, design::hex("#1a1b26").unwrap());
        assert_eq!(design.colors.panel, design::hex("#16161e").unwrap());
        assert_eq!(design.terminal.background, design::hex("#1a1b26").unwrap());
        assert_eq!(design.terminal.palette[1], design::hex("#f7768e").unwrap());
        assert_eq!(design.terminal.palette[15], design::hex("#aaccbb").unwrap());
        // A faint focus border is passed over for a visible accent.
        assert!(contrast(design.colors.accent, design.colors.surface) >= 2.0);
        assert_eq!(design.colors.accent_text, design::hex("#6183bb").unwrap());
        // Unset colours keep the built-in dark theme's.
        assert_eq!(
            design.colors.mark_eye,
            design::builtin(false).colors.mark_eye
        );
    }

    #[test]
    fn light_themes_and_files_that_are_not_themes() {
        let (name, json) = import(r#"{"type": "light", "colors": {}}"#, "From file").unwrap();
        assert_eq!(name, "From file");
        let design = super::super::resolve_text("x", &json).unwrap();
        assert_eq!(design.colors, design::builtin(true).colors);
        assert!(import(r#"{"name": "x"}"#, "x").is_err());
        assert!(import("[1]", "x").is_err());
        assert!(import("{", "x").unwrap_err().line.is_some());
        assert_eq!(colour("#abc"), design::hex("#aabbcc"));
        assert_eq!(colour("#abcd"), design::hex("#aabbccdd"));
        assert!(colour("abc").is_none());
    }
}

#[cfg(test)]
mod files {
    use super::*;

    #[test]
    fn the_theme_chosen_in_vs_code_is_found_and_read_with_its_parents() {
        let root = tempfile::tempdir().unwrap();
        let write = |path: &str, text: &str| {
            let path = root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        };
        write(
            "User/settings.json",
            "{\n  // chosen\n  \"workbench.colorTheme\": \"Paper Day\",\n}",
        );
        write(
            "ext/other/package.json",
            r#"{"contributes": {"themes": [{"label": "Else", "path": "./x.json"}]}}"#,
        );
        write(
            "ext/paper/package.json",
            r#"{"contributes": {"themes": [{"label": "%day%", "uiTheme": "vs", "path": "./themes/day.json"}]}}"#,
        );
        write("ext/paper/package.nls.json", r#"{"day": "Paper Day"}"#);
        write(
            "ext/paper/themes/day.json",
            r##"{"include": "./base.json", "colors": {"editor.background": "#fefefe"}}"##,
        );
        write(
            "ext/paper/themes/base.json",
            r##"{"colors": {"editor.background": "#000000", "editor.foreground": "#222222"}}"##,
        );
        let chosen = chosen_in(
            &root.path().join("User/settings.json"),
            &[root.path().join("ext")],
        )
        .unwrap();
        assert_eq!(chosen.name, "Paper Day");
        assert_eq!(chosen.light, Some(true));
        let (name, json) = import_file(&chosen.path, chosen.light, Some(&chosen.name)).unwrap();
        assert_eq!(name, "Paper Day");
        let design = super::super::resolve_text("x", &json).unwrap();
        assert!(design.light);
        // The theme's own colour wins over its parent's, which fills the rest.
        assert_eq!(design.colors.surface, design::hex("#fefefe").unwrap());
        assert_eq!(design.terminal.text, design::hex("#222222").unwrap());

        write(
            "User/settings.json",
            r#"{"workbench.colorTheme": "Missing"}"#,
        );
        assert!(
            chosen_in(
                &root.path().join("User/settings.json"),
                &[root.path().join("ext")]
            )
            .is_none()
        );
        // Without a stated kind, the editor background decides.
        let (_, json) =
            import_file(&root.path().join("ext/paper/themes/day.json"), None, None).unwrap();
        assert!(super::super::resolve_text("x", &json).unwrap().light);
        assert!(import_file(&root.path().join("ext/paper/package.json"), None, None).is_err());
    }
}
