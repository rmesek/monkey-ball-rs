//! Port of WebMonkeyBall `src/physics.ts` (SMB1): ball state, `stepBall`, goal entry test and the ape
//! (monkey) orientation bookkeeping that feeds the camera steering yaw and the sim hash.

use crate::collision::{Collider, PhysBall, TriggerRect};
use crate::constants::*;
use crate::geom::StageGeom;
use crate::math::*;
use crate::stage_runtime::StageRuntime;
use crate::world::World;

const RAD_TO_S16: f64 = 0x8000 as f64 / std::f64::consts::PI;

/// Result of a successful [`check_ball_entered_goal`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalHit {
    pub goal_id: usize,
    pub anim_group_id: usize,
    pub goal_type: mb_stage::GoalType,
}

#[derive(Debug, Clone)]
pub struct Ball {
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    pub rot_x: i32,
    pub rot_y: i32,
    pub rot_z: i32,
    pub flags: u32,
    pub state: i32,
    pub start_pos: Vec3,
    pub start_rot_y: i32,
    pub goal_timer: i32,
    pub curr_radius: f64,
    pub accel: f64,
    pub restitution: f64,
    pub unk60: f64,
    pub unk62: f64,
    pub unk64: f64,
    pub unk80: i32,
    /// Steering yaw used by the gameplay camera.
    pub unk92: i32,
    pub ape_yaw: i32,
    pub unk_a8: Quat,
    pub unk_b8: Vec3,
    pub unk_c4: f64,
    pub unk_f8: f64,
    pub ape_quat: Quat,
    pub ape_flags: u32,
    pub transform: Mtx,
    pub prev_transform: Mtx,
    pub unk114: Vec3,
    pub delta_quat: Quat,
    pub orientation: Quat,
    pub prev_orientation: Quat,
    pub speed: f64,
    pub bananas: i32,
    pub phys_ball: PhysBall,
    /// What the last [`Ball::step`] hit, for sound triggers (`ball.audio` in the reference).
    pub audio: BallAudio,
}

/// The reference's `ball.audio` collision record: written by [`Ball::step`], read by the audio layer.
/// Output only: the simulation never reads it and it is not part of the state hash.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BallAudio {
    /// `stageColiFlags | physBall.flags` of the last step (`COLI_OCCURRED` = touching something).
    pub last_coli_flags: u32,
    /// Hardest collision speed along the surface normal in the last step (negative = into the surface).
    pub last_coli_speed: f64,
    /// Bumpers hit in the last step.
    pub bumper_hits: u32,
}

impl Default for Ball {
    fn default() -> Self {
        Self::new()
    }
}

/// Scratch matrix stack + collision stack used by the physics routines (module-level in the reference).
#[derive(Debug, Clone, Default)]
pub struct PhysicsCtx {
    pub stack: MatrixStack,
    pub collider: Collider,
}

impl PhysicsCtx {
    pub fn new() -> PhysicsCtx {
        PhysicsCtx {
            stack: MatrixStack::new(),
            collider: Collider::new(),
        }
    }
}

fn init_ape_yaw(start_rot_y: i32) -> i32 {
    wrap_s16(start_rot_y - 0x4000)
}

impl Ball {
    pub fn new() -> Ball {
        Ball {
            pos: Vec3::ZERO,
            prev_pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            rot_x: 0,
            rot_y: 0,
            rot_z: 0,
            flags: 0,
            state: ball_states::READY,
            start_pos: Vec3::ZERO,
            start_rot_y: 0,
            goal_timer: 0,
            curr_radius: 0.5,
            accel: 0.009799992,
            restitution: 0.5,
            unk60: 0.0,
            unk62: 0.0,
            unk64: 0.0,
            unk80: 0,
            unk92: 0,
            ape_yaw: 0,
            unk_a8: Quat::IDENTITY,
            unk_b8: Vec3::ZERO,
            unk_c4: 0.0,
            unk_f8: 0.0,
            ape_quat: Quat::IDENTITY,
            ape_flags: 0,
            transform: [0.0; 12],
            prev_transform: [0.0; 12],
            unk114: Vec3::new(0.0, 1.0, 0.0),
            delta_quat: Quat::IDENTITY,
            orientation: Quat::IDENTITY,
            prev_orientation: Quat::IDENTITY,
            speed: 0.0,
            bananas: 0,
            phys_ball: PhysBall::new(),
            audio: BallAudio::default(),
        }
    }

