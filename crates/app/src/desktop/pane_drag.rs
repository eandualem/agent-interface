//! Title-bar dragging carries the pane with the pointer while the others
//! rearrange around where it would land, then saves one detached tree edit.

use std::collections::HashMap;

use agent_interface_core::{Direction, Layout, Node};
use agent_interface_store::Document;
use gpui::{Bounds, Context, Pixels, Point, Window};

use super::{
    Browser,
    pane_motion::{self, Motion, Rect},
};
use crate::library::WindowLayout;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Edge {
    Left,
    Right,
    Top,
    Bottom,
}

impl Edge {
    /// The edge of `rect` nearest to a point, relative to the rectangle's size.
    fn nearest(rect: Rect, x: f32, y: f32) -> Self {
        let x = ((x - rect.x) / rect.w.max(1.)).clamp(0., 1.);
        let y = ((y - rect.y) / rect.h.max(1.)).clamp(0., 1.);
        [
            (x, Self::Left),
            (1. - x, Self::Right),
            (y, Self::Top),
            (1. - y, Self::Bottom),
        ]
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map_or(Self::Left, |(_, edge)| edge)
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
    pub origin: Option<super::navigation_origin::Origin>,
    profile: Option<String>,
    pub dragging: bool,
    pub active: bool,
    pub destination: Option<(String, Edge)>,
    source: String,
    workspace: String,
    tab: String,
    original: Node,
    /// The tab without the carried pane: where the pointer is read against,
    /// so the rearranging preview never changes what the pointer is over.
    base: Option<Node>,
    /// A detached copy of the layout, for previewing moves.
    model: Layout,
    bounds: HashMap<String, Bounds<Pixels>>,
    start: Point<Pixels>,
    preview: Option<Node>,
    cancelled: bool,
}

/// Whether two trees place the same panes in the same arrangement, whatever
/// their split identities and ratios.
fn same_shape(a: &Node, b: &Node) -> bool {
    match (a, b) {
        (Node::Leaf(a), Node::Leaf(b)) => a.id == b.id,
        (
            Node::Split {
                direction: da,
                first: fa,
                second: sa,
                ..
            },
            Node::Split {
                direction: db,
                first: fb,
                second: sb,
                ..
            },
        ) => da == db && same_shape(fa, fb) && same_shape(sa, sb),
        _ => false,
    }
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
            origin: None,
            profile: model.workspace()?.profile_id.clone(),
            dragging: true,
            active: false,
            destination: None,
            source: source.into(),
            workspace: model.selected.clone(),
            tab: tab.id.clone(),
            original: tab.tree.clone(),
            base: tab.tree.clone().without_leaf(source),
            model: model.clone(),
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
                .workspace()
                .is_some_and(|space| space.profile_id == self.profile)
            && model
                .tab()
                .is_some_and(|tab| tab.id == self.tab && tab.tree == self.original)
    }

