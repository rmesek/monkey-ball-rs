//! Collision-side view of a parsed [`mb_stage::Stage`]: the file's `f32` data promoted to `f64` once (the
//! reference reads them through `DataView.getFloat32`, i.e. as f32-valued doubles), with indices and angles
//! pre-converted. Immutable during simulation.

use crate::math::Vec3;
use mb_stage::{AnimHeader, GoalType, Stage};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tri {
    pub pos: Vec3,
    pub normal: Vec3,
    pub rot: [i32; 3],
    pub vert2: [f64; 2],
    pub vert3: [f64; 2],
    pub edge2_normal: [f64; 2],
    pub edge3_normal: [f64; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cone {
    pub pos: Vec3,
    pub rot: [i32; 3],
    pub scale: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sphere {
    pub pos: Vec3,
    pub radius: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cylinder {
    pub pos: Vec3,
    pub radius: f64,
    pub height: f64,
    pub rot: [i32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GoalGeom {
    pub pos: Vec3,
    pub rot: [i32; 3],
    pub kind: GoalType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedObject {
    pub pos: Vec3,
    pub rot: [i32; 3],
    pub scale: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BananaGeom {
    pub pos: Vec3,
    pub kind: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FalloutBoxGeom {
    pub pos: Vec3,
    pub scale: Vec3,
    pub rot: [i32; 3],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Grid {
    pub origin_x: f64,
    pub origin_z: f64,
    pub step_x: f64,
    pub step_z: f64,
    pub count_x: i32,
    pub count_z: i32,
    pub cells: Vec<Vec<u16>>,
}

impl Grid {
    /// `coligridLookup`: triangle indices for a group-local x/z, `None` outside the grid.
    pub fn lookup(&self, x: f64, z: f64) -> Option<&[u16]> {
        if self.cells.is_empty() {
            return None;
        }
        let cell_x = ((x - self.origin_x) / self.step_x).floor();
        let cell_z = ((z - self.origin_z) / self.step_z).floor();
        if cell_x.is_nan() || cell_z.is_nan() {
            return None;
        }
        if cell_x < 0.0 || cell_x >= self.count_x as f64 {
            return None;
        }
        if cell_z < 0.0 || cell_z >= self.count_z as f64 {
            return None;
        }
        self.cells
            .get(cell_z as usize * self.count_x as usize + cell_x as usize)
            .map(|v| v.as_slice())
    }
}

#[derive(Debug, Clone)]
pub struct GroupGeom {
    pub init_pos: Vec3,
    pub init_rot: [i32; 3],
    /// `unkB8`: the group's per-frame velocity used for `prevPos` bootstrapping.
    pub unk_b8: Vec3,
    pub anim: Option<AnimHeader>,
    pub triangles: Vec<Tri>,
    pub grid: Grid,
    pub cones: Vec<Cone>,
    pub spheres: Vec<Sphere>,
    pub cylinders: Vec<Cylinder>,
    pub goals: Vec<GoalGeom>,
    pub bumpers: Vec<PlacedObject>,
    pub jamabars: Vec<PlacedObject>,
    pub bananas: Vec<BananaGeom>,
    pub fallout_boxes: Vec<FalloutBoxGeom>,
}

#[derive(Debug, Clone)]
pub struct StageGeom {
    pub stage_id: u32,
    pub loop_start_seconds: f64,
    pub loop_end_seconds: f64,
    pub groups: Vec<GroupGeom>,
    pub start_positions: Vec<(Vec3, [i32; 3])>,
    pub fallout_y: f64,
}

fn rot3(r: mb_stage::S16Vec) -> [i32; 3] {
    [r.x as i32, r.y as i32, r.z as i32]
}

impl StageGeom {
    pub fn from_stage(stage: &Stage, stage_id: u32) -> StageGeom {
        let v = Vec3::from_f32;
        let v2 = |a: mb_stage::Vec2| [a.x as f64, a.y as f64];
        let groups = stage
            .anim_groups
            .iter()
            .map(|g| GroupGeom {
                init_pos: v(g.init_pos),
                init_rot: rot3(g.init_rot),
                unk_b8: v(g.unk_b8),
                anim: g.anim.clone(),
                triangles: g
                    .triangles
                    .iter()
                    .map(|t| Tri {
                        pos: v(t.pos),
                        normal: v(t.normal),
                        rot: rot3(t.rot),
                        vert2: v2(t.vert2),
                        vert3: v2(t.vert3),
                        edge2_normal: v2(t.edge2_normal),
                        edge3_normal: v2(t.edge3_normal),
                    })
                    .collect(),
                grid: Grid {
                    origin_x: g.grid.origin_x as f64,
                    origin_z: g.grid.origin_z as f64,
                    step_x: g.grid.step_x as f64,
                    step_z: g.grid.step_z as f64,
                    count_x: g.grid.cell_count_x,
                    count_z: g.grid.cell_count_z,
                    cells: g.grid.cells.clone(),
                },
                cones: g
                    .cones
                    .iter()
                    .map(|c| Cone {
                        pos: v(c.pos),
                        rot: rot3(c.rot),
                        scale: v(c.scale),
                    })
                    .collect(),
                spheres: g
                    .spheres
                    .iter()
                    .map(|s| Sphere {
                        pos: v(s.pos),
                        radius: s.radius as f64,
                    })
                    .collect(),
                cylinders: g
                    .cylinders
                    .iter()
                    .map(|c| Cylinder {
                        pos: v(c.pos),
                        radius: c.radius as f64,
                        height: c.height as f64,
                        rot: rot3(c.rot),
                    })
                    .collect(),
                goals: g
                    .goals
                    .iter()
                    .map(|c| GoalGeom {
                        pos: v(c.pos),
                        rot: rot3(c.rot),
                        kind: c.kind,
                    })
                    .collect(),
                bumpers: g
                    .bumpers
                    .iter()
                    .map(|b| PlacedObject {
                        pos: v(b.pos),
                        rot: rot3(b.rot),
                        scale: v(b.scale),
                    })
                    .collect(),
                jamabars: g
                    .jamabars
                    .iter()
                    .map(|b| PlacedObject {
                        pos: v(b.pos),
                        rot: rot3(b.rot),
                        scale: v(b.scale),
                    })
                    .collect(),
                bananas: g
                    .bananas
                    .iter()
                    .map(|b| BananaGeom {
                        pos: v(b.pos),
                        kind: b.kind,
                    })
                    .collect(),
                fallout_boxes: g
                    .fallout_boxes
                    .iter()
                    .map(|b| FalloutBoxGeom {
                        pos: v(b.pos),
                        scale: v(b.scale),
                        rot: rot3(b.rot),
                    })
                    .collect(),
            })
            .collect();
        StageGeom {
            stage_id,
            loop_start_seconds: stage.loop_start_seconds as f64,
            loop_end_seconds: stage.loop_end_seconds as f64,
            groups,
            start_positions: stage
                .start_positions
                .iter()
                .map(|s| (v(s.pos), rot3(s.rot)))
                .collect(),
            fallout_y: stage.fallout_y as f64,
        }
    }
}
