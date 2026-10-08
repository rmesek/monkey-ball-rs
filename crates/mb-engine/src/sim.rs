//! Single-player simulation driver: the deterministic part of `Game.update` in WebMonkeyBall `game.ts`
//! (intro, tilt, stage objects, ball step, fall-out, banana pickup, goal entry, timers, gameplay camera),
//! one fixed 60 Hz tick per [`Sim::step`].
//!
//! Not modelled (render/UI/meta only): result replays, audio triggers, course progression. A finished sequence is reported through [`Phase::Finished`] instead of loading the next
//! stage.

use crate::ball::{check_ball_entered_goal, Ball, GoalHit, PhysicsCtx};
use crate::camera::GameplayCamera;
use crate::collision::{intersects_moving_spheres, PhysBall};
use crate::constants::*;
use crate::determinism::QuantizedInput;
use crate::geom::StageGeom;
use crate::hash::{hash_sim_state, hash_sim_state_visual};
use crate::math::*;
use crate::stage_runtime::{
    BoundSphere, CameraRef, StageRuntime, BANANA_HOLD_FRAMES, BANANA_STATE_HOLDING,
    GOAL_TAPE_ANCHOR_Y,
};
use crate::world::World;
use mb_stage::{GoalType, Stage};

const DROP_FRAMES: i32 = 24;
const ITEM_FLAG_COLLIDABLE: i32 = 1 << 1;
const BONUS_STAGE_MIN_ID: u32 = 91;
const BONUS_STAGE_MAX_ID: u32 = 95;
const BONUS_CLEAR_SEQUENCE_FRAMES: i32 = 180;
const BONUS_FALLOUT_SPECTATE_FRAMES: i32 = 120;

#[derive(Debug, Clone)]
pub struct SimConfig {
    pub stage_id: u32,
    /// Intro length: 360 on the first attempt, 120 on retries (the reference's `stageAttempts`).
    pub intro_frames: i32,
    /// Camera fly-in length: 350 on the first attempt, 90 on retries.
    pub fly_in_frames: i32,
    /// Time limit in frames (`course.getTimeLimitFrames()`, default one minute).
    pub time_limit_frames: i32,
    /// Bonus stages end on collecting every banana and fall out faster.
    pub bonus_stage: bool,
    /// Goal tape anchor height (the reference reads it from the common model; default is its fallback).
    pub goal_tape_anchor_y: f64,
    /// Replace the collision-derived fly-in bounds (the reference uses the stage model's bounds).
    pub bound_sphere: Option<BoundSphere>,
    /// Simulate the display-only particles (`runtime.effects`, `runtime.confetti`). Off is the reference's
    /// `suppressVisualEffects`; the simulation itself is the same either way.
    pub visual_effects: bool,
}

impl SimConfig {
    pub fn first_attempt(stage_id: u32) -> SimConfig {
        SimConfig {
            stage_id,
            intro_frames: 360,
            fly_in_frames: 350,
            time_limit_frames: DEFAULT_STAGE_TIME,
            bonus_stage: (BONUS_STAGE_MIN_ID..=BONUS_STAGE_MAX_ID).contains(&stage_id),
            goal_tape_anchor_y: GOAL_TAPE_ANCHOR_Y,
            bound_sphere: None,
            visual_effects: true,
        }
    }

    pub fn retry(stage_id: u32) -> SimConfig {
        SimConfig {
            intro_frames: 120,
            fly_in_frames: 90,
            ..SimConfig::first_attempt(stage_id)
        }
    }
}

/// How a finished run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Goal(GoalType),
    BonusClear,
    FellOut,
    TimeOver,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Fly-in and "ready" countdown; input is ignored.
    Intro,
    Playing,
    /// Ball reached the goal; the celebration sequence is running.
    GoalSequence,
    /// Ball fell out; the ring-out sequence is running.
    FallOut,
    TimeOver,
    /// The running sequence ended; the reference would now advance the course or reload the stage.
    Finished(Outcome),
}

