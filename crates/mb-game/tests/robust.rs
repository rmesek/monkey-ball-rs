//! Edge cases of the save format and of the course state machine: damaged or hostile save data loads to
//! defaults quickly with the bad bytes kept, and every sequence of stage outcomes ends in a state the menu
//! can leave.

use mb_game::progress::{BACKUP_KEY, LoadError, PROFILE_KEY, Profile};
use mb_game::{
    CourseRun, Difficulty, GoalType, MemoryStorage, Progress, Rules, StageOutcome, Step, Storage,
};
use std::time::Instant;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
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

/// A save file with a correct checksum around `body` (the lines after the header).
fn signed(body: &str) -> Vec<u8> {
    let mut text = format!("MBSAVE 1\n{body}");
    let crc = fnv1a(text.as_bytes());
    text.push_str(&format!("crc {crc:08x}\n"));
    text.into_bytes()
}

fn sample() -> Profile {
    let mut p = Profile::default();
    p.settings.music_volume = 80;
    p.settings.unlock_all = true;
    for d in Difficulty::ALL {
        for (i, e) in mb_game::course::stage_list(d).iter().enumerate() {
            p.progress.mark_reached(d, e.id);
            p.progress.record_stage_clear(e.id, 600 + i as u32);
        }
        p.progress.record_score(d, 123_456);
        p.progress.record_course_clear(d, true);
    }
    p
}

/// Loads `bytes` as the stored profile. A rejected file must give defaults, a message, and the same bytes
/// under the backup key.
fn load(bytes: &[u8]) -> (Profile, Option<String>) {
    let mut s = MemoryStorage::new();
    s.write(PROFILE_KEY, bytes).unwrap();
    let (p, warning) = Profile::load(&mut s);
    match &warning {
        Some(_) => {
            assert_eq!(p, Profile::default());
            assert_eq!(s.read(BACKUP_KEY).unwrap().as_deref(), Some(bytes));
        }
        None => assert_eq!(s.read(BACKUP_KEY).unwrap(), None),
    }
    // Whatever was loaded can be saved and read back unchanged.
    p.save(&mut s).unwrap();
    assert_eq!(Profile::load(&mut s), (p.clone(), None));
    (p, warning)
}

#[test]
fn every_truncation_and_single_byte_change_is_rejected_or_harmless() {
    let good = sample().encode();
    assert_eq!(load(&good), (sample(), None));
    for len in 0..good.len() {
        let (p, warning) = load(&good[..len]);
        // Only the final newline can go missing without loss.
        assert!(
            warning.is_some() || (len == good.len() - 1 && p == sample()),
            "truncation to {len} was accepted"
        );
    }
    let mut b = good.clone();
    for i in 0..good.len() {
        for v in [0x00, b'\n', b' ', b'9', 0x80, 0xff, good[i] ^ 1] {
            if v == good[i] {
                continue;
            }
            b[i] = v;
            let (p, warning) = load(&b);
            // A changed byte either breaks the checksum or lands in the trailing newline.
            assert!(
                warning.is_some() || p == sample(),
                "byte {i} = {v:#x} silently changed the profile"
            );
        }
        b[i] = good[i];
    }
}

#[test]
fn random_damage_never_panics() {
    let good = sample().encode();
    let mut rng = Rng(0x5eed_0040);
    for _ in 0..20_000 {
        let mut b = good.clone();
        match rng.below(4) {
            0 => {
                for _ in 0..1 + rng.below(6) {
                    let i = rng.below(b.len());
                    b[i] = rng.next() as u8;
                }
            }
            1 => {
                let at = rng.below(b.len());
                let n = rng.below(40);
                let ins: Vec<u8> = (0..n).map(|_| rng.next() as u8).collect();
                b.splice(at..at, ins);
            }
            2 => {
                let at = rng.below(b.len());
                let n = rng.below(b.len() - at);
                b.drain(at..at + n);
            }
            _ => {
                // Shuffle whole lines: the checksum line can end up anywhere.
                let mut lines: Vec<&[u8]> = good.split(|&c| c == b'\n').collect();
                let (i, j) = (rng.below(lines.len()), rng.below(lines.len()));
                lines.swap(i, j);
                b = lines.join(&b'\n');
            }
        }
        let _ = load(&b);
    }
}

