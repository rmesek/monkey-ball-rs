//! Real-disc tests. Skipped cleanly when no Super Monkey Ball 1 image is available
//! (set `MB_DISC`, or put a `Super Monkey Ball*.rvz|iso` next to this repository).

use mb_assets::disc::DiscPack;
use mb_assets::pack::PackProvider;
use mb_audio::adp::{decode_adp, ADP_FRAME_BYTES};
use mb_audio::mixer::{Mixer, Sound};
use mb_audio::music::{catalog, decode_pcm_s16be};
use mb_audio::music::{stage_music, MUSIC_BACKGROUNDS};
use mb_audio::musyx::load_library;
use mb_audio::sound_table::SoundTable;
use std::path::PathBuf;
use std::sync::Arc;

/// The reference's `src/audio/smb1_sfx_ids.ts` (copied ids only, no game data).
const REFERENCE_SFX_IDS: [u32; 72] = [
    0x0003, 0x0004, 0x0005, 0x0006, 0x0007, 0x0008, 0x0009, 0x000a, 0x000b, 0x000c, 0x0012, 0x0013,
    0x0014, 0x0015, 0x0016, 0x0017, 0x0018, 0x001a, 0x001b, 0x001c, 0x001d, 0x001e, 0x0021, 0x0023,
    0x0024, 0x0025, 0x0026, 0x0027, 0x0028, 0x0029, 0x002a, 0x002b, 0x002c, 0x002e, 0x002f, 0x0030,
    0x0039, 0x003e, 0x0046, 0x0047, 0x0050, 0x005a, 0x005b, 0x005c, 0x005d, 0x005e, 0x005f, 0x0060,
    0x0061, 0x0067, 0x0068, 0x0069, 0x00d1, 0x010b, 0x0126, 0x0127, 0x0128, 0x016c, 0x01d1, 0x01e0,
    0x022c, 0x0240, 0x281b, 0x281f, 0x2820, 0x2852, 0x2858, 0x2859, 0x5011, 0xa022, 0x3b1f4,
    0x3d806,
];

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

fn adp_files(disc: &DiscPack) -> Vec<(String, String)> {
    disc.list()
        .into_iter()
        .filter_map(|f| {
            let stem = f
                .strip_prefix("test/snd/adp/")?
                .strip_suffix(".adp")?
                .to_string();
            Some((stem, f.to_string()))
        })
        .collect()
}

#[test]
fn every_adp_decodes_with_plausible_duration_and_levels() {
    let disc = disc_or_skip!();
    let files = adp_files(&disc);
    assert_eq!(files.len(), 112);
    for (stem, path) in &files {
        let raw = disc.fetch(path).unwrap();
        assert_eq!(raw.len() % 32768, 0, "{stem}: sector-padded size");
        let a = decode_adp(&raw).unwrap_or_else(|e| panic!("{stem}: {e}"));
        let secs = a.duration_secs();
        assert!((0.5..=120.0).contains(&secs), "{stem}: {secs}s");
        // Padding trimmed: no more than one frame of all-zero data left at the end.
        assert!(a.frames() <= raw.len() / ADP_FRAME_BYTES * 28);
        let n = a.samples.len() as f64;
        let peak = a.samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 2000, "{stem}: too quiet (peak {peak})");
        let clipped = a
            .samples
            .iter()
            .filter(|&&s| s == i16::MAX || s == i16::MIN)
            .count() as f64;
        assert!(clipped / n < 0.01, "{stem}: {clipped} clipped samples");
        let rms = (a.samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / n).sqrt();
        assert!((300.0..20000.0).contains(&rms), "{stem}: rms {rms}");
    }
}

