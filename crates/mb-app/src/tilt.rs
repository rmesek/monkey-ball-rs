//! Device tilt as a steering input: turns the orientation a phone reports into the `TiltX` / `TiltY` axes of
//! [`crate::input::InputState`]. Pure maths, so the browser shell only forwards
//! raw sensor angles.
//!
//! Angles are the W3C `deviceorientation` ones in degrees: `beta` is the front-to-back tilt (positive when
//! the top edge rises), `gamma` the left-to-right tilt (positive when the right edge drops). `screen_angle`
//! is how far the page is rotated from the device's natural orientation (0, 90, 180, 270).
//!
//! Steering is relative to the pose the device was held in at the last [`TiltSteering::calibrate`] (the
//! start of a stage), so the player can hold the phone however is comfortable.

use crate::input::{Axis, InputState};

/// Tilt below this many degrees from the calibrated pose steers nothing (a hand is never perfectly still,
/// and an exact zero lets keys and the stick work while tilt is on).
const DEADZONE_DEG: f32 = 1.5;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TiltSteering {
    enabled: bool,
    /// Degrees of tilt for full deflection.
    range_deg: f32,
    /// The pose that counts as level: `(beta, gamma)`.
    neutral: Option<(f32, f32)>,
    /// The latest sample.
    last: Option<(f32, f32, i32)>,
    /// Take the next sample as the neutral pose.
    calibrate_pending: bool,
}

/// Difference of two angles in degrees, wrapped to -180..=180.
fn delta(a: f32, b: f32) -> f32 {
    let d = (a - b) % 360.0;
    if d > 180.0 {
        d - 360.0
    } else if d < -180.0 {
        d + 360.0
    } else {
        d
    }
}

impl TiltSteering {
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Switch tilt steering on or off. Switching it on calibrates at the next sample.
    pub fn set_enabled(&mut self, enabled: bool, range_deg: f32) {
        if enabled && !self.enabled {
            self.calibrate_pending = true;
        }
        self.enabled = enabled;
        self.range_deg = range_deg.clamp(5.0, 45.0);
        if !enabled {
            self.neutral = None;
        }
    }

    /// Make the pose the device is in now (or at its next sample) the level one.
    pub fn calibrate(&mut self) {
        match self.last {
            Some((beta, gamma, _)) => {
                self.neutral = Some((beta, gamma));
                self.calibrate_pending = false;
            }
            None => self.calibrate_pending = true,
        }
    }

    /// A sensor sample arrived.
    pub fn sample(&mut self, beta: f32, gamma: f32, screen_angle: i32) {
        if !(beta.is_finite() && gamma.is_finite()) {
            return;
        }
        self.last = Some((beta, gamma, screen_angle.rem_euclid(360)));
        if self.calibrate_pending || self.neutral.is_none() {
            self.neutral = Some((beta, gamma));
            self.calibrate_pending = false;
        }
    }

    /// Steering vector `(x right, y forward)` in -1..=1 for the latest sample; zero when off or unknown.
    pub fn axes(&self) -> (f32, f32) {
        let (true, Some((beta, gamma, angle)), Some((b0, g0))) =
            (self.enabled, self.last, self.neutral)
        else {
            return (0.0, 0.0);
        };
        let (db, dg) = (delta(beta, b0), delta(gamma, g0));
        // Screen-right and screen-forward tilt from the device's own axes.
        let (x, y) = match angle {
            90 => (db, dg),
            180 => (-dg, db),
            270 => (-db, -dg),
            _ => (dg, -db),
        };
        let shape = |v: f32| {
            if v.abs() < DEADZONE_DEG {
                0.0
            } else {
                ((v - DEADZONE_DEG.copysign(v)) / (self.range_deg - DEADZONE_DEG)).clamp(-1.0, 1.0)
            }
        };
        (shape(x), shape(y))
    }

