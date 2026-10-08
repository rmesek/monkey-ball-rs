//! Audio end to end, without an audio device: the real menu and gameplay states drive the audio system
//! while the autopilot plays Beginner floor 1.
//!
//! * `requests_follow_the_game`: a recording backend; the sequence of sound requests is checked.
//! * `offline_render_is_sane`: the software mixer renders the same run to PCM, which is measured (and
//!   written as WAV files to a scratch directory outside the repository: `MB_AUDIO_OUT`, or the system
//!   temporary directory).
//!
//! Both need the user's own disc image and a GPU adapter, and skip without them.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use mb_app::audio::{Audio, AudioBackend, BackendReport, Bus, Command, MixBackend, Recorded};
use mb_app::content::{open_disc_image, Content};
use mb_app::headless::{Headless, FRAME};
use mb_app::input::Action;
use mb_app::menu::Menu;
use mb_audio::driver::{mix_channel, MixDriver, MixMsg};
use mb_audio::mixer::Sound;
use mb_audio::wav::wav_bytes_f32;
use mb_game::MemoryStorage;

static GPU: Mutex<()> = Mutex::new(());

/// MusyX ids of the sounds this run must produce (USA disc; resolved from `main.dol` at run time).
const READY: u16 = 170;
const GO: u16 = 158;
const GOAL: u16 = 160;
const BANANA: u16 = 206;
const GOAL_TAPE: u16 = 211;
const TAPE_CUT: u16 = 734;
const GOAL_FLY: u16 = 733;
const TICK: u16 = 164;
const ROLL: [u16; 3] = [248, 273, 274];
const MENU_MOVE: u16 = 214;
const MENU_CONFIRM: u16 = 244;

fn find_disc() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("MB_DISC") {
        return Some(PathBuf::from(p));
    }
    let parent = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    std::fs::read_dir(parent).ok()?.flatten().find_map(|entry| {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        let image = [".rvz", ".iso", ".gcm"].iter().any(|e| name.ends_with(e));
        (name.starts_with("super monkey ball") && image).then(|| entry.path())
    })
}

/// A headless game at the title menu with `audio` attached, or `None` when the machine cannot run it.
fn start(audio: &Audio) -> Option<Headless> {
    let Some(disc) = find_disc() else {
        eprintln!("skipped: no disc image (set MB_DISC)");
        return None;
    };
    let content: Content = match open_disc_image(&disc) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipped: {}: {e}", disc.display());
            return None;
        }
    };
    let mut h = match Headless::new(&content, (320, 180), false) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("skipped: no GPU adapter ({e})");
            return None;
        }
    };
    let menu = Menu::new(Box::new(MemoryStorage::new())).with_audio(audio.clone());
    h.push(Box::new(menu)).expect("menu");
    Some(h)
}

/// What happened when, in frames since the run started.
#[derive(Debug, Default)]
struct Timeline {
    stage_start: usize,
    ready: usize,
    go: usize,
    goal: usize,
    floor_two: usize,
    end: usize,
    bananas: i32,
}

/// Title -> Course Play -> Beginner, floor 1 played by the autopilot through the goal, a moment of floor 2,
/// then pause -> quit back to the menu. `each_frame` runs after every frame.
fn play_floor_one(h: &mut Headless, each_frame: &mut dyn FnMut()) -> Timeline {
    let mut t = Timeline::default();
    let mut frames = 0usize;
    let mut step = |h: &mut Headless, tap: Option<Action>| {
        let running = match tap {
            Some(a) => h.tap(a).unwrap(),
            None => h.step(FRAME).unwrap(),
        };
        assert!(running, "the game quit");
        each_frame();
        frames += 1;
        frames
    };
    step(h, None);
    assert_eq!(h.status(), "menu: title");
    step(h, Some(Action::Confirm));
    assert_eq!(h.status(), "menu: course select");
    t.stage_start = step(h, Some(Action::Confirm));
    assert!(h.top().unwrap().play_view().is_some(), "floor 1 is running");

    let mut pilot = None;
    for _ in 0..6000 {
        assert!(h.pilot(&mut pilot, false), "left the stage");
        let now = step(h, None);
        let view = h.top().unwrap().play_view().expect("a stage is running");
        if view.request.floor == 2 {
            t.floor_two = now;
            break;
        }
        let sim = view.sim;
        t.bananas = sim.ball.bananas;
        if t.ready == 0 && sim.intro_timer <= 120 {
            t.ready = now;
        }
        if t.go == 0 && sim.intro_timer == 0 {
            t.go = now;
        }
        if t.goal == 0 && sim.goal_timer > 0 {
            t.goal = now;
        }
    }
    assert!(t.goal > 0, "the autopilot never reached the goal");
    assert!(t.floor_two > 0, "floor 2 never started");
    // Floor 2: let its READY call happen, then pause and quit from the pause menu.
    for _ in 0..300 {
        step(h, None);
    }
    step(h, Some(Action::Pause));
    assert!(h.top().unwrap().play_view().unwrap().paused);
    step(h, Some(Action::LookDown));
    step(h, Some(Action::LookDown));
    step(h, Some(Action::Confirm));
    // Quitting a course asks first; the second row is "quit course".
    step(h, Some(Action::LookDown));
    step(h, Some(Action::Confirm));
    for _ in 0..30 {
        step(h, None);
    }
    assert_eq!(h.status(), "menu: title");
    t.end = frames;
    t
}

