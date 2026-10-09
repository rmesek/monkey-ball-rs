//! The display-only particles of a stage being played: movement sparks, collision stars and flash, the
//! post-goal sparkle, their glow on the ground, and the goal confetti. The simulation owns their state
//! (`mb_engine::effects`); this turns it into the frame's instances and quads.
//!
//! Mirrors `Game.getConfettiRenderState` / `getEffectRenderState` (`game.ts`: the blend between two ticks)
//! and `World.drawConfetti` / `drawEffects` (`noclip/SuperMonkeyBall/World.ts`: the geometry):
//! * Every particle is drawn at `prev_pos + (pos - prev_pos) * t`. Stars and confetti blend their angles
//!   the short way round, flashes and glows do not.
//! * A spark is a streak: a quad built in view space from where the spark is to where it was two samples
//!   of 33.4 ms ago, a fixed number of pixels wide at any distance. A star is the `CRASH_STAR` model plus
//!   a thin streak. Sparks and stars near the ground light it (`circle_white2`, additive).
//! * A flash is the same disc on the surface that was hit, a sparkle a billboard, confetti five models.
//!
//! One draw call holds all particles of a kind (`Renderer::push_instance` / `push_quad`), so the frame
//! costs at most eleven draws for them however many there are. Nothing is allocated or created here after
//! [`Particles::build`].
//!
//! What differs from the reference is listed in `docs/GAMEPLAY.md`.

use glam::{Mat4, Vec2, Vec3};
use mb_engine::effects::{CONFETTI_MODEL_COUNT, CONFETTI_PER_GOAL, MAX_BALL_EFFECTS};
use mb_engine::{BallEffect, BallEffectKind, Sim};
use mb_render::material::sampler_key;
use mb_render::{
    BlendMode, Camera, DrawOrder, InstanceBatchId, InstanceStyle, ModelId, Quad, Renderer,
    SamplerKey, TextureId,
};

use crate::play_scene::lerp_s16;
use crate::stage_scene::{LazyTextures, S16_TO_RADIANS};

/// `confettiModels`, in the order of `ConfettiParticle::model_index`.
const CONFETTI_MODELS: [&str; CONFETTI_MODEL_COUNT] = [
    "PAPER_PIECE_DEEPGREEN",
    "PAPER_PIECE_GREEN",
    "PAPER_PIECE_ORANGE",
    "PAPER_PIECE_RED",
    "PAPER_PIECE_YELLOW",
];
/// `sparkModel`.
const STAR_MODEL: &str = "CRASH_STAR";
/// `glowModel`, which also draws the flash: the first that exists.
const GLOW_MODELS: [&str; 2] = ["circle_white2", "circle_white"];
/// The model whose first texture layer masks the projected shadow (`shadowTevLayer`).
const SHADOW_MASK_MODEL: &str = "circle_white";
/// `CommonNlModelID.CROSS_LIGHT`: the game's own sparkle, a quarter of a cross of light.
const NL_CROSS_LIGHT: usize = 0x35;

/// `EFFECT_DEPTH_BIAS`, `EFFECT_DEPTH_BIAS_MAX`: how far a glow, flash or star is moved off its surface.
const DEPTH_BIAS: f32 = 0.2;
const DEPTH_BIAS_MAX: f32 = 0.5;
/// `persistenceMs`: the age of one sample of a streak's tail.
const STREAK_PERSISTENCE_MS: f64 = 33.4;
/// One simulation tick.
pub const TICK_MS: f64 = 1000.0 / 60.0;

/// Streaks drawn at once. The engine's pool holds twice as many effects; the highest count it has seen on
/// the 200 stages is 212 (`docs/ENGINE.md`). The rest of a larger burst is not drawn.
const MAX_STREAKS: usize = MAX_BALL_EFFECTS / 2;
/// Sparkles drawn at once: one is born every other tick and lives at most 75.
const MAX_SPARKLES: usize = 48;
/// A sparkle is four quads, one per quarter of the cross.
const QUAD_CAPACITY: usize = MAX_STREAKS + 4 * MAX_SPARKLES;
/// The confetti of the one bag that opens per attempt, and a star, glow or flash for 768 effects.
const INSTANCE_CAPACITY: usize = CONFETTI_PER_GOAL + 768;

