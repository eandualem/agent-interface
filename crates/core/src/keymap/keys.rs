const NAMED: &[&str] = &[
    "Space", "Enter", "Tab", "BSpace", "Escape", "Up", "Down", "Left", "Right", "Home", "End",
    "PageUp", "PageDown", "Insert", "Delete", "BTab", "F1", "F2", "F3", "F4", "F5", "F6", "F7",
    "F8", "F9", "F10", "F11", "F12",
];

pub(super) fn prefix(value: &str) -> Result<String, String> {
    if value.is_empty() || value.chars().count() > 32 {
        return Err("expected a key such as t, C-t, M-Left or F2".into());
    }
    let (mut control, mut alt) = (false, false);
    let mut remaining = value;
    while remaining.starts_with("C-") || remaining.starts_with("M-") {
        let modifier = if remaining.starts_with("C-") {
            &mut control
        } else {
            &mut alt
        };
        if *modifier {
            return Err(format!("duplicate modifier in key {value:?}"));
        }
        *modifier = true;
        remaining = &remaining[2..];
    }
    let lowercase = remaining.to_lowercase();
    let alias = match lowercase.as_str() {
        "return" => Some("Enter"),
        "esc" => Some("Escape"),
        "backspace" => Some("BSpace"),
        _ => None,
    };
    let mut key = if let Some(named) = alias.or_else(|| {
        NAMED
            .iter()
            .copied()
            .find(|k| k.to_ascii_lowercase() == lowercase)
    }) {
        named.to_owned()
    } else if remaining.len() == 1
        && (b'!'..=b'~').contains(&remaining.as_bytes()[0])
        && !matches!(remaining, ";" | "\\")
    {
        remaining.to_owned()
    } else {
        return Err(format!("unsupported key {value:?}"));
    };
    if control {
        if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() {
            key.make_ascii_lowercase();
        }
        let alias = match key.as_str() {
            "i" => Some("Tab"),
            "m" => Some("Enter"),
            "[" => Some("Escape"),
            "?" => Some("BSpace"),
            "@" | "Space" => Some("C-Space"),
            _ => None,
        };
        if let Some(alias) = alias {
            key = alias.into();
            control = false;
        } else if key.len() == 1
            && !(key.as_bytes()[0].is_ascii_lowercase() || matches!(key.as_str(), "]" | "^" | "_"))
        {
            return Err(format!("unsupported control key {value:?}"));
        }
    }
    Ok(format!(
        "{}{}{key}",
        if alt { "M-" } else { "" },
        if control { "C-" } else { "" }
    ))
}

const MODIFIERS: &[&str] = &["super", "ctrl", "alt", "shift"];
const DIRECT_NAMES: &[&str] = &[
    "apostrophe",
    "backslash",
    "bracket_left",
    "bracket_right",
    "comma",
    "equal",
    "grave_accent",
    "minus",
    "period",
    "semicolon",
    "slash",
    "space",
    "enter",
    "tab",
    "backspace",
    "escape",
    "insert",
    "delete",
    "right",
    "left",
    "down",
    "up",
    "page_up",
    "page_down",
    "home",
    "end",
];

pub(super) fn direct(value: &str) -> Result<String, String> {
    let mut parts: Vec<_> = value.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let named = DIRECT_NAMES.contains(&key) || (1..=25).any(|n| key == format!("f{n}"));
    let ascii = key.len() == 1
        && (key.as_bytes()[0].is_ascii_lowercase() || key.as_bytes()[0].is_ascii_digit());
    if value.chars().count() > 80
        || !(named || ascii)
        || parts.is_empty()
        || parts.iter().any(|m| !MODIFIERS.contains(m))
        || parts
            .iter()
            .enumerate()
            .any(|(i, m)| parts[..i].contains(m))
        || !parts.iter().any(|m| matches!(*m, "super" | "ctrl" | "alt"))
    {
        return Err(format!(
            "unsupported direct trigger {value:?}; use super, ctrl or alt with a key"
        ));
    }
    let mut canonical: Vec<_> = MODIFIERS
        .iter()
        .copied()
        .filter(|m| parts.contains(m))
        .collect();
    canonical.push(key);
    Ok(canonical.join("+"))
}

fn punctuation(key: &str) -> Option<&'static str> {
    Some(match key {
        "apostrophe" => "'",
        "backslash" => "\\",
        "bracket_left" => "[",
        "bracket_right" => "]",
        "comma" => ",",
        "equal" => "=",
        "grave_accent" => "`",
        "minus" => "-",
        "period" => ".",
        "semicolon" => ";",
        "slash" => "/",
        _ => return None,
    })
}

// This is the Python keymap's cross-layer collision rule, not an OS shortcut probe.
pub(super) fn physical(trigger: &str) -> Option<String> {
    let mut parts: Vec<_> = trigger.split('+').collect();
    let original = parts.pop()?;
    if parts.contains(&"super") {
        return None;
    }
    let mut key = punctuation(original)
        .unwrap_or(match original {
            "backspace" => "BSpace",
            "page_up" => "PageUp",
            "page_down" => "PageDown",
            _ => original,
        })
        .to_owned();
    if parts.contains(&"shift") {
        if key.len() == 1 {
            let ch = key.as_bytes()[0].to_ascii_uppercase();
            let unshifted = b"1234567890-=[]\\;',./`";
            let shifted = b"!@#$%^&*()_+{}|:\"<>?~";
            key = char::from(
                unshifted
                    .iter()
                    .position(|&k| k == ch)
                    .map_or(ch, |i| shifted[i]),
            )
            .to_string();
        } else if key == "tab" {
            key = "BTab".into();
        } else if key != "space" {
            return None;
        }
    }
    prefix(&format!(
        "{}{}{key}",
        if parts.contains(&"alt") { "M-" } else { "" },
        if parts.contains(&"ctrl") { "C-" } else { "" }
    ))
    .ok()
}

pub(super) fn prefix_label(key: &str) -> String {
    key.replace("M-", "Alt-").replace("C-", "Ctrl-")
}

pub(super) fn direct_label(trigger: &str) -> String {
    let mut parts: Vec<_> = trigger.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let mut label: String = parts
        .iter()
        .map(|m| match *m {
            "super" => "⌘",
            "ctrl" => "⌃",
            "alt" => "⌥",
            "shift" => "⇧",
            _ => "",
        })
        .collect();
    let named = punctuation(key).or(match key {
        "left" => Some("←"),
        "right" => Some("→"),
        "up" => Some("↑"),
        "down" => Some("↓"),
        "page_up" => Some("PageUp"),
        "page_down" => Some("PageDown"),
        _ => None,
    });
    if let Some(key) = named {
        label.push_str(key);
    } else if key.len() == 1 || key.starts_with('f') {
        label.push_str(&key.to_ascii_uppercase());
    } else {
        label.push_str(&key[..1].to_ascii_uppercase());
        label.push_str(&key[1..]);
    }
    label
}
