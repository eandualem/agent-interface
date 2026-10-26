//! Title-bar dragging previews a destination, then saves one detached tree edit.

use std::collections::HashMap;

use agent_interface_core::{Direction, Layout, Node};
use agent_interface_store::Document;
use gpui::{Bounds, Context, Pixels, Point, Window, px};

use super::Browser;
use crate::library::WindowLayout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    fn at(bounds: Bounds<Pixels>, position: Point<Pixels>) -> Option<Self> {
        if !bounds.contains(&position)
            || bounds.size.width <= px(0.)
            || bounds.size.height <= px(0.)
        {
            return None;
        }
        let x = f32::from(position.x - bounds.origin.x) / f32::from(bounds.size.width);
        let y = f32::from(position.y - bounds.origin.y) / f32::from(bounds.size.height);
        [
            (x, Self::Left),
            (1. - x, Self::Right),
            (y, Self::Top),
            (1. - y, Self::Bottom),
        ]
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .filter(|(distance, _)| *distance <= 0.25)
        .map(|(_, edge)| edge)
    }

    pub fn preview(self, mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        match self {
            Self::Left | Self::Right => {
                bounds.size.width /= 2.;
                if self == Self::Right {
                    bounds.origin.x += bounds.size.width;
                }
            }
            Self::Top | Self::Bottom => {
                bounds.size.height /= 2.;
                if self == Self::Bottom {
                    bounds.origin.y += bounds.size.height;
                }
            }
        }
        bounds
    }

    fn placement(self) -> (Direction, bool) {
        match self {
            Self::Left => (Direction::Right, true),
            Self::Right => (Direction::Right, false),
            Self::Top => (Direction::Below, true),
            Self::Bottom => (Direction::Below, false),
        }
    }
}

pub(super) struct Drag {
    pub dragging: bool,
    pub active: bool,
    pub destination: Option<(String, Edge)>,
    source: String,
    workspace: String,
    tab: String,
    original: Node,
    bounds: HashMap<String, Bounds<Pixels>>,
    start: Point<Pixels>,
    preview: Option<Node>,
    cancelled: bool,
}

impl Drag {
    pub fn begin(
        model: &Layout,
        source: &str,
        bounds: &HashMap<String, Bounds<Pixels>>,
        position: Point<Pixels>,
    ) -> Option<Self> {
        let tab = model.tab()?;
        let leaves = tab.tree.leaves();
        if model.focus_mode() || leaves.len() < 2 || !leaves.iter().any(|leaf| leaf.id == source) {
            return None;
        }
        Some(Self {
            dragging: true,
            active: false,
            destination: None,
            source: source.into(),
            workspace: model.selected.clone(),
            tab: tab.id.clone(),
            original: tab.tree.clone(),
            bounds: bounds.clone(),
            start: position,
            preview: None,
            cancelled: false,
        })
    }

    pub fn valid(&self, model: &Layout) -> bool {
        !self.cancelled
            && !model.focus_mode()
            && model.selected == self.workspace
            && model
                .tab()
                .is_some_and(|tab| tab.id == self.tab && tab.tree == self.original)
    }

    pub fn same_bounds(&self, bounds: &HashMap<String, Bounds<Pixels>>) -> bool {
        &self.bounds == bounds
    }

    /// Keep the gesture until release so cancellation cannot become a click.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.destination = None;
    }

    /// Repaint only when the gesture starts or the highlighted edge changes.
    pub fn motion(&mut self, position: Point<Pixels>) -> bool {
        if self.cancelled {
            return false;
        }
        let was_active = self.active;
        let delta = position - self.start;
        self.active |= f32::from(delta.x).hypot(f32::from(delta.y)) >= 6.;
        let destination = self
            .active
            .then(|| {
                self.bounds.iter().find_map(|(id, bounds)| {
                    (id != &self.source)
                        .then(|| Edge::at(*bounds, position).map(|edge| (id.clone(), edge)))
                        .flatten()
                })
            })
            .flatten();
        let changed = was_active != self.active || destination != self.destination;
        self.destination = destination;
        changed
    }

    pub fn proposal(&mut self, window: &WindowLayout) -> Option<(Document, Layout)> {
        if !self.active || !self.valid(window.model()) {
            return None;
        }
        let (destination, edge) = self.destination.as_ref()?;
        let (direction, before) = edge.placement();
        let (base, mut model) = window.save_snapshot();
        if !model.move_pane(&self.source, destination, direction, before) {
            return None;
        }
        self.preview = Some(model.tab()?.tree.clone());
        self.dragging = false;
        Some((base, model))
    }

    pub fn tree(&self) -> Option<&Node> {
        self.preview.as_ref()
    }
}

impl Browser {
    pub(super) fn begin_pane_drag(&mut self, source: &str, position: Point<Pixels>) {
        if self.saving
            || self.save_failed
            || self.resize.is_some()
            || self.sidebar_drag.is_some()
            || self.chooser.is_some()
            || self.editing.is_some()
        {
            return;
        }
        self.pane_drag = self.layout.as_ref().and_then(|layout| {
            Drag::begin(layout.model(), source, &self.pane_bounds.borrow(), position)
        });
    }

