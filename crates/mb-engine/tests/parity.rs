//! Parity of the Rust simulation against the real WebMonkeyBall `Game` run headless under Deno.
//!
//! Skips cleanly when any of the following is missing: a Super Monkey Ball 1 disc image (`MB_DISC` or next to
//! the repository), `deno` (`DENO` env or PATH) or the reference checkout (`MB_REFERENCE` or `../WebMonkeyBall`
//! next to the repository). The reference is only read; its game logic runs unmodified (DOM/WebGL globals and
//! `gl-matrix`/`fflate` are stubbed, see `tests/ref/`). Disc data is staged in the OS temp dir for the Deno
//! process and removed afterwards.
//!
//! For each scripted stage the reference writes one line per tick (ball position/velocity as raw f64 bits, state,
//! rotations, camera yaw, timers, tilt, the `hashSimState` value, and for the display-only particles the
//! `includeVisual` hash, their counts, a hash over every field of every particle and the visual random state);
//! the Rust `Sim` is stepped with the same quantized stick feed and compared tick by tick. Environment knobs:
//! `MB_PARITY_STAGES=1,2,50` (stages), `MB_PARITY_FRAMES` (length, default 1500), `MB_PARITY_SCRIPTS=0,1,2,3`
//! (stick scripts tried per stage), `MB_PARITY_AUTOPILOT` (goal runs), and for the effect runs
//! `MB_PARITY_FX_STAGES`, `MB_PARITY_FX_SCRIPTS`, `MB_PARITY_FX_FRAMES`, `MB_PARITY_FX_FAST` (stages run with
//! the stick held forward), `MB_PARITY_FX_GOALS`.

mod common;

use mb_engine::determinism::QuantizedInput;
use mb_engine::{BallEffect, BallEffectKind, ConfettiParticle, Outcome, Phase, Sim, SimConfig};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The headless reference has no course object, so it never treats stages 91-95 as bonus stages; match that.
fn headless_cfg(id: u32, attempt: usize) -> SimConfig {
    let mut cfg = if attempt == 0 {
        SimConfig::first_attempt(id)
    } else {
        SimConfig::retry(id)
    };
    cfg.bonus_stage = false;
    cfg
}

fn deno() -> Option<String> {
    let cmd = std::env::var("DENO").unwrap_or_else(|_| "deno".into());
    Command::new(&cmd)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| cmd)
}

fn reference_src() -> Option<PathBuf> {
    let root = std::env::var_os("MB_REFERENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../WebMonkeyBall")
        });
    let src = root.join("src");
    src.join("game.ts").is_file().then_some(src)
}

fn deno_args(script: &str) -> Vec<String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/ref");
    vec![
        "run".into(),
        "--quiet".into(),
        "--no-prompt".into(),
        "--allow-read".into(),
        "--allow-write".into(),
        "--allow-env".into(),
        "--sloppy-imports".into(),
        "--import-map".into(),
        dir.join("import_map.json").to_string_lossy().into_owned(),
        dir.join(script).to_string_lossy().into_owned(),
    ]
}

struct Env {
    deno: String,
    src: PathBuf,
    disc: mb_assets::disc::DiscPack,
}

fn setup() -> Option<Env> {
    let Some(disc) = common::open_disc() else {
        eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
        return None;
    };
    let Some(deno) = deno() else {
        eprintln!("skipped: deno not found (set DENO)");
        return None;
    };
    let Some(src) = reference_src() else {
        eprintln!("skipped: WebMonkeyBall checkout not found (set MB_REFERENCE)");
        return None;
    };
    if !common::install_real_tables() {
        eprintln!("skipped: could not read the trig tables from main.dol");
        return None;
    }
    Some(Env { deno, src, disc })
}

