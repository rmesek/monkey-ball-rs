//! Which stages have a mirror floor, which of their models show the reflection, and where the mirror's
//! plane is. Pure CPU work on the parsed stage; `stage_scene.rs` turns the result into renderer objects
//! and `play_scene.rs` poses it.
//!
//! The mode is the reference's (`World.getMirrorMode`): the wavy mirror on the bonus wave, a flat one on
//! every stage whose stagedef names mirror models (and on stages 102 and 103). The plane is not the
//! reference's (`getMirrorPlaneMatrix`: the height of the first mirror model's bounding centre, anim
//! group 1's transform on stage 103, a fixed choice on a few others). It is read off the mirror models
//! themselves: the plane their vertices lie in, in the space of their anim group, riding on that group
//! when the group takes the plane with it. A renderer has one plane per frame, so a mirror is shown only
//! on models that share one ([`plan_mirror`]); `docs/GAMEPLAY.md` lists what that gives on the disc.

use glam::{Mat4, Quat, Vec3};
use mb_render::Mirror;
use mb_stage::Stage;

use crate::anim::world_from_group;
use crate::backdrop::{BONUS_WAVE_MODEL_NAME, BONUS_WAVE_STAGE_ID};
use crate::stage_scene::{naomi_model, StageAssets};

/// `StageId.St102_Hitter` and `StageId.St103_Av_Logo`: flat mirrors whatever the stagedef names.
pub const HITTER_STAGE_ID: u32 = 102;
pub const AV_LOGO_STAGE_ID: u32 = 103;
/// A background or foreground object with this flag is left out of the mirror (`BgObjectInst`).
pub const BG_FLAG_NOT_IN_MIRROR: u32 = 1 << 7;
/// An anim-group model shows in the mirror only with this flag (`AnimGroup.prepareToRender`).
pub const MODEL_FLAG_IN_MIRROR: u32 = 1 << 2;

/// `MirrorMode` of the reference, without its `none`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MirrorKind {
    Flat,
    /// The floor of the bonus wave. (The name is the reference's; its distortion is not applied, see
    /// `docs/GAMEPLAY.md`.)
    Wavy,
}

/// `World.getMirrorMode`: the wavy mirror on the bonus wave when its floor was found, else a flat one
/// when the stagedef names mirror models (`named` of them) or the stage is 102 or 103.
pub fn mirror_kind(stage_id: u32, wave_floor: bool, named: usize) -> Option<MirrorKind> {
    if stage_id == BONUS_WAVE_STAGE_ID && wave_floor {
        return Some(MirrorKind::Wavy);
    }
    let flat = named > 0 || stage_id == HITTER_STAGE_ID || stage_id == AV_LOGO_STAGE_ID;
    flat.then_some(MirrorKind::Flat)
}

/// Where a mirror's plane is, in stage space before the tilt: through the matrix's origin, facing along
/// its +Y axis (`mb_render::Mirror::plane`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MirrorPlane {
    Fixed(Mat4),
    /// Rides on an anim group: the group's transform times `local`.
    Group {
        group: usize,
        local: Mat4,
    },
}

impl MirrorPlane {
    /// The plane's matrix when anim group `i` is at `group(i)`.
    pub fn at(&self, group: impl Fn(usize) -> Mat4) -> Mat4 {
        match *self {
            MirrorPlane::Fixed(m) => m,
            MirrorPlane::Group { group: i, local } => group(i) * local,
        }
    }

    /// Whether the plane has to be set again for every frame.
    pub fn moves(&self) -> bool {
        matches!(self, MirrorPlane::Group { .. })
    }
}

/// `WAVY_MIRROR_PLANE_Y`: the plane of the bonus wave's mirror, 0.02 above the floor at rest.
pub fn wave_mirror_plane() -> MirrorPlane {
    let lift = Vec3::new(0.0, Mirror::WAVY_PLANE_Y, 0.0);
    MirrorPlane::Fixed(Mat4::from_translation(lift))
}

