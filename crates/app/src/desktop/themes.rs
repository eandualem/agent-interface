//! Themes: the built-in Dark and Light, the themes bundled with the app and
//! the person's own theme files.
//!
//! A theme is JSON that names another theme it `extends` and changes any of
//! its tokens, from a single colour to fonts, type sizes and spacing. Its own
//! files live in `agent-interface/themes/` beside the settings file; the
//! bundled themes are read-only and can be copied there to change them.

pub mod jsonc;
pub mod vscode;

use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use toml::{Table, Value};

use super::design::{self, Design, GROUPS};
pub use jsonc::Error;

/// Where a theme comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Dark or Light, from the design tokens.
    BuiltIn,
    /// Shipped with the app as JSON.
    Bundled,
    /// A file in the themes folder, which the person may edit.
    User,
}

/// A theme Settings can offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub origin: Origin,
}

/// The themes shipped with the app, by id. Each one's source and licence is
/// listed in `docs/ARCHITECTURE.md`.
const BUNDLED: &[(&str, &str)] = &[
    ("dracula", include_str!("../../assets/themes/dracula.json")),
    (
        "one-dark",
        include_str!("../../assets/themes/one-dark.json"),
    ),
    (
        "tokyo-night",
        include_str!("../../assets/themes/tokyo-night.json"),
    ),
    (
        "catppuccin-mocha",
        include_str!("../../assets/themes/catppuccin-mocha.json"),
    ),
    ("nord", include_str!("../../assets/themes/nord.json")),
    (
        "gruvbox-dark",
        include_str!("../../assets/themes/gruvbox-dark.json"),
    ),
    (
        "solarized-dark",
        include_str!("../../assets/themes/solarized-dark.json"),
    ),
    (
        "github-dark",
        include_str!("../../assets/themes/github-dark.json"),
    ),
    (
        "catppuccin-latte",
        include_str!("../../assets/themes/catppuccin-latte.json"),
    ),
    (
        "solarized-light",
        include_str!("../../assets/themes/solarized-light.json"),
    ),
    (
        "github-light",
        include_str!("../../assets/themes/github-light.json"),
    ),
    (
        "one-light",
        include_str!("../../assets/themes/one-light.json"),
    ),
];

/// The largest theme file read.
pub const MAX_THEME_BYTES: u64 = 256 * 1024;
/// How many themes a theme may extend in a chain.
const MAX_CHAIN: usize = 8;

/// The theme in use and why it could not be used, if it could not.
static CURRENT: Mutex<(String, Option<String>)> = Mutex::new((String::new(), None));

/// The folder of the person's own themes, beside the settings file.
pub fn dir() -> Option<PathBuf> {
    Some(super::settings::path()?.parent()?.join("themes"))
}

/// Whether `id` can name a theme file: lowercase letters, digits and hyphens.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !id.starts_with('-')
}

fn bundled(id: &str) -> Option<&'static str> {
    BUNDLED
        .iter()
        .find(|(bundled, _)| *bundled == id)
        .map(|(_, text)| *text)
}

fn reserved(id: &str) -> bool {
    matches!(id, "dark" | "light") || bundled(id).is_some()
}

/// Where the person's theme `id` is saved.
pub fn file(id: &str) -> Option<PathBuf> {
    (valid_id(id) && !reserved(id)).then(|| dir().map(|dir| dir.join(format!("{id}.json"))))?
}