fn vec3(v: &mb_engine::math::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// `prev + (pos - prev) * t`, or `None` when that is not a place (`getEffectRenderState` skips those).
fn blend_pos(prev: &mb_engine::math::Vec3, pos: &mb_engine::math::Vec3, t: f64) -> Option<Vec3> {
    let p = Vec3::new(
        (prev.x + (pos.x - prev.x) * t) as f32,
        (prev.y + (pos.y - prev.y) * t) as f32,
        (prev.z + (pos.z - prev.z) * t) as f32,
    );
    p.is_finite().then_some(p)
}

/// `T * Ry * Rx` with S16 angles.
fn pose_yx(pos: Vec3, rot_x: f32, rot_y: f32) -> Mat4 {
    Mat4::from_translation(pos)
        * Mat4::from_rotation_y(rot_y * S16_TO_RADIANS)
        * Mat4::from_rotation_x(rot_x * S16_TO_RADIANS)
}

/// `applyEffectDepthBias`: `pos` moved along `normal`, or towards the camera when there is none, by 0.2
/// (more for effects larger than one unit, at most 0.5).
fn depth_bias(pos: Vec3, normal: Option<Vec3>, scale: f64, camera: Vec3) -> Vec3 {
    let scale_mul = if scale.is_finite() && scale != 0.0 {
        scale.max(1.0) as f32
    } else {
        1.0
    };
    let bias = (DEPTH_BIAS * scale_mul).min(DEPTH_BIAS_MAX);
    if let Some(n) = normal.filter(|n| n.is_finite()) {
        let len = n.length();
        if len > 1e-4 {
            return pos + n * (bias / len);
        }
    }
    let dir = camera - pos;
    let len = dir.length();
    if len > 1e-4 {
        pos + dir * (bias / len)
    } else {
        pos
    }
}

/// The four corners of a streak in view space, `head0, head1, tail0, tail1` (`drawEffects`, the `streak`
/// branch). `end` is where the spark is and `tail` where its trail starts, both in view space; `None`
/// when either is behind the camera or the streak has no direction.
///
/// The reference sizes the streak in pixels, `(2 / height) * (depth / tan(fov / 2))` units per pixel times
/// `height / 1200`: the height of the target cancels out, so it is not a parameter here.
pub fn streak_corners(
    end: Vec3,
    tail: Vec3,
    scale: f32,
    life_ratio: f32,
    tan_half_fov: f32,
) -> Option<[Vec3; 4]> {
    if end.z >= -0.001 || tail.z >= -0.001 {
        return None;
    }
    let end_screen = end.truncate() / end.z.abs();
    let mut dir = end_screen - tail.truncate() / tail.z.abs();
    let mut dir_len = dir.length();
    // Nine pixels wide from three units away, widening to 27 at the camera.
    let thickness = if end.z <= -3.0 {
        9.0
    } else {
        (end.z + 3.0 + 1.5) * 6.0
    };
    let depth = (-end.z).max(-tail.z).max(0.001);
    let unit_per_pixel = 2.0 * depth / tan_half_fov / 1200.0;
    let size =
        (thickness * unit_per_pixel * scale * 0.12 * life_ratio.clamp(0.0, 1.0)).max(0.00001);
    if dir_len * 100.0 < size * 2.0 {
        // Hardly moving on screen: a dot, stretched a little along its direction.
        if dir_len < 1e-4 {
            dir_len = 1.0;
            dir = Vec2::X;
        }
        dir = dir / dir_len * size * 0.02;
        dir_len = dir.length();
    }
    if dir_len < 1e-4 {
        return None;
    }
    let along = dir / dir_len;
    let right = Vec2::new(along.y, -along.x) * size;
    // A streak shorter than it is wide is lengthened to a square about its middle.
    let true_dist = dir_len * tail.z.abs().max(end.z.abs());
    let short = size - true_dist.min(size);
    let pos_fix = along * short * 0.5;
    let head = end.truncate() + pos_fix;
    let tail_base = tail.truncate() - along * short + pos_fix;
    Some([
        (head + right).extend(end.z),
        (head - right).extend(end.z),
        (tail_base + right).extend(tail.z),
        (tail_base - right).extend(tail.z),
    ])
}

/// Colours of a streak's head and tail corners: the tail is the head darkened towards red.
fn streak_colors(e: &BallEffect) -> ([u8; 4], [u8; 4]) {
    let base = [e.color_r, e.color_g, e.color_b].map(|c| (c as f32).clamp(0.0, 1.0));
    let alpha = ((e.alpha as f32).clamp(0.0, 1.0) * 255.0).round() as u8;
    let byte = |v: f32, full: f32| (v * full).round() as u8;
    (
        [
            byte(base[0], 255.0),
            byte(base[1], 255.0),
            byte(base[2], 255.0),
            alpha,
        ],
        [
            byte(base[0], 96.0),
            byte(base[1], 88.0),
            byte(base[2], 8.0),
            alpha,
        ],
    )
}

/// Where a streak was at its last two samples, in view space (`streakHistory`).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Trail {
    id: u32,
    older: Vec3,
    prev: Vec3,
    last_update: f64,
}

