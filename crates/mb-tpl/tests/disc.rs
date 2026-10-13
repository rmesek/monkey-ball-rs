//! Real-disc tests. Skipped cleanly when no Super Monkey Ball 1 image is available (set `MB_DISC`
//! or put `Super Monkey Ball*.rvz|iso` next to this repository).

use mb_assets::disc::DiscPack;
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use std::collections::BTreeMap;
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
