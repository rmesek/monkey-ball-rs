//! Port of WebMonkeyBall `src/effects.ts` (ball sparks, collision stars and flash, post-goal sparkle) and of
//! the goal-bag confetti in `src/stage.ts` (`spawnGoalBagConfetti`, `updateConfetti`).
//!
//! Display only. Nothing here is read by the simulation: the particles draw from the stage's separate visual
//! random stream, work in their own matrix stack and collision scratch, and are left out of
//! [`crate::hash::hash_sim_state`] (they are in [`crate::hash::hash_sim_state_visual`], the reference's
//! `includeVisual` hash). Operation order follows the reference so the state is bit-identical to it.

use crate::ball::Ball;
use crate::collision::{Collider, RayHit};
use crate::constants::{COLI_OCCURRED, ball_states};
use crate::geom::StageGeom;
use crate::math::*;
use crate::objects::{AnimGroupState, GoalBagState};
use crate::rng::DeterministicRng;

const SPARK_GRAVITY_SCALE: f64 = 0.008;
const SPARK_DAMP: f64 = 0.992;
const STAR_GRAVITY_SCALE: f64 = 0.004;
const STAR_DAMP: f64 = 0.985;
const SPARK_LIFE_MIN: f64 = 15.0;
const SPARK_LIFE_RANGE: f64 = 45.0;
const STAR_LIFE_MIN: f64 = 15.0;
const STAR_LIFE_RANGE: f64 = 45.0;
const LEVITATE_LIFE_MIN: f64 = 60.0;
const LEVITATE_LIFE_RANGE: f64 = 15.0;
const SPARK_GROUND_CHECK: f64 = 0.5;
const STAR_GROUND_CHECK: f64 = 0.6;
const SPARK_GROUND_MIN_DIST: f64 = 0.025;
const STAR_GROUND_MIN_DIST_SCALE: f64 = 8.090365;
const SPARK_GROUND_BLEND: f64 = 0.03;
const STAR_GROUND_BLEND: f64 = 0.029999971;
const STAR_ROT_KICK_XZ: f64 = 131072.0;
const STAR_ROT_KICK_Y: f64 = 524288.0;
const LEVITATE_GRAVITY: f64 = 0.0004;
const LEVITATE_START_VEL: f64 = 0.003;
const LEVITATE_SCALE: f64 = 0.24;
const LEVITATE_OFFSET_RADIUS: f64 = 0.7;
const LEVITATE_DRIFT: f64 = 0.0012;
const LEVITATE_DAMP: f64 = 0.98;
const STAR_SCALE_TARGET: f64 = 0.015;
/// Life of a collision flash in ticks.
const FLASH_LIFE: i32 = 12;
/// `hardestColiSpeed` below which a collision throws stars.
const STAR_COLI_SPEED: f64 = -0.06;
/// Most stars and sparks one collision throws (`Math.min(32, ...)`), plus its flash.
const STAR_BURST_MAX: i32 = 32;

const CONFETTI_GRAVITY_SCALE: f64 = 0.004;
const CONFETTI_VEL_DAMP: f64 = 0.95;
const CONFETTI_ROT_SPEED_SCALE: f64 = 2560.0;
const CONFETTI_GROUND_CHECK: f64 = 1.0;
const CONFETTI_BOUNCE: f64 = -0.7;
const CONFETTI_LIFE_BASE: f64 = 210.0;
const CONFETTI_LIFE_RANGE: f64 = 60.0;
/// Number of confetti models the pieces cycle through (`ConfettiParticle::model_index` is below this).
pub const CONFETTI_MODEL_COUNT: usize = 5;
/// Pieces thrown by one goal bag (`spawnCount`).
pub const CONFETTI_PER_GOAL: usize = 160;

/// Capacity of the ball effect pool, allocated once per stage and never grown.
///
/// The reference's array has no limit: every tick on the ground adds `trunc(5 * speed)` sparks that live at
/// most 59 ticks, and a hard collision adds at most 33 effects (one flash, 16 stars, 16 sparks). 1024 live
/// effects need a sustained ground speed above 5 units per tick, the speed of a fall of some 1500 units;
/// the stages are a few hundred units across and the fall-out plane ends the attempt long before. Spawns
/// that find the pool full are dropped (without drawing random numbers), which is also what keeps a ball
/// with an infinite speed on a corrupt stage from spawning forever as it would in the reference.
pub const MAX_BALL_EFFECTS: usize = 1024;

/// `BallEffectKind` of the reference: `'coli' | 'colistar' | 'coliflash' | 'levitate'`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BallEffectKind {
    /// Spark: drawn as a streak from `prev_pos` to the blended position, with a ground glow.
    Coli,
    /// Collision star: a small streak plus the spinning star model, with a ground glow.
    ColiStar,
    /// Collision flash: a disc on the surface that was hit (`surface_normal`), with a ground glow.
    ColiFlash,
    /// Post-goal sparkle rising around the ball.
    Levitate,
}

