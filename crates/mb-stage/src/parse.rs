//! Big-endian stagedef reader. Mirrors `StageParser` in WebMonkeyBall `src/stage.ts`.

use crate::types::*;
use std::cell::Cell;
use std::fmt;
use std::mem::size_of;

pub const START_POS_SIZE: usize = 0x14;
pub const ANIM_GROUP_SIZE: usize = 0xc4;
pub const TRIANGLE_SIZE: usize = 0x40;
pub const GOAL_SIZE: usize = 0x14;
pub const BUMPER_SIZE: usize = 0x20;
pub const JAMABAR_SIZE: usize = 0x20;
pub const BANANA_SIZE: usize = 0x10;
pub const CONE_SIZE: usize = 0x20;
pub const SPHERE_SIZE: usize = 0x14;
pub const CYLINDER_SIZE: usize = 0x1c;
pub const ANIM_GROUP_MODEL_SIZE: usize = 0x0c;
pub const FALLOUT_BOX_SIZE: usize = 0x20;
pub const BG_OBJECT_SIZE: usize = 0x38;
pub const BG_ANIM_SIZE: usize = 0x60;
pub const FLIPBOOK_SIZE: usize = 0x10;
pub const NIGHT_WINDOW_SIZE: usize = 0x14;
pub const STORM_FIRE_SIZE: usize = 0x10;
pub const KEYFRAME_SIZE: usize = 0x14;
pub const MIRROR_ENTRY_SIZE: usize = 8;

/// Minimum size of the SMB1 stage header we read (through +0x94).
const HEADER_MIN: usize = 0x94;

/// Failure to decode a stagedef.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageError {
    /// A read at `offset` of `len` bytes ran past the end of the data.
    OutOfBounds { offset: usize, len: usize },
    /// A non-NULL pointer at `at` points outside the file.
    BadPointer { at: usize, value: u32 },
    /// SMB2-format stagedef (detected by the header magic); only SMB1 is implemented.
    Unsupported(&'static str),
    /// Structurally invalid data.
    Invalid(String),
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StageError::OutOfBounds { offset, len } => {
                write!(f, "read of {len} bytes at 0x{offset:x} is out of bounds")
            }
            StageError::BadPointer { at, value } => {
                write!(f, "pointer 0x{value:x} at 0x{at:x} is outside the file")
            }
            StageError::Unsupported(s) => write!(f, "unsupported stagedef: {s}"),
            StageError::Invalid(s) => write!(f, "invalid stagedef: {s}"),
        }
    }
}

impl std::error::Error for StageError {}

type Res<T> = Result<T, StageError>;

/// Decoded bytes a stagedef may produce per input byte, plus [`BUDGET_SLACK`]. Lists in a stagedef are found
/// through pointers, so a corrupt file can make thousands of records share one long list; without a cap the
/// decoded size grows with the square of the file size. The largest ratio on the retail disc is 3.4.
const BUDGET_PER_BYTE: usize = 24;
const BUDGET_SLACK: usize = 0x1_0000;