impl Trail {
    /// Take the samples that are due at `now_ms` and return where the tail is (`drawEffects`). A streak
    /// that is new has no tail yet.
    fn advance(found: Option<Trail>, id: u32, end: Vec3, now_ms: f64) -> (Trail, Vec3) {
        let Some(mut trail) = found else {
            let trail = Trail {
                id,
                older: end,
                prev: end,
                last_update: now_ms,
            };
            return (trail, end);
        };
        if now_ms > trail.last_update + STREAK_PERSISTENCE_MS {
            let due = ((now_ms - trail.last_update) / STREAK_PERSISTENCE_MS).floor();
            for _ in 0..due.min(4.0) as u32 {
                trail.older = trail.prev;
                trail.prev = end;
                trail.last_update += STREAK_PERSISTENCE_MS;
            }
        }
        let ratio = ((now_ms - trail.last_update) / STREAK_PERSISTENCE_MS).clamp(0.0, 1.0) as f32;
        let tail = trail.older + (trail.prev - trail.older) * ratio;
        (trail, tail)
    }
}

/// What a frame's particles are drawn with.
pub struct ParticleFrame<'a> {
    pub camera: &'a Camera,
    /// The stage tilt of this frame (`Renderer::set_tilt`).
    pub tilt: Mat4,
    /// Blend from the previous tick to the current one, 0..=1.
    pub t: f32,
    /// Time on the particles' own clock (it stands still when they do, so a paused game shows the same
    /// streaks frame after frame).
    pub now_ms: f64,
}

/// The view of one frame, worked out once for all its effects.
struct View {
    /// Stage space -> view space (`viewFromWorldTilted`).
    from_stage: Mat4,
    /// View space -> world space after the tilt, which is what quads are given in.
    to_world: Mat4,
    camera: Vec3,
    tan_half_fov: f32,
    now_ms: f64,
}

/// The mask of the projected shadow and how it is sampled: the first texture layer of `circle_white`.
pub fn shadow_mask(
    r: &mut Renderer,
    gma: &mb_gma::Gma,
    tpl: &mb_tpl::Tpl,
    set: &mut LazyTextures,
) -> Option<(TextureId, SamplerKey)> {
    let model = gma.model(SHADOW_MASK_MODEL)?;
    let layer = model.tev_layers.first()?;
    let index = usize::try_from(layer.tex_idx).ok()?;
    let id = set.for_model(r, tpl, model).get(index).copied().flatten()?;
    let texture = r.texture(id);
    Some((id, sampler_key(layer, texture.width, texture.height)))
}

/// The texture of the common NaomiLib archive's `CROSS_LIGHT` model.
pub fn sparkle_texture(
    r: &mut Renderer,
    obj: &mb_gma::NlObj,
    tpl: &mb_tpl::Tpl,
    set: &mut LazyTextures,
) -> Option<TextureId> {
    let model = obj.model(NL_CROSS_LIGHT)?;
    let index = model.meshes.first()?.tex_idx? as usize;
    set.for_nl_model(r, tpl, model)
        .get(index)
        .copied()
        .flatten()
}

