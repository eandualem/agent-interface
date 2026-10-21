//! A native divider gesture previews a detached tree and saves only on release.

use crate::library::WindowLayout;
use gpui::{Bounds, Pixels, Point};
use tmux_workspaces_core::{Direction, Layout, Node};
use tmux_workspaces_store::Document;

pub(super) const DIVIDER: f32 = 5.0;

pub(super) struct Drag {
    pub split: String,
    pub bounds: Bounds<Pixels>,
    pub dragging: bool,
    workspace: String,
    tab: String,
    original: Node,
    preview: Node,
    direction: Direction,
    start: f32,
    initial_ratio: f64,
}

fn split(node: &Node, id: &str) -> Option<(Direction, f64)> {
    match node {
        Node::Split {
            id: candidate,
            direction,
            ratio,
            ..
        } if candidate == id => Some((*direction, *ratio)),
        Node::Split { first, second, .. } => split(first, id).or_else(|| split(second, id)),
        Node::Leaf(_) => None,
    }
}

fn set_ratio(node: &mut Node, id: &str, value: f64) {
    match node {
        Node::Split {
            id: candidate,
            ratio,
            ..
        } if candidate == id => *ratio = value,
        Node::Split { first, second, .. } => {
            set_ratio(first, id, value);
            set_ratio(second, id, value);
        }
        Node::Leaf(_) => {}
    }
}

impl Drag {
    pub fn begin(
        model: &Layout,
        id: &str,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
    ) -> Option<Self> {
        if model.focus {
            return None;
        }
        let tab = model.tab()?;
        let (direction, initial_ratio) = split(&tab.tree, id)?;
        let mut drag = Self {
            split: id.into(),
            bounds,
            dragging: true,
            workspace: model.selected.clone(),
            tab: tab.id.clone(),
            original: tab.tree.clone(),
            preview: tab.tree.clone(),
            direction,
            start: 0.0,
            initial_ratio,
        };
        drag.start = drag.coordinate(position);
        (drag.extent() > 0.0 && drag.start.is_finite()).then_some(drag)
    }

    fn coordinate(&self, point: Point<Pixels>) -> f32 {
        f32::from(match self.direction {
            Direction::Right => point.x,
            Direction::Below => point.y,
        })
    }

    fn extent(&self) -> f32 {
        f32::from(match self.direction {
            Direction::Right => self.bounds.size.width,
            Direction::Below => self.bounds.size.height,
        }) - DIVIDER
    }

    pub fn motion(&mut self, position: Point<Pixels>) {
        let delta = self.coordinate(position) - self.start;
        if delta.is_finite() {
            let ratio = if delta == 0.0 {
                self.initial_ratio
            } else {
                (self.initial_ratio + f64::from(delta / self.extent())).clamp(0.15, 0.85)
            };
            set_ratio(&mut self.preview, &self.split, ratio);
        }
    }

    pub fn valid(&self, model: &Layout) -> bool {
        !model.focus
            && model.selected == self.workspace
            && model
                .tab()
                .is_some_and(|tab| tab.id == self.tab && tab.tree == self.original)
    }

    pub fn tree(&self) -> &Node {
        &self.preview
    }

    pub fn proposal(&self, window: &WindowLayout) -> Option<(Document, Layout)> {
        if !self.valid(window.model()) || self.preview == self.original {
            return None;
        }
        let (base, mut proposed) = window.save_snapshot();
        proposed
            .workspaces
            .iter_mut()
            .find(|space| space.id == self.workspace)?
            .tabs
            .iter_mut()
            .find(|tab| tab.id == self.tab)?
            .tree = self.preview.clone();
        Some((base, proposed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};
    use std::path::Path;

    #[test]
    fn divider_preview_is_detached_bounded_and_rejects_changed_trees() {
        let path = Path::new("layouts.db");
        let raw = r#"{"version":2,"workspaces":[{"id":"000000000001","name":"Workspace","tabs":[{"id":"000000000002","name":"Tab","tree":{"id":"000000000003","direction":"right","ratio":0.5,"first":{"id":"000000000004","agent":null},"second":{"id":"000000000005","agent":null}},"focus":"000000000004"}],"selected":"000000000002"}],"selected":"000000000001","focus":false}"#;
        let mut window = WindowLayout::new(Document::parse(raw.into(), path).unwrap());
        for direction in [Direction::Right, Direction::Below] {
            if let Node::Split {
                direction: saved, ..
            } = &mut window.model_mut().workspaces[0].tabs[0].tree
            {
                *saved = direction;
            }
            let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(205.0), px(205.0)));
            let mut drag = Drag::begin(
                window.model(),
                "000000000003",
                bounds,
                point(px(100.0), px(100.0)),
            )
            .unwrap();
            assert!(drag.proposal(&window).is_none());
            drag.motion(point(px(150.0), px(150.0)));
            let (_, proposed) = drag.proposal(&window).unwrap();
            assert_eq!(
                split(&proposed.tab().unwrap().tree, "000000000003")
                    .unwrap()
                    .1,
                0.75
            );
            assert_eq!(
                split(&window.model().tab().unwrap().tree, "000000000003")
                    .unwrap()
                    .1,
                0.5
            );
            drag.motion(point(px(1000.0), px(1000.0)));
            assert_eq!(split(drag.tree(), "000000000003").unwrap().1, 0.85);
            drag.motion(point(px(-1000.0), px(-1000.0)));
            assert_eq!(split(drag.tree(), "000000000003").unwrap().1, 0.15);
            window.model_mut().focus = true;
            assert!(drag.proposal(&window).is_none());
            window.model_mut().focus = false;
            window.model_mut().rename_tab("Renamed");
            assert!(drag.valid(window.model()));
            window
                .model_mut()
                .attach(Some(tmux_workspaces_core::Attachment {
                    session: "external".into(),
                    source_socket: None,
                }));
            assert!(drag.proposal(&window).is_none());
            window.model_mut().attach(None);
        }
    }
}
