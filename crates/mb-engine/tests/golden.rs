//! The simulation's behaviour, recorded: every stage on the disc is run through fixed input scripts and a
//! hash over every tick's state is compared with the value recorded here. Skips without a disc image.
//!
//! This is what holds the simulation still. The port was proved equal to WebMonkeyBall tick for tick by a
//! harness that ran the reference under Deno (`tests/parity.rs`, in the repository up to 0.2.0); the values
//! below were recorded from the engine where that comparison last passed (0.2.0, USA disc), so a change to
//! them is a change to what it proved. A behaviour change that is meant updates the table in the same
//! commit and says why; the failure message prints the new rows.
//!
//! The scripted runs use integer arithmetic only, so their inputs are the same on every machine. The state
//! itself is IEEE arithmetic and the game's own trig tables, with one call into the platform's `cos`
//! (`math::cos_quarter_turn`, off the table's grid) and its `sqrt`; a difference on another architecture
//! would be worth knowing about, and this test is where it would show.

mod common;

use mb_engine::determinism::{BUTTON_PRIMARY_ACTION, QuantizedInput};
use mb_engine::{Outcome, Phase, Sim, SimConfig};

/// FNV-1a over 32-bit words.
struct Fnv(u32);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0x811c_9dc5)
    }
    fn word(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ b as u32).wrapping_mul(16777619);
        }
    }
}

struct XorShift(u32);