/// Deterministic stick scripts (quantized -127..=127). Frames before the intro ends are ignored by both sims.
fn script(kind: usize, frames: usize) -> Vec<QuantizedInput> {
    let mut state = 0x1234_5678u32 ^ (kind as u32).wrapping_mul(0x9e37_79b9);
    let mut hold = 0usize;
    let mut cur = (0, 0);
    (0..frames)
        .map(|i| {
            let (x, y) = match kind % 4 {
                // slow figure-eight
                0 => (
                    (127.0 * 0.8 * (i as f64 * 0.021).sin()).round() as i32,
                    (127.0 * 0.8 * (i as f64 * 0.013 + 1.0).sin()).round() as i32,
                ),
                // random holds of random directions
                1 => {
                    if hold == 0 {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        cur = (
                            ((state & 0xff) as i32 - 128).clamp(-127, 127),
                            (((state >> 8) & 0xff) as i32 - 128).clamp(-127, 127),
                        );
                        hold = 10 + (state >> 16) as usize % 50;
                    }
                    hold -= 1;
                    cur
                }
                // push forward, sweep sideways
                2 => (
                    ((i as f64 * 0.05).sin() * 100.0).round() as i32,
                    if (i / 90) % 2 == 0 { -110 } else { 40 },
                ),
                // fast alternating
                _ => (
                    if (i / 7) % 2 == 0 { 127 } else { -127 },
                    ((i as f64 * 0.11).cos() * 90.0).round() as i32,
                ),
            };
            QuantizedInput { x, y }
        })
        .collect()
}

/// The display-only columns of a reference line.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Visual {
    hash: u32,
    effects: usize,
    confetti: usize,
    effects_state: u32,
    confetti_state: u32,
    rng: u32,
}

impl Visual {
    fn parse(f: &[&str]) -> Visual {
        let h = |s: &str| u32::from_str_radix(s, 16).unwrap();
        Visual {
            hash: h(f[0]),
            effects: f[1].parse().unwrap(),
            confetti: f[2].parse().unwrap(),
            effects_state: h(f[3]),
            confetti_state: h(f[4]),
            rng: h(f[5]),
        }
    }

    fn of(sim: &Sim) -> Visual {
        Visual {
            hash: sim.hash_visual(),
            effects: sim.runtime.effects.len(),
            confetti: sim.runtime.confetti.len(),
            effects_state: effects_state(&sim.runtime.effects),
            confetti_state: confetti_state(&sim.runtime.confetti),
            rng: sim.runtime.visual_rng.state,
        }
    }
}

/// The hash `tests/ref/run.ts` computes over the reference's particles: FNV-1a over 32-bit words, integers as
/// they are and floats as the two halves of their f64 bits (high first).
struct Fnv(u32);

impl Fnv {
    fn word(&mut self, v: u32) {
        self.0 = (self.0 ^ v).wrapping_mul(16777619);
    }
    fn int(&mut self, v: i32) {
        self.word(v as u32);
    }
    fn num(&mut self, v: f64) {
        self.word((v.to_bits() >> 32) as u32);
        self.word(v.to_bits() as u32);
    }
    fn vec(&mut self, v: &mb_engine::math::Vec3) {
        self.num(v.x);
        self.num(v.y);
        self.num(v.z);
    }
}

/// Every field of every ball effect except `id`, in the order of `effectsState` in `run.ts`.
fn effects_state(effects: &[BallEffect]) -> u32 {
    let mut h = Fnv(0x811c_9dc5);
    for e in effects {
        h.int(match e.kind {
            BallEffectKind::Coli => 0,
            BallEffectKind::ColiStar => 1,
            BallEffectKind::ColiFlash => 2,
            BallEffectKind::Levitate => 3,
        });
        for v in [
            &e.pos,
            &e.prev_pos,
            &e.ground_pos,
            &e.ground_normal,
            &e.ground_vel,
            &e.glow_pos,
            &e.surface_normal,
            &e.vel,
        ] {
            h.vec(v);
        }
        for v in [
            e.glow_dist,
            e.scale,
            e.scale_target,
            e.alpha,
            e.color_r,
            e.color_g,
            e.color_b,
            e.base_y,
        ] {
            h.num(v);
        }
        for v in [
            e.glow_rot_x,
            e.glow_rot_y,
            e.has_ground as i32,
            e.life,
            e.age,
            e.rot_x,
            e.rot_y,
            e.rot_z,
            e.prev_rot_x,
            e.prev_rot_y,
            e.prev_rot_z,
            e.rot_vel_x,
            e.rot_vel_y,
            e.rot_vel_z,
        ] {
            h.int(v);
        }
    }
    h.0
}

