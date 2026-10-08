//! The 3D scene of a stage being played: the static stage scene plus everything the simulation moves (ball,
//! goal bags and tapes, goal clocks, bananas, bumpers, jamabars, animated backdrops), posed from an
//! `mb_engine::Sim` once per rendered frame.
//!
//! Mirrors the reference's `World.prepareToRender` / `AnimGroup.prepareToRender`
//! (`noclip/SuperMonkeyBall/World.ts`, `AnimGroup.ts`) fed by `Game.get*RenderState` (`game.ts`):
//! * The stage tilt is purely visual: everything that belongs to the stage is rotated about the ball by
//!   `world.xrot/zrot * 0.6` (`getTiltedViewMatrix`); the backdrop is not. The reference folds that rotation
//!   into the view matrix, here it is folded into each object's model matrix, which is the same product.
//! * Models, textures and objects are created once in [`PlayScene::load`]; [`PlayScene::sync`] only updates
//!   transforms, visibility and the 28 vertices of each goal tape.

use glam::{Mat4, Quat, Vec3};
use mb_engine::constants::ball_flags;
use mb_engine::math::Mtx;
use mb_engine::stage_runtime::BoundSphere;
use mb_engine::Sim;
use mb_render::{Layer, ModelId, ObjectId, Renderer};

use crate::anim::{interpolate, loop_wrap};
use crate::content::Content;
use crate::stage_info::BgInfo;
use crate::stage_scene::{ItemKind, StageAssets, StageScene, S16_TO_RADIANS};

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
    let rx = s16(xrot) * STAGE_TILT_SCALE * factor;
    let rz = s16(zrot) * STAGE_TILT_SCALE * factor;
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
    vertices: mb_gma::Vertices,
}

/// The five-digit clock above a goal: for each slot, one object per digit.
struct Clock {
    goal: usize,
    slots: Vec<(f32, [Option<ObjectId>; 10])>,
}

