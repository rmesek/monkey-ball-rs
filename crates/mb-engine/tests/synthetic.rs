//! Simulation behaviour on a hand-built stage (no game data needed): a 20x20 floor, one banana, one goal,
//! optionally an animated group and a bumper.

mod common;

use mb_engine::determinism::QuantizedInput;
use mb_engine::math::Vec3 as V3;
use mb_engine::{Outcome, Phase, Sim, SimConfig};
use mb_stage::*;

fn v(x: f32, y: f32, z: f32) -> mb_stage::Vec3 {
    mb_stage::Vec3 { x, y, z }
}

fn group(triangles: Vec<ColiTri>, cells: Vec<Vec<u16>>) -> AnimGroup {
    AnimGroup {
        init_pos: v(0.0, 0.0, 0.0),
        init_rot: S16Vec { x: 0, y: 0, z: 0 },
        anim_loop_type: 0,
        anim: None,
        model_names: vec![],
        triangles,
        grid: CollisionGrid {
            origin_x: -10.0,
            origin_z: -10.0,
            step_x: 20.0,
            step_z: 20.0,
            cell_count_x: 1,
            cell_count_z: 1,
            cells,
        },
        goals: vec![],
        unk48: None,
        bumpers: vec![],
        jamabars: vec![],
        bananas: vec![],
        cones: vec![],
        spheres: vec![],
        cylinders: vec![],
        models: vec![],
        fallout_boxes: vec![],
        mirror_model_names: vec![],
        unk90: None,
        unk_b8: v(0.0, 0.0, 0.0),
    }
}

fn floor_tris() -> Vec<ColiTri> {
    let r2 = std::f32::consts::FRAC_1_SQRT_2;
    let a = ColiTri {
        pos: v(-10.0, 0.0, 10.0),
        normal: v(0.0, 1.0, 0.0),
        rot: S16Vec {
            x: -0x4000,
            y: 0,
            z: 0,
        },
        flags: 0,
        vert2: Vec2 { x: 20.0, y: 0.0 },
        vert3: Vec2 { x: 0.0, y: 20.0 },
        edge2_normal: Vec2 { x: -r2, y: -r2 },
        edge3_normal: Vec2 { x: 1.0, y: 0.0 },
    };
    let mut b = a;
    b.pos = v(10.0, 0.0, -10.0);
    b.rot = S16Vec {
        x: -0x4000,
        y: -0x8000,
        z: 0,
    };
    vec![a, b]
}

fn stage() -> Stage {
    let mut g0 = group(floor_tris(), vec![vec![0, 1]]);
    g0.goals.push(Goal {
        pos: v(0.0, 0.0, -6.0),
        rot: S16Vec { x: 0, y: 0, z: 0 },
        type_value: 0x42,
        kind: GoalType::Blue,
    });
    g0.bananas.push(Banana {
        pos: v(0.0, 0.5, -3.0),
        kind: 0,
    });
    Stage {
        loop_start_seconds: 0,
        loop_end_seconds: 0,
        anim_groups: vec![g0],
        start_positions: vec![StartPos {
            pos: v(0.0, 0.5, 0.0),
            rot: S16Vec { x: 0, y: 0, z: 0 },
        }],
        fallout_y: -10.0,
        goals: vec![],
        bumpers: vec![],
        jamabars: vec![],
        bananas: vec![],
        cones: vec![],
        spheres: vec![],
        cylinders: vec![],
        models: vec![],
        bg_objects: vec![],
        fg_objects: vec![],
        unk78: None,
        unk7c: 0,
        mirror_names: vec![],
        unk88: None,
        unk90: None,
    }
}

fn run(sim: &mut Sim, ticks: usize, input: QuantizedInput) {
    for _ in 0..ticks {
        sim.step(input);
    }
}

fn intro(sim: &mut Sim) {
    let n = sim.intro_timer;
    run(sim, n as usize, QuantizedInput::default());
}

