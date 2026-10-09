//! Runtime half of WebMonkeyBall `src/stage.ts` (`StageRuntime`, SMB1 paths): anim-group playback from the
//! stagedef keyframes, bumpers, jamabars, goal tape/bag, bananas and the stage timer.
//!
//! The reference's display-only state lives here too, as it does there: ball effects and goal confetti
//! ([`crate::effects`]) with their own random stream. They never feed back into ball physics or
//! [`crate::hash::hash_sim_state`].

use crate::animation::interpolate_keyframes;
use crate::ball::Ball;
use crate::collision::{Collider, PhysBall};
use crate::constants::*;
use crate::effects::{self, BallEffect, ConfettiParticle, Ground, Spawner};
use crate::geom::StageGeom;
use crate::math::*;
use crate::objects::*;
use crate::rng::DeterministicRng;
use mb_stage::Keyframe;

const GOAL_BAG_LOCAL_START: Vec3 = Vec3::new(0.0, -1.0, 0.1);
const GOAL_TAPE_SEGMENT_COUNT: usize = 8;
const GOAL_TAPE_SEGMENT_LEN: f64 = 0.225;
const GOAL_TAPE_GRAVITY_SCALE: f64 = 0.004;
const GOAL_TAPE_GROUND_OFFSET: f64 = 0.002;
/// Default tape anchor height; the reference overrides it from `common_p.lz` when that model loads.
pub const GOAL_TAPE_ANCHOR_Y: f64 = 0.7400003672;
const GOAL_TAPE_Y_STEP: f64 = 0.016666668;
const GOAL_TAPE_X_SCALE: f64 = 1.75;
const GOAL_TAPE_X_OFFSET: f64 = 0.875;
const BANANA_COLLECT_WAIT_FRAMES: i32 = 15;
const BANANA_SHRINK_STEP: f64 = 1.0 / BANANA_COLLECT_WAIT_FRAMES as f64;
const BANANA_BASE_SCALES: [f64; 4] = [0.5, 0.75, 0.5, 0.75];
const BANANA_ROT_VEL_Y: [i32; 4] = [1024, 768, 1024, 1024];
pub const BANANA_STATE_HOLDING: i32 = 7;
pub const BANANA_STATE_FLY: i32 = 8;
pub const BANANA_HOLD_FRAMES: i32 = 30;
const BANANA_HOLD_DAMP: f64 = 0.8;
const BANANA_HOLD_Y_BASE: f64 = 0.75;
const BANANA_HOLD_Y_RANGE: f64 = 0.5;
const BANANA_HOLD_Y_LERP: f64 = 0.2;
const BANANA_HOLD_ROT_TARGET: i32 = 0x1000;
const BANANA_FLY_FRAMES: i32 = 32;
const BANANA_HUD_TARGET_X: f64 = 1.0583333;
const BANANA_HUD_TARGET_Y: f64 = 0.84166664;
const BANANA_HUD_TARGET_Z: f64 = -2.0;
const FLY_IN_MIN_RADIUS: f64 = 31.25;
/// `Math.tan(Math.PI / 3 * 0.5)`
const BANANA_FOV_TAN: f64 = 0.5773502691896257;
/// `visualRng` is seeded with the stage id xor this.
const VISUAL_RNG_SEED_XOR: u32 = 0x9e37_79b9;

fn deg_to_s16(degrees: f64) -> i32 {
    to_s16((degrees * DEG_TO_S16).trunc())
}

/// Camera data the stage needs to aim collected bananas at the HUD.
#[derive(Debug, Clone, Copy)]
pub struct CameraRef {
    pub eye: Vec3,
    pub rot_x: i32,
    pub rot_y: i32,
    pub rot_z: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundSphere {
    pub pos: Vec3,
    pub radius: f64,
}

#[derive(Debug, Clone)]
pub struct StageRuntime {
    pub stage_id: u32,
    pub timer_frames: i32,
    pub anim_groups: Vec<AnimGroupState>,
    pub bumpers: Vec<Vec<BumperState>>,
    pub jamabars: Vec<Vec<JamabarState>>,
    pub goals: Vec<GoalState>,
    pub goal_bags: Vec<GoalBagState>,
    pub goal_tapes: Vec<GoalTapeState>,
    /// Per anim group: indices into `goal_bags` / `goal_tapes`.
    pub goal_indices_by_group: Vec<Vec<usize>>,
    pub bananas: Vec<BananaState>,
    pub bound_sphere: BoundSphere,
    pub goal_hold_open: bool,
    pub sim_rng: DeterministicRng,
    /// Live ball effects in spawn order (`stageRuntime.effects`): sparks, collision stars and flashes,
    /// post-goal sparkles. Display only; at most [`effects::MAX_BALL_EFFECTS`], allocated once.
    pub effects: Vec<BallEffect>,
    /// Live goal confetti in spawn order (`stageRuntime.confetti`). Display only.
    pub confetti: Vec<ConfettiParticle>,
    /// The random stream of everything display-only (`visualRng`); the simulation never draws from it.
    pub visual_rng: DeterministicRng,
    /// Off: no ball effect is spawned and no particle is updated (the reference's `suppressVisualEffects`).
    pub visuals_enabled: bool,
    /// `confettiSpawnedGoals`: a bag throws confetti the first time it opens only.
    confetti_spawned_goals: Vec<bool>,
    next_effect_id: u32,
    stack: MatrixStack,
    /// Scratch of the display-only code, so that it cannot leave anything behind in the simulation's.
    visual_stack: MatrixStack,
    visual_collider: Collider,
}

impl StageRuntime {
    pub fn new(geom: &StageGeom, goal_tape_anchor_y: f64) -> StageRuntime {
        let mut rt = StageRuntime {
            stage_id: geom.stage_id,
            timer_frames: 0,
            anim_groups: Vec::new(),
            bumpers: Vec::new(),
            jamabars: Vec::new(),
            goals: Vec::new(),
            goal_bags: Vec::new(),
            goal_tapes: Vec::new(),
            goal_indices_by_group: Vec::new(),
            bananas: Vec::new(),
            bound_sphere: BoundSphere {
                pos: Vec3::ZERO,
                radius: 50.0,
            },
            goal_hold_open: false,
            sim_rng: DeterministicRng::new(geom.stage_id),
            effects: Vec::with_capacity(effects::MAX_BALL_EFFECTS),
            confetti: Vec::new(),
            visual_rng: DeterministicRng::new(geom.stage_id ^ VISUAL_RNG_SEED_XOR),
            visuals_enabled: true,
            confetti_spawned_goals: Vec::new(),
            next_effect_id: 1,
            stack: MatrixStack::new(),
            visual_stack: MatrixStack::new(),
            visual_collider: Collider::new(),
        };
        rt.init_anim_groups(geom);
        rt.init_objects(geom, goal_tape_anchor_y);
        rt.confetti_spawned_goals = vec![false; rt.goal_bags.len()];
        // One bag opens per attempt (the goal the ball enters), so play never grows this.
        if !rt.goal_bags.is_empty() {
            rt.confetti.reserve_exact(effects::CONFETTI_PER_GOAL);
        }
        rt
    }

