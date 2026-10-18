//! When the TUI redraws. A frame is drawn when something changed: a key or a
//! resize, a message from the run, a refreshed build or picker state, a new
//! status notice. While something visible animates (a live run's elapsed
//! time and metrics) frames come at [`ANIMATION_INTERVAL`]. An idle screen is
//! not redrawn at all, and the frame after an animation stops is drawn once so
//! the last state is on screen.

use std::time::{Duration, Instant};

/// How often a frame is drawn while something on screen animates: 4 Hz.
pub(crate) const ANIMATION_INTERVAL: Duration = Duration::from_millis(250);

/// The redraw policy: whether a frame is due, and how long the loop may wait
/// for input before one is.
#[derive(Debug, Default)]
pub(crate) struct Redraw {
    /// A change happened since the last frame.
    pending: bool,
    /// When the last frame was drawn; `None` before the first.
    last_draw: Option<Instant>,
    /// Whether the last frame was drawn while something animated.
    animated_last_frame: bool,
}

impl Redraw {
    /// Records a change that the next frame must show.
    pub(crate) fn mark(&mut self) {
        self.pending = true;
    }

    /// Whether a frame is due at `now`, given whether something animates.
    pub(crate) fn due(&self, now: Instant, animating: bool) -> bool {
        let Some(last) = self.last_draw else {
            return true;
        };
        if self.pending {
            return true;
        }
        if animating {
            return now.duration_since(last) >= ANIMATION_INTERVAL;
        }
        // The frame after an animation stops shows its final state.
        self.animated_last_frame
    }

    /// How long the loop may wait for input before a frame is due. Only an
    /// animation makes the wait finite: an idle screen waits for input.
    pub(crate) fn wait(&self, now: Instant, animating: bool) -> Option<Duration> {
        if !animating || self.pending {
            return self.pending.then_some(Duration::ZERO);
        }
        let Some(last) = self.last_draw else {
            return Some(Duration::ZERO);
        };
        Some(ANIMATION_INTERVAL.saturating_sub(now.duration_since(last)))
    }

    /// Records that a frame was drawn at `now`.
    pub(crate) fn drew(&mut self, now: Instant, animating: bool) {
        self.pending = false;
        self.last_draw = Some(now);
        self.animated_last_frame = animating;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    #[test]
    fn the_first_frame_is_always_due() {
        let redraw = Redraw::default();
        assert!(redraw.due(Instant::now(), false));
    }

    #[test]
    fn an_idle_screen_is_not_redrawn_without_a_change() {
        let base = Instant::now();
        let mut redraw = Redraw::default();
        redraw.drew(base, false);
        for millis in [100, 1_000, 60_000] {
            assert!(!redraw.due(at(base, millis), false), "at {millis} ms");
        }
        assert_eq!(redraw.wait(at(base, 100), false), None);
    }

    #[test]
    fn a_change_makes_a_frame_due_at_once() {
        let base = Instant::now();
        let mut redraw = Redraw::default();
        redraw.drew(base, false);
        redraw.mark();
        assert!(redraw.due(at(base, 1), false));
        assert_eq!(redraw.wait(at(base, 1), false), Some(Duration::ZERO));
        redraw.drew(at(base, 1), false);
        assert!(!redraw.due(at(base, 2), false));
    }

    #[test]
    fn an_animation_draws_at_four_hertz() {
        let base = Instant::now();
        let mut redraw = Redraw::default();
        redraw.drew(base, true);
        assert!(!redraw.due(at(base, 100), true));
        assert!(redraw.due(at(base, 250), true));
        assert_eq!(
            redraw.wait(at(base, 100), true),
            Some(Duration::from_millis(150))
        );
    }

    #[test]
    fn the_frame_after_an_animation_stops_is_drawn_once() {
        let base = Instant::now();
        let mut redraw = Redraw::default();
        redraw.drew(base, true);
        assert!(redraw.due(at(base, 10), false));
        redraw.drew(at(base, 10), false);
        assert!(!redraw.due(at(base, 20), false));
    }
}