struct Reader<'a> {
    d: &'a [u8],
    /// Decoded bytes still allowed (see [`BUDGET_PER_BYTE`]).
    budget: Cell<usize>,
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8]) -> Self {
        Reader {
            d,
            budget: Cell::new(
                d.len()
                    .saturating_mul(BUDGET_PER_BYTE)
                    .saturating_add(BUDGET_SLACK),
            ),
        }
    }

    /// Account for `bytes` of decoded output before producing it.
    fn charge(&self, bytes: usize) -> Res<()> {
        match self.budget.get().checked_sub(bytes) {
            Some(left) => {
                self.budget.set(left);
                Ok(())
            }
            None => Err(StageError::Invalid(
                "lists overlap: the file describes more data than it can hold".into(),
            )),
        }
    }

    fn bytes(&self, off: usize, len: usize) -> Res<&'a [u8]> {
        off.checked_add(len)
            .and_then(|end| self.d.get(off..end))
            .ok_or(StageError::OutOfBounds { offset: off, len })
    }
    fn u8(&self, o: usize) -> Res<u8> {
        Ok(self.bytes(o, 1)?[0])
    }
    fn u16(&self, o: usize) -> Res<u16> {
        let b = self.bytes(o, 2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    fn i16(&self, o: usize) -> Res<i16> {
        Ok(self.u16(o)? as i16)
    }
    fn u32(&self, o: usize) -> Res<u32> {
        let b = self.bytes(o, 4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn i32(&self, o: usize) -> Res<i32> {
        Ok(self.u32(o)? as i32)
    }
    fn f32(&self, o: usize) -> Res<f32> {
        Ok(f32::from_bits(self.u32(o)?))
    }

    /// File pointer at `o`: `None` for 0; error if non-zero and outside the file (strict).
    fn ptr(&self, o: usize) -> Res<Option<usize>> {
        let v = self.u32(o)?;
        if v == 0 {
            Ok(None)
        } else if (v as usize) >= self.d.len() {
            Err(StageError::BadPointer { at: o, value: v })
        } else {
            Ok(Some(v as usize))
        }
    }
    /// Like the reference `readPtr`: NULL and out-of-range both give `None`. Used for raw/unknown fields and
    /// list terminators.
    fn ptr_lenient(&self, o: usize) -> Res<Option<usize>> {
        let v = self.u32(o)?;
        Ok(if v == 0 || (v as usize) >= self.d.len() {
            None
        } else {
            Some(v as usize)
        })
    }

    fn vec3(&self, o: usize) -> Res<Vec3> {
        Ok(Vec3 {
            x: self.f32(o)?,
            y: self.f32(o + 4)?,
            z: self.f32(o + 8)?,
        })
    }
    fn vec2(&self, o: usize) -> Res<Vec2> {
        Ok(Vec2 {
            x: self.f32(o)?,
            y: self.f32(o + 4)?,
        })
    }
    fn s16vec(&self, o: usize) -> Res<S16Vec> {
        Ok(S16Vec {
            x: self.i16(o)?,
            y: self.i16(o + 2)?,
            z: self.i16(o + 4)?,
        })
    }

    /// NUL-terminated string (lossy UTF-8; names are ASCII).
    fn string(&self, o: usize) -> Res<String> {
        let tail = self
            .d
            .get(o..)
            .ok_or(StageError::OutOfBounds { offset: o, len: 1 })?;
        let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
        self.charge(end)?;
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    /// Validate that `count` records of `size` bytes at `off` fit, returning the usable count. `T` is the
    /// decoded record, charged to the budget.
    fn list<T>(&self, off: Option<usize>, count: i32, size: usize) -> Res<(usize, usize)> {
        match off {
            Some(o) if count > 0 => {
                let n = count as usize;
                self.bytes(o, n.saturating_mul(size))?;
                self.charge(n.saturating_mul(size_of::<T>()))?;
                Ok((o, n))
            }
            _ => Ok((0, 0)),
        }
    }

    fn name_at_ptr(&self, o: usize) -> Res<String> {
        match self.ptr(o)? {
            Some(p) => self.string(p),
            None => Ok(String::new()),
        }
    }

    fn string_list(&self, off: Option<usize>) -> Res<Vec<String>> {
        let mut names = Vec::new();
        let Some(mut cur) = off else { return Ok(names) };
        while let Some(p) = self.ptr_lenient(cur)? {
            self.charge(size_of::<String>())?;
            names.push(self.string(p)?);
            cur += 4;
        }
        Ok(names)
    }

    fn mirror_names(&self, off: Option<usize>, count: i32) -> Res<Vec<String>> {
        let (base, n) = self.list::<String>(off, count, MIRROR_ENTRY_SIZE)?;
        let mut names = Vec::new();
        for i in 0..n {
            if let Some(p) = self.ptr(base + i * MIRROR_ENTRY_SIZE)? {
                names.push(self.string(p)?);
            }
        }
        Ok(names)
    }

    fn keyframes(&self, off: Option<usize>, count: u32) -> Res<Vec<Keyframe>> {
        let (base, n) =
            self.list::<Keyframe>(off, count.min(i32::MAX as u32) as i32, KEYFRAME_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * KEYFRAME_SIZE;
                Ok(Keyframe {
                    ease_type: self.i32(b)?,
                    time_seconds: self.f32(b + 4)?,
                    value: self.f32(b + 8)?,
                    tangent_in: self.f32(b + 0xc)?,
                    tangent_out: self.f32(b + 0x10)?,
                })
            })
            .collect()
    }

    /// Track = (count at `o`, pointer at `o + 4`).
    fn track(&self, o: usize) -> Res<Vec<Keyframe>> {
        self.keyframes(self.ptr(o + 4)?, self.u32(o)?)
    }

    fn anim_header(&self, off: Option<usize>) -> Res<Option<AnimHeader>> {
        let Some(o) = off else { return Ok(None) };
        Ok(Some(AnimHeader {
            rot_x: self.track(o)?,
            rot_y: self.track(o + 0x08)?,
            rot_z: self.track(o + 0x10)?,
            pos_x: self.track(o + 0x18)?,
            pos_y: self.track(o + 0x20)?,
            pos_z: self.track(o + 0x28)?,
        }))
    }

    fn bg_anim(&self, off: Option<usize>) -> Res<Option<BgAnim>> {
        let Some(o) = off else { return Ok(None) };
        self.bytes(o, BG_ANIM_SIZE)?;
        Ok(Some(BgAnim {
            loop_start_seconds: self.f32(o)?,
            loop_end_seconds: self.f32(o + 4)?,
            scale_x: self.track(o + 0x08)?,
            scale_y: self.track(o + 0x10)?,
            scale_z: self.track(o + 0x18)?,
            rot_x: self.track(o + 0x20)?,
            rot_y: self.track(o + 0x28)?,
            rot_z: self.track(o + 0x30)?,
            pos_x: self.track(o + 0x38)?,
            pos_y: self.track(o + 0x40)?,
            pos_z: self.track(o + 0x48)?,
            visible: self.track(o + 0x50)?,
            translucency: self.track(o + 0x58)?,
        }))
    }

    fn flipbooks(&self, off: Option<usize>) -> Res<Option<Flipbooks>> {
        let Some(o) = off else { return Ok(None) };
        self.bytes(o, FLIPBOOK_SIZE)?;
        let nw_count = self.i32(o)?;
        let (nw, n) =
            self.list::<NightWindowAnim>(self.ptr(o + 4)?, nw_count, NIGHT_WINDOW_SIZE)?;
        let night_window_anims = (0..n)
            .map(|i| {
                let b = nw + i * NIGHT_WINDOW_SIZE;
                Ok(NightWindowAnim {
                    pos: self.vec3(b)?,
                    rot: self.s16vec(b + 0x0c)?,
                    id: self.u8(b + 0x12)?,
                })
            })
            .collect::<Res<Vec<_>>>()?;
        let sf_count = self.i32(o + 8)?;
        let (sf, n) = self.list::<StormFireAnim>(self.ptr(o + 0xc)?, sf_count, STORM_FIRE_SIZE)?;
        let storm_fire_anims = (0..n)
            .map(|i| {
                let b = sf + i * STORM_FIRE_SIZE;
                Ok(StormFireAnim {
                    pos: self.vec3(b)?,
                    frame_offset: self.u8(b + 0x0c)?,
                })
            })
            .collect::<Res<Vec<_>>>()?;
        Ok(Some(Flipbooks {
            night_window_anims,
            storm_fire_anims,
        }))
    }

    fn bg_objects(&self, off: Option<usize>, count: i32) -> Res<Vec<BgObject>> {
        let (base, n) = self.list::<BgObject>(off, count, BG_OBJECT_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * BG_OBJECT_SIZE;
                Ok(BgObject {
                    flags: self.u32(b)?,
                    name: self.name_at_ptr(b + 4)?,
                    pos: self.vec3(b + 0x0c)?,
                    rot: self.s16vec(b + 0x18)?,
                    scale: self.vec3(b + 0x20)?,
                    translucency: self.f32(b + 0x2c)?,
                    anim: self.bg_anim(self.ptr(b + 0x30)?)?,
                    flipbooks: self.flipbooks(self.ptr(b + 0x34)?)?,
                })
            })
            .collect()
    }

    fn goals(&self, off: Option<usize>, count: i32) -> Res<Vec<Goal>> {
        let (base, n) = self.list::<Goal>(off, count, GOAL_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * GOAL_SIZE;
                let type_value = self.u16(b + 0x12)?;
                Ok(Goal {
                    pos: self.vec3(b)?,
                    rot: self.s16vec(b + 0x0c)?,
                    type_value,
                    kind: GoalType::from_value(type_value),
                })
            })
            .collect()
    }

    fn bumpers(&self, off: Option<usize>, count: i32) -> Res<Vec<Bumper>> {
        let (base, n) = self.list::<Bumper>(off, count, BUMPER_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * BUMPER_SIZE;
                Ok(Bumper {
                    pos: self.vec3(b)?,
                    rot: self.s16vec(b + 0x0c)?,
                    scale: self.vec3(b + 0x14)?,
                })
            })
            .collect()
    }

    fn jamabars(&self, off: Option<usize>, count: i32) -> Res<Vec<Jamabar>> {
        let (base, n) = self.list::<Jamabar>(off, count, JAMABAR_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * JAMABAR_SIZE;
                Ok(Jamabar {
                    pos: self.vec3(b)?,
                    rot: self.s16vec(b + 0x0c)?,
                    scale: self.vec3(b + 0x14)?,
                })
            })
            .collect()
    }

    fn bananas(&self, off: Option<usize>, count: i32) -> Res<Vec<Banana>> {
        let (base, n) = self.list::<Banana>(off, count, BANANA_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * BANANA_SIZE;
                Ok(Banana {
                    pos: self.vec3(b)?,
                    kind: self.i32(b + 0x0c)?,
                })
            })
            .collect()
    }

    fn cones(&self, off: Option<usize>, count: i32) -> Res<Vec<ColiCone>> {
        let (base, n) = self.list::<ColiCone>(off, count, CONE_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * CONE_SIZE;
                Ok(ColiCone {
                    pos: self.vec3(b)?,
                    rot: self.s16vec(b + 0x0c)?,
                    flags: self.u16(b + 0x12)?,
                    scale: self.vec3(b + 0x14)?,
                })
            })
            .collect()
    }

    fn spheres(&self, off: Option<usize>, count: i32) -> Res<Vec<ColiSphere>> {
        let (base, n) = self.list::<ColiSphere>(off, count, SPHERE_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * SPHERE_SIZE;
                Ok(ColiSphere {
                    pos: self.vec3(b)?,
                    radius: self.f32(b + 0x0c)?,
                    flags: self.u16(b + 0x10)?,
                })
            })
            .collect()
    }

    fn cylinders(&self, off: Option<usize>, count: i32) -> Res<Vec<ColiCylinder>> {
        let (base, n) = self.list::<ColiCylinder>(off, count, CYLINDER_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * CYLINDER_SIZE;
                Ok(ColiCylinder {
                    pos: self.vec3(b)?,
                    radius: self.f32(b + 0x0c)?,
                    height: self.f32(b + 0x10)?,
                    rot: self.s16vec(b + 0x14)?,
                    flags: self.u16(b + 0x1a)?,
                })
            })
            .collect()
    }

    fn models(&self, off: Option<usize>, count: i32) -> Res<Vec<AnimGroupModel>> {
        let (base, n) = self.list::<AnimGroupModel>(off, count, ANIM_GROUP_MODEL_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * ANIM_GROUP_MODEL_SIZE;
                Ok(AnimGroupModel {
                    flags: self.u32(b)?,
                    name: self.name_at_ptr(b + 4)?,
                    value: self.f32(b + 8)?,
                })
            })
            .collect()
    }

    fn fallout_boxes(&self, off: Option<usize>, count: i32) -> Res<Vec<FalloutBox>> {
        let (base, n) = self.list::<FalloutBox>(off, count, FALLOUT_BOX_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * FALLOUT_BOX_SIZE;
                Ok(FalloutBox {
                    pos: self.vec3(b)?,
                    scale: self.vec3(b + 0x0c)?,
                    rot: self.s16vec(b + 0x18)?,
                })
            })
            .collect()
    }

    fn triangles(&self, off: Option<usize>, count: usize) -> Res<Vec<ColiTri>> {
        let (base, n) =
            self.list::<ColiTri>(off, count.min(i32::MAX as usize) as i32, TRIANGLE_SIZE)?;
        (0..n)
            .map(|i| {
                let b = base + i * TRIANGLE_SIZE;
                Ok(ColiTri {
                    pos: self.vec3(b)?,
                    normal: self.vec3(b + 0x0c)?,
                    rot: self.s16vec(b + 0x18)?,
                    flags: self.u16(b + 0x1e)?,
                    vert2: self.vec2(b + 0x20)?,
                    vert3: self.vec2(b + 0x28)?,
                    edge2_normal: self.vec2(b + 0x30)?,
                    edge3_normal: self.vec2(b + 0x38)?,
                })
            })
            .collect()
    }

    /// Returns the grid and the highest triangle index referenced (-1 if none).
    fn grid(&self, o: usize, cells_ptr: Option<usize>) -> Res<(CollisionGrid, i64)> {
        let mut grid = CollisionGrid {
            origin_x: self.f32(o + 0x24)?,
            origin_z: self.f32(o + 0x28)?,
            step_x: self.f32(o + 0x2c)?,
            step_z: self.f32(o + 0x30)?,
            cell_count_x: self.i32(o + 0x34)?,
            cell_count_z: self.i32(o + 0x38)?,
            cells: Vec::new(),
        };
        let mut max_index = -1i64;
        let Some(table) = cells_ptr else {
            return Ok((grid, max_index));
        };
        if grid.cell_count_x <= 0 || grid.cell_count_z <= 0 {
            return Ok((grid, max_index));
        }
        let n = (grid.cell_count_x as usize)
            .checked_mul(grid.cell_count_z as usize)
            .ok_or_else(|| StageError::Invalid("grid cell count overflow".into()))?;
        self.bytes(table, n.saturating_mul(4))?;
        self.charge(n.saturating_mul(size_of::<Vec<u16>>()))?;
        let mut cells = Vec::with_capacity(n);
        for i in 0..n {
            let mut list = Vec::new();
            if let Some(mut cur) = self.ptr(table + i * 4)? {
                loop {
                    let idx = self.i16(cur)?;
                    cur += 2;
                    if idx < 0 {
                        break;
                    }
                    self.charge(size_of::<u16>())?;
                    max_index = max_index.max(idx as i64);
                    list.push(idx as u16);
                }
            }
            cells.push(list);
        }
        grid.cells = cells;
        Ok((grid, max_index))
    }

    fn anim_group(&self, o: usize) -> Res<AnimGroup> {
        self.bytes(o, ANIM_GROUP_SIZE)?;
        let (grid, max_index) = self.grid(o, self.ptr(o + 0x20)?)?;
        let want = (max_index + 1) as usize;
        let triangles = self.triangles(self.ptr(o + 0x1c)?, want)?;
        if triangles.len() != want {
            return Err(StageError::Invalid(format!(
                "anim group at 0x{o:x}: grid references {want} triangles but the triangle list is missing"
            )));
        }
        Ok(AnimGroup {
            init_pos: self.vec3(o)?,
            init_rot: self.s16vec(o + 0x0c)?,
            anim_loop_type: self.u16(o + 0x12)?,
            anim: self.anim_header(self.ptr(o + 0x14)?)?,
            model_names: self.string_list(self.ptr_lenient(o + 0x18)?)?,
            triangles,
            grid,
            goals: self.goals(self.ptr(o + 0x40)?, self.i32(o + 0x3c)?)?,
            unk48: self.ptr_lenient(o + 0x48)?.map(|p| p as u32),
            bumpers: self.bumpers(self.ptr(o + 0x50)?, self.i32(o + 0x4c)?)?,
            jamabars: self.jamabars(self.ptr(o + 0x58)?, self.i32(o + 0x54)?)?,
            bananas: self.bananas(self.ptr(o + 0x60)?, self.i32(o + 0x5c)?)?,
            cones: self.cones(self.ptr(o + 0x68)?, self.i32(o + 0x64)?)?,
            spheres: self.spheres(self.ptr(o + 0x70)?, self.i32(o + 0x6c)?)?,
            cylinders: self.cylinders(self.ptr(o + 0x78)?, self.i32(o + 0x74)?)?,
            models: self.models(self.ptr(o + 0x80)?, self.i32(o + 0x7c)?)?,
            fallout_boxes: self.fallout_boxes(self.ptr(o + 0x88)?, self.i32(o + 0x84)?)?,
            mirror_model_names: self.mirror_names(self.ptr(o + 0x90)?, self.i32(o + 0x8c)?)?,
            unk90: self.ptr_lenient(o + 0x90)?.map(|p| p as u32),
            unk_b8: self.vec3(o + 0xb8)?,
        })
    }

    fn start_positions(&self, off: Option<usize>, fallout: Option<usize>) -> Res<Vec<StartPos>> {
        let Some(o) = off else { return Ok(Vec::new()) };
        let mut count = 1;
        if let Some(f) = fallout {
            if f > o && f - o >= START_POS_SIZE {
                count = ((f - o) / START_POS_SIZE).max(1);
            }
        }
        self.charge(count.saturating_mul(size_of::<StartPos>()))?;
        (0..count)
            .map(|i| {
                let b = o + i * START_POS_SIZE;
                Ok(StartPos {
                    pos: self.vec3(b)?,
                    rot: self.s16vec(b + 0x0c)?,
                })
            })
            .collect()
    }
}