    /// Write the current steering into the tilt axes (zero when tilt is off, which hands steering back to
    /// the stick and keys).
    pub fn apply(&self, input: &mut InputState) {
        let (x, y) = self.axes();
        input.set_axis(Axis::TiltX, x);
        input.set_axis(Axis::TiltY, y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on(range: f32) -> TiltSteering {
        let mut t = TiltSteering::default();
        t.set_enabled(true, range);
        t
    }

    #[test]
    fn off_or_without_samples_steers_nothing() {
        let mut t = TiltSteering::default();
        t.sample(40.0, 10.0, 0);
        assert_eq!(t.axes(), (0.0, 0.0));
        assert_eq!(on(20.0).axes(), (0.0, 0.0));
    }

    #[test]
    fn steering_is_relative_to_the_calibrated_pose() {
        let mut t = on(21.5);
        // Held at 40 degrees: that is level.
        t.sample(40.0, 0.0, 0);
        assert_eq!(t.axes(), (0.0, 0.0));
        // Top edge tipped away by 10 degrees and the right edge dropped by 20: forward and right.
        t.sample(30.0, 20.0, 0);
        let (x, y) = t.axes();
        assert!((y - (10.0 - 1.5) / 20.0).abs() < 1e-5, "y {y}");
        assert!((x - (20.0 - 1.5) / 20.0).abs() < 1e-5, "x {x}");
        // Beyond the range: clamped.
        t.sample(-20.0, 80.0, 0);
        assert_eq!(t.axes(), (1.0, 1.0));
        // A new stage: the pose held now becomes level.
        t.sample(55.0, -5.0, 0);
        t.calibrate();
        assert_eq!(t.axes(), (0.0, 0.0));
        t.sample(55.0, -25.0, 0);
        assert!(t.axes().0 < -0.9 && t.axes().1 == 0.0);
    }

    #[test]
    fn small_tremor_is_ignored() {
        let mut t = on(20.0);
        t.sample(30.0, 0.0, 0);
        t.sample(31.0, -1.2, 0);
        assert_eq!(t.axes(), (0.0, 0.0));
    }

    #[test]
    fn landscape_swaps_the_axes() {
        // Landscape with the device's top edge to the left (angle 90): lifting the device's top edge
        // (beta up) drops the screen's right side, and dropping the device's right edge (gamma up) tips
        // the screen's top away.
        let mut t = on(20.0);
        t.sample(0.0, -30.0, 90);
        t.sample(10.0, -30.0, 90);
        let (x, y) = t.axes();
        assert!(x > 0.4 && y == 0.0, "{x} {y}");
        t.sample(0.0, -20.0, 90);
        let (x, y) = t.axes();
        assert!(x == 0.0 && y > 0.4, "{x} {y}");
        // The other landscape mirrors both.
        let mut t = on(20.0);
        t.sample(0.0, 30.0, 270);
        t.sample(10.0, 30.0, 270);
        assert!(t.axes().0 < -0.4);
        t.sample(0.0, 40.0, 270);
        assert!(t.axes().1 < -0.4);
    }

    #[test]
    fn writes_the_tilt_axes_and_clears_them_when_switched_off() {
        let mut input = InputState::new();
        let mut t = on(20.0);
        t.sample(0.0, 0.0, 0);
        t.sample(0.0, 21.5, 0);
        t.apply(&mut input);
        assert_eq!(input.tilt(), (1.0, 0.0));
        t.set_enabled(false, 20.0);
        t.apply(&mut input);
        assert_eq!(input.axis(Axis::TiltX), 0.0);
        // Not-a-number samples (a sensor that is not there) are ignored.
        t.set_enabled(true, 20.0);
        t.sample(f32::NAN, 0.0, 0);
        assert_eq!(t.axes(), (0.0, 0.0));
    }

    #[test]
    fn angle_differences_wrap() {
        assert_eq!(delta(170.0, -170.0), -20.0);
        assert_eq!(delta(-170.0, 170.0), 20.0);
        assert_eq!(delta(10.0, 30.0), -20.0);
    }
}
