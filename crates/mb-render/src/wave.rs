//! The bonus-wave vertex animation of stage 92 (`noclip/SuperMonkeyBall/BonusWave.ts`): a ripple that runs
//! outwards from the model's y axis and lifts each vertex by `sin(angle) * amplitude`. The vertex shader
//! evaluates it from five numbers in the object's uniform, so no vertex buffer is rewritten; this module
//! holds the constants and the same formula on the CPU.

use glam::Vec3;

/// The constants of the ripple. Angles are S16 units (65536 = one turn), time is in 60 Hz frames.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveParams {
    /// Amplitude on the axis (`0.5`).
    pub amplitude_base: f32,
    /// Amplitude gained per unit of distance from the axis (`-0.030833333333333333`).
    pub amplitude_slope: f32,
    /// Angle gained per frame (`-1092.0`): the phase runs down, the front runs outwards.
    pub angle_speed: f32,
    /// The frame at which the ripple starts on the axis (`30.0`).
    pub start_frame: f32,
    /// Angle gained per unit of distance from the axis (`16384.0`).
    pub angle_scale: f32,
}

impl WaveParams {
    /// The constants of `BONUS_WAVE_VERTEX_MAIN`, which are those of the collision ripple
    /// (`BONUS_WAVE_*` in `mb-engine/src/collision.rs`).
    pub const BONUS_WAVE: Self = Self {
        amplitude_base: 0.5,
        amplitude_slope: -0.030_833_333,
        angle_speed: -1092.0,
        start_frame: 30.0,
        angle_scale: 16384.0,
    };

    /// `BONUS_WAVE_ANGLE_TO_RAD`: 2 pi / 65536.
    pub const ANGLE_TO_RAD: f32 = 0.000_095_873_8;

    /// The angle on the axis at `frames` (the `w` the shader gets). Computed in f64: after a minute the
    /// product is in the millions, where f32 still resolves a quarter of an angle unit.
    pub fn phase(&self, frames: f32) -> f32 {
        (self.angle_speed as f64 * (frames as f64 - self.start_frame as f64)) as f32
    }

    /// What the vertex shader does to a model-space vertex and its normal at `frames`. Where the front
    /// has not arrived (angle above zero) both come back unchanged.
    pub fn displace(&self, position: Vec3, normal: Vec3, frames: f32) -> (Vec3, Vec3) {
        let dist = (position.x * position.x + position.z * position.z).sqrt();
        let amp = self.amplitude_base + self.amplitude_slope * dist;
        let angle = self.phase(frames) + self.angle_scale * dist;
        if angle.is_nan() || angle > 0.0 {
            return (position, normal);
        }
        let rad = angle * Self::ANGLE_TO_RAD;
        let (sin, cos) = rad.sin_cos();
        let mut out = position;
        out.y += sin * amp;
        let mut n = Vec3::Y;
        if dist > 0.0 {
            let dy_dr =
                cos * (self.angle_scale * Self::ANGLE_TO_RAD) * amp + sin * self.amplitude_slope;
            n = Vec3::new(-dy_dr * position.x / dist, 1.0, -dy_dr * position.z / dist).normalize();
        }
        (out, n)
    }

    /// The largest lift of a vertex within `reach` of the axis, for culling bounds.
    pub fn max_amplitude(&self, reach: f32) -> f32 {
        self.amplitude_base
            .abs()
            .max((self.amplitude_base + self.amplitude_slope * reach).abs())
    }

