//! The 3D scene of a stage being played: the static stage scene plus everything the simulation moves (ball,
//! goal bags and tapes, goal clocks, bananas, bumpers, jamabars, animated backdrops), posed from an
//! `mb_engine::Sim` once per rendered frame.
//!
//! Mirrors the reference's `World.prepareToRender` / `AnimGroup.prepareToRender`
//! (`noclip/SuperMonkeyBall/World.ts`, `AnimGroup.ts`) fed by `Game.get*RenderState` (`game.ts`):
//! * The stage tilt is purely visual: everything that belongs to the stage is rotated about the ball by
//!   `world.xrot/zrot * 0.6` (`getTiltedViewMatrix`); the backdrop is not. The reference folds that rotation
//!   into the view matrix; here it goes into the scene uniform (`Renderer::set_tilt`) and objects only say
//!   whether it applies to them, so tilting the stage re-uploads no object.
//! * Models, textures and objects are created once in [`PlayScene::build`]. [`PlayScene::sync`] records the
//!   pose of one simulation tick; [`PlayScene::apply`] blends the last two poses for the frame being drawn
//!   (the reference's `get*RenderState(alpha)` getters) and only then touches the renderer. Rendering never
//!   feeds back into the simulation.
//! * The ball's shadow is projected onto the stage models from the blended ball (`drawProjectedShadow`), and
//!   the particles ([`crate::particles`]) are queued for every frame by [`PlayScene::draw_particles`]. They
//!   carry their own previous position, so they blend straight from the simulation.

use glam::{Mat4, Quat, Vec3};
use mb_engine::constants::ball_flags;
use mb_engine::math::Mtx;
use mb_engine::stage_runtime::BoundSphere;
use mb_engine::Sim;
use mb_render::{Camera, Layer, ModelId, ObjectId, Renderer, Shadow};
use web_time::Instant;

use crate::anim::{interpolate, loop_wrap};
use crate::particles::{self, ParticleFrame, Particles, TICK_MS};
use crate::stage_scene::{
    ItemKind, LazyTextures, LoadTimings, StageAssets, StageScene, S16_TO_RADIANS,
};

/// `STAGE_TILT_SCALE`: how much of the simulated tilt is shown.
const STAGE_TILT_SCALE: f32 = 0.6;
/// `BALL_BASE_RADIUS`: radius the ball models are authored at.
const BALL_BASE_RADIUS: f32 = 0.5;
/// Half height of the goal tape ribbon.
const TAPE_HALF_HEIGHT: f32 = 0.125;

/// `common_p.lz` model ids (`CommonNlModelID`).
const NL_GOAL_TAPE: usize = 0x0e;
const NL_LCD_LARGE_0: usize = 0x13;
const NL_LCD_SMALL_0: usize = 0x1d;

/// Ball shells, innermost first, with the reference's `depthOffset` (larger draws earlier).
const BALL_MODELS: [(&str, f32); 5] = [
    ("CLEAR_HEMI_INSIDE", 4.0),
    ("RED_HEMI_INSIDE", 3.0),
    ("SPHERE_EDGE_01_RED", 2.0),
    ("CLEAR_HEMI_OUTSIDE", 1.0),
    ("RED_HEMI_OUTSIDE", 0.0),
];

fn s16(v: i32) -> f32 {
    v as f32 * S16_TO_RADIANS
}

