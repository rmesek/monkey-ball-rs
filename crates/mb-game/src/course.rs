//! Course scripts and their interpreter: a direct port of WebMonkeyBall `course.ts` (SMB1 only).
//!
//! A script is a flat command list: `Floor` starts a stage entry, an optional `Time` sets its limit, and
//! `IfFloorClear` / `IfGoalType` / `ThenJumpFloor` decide which entry follows (goal colours skip floors).
//! `stageTimeOverrides` only exist for SMB2 / pack courses and are not used here.

use crate::scripts;
use crate::stages::{stage_id_from_name, stage_label_from_name, DEFAULT_STAGE_TIME};

/// `INFO_FLAGS` bits that the scripts look at.
pub const INFO_GOAL: u32 = 1 << 0;
pub const INFO_TIMEOVER: u32 = 1 << 1;
pub const INFO_FALLOUT: u32 = 1 << 2;
pub const INFO_BONUS_CLEAR: u32 = 1 << 9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Floor(&'static str),
    /// Time limit (frames) for the preceding `Floor`.
    Time(u32),
    IfFloorClear,
    /// `b'B'`, `b'G'` or `b'R'`.
    IfGoalType(u8),
    IfTimeElapsed(u32),
    ThenJumpFloor(u32),
    /// Any other `THEN` (the reference ignores them).
    ThenOther,
    CourseEnd,
}

impl Op {
    fn is_if(self) -> bool {
        matches!(
            self,
            Op::IfFloorClear | Op::IfGoalType(_) | Op::IfTimeElapsed(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Difficulty {
    Beginner,
    Advanced,
    Expert,
    BeginnerExtra,
    AdvancedExtra,
    ExpertExtra,
    Master,
}

impl Difficulty {
    pub const ALL: [Difficulty; 7] = [
        Difficulty::Beginner,
        Difficulty::Advanced,
        Difficulty::Expert,
        Difficulty::BeginnerExtra,
        Difficulty::AdvancedExtra,
        Difficulty::ExpertExtra,
        Difficulty::Master,
    ];

    /// Key used by the reference (`beginner-extra`) and by the save file.
    pub fn key(self) -> &'static str {
        match self {
            Difficulty::Beginner => "beginner",
            Difficulty::Advanced => "advanced",
            Difficulty::Expert => "expert",
            Difficulty::BeginnerExtra => "beginner-extra",
            Difficulty::AdvancedExtra => "advanced-extra",
            Difficulty::ExpertExtra => "expert-extra",
            Difficulty::Master => "master",
        }
    }

    pub fn from_key(key: &str) -> Option<Difficulty> {
        Self::ALL.into_iter().find(|d| d.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Beginner => "Beginner",
            Difficulty::Advanced => "Advanced",
            Difficulty::Expert => "Expert",
            Difficulty::BeginnerExtra => "Beginner Extra",
            Difficulty::AdvancedExtra => "Advanced Extra",
            Difficulty::ExpertExtra => "Expert Extra",
            Difficulty::Master => "Master",
        }
    }

    pub fn is_extra(self) -> bool {
        matches!(
            self,
            Difficulty::BeginnerExtra | Difficulty::AdvancedExtra | Difficulty::ExpertExtra
        )
    }

    /// The Extra course unlocked by this main course.
    pub fn extra(self) -> Option<Difficulty> {
        match self {
            Difficulty::Beginner => Some(Difficulty::BeginnerExtra),
            Difficulty::Advanced => Some(Difficulty::AdvancedExtra),
            Difficulty::Expert => Some(Difficulty::ExpertExtra),
            _ => None,
        }
    }

    pub(crate) fn script(self) -> &'static [Op] {
        match self {
            Difficulty::Beginner => scripts::BEGINNER_MAIN,
            Difficulty::Advanced => scripts::ADVANCED_MAIN,
            Difficulty::Expert => scripts::EXPERT_MAIN,
            Difficulty::BeginnerExtra => scripts::BEGINNER_EXTRA,
            Difficulty::AdvancedExtra => scripts::ADVANCED_EXTRA,
            Difficulty::ExpertExtra => scripts::EXPERT_EXTRA,
            Difficulty::Master => scripts::MASTER,
        }
    }
}

/// One floor of a course (`getStageListForDifficulty`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FloorEntry {
    pub id: u32,
    pub name: &'static str,
    pub label: String,
    /// Script index just after the `Floor` command.
    pub next_index: usize,
    /// `Time` frames when the entry sets one.
    pub time_limit: Option<u32>,
}

fn build_stage_list(script: &'static [Op]) -> Vec<FloorEntry> {
    script
        .iter()
        .enumerate()
        .filter_map(|(i, op)| match op {
            Op::Floor(name) => Some(FloorEntry {
                id: stage_id_from_name(name),
                name,
                label: stage_label_from_name(name),
                next_index: i + 1,
                time_limit: match script.get(i + 1) {
                    Some(Op::Time(t)) => Some(*t),
                    _ => None,
                },
            }),
            _ => None,
        })
        .collect()
}

/// Floors of a course in script order.
pub fn stage_list(difficulty: Difficulty) -> Vec<FloorEntry> {
    build_stage_list(difficulty.script())
}

/// Result of a stage as seen by the scripts (`info` of `Course.advance`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdvanceInfo {
    pub flags: u32,
    /// `b'B'`, `b'G'`, `b'R'` for goals.
    pub goal_type: Option<u8>,
    pub timer_curr: u32,
    pub curr_stage_id: u32,
}

