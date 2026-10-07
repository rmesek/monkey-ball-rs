//! Runtime state of the stage objects (the mutable half of `StageRuntime` in `stage.ts`). Plain data; the
//! update logic lives in [`crate::stage_runtime`] and the collision logic in [`crate::collision`].

use crate::math::{Mtx, Vec3};
use mb_stage::GoalType;

/// Animated transform of one anim group (group 0 is the static world).
#[derive(Debug, Clone, PartialEq)]
pub struct AnimGroupState {
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub rot: [i32; 3],
    pub prev_rot: [i32; 3],
    /// Group-local -> world.
    pub transform: Mtx,
    pub prev_transform: Mtx,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BumperState {
    pub anim_group_id: usize,
    pub base_pos: Vec3,
    /// Collision centre (base + rotated bound-centre offset), group-local.
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub rot: [i32; 3],
    pub scale: Vec3,
    pub radius: f64,
    pub model_radius: f64,
    /// 0 idle, 1 just hit, 2 bouncing.
    pub state: i32,
    pub spin: i32,
    pub spin_vel: i32,
    pub pulse_x: f64,
    pub pulse_z: f64,
    pub counter: i32,
    pub transform: Mtx,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JamabarState {
    pub anim_group_id: usize,
    pub base_pos: Vec3,
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub rot: [i32; 3],
    pub scale: Vec3,
    pub local_pos: Vec3,
    pub local_vel: Vec3,
    pub radius: f64,
    pub transform: Mtx,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoalState {
    pub anim_group_id: usize,
    pub pos: Vec3,
    pub rot: [i32; 3],
    pub kind: GoalType,
    pub transform: Mtx,
}

/// The "scent bag" that pops open when the goal tape breaks.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalBagState {
    pub anim_group_id: usize,
    pub state: i32,
    pub counter: i32,
    pub flags: i32,
    pub openness: f64,
    pub prev_openness: f64,
    pub unk8: f64,
    pub open_frame: i32,
    pub goal_pos: Vec3,
    pub goal_rot: [i32; 3],
    pub rot_x: i32,
    pub rot_y: i32,
    pub rot_z: i32,
    pub prev_rot_x: i32,
    pub prev_rot_y: i32,
    pub prev_rot_z: i32,
    pub u_some_pos: Vec3,
    pub local_pos: Vec3,
    pub local_vel: Vec3,
    pub model_origin: Vec3,
    pub bound_sphere_radius: f64,
    pub position: Vec3,
    pub prev_pos: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TapePoint {
    pub pos: Vec3,
    pub prev_pos: Vec3,
    pub normal: Vec3,
    pub prev_normal: Vec3,
    pub t: f64,
    pub vel: Vec3,
    /// bit0 fixed anchor, bit1 linked to previous point, bit2 linked to next point.
    pub flags: i32,
    pub len: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GoalTapeState {
    pub anim_group_id: usize,
    pub flags: i32,
    pub break_frame: i32,
    pub ground_y: f64,
    pub anchor_y: f64,
    pub target_y: f64,
    pub goal_pos: Vec3,
    pub goal_rot: [i32; 3],
    pub points: Vec<TapePoint>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BananaState {
    pub anim_group_id: usize,
    pub local_pos: Vec3,
    pub prev_local_pos: Vec3,
    /// World position (refreshed every tick).
    pub pos: Vec3,
    pub kind: i32,
    pub flags: i32,
    pub cooldown: i32,
    pub collected: bool,
    pub state: i32,
    pub collect_timer: i32,
    pub hold_timer: i32,
    pub hold_offset: Vec3,
    pub hold_scale_target: f64,
    pub hold_rot_vel: i32,
    pub fly_timer: i32,
    pub fly_scale_target: f64,
    pub fly_start_pos: Vec3,
    pub fly_start_scale: f64,
    pub tilt_timer: i32,
    pub base_scale: f64,
    pub scale: f64,
    pub prev_scale: f64,
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
}