/// How far a vertex may be from the plane of its mirror model, and a mirror model from the plane of
/// another, to count as lying in it. The models float 0.01 to 0.015 above their floors.
pub const PLANE_TOLERANCE: f32 = 0.02;
/// Of a mirror model's vertices, one in this many may lie off its plane (stage 48: four of 1088, in
/// four slanted triangles of 1034, half a unit down). They show the reflection too, slightly displaced.
pub const STRAY_VERTICES: usize = 100;
/// Two planes are parallel when their normals agree to this cosine (about 1.4 degrees).
const PARALLEL: f32 = 0.9997;
/// A plane that rides on a group counts as standing still when it stays within these over the group's
/// whole motion (the group then only moves along it).
const STILL_NORMAL: f32 = 1.0 - 1e-6;
const STILL_HEIGHT: f32 = 0.002;
/// Moments of the stage's animation loop at which the planes are compared.
pub const POSE_SAMPLES: usize = 128;

/// A mirror model as the plane rule sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct MirrorCandidate {
    /// The anim group the model is drawn with.
    pub group: usize,
    /// Its vertices, in the model's own space.
    pub points: Vec<Vec3>,
    /// The way it faces: the sum of its vertex normals (any length).
    pub normal: Vec3,
}

impl MirrorCandidate {
    /// The vertices of every mesh of `model`; `None` when it has none that can be read.
    pub fn from_model(group: usize, model: &mb_gma::Model) -> Option<Self> {
        let meshes = model.meshes().ok()?;
        let vertices = || meshes.iter().map(|m| &m.vertices);
        let points: Vec<Vec3> = vertices()
            .flat_map(|v| v.positions.iter().map(|p| Vec3::from(*p)))
            .filter(|p| p.is_finite())
            .collect();
        let normal = vertices()
            .flat_map(|v| v.normals.iter().map(|n| Vec3::from(*n)))
            .filter(|n| n.is_finite())
            .sum();
        (!points.is_empty()).then_some(Self {
            group,
            points,
            normal,
        })
    }
}

/// A candidate that lies in a plane: the plane (in the model's space) and the vertices in it.
struct Flat<'a> {
    index: usize,
    group: usize,
    /// The point of the plane nearest the origin of the model's space.
    origin: Vec3,
    normal: Vec3,
    candidate: &'a MirrorCandidate,
}

impl<'a> Flat<'a> {
    /// The plane through the middle height of the vertices, facing the way their normals do; `None`
    /// when more than one vertex in [`STRAY_VERTICES`] is further than [`PLANE_TOLERANCE`] from it.
    fn of(index: usize, candidate: &'a MirrorCandidate) -> Option<Self> {
        let normal = candidate.normal.try_normalize()?;
        let mut heights: Vec<f32> = candidate.points.iter().map(|p| normal.dot(*p)).collect();
        if heights.is_empty() || heights.iter().any(|h| !h.is_finite()) {
            return None;
        }
        heights.sort_by(f32::total_cmp);
        let middle = heights[heights.len() / 2];
        let stray = heights
            .iter()
            .filter(|h| (*h - middle).abs() > PLANE_TOLERANCE);
        (stray.count() <= heights.len() / STRAY_VERTICES).then_some(Self {
            index,
            group: candidate.group,
            origin: normal * middle,
            normal,
            candidate,
        })
    }

    /// The plane as a point and a normal in stage space, at one moment.
    fn world(&self, pose: &[Mat4]) -> (Vec3, Vec3) {
        let m = pose.get(self.group).copied().unwrap_or(Mat4::IDENTITY);
        let normal = m.transform_vector3(self.normal).normalize_or_zero();
        (m.transform_point3(self.origin), normal)
    }