    pub(super) fn pane_drag_motion(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = &mut self.pane_drag else {
            return;
        };
        if !event.dragging()
            || !self
                .layout
                .as_ref()
                .is_some_and(|layout| drag.valid(layout.model()))
            || !drag.same_bounds(&self.pane_bounds.borrow())
        {
            drag.cancel();
            cx.notify();
            return;
        }
        let changed = drag.motion(event.position);
        if drag.active {
            if let Some(focus) = &self.navigation {
                window.focus(focus, cx);
            }
            cx.stop_propagation();
        }
        if changed {
            cx.notify();
        }
    }

    pub(super) fn finish_pane_drag(&mut self, event: &gpui::MouseUpEvent, cx: &mut Context<Self>) {
        let Some(mut drag) = self.pane_drag.take() else {
            return;
        };
        drag.motion(event.position);
        if !drag.active && !drag.cancelled {
            return;
        }
        // Called at the front of the bubble phase: GPUI has already cleared
        // button presses during capture, but has not fired their click handlers.
        cx.stop_propagation();
        if let Some(update) = self
            .library
            .as_ref()
            .and_then(crate::library::Library::take_update)
        {
            self.apply(update);
        }
        if !self.saving
            && !self.save_failed
            && drag.same_bounds(&self.pane_bounds.borrow())
            && let Some(proposal) = self
                .layout
                .as_ref()
                .and_then(|layout| drag.proposal(layout))
        {
            self.pending_save = Some(proposal);
            self.pane_drag = Some(drag);
            self.save(cx);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_store::Store;
    use gpui::{point, size};

    #[test]
    fn placement_uses_edges_and_leaves_the_center_as_a_cancel_target() {
        let bounds = Bounds::new(point(px(100.), px(200.)), size(px(400.), px(200.)));
        for (position, edge) in [
            (point(px(110.), px(300.)), Edge::Left),
            (point(px(490.), px(300.)), Edge::Right),
            (point(px(300.), px(210.)), Edge::Top),
            (point(px(300.), px(390.)), Edge::Bottom),
        ] {
            assert_eq!(Edge::at(bounds, position), Some(edge));
            assert!(edge.preview(bounds).contains(&position));
        }
        assert_eq!(Edge::at(bounds, point(px(300.), px(300.))), None);
        assert_eq!(Edge::at(bounds, point(px(99.), px(300.))), None);
    }

    #[test]
    fn drag_is_detached_persists_on_drop_and_rejects_a_changed_layout() {
        let directory = tempfile::tempdir().unwrap();
        let mut base = Store::open_or_create(directory.path())
            .unwrap()
            .load()
            .unwrap();
        let mut store = Store::open_existing_writable(directory.path()).unwrap();
        let mut model = base.model().clone();
        model.add_tab(None, "/tmp".into(), true);
        let source = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/tmp".into(), true);
        let destination = model.pane().unwrap().id.clone();
        assert!(!store.save(&mut base, &model).unwrap().conflict);
        let mut window = WindowLayout::new(base);
        let bounds = HashMap::from([
            (
                source.clone(),
                Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.))),
            ),
            (
                destination.clone(),
                Bounds::new(point(px(208.), px(0.)), size(px(200.), px(200.))),
            ),
        ]);
        let mut drag =
            Drag::begin(window.model(), &source, &bounds, point(px(20.), px(10.))).unwrap();
        assert!(!drag.motion(point(px(22.), px(10.))));
        assert!(drag.proposal(&window).is_none());
        assert!(drag.motion(point(px(308.), px(190.))));
        assert!(!drag.motion(point(px(310.), px(190.))));
        assert!(drag.tree().is_none());
        assert_eq!(drag.destination, Some((destination.clone(), Edge::Bottom)));
        let original = window.model().clone();
        let (mut base, proposed) = drag.proposal(&window).unwrap();
        assert_eq!(window.model(), &original);
        assert_eq!(drag.tree(), Some(&proposed.tab().unwrap().tree));
        assert!(matches!(
            &proposed.tab().unwrap().tree,
            Node::Split {
                direction: Direction::Below,
                ..
            }
        ));
        assert!(!store.save(&mut base, &proposed).unwrap().conflict);
        assert_eq!(
            store.load().unwrap().model().tab().unwrap().tree,
            proposed.tab().unwrap().tree
        );
        window
            .model_mut()
            .split(Direction::Below, "/tmp".into(), true);
        assert!(drag.proposal(&window).is_none());
        assert!(!drag.same_bounds(&HashMap::new()));

        let window = WindowLayout::new(store.load().unwrap());
        let mut cancelled =
            Drag::begin(window.model(), &source, &bounds, point(px(20.), px(10.))).unwrap();
        cancelled.motion(point(px(308.), px(190.)));
        cancelled.cancel();
        assert!(cancelled.dragging);
        assert!(cancelled.active);
        assert_eq!(cancelled.destination, None);
        assert!(!cancelled.motion(point(px(308.), px(10.))));
        assert!(cancelled.proposal(&window).is_none());
    }
}