/// One live ball effect (`BallEffect` in `effects.ts`). Positions are world space, angles S16.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BallEffect {
    /// Unique within one [`crate::Sim`], counting from 1 in spawn order (the reference counts across stages).
    pub id: u32,
    pub kind: BallEffectKind,
    pub pos: Vec3,
    /// Position one tick ago: the blend source, and the tail of a streak.
    pub prev_pos: Vec3,
    pub ground_pos: Vec3,
    pub ground_normal: Vec3,
    pub ground_vel: Vec3,
    /// Where the ground glow is drawn. Only meaningful while `has_ground`.
    pub glow_pos: Vec3,
    /// Smoothed height above the ground; the glow fades with it.
    pub glow_dist: f64,
    pub glow_rot_x: i32,
    /// Not wrapped to 16 bits (`atan2S16(..) - 0x8000`).
    pub glow_rot_y: i32,
    pub has_ground: bool,
    /// Normal of the surface a flash lies on (the ball's contact direction, pointing into the surface).
    pub surface_normal: Vec3,
    pub vel: Vec3,
    /// Ticks left; the effect is removed when this reaches 0.
    pub life: i32,
    /// Ticks lived.
    pub age: i32,
    pub scale: f64,
    pub scale_target: f64,
    pub alpha: f64,
    pub color_r: f64,
    pub color_g: f64,
    pub color_b: f64,
    pub rot_x: i32,
    pub rot_y: i32,
    pub rot_z: i32,
    pub prev_rot_x: i32,
    pub prev_rot_y: i32,
    pub prev_rot_z: i32,
    pub rot_vel_x: i32,
    pub rot_vel_y: i32,
    pub rot_vel_z: i32,
    pub base_y: f64,
}

impl BallEffect {
    /// `createEffect`
    fn new(id: u32, kind: BallEffectKind, pos: Vec3, vel: Vec3, life: i32) -> BallEffect {
        BallEffect {
            id,
            kind,
            pos,
            prev_pos: pos,
            ground_pos: pos,
            ground_normal: Vec3::new(0.0, 1.0, 0.0),
            ground_vel: Vec3::ZERO,
            glow_pos: pos,
            glow_dist: 0.0,
            glow_rot_x: 0,
            glow_rot_y: 0,
            has_ground: false,
            surface_normal: Vec3::new(0.0, 1.0, 0.0),
            vel,
            life,
            age: 0,
            scale: 1.0,
            scale_target: 1.0,
            alpha: 1.0,
            color_r: 1.0,
            color_g: 1.0,
            color_b: 1.0,
            rot_x: 0,
            rot_y: 0,
            rot_z: 0,
            prev_rot_x: 0,
            prev_rot_y: 0,
            prev_rot_z: 0,
            rot_vel_x: 0,
            rot_vel_y: 0,
            rot_vel_z: 0,
            base_y: pos.y,
        }
    }

    /// `life / max(1, life + age)`: the remaining share of the effect's life, as the reference's renderer
    /// uses it for streaks.
    pub fn life_ratio(&self) -> f64 {
        self.life as f64 / (self.life + self.age).max(1) as f64
    }
}

/// One piece of goal confetti (`createConfettiParticle` in `stage.ts`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConfettiParticle {
    /// Which of the [`CONFETTI_MODEL_COUNT`] models to draw.
    pub model_index: usize,
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub vel: Vec3,
    pub rot_x: i32,
    pub rot_y: i32,
    pub rot_z: i32,
    pub prev_rot_x: i32,
    pub prev_rot_y: i32,
    pub prev_rot_z: i32,
    pub rot_vel_x: i32,
    pub rot_vel_y: i32,
    pub rot_vel_z: i32,
    pub scale: f64,
    /// Ticks left; the piece is removed when this reaches 0.
    pub life: i32,
    pub on_ground: bool,
    pub ground_pos: Vec3,
    pub ground_normal: Vec3,
    pub ground_vel: Vec3,
    pub ground_bias: f64,
}

/// `Math.max` (a NaN operand gives NaN, +0 beats -0); `f64::max` differs on both.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b || (a == b && a.is_sign_positive()) {
        a
    } else {
        b
    }
}

/// `Math.min` (a NaN operand gives NaN, -0 beats +0).
fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b || (a == b && a.is_sign_negative()) {
        a
    } else {
        b
    }
}

/// `Math.trunc(x)` used as a count or a life: NaN counts as nothing, larger values saturate.
fn trunc_i32(v: f64) -> i32 {
    v.trunc() as i32
}

/// What the spawn functions write to: the pool, the visual random stream and the id counter.
pub(crate) struct Spawner<'a> {
    pub effects: &'a mut Vec<BallEffect>,
    pub rng: &'a mut DeterministicRng,
    pub next_id: &'a mut u32,
}