/// The renderer side of the particles of one loaded stage. Every part is optional: a model or texture the
/// disc lacks costs that kind of particle only.
pub struct Particles {
    confetti: [Option<InstanceBatchId>; CONFETTI_MODEL_COUNT],
    star: Option<InstanceBatchId>,
    glow: Option<InstanceBatchId>,
    flash: Option<InstanceBatchId>,
    /// Streaks are untextured (`streakDefaultTexture`).
    streak: TextureId,
    sparkle: Option<TextureId>,
    /// Trails of the streaks drawn last frame, by rising effect id (the order of `runtime.effects`).
    trails: Vec<Trail>,
    /// Scratch for the next `trails`.
    next_trails: Vec<Trail>,
}

impl Particles {
    /// Create the batches. `model` uploads a model of `common.gma` by name; `sparkle` is
    /// [`sparkle_texture`]. Everything the particles will ever need from the GPU is created here.
    pub fn build(
        r: &mut Renderer,
        model: &mut dyn FnMut(&mut Renderer, &str) -> Option<ModelId>,
        sparkle: Option<TextureId>,
    ) -> Self {
        r.reserve_instances(INSTANCE_CAPACITY);
        r.reserve_quads(QUAD_CAPACITY);
        let confetti = CONFETTI_MODELS
            .map(|name| model(r, name).map(|m| r.create_instance_batch(m, InstanceStyle::OPAQUE)));
        // Batches of one place in the frame draw in creation order: the star blends, the discs add.
        let star = model(r, STAR_MODEL).map(|m| r.create_instance_batch(m, InstanceStyle::SORTED));
        let disc = GLOW_MODELS.into_iter().find_map(|name| model(r, name));
        let glow = disc.map(|m| r.create_instance_batch(m, InstanceStyle::GLOW));
        let flash = disc.map(|m| r.create_instance_batch(m, InstanceStyle::FLASH));
        let streak = r.white_texture();
        r.warm_quad_texture(streak);
        if let Some(texture) = sparkle {
            r.warm_quad_texture(texture);
        }
        Self {
            confetti,
            star,
            glow,
            flash,
            streak,
            sparkle,
            trails: Vec::with_capacity(MAX_BALL_EFFECTS),
            next_trails: Vec::with_capacity(MAX_BALL_EFFECTS),
        }
    }

    /// A new attempt: effect ids start over, so no trail belongs to anything any more.
    pub fn reset(&mut self) {
        self.trails.clear();
    }

    /// Queue the particles of `sim` for the frame about to be drawn. Like sprites they are queued per
    /// frame, so this is called for every frame, paused or not.
    pub fn draw(&mut self, r: &mut Renderer, sim: &Sim, frame: &ParticleFrame) {
        let t = f64::from(frame.t.clamp(0.0, 1.0));
        let rt = &sim.runtime;
        self.draw_confetti(r, &rt.confetti, t);

        self.next_trails.clear();
        if !rt.effects.is_empty() {
            let to_world = frame.camera.view.inverse();
            let view = View {
                from_stage: frame.camera.view * frame.tilt,
                to_world,
                camera: to_world.w_axis.truncate(),
                tan_half_fov: (frame.camera.fov_y * 0.5).tan(),
                now_ms: frame.now_ms,
            };
            self.draw_effects(r, &rt.effects, &view, t);
        }
        std::mem::swap(&mut self.trails, &mut self.next_trails);
    }

    /// `drawConfetti`: each piece at `T * Ry * Rx * Rz * S`, lit like the stage and tilting with it.
    fn draw_confetti(&self, r: &mut Renderer, confetti: &[mb_engine::ConfettiParticle], t: f64) {
        let t32 = t as f32;
        for frag in confetti {
            let Some(batch) = self.confetti.get(frag.model_index).copied().flatten() else {
                continue;
            };
            let Some(pos) = blend_pos(&frag.prev_pos, &frag.pos, t) else {
                continue;
            };
            let mut m = pose_yx(
                pos,
                lerp_s16(frag.prev_rot_x, frag.rot_x, t32),
                lerp_s16(frag.prev_rot_y, frag.rot_y, t32),
            ) * Mat4::from_rotation_z(
                lerp_s16(frag.prev_rot_z, frag.rot_z, t32) * S16_TO_RADIANS,
            );
            if frag.scale != 1.0 {
                m *= Mat4::from_scale(Vec3::splat(frag.scale as f32));
            }
            r.push_instance(batch, &m, [1.0; 4]);
        }
    }