fn plays(log: &[Recorded], sound: u16) -> Vec<usize> {
    log.iter()
        .enumerate()
        .filter(
            |(_, r)| matches!(r, Recorded::Command(Command::Play { sound: s, .. }) if *s == sound),
        )
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn requests_follow_the_game() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let (audio, log) = Audio::recording();
    let Some(mut h) = start(&audio) else {
        return;
    };
    let load = audio.load_report();
    assert!(load.sfx_loaded, "{:?}", load.errors);
    assert!(load.sfx_from_table, "ids come from the disc's main.dol");
    assert!(load.sfx_missing.is_empty(), "{:?}", load.sfx_missing);
    assert_eq!(load.sfx_count, mb_app::audio::Sfx::COUNT);

    let t = play_floor_one(&mut h, &mut || {});
    h.shutdown();
    assert_eq!(t.bananas, 4);
    let log = log.borrow();

    // Everything played was defined at load, before the first command.
    let defined: Vec<u16> = log
        .iter()
        .filter_map(|r| match r {
            Recorded::DefineSound(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(defined.len(), mb_app::audio::Sfx::COUNT);
    let first_command = log
        .iter()
        .position(|r| matches!(r, Recorded::Command(_)))
        .unwrap();
    let last_define = log
        .iter()
        .rposition(|r| matches!(r, Recorded::DefineSound(_)))
        .unwrap();
    assert!(
        last_define < first_command,
        "sounds are decoded at load only"
    );
    for r in log.iter() {
        if let Recorded::Command(Command::Play { sound, .. } | Command::LoopStart { sound, .. }) = r
        {
            assert!(defined.contains(sound), "sound {sound} was never defined");
        }
    }

    // Music: read and started once for both floors (same background), stopped back in the menu.
    let music_defs: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, Recorded::DefineMusic { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(music_defs.len(), 1, "one track for two jungle floors");
    assert!(matches!(
        log[music_defs[0]],
        Recorded::DefineMusic { intro_frames: None, body_frames } if body_frames > 48_000 * 30
    ));
    let music_plays: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, Recorded::Command(Command::MusicPlay { .. })))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(music_plays.len(), 1);
    assert_eq!(
        log[music_plays[0]],
        Recorded::Command(Command::MusicPlay {
            volume: 140.0 / 255.0
        })
    );
    let music_stop = log
        .iter()
        .position(|r| matches!(r, Recorded::Command(Command::MusicStop { .. })))
        .expect("music stops in the menu");
    let cleared = log
        .iter()
        .position(|r| *r == Recorded::ClearMusic)
        .expect("the track is released in the menu");

    // The order of the stage's sounds.
    let ready = plays(&log, READY);
    let go = plays(&log, GO);
    let bananas = plays(&log, BANANA);
    let tape = plays(&log, GOAL_TAPE);
    let cut = plays(&log, TAPE_CUT);
    let goal = plays(&log, GOAL);
    let fly = plays(&log, GOAL_FLY);
    assert_eq!(ready.len(), 2, "READY on floor 1 and on floor 2");
    assert_eq!((go.len(), bananas.len()), (1, 4));
    assert_eq!((tape.len(), cut.len(), goal.len(), fly.len()), (1, 1, 1, 1));
    let roll_start = log
        .iter()
        .position(|r| matches!(r, Recorded::Command(Command::LoopStart { slot: 0, sound, .. }) if *sound == ROLL[0]))
        .expect("the ball rolls");
    // The ball already rolls a little when it is dropped in, before GO, as in the reference.
    assert!(roll_start > ready[0] && roll_start < bananas[0]);
    let order = [
        music_plays[0],
        ready[0],
        go[0],
        bananas[0],
        bananas[3],
        tape[0],
        cut[0],
        goal[0],
        fly[0],
        ready[1],
        cleared.min(music_stop),
    ];
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    assert!(matches!(
        log[goal[0]],
        Recorded::Command(Command::Play { bus: Bus::Announcer, delay, .. }) if delay == 0.5
    ));
    assert!(matches!(
        log[bananas[0]],
        Recorded::Command(Command::Play { bus: Bus::Sfx, delay, rate, .. }) if delay == 0.0 && rate == 1.0
    ));

    // The roll: three layers started together, updated live, never left running.
    for (slot, sound) in ROLL.iter().enumerate() {
        assert!(matches!(
            log[roll_start + slot],
            Recorded::Command(Command::LoopStart { slot: s, sound: id, bus: Bus::Sfx, .. })
                if s as usize == slot && id == *sound
        ));
    }
    let updates = log
        .iter()
        .filter(|r| matches!(r, Recorded::Command(Command::LoopSet { .. })))
        .count();
    assert!(updates > 30, "{updates} live roll updates");
    for r in log.iter() {
        if let Recorded::Command(Command::LoopSet { volume, rate, .. }) = r {
            assert!((0.0..=1.0).contains(volume) && (0.9..1.1).contains(rate));
        }
    }
    // At most three loop updates per eight ticks: parameter changes are batched, not per frame.
    assert!(updates <= (t.floor_two - t.go) / 8 * 3 + 3, "{updates}");
    let stops: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, Recorded::Command(Command::StopSounds)))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(stops.len(), 2, "one cut per stage exit");
    assert!(stops[0] > fly[0] && stops[0] < ready[1]);
    // After a cut nothing loops until a new start.
    let mut looping = false;
    for r in log.iter() {
        match r {
            Recorded::Command(Command::LoopStart { .. }) => looping = true,
            Recorded::Command(Command::LoopStop { .. } | Command::StopSounds) => looping = false,
            Recorded::Command(Command::LoopSet { .. }) => {
                assert!(looping, "a loop was updated without being started")
            }
            _ => {}
        }
    }
    assert!(!looping, "a loop was left running at the end");

    // The clock ticks once a second of play, quieter outside the last ten seconds.
    let ticks = plays(&log, TICK);
    let played_seconds = (t.goal - t.go) / 60;
    assert!(
        ticks.len() >= played_seconds.saturating_sub(1) && ticks.len() <= played_seconds + 1,
        "{} ticks in {played_seconds} s",
        ticks.len()
    );
    // Menus: two confirms into the course, then two moves in the pause menu and one in the "quit the
    // course?" question (whose confirm is cut by the quit).
    assert!(plays(&log, MENU_CONFIRM).len() >= 2);
    assert_eq!(plays(&log, MENU_MOVE).len(), 3);
    // The category volumes go out once (the defaults: 50 / 30 / 30), before anything plays; no suspend.
    let volumes: Vec<&Recorded> = log
        .iter()
        .filter(|r| matches!(r, Recorded::Command(Command::BusVolume { .. })))
        .collect();
    assert_eq!(volumes.len(), 3);
    assert_eq!(
        log[first_command],
        Recorded::Command(Command::BusVolume {
            bus: Bus::Music,
            volume: 0.5
        })
    );
    assert!(!log.iter().any(|r| matches!(r, Recorded::Suspended(_))));
}

