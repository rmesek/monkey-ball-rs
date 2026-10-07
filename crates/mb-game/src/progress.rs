//! Persistent player data: unlocks, best scores and times, and settings, plus a small versioned text
//! format.
//!
//! ```text
//! MBSAVE 1
//! setting music_volume 50
//! course beginner 1 1 123456 1,2,3,4,91
//! stage 1 1234 3
//! crc 1a2b3c4d
//! ```
//! The last line is an FNV-1a checksum of everything before it. Unknown `setting` keys and record kinds are
//! ignored so a newer build's file still loads; a bad checksum, a missing header or a newer major version is
//! reported as a [`LoadError`] and the caller starts from defaults (keeping the bad file as a backup).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::course::Difficulty;
use crate::storage::Storage;

pub const SAVE_VERSION: u32 = 1;
pub const PROFILE_KEY: &str = "profile";
pub const BACKUP_KEY: &str = "profile.corrupt";
const MAX_SCORE: u64 = 999_999_999;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlMode {
    /// On-screen virtual joystick (also keyboard / gamepad).
    Stick,
    /// Device tilt (accelerometer / gyro) where available.
    Tilt,
}

impl ControlMode {
    pub fn key(self) -> &'static str {
        match self {
            ControlMode::Stick => "stick",
            ControlMode::Tilt => "tilt",
        }
    }
    fn from_key(s: &str) -> Option<Self> {
        match s {
            "stick" => Some(ControlMode::Stick),
            "tilt" => Some(ControlMode::Tilt),
            _ => None,
        }
    }
}

/// User settings. Volumes are 0..=100, `joystick_size` is a percentage (50..=200), `tilt_range_deg` is the
/// device tilt that gives full deflection (10..=25, like the reference's gyro sensitivity; smaller = more
/// sensitive), `input_falloff` is 100..=200 (= 1.0..2.0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub music_volume: u8,
    pub sfx_volume: u8,
    pub announcer_volume: u8,
    pub control_mode: ControlMode,
    pub joystick_size: u16,
    pub left_handed: bool,
    pub tilt_range_deg: u8,
    pub input_falloff: u16,
    /// Every course and stage selectable from the start (the web reference behaves like this).
    pub unlock_all: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Defaults of the reference's settings sliders.
            music_volume: 50,
            sfx_volume: 30,
            announcer_volume: 30,
            control_mode: ControlMode::Stick,
            joystick_size: 100,
            left_handed: false,
            tilt_range_deg: 25,
            input_falloff: 100,
            unlock_all: false,
        }
    }
}

impl Settings {
    pub fn clamp(&mut self) {
        self.music_volume = self.music_volume.min(100);
        self.sfx_volume = self.sfx_volume.min(100);
        self.announcer_volume = self.announcer_volume.min(100);
        self.joystick_size = self.joystick_size.clamp(50, 200);
        self.tilt_range_deg = self.tilt_range_deg.clamp(10, 25);
        self.input_falloff = self.input_falloff.clamp(100, 200);
    }

    fn set(&mut self, key: &str, value: &str) {
        let num = value.parse::<u32>().ok();
        match (key, num) {
            ("music_volume", Some(n)) => self.music_volume = n.min(255) as u8,
            ("sfx_volume", Some(n)) => self.sfx_volume = n.min(255) as u8,
            ("announcer_volume", Some(n)) => self.announcer_volume = n.min(255) as u8,
            ("joystick_size", Some(n)) => self.joystick_size = n.min(u16::MAX as u32) as u16,
            ("tilt_range_deg", Some(n)) => self.tilt_range_deg = n.min(255) as u8,
            ("input_falloff", Some(n)) => self.input_falloff = n.min(u16::MAX as u32) as u16,
            ("left_handed", Some(n)) => self.left_handed = n != 0,
            ("unlock_all", Some(n)) => self.unlock_all = n != 0,
            ("control_mode", _) => {
                if let Some(m) = ControlMode::from_key(value) {
                    self.control_mode = m;
                }
            }
            _ => {}
        }
    }

