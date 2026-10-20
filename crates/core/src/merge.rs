//! Three-way merging of validated schema-v2 documents, without persistence.

use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Concurrent edits cannot be combined without discarding one window's change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutConflict;

impl std::fmt::Display for LayoutConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Layout changed in another window; refresh and try again")
    }
}

impl std::error::Error for LayoutConflict {}

/// Merge shared edits from three individually validated schema-v2 documents.
///
/// Navigation is window-local and removed before merging. Objects merge by field;
/// workspace and tab lists merge by stable identity. Conflicting tree changes,
/// incompatible reorders, duplicate moves and deleting every workspace fail.
///
/// The returned document has no navigation fields. Restore local navigation and
/// validate the complete merged schema and size before any future persistence.
/// This function does not access a database or change any input document.
pub fn merge_shared_layout(
    base: &Value,
    mine: &Value,
    latest: &Value,
) -> Result<Value, LayoutConflict> {
    let base = shared(base)?;
    let mine = shared(mine)?;
    let latest = shared(latest)?;
    let merged = merge(Some(&base), Some(&mine), Some(&latest), false)?.ok_or(LayoutConflict)?;
    let spaces = merged["workspaces"].as_array().ok_or(LayoutConflict)?;
    if spaces.is_empty() {
        return Err(LayoutConflict);
    }
    let mut identities = BTreeSet::new();
    for space in spaces {
        if !identities.insert(id(space)?) {
            return Err(LayoutConflict);
        }
        for tab in space["tabs"].as_array().ok_or(LayoutConflict)? {
            if !identities.insert(id(tab)?) {
                return Err(LayoutConflict);
            }
        }
    }
    Ok(merged)
}

fn shared(state: &Value) -> Result<Value, LayoutConflict> {
    let mut state = state.clone();
    let root = state.as_object_mut().ok_or(LayoutConflict)?;
    root.remove("selected");
    root.remove("focus");
    for space in root
        .get_mut("workspaces")
        .and_then(Value::as_array_mut)
        .ok_or(LayoutConflict)?
    {
        let space = space.as_object_mut().ok_or(LayoutConflict)?;
        space.remove("selected");
        for tab in space
            .get_mut("tabs")
            .and_then(Value::as_array_mut)
            .ok_or(LayoutConflict)?
        {
            tab.as_object_mut().ok_or(LayoutConflict)?.remove("focus");
        }
    }
    Ok(state)
}

fn merge(
    base: Option<&Value>,
    mine: Option<&Value>,
    latest: Option<&Value>,
    tree: bool,
) -> Result<Option<Value>, LayoutConflict> {
    if same(mine, base) {
        return Ok(latest.cloned());
    }
    if same(latest, base) || same(mine, latest) {
        return Ok(mine.cloned());
    }
    if tree {
        return Err(LayoutConflict);
    }
    match (base, mine, latest) {
        (Some(Value::Object(base)), Some(Value::Object(mine)), Some(Value::Object(latest))) => {
            let keys: BTreeSet<_> = base
                .keys()
                .chain(mine.keys())
                .chain(latest.keys())
                .collect();
            let mut result = Map::new();
            for key in keys {
                if let Some(value) =
                    merge(base.get(key), mine.get(key), latest.get(key), key == "tree")?
                {
                    result.insert(key.clone(), value);
                }
            }
            Ok(Some(Value::Object(result)))
        }
        (Some(Value::Array(base)), Some(Value::Array(mine)), Some(Value::Array(latest))) => {
            let maps = [
                identity_map(base)?,
                identity_map(mine)?,
                identity_map(latest)?,
            ];
            let common: BTreeSet<_> = maps[0]
                .keys()
                .filter(|id| maps[1].contains_key(*id) && maps[2].contains_key(*id))
                .cloned()
                .collect();
            let order = |items: &[Value]| -> Vec<Identity> {
                let mut seen = BTreeSet::new();
                items
                    .iter()
                    .filter_map(|item| identity(item).ok())
                    .filter(|id| common.contains(id) && seen.insert(id.clone()))
                    .collect()
            };
            let [base_order, mine_order, latest_order] = [order(base), order(mine), order(latest)];
            let reordered = base_order != mine_order;
            if reordered && latest_order != base_order && latest_order != mine_order {
                return Err(LayoutConflict);
            }
            let keys: BTreeSet<_> = maps.iter().flat_map(|map| map.keys()).collect();
            let mut merged = BTreeMap::new();
            for key in keys {
                if let Some(value) = merge(
                    maps[0].get(key),
                    maps[1].get(key),
                    maps[2].get(key),
                    matches!(key, Identity::Text(text) if text == "tree"),
                )? {
                    merged.insert(key.clone(), value);
                }
            }
            let (primary, secondary) = if reordered {
                (mine, latest)
            } else {
                (latest, mine)
            };
            let mut seen = BTreeSet::new();
            let mut result = Vec::new();
            for item in primary.iter().chain(secondary) {
                let id = identity(item)?;
                if seen.insert(id.clone())
                    && let Some(value) = merged.get(&id)
                {
                    result.push(value.clone());
                }
            }
            Ok(Some(Value::Array(result)))
        }
        _ => Err(LayoutConflict),
    }
}