impl Spawner<'_> {
    fn full(&self) -> bool {
        self.effects.len() >= MAX_BALL_EFFECTS
    }

    fn create(&mut self, kind: BallEffectKind, pos: Vec3, vel: Vec3, life: i32) -> BallEffect {
        let id = *self.next_id;
        *self.next_id = id.wrapping_add(1);
        BallEffect::new(id, kind, pos, vel, life)
    }
}

/// The contact point and the direction away from the surface, as both collision spawns compute them.
fn contact(ball: &Ball) -> (Vec3, Vec3) {
    let surface_normal = ball.unk114;
    let normal = Vec3::new(-surface_normal.x, -surface_normal.y, -surface_normal.z);
    let base_pos = Vec3::new(
        ball.pos.x + surface_normal.x * ball.curr_radius,
        ball.pos.y + surface_normal.y * ball.curr_radius,
        ball.pos.z + surface_normal.z * ball.curr_radius,
    );
    (base_pos, normal)
}

/// The effect block of `stepBall`: called once per ball step, after the stage collision.
pub(crate) fn spawn_for_ball_step(out: &mut Spawner, ball: &Ball) {
    let phys = &ball.phys_ball;
    let collided = phys.flags & COLI_OCCURRED != 0;
    let on_ground = collided && phys.hardest_coli_plane.normal.y > 0.0;
    spawn_movement_sparks(out, ball, on_ground);
    if collided && phys.hardest_coli_speed < STAR_COLI_SPEED {
        spawn_collision_stars(out, ball, phys.hardest_coli_speed);
    }
    if ball.state == ball_states::GOAL_MAIN && ball.unk80 & 1 != 0 {
        spawn_post_goal_sparkle(out, ball);
    }
}

/// `spawnMovementSparks`
fn spawn_movement_sparks(out: &mut Spawner, ball: &Ball, on_ground: bool) {
    if !on_ground {
        return;
    }
    let speed = sqrt(ball.vel.x * ball.vel.x + ball.vel.y * ball.vel.y + ball.vel.z * ball.vel.z);
    let intensity = speed * 5.0;
    if intensity <= 1.5 {
        return;
    }
    let (base_pos, normal) = contact(ball);
    let f2 = 0.85;
    let base_vel = Vec3::new(ball.vel.x * f2, ball.vel.y * f2, ball.vel.z * f2);
    let mut count = trunc_i32(intensity);
    while count > 0 && !out.full() {
        let life = trunc_i32(SPARK_LIFE_MIN + SPARK_LIFE_RANGE * out.rng.next_float());
        let mut spark = out.create(BallEffectKind::Coli, base_pos, base_vel, life);
        let spread = out.rng.next_float() * intensity * 0.1;
        spark.vel.x += (normal.x + (out.rng.next_float() * 1.5 - 0.75)) * spread;
        spark.vel.y += (normal.y + (out.rng.next_float() * 1.5 - 0.75)) * spread;
        spark.vel.z += (normal.z + (out.rng.next_float() * 1.5 - 0.75)) * spread;
        spark.scale = 1.0;
        spark.scale_target = 1.0;
        spark.color_r = 1.1;
        spark.color_g = 1.0;
        spark.color_b = 0.6;
        out.effects.push(spark);
        count -= 1;
    }
}