fn vec3(v: mb_engine::math::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// Engine 3x4 row-major matrix to a column-major `Mat4`.
pub fn mat4_from_mtx(m: &Mtx) -> Mat4 {
    Mat4::from_cols_array(&[
        m[0], m[4], m[8], 0.0, m[1], m[5], m[9], 0.0, m[2], m[6], m[10], 0.0, m[3], m[7], m[11],
        1.0,
    ])
}

/// `T * Rz * Ry * Rx` with S16 angles (goals, tapes, jamabars).
fn trs_zyx(pos: Vec3, rot: [i32; 3]) -> Mat4 {
    Mat4::from_translation(pos)
        * Mat4::from_rotation_z(s16(rot[2]))
        * Mat4::from_rotation_y(s16(rot[1]))
        * Mat4::from_rotation_x(s16(rot[0]))
}

/// `T * Ry * Rx * Rz` with S16 angles (bananas, goal bags).
fn trs_yxz(pos: Vec3, rot: [i32; 3]) -> Mat4 {
    Mat4::from_translation(pos)
        * Mat4::from_rotation_y(s16(rot[1]))
        * Mat4::from_rotation_x(s16(rot[0]))
        * Mat4::from_rotation_z(s16(rot[2]))
}

/// World-space rotation of the stage about `pivot` for a tilt of `xrot`, `zrot` (S16) shown at `factor`.
pub fn tilt_matrix(pivot: Vec3, xrot: i32, zrot: i32, factor: f32) -> Mat4 {
    tilt_matrix_units(pivot, xrot as f32, zrot as f32, factor)
}

/// [`tilt_matrix`] for fractional S16 angles (a tilt blended between two ticks).
pub fn tilt_matrix_units(pivot: Vec3, xrot: f32, zrot: f32, factor: f32) -> Mat4 {
    let rx = xrot * S16_TO_RADIANS * STAGE_TILT_SCALE * factor;
    let rz = zrot * S16_TO_RADIANS * STAGE_TILT_SCALE * factor;
    if rx == 0.0 && rz == 0.0 {
        return Mat4::IDENTITY;
    }
    Mat4::from_translation(pivot)
        * Mat4::from_rotation_x(rx)
        * Mat4::from_rotation_z(rz)
        * Mat4::from_translation(-pivot)
}

struct Bag {
    closed: Option<ObjectId>,
    open_a: Option<ObjectId>,
    open_b: Option<ObjectId>,
}

struct Tape {
    model: ModelId,
    object: ObjectId,
    /// What the vertex buffer holds.
    vertices: mb_gma::Vertices,
    /// Vertices of the last two ticks (`[previous, current]`).
    ticks: [mb_gma::Vertices; 2],
}

/// Pose of one renderer object at a tick.
#[derive(Clone, Copy)]
struct Slot {
    /// `None`: this tick did not pose the object (it keeps whatever it has).
    object: Option<ObjectId>,
    visible: bool,
    /// The stage tilt applies on top of `m`.
    tilted: bool,
    /// Opacity, for the objects whose animation drives it.
    alpha: Option<f32>,
    m: Mat4,
}

const EMPTY_SLOT: Slot = Slot {
    object: None,
    visible: false,
    tilted: true,
    alpha: None,
    m: Mat4::IDENTITY,
};

/// Everything [`PlayScene::apply`] needs from one simulation tick.
#[derive(Clone, Default)]
struct Pose {
    /// Indexed by `ObjectId::index`.
    slots: Vec<Slot>,
    pivot: Vec3,
    xrot: i32,
    zrot: i32,
    ball_rot: Quat,
    ball_scale: f32,
    ball_visible: bool,
}

impl Pose {
    fn begin(&mut self, objects: usize) {
        self.slots.clear();
        self.slots.resize(objects, EMPTY_SLOT);
    }

    fn slot(&mut self, o: ObjectId) -> &mut Slot {
        let slot = &mut self.slots[o.index()];
        slot.object = Some(o);
        slot
    }

    /// Show `o` at `m` (tilting with the stage).
    fn set(&mut self, o: ObjectId, m: Mat4) {
        let slot = self.slot(o);
        slot.visible = true;
        slot.m = m;
    }

    fn hide(&mut self, o: ObjectId) {
        self.slot(o).visible = false;
    }
}

/// A ball that moves further than this in one tick was placed, not rolled: do not blend across it.
const TELEPORT_DISTANCE: f32 = 3.0;

fn lerp_mat(a: &Mat4, b: &Mat4, t: f32) -> Mat4 {
    Mat4::from_cols(
        a.x_axis.lerp(b.x_axis, t),
        a.y_axis.lerp(b.y_axis, t),
        a.z_axis.lerp(b.z_axis, t),
        a.w_axis.lerp(b.w_axis, t),
    )
}

/// The reference's `lerpS16`: blend two S16 angles along the shorter way round.
pub fn lerp_s16(a: i32, b: i32, t: f32) -> f32 {
    let delta = b.wrapping_sub(a) as i16;
    a as f32 + delta as f32 * t
}

/// The parsed files a stage keeps while it is played. Models and textures live on the GPU after loading; the
/// parsed archives (several megabytes of vertex and texel data) are dropped.
pub struct PlayAssets {
    pub stagedef: mb_stage::Stage,
}

/// The five-digit clock above a goal: for each slot, one object per digit.
struct Clock {
    goal: usize,
    slots: Vec<(f32, [Option<ObjectId>; 10])>,
}

pub struct PlayScene {
    stage_id: u32,
    pub assets: PlayAssets,
    pub scene: StageScene,
    /// Poses of the last two simulation ticks (`[previous, current]`).
    poses: [Pose; 2],
    /// False until a tick has been recorded since the last reset: there is nothing to blend from.
    blend: bool,
    objects: usize,
    /// Scratch: anim group transforms of the tick being recorded.
    groups: Vec<Mat4>,
    ball: Vec<ObjectId>,
    bags: Vec<Bag>,
    tapes: Vec<Tape>,
    clocks: Vec<Clock>,
    particles: Particles,
    /// The stage tilt of the frame last posed by [`PlayScene::apply`].
    tilt: Mat4,
    /// The mask of the projected shadow was found; without it no shadow is drawn.
    shadow_mask: bool,
    /// The projected shadow is being drawn: its mask exists and the ball is visible.
    shadow: bool,
    /// Ticks in which the simulation advanced its particles since the last reset. It does not while the
    /// clock has run out, and then they are shown where they are instead of blended.
    particle_ticks: u32,
    particles_frozen: bool,
    /// The time-over sequence was running at the last recorded tick.
    time_over: bool,
    /// Goal tape anchor height from the tape model (`loadGoalTapeAnchorY`).
    pub goal_tape_anchor_y: Option<f64>,
    /// Bounds of the stage models for the fly-in camera (`loadStageModelBounds`).
    pub bound_sphere: Option<BoundSphere>,
}

/// Union of the stage GMA's model bound spheres as one sphere (`computeGmaBoundSphere`).
fn stage_bound_sphere(gma: &mb_gma::Gma) -> Option<BoundSphere> {
    let mut bounds: Option<([f64; 3], [f64; 3])> = None;
    for m in gma.iter() {
        let r = m.bound_sphere_radius as f64;
        let c = m.bound_sphere_center.map(|v| v as f64);
        let (lo, hi) = (c.map(|v| v - r), c.map(|v| v + r));
        bounds = Some(match bounds {
            None => (lo, hi),
            Some((min, max)) => (
                [min[0].min(lo[0]), min[1].min(lo[1]), min[2].min(lo[2])],
                [max[0].max(hi[0]), max[1].max(hi[1]), max[2].max(hi[2])],
            ),
        });
    }
    let (min, max) = bounds?;
    let half = [
        (max[0] - min[0]) * 0.5,
        (max[1] - min[1]) * 0.5,
        (max[2] - min[2]) * 0.5,
    ];
    Some(BoundSphere {
        pos: mb_engine::math::Vec3::new(
            (max[0] + min[0]) * 0.5,
            (max[1] + min[1]) * 0.5,
            (max[2] + min[2]) * 0.5,
        ),
        radius: mb_engine::math::sqrt(half[0] * half[0] + half[1] * half[1] + half[2] * half[2]),
    })
}

/// A tape model with room for `segments` independent quads, sharing the material of `template`.
fn tape_model(template: &mb_gma::NlModel, segments: usize) -> Option<mb_gma::NlModel> {
    let mut mesh = template.meshes.first()?.clone();
    let n = segments * 4;
    mesh.vertices = mb_gma::Vertices {
        positions: vec![[0.0; 3]; n],
        normals: vec![[0.0, 0.0, 1.0]; n],
        colors: vec![[255; 4]; n],
        uv0: vec![[0.0; 2]; n],
        uv1: vec![[0.0; 2]; n],
        uv2: vec![[0.0; 2]; n],
    };
    mesh.indices = (0..segments as u32)
        .flat_map(|s| [0, 1, 2, 2, 1, 3].map(|i| s * 4 + i))
        .collect();
    Some(mb_gma::NlModel {
        meshes: vec![mesh],
        ..template.clone()
    })
}

impl PlayScene {
    /// Upload everything the parsed `assets` need and create the objects. `timings` carries what reading
    /// and parsing cost; the GPU side is added and the whole is published for `mbPerf.lastLoad()`.
    pub fn build(r: &mut Renderer, assets: StageAssets, mut timings: LoadTimings) -> Self {
        let loaded = Instant::now();
        r.load_stats = Default::default();
        let mut scene = StageScene::build(r, &assets);

        let mut common = |r: &mut Renderer, name: &str| -> Option<ModelId> {
            let model = assets.common.gma.model(name)?;
            let textures = scene
                .common_set
                .for_model(r, &assets.common.tpl, model)
                .to_vec();
            r.upload_model(model, &textures).ok()
        };
        let mut ball = Vec::new();
        for (name, depth) in BALL_MODELS {
            if let Some(model) = common(r, name) {
                let object = r.add_object(model, Layer::World, Mat4::IDENTITY);
                // The shells share a centre: keep the reference's inside-out order explicitly.
                r.set_sort_bias(object, Some(depth * 1e-3));
                ball.push(object);
            }
        }
        let bag_models = [
            common(r, "NEW_SCENT_BAG_WHOLE"),
            common(r, "NEW_SCENT_BAG_A"),
            common(r, "NEW_SCENT_BAG_B"),
        ];
        let goal_count: usize = assets
            .stagedef
            .anim_groups
            .iter()
            .map(|g| g.goals.len())
            .sum();
        let mut bags = Vec::new();
        for _ in 0..goal_count {
            let mut add = |m: Option<ModelId>| {
                m.map(|m| {
                    let o = r.add_object(m, Layer::World, Mat4::IDENTITY);
                    // `RenderSort.All`: the bag always goes through the sorted pass.
                    r.set_sort_bias(o, Some(0.0));
                    o
                })
            };
            bags.push(Bag {
                closed: add(bag_models[0]),
                open_a: add(bag_models[1]),
                open_b: add(bag_models[2]),
            });
        }
        // The sparkle's texture is in the common NaomiLib archive, like the tape and the digits below.
        let mut nl_set = LazyTextures::default();
        let sparkle = (assets.common.nl.as_ref())
            .and_then(|(obj, tpl)| particles::sparkle_texture(r, obj, tpl, &mut nl_set));
        let particles = Particles::build(r, &mut common, sparkle);
        // The shadow falls on the models of the anim groups, not on goals, bumpers or bananas
        // (`AnimGroup.prepareToRenderShadow`). Without its mask there is no shadow.
        let mask = particles::shadow_mask(
            r,
            &assets.common.gma,
            &assets.common.tpl,
            &mut scene.common_set,
        );
        let shadow_mask = mask.is_some();
        r.set_shadow_mask(mask);
        for it in scene.items.iter().filter(|it| it.kind == ItemKind::Model) {
            r.set_shadow_receiver(it.object, true);
        }

        // Goal tape and clock digits live in the common NaomiLib archive. Missing files only cost the
        // decoration, never the stage.
        let mut tapes = Vec::new();
        let mut clocks = Vec::new();
        let mut goal_tape_anchor_y = None;
        if let Some((obj, tpl)) = &assets.common.nl {
            let set = &mut nl_set;
            if let Some(template) = obj.model(NL_GOAL_TAPE) {
                goal_tape_anchor_y = Some(template.bound_sphere_center[1] as f64);
                let segments = 7;
                if let Some(model) = tape_model(template, segments) {
                    let textures = set.for_nl_model(r, tpl, &model).to_vec();
                    for _ in 0..goal_count {
                        let id = r.upload_nl_model(&model, &textures);
                        let vertices = model.meshes[0].vertices.clone();
                        tapes.push(Tape {
                            model: id,
                            object: r.add_object(id, Layer::World, Mat4::IDENTITY),
                            ticks: [vertices.clone(), vertices.clone()],
                            vertices,
                        });
                    }
                }
            }
            let mut digit_models = |r: &mut Renderer, first: usize| -> [Option<ModelId>; 10] {
                std::array::from_fn(|d| {
                    obj.model(first + d).map(|m| {
                        let textures = set.for_nl_model(r, tpl, m).to_vec();
                        r.upload_nl_model(m, &textures)
                    })
                })
            };
            let small = digit_models(r, NL_LCD_SMALL_0);
            let large = digit_models(r, NL_LCD_LARGE_0);
            // Hundredths, tenths, then seconds from the right (`Goal.drawTimer`).
            let layout = [
                (0.0, small),
                (-0.45, small),
                (0.0, large),
                (-0.6666, large),
                (-1.3332, large),
            ];
            for goal in 0..goal_count {
                let slots = layout
                    .iter()
                    .map(|(x, models)| {
                        let objects = models.map(|m| {
                            m.map(|m| {
                                let o = r.add_object(m, Layer::World, Mat4::IDENTITY);
                                r.set_visible(o, false);
                                o
                            })
                        });
                        (*x, objects)
                    })
                    .collect();
                clocks.push(Clock { goal, slots });
            }
        }

        let bound_sphere = assets
            .stage_gma
            .as_ref()
            .and_then(|(gma, _)| stage_bound_sphere(gma));
        // Everything that belongs to the stage tilts with it; the backdrop does not.
        let tilting = (scene.items.iter().map(|it| it.object))
            .chain(scene.fg_objects.iter().copied())
            .chain(ball.iter().copied())
            .chain(
                bags.iter()
                    .flat_map(|b| [b.closed, b.open_a, b.open_b])
                    .flatten(),
            )
            .chain(tapes.iter().map(|t| t.object))
            .chain(
                (clocks.iter())
                    .flat_map(|c| c.slots.iter())
                    .flat_map(|(_, digits)| digits.iter().copied().flatten()),
            );
        for object in tilting {
            r.set_tilted(object, true);
        }
        let objects = r.object_count();

        let stats = r.load_stats;
        timings.texture_decode_ms = stats.texture_decode_ms;
        timings.texture_upload_ms = stats.texture_upload_ms;
        timings.texture_bytes = stats.texture_bytes;
        timings.textures = stats.textures;
        timings.mesh_ms = stats.mesh_ms;
        timings.gpu_total_ms = (Instant::now() - loaded).as_secs_f64() * 1000.0;
        timings.total_ms =
            timings.disc_read_ms + timings.lzss_ms + timings.parse_ms + timings.gpu_total_ms;
        crate::perf::set_last_load(timings);

        // The parsed archives are on the GPU now; only the stagedef is needed from here on.
        let stage_id = assets.id;
        let assets = PlayAssets {
            stagedef: assets.stagedef,
        };
        Self {
            stage_id,
            assets,
            scene,
            poses: [Pose::default(), Pose::default()],
            blend: false,
            objects,
            groups: Vec::new(),
            ball,
            bags,
            tapes,
            clocks,
            particles,
            tilt: Mat4::IDENTITY,
            shadow_mask,
            shadow: false,
            particle_ticks: 0,
            particles_frozen: false,
            time_over: false,
            goal_tape_anchor_y,
            bound_sphere,
        }
    }

    /// The stage this scene shows.
    pub fn stage_id(&self) -> u32 {
        self.stage_id
    }

    /// Forget the recorded ticks: the next [`PlayScene::sync`] starts a new motion (stage start, retry), so
    /// nothing is blended across the jump.
    pub fn reset_pose(&mut self) {
        self.blend = false;
        self.particles.reset();
        self.particle_ticks = 0;
        self.particles_frozen = false;
        self.time_over = false;
    }

    /// Whether the ball's shadow is drawn in the frame last posed (tests).
    pub fn shadow_visible(&self) -> bool {
        self.shadow
    }

    /// Free the scene's models, objects and textures.
    pub fn unload(&self, r: &mut Renderer) {
        r.clear_scene();
        r.truncate_textures(self.scene.texture_base);
    }

    /// Record the pose of the simulation's current tick: once for the initial state and once after every
    /// `Sim::step`. Does not touch the renderer; [`PlayScene::apply`] does, once per drawn frame.
    pub fn sync(&mut self, sim: &Sim) {
        let Self {
            assets,
            scene,
            poses,
            blend,
            objects,
            groups,
            ball: _,
            bags,
            tapes,
            clocks,
            particle_ticks,
            particles_frozen,
            time_over,
            ..
        } = self;
        // The simulation leaves its particles alone in a tick that starts with the clock run out. The
        // first record of an attempt is its initial state, not a tick.
        *particles_frozen = *time_over;
        if *blend && !*particles_frozen {
            *particle_ticks += 1;
        }
        *time_over = sim.timeover_timer > 0;
        poses.swap(0, 1);
        let (prev, pose) = poses.split_at_mut(1);
        let (prev, pose) = (&mut prev[0], &mut pose[0]);
        pose.begin(*objects);

        let rt = &sim.runtime;
        let pivot = vec3(sim.ball.pos);
        groups.clear();
        groups.extend(rt.anim_groups.iter().map(|g| mat4_from_mtx(&g.transform)));
        let group = |i: usize| groups.get(i).copied().unwrap_or(Mat4::IDENTITY);

        for it in &scene.items {
            let g = group(it.group);
            match it.kind {
                ItemKind::Model | ItemKind::Goal(_) => pose.set(it.object, g * it.local),
                ItemKind::Bumper(i) => {
                    let m = match rt.bumpers.get(it.group).and_then(|b| b.get(i)) {
                        // The simulation's own pose: spin plus the squash when the ball hits it.
                        Some(b) => {
                            trs_zyx(vec3(b.base_pos), b.rot)
                                * Mat4::from_rotation_y(s16(b.spin))
                                * Mat4::from_scale(Vec3::new(
                                    (b.scale.x * b.pulse_x) as f32,
                                    b.scale.y as f32,
                                    (b.scale.z * b.pulse_z) as f32,
                                ))
                        }
                        None => it.local,
                    };
                    pose.set(it.object, g * m);
                }
                ItemKind::Jamabar(i) => {
                    let m = match rt.jamabars.get(it.group).and_then(|j| j.get(i)) {
                        Some(j) => trs_zyx(vec3(j.pos), j.rot) * Mat4::from_scale(vec3(j.scale)),
                        None => it.local,
                    };
                    pose.set(it.object, g * m);
                }
                ItemKind::Banana(i) => {
                    let Some(b) = rt.bananas.get(i) else {
                        continue;
                    };
                    if b.state == 0 {
                        pose.hide(it.object);
                        continue;
                    }
                    let local = group(b.anim_group_id)
                        * trs_yxz(vec3(b.local_pos), [b.rot_x, b.rot_y, b.rot_z])
                        * Mat4::from_scale(Vec3::splat(b.scale as f32));
                    // A collected banana leaves the tilting stage over half a second (`bananaTiltFactor`):
                    // it then carries its own, weaker tilt instead of the stage's.
                    let factor = if b.state < 7 && !b.collected {
                        1.0
                    } else {
                        (b.tilt_timer.max(0) as f32 / 30.0).min(1.0)
                    };
                    if factor >= 1.0 {
                        pose.set(it.object, local);
                    } else {
                        let own = tilt_matrix(pivot, sim.world.xrot, sim.world.zrot, factor);
                        pose.set(it.object, own * local);
                        pose.slot(it.object).tilted = false;
                    }
                }
            }
        }

        for (bag, state) in bags.iter().zip(&rt.goal_bags) {
            let base = group(state.anim_group_id)
                * trs_yxz(
                    vec3(state.u_some_pos),
                    [state.rot_x, state.rot_y, state.rot_z],
                );
            let openness = state.openness as f32;
            let open = openness > 0.0 && bag.open_a.is_some() && bag.open_b.is_some();
            if let Some(o) = bag.closed {
                if open {
                    pose.hide(o);
                } else {
                    pose.set(o, base);
                }
            }
            let halves = base * Mat4::from_translation(Vec3::new(0.0, -0.5 * openness, 0.0));
            let swing = s16(9102) * openness;
            for (object, angle) in [(bag.open_a, -swing), (bag.open_b, swing)] {
                if let Some(o) = object {
                    if open {
                        pose.set(o, halves * Mat4::from_rotation_z(angle));
                    } else {
                        pose.hide(o);
                    }
                }
            }
        }

        for (tape, state) in tapes.iter_mut().zip(&rt.goal_tapes) {
            let base = group(state.anim_group_id) * trs_zyx(vec3(state.goal_pos), state.goal_rot);
            pose.set(tape.object, base);
            tape.ticks.swap(0, 1);
            let (before, v) = tape.ticks.split_at_mut(1);
            let v = &mut v[0];
            // Segments past the end of the strip keep their last shape.
            v.clone_from(&before[0]);
            for s in 0..v.positions.len() / 4 {
                let (Some(a), Some(b)) = (state.points.get(s), state.points.get(s + 1)) else {
                    break;
                };
                // A point without flag 4 ends the strip: its segment collapses into the point.
                let joined = a.flags & 4 != 0;
                let ends = if joined { [a, a, b, b] } else { [a; 4] };
                for (k, p) in ends.into_iter().enumerate() {
                    let n = vec3(p.normal);
                    let yaw = n.x.atan2(n.z) - std::f32::consts::PI;
                    let pitch = n.y.atan2((n.x * n.x + n.z * n.z).sqrt());
                    let up = Mat4::from_rotation_y(yaw) * Mat4::from_rotation_x(pitch);
                    let offset = up.transform_vector3(Vec3::Y * TAPE_HALF_HEIGHT);
                    let top = k % 2 == 0;
                    let edge = match (joined, top) {
                        (false, _) => Vec3::ZERO,
                        (true, true) => offset,
                        (true, false) => -offset,
                    };
                    let i = s * 4 + k;
                    v.positions[i] = (vec3(p.pos) + edge).to_array();
                    v.normals[i] = n.to_array();
                    let uv = [p.t as f32, if top { 1.0 } else { 0.0 }];
                    v.uv0[i] = uv;
                    v.uv1[i] = uv;
                    v.uv2[i] = uv;
                }
            }
        }

        // Goal clocks show the time left in hundredths of a second.
        let hundredths =
            ((sim.cfg.time_limit_frames - sim.stage_timer_frames).max(0) as i64) * 100 / 60;
        for clock in clocks.iter() {
            let Some(goal) = rt.goals.get(clock.goal) else {
                continue;
            };
            let base = group(goal.anim_group_id) * trs_zyx(vec3(goal.pos), goal.rot);
            let mut value = hundredths;
            for (x, digits) in &clock.slots {
                let digit = (value % 10) as usize;
                value /= 10;
                for (d, o) in digits.iter().enumerate() {
                    let Some(o) = o else {
                        continue;
                    };
                    if d == digit {
                        pose.set(*o, base * Mat4::from_translation(Vec3::new(*x, 0.0, 0.0)));
                    } else {
                        pose.hide(*o);
                    }
                }
            }
        }

        // Keyframed decoration. Objects without animation keep the transform they were created with.
        let seconds = rt.timer_frames as f32 / 60.0;
        let stagedef = &assets.stagedef;
        for (object, source) in scene.fg_objects.iter().zip(&scene.fg_sources) {
            let Some(o) = stagedef
                .fg_objects
                .get(*source)
                .filter(|o| o.anim.is_some())
            else {
                continue;
            };
            match bg_object_pose(o, seconds) {
                Some((m, alpha)) => {
                    pose.set(*object, m);
                    pose.slot(*object).alpha = Some(alpha);
                }
                None => pose.hide(*object),
            }
        }
        for (object, source) in scene.bg_objects.iter().zip(&scene.bg_sources) {
            let Some(o) = stagedef
                .bg_objects
                .get(*source)
                .filter(|o| o.anim.is_some())
            else {
                continue;
            };
            match bg_object_pose(o, seconds) {
                Some((m, alpha)) => {
                    pose.set(*object, m);
                    pose.slot(*object).alpha = Some(alpha);
                }
                None => pose.hide(*object),
            }
            pose.slot(*object).tilted = false;
        }

        let o = sim.ball.orientation;
        let rotation = Quat::from_xyzw(o.x as f32, o.y as f32, o.z as f32, o.w as f32);
        pose.ball_rot = if rotation.length_squared() > 1e-12 {
            rotation.normalize()
        } else {
            Quat::IDENTITY
        };
        pose.ball_scale = sim.ball.curr_radius as f32 / BALL_BASE_RADIUS;
        pose.ball_visible = sim.ball.flags & ball_flags::INVISIBLE == 0;
        pose.pivot = pivot;
        pose.xrot = sim.world.xrot;
        pose.zrot = sim.world.zrot;

        // Nothing to blend from at the start of a motion, and nothing sensible across a teleport.
        if !*blend || (pose.pivot - prev.pivot).length() > TELEPORT_DISTANCE {
            prev.clone_from(pose);
            for tape in tapes.iter_mut() {
                let (before, now) = tape.ticks.split_at_mut(1);
                before[0].clone_from(&now[0]);
            }
        }
        *blend = true;
    }

    /// Pose the renderer for a frame that lies `alpha` (0..=1) of the way from the previous tick to the
    /// current one. Only what actually changed reaches the GPU.
    pub fn apply(&mut self, r: &mut Renderer, alpha: f32) {
        let t = alpha.clamp(0.0, 1.0);
        let [prev, curr] = &self.poses;
        if curr.slots.is_empty() {
            return;
        }
        for (c, p) in curr.slots.iter().zip(&prev.slots) {
            let Some(o) = c.object else {
                continue;
            };
            r.set_visible(o, c.visible);
            if !c.visible {
                continue;
            }
            r.set_tilted(o, c.tilted);
            // An object that just appeared or changed space has no previous pose to come from.
            let blend = p.object.is_some() && p.visible && p.tilted == c.tilted;
            if blend && p.m != c.m {
                r.set_transform(o, lerp_mat(&p.m, &c.m, t));
            } else {
                r.set_transform(o, c.m);
            }
            if let Some(alpha) = c.alpha {
                let from = if blend { p.alpha } else { None }.unwrap_or(alpha);
                r.set_alpha(o, from + (alpha - from) * t);
            }
        }

        let pivot = prev.pivot.lerp(curr.pivot, t);
        self.tilt = tilt_matrix_units(
            pivot,
            lerp_s16(prev.xrot, curr.xrot, t),
            lerp_s16(prev.zrot, curr.zrot, t),
            1.0,
        );
        r.set_tilt(self.tilt);
        // The ball spins too fast per tick for a matrix blend: blend position, rotation and size apart.
        let scale = prev.ball_scale + (curr.ball_scale - prev.ball_scale) * t;
        // `drawProjectedShadow`: cast from the ball as it is drawn, in stage space.
        let shadow = (self.shadow_mask && curr.ball_visible)
            .then(|| Shadow::projected(pivot, scale * BALL_BASE_RADIUS, &r.lighting))
            .flatten();
        self.shadow = shadow.is_some();
        r.set_shadow(shadow);
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(scale),
            prev.ball_rot.lerp(curr.ball_rot, t),
            pivot,
        );
        for object in &self.ball {
            r.set_visible(*object, curr.ball_visible);
            r.set_transform(*object, m);
        }

        for tape in &mut self.tapes {
            let [before, now] = &tape.ticks;
            let shown = &mut tape.vertices;
            let mut changed = false;
            if shown.normals != now.normals || shown.uv0 != now.uv0 {
                shown.normals.clone_from(&now.normals);
                shown.uv0.clone_from(&now.uv0);
                shown.uv1.clone_from(&now.uv1);
                shown.uv2.clone_from(&now.uv2);
                changed = true;
            }
            for ((out, a), b) in shown
                .positions
                .iter_mut()
                .zip(&before.positions)
                .zip(&now.positions)
            {
                let p = Vec3::from(*a).lerp(Vec3::from(*b), t).to_array();
                if *out != p {
                    *out = p;
                    changed = true;
                }
            }
            // The tape only moves while it settles or flutters: upload on change.
            if changed {
                r.write_vertices(tape.model, 0, shown);
            }
        }
    }
}