    /// `Object.wave` of shader.wgsl.
    pub(crate) fn uniform(&self, frames: f32) -> [f32; 4] {
        [
            self.amplitude_base,
            self.amplitude_slope,
            self.angle_scale,
            self.phase(frames),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: WaveParams = WaveParams::BONUS_WAVE;

    #[test]
    fn constants_are_the_reference_ones() {
        // `BonusWave.ts` and `BONUS_WAVE_*` of the collision code.
        assert_eq!(W.amplitude_base, 0.5);
        assert_eq!(W.amplitude_slope, -0.030833333333333333_f64 as f32);
        assert_eq!(W.angle_speed, -1092.0);
        assert_eq!(W.start_frame, 30.0);
        assert_eq!(W.angle_scale, 16384.0);
        assert_eq!(
            WaveParams::ANGLE_TO_RAD,
            (std::f64::consts::TAU / 65536.0) as f32
        );
        assert_eq!(WaveParams::ANGLE_TO_RAD, 0.00009587379924285257_f64 as f32);
    }

    #[test]
    fn nothing_moves_before_the_front_arrives() {
        let p = Vec3::new(3.0, 1.0, 4.0);
        let n = Vec3::new(0.0, 0.0, 1.0);
        // Before frame 30 the angle on the axis is positive, and it only grows outwards.
        assert_eq!(W.displace(p, n, 0.0), (p, n));
        assert_eq!(W.displace(Vec3::ZERO, n, 29.0), (Vec3::ZERO, n));
        // At frame 60 the front (angle zero) is 30 * 1092 / 16384 = 2 units out; this vertex is at 5.
        assert_eq!(W.displace(p, n, 60.0), (p, n));
        let (moved, normal) = W.displace(Vec3::new(1.0, 1.0, 0.0), n, 60.0);
        assert_ne!(moved.y, 1.0);
        assert!((normal.length() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn displacement_follows_the_formula() {
        // 45 frames in: angle on the axis = -1092 * 15 = -16380, almost a quarter turn down.
        let (p, n) = W.displace(Vec3::new(0.0, 2.0, 0.0), Vec3::X, 45.0);
        let rad = -16380.0f64 * std::f64::consts::TAU / 65536.0;
        assert!((p.y as f64 - (2.0 + rad.sin() * 0.5)).abs() < 1e-6);
        assert_eq!(n, Vec3::Y, "on the axis the normal is straight up");

        // Off the axis: height and slope against the analytic surface y(r) = sin(theta(r)) * amp(r).
        let frames = 200.0;
        let at = |r: f64| {
            let theta =
                (-1092.0 * (frames as f64 - 30.0) + 16384.0 * r) * std::f64::consts::TAU / 65536.0;
            theta.sin() * (0.5 - 0.030833333333333333 * r)
        };
        let v = Vec3::new(3.0, 0.25, -4.0);
        let (p, n) = W.displace(v, Vec3::Y, frames);
        assert_eq!((p.x, p.z), (v.x, v.z), "only the height changes");
        assert!((p.y as f64 - (0.25 + at(5.0))).abs() < 2e-4, "{}", p.y);
        let slope = (at(5.0 + 1e-4) - at(5.0 - 1e-4)) / 2e-4;
        // The surface normal is (-dy/dx, 1, -dy/dz) normalised, and dy/dx = slope * x / r.
        let expect = glam::DVec3::new(-slope * 3.0 / 5.0, 1.0, slope * 4.0 / 5.0).normalize();
        assert!(
            (n.as_dvec3() - expect).length() < 2e-3,
            "{n:?} vs {expect:?}"
        );
    }

    #[test]
    fn amplitude_bound_covers_the_model() {
        assert_eq!(W.max_amplitude(0.0), 0.5);
        assert_eq!(W.max_amplitude(10.0), 0.5);
        // Far out the slope takes the amplitude through zero and beyond.
        assert!((W.max_amplitude(40.0) - (0.5f32 - 0.030_833_333 * 40.0).abs()).abs() < 1e-6);
    }

    #[test]
    fn phase_keeps_its_precision_late_in_a_stage() {
        // 60 seconds: -1092 * 3570 = -3 898 440, exactly representable.
        assert_eq!(W.phase(3600.0), -3_898_440.0);
        assert_eq!(W.uniform(30.0), [0.5, W.amplitude_slope, 16384.0, 0.0]);
    }
}
