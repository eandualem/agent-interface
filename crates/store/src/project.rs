//! Project model edits onto retained schema objects without losing extensions.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write};

use serde::{Serialize, Serializer};
use serde_json::{Value, value::RawValue};
use tmux_workspaces_core::{Direction, Layout, Node};

type RawObject<'a> = BTreeMap<String, &'a RawValue>;
type Object<'a> = BTreeMap<String, Field<'a>>;

enum Field<'a> {
    Raw(&'a RawValue),
    Value(Value),
    Object(Object<'a>),
    Array(Vec<Field<'a>>),
}

impl Serialize for Field<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Raw(value) => value.serialize(serializer),
            Self::Value(value) => value.serialize(serializer),
            Self::Object(value) => value.serialize(serializer),
            Self::Array(value) => value.serialize(serializer),
        }
    }
}

fn value<'a>(value: impl Into<Value>) -> Field<'a> {
    Field::Value(value.into())
}

fn raw_object(raw: &str) -> RawObject<'_> {
    serde_json::from_str(raw).expect("already validated schema object")
}

fn index<'a>(root: &RawObject<'a>, objects: &mut HashMap<String, RawObject<'a>>) {
    fn insert<'a>(item: RawObject<'a>, objects: &mut HashMap<String, RawObject<'a>>) {
        let id: String = serde_json::from_str(item["id"].get()).expect("validated identity");
        objects.insert(id, item);
    }
    fn node<'a>(raw: &'a RawValue, objects: &mut HashMap<String, RawObject<'a>>) {
        let item = raw_object(raw.get());
        if !item.contains_key("agent") {
            node(item["first"], objects);
            node(item["second"], objects);
        }
        insert(item, objects);
    }
    let spaces: Vec<&RawValue> =
        serde_json::from_str(root["workspaces"].get()).expect("validated workspaces");
    for space in spaces {
        let space = raw_object(space.get());
        let tabs: Vec<&RawValue> =
            serde_json::from_str(space["tabs"].get()).expect("validated tabs");
        for tab in tabs {
            let tab = raw_object(tab.get());
            node(tab["tree"], objects);
            insert(tab, objects);
        }
        insert(space, objects);
    }
}

struct Projection<'a> {
    objects: HashMap<String, RawObject<'a>>,
    count: usize,
}

impl<'a> Projection<'a> {
    fn object(&mut self, id: &str, children: &[&str]) -> Result<Object<'a>, String> {
        self.count += 1;
        if self.count > 10_000 {
            return Err("too many model objects (limit 10000)".into());
        }
        let mut object: Object<'a> = self
            .objects
            .get(id)
            .into_iter()
            .flat_map(|item| item.iter())
            .filter(|(key, _)| !children.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), Field::Raw(value)))
            .collect();
        object.insert("id".into(), value(id));
        Ok(object)
    }

    fn node(&mut self, node: &Node, depth: usize) -> Result<Field<'a>, String> {
        if depth > 64 {
            return Err("split nesting exceeds 64 levels".into());
        }
        let object = match node {
            Node::Leaf(leaf) => {
                let mut item = self.object(&leaf.id, &["first", "second", "direction", "ratio"])?;
                item.insert(
                    "agent".into(),
                    value(leaf.attachment.as_ref().map(|a| a.session.clone())),
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
                    item.insert("empty".into(), value(true));
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
                    value(match direction {
                        Direction::Right => "right",
                        Direction::Below => "below",
                    }),
                );
                if !ratio.is_finite() {
                    return Err("split ratio must be finite".into());
                }
                // Keep an absent default or an unchanged original number spelling.
                if !self.objects.contains_key(id.as_str())
                    || self
                        .objects
                        .get(id)
                        .and_then(|item| item.get("ratio"))
                        .and_then(|raw| raw.get().parse::<f64>().ok())
                        .unwrap_or(0.5)
                        != *ratio
                {
                    item.insert("ratio".into(), value(*ratio));
                }
                item.insert("first".into(), self.node(first, depth + 1)?);
                item.insert("second".into(), self.node(second, depth + 1)?);
                item
            }
        };
        Ok(Field::Object(object))
    }
}

fn optional(item: &mut Object<'_>, key: &str, text: Option<&str>) {
    if let Some(text) = text {
        item.insert(key.into(), value(text));
    } else {
        item.remove(key);
    }
}

pub(crate) fn apply(raw: &str, model: &Layout) -> Result<String, String> {
    let original = raw_object(raw);
    let mut projection = Projection {
        objects: HashMap::new(),
        count: 0,
    };
    index(&original, &mut projection.objects);
    let mut root: Object<'_> = original
        .iter()
        .filter(|(key, _)| key.as_str() != "workspaces")
        .map(|(key, value)| (key.clone(), Field::Raw(value)))
        .collect();
    let mut spaces = Vec::new();
    for space in &model.workspaces {
        let mut item = projection.object(&space.id, &["tabs"])?;
        item.insert("name".into(), value(space.name.clone()));
        item.insert("selected".into(), value(space.selected.clone()));
        // Missing and explicit null both represent no icon; retain either if unchanged.
        if space.icon.is_some()
            || projection
                .objects
                .get(&space.id)
                .and_then(|item| item.get("icon"))
                .is_some_and(|icon| icon.get() != "null")
        {
            optional(&mut item, "icon", space.icon.as_deref());
        }
        if space.symbol.is_some()
            || projection
                .objects
                .get(&space.id)
                .and_then(|item| item.get("symbol"))
                .is_some_and(|symbol| symbol.get() != "null")
        {
            optional(&mut item, "symbol", space.symbol.as_deref());
        }
        let mut tabs = Vec::new();
        for tab in &space.tabs {
            let mut item = projection.object(&tab.id, &["tree"])?;
            item.insert("name".into(), value(tab.name.clone()));
            item.insert("focus".into(), value(tab.focus.clone()));
            item.insert("tree".into(), projection.node(&tab.tree, 0)?);
            tabs.push(Field::Object(item));
        }
        item.insert("tabs".into(), Field::Array(tabs));
        spaces.push(Field::Object(item));
    }
    root.insert("workspaces".into(), Field::Array(spaces));
    root.insert("selected".into(), value(model.selected.clone()));
    root.insert("focus".into(), value(model.focus));
    if !model.show_agents || root.contains_key("show_agents") {
        root.insert("show_agents".into(), value(model.show_agents));
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