    /// `drawEffects`. Streaks first and sparkles last, so that each kind of quad is one run (one draw).
    fn draw_effects(&mut self, r: &mut Renderer, effects: &[BallEffect], view: &View, t: f64) {
        let mut old = 0;
        let mut streaks = 0;
        let mut any_sparkle = false;
        for e in effects {
            let Some(pos) = blend_pos(&e.prev_pos, &e.pos, t) else {
                continue;
            };
            match e.kind {
                BallEffectKind::Coli => {
                    if self.streak(r, e, pos, e.scale, view, &mut old, &mut streaks) {
                        self.glow(r, e, view);
                    }
                }
                BallEffectKind::ColiStar => {
                    self.streak(r, e, pos, e.scale * 0.6, view, &mut old, &mut streaks);
                    self.star(r, e, pos, view, t as f32);
                    self.glow(r, e, view);
                }
                BallEffectKind::ColiFlash => self.flash(r, e, pos, view),
                BallEffectKind::Levitate => any_sparkle = true,
            }
        }
        let Some(texture) = self.sparkle.filter(|_| any_sparkle) else {
            return;
        };
        let sparkles = effects
            .iter()
            .filter(|e| e.kind == BallEffectKind::Levitate);
        for e in sparkles.take(MAX_SPARKLES) {
            if let Some(pos) = blend_pos(&e.prev_pos, &e.pos, t) {
                Self::sparkle(r, texture, e, pos, view);
            }
        }
    }

    /// One streak. False when the reference draws neither it nor its glow.
    #[allow(clippy::too_many_arguments)]
    fn streak(
        &mut self,
        r: &mut Renderer,
        e: &BallEffect,
        pos: Vec3,
        scale: f64,
        view: &View,
        old: &mut usize,
        streaks: &mut usize,
    ) -> bool {
        let end = view.from_stage.transform_point3(pos);
        // Both lists rise by id: skip the trails of effects that are gone.
        while self.trails.get(*old).is_some_and(|trail| trail.id < e.id) {
            *old += 1;
        }
        let found = self.trails.get(*old).copied().filter(|t| t.id == e.id);
        let (trail, tail) = Trail::advance(found, e.id, end, view.now_ms);
        if self.next_trails.len() < self.next_trails.capacity() {
            self.next_trails.push(trail);
        }
        let Some(corners) = streak_corners(
            end,
            tail,
            scale as f32,
            e.life_ratio() as f32,
            view.tan_half_fov,
        ) else {
            return false;
        };
        if *streaks >= MAX_STREAKS {
            return false;
        }
        *streaks += 1;
        let (head, tail) = streak_colors(e);
        let quad = Quad {
            positions: corners.map(|c| view.to_world.transform_point3(c)),
            uvs: [[1.0, 0.0], [1.0, 1.0], [0.0, 0.0], [0.0, 1.0]],
            colors: [head, head, tail, tail],
        };
        r.push_quad(
            self.streak,
            BlendMode::Additive,
            DrawOrder::AfterSorted,
            &quad,
        );
        true
    }

    /// `drawGlow`: the light a spark or star throws on the ground below it, fading with its height.
    fn glow(&self, r: &mut Renderer, e: &BallEffect, view: &View) {
        let Some(batch) = self.glow.filter(|_| e.has_ground) else {
            return;
        };
        let dist = e.glow_dist;
        if !(dist > 0.0 && dist < 0.5) {
            return;
        }
        let scale = (0.25 - dist * 0.5) * e.scale;
        let at = vec3(&e.glow_pos);
        if !(scale > 0.0 && at.is_finite()) {
            return;
        }
        let k = 0.5 / (dist * 2.0 + 1.0);
        let at = depth_bias(at, None, e.scale, view.camera);
        let m = pose_yx(at, e.glow_rot_x as f32, e.glow_rot_y as f32)
            * Mat4::from_scale(Vec3::splat(scale as f32));
        let color = [e.color_r * k, e.color_g * k, e.color_b * k, 1.0];
        r.push_instance(batch, &m, color.map(|c| c as f32));
    }