/// Every field of every confetti piece, in the order of `confettiState` in `run.ts`.
fn confetti_state(confetti: &[ConfettiParticle]) -> u32 {
    let mut h = Fnv(0x811c_9dc5);
    for c in confetti {
        h.int(c.model_index as i32);
        for v in [
            &c.pos,
            &c.prev_pos,
            &c.vel,
            &c.ground_pos,
            &c.ground_normal,
            &c.ground_vel,
        ] {
            h.vec(v);
        }
        h.num(c.scale);
        h.num(c.ground_bias);
        for v in [
            c.on_ground as i32,
            c.life,
            c.rot_x,
            c.rot_y,
            c.rot_z,
            c.prev_rot_x,
            c.prev_rot_y,
            c.prev_rot_z,
            c.rot_vel_x,
            c.rot_vel_y,
            c.rot_vel_z,
        ] {
            h.int(v);
        }
    }
    h.0
}

#[derive(Debug, Clone)]
struct RefFrame {
    hash: u32,
    pos: [u64; 3],
    vel: [u64; 3],
    state: i64,
    flags: i64,
    rot: [i64; 3],
    ape_yaw: i64,
    cam_rot_y: i64,
    intro: i64,
    stage_timer: i64,
    goal_timer: i64,
    ringout: i64,
    tilt: [i64; 2],
    visual: Visual,
}

/// One line of the reference's output (see `tests/ref/run.ts`).
#[derive(Debug, Clone)]
enum RefLine {
    Frame(Box<RefFrame>),
    /// The attempt ended and the reference reloaded the stage; the next frame is the next attempt's first.
    Reload,
    /// The reference left the plain gameplay loop on this tick.
    End(String, usize),
    /// The stage runtime alone advanced one tick after the goal sequence.
    Tail(Visual),
}

fn parse_ref(text: &str) -> Vec<RefLine> {
    let mut lines = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "F" => {
                let h = |s: &str| u64::from_str_radix(s, 16).unwrap();
                let n = |s: &str| s.parse::<i64>().unwrap();
                lines.push(RefLine::Frame(Box::new(RefFrame {
                    hash: h(f[2]) as u32,
                    pos: [h(f[3]), h(f[4]), h(f[5])],
                    vel: [h(f[6]), h(f[7]), h(f[8])],
                    state: n(f[9]),
                    flags: n(f[10]),
                    rot: [n(f[11]), n(f[12]), n(f[13])],
                    ape_yaw: n(f[14]),
                    cam_rot_y: n(f[15]),
                    intro: n(f[16]),
                    stage_timer: n(f[17]),
                    goal_timer: n(f[18]),
                    ringout: n(f[19]),
                    tilt: [n(f[20]), n(f[21])],
                    visual: Visual::parse(&f[22..]),
                })));
            }
            "RELOAD" => lines.push(RefLine::Reload),
            "END" => lines.push(RefLine::End(f[1].to_string(), f[2].parse().unwrap())),
            "T" => lines.push(RefLine::Tail(Visual::parse(&f[2..]))),
            _ => {}
        }
    }
    lines
}

/// How the reference is asked to run (the last two arguments of `run.ts`).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    /// As the game runs: a fall-out or a goal ends in a result replay, where the comparison stops.
    Replay,
    /// Result replays off: attempts run to their end and reload; after a goal the stage runtime runs on alone
    /// for this many ticks.
    NoReplay { tail: usize },
}

fn run_reference(
    env: &Env,
    id: u32,
    tag: &str,
    inputs: &[QuantizedInput],
    mode: Mode,
    tmp: &Path,
) -> String {
    let lz = tmp.join(format!("s{id:03}.lz"));
    std::fs::write(&lz, common::stage_lz(&env.disc, id)).unwrap();
    let inp = tmp.join(format!("in{id:03}{tag}.txt"));
    let text: String = inputs
        .iter()
        .map(|i| format!("{} {}\n", i.x, i.y))
        .collect();
    std::fs::write(&inp, text).unwrap();
    let out = tmp.join(format!("out{id:03}{tag}.txt"));
    let mut args = deno_args("run.ts");
    args.extend([
        env.src.to_string_lossy().into_owned(),
        id.to_string(),
        lz.to_string_lossy().into_owned(),
        inp.to_string_lossy().into_owned(),
        inputs.len().to_string(),
        out.to_string_lossy().into_owned(),
    ]);
    if let Mode::NoReplay { tail } = mode {
        args.extend(["noreplay".to_string(), tail.to_string()]);
    }
    let res = Command::new(&env.deno)
        .args(&args)
        .output()
        .expect("spawn deno");
    assert!(
        res.status.success(),
        "reference run failed for stage {id}: {}",
        String::from_utf8_lossy(&res.stderr)
    );
    std::fs::read_to_string(out).unwrap()
}