#[test]
fn intro_then_ball_rests_on_the_floor() {
    let mut sim = Sim::new(&stage(), SimConfig::first_attempt(1));
    assert_eq!(sim.phase, Phase::Intro);
    assert_eq!(sim.bananas_left, 1);
    // input is ignored during the intro
    run(&mut sim, 100, QuantizedInput { x: 127, y: 127 });
    assert_eq!(sim.ball.pos.x, 0.0);
    assert!(sim.ball.flags & mb_engine::constants::ball_flags::INVISIBLE != 0);
    intro(&mut sim);
    assert_eq!(sim.phase, Phase::Playing);
    run(&mut sim, 60, QuantizedInput::default());
    assert!((sim.ball.pos.y - 0.5).abs() < 0.01, "{:?}", sim.ball.pos);
    assert!(sim.ball.vel.x.abs() < 1e-3 && sim.ball.vel.z.abs() < 1e-3);
    assert_eq!(sim.stage_timer_frames, 60);
}

#[test]
fn tilting_rolls_the_ball_in_opposite_directions() {
    let mut a = Sim::new(&stage(), SimConfig::first_attempt(1));
    let mut b = a.clone();
    intro(&mut a);
    intro(&mut b);
    run(&mut a, 40, QuantizedInput { x: 90, y: 0 });
    run(&mut b, 40, QuantizedInput { x: -90, y: 0 });
    assert!(a.ball.pos.x.abs() > 0.2, "{:?}", a.ball.pos);
    assert!(
        (a.ball.pos.x + b.ball.pos.x).abs() < 1e-6,
        "mirror symmetric"
    );
    assert!(a.ball.vel.x * b.ball.vel.x < 0.0);
    assert_ne!(a.hash(), b.hash());
}

#[test]
fn rolling_off_the_edge_falls_out() {
    let mut sim = Sim::new(&stage(), SimConfig::first_attempt(1));
    intro(&mut sim);
    let mut pilot = common::Autopilot {
        targets: vec![V3::new(40.0, 0.0, 25.0)],
        next: 0,
        speed: 0.2,
        gain: 40.0,
    };
    let mut fell = false;
    for _ in 0..1200 {
        let input = pilot.input(&sim);
        sim.step(input);
        if sim.phase == Phase::FallOut {
            fell = true;
            break;
        }
    }
    assert!(fell, "ball never left the floor: {:?}", sim.ball.pos);
    assert!(sim.ball.pos.y < -10.0);
    assert!(sim.ringout_timer > 0);
    let before = sim.tick;
    while !sim.is_finished() {
        sim.step(QuantizedInput::default());
    }
    assert_eq!(sim.phase, Phase::Finished(Outcome::FellOut));
    assert!(sim.tick - before >= 260);
}

#[test]
fn time_over_ends_the_run() {
    let mut cfg = SimConfig::first_attempt(1);
    cfg.time_limit_frames = 100;
    let mut sim = Sim::new(&stage(), cfg);
    intro(&mut sim);
    run(&mut sim, 100, QuantizedInput::default());
    assert_eq!(sim.phase, Phase::TimeOver);
    assert_eq!(sim.ball.vel, V3::ZERO);
    while !sim.is_finished() {
        sim.step(QuantizedInput::default());
    }
    assert_eq!(sim.phase, Phase::Finished(Outcome::TimeOver));
}

#[test]
fn identical_inputs_give_identical_hashes() {
    let inputs: Vec<QuantizedInput> = (0..700)
        .map(|i| QuantizedInput {
            x: ((i as f64 * 0.05).sin() * 60.0) as i32,
            y: ((i as f64 * 0.03).cos() * 60.0) as i32,
        })
        .collect();
    let mut a = Sim::new(&stage(), SimConfig::first_attempt(1));
    let mut b = Sim::new(&stage(), SimConfig::first_attempt(1));
    let mut c = Sim::new(&stage(), SimConfig::first_attempt(1));
    for (i, inp) in inputs.iter().enumerate() {
        a.step(*inp);
        b.step(*inp);
        let mut other = *inp;
        if i > 400 {
            other.x += 10;
        }
        c.step(other);
        assert_eq!(a.hash(), b.hash());
    }
    assert_ne!(a.hash(), c.hash());
}