pub struct PlayScene {
    pub assets: StageAssets,
    pub scene: StageScene,
    fg_base: Vec<Mat4>,
    ball: Vec<ObjectId>,
    bags: Vec<Bag>,
    tapes: Vec<Tape>,
    clocks: Vec<Clock>,
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
    /// Parse and upload everything `stage_id` needs. `bg` overrides the stage's usual backdrop.
    pub fn load(
        r: &mut Renderer,
        content: &Content,
        stage_id: u32,
        bg: Option<BgInfo>,
    ) -> Result<Self, String> {
        let assets = StageAssets::load(content, stage_id, bg)
            .map_err(|e| format!("stage {stage_id}: {e}"))?;
        let scene = StageScene::build(r, &assets);
        let fg_base = scene.fg_objects.iter().map(|o| r.transform(*o)).collect();

        let common = |r: &mut Renderer, name: &str| -> Option<ModelId> {
            let model = assets.common_gma.model(name)?;
            r.upload_model(model, &scene.common_set).ok()
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

        // Goal tape and clock digits live in the common NaomiLib archive. Missing files only cost the
        // decoration, never the stage.
        let nl = Self::load_common_nl(content);
        let mut tapes = Vec::new();
        let mut clocks = Vec::new();
        let mut goal_tape_anchor_y = None;
        if let Some((obj, tpl)) = &nl {
            let set = r.upload_tpl(tpl);
            if let Some(template) = obj.model(NL_GOAL_TAPE) {
                goal_tape_anchor_y = Some(template.bound_sphere_center[1] as f64);
                let segments = 7;
                if let Some(model) = tape_model(template, segments) {
                    for _ in 0..goal_count {
                        let id = r.upload_nl_model(&model, &set);
                        tapes.push(Tape {
                            model: id,
                            object: r.add_object(id, Layer::World, Mat4::IDENTITY),
                            vertices: model.meshes[0].vertices.clone(),
                        });
                    }
                }
            }
            let digit_models = |r: &mut Renderer, first: usize| -> [Option<ModelId>; 10] {
                std::array::from_fn(|d| obj.model(first + d).map(|m| r.upload_nl_model(m, &set)))
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
        Ok(Self {
            assets,
            scene,
            fg_base,
            ball,
            bags,
            tapes,
            clocks,
            goal_tape_anchor_y,
            bound_sphere,
        })
    }

    fn load_common_nl(content: &Content) -> Option<(mb_gma::NlObj, mb_tpl::Tpl)> {
        let obj = content.read_lz("smb1_content/test/init/common_p.lz").ok()?;
        let tpl = content.read_lz("smb1_content/test/init/common.lz").ok()?;
        Some((
            mb_gma::parse_naomi_obj(&obj).ok()?,
            mb_tpl::parse(&tpl).ok()?,
        ))
    }

    /// Free the scene's models, objects and textures.
    pub fn unload(&self, r: &mut Renderer) {
        r.clear_scene();
        r.truncate_textures(self.scene.texture_base);
    }

    /// Pose every object from the simulation state.
    pub fn sync(&mut self, r: &mut Renderer, sim: &Sim) {
        let rt = &sim.runtime;
        let pivot = vec3(sim.ball.pos);
        let tilt = tilt_matrix(pivot, sim.world.xrot, sim.world.zrot, 1.0);
        let groups: Vec<Mat4> = rt
            .anim_groups
            .iter()
            .map(|g| mat4_from_mtx(&g.transform))
            .collect();
        let group = |i: usize| groups.get(i).copied().unwrap_or(Mat4::IDENTITY);

        for it in &self.scene.items {
            let g = group(it.group);
            match it.kind {
                ItemKind::Model | ItemKind::Goal(_) => {
                    r.set_transform(it.object, tilt * g * it.local)
                }
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
                    r.set_transform(it.object, tilt * g * m);
                }
                ItemKind::Jamabar(i) => {
                    let m = match rt.jamabars.get(it.group).and_then(|j| j.get(i)) {
                        Some(j) => trs_zyx(vec3(j.pos), j.rot) * Mat4::from_scale(vec3(j.scale)),
                        None => it.local,
                    };
                    r.set_transform(it.object, tilt * g * m);
                }
                ItemKind::Banana(i) => {
                    let Some(b) = rt.bananas.get(i) else {
                        continue;
                    };
                    r.set_visible(it.object, b.state != 0);
                    if b.state == 0 {
                        continue;
                    }
                    // A collected banana leaves the tilting stage over half a second (`bananaTiltFactor`).
                    let factor = if b.state < 7 && !b.collected {
                        1.0
                    } else {
                        (b.tilt_timer.max(0) as f32 / 30.0).min(1.0)
                    };
                    let stage = if factor >= 1.0 {
                        tilt
                    } else {
                        tilt_matrix(pivot, sim.world.xrot, sim.world.zrot, factor)
                    };
                    let local = trs_yxz(vec3(b.local_pos), [b.rot_x, b.rot_y, b.rot_z])
                        * Mat4::from_scale(Vec3::splat(b.scale as f32));
                    r.set_transform(it.object, stage * group(b.anim_group_id) * local);
                }
            }
        }

        for (bag, state) in self.bags.iter().zip(&rt.goal_bags) {
            let base = tilt
                * group(state.anim_group_id)
                * trs_yxz(
                    vec3(state.u_some_pos),
                    [state.rot_x, state.rot_y, state.rot_z],
                );
            let openness = state.openness as f32;
            let open = openness > 0.0 && bag.open_a.is_some() && bag.open_b.is_some();
            if let Some(o) = bag.closed {
                r.set_visible(o, !open);
                r.set_transform(o, base);
            }
            let halves = base * Mat4::from_translation(Vec3::new(0.0, -0.5 * openness, 0.0));
            let swing = s16(9102) * openness;
            for (object, angle) in [(bag.open_a, -swing), (bag.open_b, swing)] {
                if let Some(o) = object {
                    r.set_visible(o, open);
                    r.set_transform(o, halves * Mat4::from_rotation_z(angle));
                }
            }
        }

        for (tape, state) in self.tapes.iter_mut().zip(&rt.goal_tapes) {
            let base =
                tilt * group(state.anim_group_id) * trs_zyx(vec3(state.goal_pos), state.goal_rot);
            r.set_transform(tape.object, base);
            let mut v = tape.vertices.clone();
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
            // The tape only moves while it settles or flutters: upload on change.
            if v != tape.vertices && r.write_vertices(tape.model, 0, &v) {
                tape.vertices = v;
            }
        }

        // Goal clocks show the time left in hundredths of a second.
        let hundredths =
            ((sim.cfg.time_limit_frames - sim.stage_timer_frames).max(0) as i64) * 100 / 60;
        for clock in &self.clocks {
            let Some(goal) = rt.goals.get(clock.goal) else {
                continue;
            };
            let base = tilt * group(goal.anim_group_id) * trs_zyx(vec3(goal.pos), goal.rot);
            let mut value = hundredths;
            for (x, objects) in &clock.slots {
                let digit = (value % 10) as usize;
                value /= 10;
                for (d, o) in objects.iter().enumerate() {
                    if let Some(o) = o {
                        r.set_visible(*o, d == digit);
                        if d == digit {
                            r.set_transform(
                                *o,
                                base * Mat4::from_translation(Vec3::new(*x, 0.0, 0.0)),
                            );
                        }
                    }
                }
            }
        }

        let seconds = rt.timer_frames as f32 / 60.0;
        let stagedef = &self.assets.stagedef;
        for ((object, source), base) in self
            .scene
            .fg_objects
            .iter()
            .zip(&self.scene.fg_sources)
            .zip(&self.fg_base)
        {
            let local = match stagedef.fg_objects.get(*source) {
                Some(o) if o.anim.is_some() => match bg_object_pose(o, seconds) {
                    Some((m, alpha)) => {
                        r.set_alpha(*object, alpha);
                        Some(m)
                    }
                    None => None,
                },
                _ => Some(*base),
            };
            r.set_visible(*object, local.is_some());
            if let Some(m) = local {
                r.set_transform(*object, tilt * m);
            }
        }
        for (object, source) in self.scene.bg_objects.iter().zip(&self.scene.bg_sources) {
            let Some(o) = stagedef
                .bg_objects
                .get(*source)
                .filter(|o| o.anim.is_some())
            else {
                continue;
            };
            let pose = bg_object_pose(o, seconds);
            r.set_visible(*object, pose.is_some());
            if let Some((m, alpha)) = pose {
                r.set_transform(*object, m);
                r.set_alpha(*object, alpha);
            }
        }

        let visible = sim.ball.flags & ball_flags::INVISIBLE == 0;
        let o = sim.ball.orientation;
        let rotation = Quat::from_xyzw(o.x as f32, o.y as f32, o.z as f32, o.w as f32);
        let rotation = if rotation.length_squared() > 1e-12 {
            rotation.normalize()
        } else {
            Quat::IDENTITY
        };
        let scale = Vec3::splat(sim.ball.curr_radius as f32 / BALL_BASE_RADIUS);
        let m = tilt * Mat4::from_scale_rotation_translation(scale, rotation, pivot);
        for object in &self.ball {
            r.set_visible(*object, visible);
            r.set_transform(*object, m);
        }
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
