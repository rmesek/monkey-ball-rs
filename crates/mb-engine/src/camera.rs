//! Port of the SMB1 gameplay paths of WebMonkeyBall `src/camera.ts`: stage fly-in ("ready"), level follow
//! camera, goal and fall-out cameras. Its yaw feeds the stick-to-tilt mapping, so it is part of the simulation.
//! Replay / spectator cameras are not included.

use crate::animation::smoothstep;
use crate::ball::Ball;
use crate::constants::{ball_flags, camera_state};
use crate::math::*;
use crate::stage_runtime::BoundSphere;

#[derive(Debug, Clone)]
pub struct GameplayCamera {
    pub eye: Vec3,
    pub look_at: Vec3,
    pub eye_vel: Vec3,
    pub look_at_vel: Vec3,
    pub rot_x: i32,
    pub rot_y: i32,
    pub rot_z: i32,
    pub flags: u32,
    pub state: i32,
    pub timer_curr: i32,
    pub timer_max: i32,
    unk54: Vec3,
    unk60: f64,
    unk64: f64,
    unk68: i32,
    unk6c: i32,
    unk70: i32,
    unk74: Vec3,
    unk88: i32,
    unk8c: i32,
    unk90: i32,
    unk_ac: Vec3,
    unk_b8: i32,
    unk10c: i32,
    stack: MatrixStack,
}

impl Default for GameplayCamera {
    fn default() -> Self {
        Self::new()
    }
}

impl GameplayCamera {
    pub fn new() -> GameplayCamera {
        GameplayCamera {
            eye: Vec3::ZERO,
            look_at: Vec3::ZERO,
            eye_vel: Vec3::ZERO,
            look_at_vel: Vec3::ZERO,
            rot_x: 0,
            rot_y: 0,
            rot_z: 0,
            flags: 0,
            state: camera_state::LEVEL_MAIN,
            timer_curr: 0,
            timer_max: 0,
            unk54: Vec3::ZERO,
            unk60: 0.0,
            unk64: 0.0,
            unk68: 0,
            unk6c: 0,
            unk70: 0,
            unk74: Vec3::ZERO,
            unk88: 0,
            unk8c: 0,
            unk90: 0,
            unk_ac: Vec3::ZERO,
            unk_b8: 0,
            unk10c: 0,
            stack: MatrixStack::new(),
        }
    }

    pub fn reset(&mut self) {
        let stack = std::mem::take(&mut self.stack);
        *self = GameplayCamera::new();
        self.stack = stack;
    }

    fn face_direction(&mut self, look_dir: &Vec3) {
        self.rot_y = atan2_s16(look_dir.x, look_dir.z) - 0x8000;
        self.rot_x = atan2_s16_safe(look_dir.y, sqrt(sum_sq2(look_dir.x, look_dir.z)));
        self.rot_z = 0;
    }

    /// `initForStage`: snap behind the ball.
    pub fn init_for_stage(&mut self, ball: &Ball, start_rot_y: i32) {
        self.reset();
        self.look_at = Vec3::new(ball.pos.x, ball.pos.y + 0.5, ball.pos.z);

        self.stack.from_translate(&self.look_at);
        self.stack.rotate_y(start_rot_y);
        self.eye = self.stack.tf_point(&Vec3::new(0.0, 1.0, 3.0));
        self.unk_ac = self.stack.tf_point(&Vec3::new(0.0, 0.0, 3.0));

        let d = Vec3::new(
            self.look_at.x - self.eye.x,
            self.look_at.y - self.eye.y,
            self.look_at.z - self.eye.z,
        );
        self.face_direction(&d);
        self.state = camera_state::LEVEL_MAIN;
    }