#[test]
fn banana_is_collected_and_goal_ends_the_run() {
    // Steer through the banana to the goal by trying each stick direction and keeping the first that
    // heads towards -z, then hold it.
    let stage = stage();
    let mut best = None;
    for (x, y) in [(0, 127), (0, -127), (127, 0), (-127, 0)] {
        let mut sim = Sim::new(&stage, SimConfig::first_attempt(1));
        intro(&mut sim);
        run(&mut sim, 30, QuantizedInput { x, y });
        if best.is_none_or(|(_, z)| sim.ball.pos.z < z) {
            best = Some(((x, y), sim.ball.pos.z));
        }
    }
    let ((x, y), z) = best.unwrap();
    assert!(z < -0.3, "no direction moves the ball towards -z ({z})");

    let mut sim = Sim::new(&stage, SimConfig::first_attempt(1));
    intro(&mut sim);
    let mut ticks = 0;
    while !sim.is_finished() && ticks < 900 {
        // gentle push, release once moving to avoid overshooting off the far edge
        let push = if sim.ball.vel.z > -0.12 {
            QuantizedInput { x, y }
        } else {
            QuantizedInput::default()
        };
        sim.step(push);
        ticks += 1;
    }
    assert_eq!(
        sim.phase,
        Phase::Finished(Outcome::Goal(GoalType::Blue)),
        "{:?}",
        sim.ball.pos
    );
    assert_eq!(sim.bananas_left, 0, "banana on the path was collected");
    assert_eq!(sim.ball.bananas, 1);
    assert!(sim.score > 100);
    assert!(
        sim.runtime.goal_bags[0].state != 1,
        "bag opened by the goal"
    );
    assert!(sim.runtime.goal_tapes[0].flags != 0, "tape broken");
}

#[test]
fn animated_group_follows_its_keyframes() {
    let mut s = stage();
    let mut moving = group(vec![], vec![]);
    moving.anim = Some(AnimHeader {
        pos_y: vec![
            Keyframe {
                ease_type: 1,
                time_seconds: 0.0,
                value: 0.0,
                tangent_in: 0.0,
                tangent_out: 0.0,
            },
            Keyframe {
                ease_type: 1,
                time_seconds: 2.0,
                value: 4.0,
                tangent_in: 0.0,
                tangent_out: 0.0,
            },
        ],
        rot_y: vec![
            Keyframe {
                ease_type: 1,
                time_seconds: 0.0,
                value: 0.0,
                tangent_in: 0.0,
                tangent_out: 0.0,
            },
            Keyframe {
                ease_type: 1,
                time_seconds: 2.0,
                value: 90.0,
                tangent_in: 0.0,
                tangent_out: 0.0,
            },
        ],
        ..AnimHeader::default()
    });
    s.anim_groups.push(moving);
    s.loop_start_seconds = 0;
    s.loop_end_seconds = 4;
    let mut sim = Sim::new(&s, SimConfig::first_attempt(1));
    // the stage clock only starts counting during the last 120 intro ticks
    run(&mut sim, 361, QuantizedInput::default());
    assert_eq!(sim.runtime.timer_frames, 120);
    run(&mut sim, 60, QuantizedInput::default());
    let g = &sim.runtime.anim_groups[1];
    // t = 3 s: loop wraps at 4 s so the pose is the end of the 2 s track
    assert!((g.pos.y - 4.0).abs() < 1e-9, "{:?}", g.pos);
    assert_eq!(g.rot[1], 0x4000);
    let m = sim.runtime.anim_groups[1].transform;
    assert!((m[7] as f64 - 4.0).abs() < 1e-6);
}

// Intro fast-forward (`fastForwardIntro`): the primary action held during the fly-in of a first attempt.

const ACTION: u32 = mb_engine::BUTTON_PRIMARY_ACTION;
const IDLE: QuantizedInput = QuantizedInput { x: 0, y: 0 };

