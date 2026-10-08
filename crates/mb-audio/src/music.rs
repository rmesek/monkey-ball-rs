//! Music catalog: groups the `test/snd/adp/*.adp` file names into playable tracks.
//!
//! Naming on the disc: `X_int` + `X_lp` (intro then loop), `X_all` + `X_lp` (a longer lead-in file then the
//! loop; the intro is `X_all`), or a single `X` that loops whole. A trailing `m` on the base name marks the
//! mono (L == R) mixdown of the same track (`stmm_lp` is the mono `stm_lp`; `stm_lp` itself is stereo).

use std::collections::{BTreeMap, BTreeSet};

/// One playable music track (stereo or mono variant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicTrack {
    /// Stereo base name, e.g. `st3`, `sel2`, `theme` (shared by the mono variant).
    pub key: String,
    /// File stem of the intro stream, if any.
    pub intro: Option<String>,
    /// File stem of the stream that loops.
    pub body: String,
    /// `true` for the `m` (mono) variant.
    pub mono: bool,
}

/// Group file stems (`st3_int`, without directory or `.adp`) into tracks, sorted by key then stereo first.
pub fn catalog<S: AsRef<str>>(stems: &[S]) -> Vec<MusicTrack> {
    let set: BTreeSet<String> = stems.iter().map(|s| s.as_ref().to_string()).collect();
    let lower: BTreeMap<String, String> =
        set.iter().map(|s| (s.to_lowercase(), s.clone())).collect();
    let split = |s: &str| -> (String, &'static str) {
        for suf in ["_int", "_lp", "_all"] {
            if let Some(p) = s.strip_suffix(suf) {
                return (p.to_string(), suf);
            }
        }
        (s.to_string(), "")
    };
    let mut out = Vec::new();
    for stem in &set {
        let (prefix, suf) = split(stem);
        // Only emit a track from its looping stream (or the single file).
        if suf == "_int" || suf == "_all" {
            continue;
        }
        // Stereo counterpart of a mono stem: drop the trailing `m` (`stmm_lp` -> `stm_lp`) or the `m`
        // before an underscore in single files (`selm_golf` -> `sel_golf`).
        let candidates = [
            prefix.strip_suffix('m').map(|p| format!("{p}{suf}")),
            prefix.contains("m_").then(|| prefix.replacen("m_", "_", 1)),
        ];
        let stereo = candidates
            .iter()
            .flatten()
            .find_map(|c| lower.get(&c.to_lowercase()));
        let mono = stereo.is_some();
        let key = match stereo {
            Some(st) => split(st).0,
            None => prefix.clone(),
        };
        let intro = ["_int", "_all"]
            .iter()
            .map(|s| format!("{prefix}{s}"))
            .find(|n| set.contains(n));
        out.push(MusicTrack {
            key,
            intro,
            body: stem.clone(),
            mono,
        });
    }
    out.sort_by(|a, b| (&a.key.to_lowercase(), a.mono).cmp(&(&b.key.to_lowercase(), b.mono)));
    out
}

/// The music of one stage background: file stems under `test/snd/adp/` and the stream volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageMusic {
    /// Disc stem of the track (`st3`), also what identifies "the same music" between floors.
    pub key: &'static str,
    /// Played once before the loop (`*_int`, or `st2_all`).
    pub intro: Option<&'static str>,
    /// Loops.
    pub body: &'static str,
    /// The game's stream volume for this track, 0..=255 (`AISetStreamVol*`).
    pub volume: u8,
}

impl StageMusic {
    /// Linear gain of the track's stream volume.
    pub fn gain(&self) -> f32 {
        f32::from(self.volume) / 255.0
    }

    /// Disc paths of the files this track needs, intro first.
    pub fn paths(&self) -> Vec<String> {
        self.intro
            .iter()
            .chain(std::iter::once(&self.body))
            .map(|stem| format!("{ADP_DIR}{stem}.adp"))
            .collect()
    }
}

/// Where the music streams are on the disc.
pub const ADP_DIR: &str = "test/snd/adp/";