    /// `drawFlash`: the disc on the surface the ball hit, on the ground below it when there is one.
    fn flash(&self, r: &mut Renderer, e: &BallEffect, pos: Vec3, view: &View) {
        let Some(batch) = self.flash else {
            return;
        };
        if e.alpha.is_nan() || e.alpha <= 1.0 / 255.0 {
            return;
        }
        let (at, rot_x, rot_y) = if e.has_ground {
            (vec3(&e.glow_pos), e.glow_rot_x, e.glow_rot_y)
        } else {
            (pos, e.rot_x, e.rot_y)
        };
        if !at.is_finite() {
            return;
        }
        let intensity = (e.alpha * 0.5).min(1.0) as f32;
        let at = depth_bias(at, Some(vec3(&e.surface_normal)), e.scale, view.camera);
        let m = pose_yx(at, rot_x as f32, rot_y as f32)
            * Mat4::from_scale(Vec3::splat((e.scale * 2.5) as f32));
        r.push_instance(batch, &m, [intensity, intensity, intensity * 0.8, 1.0]);
    }

    /// The star model, spinning, at `T * Ry * Rx * Rz * S`.
    fn star(&self, r: &mut Renderer, e: &BallEffect, pos: Vec3, view: &View, t: f32) {
        let Some(batch) = self.star else {
            return;
        };
        let at = depth_bias(pos, None, e.scale, view.camera);
        let mut m = pose_yx(
            at,
            lerp_s16(e.prev_rot_x, e.rot_x, t),
            lerp_s16(e.prev_rot_y, e.rot_y, t),
        ) * Mat4::from_rotation_z(lerp_s16(e.prev_rot_z, e.rot_z, t) * S16_TO_RADIANS);
        if e.scale != 1.0 {
            m *= Mat4::from_scale(Vec3::splat(e.scale as f32));
        }
        r.push_instance(batch, &m, [1.0, 1.0, 1.0, e.alpha as f32]);
    }