/// Everything the fly-in shows: the pose of the camera, and its timer.
fn fly_in(sim: &Sim) -> (V3, V3, [i32; 3], i32) {
    let c = &sim.camera;
    (c.eye, c.look_at, [c.rot_x, c.rot_y, c.rot_z], c.timer_curr)
}

fn ball_is_hidden(sim: &Sim) -> bool {
    sim.ball.flags & mb_engine::constants::ball_flags::INVISIBLE != 0
}

#[test]
fn holding_the_action_runs_the_fly_in_at_twice_the_speed() {
    let mut fast = Sim::new(&stage(), SimConfig::first_attempt(1));
    let mut plain = fast.clone();
    for tick in 1..=120 {
        fast.step_with_buttons(IDLE, ACTION);
        plain.step(IDLE);
        plain.step(IDLE);
        assert_eq!(fast.intro_timer, 360 - 2 * tick);
        // One fast tick shows what two plain ticks show.
        assert_eq!(fast.intro_timer, plain.intro_timer);
        assert_eq!(fly_in(&fast), fly_in(&plain), "tick {tick}");
        assert_eq!(fast.phase, Phase::Intro);
        assert!(fast.events.is_empty());
    }
    assert_eq!((fast.tick, plain.tick), (120, 240));
    assert_eq!(fast.intro_timer, 120);
    // The fast ticks moved nothing but the two timers: the stage clock still waits and the ball is still away.
    assert_eq!(fast.runtime.timer_frames, plain.runtime.timer_frames);
    assert!(ball_is_hidden(&fast));
    assert_eq!(fast.stage_timer_frames, 0);

    // From 120 frames on the action does nothing: tick for tick the same as without it.
    let mut released = fast.clone();
    for _ in 0..200 {
        fast.step_with_buttons(QuantizedInput { x: 40, y: -90 }, ACTION);
        released.step(QuantizedInput { x: 40, y: -90 });
        assert_eq!(fast.intro_timer, released.intro_timer);
        assert_eq!(fast.hash_visual(), released.hash_visual());
        assert_eq!(fly_in(&fast), fly_in(&released));
        assert_eq!(fast.events, released.events);
        assert_eq!(fast.phase, released.phase);
    }
    assert_eq!(fast.phase, Phase::Playing);
    assert_eq!(fast.stage_timer_frames, 80, "play began 240 ticks in");
}

#[test]
fn the_action_does_nothing_at_exactly_120_frames() {
    let mut sim = Sim::new(&stage(), SimConfig::first_attempt(1));
    run(&mut sim, 239, IDLE);
    assert_eq!(sim.intro_timer, 121);
    let mut at_121 = sim.clone();
    at_121.step_with_buttons(IDLE, ACTION);
    assert_eq!(at_121.intro_timer, 119, "more than 120 left: two frames");
    sim.step(IDLE);
    assert_eq!(sim.intro_timer, 120);
    sim.step_with_buttons(IDLE, ACTION);
    assert_eq!(sim.intro_timer, 119, "120 left: one frame");
}

#[test]
fn the_action_does_nothing_on_a_retry_or_when_switched_off() {
    // A retry starts with 120 frames, so it is never above them. A long intro with the fast-forward switched
    // off is the case the reference decides by its attempt counter.
    let mut off = SimConfig::first_attempt(1);
    off.intro_fast_forward = false;
    assert!(SimConfig::first_attempt(1).intro_fast_forward);
    assert!(!SimConfig::retry(1).intro_fast_forward);
    for (cfg, intro_ticks) in [(SimConfig::retry(1), 120), (off, 360)] {
        let mut held = Sim::new(&stage(), cfg);
        let mut plain = held.clone();
        let mut ticks = 0;
        while held.phase == Phase::Intro {
            held.step_with_buttons(IDLE, ACTION);
            plain.step(IDLE);
            ticks += 1;
            assert_eq!(held.intro_timer, plain.intro_timer);
            assert_eq!(held.hash_visual(), plain.hash_visual());
            assert_eq!(fly_in(&held), fly_in(&plain));
        }
        assert_eq!(ticks, intro_ticks);
    }
}