    /// `initReady`: the fly-in from the stage bounds to the start position.
    pub fn init_ready(
        &mut self,
        fly_in: BoundSphere,
        start_rot_y: i32,
        start_pos: Vec3,
        fly_in_frames: i32,
    ) {
        self.reset();
        self.unk54 = fly_in.pos;
        self.unk60 = fly_in.radius * 0.8;
        self.unk64 = fly_in.radius * 0.8;

        self.stack.from_translate(&self.unk54);
        self.stack.rotate_y(start_rot_y);
        let mut tmp = self.stack.tf_point(&Vec3::new(0.0, self.unk64, self.unk60));
        tmp = Vec3::new(
            self.unk54.x - tmp.x,
            self.unk54.y - tmp.y,
            self.unk54.z - tmp.z,
        );
        self.unk6c = to_s16((atan2_s16(tmp.x, tmp.z) - 0x8000) as f64);
        self.unk68 = atan2_s16_safe(tmp.y, sqrt(sum_sq2(tmp.x, tmp.z)));
        self.unk70 = 0;

        self.unk74 = Vec3::new(start_pos.x, start_pos.y + 0.5, start_pos.z);
        self.stack.from_translate(&self.unk74);
        self.stack.rotate_y(start_rot_y);
        let mut tmp = self.stack.tf_point(&Vec3::new(0.0, 1.0, 3.0));
        tmp = Vec3::new(
            self.unk74.x - tmp.x,
            self.unk74.y - tmp.y,
            self.unk74.z - tmp.z,
        );
        self.unk8c = to_s16((atan2_s16(tmp.x, tmp.z) - 0x8000) as f64) + 0x10000;
        self.unk88 = atan2_s16_safe(tmp.y, sqrt(sum_sq2(tmp.x, tmp.z)));
        self.unk90 = 0;
        self.flags |= 1;
        self.timer_curr = fly_in_frames;
        self.timer_max = fly_in_frames;
        self.state = camera_state::READY_MAIN;
        self.update_ready_main(false, false);
    }

    fn update_ready_main(&mut self, paused: bool, fast_forward: bool) {
        if paused {
            return;
        }
        if self.timer_curr > 0 {
            self.timer_curr -= 1;
            if fast_forward {
                self.timer_curr -= 1;
            }
        }

        let denom = if self.timer_max == 0 {
            1
        } else {
            self.timer_max
        };
        let mut t = self.timer_curr as f64 / denom as f64;
        t = smoothstep(t);

        self.look_at.x = self.unk74.x * (1.0 - t) + self.unk54.x * t;
        self.look_at.y = self.unk74.y * (1.0 - t) + self.unk54.y * t;
        self.look_at.z = self.unk74.z * (1.0 - t) + self.unk54.z * t;

        let delta_x = to_s16((self.unk88 - self.unk68) as f64);
        let delta_z = to_s16((self.unk90 - self.unk70) as f64);
        self.rot_x = to_s16(self.unk68 as f64 + delta_x as f64 * (1.0 - t));
        self.rot_y = to_s16(self.unk6c as f64 + (self.unk8c - self.unk6c) as f64 * (1.0 - t));
        self.rot_z = to_s16(self.unk70 as f64 + delta_z as f64 * (1.0 - t));

        self.stack.from_translate(&self.look_at);
        self.stack.rotate_y(self.rot_y);
        let p = Vec3::new(
            0.0,
            (1.0 - t) + self.unk64 * t,
            (1.0 - t) * 3.0 + self.unk60 * t,
        );
        self.eye = self.stack.tf_point(&p);
    }