/// Files with a valid checksum whose records are nonsense: the checksum only proves the file is what some
/// program wrote.
#[test]
fn signed_nonsense_is_tolerated_or_reset() {
    // Numbers beyond every field.
    let huge = "9".repeat(400);
    for body in [
        format!("setting music_volume {huge}\n"),
        format!("setting joystick_size {huge}\nsetting input_falloff 4294967295\n"),
        "setting music_volume -5\nsetting tilt_range_deg 0\nsetting unlock_all 2\n".to_string(),
        "setting\nsetting x\nsetting control_mode sideways\n\n\n   \n".to_string(),
    ] {
        let (p, warning) = load(&signed(&body));
        assert!(warning.is_none(), "{body:?}");
        assert!(p.settings.music_volume <= 100);
        assert!((50..=200).contains(&p.settings.joystick_size));
        assert!((100..=200).contains(&p.settings.input_falloff));
        assert!((10..=25).contains(&p.settings.tilt_range_deg));
    }
    for body in [
        format!("course beginner 1 1 {huge} -\n"),
        "course beginner 1 1 5 1,2,x\n".to_string(),
        "course beginner 1 1 5 4294967296\n".to_string(),
        "course beginner 1 1\n".to_string(),
        format!("stage 1 {huge} 3\n"),
        "stage 1 -3 3\n".to_string(),
        "stage 1 x 100 3\n".to_string(),
        "stage 1 2\n".to_string(),
    ] {
        let (_, warning) = load(&signed(&body));
        assert!(warning.is_some(), "{body:?} was accepted");
    }
    assert_eq!(
        Profile::decode(&signed("stage 1 x 100 3\n")),
        Err(LoadError::Corrupt)
    );

    // Scores are capped, flags are strict, later records replace earlier ones.
    let (p, warning) = load(&signed(
        "course expert 1 0 18446744073709551615 121,121,121,4294967295\n\
         course expert 7 yes 10 -\n\
         stage 5 100 1\nstage 5 0 4294967295\nstage 4294967295 4294967295 4294967295\n\
         setting music_volume 10\nsetting music_volume 20\n",
    ));
    assert!(warning.is_none());
    let c = p.progress.course(Difficulty::Expert);
    assert_eq!(
        (c.cleared, c.cleared_clean, c.best_score),
        (false, false, 10)
    );
    assert!(c.reached.is_empty());
    assert_eq!(p.progress.stage_best(5).clears, u32::MAX);
    assert_eq!(p.settings.music_volume, 20);
    // A record that is already at its limits can still be played on.
    let mut progress = p.progress.clone();
    assert!(progress.record_stage_clear(5, 300));
    assert!(!progress.record_stage_clear(u32::MAX, u32::MAX));
    assert_eq!(progress.stage_best(5).clears, u32::MAX);

    // The first score line above, alone: capped at the nine digits the HUD has.
    let (p, _) = load(&signed("course expert 1 0 18446744073709551615 -\n"));
    assert_eq!(
        p.progress.course(Difficulty::Expert).best_score,
        999_999_999
    );
}

#[test]
fn ten_megabytes_of_anything_load_to_defaults_quickly() {
    const SIZE: usize = 10 << 20;
    let mut rng = Rng(0x5eed_0041);
    let random: Vec<u8> = (0..SIZE).map(|_| rng.next() as u8).collect();
    let ascii: Vec<u8> = random.iter().map(|b| b' ' + b % 90).collect();
    let mut headed = b"MBSAVE 1\n".to_vec();
    headed.extend(ascii.iter().take(SIZE));
    let one_line = [
        b"MBSAVE 1\nsetting ".as_slice(),
        &vec![b'7'; SIZE],
        b"\ncrc 0\n",
    ]
    .concat();
    let many_crcs = b"MBSAVE 1\ncrc 00000000\n".repeat(SIZE / 22);
    let newlines = [b"MBSAVE 1".as_slice(), &vec![b'\n'; SIZE]].concat();
    let mut bad_utf8 = sample().encode();
    bad_utf8.extend_from_slice(&[0xc3, 0x28, 0xff]);
    // A real, correctly signed save that is simply enormous (a million stage records) loads as itself.
    let mut body = String::with_capacity(SIZE);
    for i in 0..600_000u32 {
        body.push_str(&format!("stage {i} {} 1\n", i + 1));
    }
    let big_valid = signed(&body);

    let cases: [(&str, &[u8], bool); 8] = [
        ("random bytes", &random, false),
        ("printable garbage", &ascii, false),
        ("header then garbage", &headed, false),
        ("one 10 MB line", &one_line, false),
        ("checksum lines only", &many_crcs, false),
        ("newlines", &newlines, false),
        ("valid save with bytes that are not UTF-8", &bad_utf8, false),
        ("enormous valid save", &big_valid, true),
    ];
    for (name, bytes, valid) in cases {
        let started = Instant::now();
        let decoded = Profile::decode(bytes);
        let took = started.elapsed();
        assert_eq!(decoded.is_ok(), valid, "{name}");
        // Far above what it takes (tens of milliseconds), far below a hang.
        assert!(took.as_secs() < 5, "{name}: {took:?}");
        if !valid {
            let mut s = MemoryStorage::new();
            s.write(PROFILE_KEY, bytes).unwrap();
            let (p, warning) = Profile::load(&mut s);
            assert_eq!(p, Profile::default(), "{name}");
            assert!(warning.is_some(), "{name}");
            assert!(
                s.read(BACKUP_KEY).unwrap().as_deref() == Some(bytes),
                "{name}"
            );
        }
    }
    assert_eq!(
        Profile::decode(&big_valid)
            .unwrap()
            .progress
            .stage_best(599_999)
            .best_frames,
        600_000
    );
}

