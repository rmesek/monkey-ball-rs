//! Real-disc tests. They run only when a Super Monkey Ball 1 image is available and skip otherwise,
//! so CI and clones without game data stay green. Point `MB_DISC` at an image, or put a
//! `Super Monkey Ball*.rvz|iso` next to this repository.

#![cfg(feature = "disc")]

use mb_assets::disc::DiscPack;
use mb_assets::layout::{stage_files, GameSource};
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
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

#[test]
fn identifies_smb1() {
    let disc = disc_or_skip!();
    assert!(disc.smb1_region().is_some(), "id {}", disc.game_id());
    assert!(disc.list().iter().any(|f| f.starts_with("test/")));
}

#[test]
fn stage_one_has_every_expected_file() {
    let disc = disc_or_skip!();
    for f in stage_files(GameSource::Smb1, 1, "bg_jun", false) {
        assert!(disc.fetch(&f).is_ok(), "missing {f}");
    }
}

#[test]
fn every_stagedef_decompresses_to_its_declared_size() {
    let disc = disc_or_skip!();
    let stagedefs: Vec<String> = disc
        .list()
        .into_iter()
        .filter(|f| {
            f.starts_with("test/st") && f.rsplit('/').next().is_some_and(|n| n.starts_with("STAGE"))
        })
        .map(String::from)
        .collect();
    assert!(!stagedefs.is_empty(), "no stagedefs found");
    for path in stagedefs {
        let raw = disc.fetch(&path).unwrap();
        let declared = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
        let out = lzss::decompress(&raw).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(out.len(), declared, "{path}");
    }
}