    fn reset_common(&mut self, ctx: &mut PhysicsCtx) {
        let init_ape = init_ape_yaw(self.start_rot_y);
        self.unk80 = 0;
        self.unk92 = 0;
        self.ape_yaw = init_ape;
        self.unk_a8 = Quat::IDENTITY;
        self.unk_b8 = Vec3::ZERO;
        self.unk_c4 = 0.0;
        self.unk_f8 = 0.0;
        self.set_ape_quat_from_yaw(init_ape, &mut ctx.stack);
        self.ape_flags = 0;
        self.unk114 = Vec3::new(0.0, 1.0, 0.0);
        self.delta_quat = Quat::IDENTITY;
        self.orientation = Quat::IDENTITY;
        self.prev_orientation = Quat::IDENTITY;
        self.update_transform(&mut ctx.stack);
        self.prev_transform = self.transform;
    }

    /// `initBallForStage`: invisible, positioned above the start so it can drop in after the intro.
    pub fn init_for_stage(&mut self, start_pos: Vec3, start_rot_y: i32, ctx: &mut PhysicsCtx) {
        self.start_pos = start_pos;
        self.start_rot_y = start_rot_y;
        self.flags = ball_flags::INVISIBLE;
        self.state = ball_states::READY;
        self.goal_timer = 0;
        self.vel = Vec3::ZERO;
        self.rot_x = 0;
        self.rot_y = 0;
        self.rot_z = 0;
        let drop_frames = 24.0;
        let drop_offset = ((self.accel * drop_frames) * drop_frames) * 0.5;
        self.pos = Vec3::new(start_pos.x, start_pos.y + drop_offset, start_pos.z);
        self.prev_pos = self.pos;
        self.reset_common(ctx);
    }

    /// `resetBall`: put the ball straight on the start position in the playing state.
    pub fn reset(&mut self, start_pos: Vec3, start_rot_y: i32, ctx: &mut PhysicsCtx) {
        self.pos = start_pos;
        self.prev_pos = start_pos;
        self.vel = Vec3::ZERO;
        self.rot_x = 0;
        self.rot_y = 0;
        self.rot_z = 0;
        self.flags = 0;
        self.state = ball_states::PLAY;
        self.goal_timer = 0;
        self.start_rot_y = start_rot_y;
        self.reset_common(ctx);
    }

    /// `startBallDrop`
    pub fn start_drop(&mut self, frames: i32, ctx: &mut PhysicsCtx) {
        let f4 = frames as f64;
        if f4 <= 0.0 {
            return;
        }
        self.prev_pos = self.pos;
        let f2 = (self.start_pos.y - self.pos.y) / f4;
        self.vel = Vec3::new(0.0, (self.accel * f4) * 0.5 + f2, 0.0);

        self.rot_x = 0x2000;
        self.rot_y = wrap_s16(self.start_rot_y - 0x4000);
        self.rot_z = 0;

        let stack = &mut ctx.stack;
        stack.from_identity();
        stack.rotate_y(self.rot_y);
        stack.rotate_x(self.rot_x);
        stack.rotate_z(self.rot_z);
        self.orientation = stack.to_quat();

        self.flags &= !ball_flags::INVISIBLE;
        self.flags |= ball_flags::FLAG_14;
        self.state = ball_states::PLAY;
        let init_ape = init_ape_yaw(self.start_rot_y);
        self.unk80 = 0;
        self.unk92 = 0;
        self.ape_yaw = init_ape;
        self.set_ape_quat_from_yaw(init_ape, stack);
        self.ape_flags = 0;
        self.update_transform(stack);
        self.prev_transform = self.transform;
    }