#[test]
fn only_the_primary_action_bit_counts() {
    let mut none = Sim::new(&stage(), SimConfig::first_attempt(1));
    let mut zero = none.clone();
    let mut others = none.clone();
    let mut all = none.clone();
    let mut action = none.clone();
    for i in 0..500 {
        let input = QuantizedInput {
            x: (i % 9) - 4,
            y: 3,
        };
        none.step(input);
        zero.step_with_buttons(input, 0);
        others.step_with_buttons(input, !ACTION);
        all.step_with_buttons(input, u32::MAX);
        action.step_with_buttons(input, ACTION);
        assert_eq!(none.hash_visual(), zero.hash_visual());
        assert_eq!(none.hash_visual(), others.hash_visual());
        assert_eq!(fly_in(&none), fly_in(&others));
        assert_eq!(all.hash_visual(), action.hash_visual());
        assert_eq!(fly_in(&all), fly_in(&action));
    }
    assert_eq!(none.stage_timer_frames, 140);
    assert_eq!(action.stage_timer_frames, 260);
}

/// The ball drop (`introTimerFrames === dropFrames`) and the end of the intro (`=== 0`) are tests for
/// equality on a timer that can go down by two. Neither is ever stepped over: a tick fast-forwards only when
/// it starts above 120, so the lowest frame a double step can reach is 119, and the one it can pass is 120.
#[test]
fn a_fast_forwarded_intro_still_drops_the_ball_at_24_frames_and_ends_at_0() {
    // Holding from the first tick keeps the timer even (.., 122, 120); from the second it is odd and goes
    // 121 -> 119. `taps` presses and releases at awkward moments.
    let from_start = |_tick: usize| true;
    let from_second = |tick: usize| tick >= 1;
    let taps = |tick: usize| tick.is_multiple_of(3) || (tick > 100 && tick % 7 < 4);
    let cases: [&dyn Fn(usize) -> bool; 3] = [&from_start, &from_second, &taps];
    let mut passed_120 = Vec::new();
    for (case, press) in cases.into_iter().enumerate() {
        let mut sim = Sim::new(&stage(), SimConfig::first_attempt(1));
        let mut timers = vec![sim.intro_timer];
        let mut dropped_at = None;
        let mut tick = 0;
        while sim.phase == Phase::Intro {
            let hidden = ball_is_hidden(&sim);
            sim.step_with_buttons(IDLE, press(tick) as u32);
            tick += 1;
            timers.push(sim.intro_timer);
            if hidden && !ball_is_hidden(&sim) {
                dropped_at = Some(sim.intro_timer);
            }
            // The fly-in ends 11 frames before the intro whatever was skipped: both timers skip together.
            if sim.intro_timer > 0 {
                assert_eq!(sim.camera.timer_curr, (sim.intro_timer - 11).max(0));
            }
            assert!(tick <= 360);
        }
        assert_eq!(dropped_at, Some(24), "case {case}");
        assert_eq!(sim.intro_timer, 0);
        assert_eq!(sim.phase, Phase::Playing);
        assert_eq!(
            sim.camera.state,
            mb_engine::constants::camera_state::LEVEL_MAIN,
            "the camera snapped behind the ball when the timer reached 0"
        );
        // 120 itself is only ever missed from 121.
        let missed = !timers.contains(&120);
        assert_eq!(timers.windows(2).any(|w| w == [121, 119]), missed);
        passed_120.push(missed);
        // Below 120 every frame is visited, one per tick.
        assert!(timers.ends_with(&(0..=119).rev().collect::<Vec<_>>()));
        // READY, as the reference and the HUD test it (`prev > 120 && now <= 120`), happens exactly once.
        let ready = timers.windows(2).filter(|w| w[0] > 120 && w[1] <= 120);
        assert_eq!(ready.count(), 1);

        // Whatever was skipped, play starts from the same stage clock, ball and camera.
        let mut plain = Sim::new(&stage(), SimConfig::first_attempt(1));
        intro(&mut plain);
        assert!(tick < plain.tick as usize);
        assert_eq!(sim.runtime.timer_frames, plain.runtime.timer_frames);
        assert_eq!(sim.ball.pos, plain.ball.pos);
        assert_eq!(sim.ball.vel, plain.ball.vel);
        assert_eq!(fly_in(&sim), fly_in(&plain));
    }
    assert_eq!(passed_120[..2], [false, true]);
}

