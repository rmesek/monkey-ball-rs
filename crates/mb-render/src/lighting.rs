//! Scene lighting, mirroring `noclip/SuperMonkeyBall/Lighting.ts`: one ambient colour plus one infinite
//! directional light whose direction is given as two S16 rotations (X then Y) of the vector `(0, 0, -1)`.

use glam::{Mat3, Vec3};

pub const S16_TO_RADIANS: f32 = std::f32::consts::PI / 32768.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    pub ambient: [f32; 3],
    pub light_color: [f32; 3],
    /// S16 angle units (65536 = full turn), as stored in the reference `BgInfo`.
    pub light_rot_x: i32,
    pub light_rot_y: i32,
    /// Scale applied to specular konst colours of world-specular layers (reserved).
    pub specular_scale: [f32; 3],
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            ambient: [0.6, 0.6, 0.6],
            light_color: [1.0, 1.0, 1.0],
            light_rot_x: 8192,
            light_rot_y: 24576,
            specular_scale: [1.0, 1.0, 1.0],
        }
    }
}

impl Lighting {
    /// Unit vector from a surface towards the light, in world space.
    pub fn direction_to_light(&self) -> Vec3 {
        let v = Vec3::new(0.0, 0.0, -1.0);
        let rx = Mat3::from_rotation_x(self.light_rot_x as f32 * S16_TO_RADIANS);
        let ry = Mat3::from_rotation_y(self.light_rot_y as f32 * S16_TO_RADIANS);
        (ry * (rx * v)).normalize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_rotation_points_down_negative_z() {
        let l = Lighting {
            light_rot_x: 0,
            light_rot_y: 0,
            ..Lighting::default()
        };
        assert!((l.direction_to_light() - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-6);
    }

    #[test]
    fn default_light_is_unit_length_and_above_the_horizon() {
        let d = Lighting::default().direction_to_light();
        assert!((d.length() - 1.0).abs() < 1e-5);
        // rotX = 45 degrees about +X turns -Z towards +Y.
        assert!(d.y > 0.0);
    }
}