    fn update_transform(&mut self, stack: &mut MatrixStack) {
        stack.from_quat(&self.orientation);
        stack.set_translate(&self.pos);
        stack.to_mtx(&mut self.transform);
    }

    fn set_ape_quat_from_yaw(&mut self, yaw: i32, stack: &mut MatrixStack) {
        stack.from_identity();
        stack.rotate_y(wrap_s16(yaw));
        self.ape_quat = stack.to_quat();
    }

    /// `startGoal`
    pub fn start_goal(&mut self) {
        self.state = ball_states::GOAL_MAIN;
        self.flags |= ball_flags::GOAL | ball_flags::FLAG_08 | ball_flags::FLAG_10;
        if self.flags & (ball_flags::GOAL | ball_flags::FLAG_13) != 0 {
            self.flags |= ball_flags::FLAG_06;
        }
        self.delta_quat = Quat::IDENTITY;
        self.orientation = Quat::IDENTITY;
        self.goal_timer = 0;
    }

    fn init_phys_ball(&mut self) {
        let p = &mut self.phys_ball;
        p.flags = 0;
        p.pos = self.pos;
        p.prev_pos = self.prev_pos;
        p.vel = self.vel;
        p.radius = self.curr_radius;
        p.gravity_accel = self.accel;
        p.restitution = self.restitution;
        p.hardest_coli_speed = 0.0;
        p.anim_group_id = 0;
        p.hardest_coli_anim_group_id = 0;
        p.friction = 0.01;
    }

    fn sync_from_phys_ball(&mut self) {
        let p = &self.phys_ball;
        if p.flags & COLI_OCCURRED != 0 {
            self.flags |= ball_flags::FLAG_00;
        }
        self.pos = p.pos;
        self.vel = p.vel;
    }

    fn update_ball_camera_steer_yaw(&mut self) {
        let speed = sqrt(sum_sq2(self.vel.x, self.vel.z));
        let mut vel_yaw = 0;
        if speed > FLT_EPSILON {
            vel_yaw = atan2_s16(self.vel.x, self.vel.z) - 0x8000;
        }
        let mut blend = 0.0;
        if speed >= 0.37037037037037035 {
            blend = 1.0;
        } else if speed >= 0.23148148148148145 {
            blend = (speed - 0.23148148148148145) / 0.1388888888888889;
        }
        let delta = to_s16((vel_yaw - self.ape_yaw) as f64);
        self.unk92 = to_s16(self.ape_yaw as f64 + delta as f64 * blend);
    }

    fn update_ape_basis(&mut self, stack: &mut MatrixStack) {
        stack.from_mtx(&self.transform);
        self.unk_c4 = vec_len(&self.unk_b8);
        let mut sp58 = stack.tf_vec(&Vec3::new(0.0, 1.0, 0.0));
        let sp40 = stack.tf_vec(&Vec3::new(1.0, 0.0, 0.0));
        let sp28 = stack.tf_vec(&Vec3::new(0.0, 0.0, 1.0));
        let sp1c = stack.rigid_inv_tf_vec(&Vec3::new(0.0, -self.curr_radius, 0.0));

        stack.from_mtx(&self.prev_transform);
        self.unk_b8 = stack.tf_vec(&sp1c);
        self.unk_b8.y += self.curr_radius;
        let mut sp64 = stack.tf_vec(&Vec3::new(0.0, 1.0, 0.0));
        let sp4c = stack.tf_vec(&Vec3::new(1.0, 0.0, 0.0));
        let sp34 = stack.tf_vec(&Vec3::new(0.0, 0.0, 1.0));

        let mut f31 = vec_dot_normalized(&sp64, &sp58);
        let mut f1 = vec_dot_normalized(&sp4c, &sp40);
        if f31 > f1 {
            f31 = f1;
            sp64 = sp4c;
            sp58 = sp40;
        }
        f1 = vec_dot_normalized(&sp34, &sp28);
        if f31 > f1 {
            f31 = f1;
            sp64 = sp34;
            sp58 = sp28;
        }

        if f31 > -0.9998 && f31 < 0.9998 {
            self.unk_a8 = quat_from_dirs(&sp64, &sp58);
        } else {
            self.unk_a8 = Quat::IDENTITY;
        }
    }