fn goal_letter(t: GoalType) -> char {
    match t {
        GoalType::Blue => 'B',
        GoalType::Green => 'G',
        GoalType::Red => 'R',
    }
}

/// `computeGoalScore`
pub fn compute_goal_score(goal: GoalType, frames_remaining: i32, time_limit_frames: i32) -> i64 {
    let time_remaining = frames_remaining.max(0) as i64;
    let mut score = (time_remaining * 100) / 60;
    match goal_letter(goal) {
        'G' => score += 10000,
        'R' => score += 20000,
        _ => {}
    }
    let mut jump = 1;
    if time_limit_frames > 0 && time_remaining > (time_limit_frames >> 1) as i64 {
        jump *= 2;
    }
    score * jump
}

/// Something that happened during one [`Sim::step`] that the presentation layer reacts to (sounds,
/// effects). The reference triggers these inline from `game.ts`; here they are collected instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SimEvent {
    /// A banana (or a bunch) was picked up (`playBananaCollect`).
    BananaCollected { bunch: bool },
    /// The ball touched a bumper (`ball.audio.bumperHit`).
    BumperHit,
    /// The ball entered a goal and broke its tape (`playGoal`).
    GoalEntered(GoalType),
    /// The ball starts flying away after the goal float (`ball_woosh`).
    GoalLaunch,
    /// The ring-out sequence began (`playFallout`).
    FallOut,
    /// Time ran out (`playAnnouncerTimeOver`).
    TimeOver,
    /// Every banana of a bonus stage is collected (`playAnnouncerPerfect`).
    BonusClear,
}

#[derive(Debug, Clone)]
pub struct Sim {
    pub cfg: SimConfig,
    pub geom: StageGeom,
    pub runtime: StageRuntime,
    pub ball: Ball,
    /// The player's tilt state.
    pub world: World,
    /// The shared stage world whose gravity (average of the players') drives stage objects.
    pub shared_world: World,
    pub camera: GameplayCamera,
    ctx: PhysicsCtx,
    pub tick: u32,
    pub phase: Phase,
    pub intro_timer: i32,
    pub stage_timer_frames: i32,
    pub goal_timer: i32,
    pub ringout_timer: i32,
    pub timeover_timer: i32,
    pub finished: bool,
    pub goal_hit: Option<GoalHit>,
    pub bananas_left: i32,
    pub score: i64,
    bonus_clear_pending: bool,
    /// What happened in the last [`Sim::step`], in order. Cleared at the start of every step; output only
    /// (never read by the simulation, not part of [`Sim::hash`]).
    pub events: Vec<SimEvent>,
    goal_launch_reported: bool,
}

impl Sim {
    /// Build the world for a parsed stage, mirroring `Game.loadStage` + `resetBallForStage(withIntro)`.
    pub fn new(stage: &Stage, cfg: SimConfig) -> Sim {
        let geom = StageGeom::from_stage(stage, cfg.stage_id);
        let mut runtime = StageRuntime::new(&geom, cfg.goal_tape_anchor_y);
        runtime.visuals_enabled = cfg.visual_effects;
        runtime.reset_timer();
        runtime.update_anim_groups(&geom, 0.0);
        runtime.compute_bound_sphere(&geom);
        if let Some(b) = cfg.bound_sphere {
            runtime.bound_sphere = b;
        }

        let (start_pos, start_rot) = geom
            .start_positions
            .first()
            .copied()
            .unwrap_or((Vec3::ZERO, [0, 0, 0]));
        let start_rot_y = start_rot[1];

        let mut ctx = PhysicsCtx::new();
        let mut ball = Ball::new();
        ball.init_for_stage(start_pos, start_rot_y, &mut ctx);
        ball.bananas = 0;

        let mut camera = GameplayCamera::new();
        camera.init_ready(
            runtime.fly_in_sphere(),
            start_rot_y,
            start_pos,
            cfg.fly_in_frames,
        );

        let bananas_left = runtime.bananas.len() as i32;
        let intro_timer = cfg.intro_frames;
        Sim {
            cfg,
            geom,
            runtime,
            ball,
            world: World::new(),
            shared_world: World::new(),
            camera,
            ctx,
            tick: 0,
            phase: Phase::Intro,
            intro_timer,
            stage_timer_frames: 0,
            goal_timer: 0,
            ringout_timer: 0,
            timeover_timer: 0,
            finished: false,
            goal_hit: None,
            bananas_left,
            score: 0,
            bonus_clear_pending: false,
            events: Vec::with_capacity(8),
            goal_launch_reported: false,
        }
    }

