//! Course Play and Practice flow: lives, continues, score and bananas around [`Course`].
//!
//! The gameplay state reports how a stage ended as a [`StageOutcome`]; [`CourseRun::apply`] turns it into
//! the next [`Step`] and updates the saved [`Progress`].
//!
//! Sources. Scoring (goal score, 100 points per banana, the 999,999,999 cap), three starting lives, the
//! bonus-stage rule (no life lost, the course still advances) and the floor order come from WebMonkeyBall
//! (`game.ts`, `course.ts`). The reference never ends a run: at zero lives it keeps reloading the stage, it
//! has no continues, and it zeroes the banana counter on every stage load so bananas never give a life.
//! Those three gaps are filled with the original game's rules: the run is over when the last life is lost,
//! a continue restores the lives and resets the score, and the banana counter carries across floors and
//! pays one life per 100.
//!
//! Score has one source of truth here: the outcome carries what happened (goal colour, frames left, bananas
//! collected) and this module prices it. [`Rules::goal_score`] is the reference's `computeGoalScore`, the
//! same formula the engine uses for the in-stage HUD, so `floor.score + engine score` shown during a stage
//! equals [`CourseRun::score`] after the outcome is applied.

use crate::course::{
    AdvanceInfo, Course, Difficulty, INFO_BONUS_CLEAR, INFO_FALLOUT, INFO_GOAL, INFO_TIMEOVER,
};
use crate::play::{PlayMode, PlayReport, PlayRequest};
use crate::progress::{Progress, Settings};
use crate::stages::{bg_name, is_bonus_stage};

/// Score cap of the reference (`beginGoalSequence`) and of the HUD's nine digits.
pub const MAX_SCORE: u64 = 999_999_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GoalType {
    Blue,
    Green,
    Red,
}

impl GoalType {
    /// The letter the course scripts use (`Op::IfGoalType`).
    pub fn letter(self) -> u8 {
        match self {
            GoalType::Blue => b'B',
            GoalType::Green => b'G',
            GoalType::Red => b'R',
        }
    }

    pub fn from_letter(letter: u8) -> Option<GoalType> {
        match letter {
            b'B' => Some(GoalType::Blue),
            b'G' => Some(GoalType::Green),
            b'R' => Some(GoalType::Red),
            _ => None,
        }
    }
}

/// How a stage ended. `bananas` is the banana value collected during this attempt (a bunch counts 10);
/// it is kept on a fall-out or time-over as well, like the reference keeps the banana points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageOutcome {
    /// The ball went through a goal with `time_left` frames on the clock.
    Goal {
        goal: GoalType,
        time_left: u32,
        bananas: u32,
    },
    /// Every banana of a bonus stage was collected (no goal score in the reference).
    BonusClear {
        time_left: u32,
        bananas: u32,
    },
    FallOut {
        bananas: u32,
    },
    TimeOver {
        bananas: u32,
    },
}

impl StageOutcome {
    pub fn bananas(self) -> u32 {
        match self {
            StageOutcome::Goal { bananas, .. }
            | StageOutcome::BonusClear { bananas, .. }
            | StageOutcome::FallOut { bananas }
            | StageOutcome::TimeOver { bananas } => bananas,
        }
    }

    /// Goal or bonus clear (a failed bonus stage still advances, but is not a clear).
    pub fn is_clear(self) -> bool {
        matches!(
            self,
            StageOutcome::Goal { .. } | StageOutcome::BonusClear { .. }
        )
    }

    fn time_left(self) -> u32 {
        match self {
            StageOutcome::Goal { time_left, .. } | StageOutcome::BonusClear { time_left, .. } => {
                time_left
            }
            StageOutcome::FallOut { .. } | StageOutcome::TimeOver { .. } => 0,
        }
    }

    fn advance_info(self, stage_id: u32, time_limit: u32) -> AdvanceInfo {
        let (flags, goal_type) = match self {
            StageOutcome::Goal { goal, .. } => (INFO_GOAL, Some(goal.letter())),
            StageOutcome::BonusClear { .. } => (INFO_BONUS_CLEAR, None),
            StageOutcome::FallOut { .. } => (INFO_FALLOUT, None),
            StageOutcome::TimeOver { .. } => (INFO_TIMEOVER, None),
        };
        AdvanceInfo {
            flags,
            goal_type,
            // The reference passes the elapsed stage timer. A fall-out does not report its time, which
            // is fine: no SMB1 script has an `IfTimeElapsed`.
            timer_curr: match self {
                StageOutcome::FallOut { .. } => 0,
                _ => time_limit.saturating_sub(self.time_left()),
            },
            curr_stage_id: stage_id,
        }
    }
}