    #[allow(clippy::if_same_then_else)]
    fn update_ape_base_orientation(&mut self, stack: &mut MatrixStack) {
        let mut tmp = self.unk_a8;
        tmp.w /= 0.65;
        tmp.x *= 0.65;
        tmp.y *= 0.65;
        tmp.z *= 0.65;
        quat_normalize(&mut tmp);

        stack.from_quat(&tmp);
        stack.b = stack.a;
        stack.from_quat(&self.ape_quat);
        stack.normalize_basis();
        let b = stack.b;
        stack.mult_left(&b);
        stack.normalize_basis();

        if self.flags & ball_flags::FLAG_05 == 0 {
            let f31 = vec_len(&self.vel);
            let f1 = vec_len(&self.unk_b8);
            if f31 > 0.032407406717538834 {
                if f1 * 100.0 < self.unk_c4 {
                    self.flags |= ball_flags::FLAG_05;
                } else if f1 * 3.0 < self.unk_c4 && f31 * 1.5 < self.unk_f8 {
                    self.flags |= ball_flags::FLAG_05;
                }
            }
        }

        if tmp.w < 0.9941 {
            return;
        }

        stack.b = stack.a;
        let mut sp48 = stack.rigid_inv_tf_vec(&Vec3::new(0.0, 1.0, 0.0));
        let sp3c = Vec3::new(0.0, 1.0, 0.0);
        let f1 = 1.0 - vec_dot_normalized(&sp48, &sp3c);
        let mut quat = if f1 > 0.01 {
            let sp30 = Vec3::new(0.0, 1.0, 0.0);
            if f1 > 1.999 {
                sp48 = Vec3::new(1.0, 0.0, 0.0);
            } else {
                sp48 = vec_cross(&sp30, &sp48);
            }
            quat_from_axis_angle(&sp48, 0x38e as f64)
        } else {
            quat_from_dirs(&Vec3::new(0.0, 1.0, 0.0), &sp48)
        };
        quat_normalize(&mut quat);
        stack.from_quat(&quat);
        stack.normalize_basis();
        let b = stack.b;
        stack.mult_left(&b);
    }

    fn update_ape_from_velocity(&mut self, stack: &mut MatrixStack) {
        let mut sp4c = Vec3::new(self.vel.x, 0.0, self.vel.z);
        if vec_len(&sp4c) < 0.00027777777 {
            return;
        }
        vec_normalize_len(&mut sp4c);
        sp4c = stack.rigid_inv_tf_vec(&sp4c);

        let forward_basis = Vec3::new(-1.0, 0.0, 0.0);
        let mut var1 = sp4c.x;
        let quat = if var1 > -0.992 {
            var1 = 1.0 - var1;
            let sp40 = if var1 > 9.99999993922529e-09 {
                vec_cross(&forward_basis, &sp4c)
            } else {
                Vec3::new(0.0, 1.0, 0.0)
            };
            quat_from_axis_angle(&sp40, 0x2d8 as f64)
        } else {
            quat_from_dirs(&forward_basis, &sp4c)
        };

        stack.push();
        stack.from_quat(&quat);
        stack.normalize_basis();
        stack.b = stack.a;
        stack.pop();
        let b = stack.b;
        stack.mult_right(&b);
    }

