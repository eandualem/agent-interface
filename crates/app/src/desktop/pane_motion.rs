//! Panes in motion: while one is dragged, and until the layout settles after
//! it is dropped, every pane is placed at a rectangle that eases toward its
//! place in the tree being shown. The dragged pane follows the pointer.

use std::{cell::Cell, collections::HashMap, rc::Rc, time::Instant};

use agent_interface_core::{Direction, Node};
use gpui::{
    AnyElement, Bounds, BoxShadow, Context, Pixels, Point, SharedString, Window, div, point,
    prelude::*, px,
};

use super::{
    Browser,
    design::{colors, tokens},
    pane_cards::Cards,
};

/// How quickly a pane closes the distance to its place, as a time constant.
const SETTLE_SECONDS: f32 = 0.085;
/// The dragged pane keeps closer to the pointer than panes do to their places.
const FOLLOW_SECONDS: f32 = 0.04;
/// The largest a pane is drawn while it is carried.
const CARRIED: (f32, f32) = (420., 280.);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn of(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: bounds.origin.x.into(),
            y: bounds.origin.y.into(),
            w: bounds.size.width.into(),
            h: bounds.size.height.into(),
        }
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    /// How far a point is from this rectangle; zero inside it.
    pub fn distance(self, x: f32, y: f32) -> f32 {
        let dx = (self.x - x).max(x - (self.x + self.w)).max(0.);
        let dy = (self.y - y).max(y - (self.y + self.h)).max(0.);
        dx.hypot(dy)
    }

    fn toward(self, target: Self, by: f32) -> Self {
        let mix = |from: f32, to: f32| from + (to - from) * by;
        Self {
            x: mix(self.x, target.x),
            y: mix(self.y, target.y),
            w: mix(self.w, target.w),
            h: mix(self.h, target.h),
        }
    }

    fn near(self, other: Self) -> bool {
        (self.x - other.x).abs() < 0.5
            && (self.y - other.y).abs() < 0.5
            && (self.w - other.w).abs() < 0.5
            && (self.h - other.h).abs() < 0.5
    }
}

/// Where each leaf of `node` lies in `area`, as the split layout places them:
/// a fixed gap between the two sides, the rest shared by the ratio.
pub(super) fn place(node: &Node, area: Rect, gap: f32, into: &mut HashMap<String, Rect>) {
    match node {
        Node::Leaf(leaf) => {
            into.insert(leaf.id.clone(), area);
        }
        Node::Split {
            direction,
            ratio,
            first,
            second,
            ..
        } => {
            let ratio = *ratio as f32;
            let (a, b) = match direction {
                Direction::Right => {
                    let free = (area.w - gap).max(0.);
                    let w = free * ratio;
                    (
                        Rect { w, ..area },
                        Rect {
                            x: area.x + w + gap,
                            w: free - w,
                            ..area
                        },
                    )
                }
                Direction::Below => {
                    let free = (area.h - gap).max(0.);
                    let h = free * ratio;
                    (
                        Rect { h, ..area },
                        Rect {
                            y: area.y + h + gap,
                            h: free - h,
                            ..area
                        },
                    )
                }
            };
            place(first, a, gap, into);
            place(second, b, gap, into);
        }
    }
}

/// The pane being carried.
struct Carried {
    leaf: String,
    pointer: Point<Pixels>,
    /// Where in the pane it was taken, as fractions of its size.
    grab: (f32, f32),
    /// Its size when it was taken.
    home: (f32, f32),
}

pub(super) struct Motion {
    /// Where each pane is drawn now, in window coordinates.
    rects: HashMap<String, Rect>,
    carried: Option<Carried>,
    /// The content area the panes share, measured as it is painted.
    area: Rc<Cell<Option<Rect>>>,
    last: Instant,
}

