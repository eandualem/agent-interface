use serde_json::{Map, Value};
use std::collections::HashSet;
use tmux_workspaces_core::{Attachment, Direction, Layout, Leaf, Node, Tab, Workspace};
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

type Result<T> = std::result::Result<T, String>;
type Object = Map<String, Value>;

fn object<'a>(value: &'a Value, at: &str) -> Result<&'a Object> {
    value
        .as_object()
        .ok_or_else(|| format!("{at}: expected an object"))
}

fn string(item: &Object, key: &str, at: &str) -> Result<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("{at}.{key}: expected a string"))
}

fn printable(value: &str) -> bool {
    value.chars().all(|ch| {
        ch == ' '
            || !matches!(
                ch.general_category_group(),
                GeneralCategoryGroup::Other | GeneralCategoryGroup::Separator
            )
    })
}

fn name(item: &Object, at: &str) -> Result<String> {
    let value = string(item, "name", at)?;
    if value.trim().is_empty() || !printable(&value) {
        return Err(format!("{at}.name: expected a nonempty printable name"));
    }
    Ok(value)
}

fn path(item: &Object, key: &str, at: &str) -> Result<Option<String>> {
    if !item.contains_key(key) {
        return Ok(None);
    }
    let value = string(item, key, at)?;
    if !value.starts_with('/') || value.contains('\0') {
        return Err(format!("{at}.{key}: expected an absolute path without NUL"));
    }
    Ok(Some(value))
}

fn identify(item: &Object, at: &str, identities: &mut HashSet<String>) -> Result<String> {
    let id = string(item, "id", at)?;
    if id.len() != 12
        || !id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(format!(
            "{at}.id: expected a 12-character lowercase hexadecimal identity"
        ));
    }
    if !identities.insert(id.clone()) {
        return Err(format!("{at}.id: duplicate identity"));
    }
    Ok(id)
}

fn node(
    value: &Value,
    at: &str,
    depth: usize,
    ids: &mut HashSet<String>,
    leaves: &mut Vec<String>,
) -> Result<Node> {
    if depth > 64 {
        return Err(format!("{at}: split nesting exceeds 64 levels"));
    }
    let item = object(value, at)?;
    let id = identify(item, at, ids)?;
    if let Some(agent) = item.get("agent") {
        if ["first", "second", "direction", "ratio"]
            .iter()
            .any(|key| item.contains_key(*key))
        {
            return Err(format!("{at}: leaf also contains split fields"));
        }
        let session = if agent.is_null() {
            None
        } else {
            let session = agent
                .as_str()
                .filter(|s| !s.is_empty() && printable(s) && !s.contains(':'))
                .ok_or_else(|| format!("{at}.agent: invalid session name"))?;
            Some(session.to_owned())
        };
        let empty = match item.get("empty") {
            None => false,
            Some(Value::Bool(true)) if session.is_none() => true,
            _ => {
                return Err(format!(
                    "{at}.empty: an empty pane is true and has no attachment"
                ));
            }
        };
        let socket = path(item, "source_socket", at)?;
        if socket.is_some() && session.is_none() {
            return Err(format!("{at}.source_socket: socket without an attachment"));
        }
        leaves.push(id.clone());
        Ok(Node::Leaf(Leaf {
            id,
            attachment: session.map(|session| Attachment {
                session,
                source_socket: socket,
            }),
            cwd: path(item, "cwd", at)?,
            empty,
        }))
    } else {
        let direction = match item.get("direction").and_then(Value::as_str) {
            Some("right") => Direction::Right,
            Some("below") => Direction::Below,
            _ => return Err(format!("{at}.direction: expected right or below")),
        };
        let ratio = match item.get("ratio") {
            None => 0.5,
            Some(value) => value
                .as_f64()
                .ok_or_else(|| format!("{at}.ratio: expected a number"))?,
        };
        if !ratio.is_finite() || !(0.01..=0.99).contains(&ratio) {
            return Err(format!(
                "{at}.ratio: expected a finite split ratio from 0.01 to 0.99"
            ));
        }
        if item.contains_key("cwd") || item.contains_key("source_socket") {
            return Err(format!("{at}: split also contains leaf fields"));
        }
        Ok(Node::Split {
            id,
            direction,
            ratio,
            first: Box::new(node(
                item.get("first").unwrap_or(&Value::Null),
                &format!("{at}.first"),
                depth + 1,
                ids,
                leaves,
            )?),
            second: Box::new(node(
                item.get("second").unwrap_or(&Value::Null),
                &format!("{at}.second"),
                depth + 1,
                ids,
                leaves,
            )?),
        })
    }
}