/// `spawnCollisionStars`
fn spawn_collision_stars(out: &mut Spawner, ball: &Ball, hardest_coli_speed: f64) {
    let (base_pos, normal) = contact(ball);
    let mut base_vel = Vec3::new(ball.vel.x * 0.5, ball.vel.y * 0.5, ball.vel.z * 0.5);
    let proj = -(normal.x * base_vel.x + normal.y * base_vel.y + normal.z * base_vel.z);
    base_vel.x += proj * normal.x;
    base_vel.y += proj * normal.y;
    base_vel.z += proj * normal.z;

    let raw_count = (hardest_coli_speed / 0.0165).abs();
    // `Math.min(32, NaN)` is NaN, which the reference's `>>` and `>` then treat as no stars at all.
    let mut count = if raw_count.is_nan() {
        0
    } else {
        trunc_i32(raw_count).min(STAR_BURST_MAX)
    };
    let scale_boost = (hardest_coli_speed / 0.33).abs() + 1.0;

    if out.full() {
        return;
    }
    let flash_scale = sqrt((hardest_coli_speed * 10.0).abs());
    let mut flash = out.create(BallEffectKind::ColiFlash, base_pos, Vec3::ZERO, FLASH_LIFE);
    flash.scale = flash_scale * 0.25;
    flash.scale_target = flash_scale;
    flash.surface_normal = ball.unk114;
    out.effects.push(flash);

    let mut star_count = count >> 1;
    while star_count > 0 && !out.full() {
        let life = trunc_i32(STAR_LIFE_MIN + STAR_LIFE_RANGE * out.rng.next_float());
        let mut star = out.create(BallEffectKind::ColiStar, base_pos, base_vel, life);
        let jitter = Vec3::new(
            scale_boost * (out.rng.next_float() * 0.05 - 0.025),
            scale_boost * (out.rng.next_float() * 0.05 - 0.025),
            scale_boost * (out.rng.next_float() * 0.05 - 0.025),
        );
        let push = scale_boost * (out.rng.next_float() * 0.055 + 0.015);
        star.vel.x += jitter.x + push * normal.x;
        star.vel.y += jitter.y + push * normal.y;
        star.vel.z += jitter.z + push * normal.z;
        star.rot_x = 0;
        star.rot_y = out.rng.next_s16();
        star.rot_z = out.rng.next_s16();
        star.rot_vel_x = 0;
        star.rot_vel_y = (out.rng.next_s16() & 0xfff) + 0x1000;
        star.rot_vel_z = 0;
        star.scale = 0.0;
        star.scale_target = STAR_SCALE_TARGET;
        out.effects.push(star);
        star_count -= 1;
    }

    count -= count >> 1;
    let spark_vel = Vec3::new(base_vel.x * 0.5, base_vel.y * 0.5, base_vel.z * 0.5);
    while count > 0 && !out.full() {
        let life = trunc_i32(SPARK_LIFE_MIN + SPARK_LIFE_RANGE * out.rng.next_float());
        let mut spark = out.create(BallEffectKind::Coli, base_pos, spark_vel, life);
        let jitter = Vec3::new(
            scale_boost * (out.rng.next_float() * 0.05 - 0.025),
            scale_boost * (out.rng.next_float() * 0.05 - 0.025),
            scale_boost * (out.rng.next_float() * 0.05 - 0.025),
        );
        let push = scale_boost * (out.rng.next_float() * 0.05 + 0.06);
        spark.vel.x += jitter.x + push * normal.x;
        spark.vel.y += jitter.y + push * normal.y;
        spark.vel.z += jitter.z + push * normal.z;
        spark.color_r = 1.1;
        spark.color_g = 1.0;
        spark.color_b = 0.6;
        out.effects.push(spark);
        count -= 1;
    }
}

/// `spawnPostGoalSparkle`
fn spawn_post_goal_sparkle(out: &mut Spawner, ball: &Ball) {
    if out.full() {
        return;
    }
    let vel = Vec3::new(
        (out.rng.next_float() - 0.5) * LEVITATE_DRIFT,
        LEVITATE_START_VEL + out.rng.next_float() * LEVITATE_START_VEL,
        (out.rng.next_float() - 0.5) * LEVITATE_DRIFT,
    );
    let life = trunc_i32(LEVITATE_LIFE_MIN + LEVITATE_LIFE_RANGE * out.rng.next_float());
    let mut sparkle = out.create(BallEffectKind::Levitate, ball.pos, vel, life);
    sparkle.pos.x += (out.rng.next_float() - 0.5) * LEVITATE_OFFSET_RADIUS;
    sparkle.pos.y += (out.rng.next_float() - 0.5) * LEVITATE_OFFSET_RADIUS;
    sparkle.pos.z += (out.rng.next_float() - 0.5) * LEVITATE_OFFSET_RADIUS;
    sparkle.prev_pos = sparkle.pos;
    sparkle.base_y = sparkle.pos.y;
    sparkle.scale = LEVITATE_SCALE;
    sparkle.scale_target = sparkle.scale;
    sparkle.rot_x = out.rng.next_s16();
    sparkle.rot_y = out.rng.next_s16();
    sparkle.rot_z = out.rng.next_s16();
    out.effects.push(sparkle);
}

/// The stage as the particles see it: collision geometry, the groups' transforms and a raycast scratch.
pub(crate) struct Ground<'a> {
    pub geom: &'a StageGeom,
    pub groups: &'a [AnimGroupState],
    pub collider: &'a mut Collider,
}

/// The glow follows the effect at its height above the remembered ground plane.
fn update_glow_from_ground(effect: &mut BallEffect, with_rotation: bool) {
    let n = effect.ground_normal;
    let dx = effect.pos.x - effect.ground_pos.x;
    let dy = effect.pos.y - effect.ground_pos.y;
    let dz = effect.pos.z - effect.ground_pos.z;
    let dist = dx * n.x + dy * n.y + dz * n.z;
    effect.glow_dist += (dist - effect.glow_dist) * 0.25;
    effect.glow_pos.x = effect.pos.x - (dist - 0.02) * n.x;
    effect.glow_pos.y = effect.pos.y - (dist - 0.02) * n.y;
    effect.glow_pos.z = effect.pos.z - (dist - 0.02) * n.z;
    if with_rotation {
        effect.glow_rot_y = atan2_s16(n.x, n.z) - 0x8000;
        effect.glow_rot_x = atan2_s16(n.y, sqrt(sum_sq2(n.x, n.z)));
    }
}