    pub fn is_finished(&self) -> bool {
        matches!(self.phase, Phase::Finished(_))
    }

    /// `hashSimState([ball], [sharedWorld, playerWorld], stageRuntime)`.
    pub fn hash(&self) -> u32 {
        hash_sim_state(
            &[&self.ball],
            &[&self.shared_world, &self.world],
            &self.runtime,
        )
    }

    /// `hashSimState(.., { includeVisual: true })`: [`Sim::hash`] extended by the confetti and ball effects.
    pub fn hash_visual(&self) -> u32 {
        hash_sim_state_visual(
            &[&self.ball],
            &[&self.shared_world, &self.world],
            &self.runtime,
        )
    }

    fn finish(&mut self, outcome: Outcome) {
        self.phase = Phase::Finished(outcome);
    }

    /// `isBallFalloutForBall`
    pub fn ball_fell_out(&mut self) -> bool {
        if self.ball.pos.y < self.geom.fallout_y {
            return true;
        }
        // Not in the reference: a position that is no longer a number (only a corrupt stage produces one)
        // compares false against every bound, so the ball would never fall out and the attempt would hang
        // until the clock ends it, or forever on a stage without a time limit.
        let p = &self.ball.pos;
        if !(p.x.is_finite() && p.y.is_finite() && p.z.is_finite()) {
            return true;
        }
        let mut phys = PhysBall::new();
        phys.pos = self.ball.pos;
        phys.prev_pos = self.ball.prev_pos;
        phys.vel = self.ball.vel;
        phys.anim_group_id = self.ball.phys_ball.anim_group_id;

        let mut stack = MatrixStack::new();
        for (group_id, ag) in self.geom.groups.iter().enumerate() {
            if ag.fallout_boxes.is_empty() {
                continue;
            }
            if group_id != phys.anim_group_id {
                self.ctx.collider.tf_physball_to_group(
                    &mut phys,
                    group_id,
                    &self.runtime.anim_groups,
                );
            }
            for b in &ag.fallout_boxes {
                stack.from_translate(&b.pos);
                stack.rotate_z(b.rot[2]);
                stack.rotate_y(b.rot[1]);
                stack.rotate_x(b.rot[0]);
                let local = stack.rigid_inv_tf_point(&phys.pos);
                if b.scale.x.abs() < 1e-7 || b.scale.y.abs() < 1e-7 || b.scale.z.abs() < 1e-7 {
                    continue;
                }
                let lx = local.x / b.scale.x;
                let ly = local.y / b.scale.y;
                let lz = local.z / b.scale.z;
                if !(-0.5..=0.5).contains(&lx)
                    || !(-0.5..=0.5).contains(&ly)
                    || !(-0.5..=0.5).contains(&lz)
                {
                    continue;
                }
                if phys.anim_group_id != 0 {
                    self.ctx
                        .collider
                        .tf_physball_to_group(&mut phys, 0, &self.runtime.anim_groups);
                }
                return true;
            }
        }
        if phys.anim_group_id != 0 {
            self.ctx
                .collider
                .tf_physball_to_group(&mut phys, 0, &self.runtime.anim_groups);
        }
        false
    }