    fn init_anim_groups(&mut self, geom: &StageGeom) {
        for g in &geom.groups {
            let mut info = AnimGroupState {
                pos: g.init_pos,
                prev_pos: Vec3::new(
                    g.init_pos.x - g.unk_b8.x,
                    g.init_pos.y - g.unk_b8.y,
                    g.init_pos.z - g.unk_b8.z,
                ),
                rot: g.init_rot,
                prev_rot: g.init_rot,
                transform: [0.0; 12],
                prev_transform: [0.0; 12],
            };
            self.stack.from_identity();
            self.stack.to_mtx(&mut info.transform);
            self.stack.translate_neg(&g.unk_b8);
            self.stack.to_mtx(&mut info.prev_transform);
            self.anim_groups.push(info);
        }
    }

    fn init_objects(&mut self, geom: &StageGeom, anchor_y: f64) {
        let count = geom.groups.len();
        self.goal_indices_by_group = vec![Vec::new(); count];
        let mut collider = Collider::new();
        for (i, g) in geom.groups.iter().enumerate() {
            let mut bumper_states = Vec::new();
            for b in &g.bumpers {
                let base_pos = b.pos;
                self.stack.from_identity();
                self.stack.rotate_z(b.rot[2]);
                self.stack.rotate_y(b.rot[1]);
                self.stack.rotate_x(b.rot[0]);
                let center_offset = self.stack.tf_point(&Vec3::new(
                    BUMPER_BOUND_CENTER.0,
                    BUMPER_BOUND_CENTER.1,
                    BUMPER_BOUND_CENTER.2,
                ));
                let coli_pos = Vec3::new(
                    base_pos.x + center_offset.x,
                    base_pos.y + center_offset.y,
                    base_pos.z + center_offset.z,
                );
                bumper_states.push(BumperState {
                    anim_group_id: i,
                    base_pos,
                    pos: coli_pos,
                    prev_pos: coli_pos,
                    rot: b.rot,
                    scale: b.scale,
                    radius: BUMPER_BOUND_RADIUS,
                    model_radius: BUMPER_MODEL_RADIUS,
                    state: 0,
                    spin: 0,
                    spin_vel: 0,
                    pulse_x: 1.0,
                    pulse_z: 1.0,
                    counter: 0,
                    transform: [0.0; 12],
                });
            }
            let jamabar_radius = sqrt((1.75 * 1.75) + (0.5 * 0.5) + (0.5 * 0.5));
            let jamabar_states: Vec<JamabarState> = g
                .jamabars
                .iter()
                .map(|j| JamabarState {
                    anim_group_id: i,
                    base_pos: j.pos,
                    pos: j.pos,
                    prev_pos: j.pos,
                    rot: j.rot,
                    scale: j.scale,
                    local_pos: Vec3::ZERO,
                    local_vel: Vec3::ZERO,
                    radius: jamabar_radius,
                    transform: [0.0; 12],
                })
                .collect();
            self.bumpers.push(bumper_states);
            self.jamabars.push(jamabar_states);

            for goal in &g.goals {
                self.goals.push(GoalState {
                    anim_group_id: i,
                    pos: goal.pos,
                    rot: goal.rot,
                    kind: goal.kind,
                    transform: [0.0; 12],
                });
                let tape =
                    self.create_goal_tape(goal.pos, goal.rot, i, anchor_y, geom, &mut collider);
                self.goal_indices_by_group[i].push(self.goal_tapes.len());
                self.goal_tapes.push(tape);
                let bag = self.create_goal_bag(goal.pos, goal.rot, i);
                self.goal_bags.push(bag);
            }
            for banana in &g.bananas {
                let kind = banana.kind & 3;
                self.bananas.push(BananaState {
                    anim_group_id: i,
                    local_pos: banana.pos,
                    prev_local_pos: banana.pos,
                    pos: banana.pos,
                    kind: banana.kind,
                    flags: 0x22,
                    cooldown: 0,
                    collected: false,
                    state: 2,
                    collect_timer: 0,
                    hold_timer: 0,
                    hold_offset: Vec3::ZERO,
                    hold_scale_target: 1.0,
                    hold_rot_vel: 0,
                    fly_timer: 0,
                    fly_scale_target: 0.0,
                    fly_start_pos: banana.pos,
                    fly_start_scale: 1.0,
                    tilt_timer: 0,
                    base_scale: BANANA_BASE_SCALES[kind as usize],
                    scale: 1.0,
                    prev_scale: 1.0,
                    vel: Vec3::ZERO,
                    rot_x: 0,
                    rot_y: 0,
                    rot_z: 0,
                    prev_rot_x: 0,
                    prev_rot_y: 0,
                    prev_rot_z: 0,
                    rot_vel_x: 0,
                    rot_vel_y: BANANA_ROT_VEL_Y[kind as usize],
                    rot_vel_z: 0,
                });
            }
        }
        self.sync_object_transforms();
    }

    fn create_goal_tape(
        &mut self,
        goal_pos: Vec3,
        goal_rot: [i32; 3],
        anim_group_id: usize,
        anchor_y: f64,
        geom: &StageGeom,
        collider: &mut Collider,
    ) -> GoalTapeState {
        let mut points = Vec::with_capacity(GOAL_TAPE_SEGMENT_COUNT);
        for i in 0..GOAL_TAPE_SEGMENT_COUNT {
            let t = (GOAL_TAPE_SEGMENT_COUNT - 1 - i) as f64 / (GOAL_TAPE_SEGMENT_COUNT - 1) as f64;
            let p = Vec3::new((GOAL_TAPE_X_SCALE * t) - GOAL_TAPE_X_OFFSET, anchor_y, 0.0);
            points.push(TapePoint {
                pos: p,
                prev_pos: p,
                normal: Vec3::new(0.0, 0.0, 1.0),
                prev_normal: Vec3::new(0.0, 0.0, 1.0),
                t,
                vel: Vec3::ZERO,
                flags: 6,
                len: GOAL_TAPE_SEGMENT_LEN,
            });
        }
        points[0].flags = (points[0].flags & !2) | 1;
        let last = GOAL_TAPE_SEGMENT_COUNT - 1;
        points[last].flags = (points[last].flags & !4) | 1;

        let stack = &mut self.stack;
        stack.from_translate(&goal_pos);
        stack.rotate_z(goal_rot[2]);
        stack.rotate_y(goal_rot[1]);
        stack.rotate_x(goal_rot[0]);
        stack.mult_left(&self.anim_groups[anim_group_id].transform);
        let anchor_pos = stack.tf_point(&Vec3::new(0.0, anchor_y, 0.0));

        let mut floor_y = GOAL_TAPE_GROUND_OFFSET;
        if let Some(hit) = collider.raycast_stage_down(&anchor_pos, geom, &self.anim_groups) {
            let local_hit = stack.rigid_inv_tf_point(&hit.pos);
            if anchor_y > local_hit.y {
                floor_y = local_hit.y + GOAL_TAPE_GROUND_OFFSET;
            }
        }

        GoalTapeState {
            anim_group_id,
            flags: 0,
            break_frame: -1,
            ground_y: floor_y,
            anchor_y,
            target_y: anchor_y,
            goal_pos,
            goal_rot,
            points,
        }
    }