/// Every theme: Dark and Light, the bundled ones, then the person's own by name.
pub fn list() -> Vec<Entry> {
    let mut entries = vec![
        Entry {
            id: "dark".into(),
            name: "Dark".into(),
            origin: Origin::BuiltIn,
        },
        Entry {
            id: "light".into(),
            name: "Light".into(),
            origin: Origin::BuiltIn,
        },
    ];
    for (id, text) in BUNDLED {
        entries.push(Entry {
            id: (*id).into(),
            name: name_of(text).unwrap_or_else(|| (*id).into()),
            origin: Origin::Bundled,
        });
    }
    let mut own = Vec::new();
    if let Some(dir) = dir()
        && let Ok(files) = std::fs::read_dir(dir)
    {
        for file in files.flatten() {
            let path = file.path();
            let Some(id) = path
                .extension()
                .filter(|extension| *extension == "json")
                .and_then(|_| path.file_stem()?.to_str())
                .filter(|id| valid_id(id) && !reserved(id))
            else {
                continue;
            };
            let name = read(&path)
                .ok()
                .and_then(|text| name_of(&text))
                .unwrap_or_else(|| id.to_owned());
            own.push(Entry {
                id: id.to_owned(),
                name,
                origin: Origin::User,
            });
        }
    }
    own.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    entries.extend(own);
    entries
}

fn name_of(text: &str) -> Option<String> {
    jsonc::parse(text)
        .ok()?
        .get("name")?
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
}

/// Read a theme file, bounded, refusing anything but a regular file.
fn read(path: &Path) -> io::Result<String> {
    use std::os::unix::fs::OpenOptionsExt;
    // Check the opened handle, as the settings reader does, so a pipe named
    // like a theme cannot leave Settings waiting for a writer.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not a regular file"));
    }
    let mut text = String::new();
    file.take(MAX_THEME_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_THEME_BYTES {
        return Err(io::Error::other("the theme is larger than 256 KiB"));
    }
    Ok(text)
}

/// The JSON of theme `id`, or `None` for Dark and Light, which have none.
pub fn source(id: &str) -> Result<Option<String>, Error> {
    if matches!(id, "dark" | "light") {
        return Ok(None);
    }
    if let Some(text) = bundled(id) {
        return Ok(Some(text.to_owned()));
    }
    let path = file(id).ok_or_else(|| Error::new(format!("There is no theme \"{id}\"")))?;
    match read(&path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(Error::new(format!("There is no theme \"{id}\"")))
        }
        Err(error) => Err(Error::new(format!("{}: {error}", path.display()))),
    }
}

/// Theme `id`, resolved through the themes it extends.
pub fn resolve(id: &str) -> Result<Design, Error> {
    let (table, light, name) = table(id, &mut Vec::new())?;
    Design::from_table(&table, &name, light).map_err(|error| Error::new(error.to_string()))
}

/// The theme `text` would be if it were saved as `id`; errors in `text`
/// carry their line and column.
pub fn resolve_text(id: &str, text: &str) -> Result<Design, Error> {
    if text.len() as u64 > MAX_THEME_BYTES {
        return Err(Error::new("A theme must be 256 KiB or smaller"));
    }
    let (table, light, name) = overlay(text, id, &mut vec![id.to_owned()])?;
    Design::from_table(&table, &name, light).map_err(|error| {
        let path: Vec<&str> = error.path.split('.').collect();
        let place = jsonc::locate(text, &path);
        Error {
            message: error.to_string(),
            line: place.map(|(line, _)| line),
            column: place.map(|(_, column)| column),
        }
    })
}

fn table(id: &str, chain: &mut Vec<String>) -> Result<(Table, bool, String), Error> {
    match id {
        "dark" => return Ok((design::base(false).clone(), false, "Dark".into())),
        "light" => return Ok((design::base(true).clone(), true, "Light".into())),
        _ => {}
    }
    if chain.iter().any(|seen| seen == id) {
        return Err(Error::new(format!(
            "The themes extend each other in a circle: {} → {id}",
            chain.join(" → ")
        )));
    }
    if chain.len() >= MAX_CHAIN {
        return Err(Error::new(format!(
            "More than {MAX_CHAIN} themes extend each other"
        )));
    }
    chain.push(id.to_owned());
    let text = source(id)?.unwrap_or_default();
    overlay(&text, id, chain)
}

