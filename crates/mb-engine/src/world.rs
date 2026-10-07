//! Port of `src/world.ts`: stage tilt from the stick and camera yaw.

use crate::math::*;

#[derive(Debug, Clone)]
pub struct World {
    pub max_tilt: f64,
    pub xrot: i32,
    pub zrot: i32,
    pub xrot_prev: i32,
    pub zrot_prev: i32,
    pub gravity: Vec3,
    stack: MatrixStack,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> World {
        World {
            max_tilt: 23.0,
            xrot: 0,
            zrot: 0,
            xrot_prev: 0,
            zrot_prev: 0,
            gravity: Vec3::new(0.0, -1.0, 0.0),
            stack: MatrixStack::new(),
        }
    }

    pub fn reset(&mut self) {
        self.xrot = 0;
        self.zrot = 0;
        self.xrot_prev = 0;
        self.zrot_prev = 0;
        self.gravity = Vec3::new(0.0, -1.0, 0.0);
    }

    /// `updateInput(stick, cameraRotY)`; the stick is in -1..=1 (dequantized).
    pub fn update_input(&mut self, stick_x: f64, stick_y: f64, camera_rot_y: i32) {
        self.xrot_prev = self.xrot;
        self.zrot_prev = self.zrot;

        let stick_x = stick_x.clamp(-1.0, 1.0);
        let stick_y = stick_y.clamp(-1.0, 1.0);

        let max_tilt_s16 = self.max_tilt * DEG_TO_S16;
        let mut inp_x_rot = to_s16(stick_y * max_tilt_s16);
        let mut inp_z_rot = to_s16(-stick_x * max_tilt_s16);

        let stack = &mut self.stack;
        stack.from_identity();
        stack.rotate_y(camera_rot_y);
        stack.rotate_x(inp_x_rot);
        stack.rotate_z(inp_z_rot);

        let up = stack.tf_vec(&Vec3::new(0.0, 1.0, 0.0));

        inp_x_rot = atan2_s16(up.z, up.y);
        inp_z_rot = -atan2_s16(up.x, sqrt(sum_sq2(up.z, up.y)));

        let dx = to_s16((inp_x_rot - self.xrot) as f64);
        let dz = to_s16((inp_z_rot - self.zrot) as f64);
        self.xrot = to_s16(self.xrot as f64 + dx as f64 * 0.2);
        self.zrot = to_s16(self.zrot as f64 + dz as f64 * 0.2);

        stack.from_identity();
        stack.rotate_x(self.xrot);
        stack.rotate_z(self.zrot);
        self.gravity = stack.rigid_inv_tf_vec(&Vec3::new(0.0, -1.0, 0.0));
    }
}
