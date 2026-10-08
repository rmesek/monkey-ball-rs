//! Rules that are stated in more than one crate must say the same thing. `mb-game` is std-only by design,
//! so it cannot call the engine; this crate depends on both and holds them to each other. No game data.

use mb_engine::sim::compute_goal_score;
use mb_game::{Difficulty, GoalType, Rules};

fn engine_goal(g: GoalType) -> mb_stage::GoalType {
    match g {
        GoalType::Blue => mb_stage::GoalType::Blue,
        GoalType::Green => mb_stage::GoalType::Green,
        GoalType::Red => mb_stage::GoalType::Red,
    }
}

const GOALS: [GoalType; 3] = [GoalType::Blue, GoalType::Green, GoalType::Red];

fn assert_same(goal: GoalType, time_left: u32, time_limit: u32) {
    let engine = compute_goal_score(engine_goal(goal), time_left as i32, time_limit as i32);
    let game = Rules::default().goal_score(goal, time_left, time_limit);
    assert_eq!(
        engine, game as i64,
        "{goal:?} with {time_left} of {time_limit} frames left: engine {engine}, course flow {game}"
    );
}

/// The in-stage score (engine) and the score the course keeps (`mb_game::Rules`) for every frame count of
/// every time limit a course uses, and for every limit up to ten minutes at the frames around the rule's
/// edges (zero, the half-time bonus threshold, the full limit).
#[test]
fn goal_score_is_the_same_in_the_engine_and_the_course_flow() {
    let mut limits: Vec<u32> = Difficulty::ALL
        .into_iter()
        .flat_map(mb_game::course::stage_list)
        .map(|e| e.time_limit.unwrap_or(mb_game::stages::DEFAULT_STAGE_TIME))
        .collect();
    limits.push(mb_game::stages::DEFAULT_STAGE_TIME);
    limits.sort_unstable();
    limits.dedup();
    assert!(limits.contains(&3600));
    for &limit in &limits {
        for left in 0..=limit + 120 {
            for goal in GOALS {
                assert_same(goal, left, limit);
            }
        }
    }
    for limit in 0..=36_000u32 {
        let half = limit / 2;
        for left in [
            0,
            1,
            59,
            60,
            half.saturating_sub(1),
            half,
            half + 1,
            limit.saturating_sub(1),
            limit,
            limit + 1,
        ] {
            for goal in GOALS {
                assert_same(goal, left, limit);
            }
        }
    }
}

/// The whole range both sides can express (the engine counts in `i32`): a coarse sweep to the top, where
/// only overflow could make them differ.
#[test]
fn goal_score_agrees_up_to_the_largest_frame_counts() {
    let top = i32::MAX as u32;
    let steps = |n: u32| (0..=n).map(move |i| (top as u64 * i as u64 / n as u64) as u32);
    for left in steps(4096) {
        for limit in steps(64).chain([0, 1, 2, 3600, top - 1, top]) {
            for goal in GOALS {
                assert_same(goal, left, limit);
            }
        }
    }
    for left in [top - 2, top - 1, top] {
        for limit in [0, 1, top / 2, top - 1, top] {
            for goal in GOALS {
                assert_same(goal, left, limit);
            }
        }
    }
    // What the engine takes and the course flow cannot express: negative values, which the gameplay state
    // clamps to zero before reporting (`PlayState::outcome`). The engine treats them the same way.
    for goal in GOALS {
        let e = engine_goal(goal);
        assert_eq!(
            compute_goal_score(e, -5, 3600),
            Rules::default().goal_score(goal, 0, 3600) as i64
        );
        assert_eq!(
            compute_goal_score(e, i32::MIN, 3600),
            compute_goal_score(e, 0, 3600)
        );
        // A non-positive limit means "no limit": no half-time bonus on either side.
        assert_eq!(
            compute_goal_score(e, 100, -1),
            Rules::default().goal_score(goal, 100, 0) as i64
        );
    }
}

/// Both sides agree on which stages are bonus stages and on the default time limit.
#[test]
fn bonus_stages_and_time_limit_agree() {
    for id in 0..=300 {
        assert_eq!(
            mb_engine::SimConfig::first_attempt(id).bonus_stage,
            mb_game::stages::is_bonus_stage(id),
            "stage {id}"
        );
    }
    assert_eq!(
        mb_engine::SimConfig::first_attempt(1).time_limit_frames as u32,
        mb_game::stages::DEFAULT_STAGE_TIME
    );
    assert_eq!(mb_game::run::MAX_SCORE, 999_999_999);
}