/// Read theme JSON over the theme it extends.
fn overlay(text: &str, id: &str, chain: &mut Vec<String>) -> Result<(Table, bool, String), Error> {
    let at = |path: &[&str], message: String| {
        let place = jsonc::locate(text, path);
        Error {
            message,
            line: place.map(|(line, _)| line),
            column: place.map(|(_, column)| column),
        }
    };
    let value = jsonc::parse(text)?;
    let object = value
        .as_object()
        .ok_or_else(|| Error::new("A theme is a JSON object, { … }"))?;
    let extends = match object.get("extends") {
        None => "dark",
        Some(serde_json::Value::String(extends)) => extends.as_str(),
        Some(_) => return Err(at(&["extends"], "extends must be a theme's id".into())),
    };
    if !matches!(extends, "dark" | "light") && !valid_id(extends) {
        return Err(at(
            &["extends"],
            format!("extends: \"{extends}\" is not a theme id"),
        ));
    }
    let (mut table, mut light, base_name) = table(extends, chain).map_err(|error| {
        if error.line.is_some() {
            Error::new(format!("In theme \"{extends}\": {error}"))
        } else {
            error
        }
    })?;
    let mut name = None;
    for (key, value) in object {
        match key.as_str() {
            "extends" | "$schema" => {}
            "name" => {
                name = Some(
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| at(&["name"], "name must be text".into()))?
                        .to_owned(),
                )
            }
            "type" => {
                light = match value.as_str() {
                    Some("light") => true,
                    Some("dark") => false,
                    _ => return Err(at(&["type"], "type must be \"dark\" or \"light\"".into())),
                }
            }
            group if GROUPS.contains(&group) => {
                let mut path = vec![group.to_owned()];
                merge(&mut table, key, value, &mut path).map_err(|(path, message)| {
                    let path: Vec<&str> = path.iter().map(String::as_str).collect();
                    at(&path, format!("{} {message}", path.join(".")))
                })?;
            }
            other => {
                return Err(at(
                    &[other],
                    format!(
                        "\"{other}\" is not a theme setting; a theme has name, extends, type and {}",
                        GROUPS.join(", ")
                    ),
                ));
            }
        }
    }
    let name = name.unwrap_or_else(|| {
        if id.is_empty() {
            base_name
        } else {
            id.to_owned()
        }
    });
    Ok((table, light, name))
}

type MergeError = (Vec<String>, String);

/// Put `value` at `key` in `table`, which must already hold that token.
fn merge(
    table: &mut Table,
    key: &str,
    value: &serde_json::Value,
    path: &mut Vec<String>,
) -> Result<(), MergeError> {
    let fail = |path: &[String], message: &str| Err((path.to_vec(), message.to_owned()));
    if path.as_slice() == ["font", "terminal_size"] {
        return fail(
            path,
            "is not a theme setting: set the terminal text size in Settings › Appearance",
        );
    }
    if path.as_slice() == ["font", "mono_linux"] {
        return fail(
            path,
            "is not a theme setting: font.mono applies on every system",
        );
    }
    let Some(existing) = table.get_mut(key) else {
        return fail(path, "is not a theme setting");
    };
    match (existing, value) {
        (Value::Table(inner), serde_json::Value::Object(members)) => {
            for (key, value) in members {
                path.push(key.clone());
                merge(inner, key, value, path)?;
                path.pop();
            }
            Ok(())
        }
        (Value::Table(_), _) => fail(path, "must be an object, { … }"),
        (existing, value) => {
            *existing = toml_value(value).map_err(|message| (path.clone(), message))?;
            Ok(())
        }
    }
}

fn toml_value(value: &serde_json::Value) -> Result<Value, String> {
    Ok(match value {
        serde_json::Value::String(text) => Value::String(text.clone()),
        serde_json::Value::Number(number) => {
            let number = number
                .as_f64()
                .filter(|number| number.is_finite() && *number >= 0.0)
                .ok_or("must be a number of zero or more")?;
            Value::Float(number)
        }
        serde_json::Value::Array(items) => {
            Value::Array(items.iter().map(toml_value).collect::<Result<_, _>>()?)
        }
        serde_json::Value::Bool(_) => return Err("must not be true or false".into()),
        serde_json::Value::Null => return Err("must not be null".into()),
        serde_json::Value::Object(_) => return Err("must not be an object".into()),
    })
}