/// The fly-in camera guards its timer only before the first of its two steps (`if (timerCurr > 0) { -1; if
/// (fast) -1 }`), so a fly-in that stands at 1 on a fast-forwarded tick ends at -1 and stays there. The two
/// configurations of the game (360 / 350 and 120 / 90) cannot get there, because the fly-in is over 11 frames
/// before the intro; a configuration with a fly-in much shorter than its intro does, as in the reference.
#[test]
fn a_fly_in_much_shorter_than_the_intro_can_overshoot_by_one() {
    let mut cfg = SimConfig::first_attempt(1);
    cfg.fly_in_frames = 4;
    let mut sim = Sim::new(&stage(), cfg);
    assert_eq!(sim.camera.timer_curr, 3, "the first step is taken at once");
    sim.step_with_buttons(IDLE, ACTION);
    assert_eq!(sim.camera.timer_curr, 1);
    sim.step_with_buttons(IDLE, ACTION);
    assert_eq!(sim.camera.timer_curr, -1);
    let overshot = fly_in(&sim);
    run(&mut sim, 300, IDLE);
    assert_eq!(fly_in(&sim), overshot, "and there it stays until GO");
    assert!(overshot.0.x.is_finite() && overshot.0.y.is_finite() && overshot.0.z.is_finite());
}

// The primary action after the intro (`updateRingout`, `canSkipGoal`): it ends a fall-out sequence from its
// 60th tick and does nothing anywhere else.

/// A retry of the stage with the ball started beside the floor, stepped (with `buttons` held) to the tick on
/// which it fell out.
fn fallen(cfg: SimConfig, buttons: u32) -> Sim {
    let mut s = stage();
    s.start_positions[0].pos = v(30.0, 0.5, 0.0);
    let mut sim = Sim::new(&s, cfg);
    while sim.phase != Phase::FallOut {
        assert!(sim.tick < 600, "the ball never fell out");
        sim.step_with_buttons(IDLE, buttons);
    }
    assert!(sim.events.contains(&mb_engine::SimEvent::FallOut));
    sim
}

/// Ticks from the fall to the end of the sequence, with the action down on the ticks `press` names (1 is the
/// first tick after the one on which the ball fell out).
fn fall_out_ticks(mut sim: Sim, press: &dyn Fn(u32) -> bool) -> u32 {
    let mut plain = sim.clone();
    let mut ticks = 0;
    while !sim.is_finished() {
        ticks += 1;
        sim.step_with_buttons(IDLE, press(ticks) as u32);
        plain.step(IDLE);
        if !sim.is_finished() {
            // Until it ends the sequence the action changes nothing.
            assert_eq!(sim.hash_visual(), plain.hash_visual(), "tick {ticks}");
            assert_eq!(fly_in(&sim), fly_in(&plain), "tick {ticks}");
            assert_eq!(sim.ringout_timer, plain.ringout_timer);
            assert_eq!(sim.ringout_skip_timer, plain.ringout_skip_timer);
            assert_eq!(sim.ringout_timer, 270 - ticks as i32);
            assert_eq!(sim.ringout_skip_timer, (60 - ticks as i32).max(0));
        }
        assert!(ticks <= 270);
    }
    assert_eq!(sim.phase, Phase::Finished(Outcome::FellOut));
    assert_eq!((sim.ringout_timer, sim.ringout_skip_timer), (0, 0));
    assert!(sim.events.is_empty());
    ticks
}

