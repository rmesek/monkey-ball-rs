//! Parity of the Rust simulation against the real WebMonkeyBall `Game` run headless under Deno.
//!
//! Skips cleanly when any of the following is missing: a Super Monkey Ball 1 disc image (`MB_DISC` or next to
//! the repository), `deno` (`DENO` env or PATH) or the reference checkout (`MB_REFERENCE` or `../WebMonkeyBall`
//! next to the repository). The reference is only read; its game logic runs unmodified (DOM/WebGL globals and
//! `gl-matrix`/`fflate` are stubbed, see `tests/ref/`). Disc data is staged in the OS temp dir for the Deno
//! process and removed afterwards.
//!
//! For each scripted stage the reference writes one line per tick (ball position/velocity as raw f64 bits, state,
//! rotations, camera yaw, timers, tilt and the `hashSimState` value); the Rust `Sim` is stepped with the same
//! quantized stick feed and compared tick by tick. Environment knobs: `MB_PARITY_STAGES=1,2,50` (stages),
//! `MB_PARITY_FRAMES` (length, default 1500), `MB_PARITY_SCRIPTS=0,1,2,3` (stick scripts tried per stage).

mod common;

use mb_engine::determinism::QuantizedInput;
use mb_engine::{Outcome, Phase, Sim, SimConfig};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The headless reference has no course object, so it never treats stages 91-95 as bonus stages; match that.
fn headless_cfg(id: u32) -> SimConfig {
    let mut cfg = SimConfig::first_attempt(id);
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
}

fn parse_ref(text: &str) -> (Vec<RefFrame>, Option<(String, usize)>) {
    let mut frames = Vec::new();
    let mut end = None;
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "F" => {
                let h = |s: &str| u64::from_str_radix(s, 16).unwrap();
                let n = |s: &str| s.parse::<i64>().unwrap();
                frames.push(RefFrame {
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
                });
            }
            "END" => end = Some((f[1].to_string(), f[2].parse().unwrap())),
            _ => {}
        }
    }
    (frames, end)
}

fn run_reference(env: &Env, id: u32, tag: &str, inputs: &[QuantizedInput], tmp: &Path) -> String {
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
    end: Option<(String, usize)>,
    first_divergence: Option<(usize, String)>,
    /// Which object kinds were actually exercised during the compared ticks.
    coverage: Coverage,
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

fn compare(id: u32, stage: &mb_stage::Stage, inputs: &[QuantizedInput], reference: &str) -> Report {
    let (frames, end) = parse_ref(reference);
    let mut sim = Sim::new(stage, headless_cfg(id));
    let mut report = Report {
        ref_ticks: frames.len(),
        end: end.clone(),
        ..Report::default()
    };
    for (i, input) in inputs.iter().enumerate() {
        sim.step(*input);
        if i >= frames.len() {
            // The reference left the plain gameplay loop on this tick (it starts a result replay or reloads the
            // stage); the Rust sim must be at the matching point of the same sequence.
            if let Some((reason, at)) = &end {
                let ok = match reason.as_str() {
                    // goal sequence over, or the fall-out sequence 30 ticks in
                    "result-replay" => {
                        matches!(sim.phase, Phase::Finished(Outcome::Goal(_)))
                            || (sim.phase == Phase::FallOut && sim.ringout_timer == 240)
                    }
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
                }
            }
            return report;
        }
        let r = &frames[i];
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
        if !diffs.is_empty() {
            report.first_divergence = Some((i, diffs.join("; ")));
            return report;
        }
        report.identical += 1;
        report.coverage.update(&sim);
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
    let mut sim = Sim::new(stage, headless_cfg(id));
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
            let reference = run_reference(&env, id, &tag, &inputs, &tmp);
            let report = compare(id, &stage, &inputs, &reference);
            match &report.first_divergence {
                None => eprintln!(
                    "stage {id:03} script {kind}: identical for {} ticks (reference ran {} ticks, end {:?}) {:?}",
                    report.identical, report.ref_ticks, report.end, report.coverage
                ),
                Some((i, msg)) => {
                    eprintln!(
                        "stage {id:03} script {kind}: DIVERGES at tick {i} after {} identical: {msg}",
                        report.identical
                    );
                    failures.push((id, kind));
                }
            }
        }
    }
    // Goal runs: the closed-loop controller collects bananas and enters the goal, exercising banana pickup and
    // fly-to-HUD, goal tape/bag and the goal sequence.
    for id in env_list("MB_PARITY_AUTOPILOT", &[1, 3, 11, 17]) {
        let id = id as u32;
        let stage = common::load_stage(&env.disc, id);
        let inputs = autopilot_script(&stage, id, frames.max(3000));
        let reference = run_reference(&env, id, "ap", &inputs, &tmp);
        let report = compare(id, &stage, &inputs, &reference);
        match &report.first_divergence {
            None => eprintln!(
                "stage {id:03} autopilot: identical for {} ticks (reference ran {} ticks, end {:?}) {:?}",
                report.identical, report.ref_ticks, report.end, report.coverage
            ),
            Some((i, msg)) => {
                eprintln!(
                    "stage {id:03} autopilot: DIVERGES at tick {i} after {} identical: {msg}",
                    report.identical
                );
                failures.push((id, 100));
            }
        }
    }
    // Idle run to the time limit: exercises the time-over sequence and the sequence ends in lock step.
    {
        let id = 1;
        let stage = common::load_stage(&env.disc, id);
        let inputs = vec![QuantizedInput::default(); 4400];
        let reference = run_reference(&env, id, "idle", &inputs, &tmp);
        let report = compare(id, &stage, &inputs, &reference);
        match &report.first_divergence {
            None => eprintln!(
                "stage {id:03} idle/time-over: identical for {} ticks (reference ran {} ticks, end {:?})",
                report.identical, report.ref_ticks, report.end
            ),
            Some((i, msg)) => {
                eprintln!("stage {id:03} idle/time-over: DIVERGES at tick {i}: {msg}");
                failures.push((id, 200));
            }
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