    fn update_ape_spin_compensation(stack: &mut MatrixStack) {
        let tmp = stack.a;
        let mut sp24 = stack.tf_vec(&Vec3::new(-1.0, 0.0, 0.0));
        let quat = if sp24.y < 0.99 {
            let sp18 = Vec3::new(0.0, 1.0, 0.0);
            sp24 = vec_cross(&sp24, &sp18);
            quat_from_axis_angle(&sp24, 0x38e as f64)
        } else {
            quat_from_dirs(&sp24, &Vec3::new(0.0, 1.0, 0.0))
        };
        stack.from_quat(&quat);
        stack.normalize_basis();
        stack.mult_right(&tmp);
    }

    fn update_ape_orientation(
        &mut self,
        geom: &StageGeom,
        rt: &StageRuntime,
        ctx: &mut PhysicsCtx,
    ) {
        const APE_FLAG_ON_GROUND: u32 = 1;
        const APE_FLAG_IN_AIR: u32 = 2;
        self.ape_flags &= !0x3;

        let ray_hit = ctx
            .collider
            .raycast_stage_down(&self.pos, geom, &rt.anim_groups);
        if ray_hit.is_none() && self.vel.y < -0.16203702986240387 {
            self.ape_flags |= APE_FLAG_IN_AIR;
        } else if vec_len(&self.unk_b8) < 0.00027777777 {
            self.ape_flags |= APE_FLAG_ON_GROUND;
        }

        let mut r27 = self.flags & ball_flags::GOAL != 0;
        r27 = r27 || (self.ape_flags & 0x3) == 0;

        let stack = &mut ctx.stack;
        self.update_ape_base_orientation(stack);
        if r27 {
            self.update_ape_from_velocity(stack);
        } else {
            stack.from_quat(&self.ape_quat);
            stack.normalize_basis();
            if self.ape_flags & APE_FLAG_IN_AIR != 0 {
                Self::update_ape_spin_compensation(stack);
            }
        }

        self.ape_quat = stack.to_quat();

        stack.from_quat(&self.ape_quat);
        let tmp_dir = stack.tf_vec(&Vec3::new(-1.0, 0.0, 0.0));
        self.ape_yaw = to_s16((atan2_s16(tmp_dir.x, tmp_dir.z) - 0x8000) as f64);
    }

    fn adjust_movement_for_anim_group(
        &self,
        movement: &mut Vec3,
        rt: &StageRuntime,
        stack: &mut MatrixStack,
    ) {
        let phys = &self.phys_ball;
        let group = &rt.anim_groups[phys.hardest_coli_anim_group_id];
        let plane_point = phys.hardest_coli_plane.point;
        let plane_normal = phys.hardest_coli_plane.normal;

        stack.from_mtx(&group.transform);
        let sp38 = stack.tf_point(&plane_point);
        let sp44 = stack.tf_vec(&plane_normal);

        let mut sp14 = Vec3::new(
            sp38.x - self.pos.x,
            sp38.y - self.pos.y,
            sp38.z - self.pos.z,
        );
        let sp2c = Vec3::new(
            self.pos.x + sp14.x * sp44.x,
            self.pos.y + sp14.y * sp44.y,
            self.pos.z + sp14.z * sp44.z,
        );
        sp14 = stack.rigid_inv_tf_point(&sp2c);
        stack.from_mtx(&group.prev_transform);
        let sp20 = stack.tf_point(&sp14);
        sp14 = Vec3::new(sp2c.x - sp20.x, sp2c.y - sp20.y, sp2c.z - sp20.z);

        movement.x -= sp14.x;
        movement.y -= sp14.y;
        movement.z -= sp14.z;
    }

    fn update_orientation(&mut self, stack: &mut MatrixStack) {
        let mut q = quat_mul(&self.orientation, &self.delta_quat);
        quat_normalize(&mut q);
        self.orientation = q;
        stack.from_quat(&self.orientation);
        let (ry, rx, rz) = stack.to_euler_yxz();
        self.rot_y = ry;
        self.rot_x = rx;
        self.rot_z = rz;
    }