#[test]
fn the_action_ends_a_fall_out_from_its_60th_tick() {
    let sim = fallen(SimConfig::retry(1), 0);
    assert_eq!((sim.ringout_timer, sim.ringout_skip_timer), (270, 60));
    assert_eq!(
        fall_out_ticks(sim.clone(), &|_| false),
        270,
        "without the action"
    );
    assert_eq!(
        fall_out_ticks(sim.clone(), &|_| true),
        60,
        "held throughout"
    );
    // Down for the first 59 ticks only: too early, the sequence runs to its end.
    assert_eq!(fall_out_ticks(sim.clone(), &|tick| tick < 60), 270);
    // One tick, anywhere from the 60th on, is the sequence's last.
    for at in [60, 61, 100, 240, 269] {
        assert_eq!(fall_out_ticks(sim.clone(), &|tick| tick == at), at);
    }
    assert_eq!(fall_out_ticks(sim.clone(), &|tick| tick == 270), 270);
    // Pressed too early, let go, pressed again: the delay is not started again.
    assert_eq!(
        fall_out_ticks(sim.clone(), &|tick| tick == 10 || tick == 59 || tick >= 90),
        90
    );
    // Only bit 0 is the action.
    let mut other = sim.clone();
    for _ in 0..269 {
        other.step_with_buttons(IDLE, !ACTION);
        assert_eq!(other.phase, Phase::FallOut);
    }
}

/// The action is a level, not a press: one that is already down when the ball falls out, even since the
/// intro, ends the sequence on its 60th tick like one pressed then.
#[test]
fn an_action_held_since_before_the_fall_counts() {
    for cfg in [SimConfig::retry(1), SimConfig::first_attempt(1)] {
        let plain = fallen(cfg.clone(), 0);
        let held = fallen(cfg.clone(), ACTION);
        // During play the action did nothing; on a first attempt it shortened the fly-in by 120 ticks.
        let skipped = if cfg.intro_fast_forward { 120 } else { 0 };
        assert_eq!(held.tick + skipped, plain.tick);
        // (The banana has spun 120 ticks less after a shortened fly-in, so only a retry hashes the same.)
        if skipped == 0 {
            assert_eq!(held.hash_visual(), plain.hash_visual());
        }
        assert_eq!(
            (held.ball.pos, held.ball.vel),
            (plain.ball.pos, plain.ball.vel)
        );
        assert_eq!(fly_in(&held), fly_in(&plain));
        assert_eq!(held.stage_timer_frames, plain.stage_timer_frames);
        assert_eq!((held.ringout_timer, held.ringout_skip_timer), (270, 60));
        assert_eq!(fall_out_ticks(held, &|_| true), 60);
    }
}

#[test]
fn the_action_does_not_end_a_fall_out_on_a_bonus_stage() {
    let mut cfg = SimConfig::retry(1);
    cfg.bonus_stage = true;
    let sim = fallen(cfg, ACTION);
    // No delay is set, and none is needed: the rule itself excludes bonus stages.
    assert_eq!((sim.ringout_timer, sim.ringout_skip_timer), (120, 0));
    let (mut held, mut plain) = (sim.clone(), sim);
    for tick in 1..=120 {
        assert_eq!(held.phase, Phase::FallOut, "tick {tick}");
        held.step_with_buttons(IDLE, ACTION);
        plain.step(IDLE);
        assert_eq!(held.hash_visual(), plain.hash_visual());
        assert_eq!(held.phase, plain.phase);
        assert_eq!(held.ringout_skip_timer, 0);
    }
    assert_eq!(held.phase, Phase::Finished(Outcome::FellOut));
}

/// Two simulations of the same run, one with the action down on every tick `press` names, stepped until both
/// are finished: the same on every tick. Returns the tick each sequence began and the one the run ended on.
fn with_and_without_action(
    stage: &Stage,
    cfg: SimConfig,
    steer: &dyn Fn(&Sim) -> QuantizedInput,
    press: &dyn Fn(&Sim) -> bool,
) -> (Sim, u32) {
    let mut held = Sim::new(stage, cfg);
    let mut plain = held.clone();
    let mut sequence_began = 0;
    while !plain.is_finished() {
        assert!(plain.tick < 2000, "the run never ended");
        let input = steer(&plain);
        let was_playing = plain.phase == Phase::Playing;
        held.step_with_buttons(input, press(&plain) as u32);
        plain.step(input);
        if was_playing && plain.phase != Phase::Playing {
            sequence_began = plain.tick;
        }
        assert_eq!(held.phase, plain.phase, "tick {}", plain.tick);
        assert_eq!(held.hash_visual(), plain.hash_visual());
        assert_eq!(fly_in(&held), fly_in(&plain));
        assert_eq!(held.events, plain.events);
        assert_eq!(
            (
                held.goal_timer,
                held.timeover_timer,
                held.stage_timer_frames
            ),
            (
                plain.goal_timer,
                plain.timeover_timer,
                plain.stage_timer_frames
            )
        );
        assert_eq!(held.score, plain.score);
    }
    (held, plain.tick - sequence_began)
}

