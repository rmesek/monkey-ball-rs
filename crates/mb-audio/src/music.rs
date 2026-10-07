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

/// Disc stem of the *stereo* track behind a reference (`WebMonkeyBall`) music name, where it can be
/// established from the files alone: only the tracks whose intro/no-intro shape is unique are listed.
/// The other six stage themes (`sky mall water desert storm extra`) map into `st2 st3 st4 st5 st6 st8`
/// but which is which needs the game's background-to-music table, which is not in `test/`.
pub fn reference_music_key(name: &str) -> Option<&'static str> {
    match name {
        "jungle" => Some("st1"),
        "arctic" => Some("st7"),
        "bonus" => Some("stb"),
        "master" => Some("stm"),
        "ending" => Some("end"),
        _ => None,
    }
}

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
    fn pcm_is_big_endian() {
        assert_eq!(
            decode_pcm_s16be(&[0xff, 0xfe, 0x01, 0x00, 9]),
            vec![-2, 256]
        );
    }
}