    /// Whether `other` lies in this plane, facing the same way, at every moment of `poses`.
    fn holds(&self, other: &Flat, poses: &[Vec<Mat4>]) -> bool {
        poses.iter().all(|pose| {
            let (origin, normal) = self.world(pose);
            let (at, facing) = other.world(pose);
            let m = pose.get(other.group).copied().unwrap_or(Mat4::IDENTITY);
            // The other's own strays aside, every vertex of it is in the plane.
            let off = |p: &Vec3| {
                let own = (other.normal.dot(*p - other.origin)).abs() <= PLANE_TOLERANCE;
                own && normal.dot(m.transform_point3(*p) - origin).abs() > PLANE_TOLERANCE
            };
            normal.dot(facing) >= PARALLEL
                && normal.dot(at - origin).abs() <= PLANE_TOLERANCE
                && !other.candidate.points.iter().any(off)
        })
    }

    fn parallel(&self, other: &Flat, poses: &[Vec<Mat4>]) -> bool {
        let agree = |pose: &Vec<Mat4>| self.world(pose).1.dot(other.world(pose).1) >= PARALLEL;
        poses.iter().all(agree)
    }

    /// The plane as [`MirrorPlane`]: fixed when it never moves, else riding on the model's group.
    fn plane(&self, poses: &[Vec<Mat4>]) -> MirrorPlane {
        let turn = Quat::from_rotation_arc(Vec3::Y, self.normal);
        let local = Mat4::from_translation(self.origin) * Mat4::from_quat(turn);
        let Some(first) = poses.first() else {
            return MirrorPlane::Fixed(local);
        };
        let (origin, normal) = self.world(first);
        let still = poses.iter().all(|pose| {
            let (o, n) = self.world(pose);
            n.dot(normal) >= STILL_NORMAL && normal.dot(o - origin).abs() <= STILL_HEIGHT
        });
        if still {
            let m = first.get(self.group).copied().unwrap_or(Mat4::IDENTITY);
            // Where it stands, as a plane of its own: the nearest point to the stage's origin.
            let at = normal * normal.dot(m.transform_point3(self.origin));
            let turn = Quat::from_rotation_arc(Vec3::Y, normal);
            MirrorPlane::Fixed(Mat4::from_translation(at) * Mat4::from_quat(turn))
        } else {
            MirrorPlane::Group {
                group: self.group,
                local,
            }
        }
    }
}

/// One plane of a stage's mirror and the candidates (by index) that lie in it.
#[derive(Clone, Debug, PartialEq)]
pub struct MirrorLevel {
    pub plane: MirrorPlane,
    pub surfaces: Vec<usize>,
}

/// What [`plan_mirror`] makes of a stage's mirror models.
#[derive(Clone, Debug, PartialEq)]
pub struct MirrorPlan {
    /// The planes the mirror can be in, highest first. One, except where the models lie in parallel
    /// planes at different heights (stage 110): the frame then mirrors in the highest one the camera
    /// is above ([`level_for_eye`]), as the reference chooses on that stage, and shows only its models.
    pub levels: Vec<MirrorLevel>,
    /// Candidates that are in none of them: not flat, or in a plane that leans against the others.
    /// They are not drawn, as without reflections.
    pub dropped: Vec<usize>,
}