/// Rule constants. The default is the reference where it has the rule and the original game elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    /// Lives at the start of a run and after a continue (`DEFAULT_LIVES`). The run ends when it reaches 0.
    pub lives: u32,
    pub max_lives: u32,
    /// Continues per run (original default; the reference has none).
    pub continues: u32,
    /// Banana value that pays one extra life; 0 turns extra lives off.
    pub bananas_per_extra_life: u32,
    /// Points per banana value (`BANANA_SINGLE_POINTS`; a bunch is worth 10 bananas and 1000 points).
    pub banana_score: u64,
    pub green_goal_score: u64,
    pub red_goal_score: u64,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            lives: 3,
            max_lives: 99,
            continues: 5,
            bananas_per_extra_life: 100,
            banana_score: 100,
            green_goal_score: 10_000,
            red_goal_score: 20_000,
        }
    }
}

impl Rules {
    /// `computeGoalScore`: 100 points per second left (integer division on frames), plus the goal colour
    /// bonus, doubled when more than half of the time limit is left.
    pub fn goal_score(&self, goal: GoalType, time_left: u32, time_limit: u32) -> u64 {
        let mut score = u64::from(time_left) * 100 / 60;
        score += match goal {
            GoalType::Blue => 0,
            GoalType::Green => self.green_goal_score,
            GoalType::Red => self.red_goal_score,
        };
        if time_limit > 0 && time_left > time_limit >> 1 {
            score *= 2;
        }
        score
    }
}

/// One stage to play, with the HUD values at its start. Everything a [`PlayRequest`] needs except the
/// settings snapshot and the report slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Floor {
    pub mode: PlayMode,
    pub difficulty: Difficulty,
    pub stage_id: u32,
    /// Background model name from [`crate::stages::bg_name`]. Every course stage has one (tested in
    /// `course.rs`); an id outside the table would give an empty name.
    pub bg: &'static str,
    /// Time limit in frames.
    pub time_limit: u32,
    /// 1-based floor number and the course's floor count.
    pub floor: u32,
    pub floors: u32,
    pub is_final: bool,
    pub stage_label: String,
    /// True when the stage is started again (after a lost life, a continue, or in practice).
    pub retry: bool,
    pub lives: u32,
    pub score: u64,
    pub bananas: u32,
}

impl Floor {
    pub fn is_bonus_stage(&self) -> bool {
        is_bonus_stage(self.stage_id)
    }

    /// The request for the gameplay state; `report` is the slot the caller will poll.
    pub fn request(&self, settings: &Settings, report: &PlayReport) -> PlayRequest {
        PlayRequest {
            mode: self.mode,
            stage_id: self.stage_id,
            bg: self.bg,
            time_limit: self.time_limit,
            retry: self.retry,
            difficulty: self.difficulty,
            floor: self.floor,
            floors: self.floors,
            stage_label: self.stage_label.clone(),
            lives: self.lives,
            score: self.score,
            bananas: self.bananas,
            settings: settings.clone(),
            report: report.clone(),
        }
    }
}

/// What the flow does after a stage ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The stage was cleared (or was a bonus stage): play `floor`. `skipped` floors were jumped over by a
    /// green or red goal (0 for the plain next floor).
    Next { floor: Floor, skipped: u32 },
    /// Play the same stage again: a life was lost (course) or the attempt failed (practice).
    Retry(Floor),
    /// The last life was lost. With `continues_left > 0` offer a continue ([`CourseRun::continue_run`]).
    /// `score` is the score of the ended credit; it has been recorded.
    GameOver { score: u64, continues_left: u32 },
    /// The last floor was cleared. `clean` = no continue was used; `unlocked` is the course this clear
    /// opened (per [`Progress::is_course_unlocked`]) if it was locked before.
    CourseComplete {
        score: u64,
        clean: bool,
        new_best_score: bool,
        unlocked: Option<Difficulty>,
    },
    /// Practice: the stage was cleared in `frames`. `floor` is the same stage again, should the player
    /// want another go.
    PracticeClear {
        floor: Floor,
        frames: u32,
        new_best_time: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Playing,
    GameOver,
    Complete {
        new_best_score: bool,
        unlocked: Option<Difficulty>,
    },
}

/// Course Play (or Practice) on one difficulty.
#[derive(Clone, Debug)]
pub struct CourseRun {
    mode: PlayMode,
    rules: Rules,
    course: Course,
    lives: u32,
    score: u64,
    bananas: u32,
    continues_left: u32,
    continues_used: u32,
    retry: bool,
    last_extra_lives: u32,
    state: State,
}

impl CourseRun {
    /// Course Play from floor 1 with the default rules.
    pub fn new(difficulty: Difficulty) -> Self {
        Self::with_rules(difficulty, Rules::default())
    }

