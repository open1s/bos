//! Smooth scrolling for the centre column.
//!
//! GPUI's own scroller applies a wheel delta in one step: a mouse notch jumps the
//! content by three lines. That reads as a cut, not a scroll. `Smooth` keeps a
//! TARGET offset and walks the live offset towards it a fraction per frame, which
//! is the motion a trackpad gets for free and a wheel does not.

/// How much of the remaining distance one frame covers.
///
/// 0.6 reaches the target in three frames (~50ms): enough easing that a notch reads
/// as motion rather than a cut, and fast enough that the wheel answers the hand.
/// 0.28 was tried first and felt sluggish - this is the knob for that.
pub(crate) const EASE: f32 = 0.6;

/// Below this the target is considered reached and the animation stops, so the
/// shell stops asking for frames.
const SETTLED: f32 = 0.25;

/// One region's eased scroll.
#[derive(Default, Clone, Copy)]
pub(crate) struct Smooth {
    /// Where the offset is heading, or `None` when nothing is animating.
    target: Option<f32>,
}

impl Smooth {
    /// Aim at `offset` (GPUI's scrolled-down offset is negative).
    pub(crate) fn aim(&mut self, offset: f32) {
        self.target = Some(offset);
    }

    /// Whether a frame is still needed.
    pub(crate) fn animating(&self) -> bool {
        self.target.is_some()
    }

    /// The next offset, given the live one.
    ///
    /// Returns `None` once the target is reached, which is also when the caller
    /// stops requesting frames.
    pub(crate) fn step(&mut self, current: f32) -> Option<f32> {
        let target = self.target?;
        let remaining = target - current;
        if remaining.abs() <= SETTLED {
            self.target = None;
            return Some(target);
        }
        Some(current + remaining * EASE)
    }
}

/// What one wheel event asks for, and where it lands.
///
/// The clamp is the scroller's own range - `max` is `max_offset`, which is positive;
/// a scrolled-down offset is negative - and a wheel event never becomes a request
/// to scroll past either end.
pub(crate) fn wheel_target(current: f32, delta: f32, max: f32) -> f32 {
    (current + delta).clamp(-max, 0.0)
}

#[cfg(test)]
mod tests {
    // By name, never `use super::*`: this file imports gpui at module scope, and a
    // glob would bring gpui's own `test` attribute macro into scope - the builtin
    // then resolves back to it and the expansion recurses until rustc gives up.
    use super::{wheel_target, Smooth};

    /// An eased scroll walks towards its target instead of jumping to it, and it
    /// stops asking for frames once it arrives.
    #[test]
    fn a_smooth_scroll_eases_in_and_settles() {
        let mut smooth = Smooth::default();
        smooth.aim(-160.0);
        assert!(smooth.animating());
        let first = smooth.step(0.0).expect("a first step");
        assert!(first < 0.0, "it moves towards the target");
        assert!(first > -160.0, "but does not jump the whole way: {first}");

        let mut current = first;
        let mut frames = 1;
        while let Some(next) = smooth.step(current) {
            current = next;
            frames += 1;
            assert!(frames < 100, "the animation terminates");
        }
        assert_eq!(current, -160.0, "and lands exactly on the target");
        assert!(!smooth.animating(), "then stops asking for frames");
    }

    /// The wheel clamps at both ends of the scroller's range.
    #[test]
    fn a_wheel_never_scrolls_past_an_end() {
        assert_eq!(wheel_target(0.0, 300.0, 500.0), 0.0, "not above the top");
        assert_eq!(
            wheel_target(-500.0, -300.0, 500.0),
            -500.0,
            "not past the end"
        );
        assert_eq!(
            wheel_target(-100.0, -50.0, 500.0),
            -150.0,
            "otherwise it moves"
        );
    }
}