impl XorShift {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// Random directions held for random times, and the action held now and then (it fast-forwards a first
/// fly-in and cuts a fall-out short).
fn wander(seed: u32, ticks: usize) -> Vec<(QuantizedInput, u32)> {
    let mut rng = XorShift(0x1234_5678 ^ seed.wrapping_mul(0x9e37_79b9));
    let (mut hold, mut cur, mut action) = (0u32, QuantizedInput::default(), 0u32);
    (0..ticks)
        .map(|_| {
            if hold == 0 {
                let r = rng.next();
                cur = QuantizedInput {
                    x: ((r & 0xff) as i32 - 128).clamp(-127, 127),
                    y: (((r >> 8) & 0xff) as i32 - 128).clamp(-127, 127),
                };
                hold = 10 + (r >> 16) % 50;
                action = if (r >> 24).is_multiple_of(4) {
                    BUTTON_PRIMARY_ACTION
                } else {
                    0
                };
            }
            hold -= 1;
            (cur, action)
        })
        .collect()
}

/// The stick held forward: off the edge on most stages, into the goal on a few.
fn forward(ticks: usize) -> Vec<(QuantizedInput, u32)> {
    vec![(QuantizedInput { x: 0, y: -127 }, 0); ticks]
}

/// Run `inputs` and fold every tick's state hash, display hash and event count into `h`.
fn run(sim: &mut Sim, inputs: &[(QuantizedInput, u32)], h: &mut Fnv) {
    for (input, buttons) in inputs {
        sim.step_with_buttons(*input, *buttons);
        h.word(sim.hash());
        h.word(sim.hash_visual());
        h.word(sim.events.len() as u32);
    }
}

/// One stage: a first attempt that wanders, then a retry with the stick held forward.
fn stage_hash(stage: &mb_stage::Stage, id: u32) -> u32 {
    let mut h = Fnv::new();
    let mut sim = Sim::new(stage, SimConfig::first_attempt(id));
    run(&mut sim, &wander(id, 1500), &mut h);
    let mut sim = Sim::new(stage, SimConfig::retry(id));
    run(&mut sim, &forward(900), &mut h);
    h.0
}

/// Stage id and the hash of its two runs, USA disc, recorded at 0.2.0.
#[rustfmt::skip]
const STAGES: &[(u32, u32)] = &[
    (1, 0x26e8db7b), (2, 0xcc01ee16), (3, 0x028deb31), (4, 0x3fcea9d7), (5, 0x10233e69),
    (6, 0xcc1d5c77), (7, 0x31469aeb), (8, 0x2298fac9), (9, 0x0707928f), (10, 0x1f6d8524),
    (11, 0xc730a44f), (12, 0x83c6c580), (13, 0x7415b6de), (14, 0x78890c0b), (15, 0x387e49d9),
    (16, 0x480a6d0c), (17, 0x40af66fb), (18, 0x7dc63a08), (19, 0xc236d7e9), (20, 0xedcac0e4),
    (21, 0xc50919dd), (22, 0x0c2471ab), (23, 0x34a6a541), (24, 0x354e4096), (25, 0xa8646ad9),
    (26, 0x00fdcf85), (27, 0xa841f23d), (28, 0xe4c786d2), (29, 0xb49bccc5), (30, 0x0c8ed945),
    (31, 0x586ad095), (32, 0x310decb4), (33, 0x6e430d87), (34, 0xf92b2bc1), (35, 0x6194adb9),
    (36, 0xadb1b460), (37, 0x90187721), (38, 0x712a758f), (39, 0x51efa330), (40, 0x9777aba3),
    (41, 0x37d0f625), (42, 0xfed46506), (43, 0xe8266783), (44, 0xa3a49686), (45, 0x378cc73c),
    (46, 0x5375244a), (47, 0x1ca5139b), (48, 0xe917647a), (49, 0xcdf9b82d), (50, 0xf90fb114),
    (51, 0x1abc88e5), (52, 0x143ddb81), (53, 0x1348bbcb), (54, 0xb7f2cc9e), (55, 0x7dc25039),
    (56, 0xb94c7049), (57, 0xc1b1dc57), (58, 0x7e196c12), (59, 0x449c9575), (60, 0xee5609f5),
    (61, 0x6bf1e1fb), (62, 0x763bb1cb), (63, 0x5dac7af1), (64, 0xcc82eb99), (65, 0xb805adec),
    (66, 0x5c8350e1), (67, 0x3b1fcc2a), (68, 0x0361e183), (69, 0xfd5a3a03), (70, 0x7d108e54),
    (71, 0x43a51766), (72, 0xcffa9747), (73, 0x0b8866c6), (74, 0x1543aca8), (75, 0x1d21176c),
    (76, 0xac5cc05a), (77, 0x023d07b6), (78, 0x48531e65), (79, 0x8d9a70ef), (80, 0x8ea99be3),
    (81, 0xee3ff331), (82, 0xe6b634e2), (83, 0x31d89ec0), (84, 0x29accabd), (85, 0xda334d9b),
    (86, 0xf19a1f68), (87, 0x8078191b), (88, 0x70e9b779), (89, 0xd26bb119), (90, 0x4a692e1f),
    (91, 0xb0fa975f), (92, 0x01ca4bfb), (93, 0x5ee6214b), (94, 0x2b2d3953), (95, 0xa602aacf),
    (96, 0x19e0b94a), (97, 0xfcd4e974), (98, 0x770d157a), (99, 0x33ad0b3e), (100, 0x588cefb6),
    (101, 0x972ca041), (102, 0x744702ab), (103, 0x39c1ef32), (104, 0x7d619875), (105, 0x9aa6c7d1),
    (106, 0xaa357089), (107, 0xef37d9db), (108, 0xdc115205), (109, 0xe3206d22), (110, 0x7ed7b10e),
    (111, 0xa393e0aa), (112, 0x0810bd9d), (113, 0xb8246a4a), (114, 0x16484364), (115, 0xb62a1e94),
    (116, 0xb082fe78), (117, 0x68823892), (118, 0x311d9d55), (119, 0x6c91135e), (120, 0x747c02b2),
    (121, 0x42b36bc5), (122, 0xe0e21b70), (123, 0x7ffc99d3), (124, 0x139b2087), (125, 0x69770748),
    (126, 0x8c44eb2f), (127, 0x6db83c73), (128, 0xafd4644e), (129, 0xaee6002a), (130, 0xdef93b45),
    (131, 0x5843afc9), (132, 0xb308ba46), (133, 0xcce66a96), (134, 0x875c70a4), (135, 0xcceab97a),
    (136, 0xbd43b999), (137, 0xefaa2b17), (138, 0x893130e5), (139, 0x73a0ad96), (140, 0xaa1c008a),
    (141, 0xdc7ecc96), (142, 0x22548b15), (143, 0xc1974f89), (144, 0xe4b0a167), (145, 0x56edf797),
    (146, 0x6740d67a), (147, 0xc89f7f93), (148, 0x20f7c124), (149, 0x662c3d74), (150, 0xcb2179f7),
    (151, 0xf03a21e6), (152, 0x1a7e53e7), (153, 0x09263abd), (154, 0xe569f5fe), (155, 0x719314f6),
    (156, 0x56123aca), (157, 0xe556c88c), (158, 0xb0ab4f75), (159, 0xd11de96a), (160, 0x0dff9461),
    (161, 0x1bd1fade), (162, 0x58a1f642), (163, 0x2476ac29), (164, 0x24590fb5), (165, 0xbfdcce4a),
    (166, 0x6cdce387), (167, 0x0fa86a4f), (168, 0xd9d1e276), (169, 0xe6456524), (170, 0x6f405d3c),
    (171, 0x2b31a813), (172, 0x136a9ddc), (173, 0x933230b9), (174, 0xad782ac4), (175, 0xf1d99784),
    (176, 0x90da5b49), (177, 0x8c926dc4), (178, 0xf69f7188), (179, 0x490c0ec8), (180, 0xf4ad43dc),
    (181, 0x5c4ff234), (182, 0x78b95416), (183, 0x689967d5), (184, 0xf0a470cd), (185, 0xbb1820fd),
    (186, 0x16757370), (187, 0x0c013f78), (188, 0x330812e5), (189, 0xcc494b7d), (190, 0x96ca3dd7),
    (191, 0x6df441fc), (192, 0xdc28e1b6), (193, 0x5ddbe85a), (194, 0xbccdeab0), (195, 0xa0720eef),
    (196, 0xa99f433a), (197, 0x49940579), (198, 0xa571eb79), (199, 0x62763c58), (200, 0x30002dcc),
];

#[test]
fn every_stage_simulates_exactly_as_recorded() {
    let Some(disc) = common::open_disc() else {
        eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
        return;
    };
    if !common::install_real_tables() {
        eprintln!("skipped: could not read the trig tables from main.dol");
        return;
    }
    let mut got = Vec::new();
    for id in 1..=200u32 {
        let Some(stage) = common::try_load_stage(&disc, id) else {
            continue;
        };
        if stage.start_positions.is_empty() || stage.anim_groups.is_empty() {
            continue;
        }
        got.push((id, stage_hash(&stage, id)));
    }
    if got != STAGES {
        let changed: Vec<u32> = got
            .iter()
            .filter(|row| !STAGES.contains(row))
            .map(|row| row.0)
            .collect();
        let mut table = String::new();
        for row in got.chunks(5) {
            let line: Vec<String> = row
                .iter()
                .map(|(id, h)| format!("({id}, {h:#010x})"))
                .collect();
            table.push_str(&format!("    {},\n", line.join(", ")));
        }
        panic!(
            "the simulation changed on stages {changed:?} ({} run, {} recorded). If that is meant, \
             the new table is:\n{table}",
            got.len(),
            STAGES.len()
        );
    }
}

/// Stage id, the tick on which the autopilot's ball entered the goal, bananas collected, and the hash of the
/// run, USA disc, recorded at 0.2.0. The autopilot steers with `f64` trigonometry, so unlike the table above
/// these inputs could differ by a unit on a machine whose `sin` rounds differently.
const GOALS: &[(u32, u32, u32, u32)] = &[
    (1, 683, 4, 0x051c8957),
    (3, 1141, 2, 0x70ef62c5),
    (11, 698, 4, 0x0b2f7aa3),
    (17, 2450, 10, 0x099a9691),
];

#[test]
fn goal_runs_simulate_exactly_as_recorded() {
    let Some(disc) = common::open_disc() else {
        eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
        return;
    };
    if !common::install_real_tables() {
        eprintln!("skipped: could not read the trig tables from main.dol");
        return;
    }
    let mut got = Vec::new();
    for id in [1u32, 3, 11, 17] {
        let stage = common::load_stage(&disc, id);
        let mut ap = common::Autopilot::for_stage(&stage);
        let mut sim = Sim::new(&stage, SimConfig::first_attempt(id));
        let total = sim.bananas_left;
        let mut h = Fnv::new();
        let mut goal_tick = 0;
        for tick in 0..3000u32 {
            let input = if sim.intro_timer <= 0 && !sim.is_finished() {
                ap.input(&sim)
            } else {
                QuantizedInput::default()
            };
            run(&mut sim, &[(input, 0)], &mut h);
            if goal_tick == 0 && sim.phase == Phase::GoalSequence {
                goal_tick = tick;
            }
        }
        assert!(
            matches!(sim.phase, Phase::Finished(Outcome::Goal(_))),
            "stage {id}: {:?}",
            sim.phase
        );
        got.push((id, goal_tick, (total - sim.bananas_left) as u32, h.0));
    }
    assert!(
        got == GOALS,
        "the goal runs changed. If that is meant, the new table is:\n{}",
        got.iter()
            .map(|(id, t, b, h)| format!("    ({id}, {t}, {b}, {h:#010x}),\n"))
            .collect::<String>()
    );
}
