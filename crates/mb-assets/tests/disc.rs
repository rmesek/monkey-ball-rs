//! Real-disc tests. They run only when a Super Monkey Ball 1 image is available and skip otherwise,
//! so CI and clones without game data stay green. Point `MB_DISC` at an image, or put a
//! `Super Monkey Ball*.rvz|iso` next to this repository.
//!
//! What the built-in readers return is held by recorded hashes. The normal run reads a sample; the whole
//! image, every byte and every file, is one command away:
//!
//! ```text
//! cargo test --release -p mb-assets --test disc -- --ignored the_whole_image_reads_as_recorded
//! ```

use mb_assets::disc::{DiscPack, DiscStream, FileStream, ImageStream, MAIN_DOL_PATH};
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

struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, data: &[u8]) {
        for b in data {
            self.0 = (self.0 ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// What the readers return for the USA disc (`GMBE8P`), as a hash: the decoded disc's size and raw bytes
/// (every 16th MiB, or all of them), the file list with lengths, `main.dol`, and file contents (every
/// file of stage 1 and its sounds and every 9th of the rest, or all of them). A second, independent
/// reader (`nod`) returned the same bytes for the whole image when these values were recorded.
fn image_hash(path: &std::path::Path, whole: bool) -> Option<(u64, usize, u64)> {
    let disc = DiscPack::open(path).expect("open disc");
    if disc.game_id() != "GMBE8P" {
        eprintln!(
            "skipped: recorded for the USA disc, this is {}",
            disc.game_id()
        );
        return None;
    }
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    const BLOCK: u64 = 1 << 20;
    let stride = if whole { 1 } else { 16 };
    let mut stream = ImageStream::open(FileStream::open(path).unwrap()).unwrap();
    let size = stream.stream_len().unwrap();
    let mut block = vec![0u8; BLOCK as usize];
    for offset in (0..size).step_by(stride * BLOCK as usize) {
        let n = BLOCK.min(size - offset) as usize;
        stream.read_exact_at(&mut block[..n], offset).unwrap();
        h.bytes(&block[..n]);
    }
    h.bytes(&disc.main_dol().unwrap());
    let files: Vec<&str> = disc
        .list()
        .into_iter()
        .filter(|p| *p != MAIN_DOL_PATH)
        .collect();
    for (i, path) in files.iter().enumerate() {
        let len = disc.file_len(path).unwrap();
        h.bytes(path.as_bytes());
        h.bytes(&len.to_le_bytes());
        let sampled = i % 9 == 0 || path.contains("st001") || path.contains("init/");
        if !whole && (!sampled || len > 32 << 20) {
            continue;
        }
        let data = disc.fetch(path).unwrap();
        assert_eq!(data.len(), len as usize, "{path}");
        h.bytes(&data);
    }
    Some((size, files.len(), h.0))
}

#[test]
fn the_image_reads_as_recorded() {
    let Some(got) = image_hash(&disc_or_skip!(), false) else {
        return;
    };
    assert_eq!(got, (1_459_978_240, 897, 0xf9b1_afcb_c0e0_d412));
}

/// The same over every byte of the decoded disc and the contents of every file.
#[test]
#[ignore = "reads the whole image; run it after touching the disc or RVZ reader"]
fn the_whole_image_reads_as_recorded() {
    let Some(got) = image_hash(&disc_or_skip!(), true) else {
        return;
    };
    assert_eq!(got, (1_459_978_240, 897, 0x8fd8_0487_85d8_4659));
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
