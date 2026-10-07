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