fn decay_glow(effect: &mut BallEffect, glow_decay: f64) {
    if effect.glow_dist > 0.0 {
        effect.glow_dist = js_max(0.0, effect.glow_dist - glow_decay);
    }
}

/// `updateEffectGround`
fn update_effect_ground(
    effect: &mut BallEffect,
    ground: &mut Ground,
    check_mask: i32,
    threshold: f64,
    glow_decay: f64,
) {
    if effect.life & check_mask == 0 {
        effect.has_ground = false;
        if let Some(hit) =
            ground
                .collider
                .raycast_stage_down(&effect.pos, ground.geom, ground.groups)
            && effect.pos.y - threshold < hit.pos.y
        {
            effect.has_ground = true;
            effect.ground_pos = hit.pos;
            effect.ground_normal = hit.normal;
            effect.ground_vel = hit.surface_vel;
        }
    }
    if !effect.has_ground {
        decay_glow(effect, glow_decay);
        return;
    }
    update_glow_from_ground(effect, true);
}

/// `applyEffectGroundResponse`; `rng` is given for stars, whose spin a bounce kicks.
fn apply_effect_ground_response(
    effect: &mut BallEffect,
    min_dist: f64,
    bounce_scale: f64,
    surface_blend: f64,
    rng: Option<&mut DeterministicRng>,
) {
    let normal = effect.ground_normal;
    let dx = effect.pos.x - effect.ground_pos.x;
    let dy = effect.pos.y - effect.ground_pos.y;
    let dz = effect.pos.z - effect.ground_pos.z;
    let dist = dx * normal.x + dy * normal.y + dz * normal.z;
    if dist >= min_dist {
        return;
    }

    let push = min_dist - dist;
    effect.pos.x += push * normal.x;
    effect.pos.y += push * normal.y;
    effect.pos.z += push * normal.z;

    let dot = vec_dot(&effect.vel, &normal);
    if dot >= 0.0 {
        return;
    }
    let impulse = -dot;
    effect.vel.x += impulse * normal.x;
    effect.vel.y += impulse * normal.y;
    effect.vel.z += impulse * normal.z;
    effect.vel.x += (effect.ground_vel.x - effect.vel.x) * surface_blend;
    effect.vel.y += (effect.ground_vel.y - effect.vel.y) * surface_blend;
    effect.vel.z += (effect.ground_vel.z - effect.vel.z) * surface_blend;
    if let Some(rng) = rng {
        effect.rot_vel_x =
            to_s16(effect.rot_vel_x as f64 + impulse * (rng.next_float() - 0.5) * STAR_ROT_KICK_XZ);
        effect.rot_vel_y =
            to_s16(effect.rot_vel_y as f64 + impulse * (rng.next_float() - 0.5) * STAR_ROT_KICK_Y);
        effect.rot_vel_z =
            to_s16(effect.rot_vel_z as f64 + impulse * (rng.next_float() - 0.5) * STAR_ROT_KICK_XZ);
    }
    effect.vel.x += impulse * bounce_scale * normal.x;
    effect.vel.y += impulse * bounce_scale * normal.y;
    effect.vel.z += impulse * bounce_scale * normal.z;
}

/// `updateEffectGlow` (flashes only: they do not move, so the ground is looked up every 16 ticks).
fn update_effect_glow(
    effect: &mut BallEffect,
    ground: &mut Ground,
    threshold: f64,
    glow_decay: f64,
) {
    if effect.life & 0x0f != 0 {
        if effect.has_ground {
            update_glow_from_ground(effect, false);
        }
        return;
    }
    let hit = ground
        .collider
        .raycast_stage_down(&effect.pos, ground.geom, ground.groups);
    // `!hit || hit.pos.y < effect.pos.y - threshold`, kept in that form for positions that are not numbers.
    let too_far = |hit: &RayHit| hit.pos.y < effect.pos.y - threshold;
    let Some(hit) = hit.filter(|hit| !too_far(hit)) else {
        effect.has_ground = false;
        decay_glow(effect, glow_decay);
        return;
    };
    effect.has_ground = true;
    effect.ground_pos = hit.pos;
    effect.ground_normal = hit.normal;
    update_glow_from_ground(effect, true);
}