/// A storage that fails every operation: loading and saving report it and nothing panics.
#[test]
fn failing_storage_is_reported_not_fatal() {
    struct Broken;
    impl Storage for Broken {
        fn read(&self, _: &str) -> std::io::Result<Option<Vec<u8>>> {
            Err(std::io::Error::other("no storage"))
        }
        fn write(&mut self, _: &str, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("quota exceeded"))
        }
        fn remove(&mut self, _: &str) -> std::io::Result<()> {
            Err(std::io::Error::other("no storage"))
        }
    }
    let (p, warning) = Profile::load(&mut Broken);
    assert_eq!(p, Profile::default());
    assert!(warning.unwrap().contains("no storage"));
    assert!(p.save(&mut Broken).is_err());

    /// Reads a damaged save but cannot store the backup (a full `localStorage`).
    struct ReadOnly(Vec<u8>);
    impl Storage for ReadOnly {
        fn read(&self, _: &str) -> std::io::Result<Option<Vec<u8>>> {
            Ok(Some(self.0.clone()))
        }
        fn write(&mut self, _: &str, _: &[u8]) -> std::io::Result<()> {
            Err(std::io::Error::other("quota exceeded"))
        }
        fn remove(&mut self, _: &str) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (p, warning) = Profile::load(&mut ReadOnly(b"junk".to_vec()));
    assert_eq!(p, Profile::default());
    assert!(warning.is_some());
}

fn random_outcome(rng: &mut Rng) -> StageOutcome {
    let n = |rng: &mut Rng| [0, 1, 59, 60, 1800, 1801, 3600, 3601, u32::MAX][rng.below(9)];
    let bananas = [0, 1, 10, 99, 100, 101, 250, u32::MAX][rng.below(8)];
    match rng.below(6) {
        0 => StageOutcome::Goal {
            goal: GoalType::Blue,
            time_left: n(rng),
            bananas,
        },
        1 => StageOutcome::Goal {
            goal: GoalType::Green,
            time_left: n(rng),
            bananas,
        },
        2 => StageOutcome::Goal {
            goal: GoalType::Red,
            time_left: n(rng),
            bananas,
        },
        3 => StageOutcome::BonusClear {
            time_left: n(rng),
            bananas,
        },
        4 => StageOutcome::FallOut { bananas },
        _ => StageOutcome::TimeOver { bananas },
    }
}