    fn rotational_kinematics(
        &mut self,
        rt: &StageRuntime,
        stack: &mut MatrixStack,
        goal_mode: bool,
    ) {
        let mut do_rot = false;
        if goal_mode {
            if self.phys_ball.hardest_coli_speed < 0.0 {
                do_rot = true;
            }
        } else if self.phys_ball.flags & COLI_OCCURRED != 0 {
            do_rot = true;
        }

        if !do_rot {
            if self.flags & ball_flags::FLAG_10 != 0 {
                self.vel.x *= 0.95;
                self.vel.y *= 0.95;
                self.vel.z *= 0.95;
            }
            return;
        }

        let mut movement = Vec3::new(
            self.pos.x - self.prev_pos.x,
            self.pos.y - self.prev_pos.y,
            self.pos.z - self.prev_pos.z,
        );
        if self.phys_ball.hardest_coli_anim_group_id > 0 {
            self.adjust_movement_for_anim_group(&mut movement, rt, stack);
        }

        let plane_normal = self.phys_ball.hardest_coli_plane.normal;
        stack.from_mtx(&rt.anim_groups[self.phys_ball.hardest_coli_anim_group_id].transform);
        let plane_normal_world = stack.tf_vec(&plane_normal);

        let contact_offset = Vec3::new(
            -plane_normal_world.x * self.curr_radius,
            -plane_normal_world.y * self.curr_radius,
            -plane_normal_world.z * self.curr_radius,
        );

        stack.from_rotate_y(self.rot_y);
        stack.rotate_x(self.rot_x);
        stack.rotate_z(self.rot_z);
        let contact_offset = stack.rigid_inv_tf_vec(&contact_offset);

        let movement_local =
            stack.rigid_inv_tf_vec(&Vec3::new(-movement.x, -movement.y, -movement.z));

        let mut axis = vec_cross(&contact_offset, &movement_local);

        let mut inv_radius_sq = 0.0;
        if self.curr_radius > FLT_EPSILON {
            inv_radius_sq = RAD_TO_S16 / (self.curr_radius * self.curr_radius);
        }
        self.unk60 = axis.x * inv_radius_sq;
        self.unk62 = axis.y * inv_radius_sq;
        self.unk64 = axis.z * inv_radius_sq;

        if self.curr_radius > FLT_EPSILON {
            let mut len = vec_len(&axis);
            if len != 0.0 {
                len = vec_normalize_len(&mut axis);
                self.delta_quat = quat_from_axis_angle(
                    &axis,
                    (len * 2.0) / (self.curr_radius * self.curr_radius * std::f64::consts::PI)
                        * RAD_TO_S16,
                );
            }
        }
    }

