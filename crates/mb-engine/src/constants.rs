//! Port of the flag/state parts of `src/constants.ts` (SMB1 only).

pub mod ball_flags {
    pub const FLAG_00: u32 = 1 << 0;
    pub const INVISIBLE: u32 = 1 << 4;
    pub const FLAG_05: u32 = 1 << 5;
    pub const FLAG_06: u32 = 1 << 6;
    pub const FLAG_08: u32 = 1 << 8;
    pub const FLAG_09: u32 = 1 << 9;
    pub const FLAG_10: u32 = 1 << 10;
    pub const GOAL: u32 = 1 << 12;
    pub const FLAG_13: u32 = 1 << 13;
    pub const FLAG_14: u32 = 1 << 14;
    pub const TIMEOVER: u32 = 1 << 15;
}

/// `BALL_STATES`
pub mod ball_states {
    pub const READY: i32 = 0;
    pub const DROP: i32 = 3;
    pub const PLAY: i32 = 4;
    pub const GOAL_INIT: i32 = 5;
    pub const GOAL_MAIN: i32 = 6;
}

/// `CAMERA_STATE`
pub mod camera_state {
    pub const LEVEL_MAIN: i32 = 1;
    pub const FALLOUT_MAIN: i32 = 4;
    pub const READY_MAIN: i32 = 11;
    pub const GOAL_MAIN: i32 = 15;
}

pub const COLI_OCCURRED: u32 = 1;

/// Derived from the `mb_bumper` bound sphere in `common.gma` (copied from the reference).
pub const BUMPER_MODEL_RADIUS: f64 = 0.4243819714;
pub const BUMPER_BOUND_RADIUS: f64 = 0.75 * BUMPER_MODEL_RADIUS;
pub const BUMPER_BOUND_CENTER: (f64, f64, f64) = (0.0, 0.35, 0.0);
pub const GOAL_BAG_BASE_CENTER: (f64, f64, f64) = (0.0, -0.7499997616, 0.0);
pub const GOAL_BAG_BASE_RADIUS: f64 = 0.7500002384;
pub const GOAL_BAG_OPEN_SCALE: f64 = 0.5;
pub const GOAL_BAG_OFFSET: (f64, f64, f64) = (0.0, 2.8, 0.0);
pub const GOAL_BAG_GUIDE_POS: (f64, f64, f64) = (0.0, 3.5499999523, 0.0);
pub const GOAL_BAG_GUIDE_DIR: (f64, f64, f64) = (1.0, 0.0, 0.0);

pub const DEFAULT_STAGE_TIME: i32 = 60 * 60;
pub const GOAL_FLOAT_FRAMES: i32 = 90;
pub const GOAL_SEQUENCE_FRAMES: i32 = 330;
pub const RINGOUT_TOTAL_FRAMES: i32 = 270;
pub const TIMEOVER_TOTAL_FRAMES: i32 = 120;