    fn create_goal_bag(
        &mut self,
        goal_pos: Vec3,
        goal_rot: [i32; 3],
        anim_group_id: usize,
    ) -> GoalBagState {
        let stack = &mut self.stack;
        stack.from_translate(&goal_pos);
        stack.rotate_z(goal_rot[2]);
        stack.rotate_y(goal_rot[1]);
        stack.rotate_x(goal_rot[0]);
        let local_offset = stack.tf_point(&Vec3::new(
            GOAL_BAG_OFFSET.0,
            GOAL_BAG_OFFSET.1,
            GOAL_BAG_OFFSET.2,
        ));
        let mut local_pos = GOAL_BAG_LOCAL_START;
        vec_normalize_len(&mut local_pos);

        let mut bag = GoalBagState {
            anim_group_id,
            state: 1,
            counter: 0,
            flags: 0,
            openness: 0.0,
            prev_openness: 0.0,
            unk8: 0.0,
            open_frame: -1,
            goal_pos,
            goal_rot,
            rot_x: goal_rot[0],
            rot_y: goal_rot[1],
            rot_z: goal_rot[2],
            prev_rot_x: goal_rot[0],
            prev_rot_y: goal_rot[1],
            prev_rot_z: goal_rot[2],
            u_some_pos: local_offset,
            local_pos,
            local_vel: Vec3::ZERO,
            model_origin: Vec3::new(
                GOAL_BAG_BASE_CENTER.0,
                GOAL_BAG_BASE_CENTER.1,
                GOAL_BAG_BASE_CENTER.2,
            ),
            bound_sphere_radius: GOAL_BAG_BASE_RADIUS,
            position: Vec3::ZERO,
            prev_pos: Vec3::ZERO,
        };
        update_goal_bag_transform(&mut bag, stack);
        bag.prev_pos = bag.position;
        bag
    }

    /// `computeBoundSphere`: world-space bounds of every group's collision triangles (the reference replaces
    /// this with the stage model's bounds when the GMA loads; see [`StageRuntime::bound_sphere`]).
    pub fn compute_bound_sphere(&mut self, geom: &StageGeom) {
        let mut stack = MatrixStack::new();
        let mut local_stack = MatrixStack::new();
        let mut has_bounds = false;
        let mut min = Vec3::ZERO;
        let mut max = Vec3::ZERO;

        for (i, ag) in geom.groups.iter().enumerate() {
            if ag.triangles.is_empty() {
                continue;
            }
            let mut local_has = false;
            let mut local_min = Vec3::ZERO;
            let mut local_max = Vec3::ZERO;
            for tri in &ag.triangles {
                local_stack.from_translate(&tri.pos);
                local_stack.rotate_y(tri.rot[1]);
                local_stack.rotate_x(tri.rot[0]);
                local_stack.rotate_z(tri.rot[2]);
                let v1 = tri.pos;
                let v2 = local_stack.tf_point(&Vec3::new(tri.vert2[0], tri.vert2[1], 0.0));
                let v3 = local_stack.tf_point(&Vec3::new(tri.vert3[0], tri.vert3[1], 0.0));
                if !local_has {
                    local_has = true;
                    local_min = v1;
                    local_max = v1;
                }
                for v in [v1, v2, v3] {
                    local_min.x = local_min.x.min(v.x);
                    local_min.y = local_min.y.min(v.y);
                    local_min.z = local_min.z.min(v.z);
                    local_max.x = local_max.x.max(v.x);
                    local_max.y = local_max.y.max(v.y);
                    local_max.z = local_max.z.max(v.z);
                }
            }
            if !local_has {
                continue;
            }
            let corners = [
                Vec3::new(local_min.x, local_min.y, local_min.z),
                Vec3::new(local_min.x, local_min.y, local_max.z),
                Vec3::new(local_min.x, local_max.y, local_min.z),
                Vec3::new(local_min.x, local_max.y, local_max.z),
                Vec3::new(local_max.x, local_min.y, local_min.z),
                Vec3::new(local_max.x, local_min.y, local_max.z),
                Vec3::new(local_max.x, local_max.y, local_min.z),
                Vec3::new(local_max.x, local_max.y, local_max.z),
            ];
            stack.from_mtx(&self.anim_groups[i].transform);
            for corner in corners {
                let c = stack.tf_point(&corner);
                if !has_bounds {
                    has_bounds = true;
                    min = c;
                    max = c;
                } else {
                    min.x = min.x.min(c.x);
                    min.y = min.y.min(c.y);
                    min.z = min.z.min(c.z);
                    max.x = max.x.max(c.x);
                    max.y = max.y.max(c.y);
                    max.z = max.z.max(c.z);
                }
            }
        }

        if !has_bounds {
            self.bound_sphere = BoundSphere {
                pos: Vec3::ZERO,
                radius: 50.0,
            };
            return;
        }
        let pos = Vec3::new(
            (max.x + min.x) * 0.5,
            (max.y + min.y) * 0.5,
            (max.z + min.z) * 0.5,
        );
        let half = Vec3::new(
            (max.x - min.x) * 0.5,
            (max.y - min.y) * 0.5,
            (max.z - min.z) * 0.5,
        );
        let mut radius = sqrt((half.x * half.x) + (half.y * half.y) + (half.z * half.z));
        if radius < FLY_IN_MIN_RADIUS {
            radius = FLY_IN_MIN_RADIUS;
        }
        self.bound_sphere = BoundSphere { pos, radius };
    }

    /// `getFlyInSphere`
    pub fn fly_in_sphere(&self) -> BoundSphere {
        let override_radius = match self.stage_id {
            4 | 14 => Some(50.0),
            144 => Some(31.25),
            _ => None,
        };
        if let Some(radius) = override_radius {
            return BoundSphere {
                pos: Vec3::ZERO,
                radius,
            };
        }
        BoundSphere {
            pos: self.bound_sphere.pos,
            radius: self.bound_sphere.radius.max(FLY_IN_MIN_RADIUS),
        }
    }

    pub fn reset_timer(&mut self) {
        self.timer_frames = 0;
    }