    fn write(&self, out: &mut String) {
        let mut line = |k: &str, v: String| {
            let _ = writeln!(out, "setting {k} {v}");
        };
        line("music_volume", self.music_volume.to_string());
        line("sfx_volume", self.sfx_volume.to_string());
        line("announcer_volume", self.announcer_volume.to_string());
        line("control_mode", self.control_mode.key().to_string());
        line("joystick_size", self.joystick_size.to_string());
        line("left_handed", (self.left_handed as u8).to_string());
        line("tilt_range_deg", self.tilt_range_deg.to_string());
        line("input_falloff", self.input_falloff.to_string());
        line("unlock_all", (self.unlock_all as u8).to_string());
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CourseProgress {
    /// Stage ids that have been reached in this course (unlocks them for practice).
    pub reached: BTreeSet<u32>,
    pub cleared: bool,
    /// Cleared without using a continue (unlocks the Extra course).
    pub cleared_clean: bool,
    pub best_score: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StageBest {
    /// Fewest elapsed frames over all goal clears, 0 = never cleared.
    pub best_frames: u32,
    pub clears: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    courses: BTreeMap<Difficulty, CourseProgress>,
    stages: BTreeMap<u32, StageBest>,
}

impl Progress {
    pub fn course(&self, d: Difficulty) -> CourseProgress {
        self.courses.get(&d).cloned().unwrap_or_default()
    }

    fn course_mut(&mut self, d: Difficulty) -> &mut CourseProgress {
        self.courses.entry(d).or_default()
    }

    /// Main courses are always open. An Extra course opens when its main course was cleared without a
    /// continue; Master opens once all three Extra courses are cleared.
    pub fn is_course_unlocked(&self, d: Difficulty, unlock_all: bool) -> bool {
        if unlock_all {
            return true;
        }
        match d {
            Difficulty::Beginner | Difficulty::Advanced | Difficulty::Expert => true,
            Difficulty::BeginnerExtra => self.course(Difficulty::Beginner).cleared_clean,
            Difficulty::AdvancedExtra => self.course(Difficulty::Advanced).cleared_clean,
            Difficulty::ExpertExtra => self.course(Difficulty::Expert).cleared_clean,
            Difficulty::Master => [
                Difficulty::BeginnerExtra,
                Difficulty::AdvancedExtra,
                Difficulty::ExpertExtra,
            ]
            .into_iter()
            .all(|e| self.course(e).cleared),
        }
    }

    /// The first stage of a course is always open; others once reached in course play.
    pub fn is_stage_unlocked(&self, d: Difficulty, stage_id: u32, unlock_all: bool) -> bool {
        if unlock_all {
            return true;
        }
        let first = crate::course::stage_list(d).first().map(|e| e.id);
        first == Some(stage_id)
            || self
                .courses
                .get(&d)
                .is_some_and(|c| c.reached.contains(&stage_id))
    }

    pub fn mark_reached(&mut self, d: Difficulty, stage_id: u32) {
        self.course_mut(d).reached.insert(stage_id);
    }

    pub fn record_score(&mut self, d: Difficulty, score: u64) {
        let c = self.course_mut(d);
        c.best_score = c.best_score.max(score.min(MAX_SCORE));
    }

    pub fn record_course_clear(&mut self, d: Difficulty, clean: bool) {
        let c = self.course_mut(d);
        c.cleared = true;
        c.cleared_clean |= clean;
    }

    /// Returns true when `frames` is a new best for the stage.
    pub fn record_stage_clear(&mut self, stage_id: u32, frames: u32) -> bool {
        let s = self.stages.entry(stage_id).or_default();
        s.clears = s.clears.saturating_add(1);
        if s.best_frames == 0 || frames < s.best_frames {
            s.best_frames = frames.max(1);
            return true;
        }
        false
    }

    pub fn stage_best(&self, stage_id: u32) -> StageBest {
        self.stages.get(&stage_id).copied().unwrap_or_default()
    }
}

/// Everything that is saved.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    pub settings: Settings,
    pub progress: Progress,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// Not a save file (missing header) or not UTF-8.
    NotASave,
    /// Written by a newer, incompatible version.
    UnsupportedVersion(u32),
    /// Checksum missing or wrong.
    Corrupt,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotASave => write!(f, "not a save file"),
            LoadError::UnsupportedVersion(v) => {
                write!(f, "save version {v} is newer than supported")
            }
            LoadError::Corrupt => write!(f, "save file is damaged"),
        }
    }
}