    /// `stepBall`: advance the ball one tick against the stage (everything except visual effects).
    pub fn step(
        &mut self,
        geom: &StageGeom,
        rt: &mut StageRuntime,
        world: &World,
        ctx: &mut PhysicsCtx,
    ) {
        self.flags &= !ball_flags::FLAG_00;
        if self.state == ball_states::GOAL_MAIN {
            self.goal_timer += 1;
            if self.goal_timer >= GOAL_FLOAT_FRAMES && self.flags & ball_flags::FLAG_09 == 0 {
                self.flags &= !(ball_flags::FLAG_08 | ball_flags::FLAG_10);
                self.flags |= ball_flags::FLAG_09;
            }
        }
        if self.transform[0] == 0.0 && self.transform[5] == 0.0 && self.transform[10] == 0.0 {
            self.update_transform(&mut ctx.stack);
        }
        self.prev_transform = self.transform;
        self.prev_pos = self.pos;
        self.prev_orientation = self.orientation;
        self.speed = vec_len(&self.vel);
        self.unk_f8 = self.speed;
        self.flags &= !ball_flags::FLAG_05;

        let mut accel = Vec3::new(0.0, -self.accel, 0.0);
        if self.flags & ball_flags::FLAG_09 != 0 {
            accel.y = -accel.y;
        } else if self.flags & ball_flags::FLAG_08 != 0 {
            accel.y = 0.0;
        }

        ctx.stack.from_identity();
        ctx.stack.rotate_x(world.xrot);
        ctx.stack.rotate_z(world.zrot);
        let accel = ctx.stack.rigid_inv_tf_vec(&accel);

        self.vel.x += accel.x;
        self.vel.y += accel.y;
        self.vel.z += accel.z;

        self.pos.x += self.vel.x;
        self.pos.y += self.vel.y;
        self.pos.z += self.vel.z;

        self.init_phys_ball();
        ctx.collider
            .collide_ball_with_stage(&mut self.phys_ball, geom, &rt.anim_groups);
        ctx.collider.collide_ball_with_bonus_wave(
            &mut self.phys_ball,
            geom.stage_id,
            rt.timer_frames,
        );
        self.sync_from_phys_ball();
        let stage_coli_flags = self.phys_ball.flags;
        let stage_coli_speed = self.phys_ball.hardest_coli_speed;

        if self.phys_ball.flags & COLI_OCCURRED != 0 {
            let id = self.phys_ball.hardest_coli_anim_group_id;
            let n = self.phys_ball.hardest_coli_plane.normal;
            if id == 0 {
                self.unk114 = Vec3::new(-n.x, -n.y, -n.z);
            } else {
                ctx.stack.from_mtx(&rt.anim_groups[id].transform);
                let t = ctx.stack.tf_vec(&n);
                self.unk114 = Vec3::new(-t.x, -t.y, -t.z);
            }
        }

        let goal_mode = self.state == ball_states::GOAL_MAIN;
        self.rotational_kinematics(rt, &mut ctx.stack, goal_mode);
        self.update_orientation(&mut ctx.stack);
        self.update_transform(&mut ctx.stack);
        self.update_ape_basis(&mut ctx.stack);
        self.update_ape_orientation(geom, rt, ctx);
        self.update_ball_camera_steer_yaw();

        self.init_phys_ball();
        let bumper_hits = ctx.collider.bumper_hits;
        rt.collide_ball_with_objects(&mut ctx.collider, &mut self.phys_ball);
        self.sync_from_phys_ball();
        self.audio = BallAudio {
            last_coli_flags: stage_coli_flags | self.phys_ball.flags,
            last_coli_speed: stage_coli_speed.min(self.phys_ball.hardest_coli_speed),
            bumper_hits: ctx.collider.bumper_hits.wrapping_sub(bumper_hits),
        };

        self.unk80 += 1;
    }
}

/// `checkBallEnteredGoal`
pub fn check_ball_entered_goal(
    ball: &mut Ball,
    geom: &StageGeom,
    rt: &StageRuntime,
    collider: &mut Collider,
) -> Option<GoalHit> {
    ball.init_phys_ball();
    let phys = &mut ball.phys_ball;
    let mut goal_id = 0;
    for (anim_group_id, ag) in geom.groups.iter().enumerate() {
        if ag.goals.is_empty() {
            continue;
        }
        if anim_group_id != phys.anim_group_id {
            collider.tf_physball_to_group(phys, anim_group_id, &rt.anim_groups);
        }
        for goal in &ag.goals {
            let mut trigger = TriggerRect {
                pos: Vec3::new(0.0, 1.0, 0.0),
                rot: goal.rot,
                width: 2.0,
                height: 2.0,
            };
            collider.stack.from_translate(&goal.pos);
            collider.stack.rotate_z(goal.rot[2]);
            collider.stack.rotate_y(goal.rot[1]);
            collider.stack.rotate_x(goal.rot[0]);
            trigger.pos = collider.stack.tf_point(&trigger.pos);
            let (pos, prev) = (phys.pos, phys.prev_pos);
            if collider.test_line_intersects_rect(&pos, &prev, &trigger) {
                return Some(GoalHit {
                    goal_id,
                    anim_group_id,
                    goal_type: goal.kind,
                });
            }
            goal_id += 1;
        }
    }
    None
}