/// Forwards everything to two software-mixed backends: the full mix and one whose music bus is muted, so
/// effects can be measured without the music on top.
struct Tee(MixBackend, MixBackend);

impl AudioBackend for Tee {
    fn define_sound(&mut self, id: u16, sound: Arc<Sound>) {
        self.0.define_sound(id, sound.clone());
        self.1.define_sound(id, sound);
    }
    fn define_music(&mut self, intro: Option<Arc<Sound>>, body: Arc<Sound>) {
        self.0.define_music(intro.clone(), body.clone());
        self.1.define_music(intro, body);
    }
    fn clear_music(&mut self) {
        self.0.clear_music();
        self.1.clear_music();
    }
    fn submit(&mut self, commands: &[Command]) {
        self.0.submit(commands);
        self.1.submit(commands);
    }
    fn frame(&mut self) {
        self.0.frame();
        self.1.frame();
    }
    fn set_suspended(&mut self, suspended: bool) {
        self.0.set_suspended(suspended);
        self.1.set_suspended(suspended);
    }
    fn report(&self) -> BackendReport {
        self.0.report()
    }
}

const RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = (RATE / 60) as usize;

fn rms(pcm: &[f32]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    (pcm.iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>()
        / pcm.len() as f64)
        .sqrt() as f32
}

/// Stereo samples of game frames `from..to`.
fn window(pcm: &[f32], from: usize, to: usize) -> &[f32] {
    let a = (from * FRAME_SAMPLES * 2).min(pcm.len());
    let b = (to * FRAME_SAMPLES * 2).min(pcm.len());
    &pcm[a..b]
}