    /// `collectBananas`
    fn collect_bananas(&mut self) {
        let mut phys = PhysBall::new();
        for bi in 0..self.runtime.bananas.len() {
            {
                let banana = &self.runtime.bananas[bi];
                if banana.collected {
                    continue;
                }
                if banana.flags & ITEM_FLAG_COLLIDABLE == 0 || banana.cooldown > 0 {
                    continue;
                }
            }
            phys.pos = self.ball.pos;
            phys.prev_pos = self.ball.prev_pos;
            phys.vel = self.ball.vel;
            phys.radius = self.ball.curr_radius;
            phys.anim_group_id = 0;
            let group_id = self.runtime.bananas[bi].anim_group_id;
            if phys.anim_group_id != group_id {
                self.ctx.collider.tf_physball_to_group(
                    &mut phys,
                    group_id,
                    &self.runtime.anim_groups,
                );
            }
            let banana = &mut self.runtime.bananas[bi];
            let is_bunch = banana.kind & 1 == 1;
            let radius = if is_bunch { 0.75 } else { 0.5 };
            if !intersects_moving_spheres(
                &phys.prev_pos,
                &phys.pos,
                &banana.prev_local_pos,
                &banana.local_pos,
                phys.radius,
                radius,
            ) {
                continue;
            }
            banana.cooldown = 8;
            banana.flags &= !ITEM_FLAG_COLLIDABLE;
            banana.state = BANANA_STATE_HOLDING;
            banana.collect_timer = 0;
            banana.hold_timer = BANANA_HOLD_FRAMES;
            banana.hold_offset = Vec3::new(
                banana.local_pos.x - phys.pos.x,
                banana.local_pos.y - phys.pos.y,
                banana.local_pos.z - phys.pos.z,
            );
            banana.hold_scale_target = banana.scale * 0.5;
            banana.hold_rot_vel = 0;
            banana.fly_timer = 0;
            banana.fly_scale_target = 0.0;
            banana.tilt_timer = 30;
            banana.vel = Vec3::ZERO;
            if self.bananas_left > 0 {
                self.bananas_left -= 1;
            }
            self.events
                .push(SimEvent::BananaCollected { bunch: is_bunch });
            self.ball.bananas += if is_bunch { 10 } else { 1 };
            self.score += if is_bunch { 1000 } else { 100 };
        }
    }

    /// `breakGoalTapeForBall`
    fn break_goal_tape(&mut self, hit: &GoalHit) {
        if self.runtime.goal_bags.is_empty() {
            return;
        }
        let mut temp = PhysBall::new();
        temp.pos = self.ball.pos;
        temp.prev_pos = self.ball.prev_pos;
        temp.vel = self.ball.vel;
        temp.anim_group_id = 0;
        self.ctx.collider.tf_physball_to_group(
            &mut temp,
            hit.anim_group_id,
            &self.runtime.anim_groups,
        );
        self.runtime.break_goal_tape(hit.goal_id, &temp);
    }

    fn begin_goal_sequence(&mut self, hit: GoalHit) {
        self.goal_hit = Some(hit);
        self.finished = true;
        let remaining = self
            .cfg
            .time_limit_frames
            .saturating_sub(self.stage_timer_frames)
            .max(0);
        self.score = (self.score
            + compute_goal_score(hit.goal_type, remaining, self.cfg.time_limit_frames))
        .min(999_999_999);
        self.goal_timer = GOAL_SEQUENCE_FRAMES;
        self.ball.start_goal();
        self.camera.set_goal_main();
        self.break_goal_tape(&hit);
        self.events.push(SimEvent::GoalEntered(hit.goal_type));
        self.phase = Phase::GoalSequence;
    }

    fn begin_fallout_sequence(&mut self) {
        if self.ringout_timer > 0 {
            return;
        }
        self.ringout_timer = if self.cfg.bonus_stage {
            BONUS_FALLOUT_SPECTATE_FRAMES
        } else {
            RINGOUT_TOTAL_FRAMES
        };
        self.camera.set_fallout_main(&self.ball);
        self.events.push(SimEvent::FallOut);
        self.phase = Phase::FallOut;
    }

