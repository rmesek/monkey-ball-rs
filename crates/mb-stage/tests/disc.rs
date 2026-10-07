//! Real-disc tests: parse every `STAGE*.lz` on a local SMB1 image. Skipped when no image is found
//! (set `MB_DISC`, or put `Super Monkey Ball*.rvz|iso` next to this repository).
//!
//! The `EXPECTED` table was produced by running the reference `StageParser` (WebMonkeyBall `src/stage.ts`)
//! under Deno on the same decompressed stagedefs. `hash` is FNV-1a (32-bit) over every collision triangle's
//! fields (f32 bit patterns, rot as u16, flags, in file order) followed by each grid cell's length + indices.

use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use mb_stage::{parse_stagedef_smb1, Stage};
use std::path::PathBuf;

fn find_disc() -> Option<PathBuf> {
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

macro_rules! disc_or_skip {
    () => {
        match find_disc() {
            Some(p) => DiscPack::open(p).expect("open disc"),
            None => {
                eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
                return;
            }
        }
    };
}

fn stagedef_paths(disc: &DiscPack) -> Vec<String> {
    let mut v: Vec<String> = disc
        .list()
        .into_iter()
        .filter(|f| {
            f.starts_with("test/st")
                && f.rsplit('/')
                    .next()
                    .is_some_and(|n| n.starts_with("STAGE") && n.ends_with(".lz"))
        })
        .map(String::from)
        .collect();
    v.sort();
    v
}

fn load(disc: &DiscPack, path: &str) -> Vec<u8> {
    let raw = disc.fetch(path).unwrap();
    lzss::decompress(&raw).unwrap_or_else(|e| panic!("{path}: {e}"))
}

struct Fnv(u32);
impl Fnv {
    fn w(&mut self, x: u32) {
        self.0 = (self.0 ^ x).wrapping_mul(16777619);
    }
    fn f(&mut self, x: f32) {
        self.w(x.to_bits());
    }
}

fn tri_hash(s: &Stage) -> u32 {
    let mut h = Fnv(2166136261);
    for g in &s.anim_groups {
        for t in &g.triangles {
            for v in [t.pos, t.normal] {
                h.f(v.x);
                h.f(v.y);
                h.f(v.z);
            }
            h.w(t.rot.x as u16 as u32);
            h.w(t.rot.y as u16 as u32);
            h.w(t.rot.z as u16 as u32);
            h.w(t.flags as u32);
            for v in [t.vert2, t.vert3, t.edge2_normal, t.edge3_normal] {
                h.f(v.x);
                h.f(v.y);
            }
        }
        for c in &g.grid.cells {
            h.w(c.len() as u32);
            for &i in c {
                h.w(i as u32);
            }
        }
    }
    h.0
}

/// (id, groups, tris, goals, bumpers, jamabars, bananas, starts, bg, fg, fallout_y, hash)
#[allow(clippy::type_complexity)]
const EXPECTED: &[(
    &str,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    f32,
    u32,
)] = &[
    ("001", 1, 324, 1, 0, 0, 4, 1, 20, 24, -0.5, 1523245220),
    ("002", 1, 418, 2, 0, 0, 3, 1, 20, 31, -9.0, 3070319167),
    ("010", 1, 2297, 1, 0, 0, 7, 1, 0, 0, -20.0, 2519013447),
    ("024", 1, 66, 1, 0, 30, 4, 1, 23, 0, -1.5, 3078490002),
    ("025", 1, 692, 1, 0, 0, 5, 1, 23, 0, -5.0, 3388279801),
    ("035", 1, 1811, 2, 9, 4, 3, 1, 68, 0, -5.0, 3815041329),
    ("050", 1, 2864, 1, 0, 0, 5, 1, 0, 0, -44.0, 1884102126),
    ("100", 16, 4968, 2, 8, 0, 3, 1, 0, 0, -1.0, 3012265675),
    ("150", 1, 158, 1, 1, 0, 2, 1, 14, 54, -2.0, 1310541269),
    ("200", 1, 2, 1, 0, 0, 0, 1, 0, 0, -2.0, 498730442),
];

#[test]
fn parses_every_stagedef_with_sane_offsets_and_indices() {
    let disc = disc_or_skip!();
    let paths = stagedef_paths(&disc);
    assert!(!paths.is_empty(), "no stagedefs found");
    let mut jamabar_stages = 0;
    for path in &paths {
        let s = parse_stagedef_smb1(&load(&disc, path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(!s.anim_groups.is_empty(), "{path}: no anim groups");
        assert!(!s.start_positions.is_empty(), "{path}: no start position");
        assert!(s.fallout_y.is_finite(), "{path}");
        let (mut goals, mut bumpers, mut jam, mut ban) = (0, 0, 0, 0);
        for g in &s.anim_groups {
            let n = g.grid.cell_count_x.max(0) as usize * g.grid.cell_count_z.max(0) as usize;
            assert_eq!(g.grid.cells.len(), n, "{path}: grid size");
            for c in &g.grid.cells {
                for &i in c {
                    assert!(
                        (i as usize) < g.triangles.len(),
                        "{path}: tri index {i} out of range"
                    );
                }
            }
            for t in &g.triangles {
                for v in [
                    t.pos.x, t.pos.y, t.pos.z, t.normal.x, t.normal.y, t.normal.z,
                ] {
                    assert!(v.is_finite(), "{path}: non-finite triangle value");
                }
            }
            goals += g.goals.len();
            bumpers += g.bumpers.len();
            jam += g.jamabars.len();
            ban += g.bananas.len();
        }
        // Per-group lists are the stage-wide lists split between groups.
        assert_eq!(goals, s.goals.len(), "{path}: goals");
        assert_eq!(bumpers, s.bumpers.len(), "{path}: bumpers");
        assert_eq!(jam, s.jamabars.len(), "{path}: jamabars");
        assert_eq!(ban, s.bananas.len(), "{path}: bananas");
        if !s.jamabars.is_empty() {
            jamabar_stages += 1;
            eprintln!("jamabars in {path}: {}", s.jamabars.len());
        }
    }
    eprintln!(
        "{} stagedefs parsed, {jamabar_stages} with jamabars",
        paths.len()
    );
}

#[test]
fn matches_reference_parser_counts_and_triangle_checksum() {
    let disc = disc_or_skip!();
    for &(id, groups, tris, goals, bumpers, jam, bananas, starts, bg, fg, fy, hash) in EXPECTED {
        let path = format!("test/st{id}/STAGE{id}.lz");
        let s = parse_stagedef_smb1(&load(&disc, &path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let got_tris: usize = s.anim_groups.iter().map(|g| g.triangles.len()).sum();
        assert_eq!(s.anim_groups.len(), groups, "{id} groups");
        assert_eq!(got_tris, tris, "{id} tris");
        assert_eq!(s.goals.len(), goals, "{id} goals");
        assert_eq!(s.bumpers.len(), bumpers, "{id} bumpers");
        assert_eq!(s.jamabars.len(), jam, "{id} jamabars");
        assert_eq!(s.bananas.len(), bananas, "{id} bananas");
        assert_eq!(s.start_positions.len(), starts, "{id} starts");
        assert_eq!(s.bg_objects.len(), bg, "{id} bg");
        assert_eq!(s.fg_objects.len(), fg, "{id} fg");
        assert_eq!(s.fallout_y, fy, "{id} fallout");
        assert_eq!(tri_hash(&s), hash, "{id} triangle/grid checksum");
    }
}
