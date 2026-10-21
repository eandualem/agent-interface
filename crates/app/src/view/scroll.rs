//! Keep one wheel gesture's ownership independent of hit testing.

use gpui::TouchPhase;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct WheelGesture {
    id: u64,
    last: Option<Instant>,
    phased: bool,
    local: bool,
}

impl WheelGesture {
    pub fn update(&mut self, phase: TouchPhase, now: Instant) -> u64 {
        // Wayland currently supplies only Moved. Without phase boundaries,
        // an idle gap separates gestures. After Ended, retain brief momentum.
        if phase == TouchPhase::Started
            || self.last.is_none()
            || (!self.phased
                && self
                    .last
                    .is_some_and(|last| now.duration_since(last) > Duration::from_millis(500)))
        {
            self.id = self.id.wrapping_add(1);
            self.local = false;
        }
        match phase {
            TouchPhase::Started => self.phased = true,
            TouchPhase::Ended | TouchPhase::Cancelled => self.phased = false,
            TouchPhase::Moved => {}
        }
        self.last = Some(now);
        self.id
    }

    pub fn keep_local(&mut self, local: bool) -> bool {
        self.local |= local;
        self.local
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_survives_motion_gaps_but_not_a_new_gesture() {
        let mut gesture = WheelGesture::default();
        let now = Instant::now();
        let first = gesture.update(TouchPhase::Started, now);
        assert!(gesture.keep_local(true));
        // No pane is involved in the identity, including moves over separators.
        assert_eq!(
            gesture.update(TouchPhase::Moved, now + Duration::from_secs(2)),
            first
        );
        assert!(gesture.keep_local(false));
        gesture.update(TouchPhase::Ended, now + Duration::from_secs(2));
        assert_eq!(
            gesture.update(TouchPhase::Moved, now + Duration::from_millis(2100)),
            first
        );
        assert!(gesture.keep_local(false));
        let next = gesture.update(TouchPhase::Moved, now + Duration::from_secs(3));
        assert_ne!(next, first);
        assert!(!gesture.keep_local(false));
        assert!(gesture.keep_local(true));
        assert_ne!(
            gesture.update(TouchPhase::Started, now + Duration::from_millis(3100)),
            next
        );
        assert!(!gesture.keep_local(false));
    }
}