    fn begin_timeover_sequence(&mut self) {
        if self.timeover_timer > 0 {
            return;
        }
        self.timeover_timer = TIMEOVER_TOTAL_FRAMES;
        self.ball.state = ball_states::READY;
        self.ball.flags |= ball_flags::TIMEOVER;
        self.ball.vel = Vec3::ZERO;
        self.ball.prev_pos = self.ball.pos;
        self.ball.speed = 0.0;
        self.events.push(SimEvent::TimeOver);
        self.phase = Phase::TimeOver;
    }

    fn begin_bonus_clear_sequence(&mut self) {
        if self.goal_timer > 0 {
            return;
        }
        self.bonus_clear_pending = true;
        self.goal_timer = BONUS_CLEAR_SEQUENCE_FRAMES;
        self.ball.start_goal();
        self.events.push(SimEvent::BonusClear);
        self.phase = Phase::GoalSequence;
    }

    /// Advance exactly one fixed 60 Hz tick with this frame's quantized stick input.
    pub fn step(&mut self, input: QuantizedInput) {
        self.events.clear();
        if self.is_finished() {
            return;
        }
        self.ball.audio.bumper_hits = 0;
        self.step_inner(input);
        if self.ball.audio.bumper_hits > 0 {
            self.events.push(SimEvent::BumperHit);
        }
        // `goalWooshPlayed`: once per goal sequence, when the float ends.
        if self.goal_timer > 0
            && !self.goal_launch_reported
            && self.ball.goal_timer >= GOAL_FLOAT_FRAMES
        {
            self.goal_launch_reported = true;
            self.events.push(SimEvent::GoalLaunch);
        }
        self.tick += 1;
    }

