//! Real-disc tests. Skipped cleanly when no Super Monkey Ball 1 image is available (set `MB_DISC`
//! or put `Super Monkey Ball*.rvz|iso` next to this repository). The reference comparison also
//! needs `deno` on PATH and the read-only `../WebMonkeyBall` checkout (or `MB_REF_SRC`).

use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

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

/// Every AV TPL on the disc (test/preview/*.tpl are raw RGB565 frame dumps, not TPL containers) as (disc path, decompressed bytes).
fn all_tpls(disc: &DiscPack) -> Vec<(String, Vec<u8>)> {
    let paths: Vec<String> = disc
        .list()
        .into_iter()
        .filter(|f| {
            (f.ends_with(".tpl") || f.ends_with(".tpl.lz")) && !f.starts_with("test/preview/")
        })
        .map(String::from)
        .collect();
    paths
        .into_iter()
        .map(|p| {
            let raw = disc.fetch(&p).unwrap();
            let bytes = if p.ends_with(".lz") {
                lzss::decompress(&raw).unwrap_or_else(|e| panic!("{p}: {e}"))
            } else {
                raw
            };
            (p, bytes)
        })
        .collect()
}

fn fnv1a(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

#[test]
fn every_tpl_parses_and_decodes() {
    let disc = disc_or_skip!();
    let tpls = all_tpls(&disc);
    assert!(tpls.len() > 20, "found only {} tpls", tpls.len());
    let mut formats = BTreeMap::new();
    let mut n_tex = 0usize;
    for (path, bytes) in &tpls {
        let tpl = mb_tpl::parse(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        for t in tpl.textures() {
            *formats.entry(format!("{:?}", t.format)).or_insert(0usize) += 1;
            for l in 0..t.level_count() {
                let (w, h) = t.level_size(l);
                let rgba = t
                    .decode_rgba(l)
                    .unwrap_or_else(|e| panic!("{path} tex {} level {l}: {e}", t.index));
                assert_eq!(rgba.len(), w * h * 4);
            }
            n_tex += 1;
        }
    }
    eprintln!("{} tpls, {n_tex} textures, formats {formats:?}", tpls.len());
}

fn ref_src_dir() -> Option<PathBuf> {
    let p = std::env::var_os("MB_REF_SRC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../WebMonkeyBall/src")
        });
    p.join("tpl.ts").exists().then(|| {
        // Strip the Windows verbatim prefix so Deno can build a file URL from it.
        let c = p.canonicalize().unwrap().to_string_lossy().into_owned();
        PathBuf::from(c.strip_prefix("\\\\?\\").unwrap_or(&c))
    })
}

/// Compares the RGBA of every level of a selection of TPLs against the reference decoder (Deno).
#[test]
fn matches_reference_decoder() {
    let disc = disc_or_skip!();
    let Some(src) = ref_src_dir() else {
        eprintln!("skipped: WebMonkeyBall/src not found (set MB_REF_SRC)");
        return;
    };
    let tpls = all_tpls(&disc);
    // A mix: HUD bitmaps, common/init, stage textures, a preview, and an LZ-compressed one.
    let wanted = [
        "test/bmp/bmp_nml.tpl",
        "test/bmp/bmp_com.tpl.lz",
        "test/init/common.tpl.lz",
        "test/st001/st001.tpl",
        "test/bg/bg_jun.tpl",
        "test/st050/st050.tpl",
        "test/ape/boy_ch.tpl.lz",
    ];
    let dir = std::env::temp_dir().join("mb-tpl-ref");
    std::fs::create_dir_all(&dir).unwrap();
    let mut files = Vec::new();
    let mut ours = BTreeMap::new();
    for w in wanted {
        let Some((_, bytes)) = tpls.iter().find(|(p, _)| p.eq_ignore_ascii_case(w)) else {
            continue;
        };
        let f = dir.join(w.replace('/', "_"));
        std::fs::write(&f, bytes).unwrap();
        let tpl = mb_tpl::parse(bytes).unwrap();
        for t in tpl.textures() {
            for l in 0..t.level_count() {
                let (tw, th) = t.level_size(l);
                let h = fnv1a(&t.decode_rgba(l).unwrap());
                ours.insert(
                    format!("{}\t{}\t{l}\t{tw}x{th}\t{h:x}", f.display(), t.index),
                    (),
                );
            }
        }
        files.push(f);
    }
    assert!(files.len() >= 3, "too few sample tpls on disc");

    let out = Command::new("deno")
        .arg("run")
        .arg("-A")
        .arg("--sloppy-imports")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/ref_hash.ts"))
        .arg(&src)
        .args(&files)
        .output();
    let out = match out {
        Ok(o) if o.status.success() => o,
        Ok(o) => panic!("deno failed: {}", String::from_utf8_lossy(&o.stderr)),
        Err(_) => {
            eprintln!("skipped: deno not available");
            return;
        }
    };
    let theirs: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert!(theirs.len() >= 3);
    let mut compared = 0;
    for line in &theirs {
        assert!(
            ours.contains_key(line),
            "mismatch / missing in rust: {line}"
        );
        compared += 1;
    }
    assert_eq!(compared, ours.len(), "level count differs from reference");
    eprintln!("compared {compared} levels across {} files", files.len());
}