impl Motion {
    /// Start from where the panes are drawn now, carrying `leaf`.
    pub fn carrying(
        leaf: &str,
        bounds: &HashMap<String, Bounds<Pixels>>,
        pointer: Point<Pixels>,
    ) -> Option<Self> {
        let rects: HashMap<String, Rect> = bounds
            .iter()
            .map(|(id, bounds)| (id.clone(), Rect::of(*bounds)))
            .collect();
        let home = *rects.get(leaf)?;
        let left = rects.values().map(|r| r.x).fold(f32::INFINITY, f32::min);
        let top = rects.values().map(|r| r.y).fold(f32::INFINITY, f32::min);
        let right = rects.values().map(|r| r.x + r.w).fold(0., f32::max);
        let bottom = rects.values().map(|r| r.y + r.h).fold(0., f32::max);
        let grab = (
            ((f32::from(pointer.x) - home.x) / home.w.max(1.)).clamp(0., 1.),
            ((f32::from(pointer.y) - home.y) / home.h.max(1.)).clamp(0., 1.),
        );
        Some(Self {
            rects,
            carried: Some(Carried {
                leaf: leaf.into(),
                pointer,
                grab,
                home: (home.w, home.h),
            }),
            area: Rc::new(Cell::new(Some(Rect {
                x: left,
                y: top,
                w: right - left,
                h: bottom - top,
            }))),
            last: Instant::now(),
        })
    }

    pub fn area(&self) -> Option<Rect> {
        self.area.get()
    }

    pub fn is_carrying(&self) -> bool {
        self.carried.is_some()
    }

    pub fn follow(&mut self, pointer: Point<Pixels>) {
        if let Some(carried) = &mut self.carried {
            carried.pointer = pointer;
        }
    }

    /// Let go: the pane eases to its place like the others.
    pub fn release(&mut self) {
        self.carried = None;
    }

    fn carried_rect(carried: &Carried) -> Rect {
        let (w, h) = carried.home;
        let scale = (CARRIED.0 / w.max(1.)).min(CARRIED.1 / h.max(1.)).min(1.);
        let (w, h) = (w * scale, h * scale);
        Rect {
            x: f32::from(carried.pointer.x) - carried.grab.0 * w,
            y: f32::from(carried.pointer.y) - carried.grab.1 * h,
            w,
            h,
        }
    }

