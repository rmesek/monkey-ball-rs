//! Plain data structs for a decoded SMB1 stagedef.
//!
//! Field names follow `WebMonkeyBall/src/stage.ts` (`StageParser`) and the smb-decomp `Stage*` structs so the
//! physics port can map 1:1. All values are decoded from big-endian; angles are raw signed 16-bit units
//! (65536 = full turn) exactly as stored. Lists that the file gives as `(count, pointer)` are `Vec`s whose
//! length equals the file's count (an absent/NULL pointer yields an empty `Vec`).

/// Three `f32`s.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Two `f32`s.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// Three `i16` rotation components (S16 angle units, 0x10000 = 360 degrees).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct S16Vec {
    pub x: i16,
    pub y: i16,
    pub z: i16,
}

/// One animation keyframe (0x14 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keyframe {
    /// 0 = constant, 1 = linear, anything else = smoothstep/hermite (the game treats other values alike).
    pub ease_type: i32,
    pub time_seconds: f32,
    pub value: f32,
    pub tangent_in: f32,
    pub tangent_out: f32,
}

/// Animation header of an anim group (0x30 bytes): six keyframe tracks (rotation then position).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimHeader {
    pub rot_x: Vec<Keyframe>,
    pub rot_y: Vec<Keyframe>,
    pub rot_z: Vec<Keyframe>,
    pub pos_x: Vec<Keyframe>,
    pub pos_y: Vec<Keyframe>,
    pub pos_z: Vec<Keyframe>,
}

/// Background object animation (0x60 bytes): loop range plus eleven tracks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BgAnim {
    pub loop_start_seconds: f32,
    pub loop_end_seconds: f32,
    pub scale_x: Vec<Keyframe>,
    pub scale_y: Vec<Keyframe>,
    pub scale_z: Vec<Keyframe>,
    pub rot_x: Vec<Keyframe>,
    pub rot_y: Vec<Keyframe>,
    pub rot_z: Vec<Keyframe>,
    pub pos_x: Vec<Keyframe>,
    pub pos_y: Vec<Keyframe>,
    pub pos_z: Vec<Keyframe>,
    /// Model visible when value >= 0.5 (per the noclip notes).
    pub visible: Vec<Keyframe>,
    pub translucency: Vec<Keyframe>,
}

/// Window flipbook placement (0x14 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NightWindowAnim {
    pub pos: Vec3,
    pub rot: S16Vec,
    /// Which flipbook model list to animate.
    pub id: u8,
}

/// Storm-fire flipbook placement (0x10 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StormFireAnim {
    pub pos: Vec3,
    pub frame_offset: u8,
}

/// Flipbook animation block of a bg object (0x10 bytes).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Flipbooks {
    pub night_window_anims: Vec<NightWindowAnim>,
    pub storm_fire_anims: Vec<StormFireAnim>,
}

/// Background / foreground model placement (0x38 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct BgObject {
    /// Bit 0 = visible (other bits also affect visibility in some stages).
    pub flags: u32,
    /// Model name in the stage GMA; empty if the pointer is NULL.
    pub name: String,
    pub pos: Vec3,
    pub rot: S16Vec,
    pub scale: Vec3,
    pub translucency: f32,
    pub anim: Option<BgAnim>,
    pub flipbooks: Option<Flipbooks>,
}

/// Ball start position (0x14 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StartPos {
    pub pos: Vec3,
    pub rot: S16Vec,
}

/// Goal tape (0x14 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Goal {
    pub pos: Vec3,
    pub rot: S16Vec,
    /// Raw u16 at +0x12 (ASCII 'B'/'G'/'R' in one byte, or small numeric codes).
    pub type_value: u16,
    pub kind: GoalType,
}

/// Goal colour (difficulty), derived exactly like `goalTypeFromValue` in the reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalType {
    Blue,
    Green,
    Red,
}

impl GoalType {
    /// Port of `goalTypeFromValue` (constants.ts).
    pub fn from_value(value: u16) -> GoalType {
        let high = (value >> 8) as u8;
        let low = (value & 0xff) as u8;
        if high == 0x42 || low == 0x42 {
            return GoalType::Blue;
        }
        if high == 0x47 || low == 0x47 {
            return GoalType::Green;
        }
        if high == 0x52 || low == 0x52 {
            return GoalType::Red;
        }
        if low == 0x01 {
            return match high {
                0x01 => GoalType::Green,
                0x02 => GoalType::Red,
                _ => GoalType::Blue,
            };
        }
        GoalType::Blue
    }
}

/// Bumper (0x20 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bumper {
    pub pos: Vec3,
    pub rot: S16Vec,
    pub scale: Vec3,
}

/// Jamabar (moving bar) (0x20 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Jamabar {
    pub pos: Vec3,
    pub rot: S16Vec,
    pub scale: Vec3,
}

/// Banana (0x10 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Banana {
    pub pos: Vec3,
    /// 0 = single, 1 = bunch.
    pub kind: i32,
}

/// Collision cone (0x20 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColiCone {
    pub pos: Vec3,
    pub rot: S16Vec,
    pub flags: u16,
    pub scale: Vec3,
}

/// Collision sphere (0x14 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColiSphere {
    pub pos: Vec3,
    pub radius: f32,
    pub flags: u16,
}

/// Collision cylinder (0x1c bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColiCylinder {
    pub pos: Vec3,
    pub radius: f32,
    pub height: f32,
    pub rot: S16Vec,
    pub flags: u16,
}