    /// `advance(frameDelta=1, ...)`. `anim_timer_override` is the intro-time override of the timer.
    #[allow(clippy::too_many_arguments)]
    pub fn advance(
        &mut self,
        geom: &StageGeom,
        gravity: Vec3,
        anim_timer_override: Option<i32>,
        ball_pos: Option<Vec3>,
        camera: Option<CameraRef>,
    ) {
        match anim_timer_override {
            Some(t) => self.timer_frames = t,
            None => self.timer_frames += 1,
        }
        self.update_anim_groups(geom, self.timer_frames as f64 / 60.0);
        self.update_objects(geom, gravity, ball_pos, camera);
    }

    /// `updateAnimGroups` (SMB1): sample the keyframe tracks at `time_seconds` and rebuild the transforms.
    pub fn update_anim_groups(&mut self, geom: &StageGeom, time_seconds: f64) {
        let loop_span = geom.loop_end_seconds - geom.loop_start_seconds;
        let mut anim_time = time_seconds + geom.loop_start_seconds;
        if loop_span > 0.0 {
            anim_time -= loop_span * (anim_time / loop_span).floor();
            anim_time += geom.loop_start_seconds;
        }

        for (i, ag) in geom.groups.iter().enumerate() {
            let info = &mut self.anim_groups[i];
            if let Some(anim) = &ag.anim {
                let track = |frames: &Vec<Keyframe>| -> Option<f64> {
                    if frames.is_empty() {
                        None
                    } else {
                        Some(interpolate_keyframes(frames, anim_time))
                    }
                };
                if let Some(v) = track(&anim.rot_x) {
                    info.prev_rot[0] = info.rot[0];
                    info.rot[0] = deg_to_s16(v);
                }
                if let Some(v) = track(&anim.rot_y) {
                    info.prev_rot[1] = info.rot[1];
                    info.rot[1] = deg_to_s16(v);
                }
                if let Some(v) = track(&anim.rot_z) {
                    info.prev_rot[2] = info.rot[2];
                    info.rot[2] = deg_to_s16(v);
                }
                if let Some(v) = track(&anim.pos_x) {
                    info.prev_pos.x = info.pos.x - ag.unk_b8.x;
                    info.pos.x = v;
                }
                if let Some(v) = track(&anim.pos_y) {
                    info.prev_pos.y = info.pos.y - ag.unk_b8.y;
                    info.pos.y = v;
                }
                if let Some(v) = track(&anim.pos_z) {
                    info.prev_pos.z = info.pos.z - ag.unk_b8.z;
                    info.pos.z = v;
                }
            }

            let stack = &mut self.stack;
            stack.from_translate(&info.pos);
            stack.rotate_z(info.rot[2]);
            stack.rotate_y(info.rot[1]);
            stack.rotate_x(info.rot[0] - ag.init_rot[0]);
            stack.rotate_y(-ag.init_rot[1]);
            stack.rotate_z(-ag.init_rot[2]);
            stack.translate_neg(&ag.init_pos);
            stack.to_mtx(&mut info.transform);

            stack.from_translate(&info.prev_pos);
            stack.rotate_z(info.prev_rot[2]);
            stack.rotate_y(info.prev_rot[1]);
            stack.rotate_x(info.prev_rot[0] - ag.init_rot[0]);
            stack.rotate_y(-ag.init_rot[1]);
            stack.rotate_z(-ag.init_rot[2]);
            stack.translate_neg(&ag.init_pos);
            stack.to_mtx(&mut info.prev_transform);
        }
    }

    /// `updateObjects` (SMB1): tapes, bags, bumpers, jamabars, bananas, then transform sync.
    pub fn update_objects(
        &mut self,
        geom: &StageGeom,
        gravity: Vec3,
        ball_pos: Option<Vec3>,
        camera: Option<CameraRef>,
    ) {
        let fly_target_world = camera.map(|cam| {
            let fov2 = BANANA_FOV_TAN * 2.0;
            let target = Vec3::new(
                BANANA_HUD_TARGET_X * fov2,
                BANANA_HUD_TARGET_Y * fov2,
                BANANA_HUD_TARGET_Z,
            );
            self.stack.from_translate(&cam.eye);
            self.stack.rotate_y(cam.rot_y);
            self.stack.rotate_x(cam.rot_x);
            self.stack.rotate_z(cam.rot_z);
            self.stack.tf_point(&target)
        });

        for tape in self.goal_tapes.iter_mut() {
            update_goal_tape(tape, &self.anim_groups, &gravity, &mut self.stack);
        }
        for bag in self.goal_bags.iter_mut() {
            update_goal_bag(
                bag,
                &self.anim_groups,
                &gravity,
                self.goal_hold_open,
                &mut self.stack,
                &mut self.sim_rng,
            );
        }
        if self.visuals_enabled {
            let mut ground = Ground {
                geom,
                groups: &self.anim_groups,
                collider: &mut self.visual_collider,
            };
            effects::update_confetti(
                &mut self.confetti,
                &gravity,
                &mut ground,
                &mut self.visual_stack,
            );
            effects::update_ball_effects(
                &mut self.effects,
                &gravity,
                &mut ground,
                &mut self.visual_rng,
            );
        }

        for i in 0..self.anim_groups.len() {
            for bumper in self.bumpers[i].iter_mut() {
                bumper.prev_pos = bumper.pos;
                if bumper.state == 0 {
                    bumper.spin_vel += (0x100 - bumper.spin_vel) >> 6;
                    if bumper.pulse_x > 1.0 {
                        bumper.pulse_x -= 0.06666666666666667;
                        if bumper.pulse_x < 1.0 {
                            bumper.pulse_x = 1.0;
                        }
                    }
                    bumper.pulse_z = bumper.pulse_x;
                } else if bumper.state == 1 {
                    bumper.state = 2;
                    bumper.counter = 7;
                }
                if bumper.state == 2 {
                    bumper.counter -= 1;
                    if bumper.counter < 0 {
                        bumper.state = 0;
                    }
                    bumper.spin_vel += 0x100;
                    bumper.pulse_x += 0.5 * (2.0 - bumper.pulse_x);
                    bumper.pulse_z = bumper.pulse_x;
                }
                bumper.spin = wrap_s16(bumper.spin.wrapping_add(bumper.spin_vel));
            }
            for jamabar in self.jamabars[i].iter_mut() {
                jamabar.prev_pos = jamabar.pos;
                let stack = &mut self.stack;
                stack.from_translate(&jamabar.base_pos);
                stack.rotate_z(jamabar.rot[2]);
                stack.rotate_y(jamabar.rot[1]);
                stack.rotate_x(jamabar.rot[0]);
                stack.push();
                stack.mult_left(&self.anim_groups[i].transform);
                let gravity_local = stack.rigid_inv_tf_vec(&gravity);
                stack.pop();

                jamabar.local_vel.z += gravity_local.z * 0.016;
                jamabar.local_vel.z *= 0.97;
                jamabar.local_pos.z += jamabar.local_vel.z;
                if jamabar.local_pos.z < -2.5 {
                    jamabar.local_pos.z = -2.5;
                    if jamabar.local_vel.z < 0.0 {
                        jamabar.local_vel.z = -jamabar.local_vel.z;
                    }
                } else if jamabar.local_pos.z > 0.0 {
                    jamabar.local_pos.z = 0.0;
                    if jamabar.local_vel.z > 0.0 {
                        jamabar.local_vel.z = -jamabar.local_vel.z;
                    }
                }
                jamabar.pos = stack.tf_point(&jamabar.local_pos);
            }
        }

        for bi in 0..self.bananas.len() {
            let (group_id, state) = {
                let b = &mut self.bananas[bi];
                b.prev_local_pos = b.local_pos;
                b.prev_scale = b.scale;
                b.prev_rot_x = b.rot_x;
                b.prev_rot_y = b.rot_y;
                b.prev_rot_z = b.rot_z;
                if b.cooldown > 0 {
                    b.cooldown -= 1;
                }
                (b.anim_group_id, b.state)
            };
            let ball_local = if state == BANANA_STATE_HOLDING || state == BANANA_STATE_FLY {
                ball_pos.map(|p| {
                    if group_id != 0 {
                        self.stack.from_mtx(&self.anim_groups[group_id].transform);
                        self.stack.rigid_inv_tf_point(&p)
                    } else {
                        p
                    }
                })
            } else {
                None
            };
            let mut fly_target_local = None;
            if state == BANANA_STATE_FLY {
                if let Some(w) = fly_target_world {
                    fly_target_local = Some(if group_id != 0 {
                        self.stack.from_mtx(&self.anim_groups[group_id].transform);
                        self.stack.rigid_inv_tf_point(&w)
                    } else {
                        w
                    });
                }
            }
            update_banana(&mut self.bananas[bi], ball_local, fly_target_local);
        }

        self.sync_object_transforms();
    }

