//! The interface between the menus / course flow and the gameplay state.
//!
//! Menus build a [`PlayRequest`] and push the gameplay state for it. When the stage ends the gameplay state
//! stores a [`PlayResult`] in `request.report` and pops itself; the state below (the menu) picks the result
//! up on its next update. Nothing here depends on rendering, so the gameplay agent can implement the other
//! side against this module alone.

use std::cell::RefCell;
use std::rc::Rc;

use crate::course::Difficulty;
use crate::progress::Settings;
use crate::run::StageOutcome;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayMode {
    /// Course play: lives, score and continues are tracked by the course flow.
    Course,
    /// One stage on its own, endless retries, no lives or score.
    Practice,
}

/// How a stage session ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlayResult {
    /// The stage reached its end (goal sequence, fall-out or time-over animation finished).
    Finished(StageOutcome),
    /// The player left from the pause menu; nothing is counted.
    Quit,
}

/// Slot the gameplay state writes its result into (single-threaded: menus and gameplay share the main
/// thread, so `Rc<RefCell<..>>` is enough, also on wasm).
#[derive(Clone, Debug, Default)]
pub struct PlayReport(Rc<RefCell<Option<PlayResult>>>);

impl PlayReport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Gameplay side: record the result (replaces an earlier one).
    pub fn set(&self, result: PlayResult) {
        *self.0.borrow_mut() = Some(result);
    }

    /// Menu side: take the result, leaving the slot empty.
    pub fn take(&self) -> Option<PlayResult> {
        self.0.borrow_mut().take()
    }

    pub fn is_set(&self) -> bool {
        self.0.borrow().is_some()
    }
}

/// Everything the gameplay state needs to start one stage.
#[derive(Clone, Debug)]
pub struct PlayRequest {
    pub mode: PlayMode,
    pub stage_id: u32,
    /// Background model name (`bg_jun`, ...), from the reference's stage table.
    pub bg: &'static str,
    /// Time limit in frames (60 per second).
    pub time_limit: u32,
    /// True when this restarts the stage after a lost life or from the retry button.
    pub retry: bool,
    pub difficulty: Difficulty,
    /// 1-based floor and floor count of the course (practice: position in the stage list).
    pub floor: u32,
    pub floors: u32,
    pub stage_label: String,
    /// HUD values at stage start (course mode; zero in practice).
    pub lives: u32,
    pub score: u64,
    pub bananas: u32,
    /// Snapshot of the user settings (volumes, controls, tilt).
    pub settings: Settings,
    /// Where to put the result.
    pub report: PlayReport,
}

impl PlayRequest {
    pub fn is_bonus_stage(&self) -> bool {
        crate::stages::is_bonus_stage(self.stage_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::GoalType;

    #[test]
    fn report_is_shared_and_taken_once() {
        let a = PlayReport::new();
        let b = a.clone();
        assert!(!a.is_set());
        b.set(PlayResult::Finished(StageOutcome::Goal {
            goal: GoalType::Blue,
            time_left: 100,
            bananas: 3,
        }));
        assert!(a.is_set());
        assert!(matches!(a.take(), Some(PlayResult::Finished(_))));
        assert_eq!(a.take(), None);
    }
}