    pub fn with_rules(difficulty: Difficulty, rules: Rules) -> Self {
        CourseRun {
            mode: PlayMode::Course,
            rules,
            course: Course::new(difficulty),
            lives: rules.lives.min(rules.max_lives),
            score: 0,
            bananas: 0,
            continues_left: rules.continues,
            continues_used: 0,
            retry: false,
            last_extra_lives: 0,
            state: State::Playing,
        }
    }

    /// Practice: the stage at `stage_index` (0-based, clamped) of the course's stage list, endless
    /// retries, no lives, score or bananas. Only per-stage best times are recorded.
    pub fn practice(difficulty: Difficulty, stage_index: usize) -> Self {
        CourseRun {
            mode: PlayMode::Practice,
            rules: Rules::default(),
            course: Course::at(difficulty, stage_index),
            lives: 0,
            score: 0,
            bananas: 0,
            continues_left: 0,
            continues_used: 0,
            retry: false,
            last_extra_lives: 0,
            state: State::Playing,
        }
    }

    pub fn mode(&self) -> PlayMode {
        self.mode
    }
    pub fn difficulty(&self) -> Difficulty {
        self.course.difficulty()
    }
    pub fn rules(&self) -> &Rules {
        &self.rules
    }
    /// The script interpreter (stage list, `next_stage_ids` for prefetching).
    pub fn course(&self) -> &Course {
        &self.course
    }
    pub fn lives(&self) -> u32 {
        self.lives
    }
    pub fn score(&self) -> u64 {
        self.score
    }
    /// Banana counter towards the next extra life.
    pub fn bananas(&self) -> u32 {
        self.bananas
    }
    pub fn continues_left(&self) -> u32 {
        self.continues_left
    }
    pub fn continues_used(&self) -> u32 {
        self.continues_used
    }
    pub fn is_game_over(&self) -> bool {
        self.state == State::GameOver
    }
    pub fn is_complete(&self) -> bool {
        matches!(self.state, State::Complete { .. })
    }
    /// Lives the most recent [`CourseRun::apply`] awarded for bananas (for the 1UP cue).
    pub fn last_extra_lives(&self) -> u32 {
        self.last_extra_lives
    }

    /// The stage to play now, with the current HUD values.
    pub fn floor(&self) -> Floor {
        let info = self.course.floor_info();
        let stage_id = self.course.stage_id();
        Floor {
            mode: self.mode,
            difficulty: self.course.difficulty(),
            stage_id,
            bg: bg_name(stage_id).unwrap_or_default(),
            time_limit: self.course.time_limit_frames(),
            floor: info.current as u32,
            floors: info.total as u32,
            is_final: info.is_final,
            stage_label: self.course.stage_label(),
            retry: self.retry,
            lives: self.lives,
            score: self.score,
            bananas: self.bananas,
        }
    }

    fn terminal_step(&self) -> Option<Step> {
        match self.state {
            State::Playing => None,
            State::GameOver => Some(Step::GameOver {
                score: self.score,
                continues_left: self.continues_left,
            }),
            State::Complete {
                new_best_score,
                unlocked,
            } => Some(Step::CourseComplete {
                score: self.score,
                clean: self.continues_used == 0,
                new_best_score,
                unlocked,
            }),
        }
    }

    /// Account for a finished stage and say what comes next. `progress` receives the records: reached
    /// floors, the stage's best time, the course's best score and its clear flags.
    ///
    /// Once the run is over (game over without a continue taken, or course complete) further outcomes
    /// change nothing and the same final step is returned.
    pub fn apply(&mut self, outcome: StageOutcome, progress: &mut Progress) -> Step {
        if let Some(step) = self.terminal_step() {
            return step;
        }
        self.last_extra_lives = 0;
        let stage_id = self.course.stage_id();
        let time_limit = self.course.time_limit_frames();
        let frames = time_limit.saturating_sub(outcome.time_left());

        if self.mode == PlayMode::Practice {
            self.retry = true;
            if !outcome.is_clear() {
                return Step::Retry(self.floor());
            }
            let new_best_time = progress.record_stage_clear(stage_id, frames);
            return Step::PracticeClear {
                floor: self.floor(),
                frames,
                new_best_time,
            };
        }

        let difficulty = self.course.difficulty();
        progress.mark_reached(difficulty, stage_id);

        let mut gained = u64::from(outcome.bananas()) * self.rules.banana_score;
        if let StageOutcome::Goal {
            goal, time_left, ..
        } = outcome
        {
            gained += self.rules.goal_score(goal, time_left, time_limit);
        }
        self.score = self.score.saturating_add(gained).min(MAX_SCORE);
        self.add_bananas(outcome.bananas());
        if outcome.is_clear() {
            progress.record_stage_clear(stage_id, frames);
        }
        let previous_best = progress.course(difficulty).best_score;
        progress.record_score(difficulty, self.score);

        if !outcome.is_clear() && !is_bonus_stage(stage_id) {
            self.lives = self.lives.saturating_sub(1);
            self.retry = true;
            if self.lives == 0 {
                self.state = State::GameOver;
                return Step::GameOver {
                    score: self.score,
                    continues_left: self.continues_left,
                };
            }
            return Step::Retry(self.floor());
        }

        let from = self.course.floor();
        if self
            .course
            .advance(&outcome.advance_info(stage_id, time_limit))
        {
            self.retry = false;
            progress.mark_reached(difficulty, self.course.stage_id());
            let skipped = (self.course.floor() - from).saturating_sub(1) as u32;
            return Step::Next {
                floor: self.floor(),
                skipped,
            };
        }

        let clean = self.continues_used == 0;
        let was_unlocked = Difficulty::ALL.map(|d| progress.is_course_unlocked(d, false));
        progress.record_course_clear(difficulty, clean);
        let unlocked = Difficulty::ALL
            .into_iter()
            .zip(was_unlocked)
            .find(|(d, was)| !was && progress.is_course_unlocked(*d, false))
            .map(|(d, _)| d);
        let new_best_score = self.score > previous_best;
        self.state = State::Complete {
            new_best_score,
            unlocked,
        };
        Step::CourseComplete {
            score: self.score,
            clean,
            new_best_score,
            unlocked,
        }
    }