#[test]
fn offline_render_is_sane() {
    let _gpu = GPU.lock().unwrap_or_else(|e| e.into_inner());
    let (full_handle, full_driver) = mix_channel(RATE);
    let (fx_handle, fx_driver) = mix_channel(RATE);
    let fx_mute = fx_handle.clone();
    let audio = Audio::new(Box::new(Tee(
        MixBackend::new(full_handle.clone()),
        MixBackend::new(fx_handle),
    )));
    let Some(mut h) = start(&audio) else {
        return;
    };
    // One game frame of audio per game frame, exactly as a device callback would pull it.
    let render = Rc::new(RefCell::new((
        full_driver,
        fx_driver,
        Vec::<f32>::new(),
        Vec::<f32>::new(),
    )));
    let r = render.clone();
    let mut pull = move || {
        // After whatever the game set this frame: the effects render never hears the music.
        fx_mute.send(MixMsg::BusVolume {
            bus: Bus::Music,
            volume: 0.0,
        });
        let mut guard = r.borrow_mut();
        let (full, fx, full_pcm, fx_pcm) = &mut *guard;
        let one = |d: &mut MixDriver, out: &mut Vec<f32>| {
            let at = out.len();
            out.resize(at + FRAME_SAMPLES * 2, 0.0);
            d.process(&mut out[at..]);
        };
        one(full, full_pcm);
        one(fx, fx_pcm);
    };
    let t = play_floor_one(&mut h, &mut pull);
    h.shutdown();
    let guard = render.borrow();
    let (_, _, full, fx) = &*guard;

    // Length: one 800-sample frame per game frame.
    assert_eq!(full.len(), t.end * FRAME_SAMPLES * 2);
    assert_eq!(fx.len(), full.len());
    let stats = full_handle.stats().snapshot();
    assert_eq!(stats.frames as usize, t.end * FRAME_SAMPLES);
    assert_eq!(stats.callbacks as usize, t.end);
    assert_eq!(stats.dropped, 0, "no request was lost to a full queue");

    // Levels: audible, far from clipping, nothing clamped.
    let peak = full.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert_eq!(peak, stats.peak);
    assert!(peak > 0.05 && peak < 1.0, "peak {peak}");
    assert_eq!(stats.clipped, 0);
    assert!(full.iter().all(|s| s.is_finite()));
    let whole = rms(window(full, t.stage_start, t.floor_two));
    assert!(whole > 0.01 && whole < 0.5, "rms {whole}");

    // Music is there through the fly-in, before any effect.
    let quiet_fx = window(fx, t.stage_start + 120, t.ready - 2);
    assert!(quiet_fx.iter().all(|s| *s == 0.0), "no effect before READY");
    let music_intro = rms(window(full, t.stage_start + 120, t.ready - 2));
    assert!(music_intro > 0.005, "music rms {music_intro}");
    // The announcer and the effects, measured without the music.
    let ready = rms(window(fx, t.ready, t.ready + 45));
    let go = rms(window(fx, t.go, t.go + 45));
    let goal = rms(window(fx, t.goal, t.goal + 120));
    let rolling = rms(window(fx, t.go + 60, t.goal));
    assert!(ready > 0.01, "READY rms {ready}");
    assert!(go > 0.01, "GO rms {go}");
    assert!(goal > 0.01, "goal rms {goal}");
    assert!(rolling > 0.002, "roll rms {rolling}");
    // Both channels carry signal; the effects are centred (mono sources).
    let (mut l, mut r_sum, mut diff) = (0f64, 0f64, 0f64);
    for lr in fx.as_chunks::<2>().0 {
        l += f64::from(lr[0].abs());
        r_sum += f64::from(lr[1].abs());
        diff += f64::from((lr[0] - lr[1]).abs());
    }
    assert!(
        l > 0.0 && r_sum > 0.0 && diff < 1e-3 * l,
        "{l} {r_sum} {diff}"
    );
    // After the quit the music has faded out and nothing sounds.
    assert!(window(full, t.end - 5, t.end).iter().all(|s| *s == 0.0));
    assert_eq!(full_handle.stats().snapshot().voices, 0);

    // Scratch output for a human to listen to; never inside the repository.
    let dir = std::env::var_os("MB_AUDIO_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("mb-audio-test"));
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    if let (Ok(d), Ok(repo)) = (
        std::fs::create_dir_all(&dir).and_then(|_| dir.canonicalize()),
        repo.canonicalize(),
    ) {
        assert!(
            !d.starts_with(&repo),
            "audio output must stay out of the repository"
        );
        std::fs::write(d.join("floor1_mix.wav"), wav_bytes_f32(full, 2, RATE)).unwrap();
        std::fs::write(d.join("floor1_effects.wav"), wav_bytes_f32(fx, 2, RATE)).unwrap();
        eprintln!(
            "wrote {} ({:.1} s): peak {peak:.3}, rms {whole:.4}; music-only intro {music_intro:.4}; \
             effects: READY {ready:.4}, GO {go:.4}, roll {rolling:.4}, goal {goal:.4}",
            d.join("floor1_mix.wav").display(),
            t.end as f64 / 60.0
        );
    }
}