#[test]
fn mono_variants_have_identical_channels_and_catalog_covers_all_files() {
    let disc = disc_or_skip!();
    let files = adp_files(&disc);
    let stems: Vec<&str> = files.iter().map(|(s, _)| s.as_str()).collect();
    let tracks = catalog(&stems);
    // Every file is used exactly once, as an intro or a body.
    let mut used: Vec<&str> = Vec::new();
    for t in &tracks {
        used.push(&t.body);
        if let Some(i) = &t.intro {
            used.push(i);
        }
    }
    used.sort_unstable();
    let mut all = stems.clone();
    all.sort_unstable();
    assert_eq!(used, all);
    // Every track has a mono twin.
    assert_eq!(
        tracks.iter().filter(|t| t.mono).count(),
        tracks.iter().filter(|t| !t.mono).count()
    );
    for t in tracks.iter().filter(|t| t.mono) {
        for stem in t.intro.iter().chain(std::iter::once(&t.body)) {
            let path = format!("test/snd/adp/{stem}.adp");
            let a = decode_adp(&disc.fetch(&path).unwrap()).unwrap();
            assert!(
                a.samples.as_chunks::<2>().0.iter().all(|p| p[0] == p[1]),
                "{stem} is not mono"
            );
        }
    }
}

#[test]
fn music_streams_into_the_mixer() {
    let disc = disc_or_skip!();
    let intro = decode_adp(&disc.fetch("test/snd/adp/st3_int.adp").unwrap()).unwrap();
    let body = decode_adp(&disc.fetch("test/snd/adp/st3_lp.adp").unwrap()).unwrap();
    let mut m = Mixer::new(48_000);
    let id = m.play_music(
        Some(Arc::new(Sound::from_adp(&intro))),
        Arc::new(Sound::from_adp(&body)),
        1.0,
    );
    let frames = intro.frames() + 48_000;
    let mut out = vec![0.0f32; frames * 2];
    m.mix(&mut out);
    assert!(m.is_playing(id));
    assert!(out.iter().all(|s| s.abs() <= 2.0));
    // The first body frame follows the last intro frame directly.
    let k = intro.frames();
    let want = f32::from(body.samples[0]) / 32768.0;
    assert!((out[k * 2] - want).abs() < 1e-4, "{} vs {want}", out[k * 2]);
}

#[test]
fn all_musyx_banks_parse_and_decode() {
    let disc = disc_or_skip!();
    let lib = load_library(|p| disc.fetch(p)).expect("banks");
    assert_eq!(lib.banks().len(), 9);

    // Global SFX indices are contiguous 0..=0x367 across the nine group banks.
    let ids: Vec<u16> = lib.sfx_ids().collect();
    assert_eq!(ids.len(), 0x368);
    assert_eq!(ids.first(), Some(&0));
    assert_eq!(ids.last(), Some(&0x367));
    assert!(ids.windows(2).all(|w| w[1] == w[0] + 1));

    let mut silent = 0;
    for bank in lib.banks() {
        for (g, _) in bank.sfx_entries().take(1) {
            assert_eq!(g.kind, 1);
        }
        for (id, e) in &bank.samples {
            assert_eq!(e.format, 0);
            assert_eq!(e.params.bytes_per_frame, 8);
            assert!(matches!(
                e.sample_rate,
                11025 | 22050 | 30000 | 32000 | 44100
            ));
            let s = bank.decode_sample(*id).unwrap();
            assert_eq!(s.pcm.len(), e.num_samples as usize);
            // First-frame predictor/scale byte recorded in the params matches the sample data.
            assert_eq!(e.params.initial_ps, bank.raw_sample_data(*id).unwrap()[0]);
            let secs = s.pcm.len() as f64 / f64::from(s.sample_rate);
            assert!((0.01..30.0).contains(&secs), "sample {id}: {secs}s");
            if let Some((ls, le)) = s.loop_range {
                assert!(ls < le && le <= s.pcm.len());
                assert_eq!(le, (e.loop_start + e.loop_length) as usize);
                // Stored loop history equals the decoded samples just before the loop point.
                if ls >= 2 {
                    assert_eq!(e.params.loop_hist1, s.pcm[ls - 1], "sample {id} hist1");
                    assert_eq!(e.params.loop_hist2, s.pcm[ls - 2], "sample {id} hist2");
                }
            }
        }
        for (_, e) in bank.sfx_entries() {
            let sid = bank.macros[&e.macro_id].first_sample();
            match sid {
                Some(sid) => assert!(bank.samples.contains_key(&sid)),
                None => silent += 1,
            }
        }
    }
    assert!(silent > 0, "silent placeholder macros expected");
}