#[derive(Default, Debug)]
struct Report {
    /// Ticks compared identical before the first divergence (or all of them).
    identical: usize,
    ref_ticks: usize,
    /// Stage-runtime-only ticks compared after a goal sequence.
    tail_ticks: usize,
    /// Attempts the reference ended by reloading the stage, each followed by a fresh Rust `Sim`.
    reloads: usize,
    end: Option<(String, usize)>,
    first_divergence: Option<(usize, String)>,
    /// Which object kinds were actually exercised during the compared ticks.
    coverage: Coverage,
    fx: FxCoverage,
}

#[derive(Default, Debug)]
struct Coverage {
    bumper_hit: bool,
    jamabar_moved: bool,
    banana_collected: bool,
    goal_bag_opened: bool,
    moving_groups: bool,
    ball_airborne_bounce: bool,
}

impl Coverage {
    fn update(&mut self, sim: &Sim) {
        let rt = &sim.runtime;
        self.bumper_hit |= rt.bumpers.iter().flatten().any(|b| b.state != 0);
        self.jamabar_moved |= rt.jamabars.iter().flatten().any(|j| j.local_pos.z != 0.0);
        self.banana_collected |= rt.bananas.iter().any(|b| b.state >= 7 || b.collected);
        self.goal_bag_opened |= rt.goal_bags.iter().any(|b| b.state != 1);
        self.moving_groups |= rt
            .anim_groups
            .iter()
            .skip(1)
            .any(|g| g.rot != [0, 0, 0] || g.pos != g.prev_pos);
        self.ball_airborne_bounce |= sim.ball.unk80 > 0 && sim.ball.flags & 1 != 0;
    }
}

/// Which parts of the particle code the compared ticks went through (all identical to the reference, or the
/// run is reported as diverged).
#[derive(Default, Debug, Clone)]
struct FxCoverage {
    /// Ticks on which at least one ball effect or confetti piece was alive.
    ticks_with_particles: usize,
    peak_effects: usize,
    sparks: bool,
    /// Sparks thrown by rolling fast (a tick that spawned sparks without a collision flash).
    movement_sparks: bool,
    stars: bool,
    flashes: bool,
    sparkles: bool,
    confetti: bool,
    /// A spark or star found ground under it (glow, ground response).
    effect_on_ground: bool,
    /// A star's spin was kicked by a bounce: the one place an update draws from the visual random stream.
    star_bounced: bool,
    /// A ball effect reached the end of its life and was removed.
    effect_expired: bool,
    confetti_on_ground: bool,
    confetti_expired: bool,
    /// A particle found ground that moves (an animated group: transformed normal, surface velocity).
    moving_ground: bool,
    /// Every particle of a goal run was gone at the end of the tail.
    tail_ran_empty: bool,
    /// Effects were alive in an attempt that followed a stage reload.
    effects_after_reload: bool,
    /// The first tick after a reload had no particles and the visual random stream at its seed.
    reset_on_reload: bool,
}

impl FxCoverage {
    fn update(&mut self, sim: &Sim, before: &FxBefore, attempt: usize) {
        let rt = &sim.runtime;
        if !rt.effects.is_empty() || !rt.confetti.is_empty() {
            self.ticks_with_particles += 1;
        }
        self.peak_effects = self.peak_effects.max(rt.effects.len());
        for e in &rt.effects {
            match e.kind {
                BallEffectKind::Coli => self.sparks = true,
                BallEffectKind::ColiStar => self.stars = true,
                BallEffectKind::ColiFlash => self.flashes = true,
                BallEffectKind::Levitate => self.sparkles = true,
            }
            if e.kind != BallEffectKind::ColiFlash && e.has_ground {
                self.effect_on_ground = true;
            }
            if e.kind == BallEffectKind::ColiStar && e.rot_vel_x != 0 {
                self.star_bounced = true;
            }
        }
        let spawned = |kind| rt.effects.iter().any(|e| e.age == 0 && e.kind == kind);
        self.movement_sparks |=
            spawned(BallEffectKind::Coli) && !spawned(BallEffectKind::ColiFlash);
        self.effect_expired |= rt.effects.first().map(|e| e.id) != before.first_effect
            && before.first_effect.is_some();
        let zero = mb_engine::math::Vec3::ZERO;
        self.moving_ground |= rt
            .effects
            .iter()
            .any(|e| e.has_ground && e.ground_vel != zero)
            || rt
                .confetti
                .iter()
                .any(|c| c.on_ground && c.ground_vel != zero);
        self.confetti |= !rt.confetti.is_empty();
        self.confetti_on_ground |= rt.confetti.iter().any(|c| c.on_ground);
        self.confetti_expired |= rt.confetti.len() < before.confetti;
        self.effects_after_reload |= attempt > 0 && !rt.effects.is_empty();
    }
}

