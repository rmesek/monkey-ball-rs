//! The simulation on stages that parse but make no sense: it must never panic, never loop forever, and a
//! ball whose position stops being a number must end the attempt instead of hanging it. No game data: the
//! stages are built in code, encoded to the stagedef format and corrupted there, so every field the parser
//! can hand to the engine is reached. The last test repeats this on stagedefs read from the user's disc
//! (in memory only) and skips without one.

mod common;

use mb_engine::determinism::QuantizedInput;
use mb_engine::{Outcome, Phase, Sim, SimConfig};
use mb_stage::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

fn rot(x: i16, y: i16, z: i16) -> S16Vec {
    S16Vec { x, y, z }
}

fn empty_group() -> AnimGroup {
    AnimGroup {
        init_pos: v(0.0, 0.0, 0.0),
        init_rot: rot(0, 0, 0),
        anim_loop_type: 0,
        anim: None,
        model_names: vec![],
        triangles: vec![],
        grid: CollisionGrid {
            origin_x: 0.0,
            origin_z: 0.0,
            step_x: 0.0,
            step_z: 0.0,
            cell_count_x: 0,
            cell_count_z: 0,
            cells: vec![],
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

/// Two triangles forming a 20 x 20 floor at y = 0, centred on the origin.
fn floor_tris() -> Vec<ColiTri> {
    let r2 = std::f32::consts::FRAC_1_SQRT_2;
    let a = ColiTri {
        pos: v(-10.0, 0.0, 10.0),
        normal: v(0.0, 1.0, 0.0),
        rot: rot(-0x4000, 0, 0),
        flags: 0,
        vert2: Vec2 { x: 20.0, y: 0.0 },
        vert3: Vec2 { x: 0.0, y: 20.0 },
        edge2_normal: Vec2 { x: -r2, y: -r2 },
        edge3_normal: Vec2 { x: 1.0, y: 0.0 },
    };
    let mut b = a;
    b.pos = v(10.0, 0.0, -10.0);
    b.rot = rot(-0x4000, -0x8000, 0);
    vec![a, b]
}

fn floor_group() -> AnimGroup {
    let mut g = empty_group();
    g.triangles = floor_tris();
    g.grid = CollisionGrid {
        origin_x: -10.0,
        origin_z: -10.0,
        step_x: 20.0,
        step_z: 20.0,
        cell_count_x: 1,
        cell_count_z: 1,
        cells: vec![vec![0, 1]],
    };
    g
}

fn key(ease: i32, t: f32, value: f32) -> Keyframe {
    Keyframe {
        ease_type: ease,
        time_seconds: t,
        value,
        tangent_in: 0.5,
        tangent_out: -0.5,
    }
}

fn bare_stage(groups: Vec<AnimGroup>) -> Stage {
    Stage {
        loop_start_seconds: 0,
        loop_end_seconds: 4,
        anim_groups: groups,
        start_positions: vec![StartPos {
            pos: v(0.0, 0.5, 0.0),
            rot: rot(0, 0, 0),
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

/// A floor with one object of every kind next to the start, and a second, keyframed group carrying a goal,
/// a banana and its own floor.
fn rich_stage() -> Stage {
    let mut g0 = floor_group();
    g0.goals.push(Goal {
        pos: v(0.0, 0.0, -6.0),
        rot: rot(0, 0, 0),
        type_value: 0x42,
        kind: GoalType::Blue,
    });
    for (i, kind) in [0, 1, 2, 3].into_iter().enumerate() {
        g0.bananas.push(Banana {
            pos: v(i as f32 - 1.5, 0.5, -1.5),
            kind,
        });
    }
    g0.bumpers.push(Bumper {
        pos: v(1.5, 0.0, 0.0),
        rot: rot(0, 0, 0),
        scale: v(1.0, 1.0, 1.0),
    });
    g0.jamabars.push(Jamabar {
        pos: v(-2.5, 0.5, 0.0),
        rot: rot(0, 0x4000, 0),
        scale: v(1.0, 1.0, 1.0),
    });
    g0.cones.push(ColiCone {
        pos: v(0.0, 0.0, 2.0),
        rot: rot(0, 0, 0),
        flags: 0,
        scale: v(1.0, 2.0, 1.0),
    });
    g0.spheres.push(ColiSphere {
        pos: v(2.0, 0.5, 2.0),
        radius: 0.75,
        flags: 0,
    });
    g0.cylinders.push(ColiCylinder {
        pos: v(-2.0, 0.5, 2.0),
        radius: 0.5,
        height: 1.0,
        rot: rot(0, 0, 0),
        flags: 0,
    });
    g0.fallout_boxes.push(FalloutBox {
        pos: v(8.0, 0.5, 8.0),
        scale: v(1.0, 1.0, 1.0),
        rot: rot(0, 0, 0),
    });

    let mut g1 = floor_group();
    g1.init_pos = v(0.0, 0.0, -25.0);
    g1.anim_loop_type = 2;
    g1.anim = Some(AnimHeader {
        rot_x: vec![key(1, 0.0, 0.0), key(2, 2.0, 20.0), key(0, 4.0, 0.0)],
        rot_y: vec![key(2, 0.0, 0.0), key(2, 4.0, 90.0)],
        rot_z: vec![key(1, 1.0, 5.0)],
        pos_x: vec![key(1, 0.0, 0.0), key(1, 2.0, 3.0), key(1, 4.0, 0.0)],
        pos_y: vec![key(2, 0.0, 0.0), key(2, 4.0, 1.0)],
        pos_z: vec![key(1, 0.0, -25.0), key(1, 4.0, -22.0)],
    });
    g1.goals.push(Goal {
        pos: v(0.0, 0.0, -4.0),
        rot: rot(0, 0x2000, 0),
        type_value: 0x52,
        kind: GoalType::Red,
    });
    g1.bananas.push(Banana {
        pos: v(0.0, 0.5, 0.0),
        kind: 1,
    });
    g1.bumpers.push(Bumper {
        pos: v(3.0, 0.0, 0.0),
        rot: rot(0, 0, 0),
        scale: v(1.0, 1.0, 1.0),
    });
    g1.unk_b8 = v(0.1, 0.0, 0.0);
    bare_stage(vec![g0, g1, empty_group()])
}

/// Stagedef encoder for the fields the engine reads (the inverse of `mb_stage::parse_stagedef_smb1`).
struct Enc(Vec<u8>);

impl Enc {
    fn alloc(&mut self, len: usize) -> usize {
        let at = self.0.len();
        self.0.resize(at + len.div_ceil(4) * 4, 0);
        at
    }
    fn u32(&mut self, o: usize, x: u32) {
        self.0[o..o + 4].copy_from_slice(&x.to_be_bytes());
    }
    fn u16(&mut self, o: usize, x: u16) {
        self.0[o..o + 2].copy_from_slice(&x.to_be_bytes());
    }
    fn f(&mut self, o: usize, x: f32) {
        self.u32(o, x.to_bits());
    }
    fn v3(&mut self, o: usize, p: Vec3) {
        self.f(o, p.x);
        self.f(o + 4, p.y);
        self.f(o + 8, p.z);
    }
    fn v2(&mut self, o: usize, p: Vec2) {
        self.f(o, p.x);
        self.f(o + 4, p.y);
    }
    fn rot(&mut self, o: usize, r: S16Vec) {
        self.u16(o, r.x as u16);
        self.u16(o + 2, r.y as u16);
        self.u16(o + 4, r.z as u16);
    }
    /// Writes `(count, pointer)` at `slot` and returns the offset of `count` records of `size` bytes.
    fn list(&mut self, slot: usize, count: usize, size: usize) -> usize {
        if count == 0 {
            return 0;
        }
        let at = self.alloc(count * size);
        self.u32(slot, count as u32);
        self.u32(slot + 4, at as u32);
        at
    }
    fn track(&mut self, slot: usize, keys: &[Keyframe]) {
        let at = self.list(slot, keys.len(), KEYFRAME_SIZE);
        for (i, k) in keys.iter().enumerate() {
            let o = at + i * KEYFRAME_SIZE;
            self.u32(o, k.ease_type as u32);
            self.f(o + 4, k.time_seconds);
            self.f(o + 8, k.value);
            self.f(o + 0xc, k.tangent_in);
            self.f(o + 0x10, k.tangent_out);
        }
    }
}

fn encode(s: &Stage) -> Vec<u8> {
    let mut e = Enc(vec![0; 0xa0]);
    e.u32(0x00, s.loop_start_seconds as u32);
    e.u32(0x04, s.loop_end_seconds as u32);
    let groups = e.list(0x08, s.anim_groups.len(), ANIM_GROUP_SIZE);
    if !s.start_positions.is_empty() {
        // The start positions end where the fall-out height begins: that is how their count is stored.
        let at = e.alloc(s.start_positions.len() * START_POS_SIZE + 4);
        e.u32(0x10, at as u32);
        for (i, p) in s.start_positions.iter().enumerate() {
            e.v3(at + i * START_POS_SIZE, p.pos);
            e.rot(at + i * START_POS_SIZE + 0xc, p.rot);
        }
        let fallout = at + s.start_positions.len() * START_POS_SIZE;
        e.u32(0x14, fallout as u32);
        e.f(fallout, s.fallout_y);
    }
    for (gi, g) in s.anim_groups.iter().enumerate() {
        let o = groups + gi * ANIM_GROUP_SIZE;
        e.v3(o, g.init_pos);
        e.rot(o + 0xc, g.init_rot);
        e.u16(o + 0x12, g.anim_loop_type);
        e.v3(o + 0xb8, g.unk_b8);
        if let Some(a) = &g.anim {
            let h = e.alloc(0x30);
            e.u32(o + 0x14, h as u32);
            for (i, t) in [&a.rot_x, &a.rot_y, &a.rot_z, &a.pos_x, &a.pos_y, &a.pos_z]
                .into_iter()
                .enumerate()
            {
                e.track(h + i * 8, t);
            }
        }
        if !g.triangles.is_empty() {
            let at = e.alloc(g.triangles.len() * TRIANGLE_SIZE);
            e.u32(o + 0x1c, at as u32);
            for (i, t) in g.triangles.iter().enumerate() {
                let b = at + i * TRIANGLE_SIZE;
                e.v3(b, t.pos);
                e.v3(b + 0xc, t.normal);
                e.rot(b + 0x18, t.rot);
                e.u16(b + 0x1e, t.flags);
                e.v2(b + 0x20, t.vert2);
                e.v2(b + 0x28, t.vert3);
                e.v2(b + 0x30, t.edge2_normal);
                e.v2(b + 0x38, t.edge3_normal);
            }
        }
        e.f(o + 0x24, g.grid.origin_x);
        e.f(o + 0x28, g.grid.origin_z);
        e.f(o + 0x2c, g.grid.step_x);
        e.f(o + 0x30, g.grid.step_z);
        e.u32(o + 0x34, g.grid.cell_count_x as u32);
        e.u32(o + 0x38, g.grid.cell_count_z as u32);
        if !g.grid.cells.is_empty() {
            let table = e.alloc(g.grid.cells.len() * 4);
            e.u32(o + 0x20, table as u32);
            for (i, cell) in g.grid.cells.iter().enumerate() {
                let at = e.alloc(cell.len() * 2 + 2);
                e.u32(table + i * 4, at as u32);
                for (k, &t) in cell.iter().enumerate() {
                    e.u16(at + k * 2, t);
                }
                e.u16(at + cell.len() * 2, 0xffff);
            }
        }
        let at = e.list(o + 0x3c, g.goals.len(), GOAL_SIZE);
        for (i, x) in g.goals.iter().enumerate() {
            e.v3(at + i * GOAL_SIZE, x.pos);
            e.rot(at + i * GOAL_SIZE + 0xc, x.rot);
            e.u16(at + i * GOAL_SIZE + 0x12, x.type_value);
        }
        let at = e.list(o + 0x4c, g.bumpers.len(), BUMPER_SIZE);
        for (i, x) in g.bumpers.iter().enumerate() {
            e.v3(at + i * BUMPER_SIZE, x.pos);
            e.rot(at + i * BUMPER_SIZE + 0xc, x.rot);
            e.v3(at + i * BUMPER_SIZE + 0x14, x.scale);
        }
        let at = e.list(o + 0x54, g.jamabars.len(), JAMABAR_SIZE);
        for (i, x) in g.jamabars.iter().enumerate() {
            e.v3(at + i * JAMABAR_SIZE, x.pos);
            e.rot(at + i * JAMABAR_SIZE + 0xc, x.rot);
            e.v3(at + i * JAMABAR_SIZE + 0x14, x.scale);
        }
        let at = e.list(o + 0x5c, g.bananas.len(), BANANA_SIZE);
        for (i, x) in g.bananas.iter().enumerate() {
            e.v3(at + i * BANANA_SIZE, x.pos);
            e.u32(at + i * BANANA_SIZE + 0xc, x.kind as u32);
        }
        let at = e.list(o + 0x64, g.cones.len(), CONE_SIZE);
        for (i, x) in g.cones.iter().enumerate() {
            e.v3(at + i * CONE_SIZE, x.pos);
            e.rot(at + i * CONE_SIZE + 0xc, x.rot);
            e.u16(at + i * CONE_SIZE + 0x12, x.flags);
            e.v3(at + i * CONE_SIZE + 0x14, x.scale);
        }
        let at = e.list(o + 0x6c, g.spheres.len(), SPHERE_SIZE);
        for (i, x) in g.spheres.iter().enumerate() {
            e.v3(at + i * SPHERE_SIZE, x.pos);
            e.f(at + i * SPHERE_SIZE + 0xc, x.radius);
            e.u16(at + i * SPHERE_SIZE + 0x10, x.flags);
        }
        let at = e.list(o + 0x74, g.cylinders.len(), CYLINDER_SIZE);
        for (i, x) in g.cylinders.iter().enumerate() {
            e.v3(at + i * CYLINDER_SIZE, x.pos);
            e.f(at + i * CYLINDER_SIZE + 0xc, x.radius);
            e.f(at + i * CYLINDER_SIZE + 0x10, x.height);
            e.rot(at + i * CYLINDER_SIZE + 0x14, x.rot);
            e.u16(at + i * CYLINDER_SIZE + 0x1a, x.flags);
        }
        let at = e.list(o + 0x84, g.fallout_boxes.len(), FALLOUT_BOX_SIZE);
        for (i, x) in g.fallout_boxes.iter().enumerate() {
            e.v3(at + i * FALLOUT_BOX_SIZE, x.pos);
            e.v3(at + i * FALLOUT_BOX_SIZE + 0xc, x.scale);
            e.rot(at + i * FALLOUT_BOX_SIZE + 0x18, x.rot);
        }
    }
    e.0
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// Words that are poison as a float (NaN, infinities, huge, denormal, negative zero) or as a count / angle.
const POISON: [u32; 20] = [
    0x7fc0_0000, // NaN
    0xffc0_0001, // NaN with payload
    0x7f80_0000, // +inf
    0xff80_0000, // -inf
    0x7f7f_ffff, // f32::MAX
    0xff7f_ffff, // f32::MIN
    0x0000_0001, // smallest denormal (and the count 1)
    0x8000_0000, // -0.0 (and i32::MIN)
    0x0000_0000,
    0x3f80_0000, // 1.0
    0xbf80_0000, // -1.0
    0x7149_f2ca, // 1e30
    0xf149_f2ca, // -1e30
    0x0da2_4260, // 1e-30
    0x7fff_ffff,
    0xffff_ffff,
    0x8000_8000,
    0x7fff_7fff,
    0x0001_0000,
    0x4974_2400, // 1e6
];

fn corrupt(src: &[u8], rng: &mut Rng) -> Vec<u8> {
    let mut b = src.to_vec();
    for _ in 0..1 + rng.below(4) {
        let o = rng.below(b.len() / 4) * 4;
        if rng.below(5) == 0 {
            let i = o + rng.below(4);
            b[i] = rng.next() as u8;
        } else {
            let w = POISON[rng.below(POISON.len())];
            b[o..o + 4].copy_from_slice(&w.to_be_bytes());
        }
    }
    b
}

fn ball_is_finite(sim: &Sim) -> bool {
    let p = sim.ball.pos;
    p.x.is_finite() && p.y.is_finite() && p.z.is_finite()
}

/// Runs `ticks` ticks with stick input from `rng`. Fails if a ball that is not a number is still "playing"
/// two seconds later.
fn drive(sim: &mut Sim, ticks: usize, rng: &mut Rng) {
    let mut lost_for = 0;
    for t in 0..ticks {
        let input = match t / 20 % 4 {
            0 => QuantizedInput::default(),
            1 => QuantizedInput { x: 127, y: -127 },
            2 => QuantizedInput {
                x: rng.below(255) as i32 - 127,
                y: rng.below(255) as i32 - 127,
            },
            // Out of range on purpose: the engine is a library and must not trust its caller either.
            _ => QuantizedInput {
                x: i32::MAX,
                y: i32::MIN,
            },
        };
        sim.step(input);
        let _ = sim.hash();
        if sim.is_finished() {
            return;
        }
        if sim.phase == Phase::Playing && !ball_is_finite(sim) {
            lost_for += 1;
            assert!(
                lost_for < 120,
                "the ball has been at {:?} for two seconds and the attempt goes on",
                sim.ball.pos
            );
        }
    }
}

fn short_cfg(rng: &mut Rng) -> SimConfig {
    let mut cfg = SimConfig::retry(1 + rng.below(120) as u32);
    cfg.intro_frames = 26;
    cfg.fly_in_frames = [0, 1, 20, 90][rng.below(4)];
    cfg.time_limit_frames = [60 * 60, 100, 1, 0, -5, i32::MAX][rng.below(6)];
    cfg.bonus_stage = rng.below(4) == 0;
    cfg
}

/// A stuck simulation would hang the test run with no output, so say what happened and stop.
fn watchdog(seconds: u64) -> std::sync::mpsc::Sender<()> {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        if rx.recv_timeout(std::time::Duration::from_secs(seconds))
            == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        {
            eprintln!("simulation did not return within {seconds} s: a tick is looping");
            std::process::abort();
        }
    });
    tx
}

fn guarded(what: &str, f: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(f)).is_err() {
        panic!("simulation failed on {what}");
    }
}

#[test]
fn encoder_round_trips_the_rich_stage() {
    let stage = rich_stage();
    let parsed = parse_stagedef_smb1(&encode(&stage)).unwrap();
    assert_eq!(parsed.anim_groups, stage.anim_groups);
    assert_eq!(parsed.start_positions, stage.start_positions);
    assert_eq!(parsed.fallout_y, stage.fallout_y);
    // And the rich stage is playable: the ball settles on the floor.
    let mut cfg = SimConfig::retry(1);
    cfg.intro_frames = 26;
    let mut sim = Sim::new(&parsed, cfg);
    for _ in 0..90 {
        sim.step(QuantizedInput::default());
    }
    assert_eq!(sim.phase, Phase::Playing);
    assert!((sim.ball.pos.y - 0.5).abs() < 0.05, "{:?}", sim.ball.pos);
}

#[test]
fn corrupted_stagedefs_never_panic_or_hang() {
    let done = watchdog(300);
    let src = encode(&rich_stage());
    let mut rng = Rng(0x5eed_0030);
    let (mut simulated, mut rejected) = (0, 0);
    for i in 0..2500 {
        let bytes = corrupt(&src, &mut rng);
        let Ok(stage) = parse_stagedef_smb1(&bytes) else {
            rejected += 1;
            continue;
        };
        simulated += 1;
        let cfg = short_cfg(&mut rng);
        guarded(&format!("corrupted stagedef {i}"), || {
            let mut sim = Sim::new(&stage, cfg);
            drive(&mut sim, 220, &mut rng);
        });
    }
    assert!(
        simulated > 1500 && rejected > 50,
        "{simulated} / {rejected}"
    );
    let _ = done.send(());
}

/// Every 4-byte word of the stagedef replaced by every poison value, one at a time.
#[test]
fn every_field_poisoned_in_turn() {
    let done = watchdog(300);
    let src = encode(&rich_stage());
    let mut rng = Rng(0x5eed_0031);
    let mut b = src.clone();
    for o in (0..src.len()).step_by(4) {
        // Padding and unused header words parse to the same stage; skip the repeats.
        let mut last = None;
        for (k, w) in POISON.into_iter().enumerate() {
            b[o..o + 4].copy_from_slice(&w.to_be_bytes());
            let Ok(stage) = parse_stagedef_smb1(&b) else {
                continue;
            };
            if last.as_ref() == Some(&stage) {
                continue;
            }
            let mut cfg = SimConfig::retry(1);
            cfg.intro_frames = 26;
            cfg.fly_in_frames = 20;
            guarded(&format!("word {o:#x} = {w:#010x} ({k})"), || {
                let mut sim = Sim::new(&stage, cfg);
                drive(&mut sim, 70, &mut rng);
            });
            last = Some(stage);
        }
        b[o..o + 4].copy_from_slice(&src[o..o + 4]);
    }
    let _ = done.send(());
}

fn run_to_end(stage: &Stage, cfg: SimConfig, max_ticks: usize) -> Sim {
    let mut sim = Sim::new(stage, cfg);
    for _ in 0..max_ticks {
        sim.step(QuantizedInput::default());
        if sim.is_finished() {
            break;
        }
    }
    sim
}

#[test]
fn empty_and_degenerate_stages() {
    let done = watchdog(120);
    let cfg = || {
        let mut c = SimConfig::retry(1);
        c.time_limit_frames = 600;
        c
    };
    // Nothing at all: no groups, no start position. The ball starts at the origin and falls.
    let mut nothing = bare_stage(vec![]);
    nothing.start_positions.clear();
    let sim = run_to_end(&nothing, cfg(), 2000);
    assert_eq!(sim.phase, Phase::Finished(Outcome::FellOut));

    // Groups that hold nothing, and a grid that claims cells it does not have.
    let mut hollow = empty_group();
    hollow.grid.cell_count_x = i32::MAX;
    hollow.grid.cell_count_z = i32::MAX;
    hollow.grid.step_x = 1.0;
    hollow.grid.step_z = 1.0;
    let sim = run_to_end(&bare_stage(vec![empty_group(), hollow]), cfg(), 2000);
    assert_eq!(sim.phase, Phase::Finished(Outcome::FellOut));

    // A floor and no goal: play runs until the clock ends it.
    let sim = run_to_end(&bare_stage(vec![floor_group()]), cfg(), 2000);
    assert_eq!(sim.phase, Phase::Finished(Outcome::TimeOver));

    // Degenerate triangles: zero area, zero normal, NaN edges. The ball falls through or rests; no panic.
    let mut g = floor_group();
    for t in &mut g.triangles {
        t.vert2 = Vec2 { x: 0.0, y: 0.0 };
        t.vert3 = Vec2 { x: 0.0, y: 0.0 };
        t.normal = v(0.0, 0.0, 0.0);
        t.edge2_normal = Vec2 {
            x: f32::NAN,
            y: f32::NAN,
        };
    }
    assert!(run_to_end(&bare_stage(vec![g]), cfg(), 2000).is_finished());

    // Grid cells that name triangles the group does not have (a hand-built stage; the parser cannot
    // produce this, the engine still must not index out of bounds).
    let mut g = floor_group();
    g.grid.cells = vec![vec![0, 1, 2, 500, u16::MAX]];
    assert!(run_to_end(&bare_stage(vec![g]), cfg(), 2000).is_finished());

    // Zero-size and negative-size objects on the ball's spot.
    let mut g = floor_group();
    for scale in [0.0, -1.0, f32::MIN_POSITIVE] {
        g.bumpers.push(Bumper {
            pos: v(0.0, 0.0, 0.0),
            rot: rot(0, 0, 0),
            scale: v(scale, scale, scale),
        });
        g.jamabars.push(Jamabar {
            pos: v(0.0, 0.5, 0.0),
            rot: rot(0, 0, 0),
            scale: v(scale, scale, scale),
        });
        g.cones.push(ColiCone {
            pos: v(0.0, 0.0, 0.0),
            rot: rot(0, 0, 0),
            flags: 0,
            scale: v(scale, scale, scale),
        });
        g.spheres.push(ColiSphere {
            pos: v(0.0, 0.5, 0.0),
            radius: scale,
            flags: 0,
        });
        g.cylinders.push(ColiCylinder {
            pos: v(0.0, 0.5, 0.0),
            radius: scale,
            height: scale,
            rot: rot(0, 0, 0),
            flags: 0,
        });
        g.fallout_boxes.push(FalloutBox {
            pos: v(5.0, 0.5, 5.0),
            scale: v(scale, scale, scale),
            rot: rot(0, 0, 0),
        });
        g.goals.push(Goal {
            pos: v(0.0, 0.0, 0.0),
            rot: rot(0, 0, 0),
            type_value: 0xffff,
            kind: GoalType::from_value(0xffff),
        });
    }
    assert!(run_to_end(&bare_stage(vec![g]), cfg(), 2000).is_finished());
    let _ = done.send(());
}

#[test]
fn odd_animations() {
    let done = watchdog(120);
    let tracks: Vec<Vec<Keyframe>> = vec![
        vec![],
        vec![key(1, 0.0, 3.0)],
        // Two keys at the same time: the interpolation divides by zero.
        vec![key(1, 1.0, 0.0), key(1, 1.0, 5.0), key(2, 1.0, 9.0)],
        // Out of order, negative and non-finite times.
        vec![key(2, 5.0, 1.0), key(2, -3.0, 2.0), key(1, 0.5, 3.0)],
        vec![key(1, f32::NAN, 1.0), key(1, f32::INFINITY, f32::NAN)],
        vec![key(2, 0.0, f32::MAX), key(2, 1.0, f32::MIN)],
        vec![key(i32::MIN, 0.0, 1.0), key(i32::MAX, 1.0, 2.0)],
    ];
    for (i, track) in tracks.iter().enumerate() {
        for loop_type in [0u16, 1, 2, 3, 0xffff] {
            for (start, end) in [(0, 0), (0, 4), (4, 0), (-1, 1), (i32::MIN, i32::MAX)] {
                let mut g = floor_group();
                g.anim_loop_type = loop_type;
                g.anim = Some(AnimHeader {
                    rot_x: track.clone(),
                    rot_y: tracks[(i + 1) % tracks.len()].clone(),
                    rot_z: track.clone(),
                    pos_x: track.clone(),
                    pos_y: tracks[(i + 2) % tracks.len()].clone(),
                    pos_z: track.clone(),
                });
                g.bananas.push(Banana {
                    pos: v(0.0, 0.5, 0.0),
                    kind: -1,
                });
                let mut stage = bare_stage(vec![floor_group(), g]);
                stage.loop_start_seconds = start;
                stage.loop_end_seconds = end;
                let mut cfg = SimConfig::retry(1);
                cfg.intro_frames = 26;
                guarded(
                    &format!("track {i}, loop type {loop_type}, loop {start}..{end}"),
                    || {
                        let mut sim = Sim::new(&stage, cfg);
                        drive(&mut sim, 200, &mut Rng(7));
                    },
                );
            }
        }
    }
    let _ = done.send(());
}

/// A ball whose position is not a number can never cross the fall-out plane, so the reference would leave
/// the player watching nothing until the clock runs out. Here it ends the attempt as a fall-out.
#[test]
fn a_nan_ball_falls_out_instead_of_hanging() {
    let done = watchdog(120);
    for poison in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for (limit, bonus) in [(0, false), (3600, false), (3600, true)] {
            let mut cfg = SimConfig::retry(if bonus { 91 } else { 1 });
            cfg.time_limit_frames = limit;
            // The rich stage has bananas left, so a bonus stage does not end as cleared at once.
            let mut sim = Sim::new(&rich_stage(), cfg);
            while sim.phase == Phase::Intro {
                sim.step(QuantizedInput::default());
            }
            sim.step(QuantizedInput::default());
            sim.ball.pos.x = poison;
            let mut ticks = 0;
            while !sim.is_finished() {
                sim.step(QuantizedInput { x: 50, y: 50 });
                ticks += 1;
                assert!(ticks < 600, "{poison}: still running after ten seconds");
            }
            assert_eq!(sim.phase, Phase::Finished(Outcome::FellOut), "{poison}");
        }
    }
    // A stage whose fall-out height is not a number has no fall-out plane at all (as in the reference): the
    // ball falls until the clock, which every game mode sets, ends the attempt.
    let mut stage = bare_stage(vec![]);
    stage.fallout_y = f32::NAN;
    let sim = run_to_end(&stage, SimConfig::retry(1), 60 * 70);
    assert_eq!(sim.phase, Phase::Finished(Outcome::TimeOver));
    assert!(sim.ball.pos.y.is_finite());
    let _ = done.send(());
}

#[test]
fn odd_configurations() {
    let done = watchdog(120);
    let stage = rich_stage();
    for intro in [i32::MIN, -1, 0, 1, 23, 24, 25, 120] {
        for fly_in in [i32::MIN, -1, 0, 1, i32::MAX] {
            for limit in [i32::MIN, 0, 1, i32::MAX] {
                let mut cfg = SimConfig::first_attempt(u32::MAX);
                cfg.intro_frames = intro;
                cfg.fly_in_frames = fly_in;
                cfg.time_limit_frames = limit;
                cfg.goal_tape_anchor_y =
                    [0.0, f64::NAN, f64::INFINITY, -1e300][(intro & 3) as usize];
                guarded(
                    &format!("intro {intro}, fly-in {fly_in}, limit {limit}"),
                    || {
                        let mut sim = Sim::new(&stage, cfg);
                        drive(&mut sim, 200, &mut Rng(9));
                    },
                );
            }
        }
    }
    let _ = done.send(());
}

#[test]
fn corrupted_real_stagedefs_never_panic_or_hang() {
    let Some(disc) = common::open_disc() else {
        eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
        return;
    };
    let done = watchdog(300);
    let mut rng = Rng(0x5eed_0032);
    let (mut simulated, mut rejected) = (0, 0);
    for id in [1u32, 5, 13, 28, 47, 64, 91, 95, 101, 115] {
        let Ok(raw) =
            mb_assets::pack::PackProvider::fetch(&disc, &format!("test/st{id:03}/STAGE{id:03}.lz"))
        else {
            continue;
        };
        let src = mb_assets::lzss::decompress(&raw).unwrap();
        for i in 0..40 {
            let bytes = corrupt(&src, &mut rng);
            let Ok(stage) = parse_stagedef_smb1(&bytes) else {
                rejected += 1;
                continue;
            };
            simulated += 1;
            let cfg = short_cfg(&mut rng);
            guarded(&format!("stage {id}, mutation {i}"), || {
                let mut sim = Sim::new(&stage, cfg);
                drive(&mut sim, 150, &mut rng);
            });
        }
    }
    eprintln!("real stagedefs: {simulated} corrupted copies simulated, {rejected} rejected");
    assert!(simulated > 100);
    let _ = done.send(());
}