/// Sort `candidates` into the planes they lie in. `poses` are the transforms of the stage's anim groups
/// at the moments to compare (one list per moment, by group index; a missing group is at rest).
///
/// * A candidate is in a plane when all its vertices but strays are ([`PLANE_TOLERANCE`],
///   [`STRAY_VERTICES`]); one that is not flat is dropped.
/// * Candidates share a plane when they lie in it at every moment, whatever groups they ride on.
/// * The plane with the most candidates is the mirror's; planes parallel to it at every moment are
///   further levels of it, any other plane is dropped.
///
/// `None` when no candidate is flat.
pub fn plan_mirror(candidates: &[MirrorCandidate], poses: &[Vec<Mat4>]) -> Option<MirrorPlan> {
    let flats: Vec<Flat> = (candidates.iter().enumerate())
        .filter_map(|(i, c)| Flat::of(i, c))
        .collect();
    // Each set is led by its first member, whose plane the others are held against.
    let mut sets: Vec<Vec<&Flat>> = Vec::new();
    for flat in &flats {
        match sets.iter_mut().find(|set| set[0].holds(flat, poses)) {
            Some(set) => set.push(flat),
            None => sets.push(vec![flat]),
        }
    }
    let size = |set: &Vec<&Flat>| {
        let points: usize = set.iter().map(|f| f.candidate.points.len()).sum();
        (set.len(), points)
    };
    // The largest; of equals the first named.
    let main = (0..sets.len()).rev().max_by_key(|i| size(&sets[*i]))?;
    let lead = sets[main][0];
    let rest: Vec<f32> = match poses.first() {
        Some(pose) => {
            let (origin, normal) = lead.world(pose);
            let height = |set: &Vec<&Flat>| normal.dot(set[0].world(pose).0 - origin);
            sets.iter().map(height).collect()
        }
        None => vec![0.0; sets.len()],
    };
    let mut order: Vec<usize> = (0..sets.len())
        .filter(|i| *i == main || lead.parallel(sets[*i][0], poses))
        .collect();
    order.sort_by(|a, b| rest[*b].total_cmp(&rest[*a]));
    let levels: Vec<MirrorLevel> = order
        .iter()
        .map(|i| MirrorLevel {
            plane: sets[*i][0].plane(poses),
            surfaces: sets[*i].iter().map(|f| f.index).collect(),
        })
        .collect();
    let kept = |i: &usize| levels.iter().any(|l| l.surfaces.contains(i));
    let dropped = (0..candidates.len()).filter(|i| !kept(i)).collect();
    Some(MirrorPlan { levels, dropped })
}

/// The level a frame mirrors in: the first (highest) of `planes` the eye is not below, else the last.
/// On stage 110 that is the reference's choice between its two floors (`cameraPos[1] >= -9.0`).
pub fn level_for_eye(planes: &[Mat4], eye: Vec3) -> usize {
    let above = |plane: &Mat4| {
        let normal = plane.transform_vector3(Vec3::Y);
        normal.dot(eye - plane.transform_point3(Vec3::ZERO)) >= 0.0
    };
    let found = planes.iter().position(above);
    found.unwrap_or(planes.len().saturating_sub(1))
}

/// The transforms of every anim group at [`POSE_SAMPLES`] moments spread over the stage's animation
/// loop, the first of them its start.
pub fn group_poses(stage: &Stage) -> Vec<Vec<Mat4>> {
    let range = (stage.loop_start_seconds, stage.loop_end_seconds);
    let span = (range.1 - range.0).max(0) as f32;
    let moments = if span > 0.0 { POSE_SAMPLES } else { 1 };
    (0..moments)
        .map(|k| {
            let time = range.0 as f32 + span * k as f32 / moments as f32;
            let groups = stage.anim_groups.iter().enumerate();
            groups
                .map(|(i, g)| world_from_group(g, i, time, range))
                .collect()
        })
        .collect()
}

/// A mirror model a stagedef names, and the anim group it is drawn with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirrorEntry {
    pub group: usize,
    pub name: String,
}

/// `mirrorModelNames`: the mirror models of every anim group, then those of the stage's own list that
/// no group names (`mirrorNamesInGroups`), which are drawn in stage space like the models of group 0.
pub fn mirror_entries(stage: &Stage) -> Vec<MirrorEntry> {
    let mut entries = Vec::new();
    for (group, g) in stage.anim_groups.iter().enumerate() {
        for name in &g.mirror_model_names {
            entries.push(MirrorEntry {
                group,
                name: name.clone(),
            });
        }
    }
    let grouped = entries.len();
    for name in &stage.mirror_names {
        if !entries[..grouped].iter().any(|e| &e.name == name) {
            entries.push(MirrorEntry {
                group: 0,
                name: name.clone(),
            });
        }
    }
    entries
}