    /// `drawSparkleSprite`: a white billboard of half side `scale`, facing the camera. The texture is one
    /// quarter of the cross with its centre in the corner `(1, 0)`, so each quarter is a quad of its own.
    fn sparkle(r: &mut Renderer, texture: TextureId, e: &BallEffect, pos: Vec3, view: &View) {
        let size = e.scale as f32;
        let alpha = ((e.alpha as f32).clamp(0.0, 1.0) * 255.0).round() as u8;
        if size.is_nan() || size <= 0.0 || alpha == 0 {
            return;
        }
        let centre = view.from_stage.transform_point3(pos);
        for (sx, sy) in [(-1.0, 1.0), (1.0, 1.0), (-1.0, -1.0), (1.0, -1.0)] {
            // Corners as top-left, top-right, bottom-left, bottom-right of this quarter.
            let (left, right) = if sx < 0.0 { (-size, 0.0) } else { (0.0, size) };
            let (bottom, top) = if sy < 0.0 { (-size, 0.0) } else { (0.0, size) };
            let corners = [(left, top), (right, top), (left, bottom), (right, bottom)];
            let quad = Quad {
                positions: corners.map(|(x, y)| {
                    view.to_world
                        .transform_point3(centre + Vec3::new(x, y, 0.0))
                }),
                uvs: corners.map(|(x, y)| [1.0 - x.abs() / size, y.abs() / size]),
                colors: [[255, 255, 255, alpha]; 4],
            };
            r.push_quad(texture, BlendMode::Additive, DrawOrder::AfterSorted, &quad);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAN_30: f32 = 0.577_350_26;

    /// A spark as the engine's `spawn_movement_sparks` leaves it.
    fn spark(id: u32) -> BallEffect {
        let zero = mb_engine::math::Vec3::new(0.0, 0.0, 0.0);
        let up = mb_engine::math::Vec3::new(0.0, 1.0, 0.0);
        BallEffect {
            id,
            kind: BallEffectKind::Coli,
            pos: zero,
            prev_pos: zero,
            ground_pos: zero,
            ground_normal: up,
            ground_vel: zero,
            glow_pos: zero,
            glow_dist: 0.0,
            glow_rot_x: 0,
            glow_rot_y: 0,
            has_ground: false,
            surface_normal: up,
            vel: zero,
            life: 30,
            age: 0,
            scale: 1.0,
            scale_target: 1.0,
            alpha: 1.0,
            color_r: 1.1,
            color_g: 1.0,
            color_b: 0.6,
            rot_x: 0,
            rot_y: 0,
            rot_z: 0,
            prev_rot_x: 0,
            prev_rot_y: 0,
            prev_rot_z: 0,
            rot_vel_x: 0,
            rot_vel_y: 0,
            rot_vel_z: 0,
            base_y: 0.0,
        }
    }

    #[test]
    fn positions_blend_between_ticks_and_bad_ones_are_skipped() {
        let a = mb_engine::math::Vec3::new(1.0, 2.0, 3.0);
        let b = mb_engine::math::Vec3::new(3.0, 2.0, -1.0);
        assert_eq!(blend_pos(&a, &b, 0.0), Some(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(blend_pos(&a, &b, 0.5), Some(Vec3::new(2.0, 2.0, 1.0)));
        assert_eq!(blend_pos(&a, &b, 1.0), Some(Vec3::new(3.0, 2.0, -1.0)));
        let nan = mb_engine::math::Vec3::new(f64::NAN, 0.0, 0.0);
        assert_eq!(blend_pos(&a, &nan, 0.5), None);
        let far = mb_engine::math::Vec3::new(f64::INFINITY, 0.0, 0.0);
        assert_eq!(blend_pos(&far, &b, 0.5), None);
    }

    #[test]
    fn a_moving_streak_is_as_wide_as_the_reference_makes_it() {
        // Ten units away, moving one unit to the right: 9 px at 2 * 10 / tan(30 deg) / 1200 units each,
        // times 0.12, is the half width.
        let end = Vec3::new(1.0, 0.0, -10.0);
        let tail = Vec3::new(0.0, 0.0, -10.0);
        let [h0, h1, t0, t1] = streak_corners(end, tail, 1.0, 1.0, TAN_30).unwrap();
        let size = 9.0 * (2.0 * 10.0 / TAN_30 / 1200.0) * 0.12;
        assert!((h0 - Vec3::new(1.0, -size, -10.0)).length() < 1e-5, "{h0}");
        assert!((h1 - Vec3::new(1.0, size, -10.0)).length() < 1e-5, "{h1}");
        assert!((t0 - Vec3::new(0.0, -size, -10.0)).length() < 1e-5, "{t0}");
        assert!((t1 - Vec3::new(0.0, size, -10.0)).length() < 1e-5, "{t1}");
        // Half the life left, or a star's 0.6 scale: thinner by that much.
        let [h0, h1, ..] = streak_corners(end, tail, 0.6, 0.5, TAN_30).unwrap();
        assert!(((h1 - h0).length() - 2.0 * size * 0.3).abs() < 1e-5);
        // Nearer than three units the streak widens towards 27 px.
        let near = |z: f32| {
            let c = streak_corners(
                Vec3::new(0.1, 0.0, z),
                Vec3::new(0.0, 0.0, z),
                1.0,
                1.0,
                TAN_30,
            );
            let [h0, h1, ..] = c.unwrap();
            (h1 - h0).length() / (2.0 * (2.0 * -z / TAN_30 / 1200.0) * 0.12)
        };
        assert!((near(-3.0) - 9.0).abs() < 1e-3);
        assert!((near(-1.5) - 18.0).abs() < 1e-3);
    }

    #[test]
    fn a_resting_streak_is_a_dot_and_one_behind_the_camera_is_nothing() {
        let at = Vec3::new(0.5, 0.25, -8.0);
        // No motion at all: as wide as any streak and centred on the spark. Its length is the half width
        // less the 2 % of it that the reference takes for the direction, seen from eight units.
        let [h0, h1, t0, t1] = streak_corners(at, at, 1.0, 1.0, TAN_30).unwrap();
        let size = 9.0 * (2.0 * 8.0 / TAN_30 / 1200.0) * 0.12;
        let centre = (h0 + h1 + t0 + t1) / 4.0;
        assert!((centre - at).length() < 1e-4, "{centre}");
        assert!(((h0 - h1).length() - 2.0 * size).abs() < 1e-5);
        assert!(((h0 - t0).length() - size * (1.0 - 0.02 * 8.0)).abs() < 1e-5);
        // Behind the camera, or with its tail there.
        assert!(streak_corners(Vec3::new(0.0, 0.0, 1.0), at, 1.0, 1.0, TAN_30).is_none());
        assert!(streak_corners(at, Vec3::new(0.0, 0.0, 0.0), 1.0, 1.0, TAN_30).is_none());
        // Too small to have a direction (a star's streak far away).
        assert!(streak_corners(at, at, 0.001, 1.0, TAN_30).is_none());
    }

    #[test]
    fn streak_colours_fade_to_a_dark_red_tail() {
        let mut e = spark(1);
        e.alpha = 0.5;
        let (head, tail) = streak_colors(&e);
        // 1.1 is clamped; the tail keeps 96, 88 and 8 of 255.
        assert_eq!(head, [255, 255, 153, 128]);
        assert_eq!(tail, [96, 88, 5, 128]);
    }

    #[test]
    fn a_trail_lags_two_samples_behind_and_stands_still_with_the_clock() {
        let p = |x: f32| Vec3::new(x, 0.0, -5.0);
        // New: no tail.
        let (mut trail, tail) = Trail::advance(None, 7, p(0.0), 100.0);
        assert_eq!((trail.id, tail), (7, p(0.0)));
        // Within the first sample period nothing is taken yet.
        let (next, tail) = Trail::advance(Some(trail), 7, p(1.0), 120.0);
        assert_eq!(tail, p(0.0));
        trail = next;
        // One sample later the newest position is remembered, and the tail moves from the older sample
        // towards it with the time since.
        let (next, tail) = Trail::advance(Some(trail), 7, p(2.0), 134.0);
        assert_eq!((next.older, next.prev), (p(0.0), p(2.0)));
        assert!((tail.x - 2.0 * (0.6 / 33.4)).abs() < 1e-4, "{tail}");
        trail = next;
        // The same instant again (a paused game): the same trail and the same tail.
        let (again, same) = Trail::advance(Some(trail), 7, p(2.0), 134.0);
        assert_eq!((again, same), (trail, tail));
        // A long gap takes at most four samples, which all hold the newest position.
        let (next, tail) = Trail::advance(Some(trail), 7, p(9.0), 1000.0);
        assert_eq!((next.older, next.prev, tail), (p(9.0), p(9.0), p(9.0)));
        assert!((next.last_update - (133.4 + 4.0 * 33.4)).abs() < 1e-9);
    }

    #[test]
    fn the_bias_goes_along_the_normal_or_else_to_the_camera() {
        let camera = Vec3::new(0.0, 10.0, 0.0);
        let at = Vec3::ZERO;
        let moved = |normal, scale, to: Vec3| {
            let p = depth_bias(at, normal, scale, camera);
            assert!(
                (p - to).length() < 1e-6,
                "{p} for {normal:?} at scale {scale}"
            );
        };
        moved(None, 1.0, Vec3::new(0.0, 0.2, 0.0));
        // A small effect still moves the full 0.2; a large one up to 0.5.
        moved(None, 0.015, Vec3::new(0.0, 0.2, 0.0));
        moved(None, 2.0, Vec3::new(0.0, 0.4, 0.0));
        moved(None, 9.0, Vec3::new(0.0, 0.5, 0.0));
        moved(None, f64::NAN, Vec3::new(0.0, 0.2, 0.0));
        moved(
            Some(Vec3::new(0.0, 0.0, -4.0)),
            1.0,
            Vec3::new(0.0, 0.0, -0.2),
        );
        // No usable normal: towards the camera.
        moved(Some(Vec3::ZERO), 1.0, Vec3::new(0.0, 0.2, 0.0));
        moved(Some(Vec3::NAN), 1.0, Vec3::new(0.0, 0.2, 0.0));
        assert_eq!(depth_bias(camera, None, 1.0, camera), camera);
    }

    #[test]
    fn the_buffers_hold_a_full_frame_of_what_is_drawn() {
        assert_eq!(QUAD_CAPACITY, 704);
        assert_eq!(INSTANCE_CAPACITY, 928);
    }
}