/// Visual model attached to an anim group (0x0c bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimGroupModel {
    pub flags: u32,
    pub name: String,
    /// f32 at +0x08 (unknown meaning).
    pub value: f32,
}

/// Fallout volume (0x20 bytes): ball falling into it counts as out of bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FalloutBox {
    pub pos: Vec3,
    pub scale: Vec3,
    pub rot: S16Vec,
}

/// Collision triangle (0x40 bytes), stored in "triangle space": `pos` is vertex 1, the triangle lies in the
/// XY plane after rotating by `rot`, vertex 2 on +X.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColiTri {
    /// Vertex 1 in anim-group space.
    pub pos: Vec3,
    /// Face normal in anim-group space.
    pub normal: Vec3,
    /// Rotation from the XY plane to the triangle.
    pub rot: S16Vec,
    /// Raw u16 at +0x1e (collision flags).
    pub flags: u16,
    /// Vertex 2 relative to vertex 1, in triangle space.
    pub vert2: Vec2,
    /// Vertex 3 relative to vertex 1, in triangle space.
    pub vert3: Vec2,
    /// Normal of edge 2->3 in triangle space.
    pub edge2_normal: Vec2,
    /// Normal of edge 3->1 in triangle space.
    pub edge3_normal: Vec2,
}

/// Spatial grid of one anim group. Cell `(x, z)` is `cells[z * cell_count_x + x]`; each cell lists indices
/// into [`AnimGroup::triangles`]. NULL cells in the file become empty lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollisionGrid {
    pub origin_x: f32,
    pub origin_z: f32,
    pub step_x: f32,
    pub step_z: f32,
    pub cell_count_x: i32,
    pub cell_count_z: i32,
    pub cells: Vec<Vec<u16>>,
}

impl CollisionGrid {
    /// Triangle indices for an anim-group-local x/z, or `None` outside the grid (mirrors collision.ts).
    pub fn cell_at(&self, x: f32, z: f32) -> Option<&[u16]> {
        if self.cells.is_empty() || self.step_x == 0.0 || self.step_z == 0.0 {
            return None;
        }
        let cx = ((x - self.origin_x) / self.step_x).floor();
        let cz = ((z - self.origin_z) / self.step_z).floor();
        if cx < 0.0 || cx >= self.cell_count_x as f32 || cz < 0.0 || cz >= self.cell_count_z as f32
        {
            return None;
        }
        self.cells
            .get(cz as usize * self.cell_count_x as usize + cx as usize)
            .map(|v| v.as_slice())
    }
}

/// One animation group (0xc4 bytes). Group 0 is the static world; others move per their [`AnimHeader`].
#[derive(Debug, Clone, PartialEq)]
pub struct AnimGroup {
    pub init_pos: Vec3,
    pub init_rot: S16Vec,
    /// Raw u16 at +0x12: 0 = loop, 1 = play once, 2 = seesaw (reference `ANIM_*`).
    pub anim_loop_type: u16,
    pub anim: Option<AnimHeader>,
    /// Name list read at +0x18 by the reference (`modelNames`).
    pub model_names: Vec<String>,
    /// Triangle count = highest index referenced by the grid + 1 (the file stores no count).
    pub triangles: Vec<ColiTri>,
    pub grid: CollisionGrid,
    pub goals: Vec<Goal>,
    /// Raw pointer at +0x48 (unknown).
    pub unk48: Option<u32>,
    pub bumpers: Vec<Bumper>,
    pub jamabars: Vec<Jamabar>,
    pub bananas: Vec<Banana>,
    pub cones: Vec<ColiCone>,
    pub spheres: Vec<ColiSphere>,
    pub cylinders: Vec<ColiCylinder>,
    pub models: Vec<AnimGroupModel>,
    pub fallout_boxes: Vec<FalloutBox>,
    /// Mirror model names (count at +0x8c, list at +0x90, 8-byte entries, name pointer first).
    pub mirror_model_names: Vec<String>,
    /// Raw pointer at +0x90.
    pub unk90: Option<u32>,
    /// Vec3 at +0xb8 (unknown).
    pub unk_b8: Vec3,
}

/// A decoded SMB1 stagedef.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    /// Stage-wide animation loop range (header +0x00 / +0x04, stored as i32 seconds).
    pub loop_start_seconds: i32,
    pub loop_end_seconds: i32,
    pub anim_groups: Vec<AnimGroup>,
    /// Ball spawn points. Count is inferred as `(fallout_offset - start_offset) / 0x14` like the reference.
    pub start_positions: Vec<StartPos>,
    /// Fallout plane height (f32 at the fallout pointer, header +0x14).
    pub fallout_y: f32,
    pub goals: Vec<Goal>,
    pub bumpers: Vec<Bumper>,
    pub jamabars: Vec<Jamabar>,
    pub bananas: Vec<Banana>,
    pub cones: Vec<ColiCone>,
    pub spheres: Vec<ColiSphere>,
    pub cylinders: Vec<ColiCylinder>,
    pub models: Vec<AnimGroupModel>,
    pub bg_objects: Vec<BgObject>,
    /// Like bg objects but tilt with the stage.
    pub fg_objects: Vec<BgObject>,
    /// Raw pointer at +0x78 (unknown).
    pub unk78: Option<u32>,
    pub unk7c: i32,
    /// Mirror model names (count at +0x80, list at +0x84, 8-byte entries).
    pub mirror_names: Vec<String>,
    pub unk88: Option<u32>,
    pub unk90: Option<u32>,
}