    /// The panes as they were when the gesture began.
    pub fn bounds(&self) -> &HashMap<String, Bounds<Pixels>> {
        &self.bounds
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// Where the header was pressed.
    pub fn start(&self) -> Point<Pixels> {
        self.start
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Keep the gesture until release so cancellation cannot become a click.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.destination = None;
        self.preview = None;
    }

    /// Follow the pointer. `area` is where the tab's panes are laid out; the
    /// destination is the pane of the remaining layout under the pointer and
    /// its nearest edge. Returns whether the gesture started or the
    /// destination changed.
    pub fn motion(&mut self, position: Point<Pixels>, area: Option<Rect>) -> bool {
        if self.cancelled {
            return false;
        }
        let was_active = self.active;
        let delta = position - self.start;
        self.active |= f32::from(delta.x).hypot(f32::from(delta.y)) >= 6.;
        let (x, y) = (f32::from(position.x), f32::from(position.y));
        let destination = self
            .active
            .then_some(())
            .and(area.filter(|area| area.contains(x, y)))
            .zip(self.base.as_ref())
            .and_then(|(area, base)| {
                let mut places = HashMap::new();
                pane_motion::place(
                    base,
                    area,
                    super::design::tokens().size.pane_gap,
                    &mut places,
                );
                // In the gap between two panes, the nearer one.
                places
                    .into_iter()
                    .min_by(|a, b| {
                        a.1.distance(x, y)
                            .total_cmp(&b.1.distance(x, y))
                            // Midway between two panes, always the same one.
                            .then_with(|| a.0.cmp(&b.0))
                    })
                    .map(|(id, rect)| (id, Edge::nearest(rect, x, y)))
            });
        let changed = was_active != self.active || destination != self.destination;
        if destination != self.destination {
            self.preview = destination.as_ref().and_then(|(destination, edge)| {
                let (direction, before) = edge.placement();
                let mut model = self.model.clone();
                model
                    .move_pane(&self.source, destination, direction, before)
                    .then(|| model.tab().map(|tab| tab.tree.clone()))
                    .flatten()
                    // Back where it began: keep the tree and its ratios.
                    .filter(|tree| !same_shape(tree, &self.original))
            });
            self.destination = destination;
        }
        changed
    }

    /// The saved change for the previewed arrangement, if it differs from
    /// the tab as it is.
    pub fn proposal(&mut self, window: &WindowLayout) -> Option<(Document, Layout)> {
        if !self.active || !self.valid(window.model()) {
            return None;
        }
        self.preview.as_ref()?;
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
    pub(super) fn begin_pane_drag(
        &mut self,
        origin: Option<&super::navigation_origin::Origin>,
        source: &str,
        position: Point<Pixels>,
    ) {
        if !self.selected_origin_is_current(origin) {
            return;
        }
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
            Drag::begin(layout.model(), source, &self.pane_bounds.borrow(), position).map(
                |mut drag| {
                    drag.origin = origin.cloned();
                    drag
                },
            )
        });
    }

    pub(super) fn pane_drag_motion(
        &mut self,
        event: &gpui::MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let origin_valid = self
            .pane_drag
            .as_ref()
            .is_some_and(|drag| self.selected_origin_is_current(drag.origin.as_ref()));
        let Some(drag) = &mut self.pane_drag else {
            return;
        };
        if !origin_valid
            || !event.dragging()
            || !self
                .layout
                .as_ref()
                .is_some_and(|layout| drag.valid(layout.model()))
        {
            drag.cancel();
            if let Some(motion) = &mut self.pane_motion {
                motion.release();
            }
            cx.notify();
            return;
        }
        let area = self.pane_motion.as_ref().and_then(Motion::area);
        drag.motion(event.position, area);
        if drag.active && !drag.is_cancelled() {
            // A drag begun while an earlier one still settles carries anew.
            if !self.pane_motion.as_ref().is_some_and(Motion::is_carrying) {
                self.pane_motion = Motion::carrying(drag.source(), drag.bounds(), drag.start());
                // The panes move; their sessions keep their sizes until they land.
                for terminal in self.terminals.values() {
                    terminal.update(cx, |terminal, cx| terminal.hold_size(true, cx));
                }
                // The area is known only now: read the pointer against it, so
                // the preview already shows where a release would land.
                let area = self.pane_motion.as_ref().and_then(Motion::area);
                drag.motion(event.position, area);
            }
            if let Some(motion) = &mut self.pane_motion {
                motion.follow(event.position);
            }
            if let Some(focus) = &self.navigation {
                window.focus(focus, cx);
            }
            cx.stop_propagation();
            cx.notify();
        }
    }