/// What a tick's particle coverage is measured against.
struct FxBefore {
    first_effect: Option<u32>,
    confetti: usize,
}

impl FxBefore {
    fn of(sim: &Sim) -> FxBefore {
        FxBefore {
            first_effect: sim.runtime.effects.first().map(|e| e.id),
            confetti: sim.runtime.confetti.len(),
        }
    }
}

fn visual_diff(sim: &Sim, reference: &Visual, diffs: &mut Vec<String>) {
    let rust = Visual::of(sim);
    if rust != *reference {
        diffs.push(format!("visual: rust {rust:x?} ref {reference:x?}"));
    }
}

fn compare(id: u32, stage: &mb_stage::Stage, inputs: &[QuantizedInput], reference: &str) -> Report {
    let lines = parse_ref(reference);
    let mut sim = Sim::new(stage, headless_cfg(id, 0));
    let mut report = Report::default();
    // Index into the stick feed, which the reference restarts with every stage load.
    let mut feed_at = 0;
    for (i, line) in lines.iter().enumerate() {
        let input = |at: usize| inputs.get(at).copied().unwrap_or_default();
        match line {
            RefLine::Frame(r) => {
                let before = FxBefore::of(&sim);
                sim.step(input(feed_at));
                feed_at += 1;
                report.ref_ticks += 1;
                let b = &sim.ball;
                let mut diffs: Vec<String> = Vec::new();
                let mut chk = |name: &str, ok: bool, detail: &dyn Fn() -> String| {
                    if !ok {
                        diffs.push(format!("{name}: {}", detail()));
                    }
                };
                let rb = |a: [u64; 3]| a.map(f64::from_bits);
                let pos = [b.pos.x, b.pos.y, b.pos.z];
                let vel = [b.vel.x, b.vel.y, b.vel.z];
                chk("pos", pos.map(f64::to_bits) == r.pos, &|| {
                    format!("rust {pos:?} ref {:?}", rb(r.pos))
                });
                chk("vel", vel.map(f64::to_bits) == r.vel, &|| {
                    format!("rust {vel:?} ref {:?}", rb(r.vel))
                });
                chk("state", b.state as i64 == r.state, &|| {
                    format!("{} vs {}", b.state, r.state)
                });
                chk("flags", b.flags as i64 == r.flags, &|| {
                    format!("{} vs {}", b.flags, r.flags)
                });
                let rot = [b.rot_x as i64, b.rot_y as i64, b.rot_z as i64];
                chk("rot", rot == r.rot, &|| format!("{rot:?} vs {:?}", r.rot));
                chk("apeYaw", b.ape_yaw as i64 == r.ape_yaw, &|| {
                    format!("{} vs {}", b.ape_yaw, r.ape_yaw)
                });
                chk("camRotY", sim.camera.rot_y as i64 == r.cam_rot_y, &|| {
                    format!("{} vs {}", sim.camera.rot_y, r.cam_rot_y)
                });
                chk("intro", sim.intro_timer as i64 == r.intro, &|| {
                    format!("{} vs {}", sim.intro_timer, r.intro)
                });
                chk(
                    "stageTimer",
                    sim.stage_timer_frames as i64 == r.stage_timer,
                    &|| format!("{} vs {}", sim.stage_timer_frames, r.stage_timer),
                );
                chk("goalTimer", sim.goal_timer as i64 == r.goal_timer, &|| {
                    format!("{} vs {}", sim.goal_timer, r.goal_timer)
                });
                chk("ringout", sim.ringout_timer as i64 == r.ringout, &|| {
                    format!("{} vs {}", sim.ringout_timer, r.ringout)
                });
                let tilt = [sim.world.xrot as i64, sim.world.zrot as i64];
                chk("tilt", tilt == r.tilt, &|| {
                    format!("{tilt:?} vs {:?}", r.tilt)
                });
                let hash = sim.hash();
                chk("hash", hash == r.hash, &|| {
                    format!("{hash:08x} vs {:08x}", r.hash)
                });
                visual_diff(&sim, &r.visual, &mut diffs);
                if !diffs.is_empty() {
                    report.first_divergence = Some((i, diffs.join("; ")));
                    return report;
                }
                report.identical += 1;
                report.coverage.update(&sim);
                report.fx.update(&sim, &before, report.reloads);
                if report.reloads > 0 && feed_at == 1 {
                    report.fx.reset_on_reload = r.visual.effects == 0
                        && r.visual.confetti == 0
                        && sim.runtime.visual_rng.state
                            == Sim::new(stage, headless_cfg(id, 1))
                                .runtime
                                .visual_rng
                                .state;
                }
            }
            RefLine::Reload => {
                // The attempt ended on this tick in the reference (fall-out or time over); the Rust sim must
                // end on the same tick, and the next attempt starts from a fresh `Sim` as the reference starts
                // from a fresh `StageRuntime`.
                sim.step(input(feed_at));
                if !matches!(
                    sim.phase,
                    Phase::Finished(Outcome::FellOut | Outcome::TimeOver)
                ) {
                    report.first_divergence = Some((
                        i,
                        format!("reference reloaded the stage but Rust is {:?}", sim.phase),
                    ));
                    return report;
                }
                report.reloads += 1;
                // The reference keeps its ball object across the reload and `initBallForStage` leaves
                // `speed` as the last step set it. Nothing reads it before the next ball step computes it
                // again, but it is part of the hash, so carry it over like the reference does.
                let stale_speed = sim.ball.speed;
                sim = Sim::new(stage, headless_cfg(id, report.reloads));
                sim.ball.speed = stale_speed;
                feed_at = 0;
            }
            RefLine::End(reason, at) => {
                // The reference left the plain gameplay loop on this tick (it starts a result replay, reloads
                // the stage or ends the goal sequence); the Rust sim must be at the matching point of the same
                // sequence.
                sim.step(input(feed_at));
                report.end = Some((reason.clone(), *at));
                let ok = match reason.as_str() {
                    // goal sequence over, or the fall-out sequence 30 ticks in
                    "result-replay" => {
                        matches!(sim.phase, Phase::Finished(Outcome::Goal(_)))
                            || (sim.phase == Phase::FallOut && sim.ringout_timer == 240)
                    }
                    "goal-finished" => matches!(sim.phase, Phase::Finished(Outcome::Goal(_))),
                    _ => sim.is_finished(),
                };
                if !ok {
                    report.first_divergence = Some((
                        i,
                        format!(
                            "reference ended at {at} ({reason}) but Rust is {:?} (ringout {}, goal {})",
                            sim.phase, sim.ringout_timer, sim.goal_timer
                        ),
                    ));
                    return report;
                }
            }
            RefLine::Tail(visual) => {
                let before = FxBefore::of(&sim);
                let gravity = sim.shared_world.gravity;
                sim.runtime.advance(&sim.geom, gravity, None, None, None);
                let mut diffs = Vec::new();
                visual_diff(&sim, visual, &mut diffs);
                if !diffs.is_empty() {
                    report.first_divergence = Some((i, format!("tail: {}", diffs.join("; "))));
                    return report;
                }
                report.tail_ticks += 1;
                report.fx.update(&sim, &before, report.reloads);
                report.fx.tail_ran_empty =
                    sim.runtime.effects.is_empty() && sim.runtime.confetti.is_empty();
            }
        }
    }
    report
}