/// The request ids the reference lists resolve through the executable's own table to sounds in the banks.
#[test]
fn sound_table_resolves_the_reference_ids() {
    let disc = disc_or_skip!();
    let lib = load_library(|p| disc.fetch(p)).expect("banks");
    let table = SoundTable::from_dol(&disc.main_dol().expect("main.dol")).expect("sound table");
    assert_eq!(table.entries.len(), 1061);
    assert_eq!(table.entries[0].name, "GRPse04");
    assert_eq!(table.entries.last().unwrap().name, "GRPend");

    let (mut resolved, mut audible) = (0, 0);
    for id in REFERENCE_SFX_IDS {
        // 0x33 and friends that no entry answers to do not occur in the reference list; voices resolve
        // per character.
        for character in 0..4 {
            let Some(r) = table.resolve(id, character) else {
                continue;
            };
            assert!(lib.contains(r.musyx_id), "{id:#x} -> {}", r.musyx_id);
            if character == 0 {
                resolved += 1;
                if lib.render(r.musyx_id).unwrap().is_some() {
                    audible += 1;
                }
            }
        }
    }
    eprintln!(
        "{resolved}/{} reference ids resolve, {audible} to an audible sample",
        REFERENCE_SFX_IDS.len()
    );
    assert_eq!(resolved, REFERENCE_SFX_IDS.len());
    assert!(audible >= 60, "{audible}");

    // Spot checks against the disassembled call sites.
    let get = |id| table.resolve(id, 0).map(|r| (r.musyx_id, r.velocity));
    assert_eq!(get(0x4), Some((170, 110))); // SND_SED_NAR_READY
    assert_eq!(get(0x5), Some((158, 110))); // SND_SED_NAR_GO
    assert_eq!(get(0x8), Some((160, 110))); // SND_SED_NAR_GOAL
    assert_eq!(get(0x5011), Some((165, 110))); // bumper
    assert_eq!(get(0x3), Some((206, 110))); // banana
    assert_eq!(get(0x39), Some((207, 110))); // bunch
    assert_eq!(get(0x2f), Some((176, 100))); // hard hit
    assert_eq!(get(0x14), Some((185, 127)));
    assert_eq!(get(0x13), Some((178, 127)));
    assert_eq!(get(0x3d806), Some((164, 95))); // timer tick
    assert_eq!(table.by_name("SND_SE01_RUN01"), Some(248));
    assert_eq!(table.by_name("SND_SE01_RUN02"), Some(273));
    assert_eq!(table.by_name("SND_SE01_RUN03"), Some(274));
}

/// Every stage background's music exists on the disc and its intro joins the loop.
#[test]
fn stage_music_files_exist_and_decode() {
    let disc = disc_or_skip!();
    for bg in MUSIC_BACKGROUNDS {
        let m = stage_music(bg).unwrap();
        let mut frames = 0;
        for path in m.paths() {
            let a =
                decode_adp(&disc.fetch(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap();
            assert!(a.frames() > 1000, "{path}");
            frames += a.frames();
        }
        assert!(frames > 48_000 * 20, "{bg}: {frames} frames");
    }
}

#[test]
fn race_adp_copy_and_pcm_streams() {
    let disc = disc_or_skip!();
    assert_eq!(
        disc.fetch("test/snd/mkb/race.adp").unwrap(),
        disc.fetch("test/snd/adp/race.adp").unwrap()
    );
    let l = decode_pcm_s16be(&disc.fetch("test/snd/test/streamL.pcm").unwrap());
    assert_eq!(l.len() * 2, 18_547_200);
    // Big-endian reading yields a smooth waveform (neighbour differences far below the level).
    let head = &l[..200_000];
    let level = head.iter().map(|&s| f64::from(s).abs()).sum::<f64>() / head.len() as f64;
    let step = head
        .windows(2)
        .map(|w| f64::from(w[1]) - f64::from(w[0]))
        .map(f64::abs)
        .sum::<f64>()
        / head.len() as f64;
    assert!(step < level * 0.5, "step {step} level {level}");
}
