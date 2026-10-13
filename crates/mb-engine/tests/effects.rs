//! The display-only particles (`runtime.effects`, `runtime.confetti`) on a hand-built stage, no game data
//! needed: when they spawn, how long they live, that the pools never grow, and that the simulation is the
//! same with and without them. Bit-for-bit agreement with the reference is `tests/parity.rs`.

use mb_engine::determinism::QuantizedInput;
use mb_engine::effects::{CONFETTI_MODEL_COUNT, CONFETTI_PER_GOAL, MAX_BALL_EFFECTS};
use mb_engine::math::Vec3 as V3;
use mb_engine::{BallEffectKind, Outcome, Phase, Sim, SimConfig};
use mb_stage::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    /// Heap allocations (and growths) made by this thread.
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: every call is passed straight to the system allocator; the counter is a plain thread-local cell
// without a destructor, so touching it allocates nothing.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations() -> usize {
    ALLOCATIONS.with(|n| n.get())
}

fn v(x: f32, y: f32, z: f32) -> mb_stage::Vec3 {
    mb_stage::Vec3 { x, y, z }
}

/// A 20 x 20 floor at y = 0 with the start in the middle and a goal 6 units in front of it.
fn stage() -> Stage {
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
    let group = AnimGroup {
        init_pos: v(0.0, 0.0, 0.0),
        init_rot: S16Vec { x: 0, y: 0, z: 0 },
        anim_loop_type: 0,
        anim: None,
        model_names: vec![],
        triangles: vec![a, b],
        grid: CollisionGrid {
            origin_x: -10.0,
            origin_z: -10.0,
            step_x: 20.0,
            step_z: 20.0,
            cell_count_x: 1,
            cell_count_z: 1,
            cells: vec![vec![0, 1]],
        },
        goals: vec![Goal {
            pos: v(0.0, 0.0, -6.0),
            rot: S16Vec { x: 0, y: 0, z: 0 },
            type_value: 0x42,
            kind: GoalType::Blue,
        }],
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
    };
    Stage {
        loop_start_seconds: 0,
        loop_end_seconds: 0,
        anim_groups: vec![group],
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

const IDLE: QuantizedInput = QuantizedInput { x: 0, y: 0 };

/// A sim whose ball lies still on the floor with no particle left from the drop.
fn at_rest(cfg: SimConfig) -> Sim {
    let mut sim = Sim::new(&stage(), cfg);
    while sim.phase == Phase::Intro {
        sim.step(IDLE);
    }
    for _ in 0..200 {
        sim.step(IDLE);
    }
    assert_eq!(sim.phase, Phase::Playing);
    assert!(
        sim.runtime.effects.is_empty(),
        "the drop's effects are gone"
    );
    assert!(sim.ball.vel.x.abs() < 1e-3 && sim.ball.vel.y.abs() < 0.02);
    sim
}

fn count(sim: &Sim, kind: BallEffectKind) -> usize {
    sim.runtime
        .effects
        .iter()
        .filter(|e| e.kind == kind)
        .count()
}

/// The stick direction that rolls the ball towards the goal (-z).
fn towards_goal() -> QuantizedInput {
    let mut best = (IDLE, f64::INFINITY);
    for (x, y) in [(0, 127), (0, -127), (127, 0), (-127, 0)] {
        let mut sim = at_rest(SimConfig::retry(1));
        for _ in 0..30 {
            sim.step(QuantizedInput { x, y });
        }
        if sim.ball.pos.z < best.1 {
            best = (QuantizedInput { x, y }, sim.ball.pos.z);
        }
    }
    assert!(
        best.1 < -0.3,
        "no direction moves the ball towards the goal"
    );
    best.0
}

/// Steps until the ball is in the goal; returns the tick input used.
fn roll_into_goal(sim: &mut Sim, push: QuantizedInput) {
    for _ in 0..900 {
        let input = if sim.ball.vel.z > -0.12 { push } else { IDLE };
        sim.step(input);
        if sim.phase != Phase::Playing {
            break;
        }
    }
    assert_eq!(sim.phase, Phase::GoalSequence, "{:?}", sim.ball.pos);
}

#[test]
fn the_visual_random_stream_is_seeded_from_the_stage_id() {
    for id in [1u32, 2, 95, 200] {
        let sim = Sim::new(&stage(), SimConfig::first_attempt(id));
        assert_eq!(sim.runtime.visual_rng.state, id ^ 0x9e37_79b9);
        assert_eq!(sim.runtime.sim_rng.state, id);
        assert!(sim.runtime.effects.is_empty() && sim.runtime.confetti.is_empty());
    }
    // The one id that would seed the generator with zero takes the generator's own fallback.
    let sim = Sim::new(&stage(), SimConfig::first_attempt(0x9e37_79b9));
    assert_eq!(sim.runtime.visual_rng.state, 0x6d2b_79f5);
}

#[test]
fn sparks_need_a_fast_roll() {
    // `intensity = speed * 5` must exceed 1.5, and `trunc(intensity)` sparks are thrown per tick.
    for (speed, per_tick) in [(0.25, 0), (0.295, 0), (0.32, 1), (0.5, 2), (0.9, 4)] {
        let mut sim = at_rest(SimConfig::retry(1));
        sim.ball.vel = V3::new(speed, 0.0, 0.0);
        sim.step(IDLE);
        assert_eq!(
            count(&sim, BallEffectKind::Coli),
            per_tick,
            "rolling at {speed}"
        );
        assert_eq!(sim.runtime.effects.len(), per_tick, "sparks only");
        for e in &sim.runtime.effects {
            assert_eq!(e.age, 0);
            assert!((15..=59).contains(&e.life), "life {}", e.life);
            assert_eq!((e.color_r, e.color_g, e.color_b), (1.1, 1.0, 0.6));
            // Thrown from the contact point under the ball.
            assert!(e.pos.y.abs() < 1e-6, "{:?}", e.pos);
            assert_eq!(e.pos, e.prev_pos);
        }
    }
    // In the air the same speed throws nothing.
    let mut sim = at_rest(SimConfig::retry(1));
    sim.ball.pos.y = 3.0;
    sim.ball.vel = V3::new(0.9, 0.0, 0.0);
    sim.step(IDLE);
    assert!(sim.runtime.effects.is_empty());
}

#[test]
fn a_hard_landing_throws_a_flash_stars_and_sparks() {
    // Below 0.06 into the surface: nothing.
    let mut sim = at_rest(SimConfig::retry(1));
    sim.ball.vel = V3::new(0.0, -0.045, 0.0);
    sim.step(IDLE);
    assert!(sim.ball.audio.last_coli_speed > -0.06);
    assert!(sim.runtime.effects.is_empty());

    for fall in [0.08, 0.2, 0.4, 3.0] {
        let mut sim = at_rest(SimConfig::retry(1));
        sim.ball.vel = V3::new(0.0, -fall, 0.0);
        sim.step(IDLE);
        let hit = sim.ball.audio.last_coli_speed;
        assert!(hit < -0.06, "{hit}");
        let burst = ((hit / 0.0165).abs().trunc() as usize).min(32);
        assert_eq!(count(&sim, BallEffectKind::ColiFlash), 1, "fall {fall}");
        assert_eq!(count(&sim, BallEffectKind::ColiStar), burst / 2);
        // The bounce may be fast enough to throw rolling sparks on top of the burst's own.
        let rolling = {
            let s = sim.ball.vel;
            let speed = (s.x * s.x + s.y * s.y + s.z * s.z).sqrt() * 5.0;
            if speed > 1.5 { speed as usize } else { 0 }
        };
        let sparks = count(&sim, BallEffectKind::Coli);
        assert!(
            sparks >= burst - burst / 2 && sparks <= burst - burst / 2 + rolling + 1,
            "fall {fall}: {sparks} sparks for a burst of {burst} and {rolling} rolling"
        );
        let flash = sim
            .runtime
            .effects
            .iter()
            .find(|e| e.kind == BallEffectKind::ColiFlash)
            .unwrap();
        assert_eq!(flash.life, 12);
        // The flash lies on the floor: the contact direction points down into it.
        assert!(flash.surface_normal.y < -0.99, "{:?}", flash.surface_normal);
        assert!(flash.scale > 0.0 && flash.scale_target == flash.scale * 4.0);
        for star in sim
            .runtime
            .effects
            .iter()
            .filter(|e| e.kind == BallEffectKind::ColiStar)
        {
            assert_eq!((star.scale, star.scale_target), (0.0, 0.015));
            assert!((0x1000..0x2000).contains(&star.rot_vel_y));
            assert!(star.vel.y > 0.0, "stars fly away from the floor");
        }
    }
    // The hardest landing reaches the cap of 16 stars.
    let mut sim = at_rest(SimConfig::retry(1));
    sim.ball.vel = V3::new(0.0, -3.0, 0.0);
    sim.step(IDLE);
    assert_eq!(count(&sim, BallEffectKind::ColiStar), 16);
}

#[test]
fn effects_live_exactly_their_life_and_keep_their_order() {
    let mut sim = at_rest(SimConfig::retry(1));
    sim.ball.vel = V3::new(0.0, -0.5, 0.0);
    sim.step(IDLE);
    // (id, tick of death) for everything the landing threw.
    let born: Vec<(u32, i32)> = sim.runtime.effects.iter().map(|e| (e.id, e.life)).collect();
    assert!(born.len() > 20);
    assert!(
        born.windows(2).all(|w| w[0].0 < w[1].0),
        "ids count up in spawn order"
    );
    let mut found_ground = false;
    let mut faded = false;
    for tick in 1..=60 {
        sim.step(IDLE);
        let alive: Vec<u32> = sim.runtime.effects.iter().map(|e| e.id).collect();
        let expected: Vec<u32> = born
            .iter()
            .filter(|(_, life)| *life > tick)
            .map(|(id, _)| *id)
            .collect();
        let old: Vec<u32> = alive
            .iter()
            .copied()
            .filter(|id| born.iter().any(|b| b.0 == *id))
            .collect();
        assert_eq!(
            old, expected,
            "tick {tick}: survivors of the landing, in spawn order"
        );
        for e in &sim.runtime.effects {
            if let Some((_, life)) = born.iter().find(|b| b.0 == e.id) {
                assert_eq!(e.age, tick);
                assert_eq!(e.life, life - tick);
                assert!((0.0..=1.0).contains(&e.alpha));
                assert!(e.life_ratio() > 0.0 && e.life_ratio() < 1.0);
                found_ground |= e.has_ground && e.glow_dist > 0.0;
                faded |= e.alpha < 1.0;
            }
        }
    }
    assert!(found_ground, "sparks near the floor have a glow");
    assert!(faded, "effects fade towards their end");
    assert!(
        sim.runtime
            .effects
            .iter()
            .all(|e| born.iter().all(|b| b.0 != e.id)),
        "nothing of the landing outlives 59 ticks"
    );
}

#[test]
fn the_effect_pool_is_full_at_its_capacity_and_never_grows() {
    for speed in [50.0, 1.0e6, 1.0e300, f64::INFINITY] {
        let mut sim = at_rest(SimConfig::retry(1));
        let buffer = sim.runtime.effects.as_ptr();
        assert_eq!(sim.runtime.effects.capacity(), MAX_BALL_EFFECTS);
        // Hitting the floor at an absurd speed leaves the ball on it at half that speed, which asks for
        // millions of sparks in one tick (an endless loop in the reference once the speed is infinite).
        sim.ball.vel = V3::new(0.0, -speed, 0.0);
        sim.step(IDLE);
        let bounce = sim.ball.vel.y;
        if bounce.is_finite() {
            // The burst of the landing (a flash, 16 stars, 16 sparks) and five sparks per unit of speed.
            let wanted = 33.0 + (bounce * 5.0).trunc();
            let expected = wanted.min(MAX_BALL_EFFECTS as f64) as usize;
            assert_eq!(sim.runtime.effects.len(), expected, "speed {speed}");
            assert_eq!(expected == MAX_BALL_EFFECTS, speed > 50.0);
        }
        // More of the same finds the pool full.
        for _ in 0..5 {
            sim.ball.pos = V3::new(0.0, 0.5, 0.0);
            sim.ball.vel = V3::new(0.0, -speed, 0.0);
            sim.step(IDLE);
            assert!(sim.runtime.effects.len() <= MAX_BALL_EFFECTS);
        }
        assert_eq!(sim.runtime.effects.capacity(), MAX_BALL_EFFECTS);
        assert_eq!(
            sim.runtime.effects.as_ptr(),
            buffer,
            "the pool was not moved"
        );
        // The pool drains again: nothing lives longer than 59 ticks.
        sim.ball.pos = V3::new(0.0, 0.5, 0.0);
        sim.ball.vel = V3::ZERO;
        for _ in 0..60 {
            if sim.is_finished() {
                break;
            }
            sim.ball.pos = V3::new(0.0, 0.5, 0.0);
            sim.ball.vel = V3::ZERO;
            sim.step(IDLE);
        }
        assert!(sim.runtime.effects.is_empty() || sim.is_finished());
    }
}

#[test]
fn a_goal_throws_confetti_once_and_sparkles_while_the_ball_floats() {
    let push = towards_goal();
    let mut sim = at_rest(SimConfig::retry(7));
    let buffer = sim.runtime.confetti.as_ptr();
    assert!(sim.runtime.confetti.capacity() >= CONFETTI_PER_GOAL);
    roll_into_goal(&mut sim, push);

    let confetti = &sim.runtime.confetti;
    assert_eq!(confetti.len(), CONFETTI_PER_GOAL);
    assert_eq!(confetti.as_ptr(), buffer, "the confetti pool was not moved");
    for (i, c) in confetti.iter().enumerate() {
        assert_eq!(c.model_index, i % CONFETTI_MODEL_COUNT);
        assert!((210..=269).contains(&c.life), "life {}", c.life);
        assert!((0.5..1.0).contains(&c.scale));
        assert!((0.0..0.0001).contains(&c.ground_bias));
        assert_eq!(c.pos, c.prev_pos);
        // Thrown from the bag, which hangs above the goal.
        assert!(c.pos.y > 1.0 && (c.pos.z + 6.0).abs() < 1.5, "{:?}", c.pos);
    }
    // Opening the same bag again throws nothing more.
    let ball = sim.ball.phys_ball.clone();
    sim.runtime.goal_bags[0].flags = 0;
    sim.runtime.open_goal_bag(0, &ball);
    assert_eq!(sim.runtime.confetti.len(), CONFETTI_PER_GOAL);

    // A sparkle every other tick while the ball is in its goal state, each living 60 to 74 ticks.
    let mut sparkles = 0;
    let mut landed = false;
    let mut ticks = 0;
    while !sim.is_finished() {
        let before: Vec<u32> = sim.runtime.effects.iter().map(|e| e.id).collect();
        let odd = sim.ball.unk80 & 1 != 0;
        sim.step(IDLE);
        ticks += 1;
        let new: Vec<_> = sim
            .runtime
            .effects
            .iter()
            .filter(|e| e.kind == BallEffectKind::Levitate && !before.contains(&e.id))
            .collect();
        if sim.is_finished() {
            break;
        }
        assert_eq!(new.len(), odd as usize, "tick {ticks}");
        for s in new {
            sparkles += 1;
            assert!((60..=74).contains(&s.life));
            assert_eq!(s.scale, 0.24);
            let d = V3::new(
                s.pos.x - sim.ball.pos.x,
                s.pos.y - sim.ball.pos.y,
                s.pos.z - sim.ball.pos.z,
            );
            assert!(d.x.abs() <= 0.35 && d.y.abs() <= 0.35 && d.z.abs() <= 0.35);
        }
        landed |= sim.runtime.confetti.iter().any(|c| c.on_ground);
        assert_eq!(sim.runtime.confetti.len(), CONFETTI_PER_GOAL);
    }
    assert!(matches!(sim.phase, Phase::Finished(Outcome::Goal(_))));
    assert!(sparkles > 50, "{sparkles} sparkles in {ticks} ticks");
    assert!(
        landed,
        "confetti reaches the floor during the goal sequence"
    );

    // The goal sequence is shorter than the confetti's life; on its own the stage runtime sees it out.
    let before = sim.hash();
    let gravity = sim.shared_world.gravity;
    let mut last = sim.runtime.confetti.len();
    for _ in 0..270 {
        sim.runtime.advance(&sim.geom, gravity, None, None, None);
        assert!(sim.runtime.confetti.len() <= last);
        last = sim.runtime.confetti.len();
        for c in &sim.runtime.confetti {
            assert!(c.pos.y > -0.01, "confetti does not fall through the floor");
        }
    }
    assert!(sim.runtime.confetti.is_empty() && sim.runtime.effects.is_empty());
    assert_eq!(sim.runtime.confetti.as_ptr(), buffer);
    assert_ne!(before, sim.hash(), "the stage itself went on");
}

/// Everything the simulation is, as numbers that must not depend on the particles.
fn outcome(sim: &Sim) -> (u32, [u64; 6], i32, u32, i32, i32, i32, Phase, i64) {
    let b = &sim.ball;
    (
        sim.hash(),
        [b.pos.x, b.pos.y, b.pos.z, b.vel.x, b.vel.y, b.vel.z].map(f64::to_bits),
        b.state,
        b.flags,
        sim.camera.rot_y,
        sim.stage_timer_frames,
        sim.goal_timer,
        sim.phase,
        sim.score,
    )
}

#[test]
fn particles_never_change_the_simulation() {
    let push = towards_goal();
    let mut off_cfg = SimConfig::first_attempt(3);
    off_cfg.visual_effects = false;
    let mut on = Sim::new(&stage(), SimConfig::first_attempt(3));
    let mut off = Sim::new(&stage(), off_cfg);
    let mut twin = Sim::new(&stage(), SimConfig::first_attempt(3));
    let (mut had_effects, mut had_confetti, mut differed) = (false, false, false);
    for tick in 0..2000 {
        // Two hard landings (the same in every sim), then into the goal.
        let input = if on.phase != Phase::Playing || tick < 700 || on.ball.vel.z <= -0.12 {
            IDLE
        } else {
            push
        };
        if matches!(tick, 450 | 520 | 700) {
            for sim in [&mut on, &mut off, &mut twin] {
                sim.ball.pos = V3::new(0.0, 0.5, 0.0);
                sim.ball.vel = V3::new(0.0, if tick == 700 { 0.0 } else { -0.6 }, 0.0);
            }
        }
        on.step(input);
        off.step(input);
        twin.step(input);
        assert_eq!(outcome(&on), outcome(&off), "tick {tick}");
        assert_eq!(on.events, off.events, "tick {tick}");
        // Same inputs, same particles.
        assert_eq!(on.hash_visual(), twin.hash_visual(), "tick {tick}");
        assert_eq!(on.runtime.effects, twin.runtime.effects);
        assert_eq!(on.runtime.confetti, twin.runtime.confetti);
        assert_eq!(on.runtime.visual_rng, twin.runtime.visual_rng);
        assert!(
            off.runtime.effects.is_empty(),
            "no effects when switched off"
        );
        had_effects |= !on.runtime.effects.is_empty();
        had_confetti |= !on.runtime.confetti.is_empty();
        differed |= on.hash_visual() != off.hash_visual();
        if on.runtime.effects.is_empty() && on.runtime.confetti.is_empty() {
            // Nothing alive: the visual hash is the plain state plus two zero counts.
            assert_eq!(on.hash_visual(), off.hash_visual(), "tick {tick}");
        }
        assert_ne!(on.hash(), on.hash_visual(), "the two hashes are distinct");
        if on.is_finished() {
            break;
        }
    }
    assert!(matches!(on.phase, Phase::Finished(Outcome::Goal(_))));
    assert!(had_effects && had_confetti && differed);
    // As in the reference, a bag that opens throws its confetti even when visuals are off; it is then
    // never moved.
    assert_eq!(off.runtime.confetti.len(), CONFETTI_PER_GOAL);
    assert!(off.runtime.confetti.iter().all(|c| c.pos == c.prev_pos));
}

#[test]
fn a_retry_starts_without_particles_and_repeats_them_exactly() {
    let run = |cfg: SimConfig| {
        let mut sim = at_rest(cfg);
        let mut hashes = Vec::new();
        for tick in 0..150 {
            if tick % 40 == 0 {
                sim.ball.pos = V3::new(0.0, 0.5, 0.0);
                sim.ball.vel = V3::new(0.4, -0.3, 0.0);
            }
            sim.step(IDLE);
            hashes.push((sim.hash_visual(), sim.runtime.effects.len()));
        }
        (sim, hashes)
    };
    let (first, a) = run(SimConfig::retry(5));
    assert!(!first.runtime.effects.is_empty());
    assert_ne!(first.runtime.visual_rng.state, 5 ^ 0x9e37_79b9);
    // The next attempt is a new `Sim`, as it is a new `StageRuntime` in the reference: empty pools, the
    // random stream back at its seed, and so the same particles for the same play.
    let (_, b) = run(SimConfig::retry(5));
    assert_eq!(a, b);
    // Another stage id draws from another stream.
    let (_, c) = run(SimConfig::retry(6));
    assert_ne!(a, c);
}

/// Heap allocations made while stepping a busy run: hard landings, a goal, the confetti to its end.
fn allocations_of_a_busy_run(visual_effects: bool, push: QuantizedInput) -> usize {
    let mut cfg = SimConfig::retry(1);
    cfg.visual_effects = visual_effects;
    // Built outside the measurement: the pools are reserved when the stage is.
    let mut sim = at_rest(cfg);

    let start = allocations();
    let mut peak = 0;
    for tick in 0..400 {
        // A hard landing every 25 ticks: bursts that overlap, and sparks from the bounce.
        if tick % 25 == 0 {
            sim.ball.pos = V3::new(0.0, 0.5, 0.0);
            sim.ball.vel = V3::new(0.0, -1.0, 0.0);
        }
        sim.step(IDLE);
        peak = peak.max(sim.runtime.effects.len());
    }
    sim.ball.pos = V3::new(0.0, 0.5, 0.0);
    sim.ball.vel = V3::ZERO;
    roll_into_goal(&mut sim, push);
    while !sim.is_finished() {
        sim.step(IDLE);
        std::hint::black_box(sim.hash_visual());
    }
    let gravity = sim.shared_world.gravity;
    for _ in 0..270 {
        sim.runtime.advance(&sim.geom, gravity, None, None, None);
    }
    let allocated = allocations() - start;
    if visual_effects {
        assert!(peak > 40, "the run was busy: {peak} effects at once");
        assert!(sim.runtime.confetti.is_empty(), "and ran to the last piece");
    } else {
        assert_eq!(peak, 0);
    }
    allocated
}

#[test]
fn stepping_with_particles_allocates_nothing() {
    let push = towards_goal();
    let with = allocations_of_a_busy_run(true, push);
    let without = allocations_of_a_busy_run(false, push);
    // The particle pools are sized when the stage is built, so the particles add no allocation at all.
    assert_eq!(
        with, without,
        "heap allocations while stepping with and without particles"
    );
    // And the rest of the tick needs none either.
    assert_eq!(with, 0, "heap allocations while stepping");
}