/// `updateBallEffects`: one tick for every live effect, newest first as in the reference (the order in
/// which stars draw from `rng`), then the expired ones are removed in place.
pub(crate) fn update_ball_effects(
    effects: &mut Vec<BallEffect>,
    gravity: &Vec3,
    ground: &mut Ground,
    rng: &mut DeterministicRng,
) {
    for effect in effects.iter_mut().rev() {
        effect.prev_pos = effect.pos;
        effect.prev_rot_x = effect.rot_x;
        effect.prev_rot_y = effect.rot_y;
        effect.prev_rot_z = effect.rot_z;
        effect.age += 1;
        effect.life -= 1;
        if effect.life <= 0 {
            continue;
        }
        let life = effect.life as f64;

        match effect.kind {
            BallEffectKind::Levitate => {
                effect.vel.x *= LEVITATE_DAMP;
                effect.vel.y = effect.vel.y * LEVITATE_DAMP + LEVITATE_GRAVITY;
                effect.vel.z *= LEVITATE_DAMP;
                effect.pos.x += effect.vel.x;
                effect.pos.y += effect.vel.y;
                effect.pos.z += effect.vel.z;
                effect.alpha = js_min(1.0, js_max(0.0, effect.life_ratio()));
                effect.scale = effect.scale_target * (0.6 + 0.4 * effect.alpha);
                continue;
            }
            BallEffectKind::ColiFlash => {
                effect.scale += (effect.scale_target - effect.scale) * 0.1;
                let normal = effect.surface_normal;
                let nx = -normal.x;
                let nz = -normal.z;
                effect.rot_y = atan2_s16(nx, nz);
                effect.rot_x = wrap_s16(atan2_s16(normal.y, sqrt(sum_sq2(nx, nz))) - 0x8000);
                if effect.life < 8 {
                    effect.alpha *= js_max(0.0, (life - 1.0) / life);
                }
                update_effect_glow(effect, ground, STAR_GROUND_CHECK, 0.0625);
                continue;
            }
            BallEffectKind::Coli | BallEffectKind::ColiStar => {}
        }

        let star = effect.kind == BallEffectKind::ColiStar;
        let gravity_scale = if star {
            STAR_GRAVITY_SCALE
        } else {
            SPARK_GRAVITY_SCALE
        };
        let damp = if star { STAR_DAMP } else { SPARK_DAMP };
        effect.vel.x += gravity.x * gravity_scale;
        effect.vel.y += gravity.y * gravity_scale;
        effect.vel.z += gravity.z * gravity_scale;
        effect.vel.x *= damp;
        effect.vel.y *= damp;
        effect.vel.z *= damp;
        effect.pos.x += effect.vel.x;
        effect.pos.y += effect.vel.y;
        effect.pos.z += effect.vel.z;

        if star {
            effect.rot_vel_x -= effect.rot_vel_x >> 2;
            effect.rot_vel_y -= effect.rot_vel_y >> 5;
            effect.rot_vel_z -= effect.rot_vel_z >> 3;
            effect.rot_x = wrap_s16(effect.rot_x + effect.rot_vel_x);
            effect.rot_y = wrap_s16(effect.rot_y + effect.rot_vel_y);
            effect.rot_z = wrap_s16(effect.rot_z + effect.rot_vel_z);
            if effect.life < 24 {
                effect.scale *= life / (life + 1.0);
            } else {
                effect.scale += (effect.scale_target - effect.scale) * 0.3312;
            }
            update_effect_ground(effect, ground, 0x07, STAR_GROUND_CHECK, 0.0625);
            if effect.has_ground {
                let min_dist = effect.scale * STAR_GROUND_MIN_DIST_SCALE;
                apply_effect_ground_response(
                    effect,
                    min_dist,
                    1.0,
                    STAR_GROUND_BLEND,
                    Some(&mut *rng),
                );
            }
        } else {
            update_effect_ground(effect, ground, 0x0f, SPARK_GROUND_CHECK, 0.125);
            if effect.has_ground {
                apply_effect_ground_response(
                    effect,
                    SPARK_GROUND_MIN_DIST,
                    0.9,
                    SPARK_GROUND_BLEND,
                    None,
                );
            }
            if effect.life < 24 {
                effect.scale = life / 24.0;
                effect.color_r *= 0.96;
                effect.color_g *= 0.87;
                effect.color_b *= 0.86;
            } else {
                effect.scale = 1.0;
            }
        }
        if effect.life < 24 {
            effect.alpha = js_max(0.0, life / 24.0);
        } else {
            effect.alpha = 1.0;
        }
    }
    // The reference splices each expired effect out as it meets it; the survivors keep their order.
    effects.retain(|effect| effect.life > 0);
}