fn fnv1a(data: &[u8]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for &b in data {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

impl Profile {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = format!("MBSAVE {SAVE_VERSION}\n");
        self.settings.write(&mut out);
        for d in Difficulty::ALL {
            let Some(c) = self.progress.courses.get(&d) else {
                continue;
            };
            let reached = if c.reached.is_empty() {
                "-".to_string()
            } else {
                c.reached
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let _ = writeln!(
                out,
                "course {} {} {} {} {}",
                d.key(),
                c.cleared as u8,
                c.cleared_clean as u8,
                c.best_score,
                reached
            );
        }
        for (id, s) in &self.progress.stages {
            let _ = writeln!(out, "stage {id} {} {}", s.best_frames, s.clears);
        }
        let crc = fnv1a(out.as_bytes());
        let _ = writeln!(out, "crc {crc:08x}");
        out.into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Result<Profile, LoadError> {
        let text = std::str::from_utf8(bytes).map_err(|_| LoadError::NotASave)?;
        let mut lines = text.lines();
        let header = lines.next().ok_or(LoadError::NotASave)?;
        let version: u32 = header
            .strip_prefix("MBSAVE ")
            .and_then(|v| v.trim().parse().ok())
            .ok_or(LoadError::NotASave)?;
        if version > SAVE_VERSION {
            return Err(LoadError::UnsupportedVersion(version));
        }
        // Checksum covers every byte before the final "crc" line.
        let crc_at = text.rfind("crc ").ok_or(LoadError::Corrupt)?;
        if crc_at != 0 && text.as_bytes()[crc_at - 1] != b'\n' {
            return Err(LoadError::Corrupt);
        }
        let want =
            u32::from_str_radix(text[crc_at + 4..].trim(), 16).map_err(|_| LoadError::Corrupt)?;
        if fnv1a(&text.as_bytes()[..crc_at]) != want {
            return Err(LoadError::Corrupt);
        }

        let mut p = Profile::default();
        for line in text[..crc_at].lines().skip(1) {
            let mut it = line.split_whitespace();
            match it.next() {
                Some("setting") => {
                    if let (Some(k), Some(v)) = (it.next(), it.next()) {
                        p.settings.set(k, v);
                    }
                }
                Some("course") => {
                    let f: Vec<&str> = it.collect();
                    if f.len() < 5 {
                        return Err(LoadError::Corrupt);
                    }
                    // Unknown courses (from a newer build) are skipped.
                    let Some(d) = Difficulty::from_key(f[0]) else {
                        continue;
                    };
                    let reached = if f[4] == "-" {
                        BTreeSet::new()
                    } else {
                        f[4].split(',')
                            .map(|s| s.parse::<u32>().map_err(|_| LoadError::Corrupt))
                            .collect::<Result<_, _>>()?
                    };
                    p.progress.courses.insert(
                        d,
                        CourseProgress {
                            cleared: f[1] == "1",
                            cleared_clean: f[2] == "1",
                            best_score: f[3]
                                .parse::<u64>()
                                .map_err(|_| LoadError::Corrupt)?
                                .min(MAX_SCORE),
                            reached,
                        },
                    );
                }
                Some("stage") => {
                    let f: Vec<u32> = it.filter_map(|s| s.parse().ok()).collect();
                    if f.len() < 3 {
                        return Err(LoadError::Corrupt);
                    }
                    p.progress.stages.insert(
                        f[0],
                        StageBest {
                            best_frames: f[1],
                            clears: f[2],
                        },
                    );
                }
                _ => {}
            }
        }
        p.settings.clamp();
        Ok(p)
    }

    /// Load from storage. A missing file gives defaults silently; an unreadable or damaged one gives
    /// defaults plus a message, and the bad bytes are kept under `profile.corrupt` so nothing is lost.
    pub fn load(storage: &mut dyn Storage) -> (Profile, Option<String>) {
        match storage.read(PROFILE_KEY) {
            Ok(None) => (Profile::default(), None),
            Ok(Some(bytes)) => match Profile::decode(&bytes) {
                Ok(p) => (p, None),
                Err(e) => {
                    let _ = storage.write(BACKUP_KEY, &bytes);
                    (
                        Profile::default(),
                        Some(format!("Save data was reset ({e})")),
                    )
                }
            },
            Err(e) => (
                Profile::default(),
                Some(format!("Could not read save data: {e}")),
            ),
        }
    }

    pub fn save(&self, storage: &mut dyn Storage) -> std::io::Result<()> {
        storage.write(PROFILE_KEY, &self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MemoryStorage;

    fn sample() -> Profile {
        let mut p = Profile::default();
        p.settings.music_volume = 80;
        p.settings.control_mode = ControlMode::Tilt;
        p.settings.left_handed = true;
        p.settings.tilt_range_deg = 12;
        p.progress.mark_reached(Difficulty::Beginner, 1);
        p.progress.mark_reached(Difficulty::Beginner, 91);
        p.progress.record_score(Difficulty::Beginner, 123_456);
        p.progress.record_course_clear(Difficulty::Beginner, true);
        p.progress.record_stage_clear(1, 1500);
        p
    }

    #[test]
    fn round_trip() {
        let p = sample();
        let q = Profile::decode(&p.encode()).unwrap();
        assert_eq!(p, q);
        assert_eq!(q.settings.control_mode, ControlMode::Tilt);
        assert_eq!(q.progress.stage_best(1).best_frames, 1500);
    }

    #[test]
    fn default_profile_round_trips() {
        let p = Profile::default();
        assert_eq!(Profile::decode(&p.encode()).unwrap(), p);
    }

    #[test]
    fn bit_flip_is_detected() {
        let mut bytes = sample().encode();
        let at = bytes.iter().position(|&b| b == b'8').unwrap();
        bytes[at] = b'9';
        assert_eq!(Profile::decode(&bytes), Err(LoadError::Corrupt));
    }

    #[test]
    fn truncation_and_garbage_are_rejected() {
        let bytes = sample().encode();
        for cut in [0, 5, bytes.len() / 2, bytes.len() - 5] {
            assert!(Profile::decode(&bytes[..cut]).is_err(), "cut {cut}");
        }
        assert_eq!(Profile::decode(b"hello"), Err(LoadError::NotASave));
        assert_eq!(
            Profile::decode(&[0xff, 0xfe, 0x00]),
            Err(LoadError::NotASave)
        );
        assert_eq!(Profile::decode(b"MBSAVE 1\n"), Err(LoadError::Corrupt));
    }

    #[test]
    fn newer_version_is_refused_not_misread() {
        assert_eq!(
            Profile::decode(b"MBSAVE 9\ncrc 00000000\n"),
            Err(LoadError::UnsupportedVersion(9))
        );
    }

    #[test]
    fn unknown_records_and_out_of_range_values_are_tolerated() {
        let mut body = String::from(
            "MBSAVE 1\nsetting music_volume 250\nsetting future_thing 7\nsetting joystick_size 5\n\
             course nightmare 1 1 5 -\nmystery a b c\n",
        );
        let crc = fnv1a(body.as_bytes());
        body.push_str(&format!("crc {crc:08x}\n"));
        let p = Profile::decode(body.as_bytes()).unwrap();
        assert_eq!(p.settings.music_volume, 100);
        assert_eq!(p.settings.joystick_size, 50);
        assert_eq!(p.progress, Progress::default());
    }

    #[test]
    fn load_handles_missing_and_corrupt_files() {
        let mut s = MemoryStorage::new();
        let (p, warn) = Profile::load(&mut s);
        assert_eq!((p, warn), (Profile::default(), None));

        sample().save(&mut s).unwrap();
        let (p, warn) = Profile::load(&mut s);
        assert_eq!(p, sample());
        assert!(warn.is_none());

        s.write(
            PROFILE_KEY,
            b"MBSAVE 1\nsetting music_volume 1\ncrc 12345678\n",
        )
        .unwrap();
        let (p, warn) = Profile::load(&mut s);
        assert_eq!(p, Profile::default());
        assert!(warn.unwrap().contains("reset"));
        assert!(s.read(BACKUP_KEY).unwrap().is_some());
    }

    #[test]
    fn unlock_rules() {
        let mut pr = Progress::default();
        assert!(pr.is_course_unlocked(Difficulty::Expert, false));
        assert!(!pr.is_course_unlocked(Difficulty::BeginnerExtra, false));
        assert!(pr.is_course_unlocked(Difficulty::BeginnerExtra, true));
        pr.record_course_clear(Difficulty::Beginner, false);
        assert!(
            !pr.is_course_unlocked(Difficulty::BeginnerExtra, false),
            "needs a clean clear"
        );
        pr.record_course_clear(Difficulty::Beginner, true);
        assert!(pr.is_course_unlocked(Difficulty::BeginnerExtra, false));
        assert!(!pr.is_course_unlocked(Difficulty::Master, false));
        for d in [
            Difficulty::BeginnerExtra,
            Difficulty::AdvancedExtra,
            Difficulty::ExpertExtra,
        ] {
            pr.record_course_clear(d, false);
        }
        assert!(pr.is_course_unlocked(Difficulty::Master, false));
    }

    #[test]
    fn stage_unlocks_follow_reached_floors() {
        let mut pr = Progress::default();
        assert!(pr.is_stage_unlocked(Difficulty::Beginner, 1, false));
        assert!(!pr.is_stage_unlocked(Difficulty::Beginner, 2, false));
        pr.mark_reached(Difficulty::Beginner, 2);
        assert!(pr.is_stage_unlocked(Difficulty::Beginner, 2, false));
        assert!(!pr.is_stage_unlocked(Difficulty::Advanced, 2, false));
        assert!(pr.is_stage_unlocked(Difficulty::Advanced, 2, true));
    }

    #[test]
    fn best_times_keep_the_fastest() {
        let mut pr = Progress::default();
        assert!(pr.record_stage_clear(5, 2000));
        assert!(!pr.record_stage_clear(5, 2500));
        assert!(pr.record_stage_clear(5, 1800));
        assert_eq!(
            pr.stage_best(5),
            StageBest {
                best_frames: 1800,
                clears: 3
            }
        );
        assert_eq!(pr.stage_best(6), StageBest::default());
    }

    #[test]
    fn best_score_only_increases_and_is_capped() {
        let mut pr = Progress::default();
        pr.record_score(Difficulty::Expert, 500);
        pr.record_score(Difficulty::Expert, 100);
        assert_eq!(pr.course(Difficulty::Expert).best_score, 500);
        pr.record_score(Difficulty::Expert, u64::MAX);
        assert_eq!(pr.course(Difficulty::Expert).best_score, MAX_SCORE);
    }
}