#[derive(Clone, Debug)]
pub struct Course {
    difficulty: Difficulty,
    script: &'static [Op],
    stages: Vec<FloorEntry>,
    script_index: usize,
    current_floor: usize,
    current_name: &'static str,
    current_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloorInfo {
    pub current: usize,
    pub total: usize,
    pub is_final: bool,
}

impl Course {
    pub fn new(difficulty: Difficulty) -> Self {
        Self::at(difficulty, 0)
    }

    /// Start at floor index `stage_index` (0-based, clamped), like `new Course(difficulty, stageIndex)`.
    pub fn at(difficulty: Difficulty, stage_index: usize) -> Self {
        let script = difficulty.script();
        let stages = build_stage_list(script);
        let mut c = Course {
            difficulty,
            script,
            stages,
            script_index: 0,
            current_floor: 1,
            current_name: "",
            current_id: 0,
        };
        c.set_stage_index(stage_index);
        c
    }

    pub fn difficulty(&self) -> Difficulty {
        self.difficulty
    }
    pub fn stages(&self) -> &[FloorEntry] {
        &self.stages
    }
    pub fn stage_id(&self) -> u32 {
        self.current_id
    }
    pub fn stage_name(&self) -> &'static str {
        self.current_name
    }
    pub fn stage_label(&self) -> String {
        stage_label_from_name(self.current_name)
    }
    /// 1-based floor number (the reference's `currentFloor`).
    pub fn floor(&self) -> usize {
        self.current_floor
    }

    pub fn floor_info(&self) -> FloorInfo {
        let total = self.stages.len();
        FloorInfo {
            current: self.current_floor,
            total,
            is_final: self.current_floor >= total,
        }
    }

    /// `getTimeLimitFrames`: the `Time` right after the current floor, else 60 s.
    pub fn time_limit_frames(&self) -> u32 {
        match self.script.get(self.script_index) {
            Some(Op::Time(t)) => *t,
            _ => DEFAULT_STAGE_TIME,
        }
    }

    pub fn set_stage_index(&mut self, stage_index: usize) {
        let Some(last) = self.stages.len().checked_sub(1) else {
            return;
        };
        let i = stage_index.min(last);
        let e = &self.stages[i];
        self.current_name = e.name;
        self.current_id = e.id;
        self.script_index = e.next_index;
        self.current_floor = i + 1;
    }

    fn eval_if(cmd: Op, info: &AdvanceInfo) -> bool {
        match cmd {
            Op::IfFloorClear => {
                info.flags & (INFO_GOAL | INFO_BONUS_CLEAR) != 0
                    || (crate::stages::is_bonus_stage(info.curr_stage_id)
                        && info.flags & (INFO_TIMEOVER | INFO_FALLOUT) != 0)
            }
            Op::IfGoalType(c) => info.goal_type == Some(c),
            Op::IfTimeElapsed(t) => info.timer_curr >= t,
            _ => false,
        }
    }

