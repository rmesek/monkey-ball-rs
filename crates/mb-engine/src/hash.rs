//! Port of `src/sim_hash.ts`: FNV-1a style rolling hash over the deterministic simulation state.

use crate::ball::Ball;
use crate::math::Vec3;
use crate::stage_runtime::StageRuntime;
use crate::world::World;

#[derive(Debug, Clone, Copy)]
pub struct SimHasher(pub u32);

impl SimHasher {
    pub fn new() -> Self {
        SimHasher(0x811c_9dc5)
    }
    pub fn u32(&mut self, v: u32) {
        self.0 = (self.0 ^ v).wrapping_mul(16777619);
    }
    pub fn i32(&mut self, v: i32) {
        self.u32(v as u32);
    }
    pub fn f32(&mut self, v: f64) {
        self.u32((v as f32).to_bits());
    }
    pub fn vec3(&mut self, v: &Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
    pub fn s16(&mut self, v: i32) {
        self.u32((v & 0xffff) as u32);
    }
}

impl Default for SimHasher {
    fn default() -> Self {
        Self::new()
    }
}

/// `hashSimState(balls, worlds, stageRuntime)` (without the `includeVisual` extras).
pub fn hash_sim_state(balls: &[&Ball], worlds: &[&World], rt: &StageRuntime) -> u32 {
    hash_deterministic_state(balls, worlds, rt).0
}

/// `hashSimState(balls, worlds, stageRuntime, { includeVisual: true })`: the simulation state followed by
/// the confetti and the ball effects (count, then position, velocity and rotation of each).
pub fn hash_sim_state_visual(balls: &[&Ball], worlds: &[&World], rt: &StageRuntime) -> u32 {
    let mut h = hash_deterministic_state(balls, worlds, rt);
    h.u32(rt.confetti.len() as u32);
    for frag in &rt.confetti {
        h.vec3(&frag.pos);
        h.vec3(&frag.vel);
        h.s16(frag.rot_x);
        h.s16(frag.rot_y);
        h.s16(frag.rot_z);
    }
    h.u32(rt.effects.len() as u32);
    for fx in &rt.effects {
        h.vec3(&fx.pos);
        h.vec3(&fx.vel);
        h.s16(fx.rot_x);
        h.s16(fx.rot_y);
        h.s16(fx.rot_z);
    }
    h.0
}

fn hash_deterministic_state(balls: &[&Ball], worlds: &[&World], rt: &StageRuntime) -> SimHasher {
    let mut h = SimHasher::new();
    h.u32(balls.len() as u32);
    for ball in balls {
        h.vec3(&ball.pos);
        h.vec3(&ball.vel);
        h.s16(ball.rot_x);
        h.s16(ball.rot_y);
        h.s16(ball.rot_z);
        h.i32(ball.state);
        h.u32(ball.flags);
        h.f32(ball.curr_radius);
        h.f32(ball.speed);
        // `ball.animGroupId` does not exist on the reference ball object (`undefined | 0`).
        h.u32(0);
        h.s16(ball.ape_yaw);
        h.f32(ball.orientation.x);
        h.f32(ball.orientation.y);
        h.f32(ball.orientation.z);
        h.f32(ball.orientation.w);
    }

    h.u32(worlds.len() as u32);
    for world in worlds {
        h.s16(world.xrot);
        h.s16(world.zrot);
        h.vec3(&world.gravity);
    }

    h.i32(rt.timer_frames);
    h.u32(rt.anim_groups.len() as u32);
    for group in &rt.anim_groups {
        h.vec3(&group.pos);
        h.f32(group.rot[0] as f64);
        h.f32(group.rot[1] as f64);
        h.f32(group.rot[2] as f64);
    }
    h.u32(rt.goal_bags.len() as u32);
    for bag in &rt.goal_bags {
        h.i32(bag.state);
        h.f32(bag.openness);
        h.vec3(&bag.local_pos);
        h.vec3(&bag.local_vel);
        h.s16(bag.rot_x);
        h.s16(bag.rot_y);
        h.s16(bag.rot_z);
    }
    h.u32(rt.bananas.len() as u32);
    for banana in &rt.bananas {
        if banana.collected || banana.state == 8 {
            h.u32(0);
            continue;
        }
        h.i32(banana.state);
        h.vec3(&banana.local_pos);
        h.vec3(&banana.vel);
        h.s16(banana.rot_x);
        h.s16(banana.rot_y);
        h.s16(banana.rot_z);
        h.f32(banana.scale);
    }
    h
}