    pub(super) fn finish_pane_drag(&mut self, event: &gpui::MouseUpEvent, cx: &mut Context<Self>) {
        let Some(mut drag) = self.pane_drag.take() else {
            return;
        };
        let area = self.pane_motion.as_ref().and_then(Motion::area);
        drag.motion(event.position, area);
        if let Some(motion) = &mut self.pane_motion {
            motion.release();
        }
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
            && self.selected_origin_is_current(drag.origin.as_ref())
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
    use gpui::{point, px, size};

    const AREA: Rect = Rect {
        x: 0.,
        y: 0.,
        w: 408.,
        h: 200.,
    };

    fn two_panes() -> (HashMap<String, Bounds<Pixels>>, [Bounds<Pixels>; 2]) {
        let left = Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.)));
        let right = Bounds::new(point(px(208.), px(0.)), size(px(200.), px(200.)));
        (HashMap::new(), [left, right])
    }

    #[test]
    fn the_nearest_edge_covers_the_whole_pane() {
        let rect = Rect {
            x: 100.,
            y: 200.,
            w: 400.,
            h: 200.,
        };
        for (x, y, edge) in [
            (110., 300., Edge::Left),
            (490., 300., Edge::Right),
            (300., 210., Edge::Top),
            (300., 390., Edge::Bottom),
            (250., 300., Edge::Left),
        ] {
            assert_eq!(Edge::nearest(rect, x, y), edge);
        }
    }

    #[test]
    fn drag_previews_while_moving_persists_on_drop_and_rejects_a_changed_layout() {
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
        let (mut bounds, [left, right]) = two_panes();
        bounds.insert(source.clone(), left);
        bounds.insert(destination.clone(), right);
        let mut drag =
            Drag::begin(window.model(), &source, &bounds, point(px(20.), px(10.))).unwrap();
        // Below the threshold nothing starts.
        assert!(!drag.motion(point(px(22.), px(10.)), Some(AREA)));
        assert!(drag.proposal(&window).is_none());
        // Over the left of what remains, the pane is back where it began.
        assert!(drag.motion(point(px(40.), px(100.)), Some(AREA)));
        assert_eq!(drag.destination, Some((destination.clone(), Edge::Left)));
        assert!(drag.tree().is_none());
        assert!(drag.proposal(&window).is_none());
        // Near the bottom the others already rearrange, before any release.
        assert!(drag.motion(point(px(204.), px(190.)), Some(AREA)));
        assert!(!drag.motion(point(px(206.), px(190.)), Some(AREA)));
        assert_eq!(drag.destination, Some((destination.clone(), Edge::Bottom)));
        assert!(matches!(
            drag.tree(),
            Some(Node::Split {
                direction: Direction::Below,
                ..
            })
        ));
        // Outside the panes there is nowhere to land.
        assert!(drag.motion(point(px(204.), px(400.)), Some(AREA)));
        assert!(drag.tree().is_none());
        assert!(drag.motion(point(px(204.), px(190.)), Some(AREA)));
        let original = window.model().clone();
        let (mut base, proposed) = drag.proposal(&window).unwrap();
        assert_eq!(window.model(), &original);
        assert_eq!(drag.tree(), Some(&proposed.tab().unwrap().tree));
        assert_eq!(proposed.tab().unwrap().focus, source);
        assert!(!store.save(&mut base, &proposed).unwrap().conflict);
        assert_eq!(
            store.load().unwrap().model().tab().unwrap().tree,
            proposed.tab().unwrap().tree
        );
        window
            .model_mut()
            .split(Direction::Below, "/tmp".into(), true);
        assert!(drag.proposal(&window).is_none());

        let window = WindowLayout::new(store.load().unwrap());
        let mut cancelled =
            Drag::begin(window.model(), &source, &bounds, point(px(20.), px(10.))).unwrap();
        cancelled.motion(point(px(204.), px(10.)), Some(AREA));
        cancelled.cancel();
        assert!(cancelled.dragging);
        assert!(cancelled.active);
        assert_eq!(cancelled.destination, None);
        assert!(cancelled.tree().is_none());
        assert!(!cancelled.motion(point(px(308.), px(10.)), Some(AREA)));
        assert!(cancelled.proposal(&window).is_none());
    }

    #[test]
    fn a_binding_change_invalidates_a_gesture_with_an_unchanged_tree() {
        let mut model = Layout::initial("/".into());
        let first = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/".into(), true);
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(200.)));
        let drag = Drag::begin(
            &model,
            &first,
            &HashMap::from([(first.clone(), bounds)]),
            point(px(10.), px(10.)),
        )
        .unwrap();
        assert!(drag.valid(&model));
        let tree = model.tab().unwrap().tree.clone();
        model.workspaces[0].profile_id = Some("profile-a".into());
        assert_eq!(model.tab().unwrap().tree, tree);
        assert!(!drag.valid(&model));
    }
}