    /// Move every pane one step toward its place in `node`. Returns false
    /// once nothing is carried and every pane has arrived.
    fn step(&mut self, node: &Node) -> (HashMap<String, Rect>, bool) {
        let gap = tokens().size.pane_gap;
        let mut places = HashMap::new();
        if let Some(area) = self.area.get() {
            place(node, area, gap, &mut places);
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f32().min(0.05);
        self.last = now;
        let ease = |seconds: f32| 1. - (-elapsed / seconds).exp();
        let mut moving = self.carried.is_some();
        self.rects.retain(|id, _| places.contains_key(id));
        for (id, place) in &places {
            let (target, by) = match &self.carried {
                Some(carried) if &carried.leaf == id => {
                    (Self::carried_rect(carried), ease(FOLLOW_SECONDS))
                }
                _ => (*place, ease(SETTLE_SECONDS)),
            };
            let rect = self.rects.entry(id.clone()).or_insert(target);
            *rect = if rect.near(target) {
                target
            } else {
                moving = true;
                rect.toward(target, by)
            };
        }
        (places, moving)
    }
}

impl Browser {
    /// The tab's panes at their eased rectangles, or `None` once they have
    /// all arrived and the ordinary layout can take over.
    pub(super) fn panes_in_motion(
        motion: &mut Motion,
        node: &Node,
        cards: &Cards<'_>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (places, moving) = motion.step(node);
        if !moving {
            return None;
        }
        window.request_animation_frame();
        let (t, c) = (tokens(), colors());
        let origin = motion.area.get().map_or((0., 0.), |area| (area.x, area.y));
        let carried = motion.carried.as_ref().map(|carried| carried.leaf.clone());
        let at = |rect: Rect| {
            div()
                .absolute()
                .left(px(rect.x - origin.0))
                .top(px(rect.y - origin.1))
                .w(px(rect.w.max(1.)))
                .h(px(rect.h.max(1.)))
        };
        let mut layer = div().relative().size_full().child(
            gpui::canvas(|_, _, _| (), {
                let area = Rc::clone(&motion.area);
                move |bounds, (), _, _| area.set(Some(Rect::of(bounds)))
            })
            .absolute()
            .size_full(),
        );
        let leaves = node.leaves();
        for leaf in leaves
            .iter()
            .filter(|leaf| carried.as_deref() != Some(leaf.id.as_str()))
        {
            if let Some(rect) = motion.rects.get(&leaf.id) {
                layer = layer.child(
                    at(*rect)
                        .id(SharedString::from(format!("moving-{}", leaf.id)))
                        .child(Self::card(leaf, cards, cx)),
                );
            }
        }
        if let Some(leaf) = leaves
            .iter()
            .find(|leaf| carried.as_deref() == Some(leaf.id.as_str()))
        {
            // Where it lands if released now.
            if let Some(place) = places.get(&leaf.id) {
                let mut tint: gpui::Hsla = c.focus.into();
                tint.a = 0.12;
                layer = layer.child(
                    at(*place)
                        .rounded(px(t.radius.pane))
                        .border(px(t.size.hairline * 2.))
                        .border_color(c.focus)
                        .bg(tint),
                );
            }
            if let Some(rect) = motion.rects.get(&leaf.id) {
                layer = layer.child(
                    at(*rect)
                        .id(SharedString::from(format!("moving-{}", leaf.id)))
                        .rounded(px(t.radius.pane))
                        .opacity(0.94)
                        .shadow(vec![BoxShadow {
                            color: gpui::hsla(0., 0., 0., 0.35),
                            offset: point(px(0.), px(12.)),
                            blur_radius: px(32.),
                            spread_radius: px(0.),
                            inset: false,
                        }])
                        .child(Self::card(leaf, cards, cx)),
                );
            }
        }
        Some(layer.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_core::Layout;
    use gpui::size;

    #[test]
    fn places_follow_the_split_ratios_with_a_fixed_gap() {
        let mut model = Layout::initial("/".into());
        let first = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/".into(), true);
        let second = model.pane().unwrap().id.clone();
        model.split(Direction::Below, "/".into(), true);
        let third = model.pane().unwrap().id.clone();
        let area = Rect {
            x: 10.,
            y: 20.,
            w: 408.,
            h: 208.,
        };
        let mut places = HashMap::new();
        place(&model.tab().unwrap().tree, area, 8., &mut places);
        assert_eq!(
            places[&first],
            Rect {
                x: 10.,
                y: 20.,
                w: 200.,
                h: 208.
            }
        );
        assert_eq!(
            places[&second],
            Rect {
                x: 218.,
                y: 20.,
                w: 200.,
                h: 100.
            }
        );
        assert_eq!(
            places[&third],
            Rect {
                x: 218.,
                y: 128.,
                w: 200.,
                h: 100.
            }
        );
    }

    #[test]
    fn a_released_pane_arrives_and_the_motion_ends() {
        let mut model = Layout::initial("/".into());
        let first = model.pane().unwrap().id.clone();
        model.split(Direction::Right, "/".into(), true);
        let second = model.pane().unwrap().id.clone();
        let bounds = HashMap::from([
            (
                first.clone(),
                Bounds::new(point(px(0.), px(0.)), size(px(200.), px(200.))),
            ),
            (
                second,
                Bounds::new(point(px(208.), px(0.)), size(px(200.), px(200.))),
            ),
        ]);
        let mut motion = Motion::carrying(&first, &bounds, point(px(50.), px(10.))).unwrap();
        assert_eq!(
            motion.area(),
            Some(Rect {
                x: 0.,
                y: 0.,
                w: 408.,
                h: 200.
            })
        );
        motion.follow(point(px(300.), px(100.)));
        let tree = &model.tab().unwrap().tree;
        assert!(motion.step(tree).1);
        motion.release();
        let mut steps = 0;
        loop {
            motion.last -= std::time::Duration::from_millis(16);
            let (places, moving) = motion.step(tree);
            if !moving {
                assert_eq!(motion.rects[&first], places[&first]);
                break;
            }
            steps += 1;
            assert!(steps < 200);
        }
    }
}