/// Why a stage has no mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoMirror {
    /// The stagedef names no mirror model (and the stage is none of the three special ones).
    NotNamed,
    /// None of the named models is in the stage's or the background's archive.
    NoModel,
    /// No named model lies in a plane.
    NotFlat,
}

/// The mirror of a stage, decided from its parsed files.
#[derive(Clone, Debug, PartialEq)]
pub enum StageMirror {
    None(NoMirror),
    /// The bonus wave: its floor shows the reflection over its own picture.
    Wavy(MirrorPlane),
    Flat {
        /// The models the stagedef names.
        entries: Vec<MirrorEntry>,
        /// Of them, how many are in neither archive (`GmaSrc.StageAndBg`).
        missing: usize,
        /// Levels and dropped models, as indices into `entries`.
        plan: MirrorPlan,
    },
}

impl StageMirror {
    pub fn kind(&self) -> Option<MirrorKind> {
        match self {
            StageMirror::None(_) => None,
            StageMirror::Wavy(_) => Some(MirrorKind::Wavy),
            StageMirror::Flat { .. } => Some(MirrorKind::Flat),
        }
    }

    /// The planes of the mirror, highest first.
    pub fn planes(&self) -> Vec<MirrorPlane> {
        match self {
            StageMirror::None(_) => Vec::new(),
            StageMirror::Wavy(plane) => vec![*plane],
            StageMirror::Flat { plan, .. } => plan.levels.iter().map(|l| l.plane).collect(),
        }
    }
}

