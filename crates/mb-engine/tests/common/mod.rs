//! Shared helpers for the disc-backed tests: locate the user's disc image, load stagedefs and the game's
//! trig tables. Everything skips cleanly when no image is present. Nothing read from the disc is written into
//! the repository.
#![allow(dead_code)]

use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use mb_engine::tables::Tables;
use mb_stage::{parse_stagedef_smb1, Stage};
use std::path::PathBuf;
use std::sync::OnceLock;

pub fn find_disc_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MB_DISC") {
        return Some(PathBuf::from(p));
    }
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for entry in std::fs::read_dir(parent).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("super monkey ball")
            && (name.ends_with(".rvz") || name.ends_with(".iso") || name.ends_with(".gcm"))
        {
            let disc = DiscPack::open(entry.path()).ok()?;
            if disc.smb1_region().is_some() {
                return Some(entry.path());
            }
        }
    }
    None
}

pub fn open_disc() -> Option<DiscPack> {
    DiscPack::open(find_disc_path()?).ok()
}

/// The raw (still LZSS-compressed) `STAGENNN.lz` bytes.
pub fn stage_lz(disc: &DiscPack, id: u32) -> Vec<u8> {
    disc.fetch(&format!("test/st{id:03}/STAGE{id:03}.lz"))
        .unwrap_or_else(|e| panic!("stage {id}: {e}"))
}

pub fn load_stage(disc: &DiscPack, id: u32) -> Stage {
    let raw = stage_lz(disc, id);
    let data = lzss::decompress(&raw).unwrap_or_else(|e| panic!("stage {id}: {e}"));
    parse_stagedef_smb1(&data).unwrap_or_else(|e| panic!("stage {id}: {e}"))
}

/// Read `main.dol` from the disc image (it lives outside the file system table).
pub fn read_main_dol() -> Option<Vec<u8>> {
    use nod::common::PartitionKind;
    use nod::read::{DiscOptions, DiscReader, PartitionOptions};
    let disc = DiscReader::new(find_disc_path()?, &DiscOptions::default()).ok()?;
    let mut part = disc
        .open_partition_kind(PartitionKind::Data, &PartitionOptions::default())
        .ok()?;
    let meta = part.meta().ok()?;
    Some(meta.raw_dol.to_vec())
}

static INSTALLED: OnceLock<bool> = OnceLock::new();

/// Install the game's real trig tables for this test process. Returns false (and leaves the computed
/// approximation in place) if they cannot be read.
pub fn install_real_tables() -> bool {
    *INSTALLED.get_or_init(|| {
        let Some(dol) = read_main_dol() else {
            return false;
        };
        let Some(tables) = Tables::from_dol(&dol) else {
            return false;
        };
        mb_engine::install_tables(tables).is_ok()
    })
}

/// A crude closed-loop controller that steers the ball through a list of XZ waypoints (bananas, then the
/// goal). Used to produce stick scripts that actually reach bananas/goals. The Rust sim is deterministic, so a
/// script recorded here can be replayed verbatim on the reference.
pub struct Autopilot {
    pub targets: Vec<mb_engine::math::Vec3>,
    pub next: usize,
    pub speed: f64,
    pub gain: f64,
}

impl Autopilot {
    /// Bananas of group 0 (nearest first from the start), then the first goal.
    pub fn for_stage(stage: &Stage) -> Autopilot {
        use mb_engine::math::Vec3;
        let g = &stage.anim_groups[0];
        let mut cur = stage.start_positions[0].pos;
        let mut left: Vec<Vec3> = g
            .bananas
            .iter()
            .map(|b| Vec3::new(b.pos.x as f64, b.pos.y as f64, b.pos.z as f64))
            .collect();
        let mut targets = Vec::new();
        while !left.is_empty() {
            let d = |p: &Vec3| ((p.x - cur.x as f64).powi(2) + (p.z - cur.z as f64).powi(2)).sqrt();
            let (i, _) = left
                .iter()
                .enumerate()
                .min_by(|a, b| d(a.1).partial_cmp(&d(b.1)).unwrap())
                .unwrap();
            let p = left.remove(i);
            cur = mb_stage::Vec3 {
                x: p.x as f32,
                y: p.y as f32,
                z: p.z as f32,
            };
            targets.push(p);
        }
        if let Some(goal) = g.goals.first() {
            // Aim a few units beyond the goal so the ball crosses its plane at speed.
            let (gx, gz) = (goal.pos.x as f64, goal.pos.z as f64);
            let (px, pz) = targets.last().map(|t| (t.x, t.z)).unwrap_or((
                stage.start_positions[0].pos.x as f64,
                stage.start_positions[0].pos.z as f64,
            ));
            let (dx, dz) = (gx - px, gz - pz);
            let len = (dx * dx + dz * dz).sqrt().max(1e-6);
            targets.push(Vec3::new(
                gx + dx / len * 3.0,
                goal.pos.y as f64,
                gz + dz / len * 3.0,
            ));
        }
        Autopilot {
            targets,
            next: 0,
            speed: 0.15,
            gain: 40.0,
        }
    }

    pub fn input(&mut self, sim: &mb_engine::Sim) -> mb_engine::QuantizedInput {
        let b = &sim.ball;
        while self.next + 1 < self.targets.len() {
            let t = &self.targets[self.next];
            if ((t.x - b.pos.x).powi(2) + (t.z - b.pos.z).powi(2)).sqrt() < 1.2 {
                self.next += 1;
            } else {
                break;
            }
        }
        let Some(target) = self.targets.get(self.next) else {
            return mb_engine::QuantizedInput::default();
        };
        let (dx, dz) = (target.x - b.pos.x, target.z - b.pos.z);
        let dist = (dx * dx + dz * dz).sqrt().max(1e-6);
        let speed = if self.next + 1 == self.targets.len() {
            self.speed
        } else {
            self.speed.min(dist * 0.08)
        };
        let want = (dx / dist * speed, dz / dist * speed);
        let err = (want.0 - b.vel.x, want.1 - b.vel.z);
        let theta = sim.camera.rot_y as f64 * std::f64::consts::PI / 32768.0;
        let forward = (-theta.sin(), -theta.cos());
        let right = (theta.cos(), -theta.sin());
        let af = err.0 * forward.0 + err.1 * forward.1;
        let ar = err.0 * right.0 + err.1 * right.1;
        let q = |v: f64| (v * self.gain * 127.0).round().clamp(-127.0, 127.0) as i32;
        mb_engine::QuantizedInput {
            x: q(ar),
            y: q(-af),
        }
    }
}