pub(crate) fn layout(value: &Value) -> Result<Layout> {
    read_layout(value, true)
}

/// Validate shared fields and seed valid defaults before restoring local navigation.
pub(crate) fn shared_layout(value: &Value) -> Result<Layout> {
    read_layout(value, false)
}

fn read_layout(value: &Value, navigation: bool) -> Result<Layout> {
    let root = object(value, "layout")?;
    if root.get("version").and_then(Value::as_u64) != Some(2) {
        return Err("layout.version: unsupported version".into());
    }
    let spaces = root
        .get("workspaces")
        .and_then(Value::as_array)
        .filter(|v| !v.is_empty())
        .ok_or("layout.workspaces: expected a nonempty list")?;
    let show_agents = match root.get("show_agents") {
        None => false,
        Some(Value::Bool(value)) => *value,
        _ => return Err("layout.show_agents: expected a boolean".into()),
    };
    let mut ids = HashSet::new();
    let mut workspaces = Vec::new();
    for (i, space) in spaces.iter().enumerate() {
        let at = format!("workspaces[{i}]");
        let item = object(space, &at)?;
        let id = identify(item, &at, &mut ids)?;
        let name = name(item, &at)?;
        let icon = match item.get("icon") {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) if (1..=2).contains(&s.chars().count()) && printable(s) => {
                Some(s.clone())
            }
            _ => {
                return Err(format!(
                    "{at}.icon: expected a printable icon of one or two characters"
                ));
            }
        };
        let raw_tabs = item
            .get("tabs")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{at}.tabs: expected a list"))?;
        let mut tabs = Vec::new();
        for (j, tab) in raw_tabs.iter().enumerate() {
            let at = format!("{at}.tabs[{j}]");
            let tab = object(tab, &at)?;
            let id = identify(tab, &at, &mut ids)?;
            let name = self::name(tab, &at)?;
            let mut leaves = Vec::new();
            let tree = node(
                tab.get("tree").unwrap_or(&Value::Null),
                &format!("{at}.tree"),
                0,
                &mut ids,
                &mut leaves,
            )?;
            let focus = if navigation {
                string(tab, "focus", &at)?
            } else {
                leaves[0].clone()
            };
            if !leaves.contains(&focus) {
                return Err(format!("{at}.focus: expected an existing leaf identity"));
            }
            tabs.push(Tab {
                id,
                name,
                tree,
                focus,
            });
        }
        let selected = if navigation {
            string(item, "selected", &at)?
        } else {
            tabs.first().map(|tab| tab.id.clone()).unwrap_or_default()
        };
        if !(tabs.is_empty() && selected.is_empty() || tabs.iter().any(|t| t.id == selected)) {
            return Err(format!(
                "{at}.selected: expected an existing tab identity (or empty for an empty workspace)"
            ));
        }
        workspaces.push(Workspace {
            id,
            name,
            icon,
            tabs,
            selected,
        });
    }
    let selected = if navigation {
        string(root, "selected", "layout")?
    } else {
        workspaces[0].id.clone()
    };
    if !workspaces.iter().any(|w| w.id == selected) {
        return Err("layout.selected: expected an existing workspace identity".into());
    }
    let focus = if navigation {
        root.get("focus")
            .and_then(Value::as_bool)
            .ok_or("layout.focus: expected a boolean")?
    } else {
        false
    };
    Ok(Layout {
        workspaces,
        selected,
        focus,
        show_agents,
    })
}