/// A theme file for `design` named `name`, over the built-in theme of its
/// kind: every colour, and the other tokens where they differ from it.
pub fn full_json(design: &Design, name: &str) -> String {
    format!(
        "{{\n  \"name\": {},\n  \"extends\": \"{}\",\n{}\n}}\n",
        serde_json::Value::from(name),
        if design.light { "light" } else { "dark" },
        design.theme_json(design::builtin(design.light))
    )
}

/// A new theme id for `name` that no theme uses yet.
pub fn new_id(name: &str) -> String {
    let mut stem: String = name
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    while stem.contains("--") {
        stem = stem.replace("--", "-");
    }
    let stem = stem.trim_matches('-');
    let stem = if stem.is_empty() { "theme" } else { stem };
    let stem: String = stem.chars().take(48).collect();
    let taken: Vec<String> = list().into_iter().map(|entry| entry.id).collect();
    let free = |id: &str| valid_id(id) && !reserved(id) && !taken.iter().any(|taken| taken == id);
    let stem = stem.trim_end_matches('-').to_owned();
    if free(&stem) {
        return stem;
    }
    (2..)
        .map(|n| format!("{stem}-{n}"))
        .find(|id| free(id))
        .unwrap_or_else(|| "theme".into())
}

/// Save `text` as the person's theme `id`, replacing the file whole.
pub fn save(id: &str, text: &str) -> io::Result<()> {
    if text.len() as u64 > MAX_THEME_BYTES {
        return Err(io::Error::other("a theme must be 256 KiB or smaller"));
    }
    let path = file(id).ok_or_else(|| io::Error::other(format!("\"{id}\" cannot be saved")))?;
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("no themes folder"))?;
    std::fs::DirBuilder::new().recursive(true).create(dir)?;
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    temporary.write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(&path).map_err(|error| error.error)?;
    Ok(())
}

/// Copy theme `id` as a new theme of the person's own, and return its id.
pub fn customize(id: &str) -> Result<String, Error> {
    let design = resolve(id)?;
    let name = format!("{} (custom)", design.name);
    let new = new_id(&name);
    save(&new, &full_json(&design, &name))
        .map_err(|error| Error::new(format!("Not saved: {error}")))?;
    Ok(new)
}

/// Use theme `id` in every window. A theme that cannot be read leaves the
/// built-in Dark theme in use, and Settings shows why.
pub fn apply(id: &str) {
    let (design, problem) = match resolve(id) {
        Ok(design) => (design, None),
        Err(error) => (
            design::builtin(false).clone(),
            Some(format!("Theme \"{id}\" is not in use. {error}")),
        ),
    };
    design::set_theme(design);
    *CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = (id.to_owned(), problem);
}

/// The id of the theme chosen in Settings.
pub fn current() -> String {
    let current = CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if current.0.is_empty() {
        "dark".into()
    } else {
        current.0.clone()
    }
}