/// The stick direction that rolls the ball towards the banana and the goal (-z), found by trying each.
fn towards_the_goal(stage: &Stage) -> QuantizedInput {
    [(0, 127), (0, -127), (127, 0), (-127, 0)]
        .map(|(x, y)| {
            let mut sim = Sim::new(stage, SimConfig::retry(1));
            intro(&mut sim);
            run(&mut sim, 30, QuantizedInput { x, y });
            (QuantizedInput { x, y }, sim.ball.pos.z)
        })
        .into_iter()
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap()
        .0
}

/// The reference would let the action leave a goal sequence once the ball has launched and its
/// `goalSkipTimerFrames` (210) has run down, but the sequence is over after 151 ticks (a bonus clear after
/// 180), so without a result replay that never happens: the action does nothing there, whenever it is down.
#[test]
fn the_action_does_nothing_in_a_goal_sequence_or_a_bonus_clear() {
    let stage = stage();
    let push = towards_the_goal(&stage);
    let steer = |sim: &Sim| {
        if sim.phase == Phase::Playing && sim.ball.vel.z > -0.12 {
            push
        } else {
            IDLE
        }
    };
    let mut bonus = SimConfig::retry(1);
    bonus.bonus_stage = true;
    let goal = Outcome::Goal(GoalType::Blue);
    for (cfg, outcome, length) in [
        (SimConfig::retry(1), goal, 151),
        (SimConfig::first_attempt(1), goal, 151),
        (bonus, Outcome::BonusClear, 180),
    ] {
        let always = |_: &Sim| true;
        let in_sequence = |sim: &Sim| sim.phase == Phase::GoalSequence;
        let before_launch = |sim: &Sim| sim.ball.goal_timer < 90;
        let after_launch = |sim: &Sim| sim.ball.goal_timer >= 90;
        let taps = |sim: &Sim| sim.tick.is_multiple_of(3);
        let presses: [&dyn Fn(&Sim) -> bool; 5] =
            [&always, &in_sequence, &before_launch, &after_launch, &taps];
        for (case, press) in presses.into_iter().enumerate() {
            // The fly-in of a first attempt is the one thing an action held throughout would change.
            if cfg.intro_fast_forward && case != 1 {
                continue;
            }
            let (sim, took) = with_and_without_action(&stage, cfg.clone(), &steer, press);
            assert_eq!(sim.phase, Phase::Finished(outcome), "case {case}");
            assert_eq!(took, length, "case {case}");
        }
    }
}

#[test]
fn the_action_does_nothing_in_play_or_during_time_over() {
    let mut cfg = SimConfig::retry(1);
    cfg.time_limit_frames = 200;
    let steer = |sim: &Sim| QuantizedInput {
        x: (sim.tick % 40) as i32 - 20,
        y: 15,
    };
    let always = |_: &Sim| true;
    let in_sequence = |sim: &Sim| sim.phase == Phase::TimeOver;
    let taps = |sim: &Sim| sim.tick % 7 < 2;
    let presses: [&dyn Fn(&Sim) -> bool; 3] = [&always, &in_sequence, &taps];
    for press in presses {
        let (sim, took) = with_and_without_action(&stage(), cfg.clone(), &steer, press);
        assert_eq!(sim.phase, Phase::Finished(Outcome::TimeOver));
        assert_eq!(took, 120);
        assert_eq!(sim.stage_timer_frames, 200);
    }
}