/// SMB2 stagedefs begin with `0x00000000` followed by `1000.0f` or `100.0f`.
fn looks_like_smb2(d: &[u8]) -> bool {
    if d.len() < 8 {
        return false;
    }
    let a = u32::from_be_bytes([d[0], d[1], d[2], d[3]]);
    let b = u32::from_be_bytes([d[4], d[5], d[6], d[7]]);
    a == 0 && (b == 0x447a_0000 || b == 0x42c8_0000)
}

/// Parse a decompressed SMB1 `STAGENNN.lz` payload.
///
/// SMB2 stagedefs are detected by their header magic (as in the reference `parseStageDef`) and rejected with
/// [`StageError::Unsupported`].
pub fn parse_stagedef_smb1(bytes: &[u8]) -> Result<Stage, StageError> {
    if looks_like_smb2(bytes) {
        return Err(StageError::Unsupported(
            "SMB2 stagedef format is not implemented",
        ));
    }
    if bytes.len() < HEADER_MIN {
        return Err(StageError::OutOfBounds {
            offset: 0,
            len: HEADER_MIN,
        });
    }
    let r = Reader::new(bytes);

    let group_count = r.i32(0x08)?;
    let groups_ptr = r.ptr(0x0c)?;
    let start_ptr = r.ptr(0x10)?;
    let fallout_ptr = r.ptr(0x14)?;

    let mut anim_groups = Vec::new();
    if let Some(base) = groups_ptr {
        for i in 0..group_count.max(0) as usize {
            r.charge(size_of::<AnimGroup>())?;
            anim_groups.push(r.anim_group(base + i * ANIM_GROUP_SIZE)?);
        }
    }

    Ok(Stage {
        loop_start_seconds: r.i32(0x00)?,
        loop_end_seconds: r.i32(0x04)?,
        anim_groups,
        start_positions: r.start_positions(start_ptr, fallout_ptr)?,
        fallout_y: match fallout_ptr {
            Some(p) => r.f32(p)?,
            None => 0.0,
        },
        goals: r.goals(r.ptr(0x1c)?, r.i32(0x18)?)?,
        bumpers: r.bumpers(r.ptr(0x2c)?, r.i32(0x28)?)?,
        jamabars: r.jamabars(r.ptr(0x34)?, r.i32(0x30)?)?,
        bananas: r.bananas(r.ptr(0x3c)?, r.i32(0x38)?)?,
        cones: r.cones(r.ptr(0x44)?, r.i32(0x40)?)?,
        spheres: r.spheres(r.ptr(0x4c)?, r.i32(0x48)?)?,
        cylinders: r.cylinders(r.ptr(0x54)?, r.i32(0x50)?)?,
        models: r.models(r.ptr(0x5c)?, r.i32(0x58)?)?,
        bg_objects: r.bg_objects(r.ptr(0x6c)?, r.i32(0x68)?)?,
        fg_objects: r.bg_objects(r.ptr(0x74)?, r.i32(0x70)?)?,
        unk78: r.ptr_lenient(0x78)?.map(|p| p as u32),
        unk7c: r.i32(0x7c)?,
        mirror_names: r.mirror_names(r.ptr(0x84)?, r.i32(0x80)?)?,
        unk88: r.ptr_lenient(0x88)?.map(|p| p as u32),
        unk90: r.ptr_lenient(0x90)?.map(|p| p as u32),
    })
}
