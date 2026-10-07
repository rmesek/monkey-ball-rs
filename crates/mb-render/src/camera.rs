//! Camera maths (right-handed, -Z forward, depth range 0..1 for wgpu).

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    /// World -> view.
    pub view: Mat4,
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
    /// Depth range used for the background pass (backdrops are huge and distant).
    pub bg_near: f32,
    pub bg_far: f32,
}

impl Camera {
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Self {
        Self {
            view: Mat4::look_at_rh(eye, target, up),
            fov_y: 60f32.to_radians(),
            near: 0.1,
            far: 3000.0,
            bg_near: 0.5,
            bg_far: 100_000.0,
        }
    }

    /// From a free-fly pose: yaw about +Y (0 looks down -Z), pitch about the camera's X (positive looks up).
    pub fn from_pose(eye: Vec3, yaw: f32, pitch: f32) -> Self {
        let rot = Mat4::from_rotation_y(yaw) * Mat4::from_rotation_x(pitch);
        let world_from_view = Mat4::from_translation(eye) * rot;
        Self {
            view: world_from_view.inverse(),
            ..Self::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y)
        }
    }

    pub fn position(&self) -> Vec3 {
        self.view.inverse().w_axis.truncate()
    }

    pub fn projection(&self, aspect: f32, near: f32, far: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect.max(1e-4), near, far)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn look_at_origin_forward_is_identity() {
        let c = Camera::look_at(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        assert!(c.view.abs_diff_eq(Mat4::IDENTITY, 1e-6));
    }

    #[test]
    fn from_pose_round_trips_position() {
        let eye = Vec3::new(3.0, 4.0, -5.0);
        let c = Camera::from_pose(eye, 0.7, -0.3);
        assert!((c.position() - eye).length() < 1e-4);
    }

    #[test]
    fn point_in_front_has_negative_view_z_and_valid_depth() {
        let c = Camera::from_pose(Vec3::new(0.0, 0.0, 10.0), 0.0, 0.0);
        let p = c.projection(1.0, c.near, c.far) * c.view * Vec3::ZERO.extend(1.0);
        let ndc_z = p.z / p.w;
        assert!(p.w > 0.0 && (0.0..=1.0).contains(&ndc_z));
    }
}
