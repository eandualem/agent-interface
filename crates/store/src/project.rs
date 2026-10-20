//! Project model edits onto retained schema objects without losing extensions.

use std::collections::HashMap;
use std::io::{self, Write};

use serde_json::{Map, Value};
use tmux_workspaces_core::{Direction, Layout, Node};

type Object = Map<String, Value>;

fn index<'a>(root: &'a Object, objects: &mut HashMap<&'a str, &'a Object>) {
    fn insert<'a>(item: &'a Value, objects: &mut HashMap<&'a str, &'a Object>) {
        let object = item.as_object().expect("validated schema object");
        objects.insert(object["id"].as_str().expect("validated identity"), object);
    }
    fn node<'a>(item: &'a Value, objects: &mut HashMap<&'a str, &'a Object>) {
        insert(item, objects);
        if item.get("agent").is_none() {
            node(&item["first"], objects);
            node(&item["second"], objects);
        }
    }
    for space in root["workspaces"].as_array().expect("validated workspaces") {
        insert(space, objects);
        for tab in space["tabs"].as_array().expect("validated tabs") {
            insert(tab, objects);
            node(&tab["tree"], objects);
        }
    }
}

struct Projection<'a> {
    objects: HashMap<&'a str, &'a Object>,
    count: usize,
}

impl Projection<'_> {
    fn object(&mut self, id: &str, children: &[&str]) -> Result<Object, String> {
        self.count += 1;
        if self.count > 10_000 {
            return Err("too many model objects (limit 10000)".into());
        }
        let mut object: Object = self
            .objects
            .get(id)
            .into_iter()
            .flat_map(|item| item.iter())
            .filter(|(key, _)| !children.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        object.insert("id".into(), id.into());
        Ok(object)
    }

    fn node(&mut self, node: &Node, depth: usize) -> Result<Value, String> {
        if depth > 64 {
            return Err("split nesting exceeds 64 levels".into());
        }
        let object = match node {
            Node::Leaf(leaf) => {
                let mut item = self.object(&leaf.id, &["first", "second", "direction", "ratio"])?;
                item.insert(
                    "agent".into(),
                    leaf.attachment.as_ref().map(|a| a.session.clone()).into(),
                );
                optional(&mut item, "cwd", leaf.cwd.as_deref());
                optional(
                    &mut item,
                    "source_socket",
                    leaf.attachment
                        .as_ref()
                        .and_then(|a| a.source_socket.as_deref()),
                );
                if leaf.empty {
                    item.insert("empty".into(), true.into());
                } else {
                    item.remove("empty");
                }
                item
            }
            Node::Split {
                id,
                direction,
                ratio,
                first,
                second,
            } => {
                let mut item =
                    self.object(id, &["first", "second", "agent", "cwd", "source_socket"])?;
                item.insert(
                    "direction".into(),
                    match direction {
                        Direction::Right => "right",
                        Direction::Below => "below",
                    }
                    .into(),
                );
                if !ratio.is_finite() {
                    return Err("split ratio must be finite".into());
                }
                // Keep an absent default or an unchanged original number spelling.
                if !self.objects.contains_key(id.as_str())
                    || item.get("ratio").and_then(Value::as_f64).unwrap_or(0.5) != *ratio
                {
                    item.insert("ratio".into(), (*ratio).into());
                }
                item.insert("first".into(), self.node(first, depth + 1)?);
                item.insert("second".into(), self.node(second, depth + 1)?);
                item
            }
        };
        Ok(Value::Object(object))
    }
}

fn optional(item: &mut Object, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        item.insert(key.into(), value.into());
    } else {
        item.remove(key);
    }
}

pub(crate) fn apply(raw: &Value, model: &Layout) -> Result<String, String> {
    let original = raw.as_object().expect("validated document");
    let mut projection = Projection {
        objects: HashMap::new(),
        count: 0,
    };
    index(original, &mut projection.objects);
    let mut root: Object = original
        .iter()
        .filter(|(key, _)| key.as_str() != "workspaces")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut spaces = Vec::new();
    for space in &model.workspaces {
        let mut item = projection.object(&space.id, &["tabs"])?;
        item.insert("name".into(), space.name.clone().into());
        item.insert("selected".into(), space.selected.clone().into());
        // Missing and explicit null both represent no icon; retain either if unchanged.
        if space.icon.is_some() || item.get("icon").is_some_and(|icon| !icon.is_null()) {
            optional(&mut item, "icon", space.icon.as_deref());
        }
        let mut tabs = Vec::new();
        for tab in &space.tabs {
            let mut item = projection.object(&tab.id, &["tree"])?;
            item.insert("name".into(), tab.name.clone().into());
            item.insert("focus".into(), tab.focus.clone().into());
            item.insert("tree".into(), projection.node(&tab.tree, 0)?);
            tabs.push(Value::Object(item));
        }
        item.insert("tabs".into(), tabs.into());
        spaces.push(Value::Object(item));
    }
    root.insert("workspaces".into(), spaces.into());
    root.insert("selected".into(), model.selected.clone().into());
    root.insert("focus".into(), model.focus.into());
    if model.show_agents || root.contains_key("show_agents") {
        root.insert("show_agents".into(), model.show_agents.into());
    }
    let mut output = Limited(Vec::new());
    serde_json::to_writer(&mut output, &root).map_err(|error| error.to_string())?;
    String::from_utf8(output.0).map_err(|error| error.to_string())
}

struct Limited(Vec<u8>);
impl Write for Limited {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > crate::MAX_RECORD_BYTES - self.0.len() {
            return Err(io::Error::other("layout record exceeds 16 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