    fn step_inner(&mut self, input: QuantizedInput) {
        let timeover_active = self.timeover_timer > 0;
        let ringout_active = self.ringout_timer > 0;
        let stage_input_enabled = self.intro_timer <= 0 && !timeover_active;
        let local_input_enabled =
            stage_input_enabled && self.goal_timer <= 0 && !ringout_active && !self.finished;
        let is_bonus = self.cfg.bonus_stage;
        let anim_timer_override = if self.intro_timer > 0 {
            Some((120 - self.intro_timer).max(0))
        } else {
            None
        };
        self.runtime.goal_hold_open = self.goal_timer > 0;

        // Tilt from the stick, relative to the camera yaw.
        let camera_rot_y = self.camera.rot_y;
        let (sx, sy) = if local_input_enabled {
            input.dequantize()
        } else {
            (0.0, 0.0)
        };
        self.world.update_input(sx, sy, camera_rot_y);
        let mut tilt_count = 0;
        let (mut avg_x, mut avg_y, mut avg_z) = (0.0, 0.0, 0.0);
        if !self.finished {
            tilt_count += 1;
            avg_x += self.world.gravity.x;
            avg_y += self.world.gravity.y;
            avg_z += self.world.gravity.z;
        }
        if tilt_count > 0 {
            let inv = 1.0 / tilt_count as f64;
            avg_x *= inv;
            avg_y *= inv;
            avg_z *= inv;
            let len = sqrt(avg_x * avg_x + avg_y * avg_y + avg_z * avg_z);
            if len > 1e-6 {
                self.shared_world.gravity = Vec3::new(avg_x / len, avg_y / len, avg_z / len);
            }
        } else {
            self.shared_world.gravity = Vec3::new(0.0, -1.0, 0.0);
        }

        if !timeover_active {
            let cam = CameraRef {
                eye: self.camera.eye,
                rot_x: self.camera.rot_x,
                rot_y: self.camera.rot_y,
                rot_z: self.camera.rot_z,
            };
            self.runtime.advance(
                &self.geom,
                self.shared_world.gravity,
                anim_timer_override,
                Some(self.ball.pos),
                Some(cam),
            );
        }

        if self.goal_timer > 0 {
            self.goal_timer -= 1;
            if self.ball.flags & ball_flags::FLAG_09 != 0
                && !self.bonus_clear_pending
                && self.goal_timer > 60
                && self.goal_timer < 240
            {
                // SMB1 goal-main: once the launch event has happened, clamp to the final 60 frames.
                self.goal_timer = 60;
            }
            if self.goal_timer <= 0 {
                self.goal_timer = 0;
                let outcome = if self.bonus_clear_pending {
                    Outcome::BonusClear
                } else {
                    Outcome::Goal(self.goal_hit.map(|h| h.goal_type).unwrap_or(GoalType::Blue))
                };
                self.finish(outcome);
                return;
            }
        }

        if self.intro_timer > 0 {
            self.intro_timer -= 1;
            if self.intro_timer == DROP_FRAMES {
                self.ball.start_drop(DROP_FRAMES, &mut self.ctx);
            }
            if self.intro_timer == 0 {
                self.camera
                    .init_for_stage(&self.ball, self.ball.start_rot_y);
                // The ball is live for the last frames of the intro (the drop), so a sequence can already
                // have begun on a broken stage; do not report that as normal play.
                if self.phase == Phase::Intro {
                    self.phase = Phase::Playing;
                }
            }
        }

        if !timeover_active {
            if self.ball.flags & ball_flags::INVISIBLE == 0
                && (self.ball.state == ball_states::PLAY
                    || self.ball.state == ball_states::GOAL_MAIN)
            {
                self.ball
                    .step(&self.geom, &mut self.runtime, &self.world, &mut self.ctx);
                if !self.finished && !ringout_active && self.goal_timer <= 0 && self.ball_fell_out()
                {
                    self.begin_fallout_sequence();
                }
            }

            let timer_should_run = local_input_enabled && self.ringout_timer <= 0;
            if timer_should_run {
                self.stage_timer_frames += 1;
                if self.cfg.time_limit_frames > 0 {
                    let time_left = self.cfg.time_limit_frames - self.stage_timer_frames;
                    if time_left <= 0 && self.timeover_timer <= 0 {
                        self.begin_timeover_sequence();
                        return;
                    }
                }
            }
        }

        let has_playable_ball =
            self.ball.state == ball_states::PLAY || self.ball.state == ball_states::GOAL_MAIN;
        if !ringout_active && !timeover_active && has_playable_ball {
            self.collect_bananas();
        }

        let block_bonus_clear = self.goal_timer > 0;
        let bonus_finish_ready = is_bonus
            && stage_input_enabled
            && !block_bonus_clear
            && (self.bananas_left <= 0 || self.finished);
        if bonus_finish_ready && !self.bonus_clear_pending {
            self.begin_bonus_clear_sequence();
            return;
        }

        // A position that is not a number fails every "outside the goal" test, so it would count as a goal.
        let ball_is_somewhere = self.ball.pos.x.is_finite()
            && self.ball.pos.y.is_finite()
            && self.ball.pos.z.is_finite();
        if stage_input_enabled
            && !self.finished
            && ball_is_somewhere
            && self.ball.flags & ball_flags::INVISIBLE == 0
            && self.ball.state == ball_states::PLAY
        {
            if let Some(hit) = check_ball_entered_goal(
                &mut self.ball,
                &self.geom,
                &self.runtime,
                &mut self.ctx.collider,
            ) {
                self.begin_goal_sequence(hit);
            }
        }

        if ringout_active {
            self.ringout_timer -= 1;
            if self.ringout_timer <= 0 {
                self.ringout_timer = 0;
                self.finish(Outcome::FellOut);
                return;
            }
        }
        if timeover_active {
            self.timeover_timer -= 1;
            if self.timeover_timer <= 0 {
                self.timeover_timer = 0;
                self.finish(Outcome::TimeOver);
                return;
            }
        }

        self.camera.update(&self.ball, timeover_active, false);
    }
}