fn id(value: &Value) -> Result<&str, LayoutConflict> {
    value
        .get("id")
        .and_then(Value::as_str)
        .ok_or(LayoutConflict)
}

// Known workspace/tab IDs are strings. Extension lists can also use Python's
// other hashable JSON values as IDs; preserve their equality and overwrite rules.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Identity {
    Text(String),
    Integer(String),
    Float(u64),
    Null,
}

fn identity(item: &Value) -> Result<Identity, LayoutConflict> {
    let value = item.get("id").ok_or(LayoutConflict)?;
    match value {
        Value::String(text) => Ok(Identity::Text(text.clone())),
        Value::Null => Ok(Identity::Null),
        _ => {
            let text = number(value).ok_or(LayoutConflict)?;
            if is_integer(text) {
                return Ok(Identity::Integer(normalize(text).into()));
            }
            let number = text.parse::<f64>().map_err(|_| LayoutConflict)?;
            if !number.is_finite() {
                return Err(LayoutConflict);
            }
            if number.fract() == 0.0 {
                Ok(Identity::Integer(normalize(&format!("{number:.0}")).into()))
            } else {
                Ok(Identity::Float(number.to_bits()))
            }
        }
    }
}

fn identity_map(items: &[Value]) -> Result<BTreeMap<Identity, Value>, LayoutConflict> {
    let mut result = BTreeMap::new();
    for item in items {
        result.insert(identity(item)?, item.clone());
    }
    Ok(result)
}

// Python compares numeric JSON values by value, including bool/int equivalence.
// Large integers must not be rounded through f64 when compared to a float.
fn same(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => match (a, b) {
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(Some(a), Some(b)))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len() && a.iter().all(|(key, a)| same(Some(a), b.get(key)))
            }
            _ => match (number(a), number(b)) {
                (Some(a), Some(b)) => same_number(a, b),
                _ => a == b,
            },
        },
        _ => false,
    }
}

fn number(value: &Value) -> Option<&str> {
    match value {
        Value::Number(value) => Some(value.as_str()),
        Value::Bool(true) => Some("1"),
        Value::Bool(false) => Some("0"),
        _ => None,
    }
}

fn same_number(a: &str, b: &str) -> bool {
    match (is_integer(a), is_integer(b)) {
        (true, true) => normalize(a) == normalize(b),
        (false, false) => a.parse::<f64>().ok() == b.parse::<f64>().ok(),
        _ => {
            let (int, float) = if is_integer(a) { (a, b) } else { (b, a) };
            float.parse::<f64>().is_ok_and(|float| {
                float.is_finite()
                    && float.fract() == 0.0
                    && normalize(int) == normalize(&format!("{float:.0}"))
            })
        }
    }
}

fn is_integer(value: &str) -> bool {
    !value.contains(['.', 'e', 'E'])
}

fn normalize(value: &str) -> &str {
    if value == "-0" { "0" } else { value }
}