/// Random walks through Course Play, with outcomes the gameplay state could never legitimately send
/// (a bonus clear on a normal stage, more time left than the limit, four billion bananas). Every run must
/// reach a terminal step, the terminal step must be stable, and the numbers must stay in range.
#[test]
fn every_course_run_ends_in_a_state_the_menu_can_leave() {
    let mut rng = Rng(0x5eed_0042);
    let (mut completed, mut over, mut continued) = (0, 0, 0);
    for i in 0..4000 {
        let difficulty = Difficulty::ALL[i % 7];
        let rules = match i % 5 {
            0 => Rules {
                lives: 0,
                continues: 0,
                ..Rules::default()
            },
            1 => Rules {
                lives: u32::MAX,
                max_lives: 1,
                bananas_per_extra_life: 0,
                continues: 1,
                ..Rules::default()
            },
            2 => Rules {
                banana_score: u64::MAX,
                green_goal_score: u64::MAX,
                red_goal_score: u64::MAX,
                bananas_per_extra_life: 1,
                max_lives: u32::MAX,
                ..Rules::default()
            },
            _ => Rules::default(),
        };
        let mut run = CourseRun::with_rules(difficulty, rules);
        let mut progress = Progress::default();
        let floors = run.floor().floors;
        let mut last_floor = run.floor().floor;
        let mut steps = 0;
        loop {
            steps += 1;
            // Lives are finite and continues are finite, so a run cannot go on forever: with at most 99
            // lives per credit... except when bananas keep paying lives, so bound it generously.
            assert!(steps < 200_000, "run {i} does not end");
            let floor = run.floor();
            assert!(floor.floor >= 1 && floor.floor <= floors, "{floor:?}");
            assert!(floor.floor >= last_floor, "the course went backwards");
            last_floor = floor.floor;
            assert!(floor.score <= 999_999_999);
            assert!(!floor.bg.is_empty() && mb_game::stages::is_playable_stage(floor.stage_id));
            // Mostly failures, so game overs and continues are exercised.
            let outcome = if rng.below(3) == 0 {
                random_outcome(&mut rng)
            } else {
                StageOutcome::FallOut { bananas: 0 }
            };
            match run.apply(outcome, &mut progress) {
                Step::Next { floor, skipped } => {
                    assert!(skipped < floors && floor.floor > last_floor);
                    assert!(!floor.retry);
                }
                Step::Retry(floor) => {
                    assert!(floor.retry && floor.floor == last_floor);
                    assert!(floor.lives >= 1);
                }
                Step::GameOver {
                    score,
                    continues_left,
                } => {
                    assert!(run.is_game_over() && score <= 999_999_999);
                    assert_eq!(continues_left, run.continues_left());
                    // Applying more outcomes to a finished credit changes nothing.
                    let before = progress.clone();
                    assert!(matches!(
                        run.apply(random_outcome(&mut rng), &mut progress),
                        Step::GameOver { .. }
                    ));
                    assert_eq!(progress, before);
                    if continues_left > 0 && rng.below(4) != 0 {
                        continued += 1;
                        match run.continue_run() {
                            Some(Step::Retry(f)) => {
                                assert_eq!((f.score, f.floor), (0, last_floor));
                                assert!(f.lives >= 1 || rules.lives == 0);
                            }
                            other => panic!("continue gave {other:?}"),
                        }
                        if rules.lives == 0 {
                            // A rule set with no lives: the continue is consumed and the next failure
                            // ends the credit again; still finite.
                            continue;
                        }
                    } else {
                        if continues_left == 0 {
                            assert_eq!(run.continue_run(), None);
                        }
                        over += 1;
                        break;
                    }
                }
                Step::CourseComplete { score, .. } => {
                    assert!(run.is_complete() && score <= 999_999_999);
                    assert_eq!(run.continue_run(), None);
                    assert!(matches!(
                        run.apply(random_outcome(&mut rng), &mut progress),
                        Step::CourseComplete { .. }
                    ));
                    assert!(progress.course(difficulty).cleared);
                    completed += 1;
                    break;
                }
                Step::PracticeClear { .. } => panic!("practice step in course play"),
            }
        }
        // The records written along the way survive a save and a load.
        let profile = Profile {
            progress,
            ..Profile::default()
        };
        assert_eq!(Profile::decode(&profile.encode()).as_ref(), Ok(&profile));
    }
    assert!(
        completed > 20 && over > 500 && continued > 500,
        "{completed} {over} {continued}"
    );
}

#[test]
fn practice_never_ends_and_never_counts() {
    let mut rng = Rng(0x5eed_0043);
    for d in Difficulty::ALL {
        for index in [0, 1, 7, 49, 50, usize::MAX] {
            let mut run = CourseRun::practice(d, index);
            let mut progress = Progress::default();
            let stage = run.floor().stage_id;
            for _ in 0..200 {
                let outcome = random_outcome(&mut rng);
                match run.apply(outcome, &mut progress) {
                    Step::Retry(f) => assert!(!outcome.is_clear() && f.stage_id == stage),
                    Step::PracticeClear { floor, .. } => {
                        assert!(outcome.is_clear() && floor.stage_id == stage)
                    }
                    other => panic!("practice gave {other:?}"),
                }
                assert_eq!((run.lives(), run.score(), run.bananas()), (0, 0, 0));
                assert!(!run.is_game_over() && !run.is_complete());
                assert_eq!(run.continue_run(), None);
            }
            assert_eq!(progress.course(d).best_score, 0);
        }
    }
}