/// Music for a stage background (`bg_jun`, ...).
///
/// From the executable (USA `main.dol`): the background-id -> stream table at `0x801101dc`
/// (jun 20, wat 22, nig 24, sun 26, spa 28, snd 30, ice 32, stm 34, bns 36, mst 38) and the stream table at
/// `0x801b2a5c` (16-byte entries: name pointer, volume byte, length; even index = intro, odd = loop).
/// It agrees with the reference's names where those can be checked from the files (`jungle` and `arctic`
/// have no intro; `bonus`, `master`, `ending`), and settles the rest: water = `st2`, mall (`bg_nig`) =
/// `st3`, sky (`bg_sun`) = `st4`, extra (`bg_spa`) = `st5`, desert (`bg_snd`) = `st6`, storm = `st8`.
pub fn stage_music(bg: &str) -> Option<StageMusic> {
    let t = |key, intro, body, volume| {
        Some(StageMusic {
            key,
            intro,
            body,
            volume,
        })
    };
    match bg {
        "bg_jun" => t("st1", None, "st1", 0x8c),
        "bg_wat" => t("st2", Some("st2_all"), "st2_lp", 0x96),
        "bg_nig" => t("st3", Some("st3_int"), "st3_lp", 0x9b),
        "bg_sun" => t("st4", Some("st4_int"), "st4_lp", 0x87),
        "bg_spa" => t("st5", Some("st5_int"), "st5_lp", 0x96),
        "bg_snd" => t("st6", Some("st6_int"), "st6_lp", 0x96),
        "bg_ice" => t("st7", None, "st7", 0x91),
        "bg_stm" => t("st8", Some("st8_int"), "st8_lp", 0xa0),
        "bg_bns" => t("stb", Some("stb_int"), "stb_lp", 0x91),
        "bg_mst" => t("stm", Some("stm_int"), "stm_lp", 0x78),
        "bg_ending" => t("end", Some("end_int"), "end_lp", 0x91),
        _ => None,
    }
}

/// Every background [`stage_music`] knows.
pub const MUSIC_BACKGROUNDS: [&str; 11] = [
    "bg_jun",
    "bg_wat",
    "bg_nig",
    "bg_sun",
    "bg_spa",
    "bg_snd",
    "bg_ice",
    "bg_stm",
    "bg_bns",
    "bg_mst",
    "bg_ending",
];

/// Decode raw big-endian signed 16-bit PCM (`test/snd/test/stream{L,R}.pcm`, mono per file).
pub fn decode_pcm_s16be(data: &[u8]) -> Vec<i16> {
    data.as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_be_bytes(*b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_pairs_singles_and_mono() {
        let stems = [
            "st1",
            "st1m",
            "st3_int",
            "st3_lp",
            "st3m_int",
            "st3m_lp",
            "st2_all",
            "st2_lp",
            "stm_int",
            "stm_lp",
            "stmm_int",
            "stmm_lp",
            "sel_Fight",
            "selm_fight",
        ];
        let t = catalog(&stems);
        let find = |key: &str, mono: bool| {
            t.iter()
                .find(|x| x.key.eq_ignore_ascii_case(key) && x.mono == mono)
                .unwrap()
                .clone()
        };
        assert_eq!(find("st1", false).intro, None);
        assert!(find("st1", true).mono);
        let st3 = find("st3", false);
        assert_eq!(st3.intro.as_deref(), Some("st3_int"));
        assert_eq!(st3.body, "st3_lp");
        assert_eq!(find("st3", true).body, "st3m_lp");
        assert_eq!(find("st2", false).intro.as_deref(), Some("st2_all"));
        // `stm` ends in 'm' but is the stereo master theme.
        assert!(!find("stm", false).mono);
        assert_eq!(find("stm", true).body, "stmm_lp");
        assert_eq!(find("sel_Fight", true).body, "selm_fight");
    }

    #[test]
    fn stage_music_lists_its_files() {
        let jun = stage_music("bg_jun").unwrap();
        assert_eq!(jun.paths(), vec!["test/snd/adp/st1.adp"]);
        let wat = stage_music("bg_wat").unwrap();
        assert_eq!(
            wat.paths(),
            vec!["test/snd/adp/st2_all.adp", "test/snd/adp/st2_lp.adp"]
        );
        assert!((wat.gain() - 150.0 / 255.0).abs() < 1e-6);
        assert_eq!(stage_music("bg_pil"), None);
        // One track per background.
        let mut keys: Vec<_> = MUSIC_BACKGROUNDS
            .iter()
            .map(|b| stage_music(b).unwrap().key)
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), MUSIC_BACKGROUNDS.len());
    }

    #[test]
    fn pcm_is_big_endian() {
        assert_eq!(
            decode_pcm_s16be(&[0xff, 0xfe, 0x01, 0x00, 9]),
            vec![-2, 256]
        );
    }
}