    /// Walk the script like `advance`/`peekJumpCount`: the `ThenJumpFloor` count that fires for `info`,
    /// with the script index of that command.
    fn find_jump(&self, info: &AdvanceInfo) -> Option<(usize, u32)> {
        let mut prev_if = false;
        let mut cond = false;
        for i in self.script_index..self.script.len() {
            let cmd = self.script[i];
            match cmd {
                Op::CourseEnd | Op::Floor(_) => return None,
                c if c.is_if() => {
                    let r = Self::eval_if(c, info);
                    // Consecutive IFs are ANDed; a new run of IFs starts afresh.
                    if !prev_if || cond {
                        cond = r;
                    }
                }
                Op::ThenJumpFloor(n) if cond => return Some((i, n)),
                _ => {}
            }
            prev_if = cmd.is_if();
        }
        None
    }

    /// Index of the `n`-th `Floor` at or after `from`, or `None` if `CourseEnd` comes first.
    fn nth_floor_from(&self, from: usize, n: u32) -> Option<usize> {
        if n == 0 {
            return None;
        }
        let mut count = 0;
        for i in from..self.script.len() {
            match self.script[i] {
                Op::CourseEnd => return None,
                Op::Floor(_) => {
                    count += 1;
                    if count == n {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Number of floors the script would jump for `info` (`peekJumpCount`); the course is not changed.
    pub fn peek_jump_count(&self, info: &AdvanceInfo) -> Option<u32> {
        let (at, n) = self.find_jump(info)?;
        self.nth_floor_from(at, n).map(|_| n)
    }

    /// Run the script for the finished stage. `true` = moved to another floor, `false` = course over.
    pub fn advance(&mut self, info: &AdvanceInfo) -> bool {
        let Some((at, n)) = self.find_jump(info) else {
            return false;
        };
        let Some(Op::Floor(name)) = self.nth_floor_from(at, n).map(|i| self.script[i]) else {
            return false;
        };
        let target = self.nth_floor_from(at, n).unwrap_or(0);
        self.current_name = name;
        self.current_id = stage_id_from_name(name);
        self.current_floor += n as usize;
        self.script_index = target + 1;
        true
    }

    /// Stage ids that can follow the current one (`getNextStageIds`, used for prefetching).
    pub fn next_stage_ids(&self) -> Vec<u32> {
        let mut ids = Vec::new();
        let mut add = |ids: &mut Vec<u32>, i: Option<usize>| {
            if let Some(Op::Floor(n)) = i.map(|i| self.script[i]) {
                let id = stage_id_from_name(n);
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        };
        add(&mut ids, self.nth_floor_from(self.script_index, 1));
        for goal in [b'B', b'G', b'R'] {
            for timer in [0, DEFAULT_STAGE_TIME] {
                let info = AdvanceInfo {
                    flags: INFO_GOAL,
                    goal_type: Some(goal),
                    timer_curr: timer,
                    curr_stage_id: self.current_id,
                };
                if let Some(n) = self.peek_jump_count(&info).filter(|n| *n > 0) {
                    add(&mut ids, self.nth_floor_from(self.script_index, n));
                }
            }
        }
        ids
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(c: &Course, goal: u8) -> AdvanceInfo {
        AdvanceInfo {
            flags: INFO_GOAL,
            goal_type: Some(goal),
            timer_curr: 100,
            curr_stage_id: c.stage_id(),
        }
    }

    #[test]
    fn floor_counts_match_the_reference() {
        let n = |d| stage_list(d).len();
        assert_eq!(n(Difficulty::Beginner), 10);
        assert_eq!(n(Difficulty::Advanced), 30);
        assert_eq!(n(Difficulty::Expert), 50);
        assert_eq!(n(Difficulty::BeginnerExtra), 3);
        assert_eq!(n(Difficulty::AdvancedExtra), 5);
        assert_eq!(n(Difficulty::ExpertExtra), 10);
        assert_eq!(n(Difficulty::Master), 10);
    }

    #[test]
    fn beginner_order_and_bonus_floor() {
        let ids: Vec<u32> = stage_list(Difficulty::Beginner)
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, [1, 2, 3, 4, 91, 5, 6, 7, 8, 9]);
        let ids: Vec<u32> = stage_list(Difficulty::Master)
            .iter()
            .map(|e| e.id)
            .collect();
        assert_eq!(ids, (121..=130).collect::<Vec<_>>());
    }

    #[test]
    fn plain_goal_goes_to_next_floor() {
        let mut c = Course::new(Difficulty::Beginner);
        assert_eq!((c.stage_id(), c.floor()), (1, 1));
        let g = goal(&c, b'B');
        assert!(c.advance(&g));
        assert_eq!((c.stage_id(), c.floor()), (2, 2));
        assert_eq!(c.stage_label(), "DIAMOND");
    }

    #[test]
    fn green_goal_skips_floors() {
        // ST_002 green goal jumps 3 floors: 3 (1), 4 (2), 091 (3).
        let mut c = Course::at(Difficulty::Beginner, 1);
        let g = goal(&c, b'G');
        assert_eq!(c.peek_jump_count(&g), Some(3));
        assert!(c.advance(&g));
        assert_eq!((c.stage_id(), c.floor()), (91, 5));
        // Blue goal on the same floor just moves on.
        let mut c = Course::at(Difficulty::Beginner, 1);
        let g = goal(&c, b'B');
        assert!(c.advance(&g));
        assert_eq!(c.stage_id(), 3);
    }

    #[test]
    fn failing_a_normal_stage_does_not_advance() {
        for flags in [INFO_FALLOUT, INFO_TIMEOVER] {
            let c = Course::new(Difficulty::Beginner);
            let info = AdvanceInfo {
                flags,
                goal_type: None,
                timer_curr: 0,
                curr_stage_id: c.stage_id(),
            };
            assert!(!c.clone().advance(&info));
            assert_eq!(c.stage_id(), 1);
        }
    }

    #[test]
    fn bonus_floor_clears_on_fallout_and_timeover() {
        for flags in [INFO_FALLOUT, INFO_TIMEOVER, INFO_BONUS_CLEAR] {
            let mut c = Course::at(Difficulty::Beginner, 4);
            assert_eq!(c.stage_id(), 91);
            let info = AdvanceInfo {
                flags,
                goal_type: None,
                timer_curr: 0,
                curr_stage_id: 91,
            };
            assert!(c.advance(&info));
            assert_eq!(c.stage_id(), 5);
        }
    }

    #[test]
    fn last_floor_ends_the_course() {
        let mut c = Course::at(Difficulty::Beginner, 99);
        assert_eq!(c.stage_id(), 9);
        assert!(c.floor_info().is_final);
        let g = goal(&c, b'B');
        assert!(!c.advance(&g));
    }

    #[test]
    fn time_limits_come_from_the_script() {
        // Advanced floor 5 (ST_091) has a 30 s limit, floor 1 uses the default.
        assert_eq!(
            Course::at(Difficulty::Advanced, 0).time_limit_frames(),
            3600
        );
        assert_eq!(
            Course::at(Difficulty::Advanced, 4).time_limit_frames(),
            1800
        );
        assert_eq!(
            Course::new(Difficulty::BeginnerExtra).time_limit_frames(),
            1800
        );
        let l = stage_list(Difficulty::Advanced);
        assert_eq!(l[4].time_limit, Some(1800));
        assert_eq!(l[0].time_limit, None);
    }

    #[test]
    fn next_stage_ids_lists_all_branches() {
        let c = Course::at(Difficulty::Beginner, 1);
        let mut ids = c.next_stage_ids();
        ids.sort();
        assert_eq!(ids, [3, 91]);
    }

    #[test]
    fn difficulty_keys_round_trip() {
        for d in Difficulty::ALL {
            assert_eq!(Difficulty::from_key(d.key()), Some(d));
        }
        assert_eq!(Difficulty::from_key("nope"), None);
    }

    #[test]
    fn every_listed_stage_has_a_background() {
        for d in Difficulty::ALL {
            for e in stage_list(d) {
                assert!(
                    crate::stages::bg_name(e.id).is_some(),
                    "{} {}",
                    d.key(),
                    e.id
                );
            }
        }
    }
}