    fn add_bananas(&mut self, bananas: u32) {
        self.bananas = self.bananas.saturating_add(bananas);
        let per_life = self.rules.bananas_per_extra_life;
        if per_life == 0 {
            return;
        }
        let earned = self.bananas / per_life;
        self.bananas %= per_life;
        let before = self.lives;
        self.lives = self
            .lives
            .saturating_add(earned)
            .min(self.rules.max_lives.max(before));
        self.last_extra_lives = self.lives - before;
    }

    /// Take a continue after a game over: lives are restored, the score starts again from zero (its
    /// best was recorded at the game over), and the same floor is replayed. `None` when the run is not
    /// at a game over or no continue is left; nothing changes then.
    pub fn continue_run(&mut self) -> Option<Step> {
        if self.state != State::GameOver || self.continues_left == 0 {
            return None;
        }
        self.continues_left -= 1;
        self.continues_used += 1;
        self.lives = self.rules.lives.min(self.rules.max_lives);
        self.score = 0;
        self.retry = true;
        self.state = State::Playing;
        Some(Step::Retry(self.floor()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::course::stage_list;
    use crate::play::PlayResult;

    const BLUE: StageOutcome = StageOutcome::Goal {
        goal: GoalType::Blue,
        time_left: 3000,
        bananas: 0,
    };

    fn goal(goal: GoalType) -> StageOutcome {
        StageOutcome::Goal {
            goal,
            time_left: 600,
            bananas: 0,
        }
    }

    fn next(step: Step) -> (Floor, u32) {
        match step {
            Step::Next { floor, skipped } => (floor, skipped),
            other => panic!("expected Next, got {other:?}"),
        }
    }

    /// Clear floors with blue goals until the run sits on 0-based floor `index`.
    fn run_at(difficulty: Difficulty, index: usize, progress: &mut Progress) -> CourseRun {
        let mut run = CourseRun::new(difficulty);
        while run.floor().floor as usize <= index {
            next(run.apply(goal(GoalType::Blue), progress));
        }
        run
    }

    #[test]
    fn goal_letters_round_trip() {
        for g in [GoalType::Blue, GoalType::Green, GoalType::Red] {
            assert_eq!(GoalType::from_letter(g.letter()), Some(g));
        }
        assert_eq!(GoalType::Red.letter(), b'R');
        assert_eq!(GoalType::from_letter(b'X'), None);
    }

    #[test]
    fn goal_score_matches_the_reference_formula() {
        let r = Rules::default();
        // floor(frames * 100 / 60), colour bonus, doubled above half of the limit.
        assert_eq!(r.goal_score(GoalType::Blue, 0, 3600), 0);
        assert_eq!(r.goal_score(GoalType::Blue, 1799, 3600), 2998);
        assert_eq!(r.goal_score(GoalType::Blue, 1800, 3600), 3000);
        assert_eq!(r.goal_score(GoalType::Blue, 1801, 3600), 6002);
        assert_eq!(r.goal_score(GoalType::Blue, 3000, 3600), 10_000);
        assert_eq!(r.goal_score(GoalType::Green, 600, 3600), 11_000);
        assert_eq!(r.goal_score(GoalType::Red, 600, 3600), 21_000);
        assert_eq!(r.goal_score(GoalType::Red, 1000, 1800), 43_332);
        assert_eq!(r.goal_score(GoalType::Blue, 100, 0), 166);
    }

    #[test]
    fn default_rules() {
        let r = Rules::default();
        assert_eq!(
            (r.lives, r.continues, r.bananas_per_extra_life),
            (3, 5, 100)
        );
        assert_eq!(r.banana_score, 100);
        let run = CourseRun::new(Difficulty::Beginner);
        assert_eq!((run.lives(), run.score(), run.bananas()), (3, 0, 0));
        assert_eq!(run.continues_left(), 5);
        assert_eq!(run.mode(), PlayMode::Course);
    }

    #[test]
    fn full_beginner_run_with_blue_goals() {
        let mut progress = Progress::default();
        let mut run = CourseRun::new(Difficulty::Beginner);
        let list = stage_list(Difficulty::Beginner);
        let first = run.floor();
        assert_eq!((first.stage_id, first.floor, first.floors), (1, 1, 10));
        assert!(!first.retry && !first.is_final);

        let mut played = vec![first.stage_id];
        let last = loop {
            let floor = run.floor();
            let outcome = if floor.is_bonus_stage() {
                StageOutcome::BonusClear {
                    time_left: 1200,
                    bananas: 50,
                }
            } else {
                BLUE
            };
            match run.apply(outcome, &mut progress) {
                Step::Next { floor, skipped } => {
                    assert_eq!(skipped, 0);
                    assert!(!floor.retry);
                    assert_eq!(floor.lives, 3);
                    assert_eq!(floor.score, run.score());
                    played.push(floor.stage_id);
                }
                other => break other,
            }
        };
        assert_eq!(played, list.iter().map(|e| e.id).collect::<Vec<_>>());
        // Nine goals at 10,000 each plus 50 bananas; the bonus clear itself scores nothing.
        let score = 9 * 10_000 + 50 * 100;
        assert_eq!(
            last,
            Step::CourseComplete {
                score,
                clean: true,
                new_best_score: true,
                unlocked: Some(Difficulty::BeginnerExtra),
            }
        );
        assert!(run.is_complete() && !run.is_game_over());
        assert_eq!(run.bananas(), 50);

        let saved = progress.course(Difficulty::Beginner);
        assert!(saved.cleared && saved.cleared_clean);
        assert_eq!(saved.best_score, score);
        assert_eq!(saved.reached.len(), 10);
        assert!(progress.is_course_unlocked(Difficulty::BeginnerExtra, false));
        for e in &list {
            assert!(progress.is_stage_unlocked(Difficulty::Beginner, e.id, false));
            assert_eq!(progress.stage_best(e.id).clears, 1);
        }
        assert_eq!(progress.stage_best(1).best_frames, 600);
        assert_eq!(progress.stage_best(91).best_frames, 2400);

        // A finished run is inert.
        let before = progress.clone();
        assert_eq!(run.apply(BLUE, &mut progress), last);
        assert_eq!(progress, before);
        assert_eq!(run.continue_run(), None);
    }

    #[test]
    fn steps_agree_with_the_course_interpreter() {
        for difficulty in Difficulty::ALL {
            let mut progress = Progress::default();
            let mut run = CourseRun::new(difficulty);
            let list = stage_list(difficulty);
            for (i, entry) in list.iter().enumerate() {
                let floor = run.floor();
                let reference = Course::at(difficulty, i);
                assert_eq!(floor.stage_id, entry.id);
                assert_eq!(floor.stage_label, entry.label);
                assert_eq!(floor.floor as usize, i + 1);
                assert_eq!(floor.floors as usize, list.len());
                assert_eq!(floor.is_final, i + 1 == list.len());
                assert_eq!(floor.time_limit, reference.time_limit_frames());
                assert_eq!(
                    floor.time_limit,
                    entry
                        .time_limit
                        .unwrap_or(crate::stages::DEFAULT_STAGE_TIME)
                );
                assert_eq!(Some(floor.bg), bg_name(entry.id));
                assert!(!floor.bg.is_empty());
                assert_eq!(floor.difficulty, difficulty);
                assert_eq!(run.course().next_stage_ids(), reference.next_stage_ids());
                let step = run.apply(goal(GoalType::Blue), &mut progress);
                if i + 1 == list.len() {
                    assert!(matches!(step, Step::CourseComplete { .. }), "{step:?}");
                } else {
                    assert_eq!(
                        step,
                        Step::Next {
                            floor: run.floor(),
                            skipped: 0
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn green_and_red_goals_skip_floors_like_the_scripts() {
        let mut progress = Progress::default();

        // Beginner floor 2: green jumps 3 floors to the bonus stage on floor 5.
        let mut run = run_at(Difficulty::Beginner, 1, &mut progress);
        assert_eq!(run.floor().stage_id, 2);
        let (floor, skipped) = next(run.apply(goal(GoalType::Green), &mut progress));
        assert_eq!((floor.stage_id, floor.floor, skipped), (91, 5, 2));
        assert_eq!(run.score(), 1000 + 11_000);
        // Skipped floors are not reached.
        assert!(!progress.is_stage_unlocked(Difficulty::Beginner, 3, false));
        assert!(progress.is_stage_unlocked(Difficulty::Beginner, 91, false));

        // Advanced floor 18 (ST_028): blue +1, green +2, red +7.
        for (colour, stage, floor_no) in [
            (GoalType::Blue, 29, 19),
            (GoalType::Green, 93, 20),
            (GoalType::Red, 35, 25),
        ] {
            let mut run = run_at(Difficulty::Advanced, 17, &mut progress);
            assert_eq!(run.floor().stage_id, 28);
            let (floor, skipped) = next(run.apply(goal(colour), &mut progress));
            assert_eq!(
                (floor.stage_id, floor.floor),
                (stage, floor_no),
                "{colour:?}"
            );
            assert_eq!(skipped, floor_no - 19);
        }

        // Expert floor 2 (ST_042): red jumps 3 floors.
        let mut run = run_at(Difficulty::Expert, 1, &mut progress);
        let (floor, skipped) = next(run.apply(goal(GoalType::Red), &mut progress));
        assert_eq!((floor.stage_id, floor.floor, skipped), (91, 5, 2));
        assert_eq!(floor.time_limit, 1800);
    }

    #[test]
    fn losing_lives_then_game_over_then_continue() {
        let mut progress = Progress::default();
        let mut run = run_at(Difficulty::Beginner, 2, &mut progress);
        let score = run.score();
        assert_eq!(score, 2 * 1000);

        let step = run.apply(StageOutcome::FallOut { bananas: 2 }, &mut progress);
        let Step::Retry(floor) = step else {
            panic!("{step:?}")
        };
        assert_eq!((floor.stage_id, floor.floor, floor.lives), (3, 3, 2));
        assert!(floor.retry);
        // Banana points survive a fall-out, as in the reference.
        assert_eq!((floor.score, floor.bananas), (score + 200, 2));

        let step = run.apply(StageOutcome::TimeOver { bananas: 0 }, &mut progress);
        assert!(matches!(&step, Step::Retry(f) if f.lives == 1 && f.stage_id == 3));

        let step = run.apply(StageOutcome::FallOut { bananas: 0 }, &mut progress);
        assert_eq!(
            step,
            Step::GameOver {
                score: score + 200,
                continues_left: 5
            }
        );
        assert!(run.is_game_over());
        assert_eq!(run.lives(), 0);
        assert_eq!(
            progress.course(Difficulty::Beginner).best_score,
            score + 200
        );
        // Outcomes are ignored until the continue is taken.
        assert_eq!(run.apply(BLUE, &mut progress), step);
        assert_eq!(run.floor().stage_id, 3);

        let step = run.continue_run().expect("continues are left");
        let Step::Retry(floor) = step else {
            panic!("{step:?}")
        };
        assert_eq!((floor.stage_id, floor.floor), (3, 3));
        assert_eq!((floor.lives, floor.score, floor.bananas), (3, 0, 2));
        assert!(floor.retry);
        assert_eq!((run.continues_left(), run.continues_used()), (4, 1));
        assert!(!run.is_game_over());
        assert_eq!(run.continue_run(), None, "only after a game over");

        // Finishing after a continue clears the course but does not open the Extra course.
        let last = loop {
            let floor = run.floor();
            let outcome = if floor.is_bonus_stage() {
                StageOutcome::TimeOver { bananas: 0 }
            } else {
                BLUE
            };
            match run.apply(outcome, &mut progress) {
                Step::Next { .. } => {}
                other => break other,
            }
        };
        assert_eq!(
            last,
            Step::CourseComplete {
                score: 7 * 10_000,
                clean: false,
                new_best_score: true,
                unlocked: None,
            }
        );
        let saved = progress.course(Difficulty::Beginner);
        assert!(saved.cleared && !saved.cleared_clean);
        assert_eq!(saved.best_score, 70_000);
        assert!(!progress.is_course_unlocked(Difficulty::BeginnerExtra, false));
    }

    #[test]
    fn running_out_of_continues_ends_the_run() {
        let rules = Rules {
            lives: 1,
            continues: 1,
            ..Rules::default()
        };
        let mut progress = Progress::default();
        let mut run = CourseRun::with_rules(Difficulty::Master, rules);
        let fall = StageOutcome::FallOut { bananas: 0 };
        assert_eq!(
            run.apply(fall, &mut progress),
            Step::GameOver {
                score: 0,
                continues_left: 1
            }
        );
        assert!(matches!(run.continue_run(), Some(Step::Retry(f)) if f.lives == 1));
        let over = Step::GameOver {
            score: 0,
            continues_left: 0,
        };
        assert_eq!(run.apply(fall, &mut progress), over);
        assert_eq!(run.continue_run(), None);
        assert_eq!(run.apply(BLUE, &mut progress), over);
        assert!(run.is_game_over());
        assert!(!progress.course(Difficulty::Master).cleared);
        assert_eq!(progress.course(Difficulty::Master).reached.len(), 1);
    }

    #[test]
    fn bonus_stage_failure_advances_without_losing_a_life() {
        for outcome in [
            StageOutcome::FallOut { bananas: 7 },
            StageOutcome::TimeOver { bananas: 7 },
        ] {
            let mut progress = Progress::default();
            let mut run = run_at(Difficulty::Beginner, 4, &mut progress);
            assert!(run.floor().is_bonus_stage());
            let score = run.score();
            let (floor, skipped) = next(run.apply(outcome, &mut progress));
            assert_eq!((floor.stage_id, floor.floor, skipped), (5, 6, 0));
            assert_eq!((floor.lives, floor.bananas), (3, 7));
            assert_eq!(floor.score, score + 700);
            assert!(!floor.retry);
            // Not a clear: no best time for the bonus stage.
            assert_eq!(progress.stage_best(91).clears, 0);
        }
    }

    #[test]
    fn bananas_pay_extra_lives() {
        let mut progress = Progress::default();
        let mut run = CourseRun::new(Difficulty::Beginner);
        let with = |bananas| StageOutcome::Goal {
            goal: GoalType::Blue,
            time_left: 0,
            bananas,
        };
        next(run.apply(with(60), &mut progress));
        assert_eq!(
            (run.lives(), run.bananas(), run.last_extra_lives()),
            (3, 60, 0)
        );
        // The counter carries over floors and wraps at 100.
        let (floor, _) = next(run.apply(with(45), &mut progress));
        assert_eq!(
            (run.lives(), run.bananas(), run.last_extra_lives()),
            (4, 5, 1)
        );
        assert_eq!((floor.lives, floor.bananas), (4, 5));
        assert_eq!(run.score(), 105 * 100);
        // Bananas of a failed attempt count too; the life is paid before the loss is taken.
        let step = run.apply(StageOutcome::FallOut { bananas: 195 }, &mut progress);
        assert!(matches!(&step, Step::Retry(f) if f.lives == 5 && f.bananas == 0));
        assert_eq!(run.last_extra_lives(), 2);
        next(run.apply(with(1), &mut progress));
        assert_eq!(run.last_extra_lives(), 0);

        // A life earned on the last-life fall-out keeps the run alive.
        let mut run = CourseRun::with_rules(
            Difficulty::Beginner,
            Rules {
                lives: 1,
                ..Rules::default()
            },
        );
        let step = run.apply(StageOutcome::FallOut { bananas: 100 }, &mut progress);
        assert!(matches!(&step, Step::Retry(f) if f.lives == 1));

        // Capped lives and switched-off extra lives.
        let mut run = CourseRun::with_rules(
            Difficulty::Beginner,
            Rules {
                max_lives: 4,
                ..Rules::default()
            },
        );
        next(run.apply(with(500), &mut progress));
        assert_eq!((run.lives(), run.last_extra_lives()), (4, 1));
        let mut run = CourseRun::with_rules(
            Difficulty::Beginner,
            Rules {
                bananas_per_extra_life: 0,
                ..Rules::default()
            },
        );
        next(run.apply(with(500), &mut progress));
        assert_eq!((run.lives(), run.bananas()), (3, 500));
    }

    #[test]
    fn score_is_capped() {
        let mut progress = Progress::default();
        let mut run = CourseRun::new(Difficulty::Beginner);
        let step = run.apply(StageOutcome::FallOut { bananas: u32::MAX }, &mut progress);
        assert!(matches!(step, Step::Retry(_)));
        assert_eq!(run.score(), MAX_SCORE);
        assert_eq!(progress.course(Difficulty::Beginner).best_score, MAX_SCORE);
    }

    #[test]
    fn best_score_is_only_new_when_beaten() {
        let mut progress = Progress::default();
        progress.record_score(Difficulty::BeginnerExtra, 1_000_000);
        let mut run = CourseRun::new(Difficulty::BeginnerExtra);
        next(run.apply(BLUE, &mut progress));
        next(run.apply(BLUE, &mut progress));
        let step = run.apply(BLUE, &mut progress);
        // Extra courses have a 30 s limit: 3000 frames left is above half, so 5000 * 2 each.
        assert_eq!(
            step,
            Step::CourseComplete {
                score: 30_000,
                clean: true,
                new_best_score: false,
                unlocked: None,
            }
        );
        assert_eq!(
            progress.course(Difficulty::BeginnerExtra).best_score,
            1_000_000
        );
    }

    #[test]
    fn clearing_the_last_extra_course_opens_master() {
        let mut progress = Progress::default();
        progress.record_course_clear(Difficulty::BeginnerExtra, true);
        progress.record_course_clear(Difficulty::AdvancedExtra, false);
        let mut run = CourseRun::new(Difficulty::ExpertExtra);
        let last = loop {
            match run.apply(BLUE, &mut progress) {
                Step::Next { .. } => {}
                other => break other,
            }
        };
        assert!(
            matches!(
                last,
                Step::CourseComplete {
                    unlocked: Some(Difficulty::Master),
                    ..
                }
            ),
            "{last:?}"
        );
        // Clearing it again unlocks nothing new.
        let mut run = CourseRun::new(Difficulty::ExpertExtra);
        let last = loop {
            match run.apply(BLUE, &mut progress) {
                Step::Next { .. } => {}
                other => break other,
            }
        };
        assert!(matches!(last, Step::CourseComplete { unlocked: None, .. }));
    }

    #[test]
    fn practice_is_one_stage_with_endless_retries() {
        let mut progress = Progress::default();
        let mut run = CourseRun::practice(Difficulty::Advanced, 4);
        assert_eq!(run.mode(), PlayMode::Practice);
        let floor = run.floor();
        assert_eq!((floor.stage_id, floor.floor, floor.floors), (91, 5, 30));
        assert_eq!(floor.time_limit, 1800);
        assert_eq!((floor.lives, floor.score, floor.bananas), (0, 0, 0));
        assert!(!floor.retry);

        for _ in 0..10 {
            let step = run.apply(StageOutcome::FallOut { bananas: 150 }, &mut progress);
            let Step::Retry(again) = step else {
                panic!("{step:?}")
            };
            assert_eq!(
                again,
                Floor {
                    retry: true,
                    ..floor.clone()
                }
            );
        }
        let step = run.apply(StageOutcome::TimeOver { bananas: 0 }, &mut progress);
        assert!(matches!(step, Step::Retry(_)));
        assert!(!run.is_game_over());
        assert_eq!(run.last_extra_lives(), 0);
        assert_eq!(run.continue_run(), None);

        let clear = StageOutcome::BonusClear {
            time_left: 800,
            bananas: 200,
        };
        let step = run.apply(clear, &mut progress);
        let Step::PracticeClear {
            floor: again,
            frames,
            new_best_time,
        } = step
        else {
            panic!("{step:?}")
        };
        assert_eq!((frames, new_best_time), (1000, true));
        assert_eq!((again.stage_id, again.retry, again.score), (91, true, 0));
        // A slower clear is not a new best; the stage never changes.
        let step = run.apply(
            StageOutcome::BonusClear {
                time_left: 100,
                bananas: 0,
            },
            &mut progress,
        );
        assert!(matches!(
            step,
            Step::PracticeClear {
                frames: 1700,
                new_best_time: false,
                ..
            }
        ));
        assert_eq!(run.floor().stage_id, 91);
        assert!(!run.is_complete());

        // Only the best time is saved: no course records, no unlocks.
        assert_eq!(progress.stage_best(91).best_frames, 1000);
        assert_eq!(progress.stage_best(91).clears, 2);
        assert_eq!(progress.course(Difficulty::Advanced), Default::default());

        // The index is clamped to the last floor.
        assert_eq!(
            CourseRun::practice(Difficulty::Beginner, 99)
                .floor()
                .stage_id,
            9
        );
    }

    #[test]
    fn floor_builds_the_play_request() {
        let mut progress = Progress::default();
        let mut run = run_at(Difficulty::Advanced, 4, &mut progress);
        // Falling out of the bonus stage on floor 5 moves on to floor 6.
        let (floor, _) = next(run.apply(StageOutcome::FallOut { bananas: 3 }, &mut progress));
        let settings = Settings {
            music_volume: 7,
            ..Settings::default()
        };
        let report = PlayReport::new();
        let req = floor.request(&settings, &report);
        assert_eq!(req.mode, PlayMode::Course);
        assert_eq!(req.stage_id, 15);
        assert_eq!(Some(req.bg), bg_name(15));
        assert_eq!((req.floor, req.floors), (6, 30));
        assert_eq!(req.time_limit, 3600);
        assert_eq!(req.stage_label, "BREAK");
        assert_eq!(req.difficulty, Difficulty::Advanced);
        assert_eq!((req.lives, req.score, req.bananas), (3, run.score(), 3));
        assert!(!req.retry && !req.is_bonus_stage());
        assert_eq!(req.settings.music_volume, 7);

        // The gameplay side answers through the shared slot, and the answer drives the run.
        req.report.set(PlayResult::Finished(BLUE));
        let Some(PlayResult::Finished(outcome)) = report.take() else {
            panic!("no result")
        };
        let (floor, _) = next(run.apply(outcome, &mut progress));
        assert_eq!(floor.stage_id, 16);
    }
}