/// Why the chosen theme is not in use, if it is not.
pub fn problem() -> Option<String> {
    CURRENT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .1
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_theme_resolves_and_sets_every_colour() {
        for (id, text) in BUNDLED {
            let design = resolve(id).unwrap_or_else(|error| panic!("{id}: {error}"));
            assert!(name_of(text).is_some(), "{id} has a name");
            let object = jsonc::parse(text).unwrap();
            let colours = object["color"].as_object().unwrap();
            let base = if design.light {
                design::base(true)
            } else {
                design::base(false)
            };
            for key in base["color"].as_table().unwrap().keys() {
                assert!(colours.contains_key(key), "{id} sets color.{key}");
            }
            let terminal = object["terminal"].as_object().unwrap();
            for key in base["terminal"].as_table().unwrap().keys() {
                assert!(terminal.contains_key(key), "{id} sets terminal.{key}");
            }
        }
    }

    #[test]
    fn a_theme_changes_only_what_it_sets_and_reports_mistakes_where_they_are() {
        let design = resolve_text(
            "mine",
            "{\n  // Warmer\n  \"name\": \"Mine\",\n  \"extends\": \"light\",\n  \"color\": { \"accent\": \"#112233\", },\n  \"text\": { \"row\": { \"size\": 15 } },\n  \"font\": { \"mono\": \"Iosevka\" }\n}",
        )
        .unwrap();
        let light = design::builtin(true);
        assert_eq!(design.name, "Mine");
        assert!(design.light);
        assert_eq!(design.colors.accent, design::hex("#112233").unwrap());
        assert_eq!(design.colors.ground, light.colors.ground);
        assert_eq!(design.text.row.size, 15.0);
        assert_eq!(design.text.row.weight, light.text.row.weight);
        assert_eq!(design.font.mono, "Iosevka");

        let unknown = resolve_text(
            "mine",
            "{\n  \"color\": {\n    \"grond\": \"#000000\"\n  }\n}",
        )
        .unwrap_err();
        assert_eq!((unknown.line, unknown.column), (Some(3), Some(5)));
        assert!(
            unknown
                .message
                .contains("color.grond is not a theme setting")
        );

        let bad =
            resolve_text("mine", "{\n  \"color\": {\n    \"ground\": \"red\"\n  }\n}").unwrap_err();
        assert_eq!(bad.line, Some(3));
        assert!(bad.message.contains("must be a colour"), "{}", bad.message);

        let palette =
            resolve_text("mine", "{ \"terminal\": { \"palette\": [\"#000000\"] } }").unwrap_err();
        assert!(palette.message.contains("16 colours"));

        let syntax = resolve_text("mine", "{\n  \"color\": {\n}").unwrap_err();
        assert!(syntax.line.is_some());

        for refused in [
            "{ \"font\": { \"terminal_size\": 14 } }",
            "{ \"size\": { \"row\": -1 } }",
            "{ \"color\": \"#000000\" }",
            "{ \"extends\": \"Not An Id\" }",
            "{ \"type\": \"dim\" }",
            "{ \"colours\": {} }",
        ] {
            assert!(resolve_text("mine", refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn themes_extend_bundled_themes_and_cycles_are_refused() {
        let design = resolve_text(
            "mine",
            "{ \"extends\": \"nord\", \"color\": { \"accent\": \"#ffffff\" } }",
        )
        .unwrap();
        let nord = resolve("nord").unwrap();
        assert_eq!(design.colors.ground, nord.colors.ground);
        assert_eq!(design.colors.accent, design::hex("#ffffff").unwrap());
        assert_eq!(design.name, "mine");
        let cycle = resolve_text("mine", "{ \"extends\": \"mine\" }");
        assert!(cycle.is_err());
    }

    #[test]
    fn a_full_theme_file_reads_back_as_the_same_theme() {
        for id in ["dark", "light", "dracula", "github-light"] {
            let design = resolve(id).unwrap();
            let text = full_json(&design, &design.name);
            let again = resolve_text("copy", &text).unwrap();
            assert_eq!(again, design, "{id}");
        }
    }

    #[test]
    fn theme_files_are_bounded_and_pipes_are_refused_without_waiting() {
        let directory = tempfile::tempdir().unwrap();
        let pipe = directory.path().join("pipe.json");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&pipe)
                .status()
                .unwrap()
                .success()
        );
        assert!(read(&pipe).is_err());
        let large = directory.path().join("large.json");
        std::fs::write(&large, " ".repeat(MAX_THEME_BYTES as usize + 1)).unwrap();
        assert!(read(&large).is_err());
    }

    #[test]
    fn ids_are_file_names_that_never_shadow_shipped_themes() {
        assert!(valid_id("my-theme-2"));
        for id in ["", "-x", "My", "a/b", "a.json", &"x".repeat(65)] {
            assert!(!valid_id(id), "{id}");
        }
        assert!(file("nord").is_none() && file("dark").is_none());
        let id = new_id("Nord");
        assert_ne!(id, "nord");
        assert!(valid_id(&id));
        assert_eq!(new_id("  Café Ünïcode!! "), "caf-n-code");
    }
}