/// Decide the mirror of a stage: [`mirror_kind`], and for a flat one [`plan_mirror`] over the models its
/// stagedef names, looked up like the reference's `getModel(name, GmaSrc.StageAndBg)`.
pub fn stage_mirror(a: &StageAssets) -> StageMirror {
    let wave = a.id == BONUS_WAVE_STAGE_ID
        && (a.naomi.as_ref()).is_some_and(|n| naomi_model(n, BONUS_WAVE_MODEL_NAME).is_some());
    let entries = mirror_entries(&a.stagedef);
    match mirror_kind(a.id, wave, a.stagedef.mirror_names.len()) {
        None => return StageMirror::None(NoMirror::NotNamed),
        Some(MirrorKind::Wavy) => return StageMirror::Wavy(wave_mirror_plane()),
        Some(MirrorKind::Flat) => {}
    }
    let model = |name: &str| {
        let stage = a.stage_gma.as_ref().and_then(|(gma, _)| gma.model(name));
        stage.or_else(|| a.backdrop.gma.model(name))
    };
    // Candidates for the entries whose model exists, and the entry each belongs to.
    let found: Vec<(usize, MirrorCandidate)> = (entries.iter().enumerate())
        .filter_map(|(i, e)| Some((i, MirrorCandidate::from_model(e.group, model(&e.name)?)?)))
        .collect();
    if found.is_empty() {
        let reason = if entries.is_empty() {
            NoMirror::NotNamed
        } else {
            NoMirror::NoModel
        };
        return StageMirror::None(reason);
    }
    let candidates: Vec<MirrorCandidate> = found.iter().map(|(_, c)| c.clone()).collect();
    let Some(mut plan) = plan_mirror(&candidates, &group_poses(&a.stagedef)) else {
        return StageMirror::None(NoMirror::NotFlat);
    };
    // From indices of candidates to indices of entries.
    let entry = |i: &mut usize| *i = found[*i].0;
    plan.levels
        .iter_mut()
        .for_each(|l| l.surfaces.iter_mut().for_each(entry));
    plan.dropped.iter_mut().for_each(entry);
    StageMirror::Flat {
        missing: entries.len() - found.len(),
        entries,
        plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square of side 2 in the plane y = `height` around (`x`, `z`), facing up.
    fn square(group: usize, x: f32, height: f32, z: f32) -> MirrorCandidate {
        let corner = |dx: f32, dz: f32| Vec3::new(x + dx, height, z + dz);
        MirrorCandidate {
            group,
            points: vec![
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ],
            normal: Vec3::Y * 4.0,
        }
    }

    fn at_rest(groups: usize) -> Vec<Vec<Mat4>> {
        vec![vec![Mat4::IDENTITY; groups]]
    }

    fn level_surfaces(plan: &MirrorPlan) -> Vec<Vec<usize>> {
        plan.levels.iter().map(|l| l.surfaces.clone()).collect()
    }

    #[test]
    fn the_mode_is_the_reference_one() {
        // The wave needs its floor; then any named mirror model, or stage 102 or 103.
        assert_eq!(mirror_kind(92, true, 0), Some(MirrorKind::Wavy));
        assert_eq!(mirror_kind(92, false, 0), None);
        assert_eq!(mirror_kind(92, false, 1), Some(MirrorKind::Flat));
        for stage in [102, 103] {
            assert_eq!(mirror_kind(stage, false, 0), Some(MirrorKind::Flat));
            assert_eq!(mirror_kind(stage, true, 1), Some(MirrorKind::Flat));
        }
        for stage in [0, 1, 48, 91, 104, 130] {
            assert_eq!(mirror_kind(stage, true, 0), None, "stage {stage}");
            assert_eq!(mirror_kind(stage, false, 3), Some(MirrorKind::Flat));
        }
        assert_eq!((BG_FLAG_NOT_IN_MIRROR, MODEL_FLAG_IN_MIRROR), (0x80, 4));
        // `WAVY_MIRROR_PLANE_Y`.
        let wave = wave_mirror_plane();
        assert_eq!(
            wave,
            MirrorPlane::Fixed(Mat4::from_translation(Vec3::new(0.0, 0.02, 0.0)))
        );
        assert!(!wave.moves());
        let anywhere = wave.at(|_| Mat4::from_scale(Vec3::splat(9.0)));
        assert_eq!(anywhere.w_axis.y, 0.02);
    }

    #[test]
    fn one_floor_gives_one_fixed_plane_at_its_height() {
        let plan = plan_mirror(&[square(0, 3.0, 0.015, -7.0)], &at_rest(1)).unwrap();
        assert_eq!(level_surfaces(&plan), [[0]]);
        assert!(plan.dropped.is_empty());
        // As the reference writes it: a translation to the floor's height, nothing sideways.
        let at = Mat4::from_translation(Vec3::new(0.0, 0.015, 0.0));
        let MirrorPlane::Fixed(m) = plan.levels[0].plane else {
            panic!("a floor that stands still");
        };
        assert!(m.abs_diff_eq(at, 1e-6), "{m}");
        // No moments at all: the plane of the model as it is.
        let plan = plan_mirror(&[square(0, 3.0, 0.015, -7.0)], &[]).unwrap();
        assert_eq!(plan.levels[0].plane, MirrorPlane::Fixed(at));
        assert_eq!(plan_mirror(&[], &at_rest(1)), None);
    }

    #[test]
    fn models_in_one_plane_share_the_mirror_whatever_group_carries_them() {
        // Three tiles at one height on three groups; two of the groups slide along the plane, in
        // different directions, at the second moment.
        let tiles = [
            square(0, 0.0, 1.0, 0.0),
            square(1, 5.0, 1.0, 0.0),
            square(2, -5.0, 1.0, 3.0),
        ];
        let slide = |x: f32, z: f32| Mat4::from_translation(Vec3::new(x, 0.0, z));
        let poses = vec![
            vec![Mat4::IDENTITY; 3],
            vec![Mat4::IDENTITY, slide(4.0, 0.0), slide(-2.0, 7.0)],
        ];
        let plan = plan_mirror(&tiles, &poses).unwrap();
        assert_eq!(level_surfaces(&plan), [[0, 1, 2]]);
        assert!(!plan.levels[0].plane.moves());

        // The third group lifts its tile at the second moment: it leaves the plane and is dropped.
        let lifted = vec![
            vec![Mat4::IDENTITY; 3],
            vec![
                Mat4::IDENTITY,
                slide(4.0, 0.0),
                Mat4::from_translation(Vec3::new(0.0, 0.5, 0.0)),
            ],
        ];
        let plan = plan_mirror(&tiles, &lifted).unwrap();
        // It is parallel still, so it is a level of its own, above or below as it starts: the same
        // height here, and the larger set keeps the mirror while the camera is above both.
        assert_eq!(plan.levels.len(), 2);
        assert!(plan.levels.iter().any(|l| l.surfaces == [0, 1]));
        assert!(plan.levels.iter().any(|l| l.surfaces == [2]));
        assert!(plan.dropped.is_empty());

        // It tips instead: no longer parallel, so it is not part of the mirror at all.
        let tipped = vec![
            vec![Mat4::IDENTITY; 3],
            vec![Mat4::IDENTITY, slide(4.0, 0.0), Mat4::from_rotation_x(0.3)],
        ];
        let plan = plan_mirror(&tiles, &tipped).unwrap();
        assert_eq!(level_surfaces(&plan), [[0, 1]]);
        assert_eq!(plan.dropped, [2]);
    }

    #[test]
    fn a_mirror_on_a_group_that_takes_it_along_rides_on_the_group() {
        // The mirror of stage 103 in small: a tile authored leaning, on a group that turns.
        let lean = Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0)) * Mat4::from_rotation_z(0.15);
        let mut tile = square(1, 0.0, 0.0, 0.0);
        for p in &mut tile.points {
            *p = lean.transform_point3(*p);
        }
        tile.normal = lean.transform_vector3(Vec3::Y) * 4.0;
        let turn = Mat4::from_rotation_x(0.2);
        let poses = vec![vec![Mat4::IDENTITY; 2], vec![Mat4::IDENTITY, turn]];
        let plan = plan_mirror(std::slice::from_ref(&tile), &poses).unwrap();
        let plane = plan.levels[0].plane;
        assert!(matches!(plane, MirrorPlane::Group { group: 1, .. }));
        // Wherever the group is, the tile's corners are in the plane and it faces the tile's way.
        for group in [
            Mat4::IDENTITY,
            turn,
            Mat4::from_translation(Vec3::ONE) * turn,
        ] {
            let m = plane.at(|i| {
                assert_eq!(i, 1);
                group
            });
            let origin = m.transform_point3(Vec3::ZERO);
            let normal = m.transform_vector3(Vec3::Y);
            let facing = group.transform_vector3(tile.normal).normalize();
            assert!((normal - facing).length() < 1e-5, "{normal} {facing}");
            for p in &tile.points {
                let gap = normal.dot(group.transform_point3(*p) - origin);
                assert!(gap.abs() < 1e-4, "{gap}");
            }
        }
        // The same tile on a group that only spins about the tile's own normal stands still.
        let axis = tile.normal.normalize();
        let about = Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0))
            * Mat4::from_axis_angle(axis, 1.0)
            * Mat4::from_translation(Vec3::new(0.0, -5.0, 0.0));
        let spin = vec![vec![Mat4::IDENTITY; 2], vec![Mat4::IDENTITY, about]];
        let plan = plan_mirror(std::slice::from_ref(&tile), &spin).unwrap();
        assert!(!plan.levels[0].plane.moves());
    }

    #[test]
    fn floors_at_two_heights_are_two_levels_chosen_by_the_eye() {
        // Stage 110 in small: a floor at -9 named first and one at -43 on another group.
        let floors = [square(0, 0.0, -8.99, 22.0), square(4, 0.0, -42.99, -75.0)];
        let plan = plan_mirror(&floors, &at_rest(5)).unwrap();
        assert_eq!(level_surfaces(&plan), [[0], [1]]);
        assert!(plan.dropped.is_empty());
        let planes: Vec<Mat4> = (plan.levels.iter())
            .map(|l| l.plane.at(|_| Mat4::IDENTITY))
            .collect();
        assert!((planes[0].w_axis.y + 8.99).abs() < 1e-5);
        assert!((planes[1].w_axis.y + 42.99).abs() < 1e-5);
        // `cameraPos[1] >= -9.0 ? -9.0 : -43.0`.
        let eye = |y: f32| Vec3::new(3.0, y, -20.0);
        assert_eq!(level_for_eye(&planes, eye(4.0)), 0);
        assert_eq!(level_for_eye(&planes, eye(-8.0)), 0);
        assert_eq!(level_for_eye(&planes, eye(-9.5)), 1);
        assert_eq!(level_for_eye(&planes, eye(-30.0)), 1);
        // Below both: the lowest still. One plane, or none: always the first.
        assert_eq!(level_for_eye(&planes, eye(-60.0)), 1);
        assert_eq!(level_for_eye(&planes[..1], eye(-60.0)), 0);
        assert_eq!(level_for_eye(&[], eye(0.0)), 0);
        // Named the other way round, the levels are still highest first.
        let swapped = [floors[1].clone(), floors[0].clone()];
        let plan = plan_mirror(&swapped, &at_rest(5)).unwrap();
        assert_eq!(level_surfaces(&plan), [[1], [0]]);
    }

    #[test]
    fn a_model_that_is_not_flat_is_no_mirror_but_a_few_stray_vertices_are_let_pass() {
        // A step: half the vertices half a unit down.
        let mut step = square(0, 0.0, 0.0, 0.0);
        step.points[0].y = -0.5;
        step.points[1].y = -0.5;
        assert_eq!(plan_mirror(std::slice::from_ref(&step), &at_rest(1)), None);
        // Beside a flat one it is dropped and the flat one is the mirror.
        let plan = plan_mirror(&[step.clone(), square(0, 9.0, 0.0, 0.0)], &at_rest(1)).unwrap();
        assert_eq!(level_surfaces(&plan), [[1]]);
        assert_eq!(plan.dropped, [0]);
        // Stage 48 in small: 4 vertices of 1088 half a unit down.
        let mut floor = square(0, 0.0, 0.015, 0.0);
        let corners = floor.points.clone();
        floor.points = corners.iter().cycle().take(1084).copied().collect();
        floor.normal = Vec3::Y * 1088.0;
        (floor.points).extend(corners.iter().map(|p| *p - Vec3::Y * 0.5));
        let plan = plan_mirror(std::slice::from_ref(&floor), &at_rest(1)).unwrap();
        assert_eq!(level_surfaces(&plan), [[0]]);
        let m = plan.levels[0].plane.at(|_| Mat4::IDENTITY);
        assert!((m.w_axis.y - 0.015).abs() < 1e-6);
        // Eight more and it is more than one in a hundred.
        (floor.points).extend(corners.iter().cycle().take(8).map(|p| *p - Vec3::Y * 0.5));
        assert_eq!(plan_mirror(&[floor], &at_rest(1)), None);
        // Within the tolerance is flat: models float 0.01 to 0.015 above their floors.
        let mut rough = square(0, 0.0, 0.0, 0.0);
        rough.points[0].y = 0.015;
        assert!(plan_mirror(&[rough.clone()], &at_rest(1)).is_some());
        rough.points[0].y = 0.05;
        assert!(plan_mirror(&[rough], &at_rest(1)).is_none());
        // Without a facing there is no plane.
        let mut blind = square(0, 0.0, 0.0, 0.0);
        blind.normal = Vec3::ZERO;
        assert!(plan_mirror(&[blind], &at_rest(1)).is_none());
    }

    #[test]
    fn models_facing_opposite_ways_do_not_share_a_plane() {
        let up = square(0, 0.0, 1.0, 0.0);
        let mut down = square(0, 5.0, 1.0, 0.0);
        down.normal = -down.normal;
        let plan = plan_mirror(&[up, down], &at_rest(1)).unwrap();
        assert_eq!(level_surfaces(&plan), [[0]]);
        assert_eq!(plan.dropped, [1]);
    }
}