fn env_list(name: &str, default: &[usize]) -> Vec<usize> {
    std::env::var(name)
        .ok()
        .map(|s| s.split(',').filter_map(|p| p.trim().parse().ok()).collect())
        .unwrap_or_else(|| default.to_vec())
}

/// Stick script produced by the closed-loop waypoint controller driving the Rust sim (bananas, then the goal).
fn autopilot_script(stage: &mb_stage::Stage, id: u32, frames: usize) -> Vec<QuantizedInput> {
    let mut ap = common::Autopilot::for_stage(stage);
    let mut sim = Sim::new(stage, headless_cfg(id, 0));
    let mut out = Vec::with_capacity(frames);
    for _ in 0..frames {
        let input = if sim.intro_timer <= 0 && !sim.is_finished() {
            ap.input(&sim)
        } else {
            QuantizedInput::default()
        };
        out.push(input);
        sim.step(input);
    }
    out
}

#[test]
fn tables_match_reference() {
    let Some(env) = setup() else { return };
    let mut args = deno_args("tables.ts");
    args.push(env.src.to_string_lossy().into_owned());
    let out = Command::new(&env.deno)
        .args(&args)
        .output()
        .expect("spawn deno");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout);
    let dol = common::read_main_dol().unwrap();
    let t = mb_engine::Tables::from_dol(&dol).unwrap();
    let fnv = |v: &[f32]| {
        let mut h = 0x811c_9dc5u32;
        for f in v {
            h = (h ^ f.to_bits()).wrapping_mul(16777619);
        }
        format!("{h:08x}")
    };
    assert_eq!(text.trim(), format!("{} {}", fnv(&t.sin), fnv(&t.atan)));
}