    #[allow(clippy::if_same_then_else)]
    fn update_level_main(&mut self, ball: &Ball, paused: bool) {
        if paused {
            return;
        }
        let prev_eye = self.eye;
        let prev_look_at = self.look_at;

        let mut tmp = Vec3::new(
            self.unk_ac.x - self.look_at.x,
            self.unk_ac.y - self.look_at.y,
            self.unk_ac.z - self.look_at.z,
        );
        let mut f1 = sum_sq3(tmp.x, tmp.y, tmp.z);
        if f1 > 1e-7 {
            f1 = rsqrt(f1);
            tmp.x *= f1;
            tmp.y *= f1;
            tmp.z *= f1;
        } else {
            tmp = Vec3::new(1.0, 0.0, 0.0);
        }

        tmp.x = tmp.x * 0.75 + self.look_at.x;
        tmp.y = tmp.y * 0.75 + self.look_at.y;
        tmp.z = tmp.z * 0.75 + self.look_at.z;

        self.look_at = Vec3::new(ball.pos.x, ball.pos.y + 0.5, ball.pos.z);

        tmp.x = self.look_at.x - tmp.x;
        tmp.y = self.look_at.y - tmp.y;
        tmp.z = self.look_at.z - tmp.z;

        let mut pitch = 0;
        if ball.unk80 >= 60 {
            pitch = atan2_s16_safe(tmp.y, sqrt(sum_sq2(tmp.x, tmp.z)));
        }

        let mut yaw = to_s16((atan2_s16(tmp.x, tmp.z) - 0x8000) as f64);
        let mut delta_yaw = to_s16((yaw - self.rot_y) as f64);
        yaw = to_s16((self.rot_y + delta_yaw.clamp(-512, 512)) as f64);

        if self.flags & (1 << 1) == 0 && ball.flags & ball_flags::GOAL == 0 {
            let mut steer = to_s16((ball.unk92 - yaw) as f64);
            if steer > 0x800 {
                steer -= 0x800;
            } else if steer < -0x800 {
                steer += 0x800;
            } else {
                steer = 0;
            }
            steer >>= 7;
            let mut steer_vel = self.unk10c;
            if steer == 0 {
                steer_vel = 0;
            } else if (steer_vel < 0 && steer > 0) || (steer_vel > 0 && steer < 0) {
                steer_vel = 0;
            } else if steer < 0 {
                if steer < steer_vel - 4 {
                    steer_vel -= 4;
                } else {
                    steer_vel = steer;
                }
            } else if steer > steer_vel + 4 {
                steer_vel += 4;
            } else {
                steer_vel = steer;
            }

            yaw = to_s16((yaw + steer_vel) as f64);
            delta_yaw = to_s16((yaw - self.rot_y) as f64).clamp(-768, 768);
            yaw = to_s16((self.rot_y + delta_yaw) as f64);
        }

        pitch = pitch.clamp(-6144, 6144);
        let pitch_smoothed = to_s16(self.unk_b8 as f64 + 0.2 * (pitch - self.unk_b8) as f64);
        self.unk_b8 = pitch_smoothed;

        self.stack.from_translate(&self.look_at);
        self.stack.rotate_y(yaw);
        self.stack.rotate_x(pitch_smoothed);
        self.unk_ac = self.stack.tf_point(&Vec3::new(0.0, 0.0, 3.0));

        self.unk10c = to_s16((yaw - self.rot_y) as f64);
        self.rot_y = yaw;
        self.rot_x = to_s16((pitch_smoothed + 62208) as f64);

        self.stack.from_translate(&self.look_at);
        self.stack.rotate_y(self.rot_y);
        self.stack.rotate_x(self.rot_x);
        self.eye = self
            .stack
            .tf_point(&Vec3::new(0.0, 0.0, sqrt(sum_sq2(3.0, 1.0))));

        self.eye_vel = Vec3::new(
            self.eye.x - prev_eye.x,
            self.eye.y - prev_eye.y,
            self.eye.z - prev_eye.z,
        );
        self.look_at_vel = Vec3::new(
            self.look_at.x - prev_look_at.x,
            self.look_at.y - prev_look_at.y,
            self.look_at.z - prev_look_at.z,
        );
    }

