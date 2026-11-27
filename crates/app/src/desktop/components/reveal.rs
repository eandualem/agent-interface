//! Keeping a keyboard-focused control in view.
//!
//! GPUI moves focus without scrolling, so Tab can land on a control that is
//! scrolled out of sight. A view records the focus move with
//! [`Reveal::focus_moved`] and gives each control in a scrolling element a
//! [`Reveal::within`] layer; the layer of the control that took focus scrolls
//! it into view the next time it is drawn.

use gpui::{
    App, Bounds, Canvas, FocusHandle, Pixels, ScrollHandle, Styled, WeakFocusHandle, Window,
    canvas, point, px,
};
use std::{cell::RefCell, rc::Rc};

/// The control a keyboard focus move landed on, until it is in view.
#[derive(Clone, Default)]
pub struct Reveal(Rc<RefCell<Option<WeakFocusHandle>>>);

impl Reveal {
    /// Call after moving focus by keyboard: the control that now has focus
    /// scrolls into view when next drawn.
    pub fn focus_moved(&self, window: &Window, cx: &App) {
        *self.0.borrow_mut() = window.focused(cx).map(|focus| focus.downgrade());
    }

    /// Forget a focus move no layer took, as when focus went to a control
    /// outside the scrolling elements, so a later focus on a control with a
    /// layer, such as by a click, does not scroll. Call once a frame after
    /// the layers' prepaint, as from a paint callback.
    pub fn expire(&self) {
        self.0.borrow_mut().take();
    }

    /// An invisible layer for a relatively positioned control that tracks
    /// `focus`, inside the scrolling element `scroll` tracks. When a focus
    /// move lands on the control, it scrolls the least distance that shows
    /// the control.
    pub fn within(&self, focus: &FocusHandle, scroll: &ScrollHandle) -> Canvas<()> {
        let (pending, focus, scroll) = (self.0.clone(), focus.clone(), scroll.clone());
        canvas(
            move |bounds, window, _| {
                if !pending
                    .borrow()
                    .as_ref()
                    .is_some_and(|moved| *moved == focus)
                {
                    return;
                }
                pending.take();
                let offset = scroll.offset();
                let y = shown_offset(scroll.bounds(), bounds, offset.y, scroll.max_offset().y);
                if y != offset.y {
                    scroll.set_offset(point(offset.x, y));
                    // The scrolling element has placed its children for this
                    // frame already; the next one draws them moved.
                    window.request_animation_frame();
                }
            },
            |_, (), _, _| (),
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}

/// The vertical scroll offset that shows `item` in `viewport`, from the
/// current `offset`, which is zero at the top and `-max` at the bottom: the
/// item's top when it is above the viewport or taller than it, its bottom
/// when it is below, and no change when it is in view.
fn shown_offset(
    viewport: Bounds<Pixels>,
    item: Bounds<Pixels>,
    offset: Pixels,
    max: Pixels,
) -> Pixels {
    let y = if item.top() < viewport.top()
        || (item.bottom() > viewport.bottom() && item.size.height > viewport.size.height)
    {
        offset + viewport.top() - item.top()
    } else if item.bottom() > viewport.bottom() {
        offset - (item.bottom() - viewport.bottom())
    } else {
        offset
    };
    y.clamp(-max, px(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    fn rows(top: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(top)), size(px(100.0), px(height)))
    }

    #[test]
    fn a_control_focused_out_of_view_scrolls_the_least_distance_into_it() {
        // A 300 px thread at y 100, scrolled 500 px down a 1000 px range.
        let viewport = rows(100.0, 300.0);
        let (offset, max) = (px(-500.0), px(1000.0));
        // Above: its top comes to the viewport's top.
        assert_eq!(
            shown_offset(viewport, rows(40.0, 20.0), offset, max),
            px(-440.0)
        );
        // Below: its bottom comes to the viewport's bottom.
        assert_eq!(
            shown_offset(viewport, rows(420.0, 20.0), offset, max),
            px(-540.0)
        );
        // Taller than the viewport and starting below it: its top shows.
        assert_eq!(
            shown_offset(viewport, rows(380.0, 400.0), offset, max),
            px(-780.0)
        );
        // In view: nothing moves.
        assert_eq!(
            shown_offset(viewport, rows(200.0, 20.0), offset, max),
            offset
        );
        // Never past either end.
        assert_eq!(
            shown_offset(viewport, rows(-900.0, 20.0), offset, max),
            px(0.0)
        );
        assert_eq!(
            shown_offset(viewport, rows(1200.0, 20.0), offset, max),
            px(-1000.0)
        );
    }
}