/// Prints one run's result and returns whether it diverged.
fn diverged(what: &str, report: &Report) -> bool {
    match &report.first_divergence {
        None => {
            eprintln!(
                "{what}: identical for {} ticks (reference ran {} ticks, {} reloads, {} tail ticks, end {:?}) {:?} {:?}",
                report.identical,
                report.ref_ticks,
                report.reloads,
                report.tail_ticks,
                report.end,
                report.coverage,
                report.fx
            );
            false
        }
        Some((line, msg)) => {
            eprintln!(
                "{what}: DIVERGES at line {line} after {} identical ticks: {msg}",
                report.identical
            );
            true
        }
    }
}

#[test]
fn matches_reference_game_loop() {
    let Some(env) = setup() else { return };
    let frames = env_list("MB_PARITY_FRAMES", &[1500])[0];
    let tmp = std::env::temp_dir().join("mb-engine-parity");
    std::fs::create_dir_all(&tmp).unwrap();
    let mut failures = Vec::new();
    for id in env_list("MB_PARITY_STAGES", &[1, 2, 10, 50]) {
        let id = id as u32;
        let stage = common::load_stage(&env.disc, id);
        for kind in env_list("MB_PARITY_SCRIPTS", &[0, 1]) {
            let inputs = script(kind, frames);
            let tag = format!("k{kind}");
            let reference = run_reference(&env, id, &tag, &inputs, Mode::Replay, &tmp);
            let report = compare(id, &stage, &inputs, &reference);
            if diverged(&format!("stage {id:03} script {kind}"), &report) {
                failures.push((id, kind));
            }
        }
    }
    // Goal runs: the closed-loop controller collects bananas and enters the goal, exercising banana pickup and
    // fly-to-HUD, goal tape/bag and the goal sequence.
    for id in env_list("MB_PARITY_AUTOPILOT", &[1, 3, 11, 17]) {
        let id = id as u32;
        let stage = common::load_stage(&env.disc, id);
        let inputs = autopilot_script(&stage, id, frames.max(3000));
        let reference = run_reference(&env, id, "ap", &inputs, Mode::Replay, &tmp);
        let report = compare(id, &stage, &inputs, &reference);
        if diverged(&format!("stage {id:03} autopilot"), &report) {
            failures.push((id, 100));
        }
    }
    // Idle run to the time limit: exercises the time-over sequence and the sequence ends in lock step.
    {
        let id = 1;
        let stage = common::load_stage(&env.disc, id);
        let inputs = vec![QuantizedInput::default(); 4400];
        let reference = run_reference(&env, id, "idle", &inputs, Mode::Replay, &tmp);
        let report = compare(id, &stage, &inputs, &reference);
        if diverged(&format!("stage {id:03} idle/time-over"), &report) {
            failures.push((id, 200));
        }
        assert_eq!(
            report.end.as_ref().map(|e| e.0.as_str()),
            Some("reload-or-advance"),
            "time over should reload the stage in the reference"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        failures.is_empty(),
        "diverged on (stage, script): {failures:?}"
    );
}

/// The display-only particles against the reference, on runs chosen to produce them: hard pushes (collision
/// stars, flashes and sparks), the stick held forward on stages that slope away from the start or move under
/// the ball (the sparks of fast rolling, by the dozen; ground that carries particles along), fall-outs that run to their end and are retried on a reloaded stage, and goal
/// runs (confetti, post-goal sparkles) whose particles are followed until the last one is gone. Every tick
/// compares the `includeVisual` hash, both counts, a hash over every field of every particle and the visual
/// random state, next to everything `matches_reference_game_loop` compares.
#[test]
fn effects_match_reference() {
    let Some(env) = setup() else { return };
    let frames = env_list("MB_PARITY_FX_FRAMES", &[1800])[0];
    let tmp = std::env::temp_dir().join("mb-engine-parity-fx");
    std::fs::create_dir_all(&tmp).unwrap();
    let mut failures = Vec::new();
    let mut seen = FxCoverage::default();
    let mut merge = |fx: &FxCoverage| {
        seen.ticks_with_particles += fx.ticks_with_particles;
        seen.peak_effects = seen.peak_effects.max(fx.peak_effects);
        seen.sparks |= fx.sparks;
        seen.movement_sparks |= fx.movement_sparks;
        seen.stars |= fx.stars;
        seen.flashes |= fx.flashes;
        seen.sparkles |= fx.sparkles;
        seen.confetti |= fx.confetti;
        seen.effect_on_ground |= fx.effect_on_ground;
        seen.star_bounced |= fx.star_bounced;
        seen.effect_expired |= fx.effect_expired;
        seen.confetti_on_ground |= fx.confetti_on_ground;
        seen.confetti_expired |= fx.confetti_expired;
        seen.moving_ground |= fx.moving_ground;
        seen.tail_ran_empty |= fx.tail_ran_empty;
        seen.effects_after_reload |= fx.effects_after_reload;
        seen.reset_on_reload |= fx.reset_on_reload;
    };
    let mut reloads = 0;
    for id in env_list("MB_PARITY_FX_STAGES", &[1, 2, 10, 50]) {
        let id = id as u32;
        let stage = common::load_stage(&env.disc, id);
        for kind in env_list("MB_PARITY_FX_SCRIPTS", &[2, 3]) {
            let inputs = script(kind, frames);
            let tag = format!("fx{kind}");
            let mode = Mode::NoReplay { tail: 0 };
            let reference = run_reference(&env, id, &tag, &inputs, mode, &tmp);
            let report = compare(id, &stage, &inputs, &reference);
            if diverged(&format!("stage {id:03} script {kind}, no replay"), &report) {
                failures.push((id, kind));
            }
            reloads += report.reloads;
            merge(&report.fx);
        }
    }
    for id in env_list("MB_PARITY_FX_FAST", &[36, 77, 95, 126, 133, 151, 197]) {
        let id = id as u32;
        let stage = common::load_stage(&env.disc, id);
        let inputs = vec![QuantizedInput { x: 0, y: -127 }; frames.max(2400)];
        let mode = Mode::NoReplay { tail: 400 };
        let reference = run_reference(&env, id, "fxfast", &inputs, mode, &tmp);
        let report = compare(id, &stage, &inputs, &reference);
        if diverged(&format!("stage {id:03} held forward, no replay"), &report) {
            failures.push((id, 300));
        }
        reloads += report.reloads;
        merge(&report.fx);
    }
    for id in env_list("MB_PARITY_FX_GOALS", &[1, 3, 11, 17]) {
        let id = id as u32;
        let stage = common::load_stage(&env.disc, id);
        let inputs = autopilot_script(&stage, id, 3000);
        let mode = Mode::NoReplay { tail: 400 };
        let reference = run_reference(&env, id, "fxap", &inputs, mode, &tmp);
        let report = compare(id, &stage, &inputs, &reference);
        if diverged(&format!("stage {id:03} autopilot, no replay"), &report) {
            failures.push((id, 100));
        }
        assert_eq!(
            report.end.as_ref().map(|e| e.0.as_str()),
            Some("goal-finished"),
            "stage {id}: the autopilot run should end in a goal"
        );
        merge(&report.fx);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    eprintln!("particles compared: {seen:?}, {reloads} reloads");
    assert!(
        failures.is_empty(),
        "diverged on (stage, script): {failures:?}"
    );
    // With the default runs every part of the particle code must have been reached, or the comparison above
    // proved less than it says.
    if std::env::vars().any(|(k, _)| k.starts_with("MB_PARITY_FX_")) {
        return;
    }
    assert!(seen.sparks && seen.movement_sparks && seen.stars && seen.flashes);
    assert!(seen.sparkles && seen.confetti);
    assert!(seen.effect_on_ground && seen.star_bounced && seen.effect_expired);
    assert!(seen.moving_ground);
    assert!(seen.confetti_on_ground && seen.confetti_expired && seen.tail_ran_empty);
    assert!(reloads > 0 && seen.effects_after_reload && seen.reset_on_reload);
}