/// `spawnGoalBagConfetti`: `ball_vel` is the ball's velocity in the bag's anim-group space.
pub(crate) fn spawn_goal_bag_confetti(
    confetti: &mut Vec<ConfettiParticle>,
    bag: &GoalBagState,
    group: &AnimGroupState,
    ball_vel: &Vec3,
    stack: &mut MatrixStack,
    rng: &mut DeterministicRng,
) {
    stack.from_mtx(&group.transform);
    let mut vel = stack.tf_vec(ball_vel);
    let world_pos = stack.tf_point(&bag.position);
    stack.from_mtx(&group.prev_transform);
    let prev_world_pos = stack.tf_point(&bag.position);
    vel.x += world_pos.x - prev_world_pos.x;
    vel.y += world_pos.y - prev_world_pos.y;
    vel.z += world_pos.z - prev_world_pos.z;

    let base_radius = bag.bound_sphere_radius;
    stack.from_mtx(&group.transform);
    stack.translate(&bag.position);

    for i in 0..CONFETTI_PER_GOAL {
        let local_z = 0.5 * (base_radius * (1.0 + rng.next_float()));
        // The rotations accumulate from piece to piece, as in the reference.
        stack.rotate_y(rng.next_s16());
        stack.rotate_x(rng.next_s16());
        let spawn_pos = stack.tf_point(&Vec3::new(0.0, 0.0, local_z));

        let rot_x = rng.next_s16();
        let rot_y = rng.next_s16();
        let rot_z = rng.next_s16();
        let scale = 0.5 + 0.5 * rng.next_float();
        let life = trunc_i32(CONFETTI_LIFE_BASE + CONFETTI_LIFE_RANGE * rng.next_float());
        let ground_bias = 0.0001 * rng.next_float();
        confetti.push(ConfettiParticle {
            model_index: i % CONFETTI_MODEL_COUNT,
            pos: spawn_pos,
            prev_pos: spawn_pos,
            vel,
            rot_x,
            rot_y,
            rot_z,
            prev_rot_x: rot_x,
            prev_rot_y: rot_y,
            prev_rot_z: rot_z,
            rot_vel_x: 0,
            rot_vel_y: 0,
            rot_vel_z: 0,
            scale,
            life,
            on_ground: false,
            ground_pos: Vec3::ZERO,
            ground_normal: Vec3::new(0.0, 1.0, 0.0),
            ground_vel: Vec3::ZERO,
            ground_bias,
        });
    }
}

/// The piece's own up axis in world space.
fn confetti_up(frag: &ConfettiParticle, stack: &mut MatrixStack) -> Vec3 {
    stack.from_identity();
    stack.rotate_y(frag.rot_y);
    stack.rotate_x(frag.rot_x);
    stack.rotate_z(frag.rot_z);
    stack.tf_vec(&Vec3::new(0.0, 1.0, 0.0))
}

