//! The stagedef parser on hostile input: it must return an error (or a stage), never panic, and never
//! allocate more than a small multiple of the input. Inputs are built in code; the last test repeats the
//! campaign on stagedefs read from the user's disc (in memory only) and skips without one.

mod common;
use common::*;
use mb_stage::{parse_stagedef_smb1, Stage, StageError};

#[global_allocator]
static ALLOC: CountingAlloc = CountingAlloc;

/// Heap a parse may use: the parser's own budget is 24 bytes per input byte plus 64 KiB; `Vec` growth can
/// double that.
fn heap_limit(len: usize) -> usize {
    len * 48 + (1 << 18)
}

fn parse_checked(what: &str, bytes: &[u8]) -> Result<Stage, StageError> {
    let (r, heap) = run_guarded(what, bytes, parse_stagedef_smb1);
    assert!(
        heap <= heap_limit(bytes.len()),
        "{what}: {heap} bytes of heap for {} bytes of input",
        bytes.len()
    );
    r
}

struct Img(Vec<u8>);

impl Img {
    fn u32(&mut self, o: usize, v: u32) {
        self.0[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn u16(&mut self, o: usize, v: u16) {
        self.0[o..o + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, o: usize, v: f32) {
        self.u32(o, v.to_bits());
    }
    /// `(count, pointer)` pair at `o`.
    fn list(&mut self, o: usize, count: u32, ptr: u32) {
        self.u32(o, count);
        self.u32(o + 4, ptr);
    }
}

/// A stagedef that exercises every list the parser reads: two anim groups (grid, triangles, animation,
/// model names and every per-group list), stage-wide lists, background objects with animation and flipbooks,
/// mirror names.
fn synthetic() -> Vec<u8> {
    let mut m = Img(vec![0; 0xa00]);
    m.u32(0x04, 600);
    m.list(0x08, 2, 0x100);
    m.u32(0x10, 0x300);
    m.u32(0x14, 0x328);
    m.list(0x18, 1, 0x340);
    m.list(0x28, 1, 0x360);
    m.list(0x30, 1, 0x380);
    m.list(0x38, 2, 0x3a0);
    m.list(0x40, 1, 0x3c0);
    m.list(0x48, 1, 0x3e0);
    m.list(0x50, 1, 0x400);
    m.list(0x58, 1, 0x420);
    m.list(0x68, 1, 0x440);
    m.list(0x70, 1, 0x480);
    m.list(0x80, 1, 0x4c0);
    m.f32(0x300, 1.0);
    m.f32(0x328, -20.0);
    m.f32(0x340, 4.0);
    m.u16(0x352, 0x0047);
    m.u32(0x424, 0x4d0); // model name
    m.0[0x4d0..0x4d7].copy_from_slice(b"MODEL_A");
    m.u32(0x4c0, 0x4d0); // mirror name
    for bg in [0x440, 0x480] {
        m.u32(bg + 4, 0x4d0);
        m.u32(bg + 0x30, 0x500); // anim
        m.u32(bg + 0x34, 0x560); // flipbooks
    }
    // bg anim: eleven tracks, the first with two keyframes
    m.list(0x508, 2, 0x580);
    m.f32(0x584, 0.5);
    m.f32(0x588, 12.0);
    m.f32(0x598, 1.0);
    // flipbooks
    m.list(0x560, 1, 0x5b0);
    m.list(0x568, 1, 0x5d0);
    for (i, g) in [0x100usize, 0x1c4].into_iter().enumerate() {
        m.f32(g, 7.0);
        m.u16(g + 0x12, i as u16);
        m.u32(g + 0x14, 0x600); // anim header
        m.u32(g + 0x18, 0x640); // model name list
        m.u32(g + 0x1c, 0x700); // triangles
        m.u32(g + 0x20, 0x660); // grid table
        m.f32(g + 0x24, -10.0);
        m.f32(g + 0x28, -5.0);
        m.f32(g + 0x2c, 10.0);
        m.f32(g + 0x30, 20.0);
        m.u32(g + 0x34, 2);
        m.u32(g + 0x38, 2);
        m.list(g + 0x3c, 1, 0x340);
        m.list(g + 0x4c, 1, 0x360);
        m.list(g + 0x54, 1, 0x380);
        m.list(g + 0x5c, 2, 0x3a0);
        m.list(g + 0x64, 1, 0x3c0);
        m.list(g + 0x6c, 1, 0x3e0);
        m.list(g + 0x74, 1, 0x400);
        m.list(g + 0x7c, 1, 0x420);
        m.list(g + 0x84, 1, 0x800);
        m.list(g + 0x8c, 1, 0x4c0);
    }
    // anim header: pos_y with two keyframes
    m.list(0x620, 2, 0x580);
    // model name list, NULL terminated
    m.u32(0x640, 0x4d0);
    // grid: four cells; [0], [1, 2], NULL, [3]
    m.u32(0x660, 0x680);
    m.u32(0x664, 0x684);
    m.u32(0x66c, 0x68a);
    m.u16(0x680, 0);
    m.u16(0x682, 0xffff);
    m.u16(0x684, 1);
    m.u16(0x686, 2);
    m.u16(0x688, 0xffff);
    m.u16(0x68a, 3);
    m.u16(0x68c, 0xffff);
    for t in 0..4 {
        m.f32(0x700 + t * 0x40, t as f32);
        m.f32(0x700 + t * 0x40 + 0x10, 1.0);
    }
    m.0
}

#[test]
fn synthetic_stage_is_valid() {
    let _g = serial();
    let s = parse_stagedef_smb1(&synthetic()).unwrap();
    assert_eq!(s.anim_groups.len(), 2);
    let g = &s.anim_groups[1];
    assert_eq!(g.triangles.len(), 4);
    assert_eq!(g.grid.cells.len(), 4);
    assert_eq!(g.grid.cells[1], vec![1, 2]);
    assert_eq!(g.model_names, vec!["MODEL_A"]);
    assert_eq!(g.anim.as_ref().unwrap().pos_y.len(), 2);
    assert_eq!(
        (g.goals.len(), g.bumpers.len(), g.jamabars.len()),
        (1, 1, 1)
    );
    assert_eq!((g.bananas.len(), g.cones.len(), g.spheres.len()), (2, 1, 1));
    assert_eq!((g.cylinders.len(), g.fallout_boxes.len()), (1, 1));
    assert_eq!(g.mirror_model_names, vec!["MODEL_A"]);
    assert_eq!(s.bg_objects.len(), 1);
    let bg = &s.fg_objects[0];
    assert_eq!(bg.name, "MODEL_A");
    assert_eq!(bg.anim.as_ref().unwrap().scale_x.len(), 2);
    let fb = bg.flipbooks.as_ref().unwrap();
    assert_eq!(
        (fb.night_window_anims.len(), fb.storm_fire_anims.len()),
        (1, 1)
    );
    assert_eq!(s.mirror_names, vec!["MODEL_A"]);
    assert_eq!(s.models[0].name, "MODEL_A");
}

#[test]
fn truncation_at_every_length() {
    let _g = serial();
    let src = synthetic();
    for len in 0..src.len() {
        let _ = parse_checked(&format!("truncation to {len}"), &src[..len]);
    }
    // The data ends at 0x820; cutting inside it must be an error, not a shorter stage.
    assert!(parse_stagedef_smb1(&src[..0x810]).is_err());
}

#[test]
fn every_single_byte_corruption() {
    let _g = serial();
    let src = synthetic();
    let mut b = src.clone();
    for i in 0..src.len() {
        for v in [0x00, 0x01, 0x7f, 0x80, 0xff, src[i] ^ 0x55] {
            b[i] = v;
            let _ = parse_checked(&format!("byte {i:#x} = {v:#x}"), &b);
        }
        b[i] = src[i];
    }
}

#[test]
fn hostile_words_at_every_offset() {
    let _g = serial();
    let src = synthetic();
    let mut b = src.clone();
    for o in (0..src.len()).step_by(4) {
        let mut values = hostile_u32(src.len());
        // Pointers at themselves and at their own record.
        values.extend([o as u32, o as u32 & !0xff, (o as u32).wrapping_sub(4)]);
        for v in values {
            b[o..o + 4].copy_from_slice(&v.to_be_bytes());
            let _ = parse_checked(&format!("word {o:#x} = {v:#x}"), &b);
        }
        b[o..o + 4].copy_from_slice(&src[o..o + 4]);
    }
}

#[test]
fn random_corruption() {
    let _g = serial();
    let src = synthetic();
    let mut rng = Rng(0x5eed_0001);
    let (mut ok, mut err) = (0, 0);
    for i in 0..20_000 {
        let b = mutate(&src, &mut rng);
        match parse_checked(&format!("random mutation {i}"), &b) {
            Ok(_) => ok += 1,
            Err(_) => err += 1,
        }
    }
    // The campaign must reach both outcomes, or it is not testing the parser.
    assert!(ok > 1000 && err > 1000, "ok {ok}, err {err}");
}

/// Every grid cell of every anim group points at one long shared index list: the decoded grid would be
/// (cells x list length) entries, far more than the file holds.
#[test]
fn shared_grid_lists_are_rejected_before_allocating() {
    let _g = serial();
    let len = 0x40000;
    let mut m = Img(vec![0; len]);
    let groups = 64u32;
    m.list(0x08, groups, 0x100);
    let table = 0x4000u32;
    let list = 0x20000u32;
    let cells = (list - table) / 4;
    for o in (table..list).step_by(4) {
        m.u32(o as usize, list);
    }
    // 0x1fffe / 2 non-negative indices, then the terminator at the end of the file.
    let end = len - 2;
    m.u16(end, 0xffff);
    for g in 0..groups as usize {
        let o = 0x100 + g * mb_stage::ANIM_GROUP_SIZE;
        m.u32(o + 0x20, table);
        m.u32(o + 0x34, cells);
        m.u32(o + 0x38, 1);
    }
    let r = parse_checked("shared grid lists", &m.0);
    assert!(matches!(r, Err(StageError::Invalid(_))), "{r:?}");
}

/// Thousands of background objects sharing one animation whose eleven tracks share one long keyframe list.
#[test]
fn shared_keyframe_tracks_are_rejected_before_allocating() {
    let _g = serial();
    let len = 0x40000;
    let mut m = Img(vec![0; len]);
    let objects = 2048u32;
    m.list(0x68, objects, 0x1000);
    for i in 0..objects as usize {
        m.u32(0x1000 + i * mb_stage::BG_OBJECT_SIZE + 0x30, 0x100);
    }
    let keys = 0x20000u32;
    let count = (len as u32 - keys) / mb_stage::KEYFRAME_SIZE as u32;
    for t in 0..11 {
        m.list(0x108 + t * 8, count, keys);
    }
    let r = parse_checked("shared keyframe tracks", &m.0);
    assert!(matches!(r, Err(StageError::Invalid(_))), "{r:?}");
}

/// Counts that cannot fit, in every `(count, pointer)` slot of the header.
#[test]
fn hostile_counts_in_every_header_list() {
    let _g = serial();
    let src = synthetic();
    for slot in [
        0x08, 0x18, 0x28, 0x30, 0x38, 0x40, 0x48, 0x50, 0x58, 0x68, 0x70, 0x80,
    ] {
        for count in [
            0x7fff_ffffu32,
            0x8000_0000,
            u32::MAX,
            src.len() as u32,
            0x1_0000,
        ] {
            let mut b = Img(src.clone());
            b.u32(slot, count);
            let r = parse_checked(&format!("count {count:#x} at {slot:#x}"), &b.0);
            // Negative counts read as empty lists (the reference loops zero times); the others overrun.
            if (count as i32) > 0 {
                assert!(r.is_err(), "count {count:#x} at {slot:#x} was accepted");
            }
        }
    }
}

#[test]
fn real_stagedefs_survive_the_same_campaign() {
    let _g = serial();
    let Some(disc) = open_disc() else { return };
    let paths = disc_paths(&disc, |f| {
        f.starts_with("test/st")
            && f.rsplit('/')
                .next()
                .is_some_and(|n| n.starts_with("STAGE") && n.ends_with(".lz"))
    });
    let mut rng = Rng(0x5eed_0002);
    let (mut ok, mut err) = (0, 0);
    for p in paths.iter().step_by(8) {
        let src = fetch(&disc, p);
        assert!(parse_checked(p, &src).is_ok(), "{p}");
        for i in 0..150 {
            let b = mutate(&src, &mut rng);
            match parse_checked(&format!("{p} mutation {i}"), &b) {
                Ok(_) => ok += 1,
                Err(_) => err += 1,
            }
        }
        for cut in [0, 1, 0x93, 0x94, src.len() / 2, src.len() - 1] {
            let _ = parse_checked(&format!("{p} cut at {cut}"), &src[..cut]);
        }
    }
    eprintln!("real stagedef mutations: {ok} parsed, {err} rejected");
    assert!(err > 0);
}