    /// The effect block of `stepBall`: sparks, collision stars and the post-goal sparkle for this ball step.
    pub(crate) fn spawn_ball_effects(&mut self, ball: &Ball) {
        if !self.visuals_enabled {
            return;
        }
        let mut out = Spawner {
            effects: &mut self.effects,
            rng: &mut self.visual_rng,
            next_id: &mut self.next_effect_id,
        };
        effects::spawn_for_ball_step(&mut out, ball);
    }

    /// `syncObjectTransforms`
    pub fn sync_object_transforms(&mut self) {
        let stack = &mut self.stack;
        for goal in self.goals.iter_mut() {
            stack.from_translate(&goal.pos);
            stack.rotate_z(goal.rot[2]);
            stack.rotate_y(goal.rot[1]);
            stack.rotate_x(goal.rot[0]);
            stack.mult_left(&self.anim_groups[goal.anim_group_id].transform);
            stack.to_mtx(&mut goal.transform);
        }
        for (i, bumpers) in self.bumpers.iter_mut().enumerate() {
            for bumper in bumpers.iter_mut() {
                stack.from_translate(&bumper.base_pos);
                stack.rotate_z(bumper.rot[2]);
                stack.rotate_y(bumper.rot[1]);
                stack.rotate_x(bumper.rot[0]);
                stack.rotate_y(bumper.spin);
                stack.scale_xyz(
                    bumper.scale.x * bumper.pulse_x,
                    bumper.scale.y,
                    bumper.scale.z * bumper.pulse_z,
                );
                stack.mult_left(&self.anim_groups[i].transform);
                stack.to_mtx(&mut bumper.transform);
            }
        }
        for (i, jamabars) in self.jamabars.iter_mut().enumerate() {
            for jamabar in jamabars.iter_mut() {
                stack.from_translate(&jamabar.pos);
                stack.rotate_z(jamabar.rot[2]);
                stack.rotate_y(jamabar.rot[1]);
                stack.rotate_x(jamabar.rot[0]);
                stack.scale_xyz(jamabar.scale.x, jamabar.scale.y, jamabar.scale.z);
                stack.mult_left(&self.anim_groups[i].transform);
                stack.to_mtx(&mut jamabar.transform);
            }
        }
        for banana in self.bananas.iter_mut() {
            stack.from_mtx(&self.anim_groups[banana.anim_group_id].transform);
            banana.pos = stack.tf_point(&banana.local_pos);
        }
    }

    /// `openGoalBag`
    pub fn open_goal_bag(&mut self, goal_id: usize, ball: &PhysBall) {
        let timer_frames = self.timer_frames;
        let Some(bag) = self.goal_bags.get_mut(goal_id) else {
            return;
        };
        if bag.flags != 0 {
            return;
        }
        bag.flags = 1;
        bag.state = 2;
        bag.bound_sphere_radius = GOAL_BAG_BASE_RADIUS * GOAL_BAG_OPEN_SCALE;
        bag.model_origin.x = GOAL_BAG_BASE_CENTER.0 * GOAL_BAG_OPEN_SCALE;
        bag.model_origin.y = GOAL_BAG_BASE_CENTER.1 * GOAL_BAG_OPEN_SCALE;
        bag.model_origin.z = GOAL_BAG_BASE_CENTER.2 * GOAL_BAG_OPEN_SCALE;
        bag.local_vel.x += 0.5 * ball.vel.x;
        bag.local_vel.y += 0.5 * ball.vel.y;
        bag.local_vel.z += 0.5 * ball.vel.z;
        if bag.open_frame < 0 {
            bag.open_frame = timer_frames;
        }
        update_goal_bag_transform(bag, &mut self.stack);
        // The reference spawns the confetti whether or not visuals are suppressed.
        if let Some(spawned) = self.confetti_spawned_goals.get_mut(goal_id) {
            if !*spawned {
                *spawned = true;
                effects::spawn_goal_bag_confetti(
                    &mut self.confetti,
                    bag,
                    &self.anim_groups[bag.anim_group_id],
                    &ball.vel,
                    &mut self.visual_stack,
                    &mut self.visual_rng,
                );
            }
        }
    }

    /// `breakGoalTape`: `ball` is expressed in the goal's anim-group space.
    pub fn break_goal_tape(&mut self, goal_id: usize, ball: &PhysBall) {
        self.open_goal_bag(goal_id, ball);
        let timer_frames = self.timer_frames;
        let Some(tape) = self.goal_tapes.get_mut(goal_id) else {
            return;
        };
        if tape.flags != 0 {
            return;
        }
        let stack = &mut self.stack;
        stack.from_identity();
        stack.translate(&tape.goal_pos);
        stack.rotate_z(tape.goal_rot[2]);
        stack.rotate_y(tape.goal_rot[1]);
        stack.rotate_x(tape.goal_rot[0]);
        let local_pos = stack.rigid_inv_tf_point(&ball.pos);
        let local_vel = stack.rigid_inv_tf_vec(&ball.vel);

        let mut nearest_index: Option<usize> = None;
        let mut nearest_dist = 17.5;
        for (i, point) in tape.points.iter().enumerate() {
            if point.flags & 4 == 0 {
                continue;
            }
            let dx = point.pos.x - local_pos.x;
            let dy = point.pos.y - local_pos.y;
            let dz = point.pos.z - local_pos.z;
            let dist = sqrt((dx * dx) + (dy * dy) + (dz * dz));
            if dist < nearest_dist {
                nearest_dist = dist;
                nearest_index = Some(i);
            }
        }
        let Some(ni) = nearest_index else { return };
        if ni + 1 >= tape.points.len() {
            return;
        }
        {
            let a = &mut tape.points[ni];
            a.flags &= !4;
            a.vel.x += local_vel.x;
            a.vel.y += local_vel.y;
            a.vel.z += local_vel.z;
        }
        {
            let b = &mut tape.points[ni + 1];
            b.flags &= !2;
            b.vel.x += local_vel.x;
            b.vel.y += local_vel.y;
            b.vel.z += local_vel.z;
        }
        tape.flags = 1;
        if tape.break_frame < 0 {
            tape.break_frame = timer_frames;
        }
    }