/// `updateConfetti`
pub(crate) fn update_confetti(
    confetti: &mut Vec<ConfettiParticle>,
    gravity: &Vec3,
    ground: &mut Ground,
    stack: &mut MatrixStack,
) {
    for frag in confetti.iter_mut().rev() {
        frag.prev_pos = frag.pos;
        frag.prev_rot_x = frag.rot_x;
        frag.prev_rot_y = frag.rot_y;
        frag.prev_rot_z = frag.rot_z;
        frag.life -= 1;
        if frag.life <= 0 {
            continue;
        }

        frag.vel.x += gravity.x * CONFETTI_GRAVITY_SCALE;
        frag.vel.y += gravity.y * CONFETTI_GRAVITY_SCALE;
        frag.vel.z += gravity.z * CONFETTI_GRAVITY_SCALE;

        // Air resistance along the piece's flat side: it sheds most of the speed across its face.
        let dir = confetti_up(frag, stack);
        let dot = (dir.x * frag.vel.x) + (dir.y * frag.vel.y) + (dir.z * frag.vel.z);
        let align = CONFETTI_BOUNCE * dot;
        frag.vel.x += dir.x * align;
        frag.vel.y += dir.y * align;
        frag.vel.z += dir.z * align;

        frag.vel.x *= CONFETTI_VEL_DAMP;
        frag.vel.y *= CONFETTI_VEL_DAMP;
        frag.vel.z *= CONFETTI_VEL_DAMP;

        frag.pos.x += frag.vel.x;
        frag.pos.y += frag.vel.y;
        frag.pos.z += frag.vel.z;

        let rot_scale = CONFETTI_ROT_SPEED_SCALE / js_max(frag.scale, 0.001);
        // `(a + Math.trunc(b)) | 0`
        let spin = |rot_vel: i32, v: f64| to_int32(rot_vel as f64 + (rot_scale * v).trunc());
        frag.rot_vel_x = spin(frag.rot_vel_x, frag.vel.x + frag.vel.y);
        frag.rot_vel_y = spin(frag.rot_vel_y, frag.vel.y + frag.vel.z);
        frag.rot_vel_z = spin(frag.rot_vel_z, frag.vel.z + frag.vel.x);

        frag.rot_vel_x -= frag.rot_vel_x >> 5;
        frag.rot_vel_y -= frag.rot_vel_y >> 5;
        frag.rot_vel_z -= frag.rot_vel_z >> 5;

        frag.rot_x = to_s16(frag.rot_x as f64 + frag.rot_vel_x as f64);
        frag.rot_y = to_s16(frag.rot_y as f64 + frag.rot_vel_y as f64);
        frag.rot_z = to_s16(frag.rot_z as f64 + frag.rot_vel_z as f64);

        if frag.life & 0x0f == 0 {
            frag.on_ground = false;
            if let Some(hit) =
                ground
                    .collider
                    .raycast_stage_down(&frag.pos, ground.geom, ground.groups)
                && hit.pos.y > frag.pos.y - CONFETTI_GROUND_CHECK
            {
                frag.on_ground = true;
                frag.ground_pos = hit.pos;
                frag.ground_normal = hit.normal;
                frag.ground_vel = hit.surface_vel;
            }
        }

        if !frag.on_ground {
            continue;
        }
        let mut nx = frag.ground_normal.x;
        let mut ny = frag.ground_normal.y;
        let mut nz = frag.ground_normal.z;
        let mut ground_dir = confetti_up(frag, stack);
        let normal_dot = (ground_dir.x * nx) + (ground_dir.y * ny) + (ground_dir.z * nz);
        let contact_eps = frag.ground_bias + (1.0 - normal_dot.abs()) * 0.1;
        let dist = nx * (frag.pos.x - frag.ground_pos.x)
            + ny * (frag.pos.y - frag.ground_pos.y)
            + nz * (frag.pos.z - frag.ground_pos.z);
        if dist < contact_eps + 0.05 {
            let push = (contact_eps + 0.05) - dist;
            frag.pos.x += push * nx;
            frag.pos.y += push * ny;
            frag.pos.z += push * nz;
            let mut normal_speed = (frag.vel.x * nx) + (frag.vel.y * ny) + (frag.vel.z * nz);
            if normal_speed < 0.0 {
                normal_speed = -normal_speed;
                frag.vel.x += normal_speed * nx;
                frag.vel.y += normal_speed * ny;
                frag.vel.z += normal_speed * nz;
                let surface_blend = normal_dot.abs() * 0.125;
                frag.vel.x += surface_blend * (frag.ground_vel.x - frag.vel.x);
                frag.vel.y += surface_blend * (frag.ground_vel.y - frag.vel.y);
                frag.vel.z += surface_blend * (frag.ground_vel.z - frag.vel.z);
                normal_speed *= 0.5;
                frag.vel.x += normal_speed * nx;
                frag.vel.y += normal_speed * ny;
                frag.vel.z += normal_speed * nz;
            }
            // Lay the piece flat: turn its up axis a twentieth of the way to the ground normal.
            if normal_dot < 0.0 {
                nx = -nx;
                ny = -ny;
                nz = -nz;
            }
            ground_dir.x = nx * 0.05 + ground_dir.x * 0.95;
            ground_dir.y = ny * 0.05 + ground_dir.y * 0.95;
            ground_dir.z = nz * 0.05 + ground_dir.z * 0.95;
            vec_normalize_len(&mut ground_dir);
            let prev_rot_x = frag.rot_x;
            let prev_rot_z = frag.rot_z;
            stack.from_rotate_y(-frag.rot_y);
            stack.rotate_x(0);
            let rot_vec = stack.tf_vec(&ground_dir);
            frag.rot_x = atan2_s16(rot_vec.z, rot_vec.y);
            frag.rot_z = -atan2_s16(rot_vec.x, sqrt(sum_sq2(rot_vec.z, rot_vec.y)));
            frag.rot_vel_x = (frag.rot_vel_x >> 2) + ((frag.rot_x - prev_rot_x) >> 2);
            frag.rot_vel_y -= frag.rot_vel_y >> 4;
            frag.rot_vel_z = (frag.rot_vel_z >> 2) + ((frag.rot_z - prev_rot_z) >> 2);
        }
    }
    confetti.retain(|frag| frag.life > 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_max_and_min_follow_javascript() {
        assert!(js_max(0.0, f64::NAN).is_nan());
        assert!(js_min(f64::NAN, 1.0).is_nan());
        assert!(js_max(0.0, -0.0).is_sign_positive());
        assert!(js_max(-0.0, 0.0).is_sign_positive());
        assert!(js_min(0.0, -0.0).is_sign_negative());
        assert!(js_min(-0.0, 0.0).is_sign_negative());
        assert_eq!(js_max(0.0, -3.0), 0.0);
        assert_eq!(js_max(0.0, 3.0), 3.0);
        assert_eq!(js_min(1.0, 3.0), 1.0);
        assert_eq!(js_max(f64::NEG_INFINITY, f64::INFINITY), f64::INFINITY);
    }

    #[test]
    fn the_effect_pool_stays_within_its_memory_budget() {
        // The pool is allocated whole for every stage; wasm memory never shrinks.
        let bytes = std::mem::size_of::<BallEffect>() * MAX_BALL_EFFECTS;
        assert!(bytes <= 320 * 1024, "{bytes} bytes");
        let confetti = std::mem::size_of::<ConfettiParticle>() * CONFETTI_PER_GOAL;
        assert!(confetti <= 40 * 1024, "{confetti} bytes");
    }

    #[test]
    fn counts_from_bad_numbers_are_bounded() {
        assert_eq!(trunc_i32(f64::NAN), 0);
        assert_eq!(trunc_i32(f64::INFINITY), i32::MAX);
        assert_eq!(trunc_i32(-1e300), i32::MIN);
        assert_eq!(trunc_i32(59.999), 59);
    }
}
