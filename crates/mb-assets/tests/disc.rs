//! Real-disc tests. They run only when a Super Monkey Ball 1 image is available and skip otherwise,
//! so CI and clones without game data stay green. Point `MB_DISC` at an image, or put a
//! `Super Monkey Ball*.rvz|iso` next to this repository.
//!
//! `nod` is the reference here: the built-in readers must list the same files and return the same bytes.
//! (`cargo run --release -p xtask -- check-rvz <image>` runs the same comparison over the whole disc.)

use mb_assets::disc::{DiscPack, DiscStream, FileStream, ImageStream, MAIN_DOL_PATH};
use mb_assets::lzss;
use mb_assets::pack::PackProvider;
use nod::common::PartitionKind;
use nod::read::{DiscOptions, DiscReader, PartitionOptions};
use std::io::{Read, Seek, SeekFrom};
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
            Some(p) => p,
            None => {
                eprintln!("skipped: no Super Monkey Ball 1 disc image found (set MB_DISC)");
                return;
            }
        }
    };
}

#[test]
fn identifies_smb1() {
    let disc = DiscPack::open(disc_or_skip!()).expect("open disc");
    assert!(disc.smb1_region().is_some(), "id {}", disc.game_id());
    assert!(disc.list().iter().any(|f| f.starts_with("test/")));
    assert!(disc.contains(MAIN_DOL_PATH));
}

/// Same file list, same executable, same file contents and same raw disc bytes as nod.
#[test]
fn matches_nod() {
    let path = disc_or_skip!();
    let ours = DiscPack::open(&path).expect("open with the built-in reader");
    let mut reference = DiscReader::new(&path, &DiscOptions::default()).expect("open with nod");
    assert_eq!(ours.game_id(), reference.header().game_id_str());

    // Raw disc bytes: every 16th MiB (the whole disc is compared by `xtask check-rvz`).
    const BLOCK: u64 = 1 << 20;
    let mut stream = ImageStream::open(FileStream::open(&path).unwrap()).unwrap();
    let size = stream.stream_len().unwrap();
    assert_eq!(size, reference.disc_size());
    let (mut a, mut b) = (vec![0u8; BLOCK as usize], vec![0u8; BLOCK as usize]);
    for offset in (0..size).step_by(16 * BLOCK as usize) {
        let n = BLOCK.min(size - offset) as usize;
        reference.seek(SeekFrom::Start(offset)).unwrap();
        reference.read_exact(&mut a[..n]).unwrap();
        stream.read_exact_at(&mut b[..n], offset).unwrap();
        assert!(
            a[..n] == b[..n],
            "disc bytes differ in the MiB at {offset:#x}"
        );
    }

    let mut partition = reference
        .open_partition_kind(PartitionKind::Data, &PartitionOptions::default())
        .expect("data partition");
    let meta = partition.meta().expect("partition meta");
    assert!(
        ours.main_dol().unwrap() == meta.raw_dol.as_ref(),
        "main.dol differs"
    );
    let fst = meta.fst().expect("fst");
    let files: Vec<_> = fst
        .iter()
        .filter(|(_, node, _)| node.is_file())
        .map(|(_, node, path)| (path.to_string(), node))
        .collect();
    let mut theirs: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
    theirs.sort_unstable();
    let mine: Vec<&str> = ours
        .list()
        .into_iter()
        .filter(|p| *p != MAIN_DOL_PATH)
        .collect();
    assert_eq!(mine, theirs, "file lists differ");
    // Contents: every file of stage 1 and its sounds, and every 9th file of the rest.
    for (i, (path, node)) in files.iter().enumerate() {
        let wanted = i % 9 == 0 || path.contains("st001") || path.contains("init/");
        if !wanted || node.length() > 32 << 20 {
            continue;
        }
        let mut expected = Vec::new();
        partition
            .open_file(*node)
            .unwrap()
            .read_to_end(&mut expected)
            .unwrap();
        assert!(ours.fetch(path).unwrap() == expected, "{path} differs");
        assert_eq!(ours.file_len(path), Some(expected.len() as u32));
    }
}

/// Every `.lz` file on the disc decompresses to the size its header declares (which also shows the
/// output-size guard rejects no real file).
#[test]
fn every_lz_file_decompresses_to_its_declared_size() {
    let disc = DiscPack::open(disc_or_skip!()).expect("open disc");
    let names: Vec<String> = disc
        .list()
        .into_iter()
        .filter(|f| f.starts_with("test/") && f.ends_with(".lz"))
        .map(String::from)
        .collect();
    assert!(names.len() > 100, "only {} .lz files", names.len());
    for path in names {
        let raw = disc.fetch(&path).unwrap();
        if raw.len() < 8 {
            continue;
        }
        let declared = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
        let out = lzss::decompress(&raw).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(out.len(), declared, "{path}");
    }
}