    /// `setFalloutMain`
    pub fn set_fallout_main(&mut self, ball: &Ball) {
        self.state = camera_state::FALLOUT_MAIN;
        self.flags |= 1;
        self.look_at = ball.pos;
        let d = Vec3::new(
            self.look_at.x - self.eye.x,
            self.look_at.y - self.eye.y,
            self.look_at.z - self.eye.z,
        );
        self.face_direction(&d);
    }

    /// `setGoalMain`
    pub fn set_goal_main(&mut self) {
        self.state = camera_state::GOAL_MAIN;
    }

    fn update_fallout_main(&mut self, ball: &Ball, paused: bool) {
        if paused {
            return;
        }
        self.eye_vel.x *= 0.97;
        self.eye_vel.y *= 0.955;
        self.eye_vel.z *= 0.97;

        self.eye.x += self.eye_vel.x;
        self.eye.y += self.eye_vel.y;
        self.eye.z += self.eye_vel.z;

        self.look_at = ball.pos;
        let d = Vec3::new(
            self.look_at.x - self.eye.x,
            self.look_at.y - self.eye.y,
            self.look_at.z - self.eye.z,
        );
        self.face_direction(&d);
    }

    fn update_goal_main(&mut self, ball: &Ball, paused: bool) {
        if paused {
            return;
        }
        let mut tmp = Vec3::new(ball.pos.x - self.eye.x, 0.0, ball.pos.z - self.eye.z);

        let dot = tmp.x * self.eye_vel.x + tmp.y * self.eye_vel.y + tmp.z * self.eye_vel.z;
        let dir_len = sum_sq2(tmp.x, tmp.z);
        if dir_len > 0.0 {
            let inv_len = rsqrt(dir_len);
            tmp.x *= inv_len;
            tmp.z *= inv_len;
        }

        self.eye_vel.y *= 0.97;
        let mut f2 = -0.01 * dot;
        self.eye_vel.x += f2 * tmp.x;
        self.eye_vel.z += f2 * tmp.z;
        if f2 < 0.0 {
            f2 *= 0.5;
            self.eye_vel.x += f2 * tmp.z;
            self.eye_vel.z += f2 * tmp.x;
        }

        self.eye.x += self.eye_vel.x;
        self.eye.y += self.eye_vel.y;
        self.eye.z += self.eye_vel.z;

        self.look_at_vel.x = 0.3 * (ball.pos.x - self.look_at.x);
        self.look_at_vel.y = 0.3 * (ball.pos.y - self.look_at.y);
        self.look_at_vel.z = 0.3 * (ball.pos.z - self.look_at.z);

        self.look_at.x += self.look_at_vel.x;
        self.look_at.y += self.look_at_vel.y;
        self.look_at.z += self.look_at_vel.z;

        tmp.x = self.eye.x - self.look_at.x;
        tmp.y = 0.0;
        tmp.z = self.eye.z - self.look_at.z;
        let dist = sqrt(sum_sq2(tmp.x, tmp.z));
        if dist > 1e-7 {
            let f3 = (dist + 0.08 * (2.0 - dist)) / dist;
            self.eye.x = tmp.x * f3 + self.look_at.x;
            self.eye.z = tmp.z * f3 + self.look_at.z;
            if ball.flags & ball_flags::FLAG_09 == 0 {
                tmp.y = self.look_at.y - self.eye.y;
                self.eye.y += 0.01 * tmp.y;
            }
        }

        let d = Vec3::new(
            self.look_at.x - self.eye.x,
            self.look_at.y - self.eye.y,
            self.look_at.z - self.eye.z,
        );
        self.face_direction(&d);
    }

    /// `update`
    pub fn update(&mut self, ball: &Ball, paused: bool, fast_forward_intro: bool) {
        match self.state {
            camera_state::FALLOUT_MAIN => self.update_fallout_main(ball, paused),
            camera_state::READY_MAIN => self.update_ready_main(paused, fast_forward_intro),
            camera_state::GOAL_MAIN => self.update_goal_main(ball, paused),
            _ => self.update_level_main(ball, paused),
        }
    }
}