impl PlayScene {
    /// Queue this frame's particles (sparks, stars, flash, sparkle, glow, confetti) from the simulation
    /// whose ticks were recorded with [`PlayScene::sync`]. Call it after [`PlayScene::apply`], with the
    /// same `alpha` and the camera the frame is drawn with, for every frame: the queue is per frame.
    pub fn draw_particles(&mut self, r: &mut Renderer, sim: &Sim, camera: &Camera, alpha: f32) {
        let t = if self.particles_frozen { 1.0 } else { alpha };
        // What is shown lies between the previous tick and the current one.
        let now_ms = (f64::from(self.particle_ticks) - 1.0 + f64::from(t)) * TICK_MS;
        let frame = ParticleFrame {
            camera,
            tilt: self.tilt,
            t,
            now_ms,
        };
        self.particles.draw(r, sim, &frame);
    }
}

/// Keyframed pose and opacity of a background object at `seconds` (`BgObjectInst.update`); `None` while it
/// is hidden.
fn bg_object_pose(o: &mb_stage::BgObject, seconds: f32) -> Option<(Mat4, f32)> {
    let anim = o.anim.as_ref()?;
    let t = loop_wrap(seconds, anim.loop_start_seconds, anim.loop_end_seconds);
    if interpolate(t, &anim.visible).is_some_and(|v| v < 0.5) {
        return None;
    }
    let translucency = interpolate(t, &anim.translucency).unwrap_or(o.translucency);
    if translucency >= 1.0 {
        return None;
    }
    let key = |track: &[mb_stage::Keyframe], default: f32| interpolate(t, track).unwrap_or(default);
    let deg = |track: &[mb_stage::Keyframe], default: i16| {
        interpolate(t, track)
            .map(f32::to_radians)
            .unwrap_or(default as f32 * S16_TO_RADIANS)
    };
    let pos = Vec3::new(
        key(&anim.pos_x, o.pos.x),
        key(&anim.pos_y, o.pos.y),
        key(&anim.pos_z, o.pos.z),
    );
    let scale = Vec3::new(
        key(&anim.scale_x, o.scale.x),
        key(&anim.scale_y, o.scale.y),
        key(&anim.scale_z, o.scale.z),
    );
    let m = Mat4::from_translation(pos)
        * Mat4::from_rotation_z(deg(&anim.rot_z, o.rot.z))
        * Mat4::from_rotation_y(deg(&anim.rot_y, o.rot.y))
        * Mat4::from_rotation_x(deg(&anim.rot_x, o.rot.x))
        * Mat4::from_scale(scale);
    Some((m, 1.0 - translucency))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mtx_rows_become_columns() {
        let m: Mtx = [1., 0., 0., 5., 0., 1., 0., 6., 0., 0., 1., 7.];
        let p = mat4_from_mtx(&m).transform_point3(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(p, Vec3::new(6.0, 8.0, 10.0));
    }

    #[test]
    fn tilt_keeps_the_pivot_and_is_identity_when_level() {
        let pivot = Vec3::new(3.0, 1.0, -2.0);
        assert_eq!(tilt_matrix(pivot, 0, 0, 1.0), Mat4::IDENTITY);
        let t = tilt_matrix(pivot, 0x800, -0x400, 1.0);
        assert!((t.transform_point3(pivot) - pivot).length() < 1e-5);
        // A point ahead of the ball (-Z) dips when the stage tilts forward (negative X rotation).
        let ahead = tilt_matrix(pivot, -0x800, 0, 1.0).transform_point3(pivot + Vec3::NEG_Z);
        assert!(ahead.y < pivot.y);
        // Only 60 % of the simulated angle is shown.
        let shown = tilt_matrix(Vec3::ZERO, 0x1000, 0, 1.0).transform_vector3(Vec3::Y);
        let expect = (0x1000 as f32 * S16_TO_RADIANS * 0.6).cos();
        assert!((shown.y - expect).abs() < 1e-5);
    }

    #[test]
    fn s16_angles_blend_the_short_way_round() {
        assert_eq!(lerp_s16(0, 0x1000, 0.5), 0x800 as f32);
        assert_eq!(lerp_s16(0x7ff0, -0x7ff0, 0.5), 0x8000 as f32);
        assert_eq!(lerp_s16(-0x100, 0x100, 0.25), -0x80 as f32);
        // Angles that are not reduced to 16 bits still take the short way.
        assert_eq!(lerp_s16(0x10000, 0x100, 1.0), 0x10100 as f32);
    }

    #[test]
    fn poses_blend_and_snap() {
        let a = Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0));
        let b = Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(
            lerp_mat(&a, &b, 0.5).w_axis.truncate(),
            Vec3::new(2.0, 0.0, 0.0)
        );
        assert_eq!(lerp_mat(&a, &b, 0.0), a);
        assert_eq!(lerp_mat(&a, &b, 1.0), b);
        // The fractional tilt agrees with the integer one at whole angles.
        let pivot = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(
            tilt_matrix_units(pivot, 0x400 as f32, -0x200 as f32, 1.0),
            tilt_matrix(pivot, 0x400, -0x200, 1.0)
        );
    }

    #[test]
    fn tape_model_has_independent_quads() {
        let mesh = mb_gma::NlMesh {
            flags: 0,
            tex_flags: 0,
            tex_idx: None,
            mesh_type: 0,
            ambient_color_scale: 1.0,
            material_color: [1.0; 4],
            cull: mb_gma::CullMode::None,
            z_compare: 4,
            depth_write: true,
            vertices: Default::default(),
            indices: Vec::new(),
        };
        let template = mb_gma::NlModel {
            id: 14,
            flags: 1,
            kind: mb_gma::VertexKind::B,
            bound_sphere_center: [0.0, 0.74, 0.0],
            bound_sphere_radius: 1.0,
            meshes: vec![mesh.clone(), mesh],
        };
        let m = tape_model(&template, 7).unwrap();
        assert_eq!(m.meshes.len(), 1);
        assert_eq!(m.meshes[0].vertices.len(), 28);
        assert_eq!(m.meshes[0].indices.len(), 42);
        assert_eq!(&m.meshes[0].indices[6..12], &[4, 5, 6, 6, 5, 7]);
    }
}