    /// `collideBallWithStageObjects` against this runtime's bumpers, jamabars and goal bags/tapes.
    pub fn collide_ball_with_objects(&mut self, collider: &mut Collider, ball: &mut PhysBall) {
        let mut objs = crate::collision::ObjectsMut {
            bumpers: &mut self.bumpers,
            jamabars: &mut self.jamabars,
            goal_bags: &mut self.goal_bags,
            goal_tapes: &mut self.goal_tapes,
            bag_indices_by_group: &self.goal_indices_by_group,
        };
        collider.collide_ball_with_stage_objects(ball, &mut objs, &self.anim_groups);
    }
}

fn update_goal_bag_transform(bag: &mut GoalBagState, stack: &mut MatrixStack) {
    bag.prev_pos = bag.position;
    stack.from_translate(&bag.u_some_pos);
    stack.rotate_y(bag.rot_y);
    stack.rotate_x(bag.rot_x);
    stack.rotate_z(bag.rot_z);
    bag.position = stack.tf_point(&bag.model_origin);
}

fn update_goal_bag(
    bag: &mut GoalBagState,
    groups: &[AnimGroupState],
    gravity: &Vec3,
    hold_open: bool,
    stack: &mut MatrixStack,
    rng: &mut DeterministicRng,
) {
    bag.prev_openness = bag.openness;
    bag.prev_rot_x = bag.rot_x;
    bag.prev_rot_y = bag.rot_y;
    bag.prev_rot_z = bag.rot_z;
    match bag.state {
        2..=4 => {
            if bag.state != 4 {
                bag.state = 4;
                bag.counter = -1;
                bag.unk8 = 0.05 + 0.1 * rng.next_float();
            }
            if bag.counter > 0 {
                bag.counter -= 1;
                if bag.counter == 0 && !hold_open {
                    bag.state = 5;
                }
            }
            bag.unk8 += 0.005;
            bag.unk8 *= 0.99;
            bag.openness += bag.unk8;
            if bag.openness < 0.0 {
                bag.openness = 0.0;
                if bag.unk8 < 0.0 {
                    bag.unk8 = 0.5 * -bag.unk8;
                }
            } else if bag.openness > 1.0 {
                bag.openness = 1.0;
                if bag.unk8 > 0.0 {
                    bag.unk8 = 0.5 * -bag.unk8;
                }
            }
        }
        5..=7 => {
            if bag.state != 7 {
                bag.state = 7;
                bag.counter = 60;
                bag.unk8 = 0.05 + 0.1 * rng.next_float();
            }
            bag.counter -= 1;
            bag.unk8 -= 0.005;
            bag.unk8 *= 0.99;
            bag.openness += bag.unk8;
            if bag.openness < 0.0 {
                bag.openness = 0.0;
                bag.flags = 0;
                bag.open_frame = -1;
                if bag.counter < 0 {
                    bag.state = 1;
                    bag.unk8 = 0.0;
                    bag.bound_sphere_radius = GOAL_BAG_BASE_RADIUS;
                    bag.model_origin = Vec3::new(
                        GOAL_BAG_BASE_CENTER.0,
                        GOAL_BAG_BASE_CENTER.1,
                        GOAL_BAG_BASE_CENTER.2,
                    );
                } else if bag.unk8 < 0.0 {
                    bag.unk8 = 0.5 * -bag.unk8;
                }
            } else if bag.openness > 1.0 {
                bag.openness = 1.0;
                if bag.unk8 > 0.0 {
                    bag.unk8 = 0.5 * -bag.unk8;
                }
            }
        }
        _ => {}
    }

    let mut sp3c = Vec3::new(0.008 * gravity.x, 0.008 * gravity.y, 0.008 * gravity.z);
    if bag.anim_group_id > 0 {
        stack.from_mtx(&groups[bag.anim_group_id].transform);
        sp3c = stack.rigid_inv_tf_vec(&sp3c);
    }
    bag.local_vel.x += sp3c.x;
    bag.local_vel.y += sp3c.y;
    bag.local_vel.z += sp3c.z;

    if bag.anim_group_id > 0 {
        let group = &groups[bag.anim_group_id];
        stack.from_mtx(&group.prev_transform);
        let mut prev_world = stack.tf_point(&bag.position);
        stack.from_mtx(&group.transform);
        let curr_world = stack.tf_point(&bag.position);
        prev_world.x -= curr_world.x;
        prev_world.y -= curr_world.y;
        prev_world.z -= curr_world.z;
        let prev_world = stack.rigid_inv_tf_vec(&prev_world);
        bag.local_vel.x += 0.02 * (prev_world.x - bag.local_vel.x);
        bag.local_vel.y += 0.02 * (prev_world.y - bag.local_vel.y);
        bag.local_vel.z += 0.02 * (prev_world.z - bag.local_vel.z);
    } else {
        bag.local_vel.x *= 0.98;
        bag.local_vel.y *= 0.98;
        bag.local_vel.z *= 0.98;
    }

    stack.from_translate(&bag.goal_pos);
    stack.rotate_z(bag.goal_rot[2]);
    stack.rotate_y(bag.goal_rot[1]);
    stack.rotate_x(bag.goal_rot[0]);
    let sp30 = stack.tf_point(&Vec3::new(
        GOAL_BAG_GUIDE_POS.0,
        GOAL_BAG_GUIDE_POS.1,
        GOAL_BAG_GUIDE_POS.2,
    ));
    let sp24 = stack.tf_vec(&Vec3::new(
        GOAL_BAG_GUIDE_DIR.0,
        GOAL_BAG_GUIDE_DIR.1,
        GOAL_BAG_GUIDE_DIR.2,
    ));

    let temp_f31 = bag.bound_sphere_radius;
    let sp18 = Vec3::new(
        bag.u_some_pos.x + temp_f31 * bag.local_pos.x,
        bag.u_some_pos.y + temp_f31 * bag.local_pos.y,
        bag.u_some_pos.z + temp_f31 * bag.local_pos.z,
    );
    let axis_delta = Vec3::new(sp18.x - sp30.x, sp18.y - sp30.y, sp18.z - sp30.z);
    let temp_f12 = vec_dot(&axis_delta, &sp24);
    if axis_delta.x * axis_delta.x + axis_delta.y * axis_delta.y + axis_delta.z * axis_delta.z
        - temp_f12 * temp_f12
        < temp_f31 * temp_f31
    {
        let projected = Vec3::new(
            sp30.x + sp24.x * temp_f12,
            sp30.y + sp24.y * temp_f12,
            sp30.z + sp24.z * temp_f12,
        );
        let mut sp_c = Vec3::new(
            sp18.x - projected.x,
            sp18.y - projected.y,
            sp18.z - projected.z,
        );
        vec_normalize_len(&mut sp_c);
        bag.local_pos.x = projected.x + sp_c.x * temp_f31 - bag.u_some_pos.x;
        bag.local_pos.y = projected.y + sp_c.y * temp_f31 - bag.u_some_pos.y;
        bag.local_pos.z = projected.z + sp_c.z * temp_f31 - bag.u_some_pos.z;
        vec_normalize_len(&mut bag.local_pos);
        let mut temp_f2_4 =
            sp_c.x * bag.local_vel.x + sp_c.y * bag.local_vel.y + sp_c.z * bag.local_vel.z;
        if temp_f2_4 < 0.0 {
            temp_f2_4 *= -1.5;
            bag.local_vel.x += temp_f2_4 * sp_c.x;
            bag.local_vel.y += temp_f2_4 * sp_c.y;
            bag.local_vel.z += temp_f2_4 * sp_c.z;
        }
    }

    let temp_f2_6 = -vec_dot(&bag.local_pos, &bag.local_vel);
    bag.local_vel.x += temp_f2_6 * bag.local_pos.x;
    bag.local_vel.y += temp_f2_6 * bag.local_pos.y;
    bag.local_vel.z += temp_f2_6 * bag.local_pos.z;
    bag.local_pos.x += bag.local_vel.x;
    bag.local_pos.y += bag.local_vel.y;
    bag.local_pos.z += bag.local_vel.z;
    vec_normalize_len(&mut bag.local_pos);

    stack.from_rotate_y(-bag.rot_y);
    stack.rotate_x(0);
    let rot_vec = stack.tf_vec(&bag.local_pos);
    bag.rot_x = atan2_s16_safe(rot_vec.z, rot_vec.y) - 0x8000;
    bag.rot_z = atan2_s16_safe(rot_vec.x, sqrt(sum_sq2(rot_vec.z, rot_vec.y)));

    update_goal_bag_transform(bag, stack);
}

fn update_banana(
    banana: &mut BananaState,
    ball_local: Option<Vec3>,
    fly_target_local: Option<Vec3>,
) {
    if banana.state == 0 {
        return;
    }
    if banana.tilt_timer > 0 {
        banana.tilt_timer -= 1;
    }
    if banana.state == BANANA_STATE_HOLDING {
        banana.hold_timer -= 1;
        if banana.hold_timer <= 0 {
            banana.state = BANANA_STATE_FLY;
            banana.fly_timer = BANANA_FLY_FRAMES;
            if let Some(b) = ball_local {
                banana.fly_start_pos = Vec3::new(
                    b.x + banana.hold_offset.x,
                    b.y + banana.hold_offset.y,
                    b.z + banana.hold_offset.z,
                );
            } else {
                banana.fly_start_pos = banana.local_pos;
            }
            banana.fly_start_scale = banana.scale;
            return;
        }
        if let Some(b) = ball_local {
            banana.hold_offset.x *= BANANA_HOLD_DAMP;
            banana.hold_offset.z *= BANANA_HOLD_DAMP;
            let t = (BANANA_HOLD_FRAMES - banana.hold_timer) as f64 / BANANA_HOLD_FRAMES as f64;
            let target_y = BANANA_HOLD_Y_BASE + (BANANA_HOLD_Y_RANGE * t);
            banana.hold_offset.y += (target_y - banana.hold_offset.y) * BANANA_HOLD_Y_LERP;
            banana.local_pos.x = b.x + banana.hold_offset.x;
            banana.local_pos.y = b.y + banana.hold_offset.y;
            banana.local_pos.z = b.z + banana.hold_offset.z;
        }
        let rot_delta = wrap_s16(BANANA_HOLD_ROT_TARGET - banana.hold_rot_vel);
        banana.hold_rot_vel = wrap_s16(banana.hold_rot_vel + (rot_delta >> 3));
        banana.rot_y = wrap_s16(banana.rot_y + banana.hold_rot_vel);
        banana.scale += (banana.hold_scale_target - banana.scale) * 0.2;
        return;
    }
    if banana.state == BANANA_STATE_FLY {
        let Some(target) = fly_target_local else {
            banana.local_pos.y += 0.02;
            banana.scale -= 0.01;
            if banana.scale <= 0.0 {
                banana.scale = 0.0;
                banana.state = 0;
                banana.collected = true;
            }
            return;
        };
        banana.fly_timer -= 1;
        let t = 1.0 - (banana.fly_timer.max(0) as f64) / BANANA_FLY_FRAMES as f64;
        let eased = 1.0 - cos_quarter_turn(t);
        let mut start = banana.fly_start_pos;
        if let Some(b) = ball_local {
            banana.local_pos.x = b.x + banana.hold_offset.x;
            banana.local_pos.y = b.y + banana.hold_offset.y;
            banana.local_pos.z = b.z + banana.hold_offset.z;
            start = banana.local_pos;
        }
        banana.local_pos.x = start.x + (target.x - start.x) * eased;
        banana.local_pos.y = start.y + (target.y - start.y) * eased;
        banana.local_pos.z = start.z + (target.z - start.z) * eased;
        banana.scale =
            banana.fly_start_scale + (banana.fly_scale_target - banana.fly_start_scale) * eased;
        banana.rot_y = wrap_s16(banana.rot_y + banana.hold_rot_vel);
        if banana.fly_timer <= 0 {
            banana.state = 0;
            banana.collected = true;
        }
        return;
    }
    // Generic states 1..=6 with fall-through (3 -> 4 -> 5).
    if banana.state == 1 {
        banana.state = 2;
    } else if (3..=5).contains(&banana.state) {
        if banana.state == 3 {
            banana.state = 4;
        }
        if banana.state == 4 {
            banana.state = 5;
            banana.collect_timer = BANANA_COLLECT_WAIT_FRAMES;
        }
        banana.collect_timer -= 1;
        if banana.collect_timer < 0 {
            banana.state = 6;
        }
    } else if banana.state == 6 {
        banana.scale -= BANANA_SHRINK_STEP;
        if banana.scale <= 0.0 {
            banana.scale = 0.0;
            banana.state = 0;
            banana.collected = true;
        }
    }

    banana.local_pos.x += banana.vel.x;
    banana.local_pos.y += banana.vel.y;
    banana.local_pos.z += banana.vel.z;
    banana.rot_x = wrap_s16(banana.rot_x + banana.rot_vel_x);
    banana.rot_y = wrap_s16(banana.rot_y + banana.rot_vel_y);
    banana.rot_z = wrap_s16(banana.rot_z + banana.rot_vel_z);
}

fn update_goal_tape(
    tape: &mut GoalTapeState,
    groups: &[AnimGroupState],
    gravity: &Vec3,
    stack: &mut MatrixStack,
) {
    let mut motion_delta = Vec3::ZERO;
    let mut has_group_motion = false;
    if tape.anim_group_id != 0 {
        let group = &groups[tape.anim_group_id];
        has_group_motion = true;
        stack.from_mtx(&group.prev_transform);
        let prev_world = stack.tf_point(&tape.goal_pos);
        stack.from_mtx(&group.transform);
        let curr_world = stack.tf_point(&tape.goal_pos);
        motion_delta = Vec3::new(
            prev_world.x - curr_world.x,
            prev_world.y - curr_world.y,
            prev_world.z - curr_world.z,
        );
        stack.from_rotate_z(tape.goal_rot[2]);
        stack.rotate_y(tape.goal_rot[1]);
        stack.rotate_x(tape.goal_rot[0]);
        motion_delta = stack.rigid_inv_tf_vec(&motion_delta);
    }

    let mut gravity_local = Vec3::new(
        GOAL_TAPE_GRAVITY_SCALE * gravity.x,
        GOAL_TAPE_GRAVITY_SCALE * gravity.y,
        GOAL_TAPE_GRAVITY_SCALE * gravity.z,
    );
    stack.from_mtx(&groups[tape.anim_group_id].transform);
    stack.translate(&tape.goal_pos);
    stack.rotate_z(tape.goal_rot[2]);
    stack.rotate_y(tape.goal_rot[1]);
    stack.rotate_x(tape.goal_rot[0]);
    gravity_local = stack.rigid_inv_tf_vec(&gravity_local);

    for point in tape.points.iter_mut() {
        point.prev_pos = point.pos;
        point.prev_normal = point.normal;
    }

    let n = tape.points.len();
    for i in 0..n {
        if tape.points[i].flags & 1 != 0 {
            continue;
        }
        let mut neighbor_sum = Vec3::ZERO;
        if tape.points[i].flags & 2 != 0 {
            let prev = tape.points[i - 1].clone();
            neighbor_sum.x += prev.normal.x;
            neighbor_sum.y += prev.normal.y;
            neighbor_sum.z += prev.normal.z;
            let point = &mut tape.points[i];
            let mut delta = Vec3::new(
                prev.pos.x - point.pos.x,
                prev.pos.y - point.pos.y,
                prev.pos.z - point.pos.z,
            );
            let len = vec_normalize_len(&mut delta);
            let mut target = prev.len;
            if len > target {
                target = 1.05 * (len - target);
                point.vel.x += delta.x * target;
                point.vel.y += delta.y * target;
                point.vel.z += delta.z * target;
            } else {
                target = -target;
                point.pos.x = prev.pos.x + delta.x * target;
                point.pos.y = prev.pos.y + delta.y * target;
                point.pos.z = prev.pos.z + delta.z * target;
                let dot = -vec_dot(&point.vel, &delta);
                point.vel.x += delta.x * dot;
                point.vel.y += delta.y * dot;
                point.vel.z += delta.z * dot;
            }
        }
        if tape.points[i].flags & 4 != 0 {
            let next = tape.points[i + 1].clone();
            neighbor_sum.x += next.normal.x;
            neighbor_sum.y += next.normal.y;
            neighbor_sum.z += next.normal.z;
            let point = &mut tape.points[i];
            let mut delta = Vec3::new(
                next.pos.x - point.pos.x,
                next.pos.y - point.pos.y,
                next.pos.z - point.pos.z,
            );
            let len = vec_normalize_len(&mut delta);
            let mut target = point.len;
            if len > target {
                target = 1.05 * (len - target);
                point.vel.x += delta.x * target;
                point.vel.y += delta.y * target;
                point.vel.z += delta.z * target;
            } else {
                target = -target;
                point.pos.x = next.pos.x + delta.x * target;
                point.pos.y = next.pos.y + delta.y * target;
                point.pos.z = next.pos.z + delta.z * target;
                let dot = -vec_dot(&point.vel, &delta);
                point.vel.x += delta.x * dot;
                point.vel.y += delta.y * dot;
                point.vel.z += delta.z * dot;
            }
        }

        let point = &mut tape.points[i];
        point.vel.x += gravity_local.x;
        point.vel.y += gravity_local.y;
        point.vel.z += gravity_local.z;
        stack.from_rotate_y(to_s16(49152.0 * (point.vel.x + point.vel.z)));
        stack.rotate_x(to_s16(98304.0 * point.vel.y));
        point.normal = stack.tf_vec(&point.normal);
        point.normal.x += 0.075 * (neighbor_sum.x - point.normal.x);
        point.normal.y += 0.075 * (neighbor_sum.y - point.normal.y);
        point.normal.z += 0.075 * (neighbor_sum.z - point.normal.z);
        vec_normalize_len(&mut point.normal);
    }

    if tape.anchor_y > tape.target_y {
        tape.anchor_y -= GOAL_TAPE_Y_STEP;
        if tape.anchor_y < tape.target_y {
            tape.anchor_y = tape.target_y;
        }
    } else if tape.anchor_y < tape.target_y {
        tape.anchor_y += GOAL_TAPE_Y_STEP;
        if tape.anchor_y > tape.target_y {
            tape.anchor_y = tape.target_y;
        }
    }

    tape.points[0].pos.y = tape.anchor_y;
    tape.points[n - 1].pos.y = tape.anchor_y;

    for point in tape.points.iter_mut() {
        if point.flags & 1 != 0 {
            continue;
        }
        if has_group_motion && (point.flags & 6) != 6 {
            point.vel.x += 0.15 * (motion_delta.x - point.vel.x);
            point.vel.y += 0.15 * (motion_delta.y - point.vel.y);
            point.vel.z += 0.15 * (motion_delta.z - point.vel.z);
        } else {
            point.vel.x *= 0.85;
            point.vel.y *= 0.85;
            point.vel.z *= 0.85;
        }
        point.pos.x += point.vel.x;
        point.pos.y += point.vel.y;
        point.pos.z += point.vel.z;
        if point.pos.y < tape.ground_y {
            point.pos.y = tape.ground_y;
            point.vel.y = 0.0;
            point.normal.x *= 0.9;
            point.normal.y += 0.1 * (1.0 - point.normal.y);
            point.normal.z *= 0.9;
            vec_normalize_len(&mut point.normal);
        }
    }
}
