//! Game audio, platform-neutral: what the game states talk to.
//!
//! * [`Sfx`]: the sounds the game uses, each tied to the original game's sound-request id (resolved to a
//!   MusyX bank entry through the executable's own table, see `mb_audio::sound_table`).
//! * [`Audio`]: the handle states hold. They ask for sounds, the ball roll, stage music and volumes; the
//!   requests of a frame are batched and handed to the backend once, in [`Audio::flush`].
//! * [`AudioBackend`]: what actually makes sound. `MixBackend` feeds `mb-audio`'s software mixer (desktop
//!   through cpal in `audio_cpal`; also the offline renderer used by tests), the browser
//!   shell has a Web Audio one (`mb-web`), [`NullBackend`] is silence, and `RecordingBackend` (tests, behind
//!   the `dev` feature) logs requests.
//! * [`StageSounds`]: turns one simulation tick into sound requests, mirroring the call sites of the
//!   reference's `game.ts` / `audio.ts`.
//!
//! Disc access is confined to [`sfx_paths`] / [`load_sfx`] and `StageMusic::paths` / [`load_music`]: they
//! name the files they need and take a fetch function, and nothing is read while a stage is being played.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::Arc;

use mb_audio::adp::decode_adp;
use mb_audio::driver::{MixHandle, MixMsg, MixSnapshot};
pub use mb_audio::mixer::Bus;
use mb_audio::mixer::Sound;
use mb_audio::music::{StageMusic, stage_music};
use mb_audio::musyx::{MusyxBank, SfxLibrary};
use mb_audio::sound_table::{SoundTable, velocity_amplitude};
use mb_engine::constants::COLI_OCCURRED;
use mb_engine::{Sim, SimEvent};
use web_time::Instant;

/// The reference's master gain (`masterGain.gain.value = 0.9`).
pub const MASTER_GAIN: f32 = 0.9;
/// Seconds over which stage music fades when it is stopped.
const MUSIC_FADE: f32 = 0.3;
/// Loop slots of the three ball-roll layers.
const ROLL_SLOTS: [u8; 3] = [0, 1, 2];

/// A sound the game plays. `Count(n)` is the announcer counting `n` (0..=9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sfx {
    Ready,
    Go,
    Goal,
    HurryUp,
    TimeOver,
    /// The announcer's "FALL OUT".
    FallOutCall,
    BonusFinish,
    Perfect,
    Count(u8),
    /// The three layers of the rolling sound.
    Roll(u8),
    HitHard,
    HitMedium,
    HitSoft,
    Bumper,
    Banana,
    BananaBunch,
    /// Goal tape breaking: two sounds at once in the original.
    GoalTape,
    GoalTapeCut,
    /// The ball flying off after the goal (`ball_woosh` in the reference).
    GoalFly,
    FallOut,
    /// The clock tick the original plays every second of play.
    TimerTick,
    MenuMove,
    MenuConfirm,
    MenuCancel,
}

/// Where a sound comes from in the original game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    /// Request id passed to the game's sound function (low 11 bits select, the rest is velocity).
    Request(u32),
    /// Started by table entry, not by request id.
    Named(&'static str),
}

struct Spec {
    source: Source,
    bus: Bus,
    /// MusyX SFX id and velocity on the USA disc, used when the executable's table is not available.
    usa: (u16, u8),
}

impl Sfx {
    pub const COUNT: usize = 35;

    /// Every sound, in table order.
    pub fn all() -> impl Iterator<Item = Sfx> {
        use Sfx::*;
        [
            Ready,
            Go,
            Goal,
            HurryUp,
            TimeOver,
            FallOutCall,
            BonusFinish,
            Perfect,
        ]
        .into_iter()
        .chain((0..10).map(Count))
        .chain((0..3).map(Roll))
        .chain([
            HitHard,
            HitMedium,
            HitSoft,
            Bumper,
            Banana,
            BananaBunch,
            GoalTape,
            GoalTapeCut,
            GoalFly,
            FallOut,
            TimerTick,
            MenuMove,
            MenuConfirm,
            MenuCancel,
        ])
    }

    fn index(self) -> usize {
        use Sfx::*;
        match self {
            Ready => 0,
            Go => 1,
            Goal => 2,
            HurryUp => 3,
            TimeOver => 4,
            FallOutCall => 5,
            BonusFinish => 6,
            Perfect => 7,
            Count(n) => 8 + (n.min(9) as usize),
            Roll(n) => 18 + (n.min(2) as usize),
            HitHard => 21,
            HitMedium => 22,
            HitSoft => 23,
            Bumper => 24,
            Banana => 25,
            BananaBunch => 26,
            GoalTape => 27,
            GoalTapeCut => 28,
            GoalFly => 29,
            FallOut => 30,
            TimerTick => 31,
            MenuMove => 32,
            MenuConfirm => 33,
            MenuCancel => 34,
        }
    }

    /// Request ids are the ones found at the call sites of the USA executable (see `docs/AUDIO.md`).
    fn spec(self) -> Spec {
        use Sfx::*;
        let announcer = |id: u32, musyx: u16| Spec {
            source: Source::Request(id),
            bus: Bus::Announcer,
            usa: (musyx, 110),
        };
        let sfx = |id: u32, musyx: u16, velocity: u8| Spec {
            source: Source::Request(id),
            bus: Bus::Sfx,
            usa: (musyx, velocity),
        };
        match self {
            Ready => announcer(0x4, 170),
            Go => announcer(0x5, 158),
            Goal => announcer(0x8, 160),
            HurryUp => announcer(0x7, 163),
            TimeOver => announcer(0xb, 255),
            FallOutCall => announcer(0xc, 108),
            BonusFinish => announcer(0x45, 52),
            Perfect => announcer(0x48, 166),
            Count(n) => announcer(0x23 + u32::from(n.min(9)), u16::from(n.min(9))),
            Roll(n) => {
                let (name, musyx) = [
                    ("SND_SE01_RUN01", 248),
                    ("SND_SE01_RUN02", 273),
                    ("SND_SE01_RUN03", 274),
                ][n.min(2) as usize];
                Spec {
                    source: Source::Named(name),
                    bus: Bus::Sfx,
                    usa: (musyx, 127),
                }
            }
            HitHard => sfx(0x2f, 176, 100),
            HitMedium => sfx(0x14, 185, 127),
            HitSoft => sfx(0x13, 178, 127),
            Bumper => sfx(0x5011, 165, 110),
            Banana => sfx(0x3, 206, 110),
            BananaBunch => sfx(0x39, 207, 110),
            GoalTape => sfx(0x16, 211, 100),
            GoalTapeCut => sfx(0x127, 734, 100),
            GoalFly => sfx(0x126, 733, 100),
            FallOut => sfx(0x15, 105, 100),
            TimerTick => sfx(0x6, 164, 100),
            MenuMove => sfx(0x6c, 214, 100),
            MenuConfirm => sfx(0x6a, 244, 100),
            MenuCancel => sfx(0x6b, 175, 100),
        }
    }

    pub fn bus(self) -> Bus {
        self.spec().bus
    }
}

/// A request to the backend. Sounds are named by MusyX SFX id (what [`AudioBackend::define_sound`] got).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Play {
        sound: u16,
        bus: Bus,
        /// Linear gain.
        volume: f32,
        /// -1 (left) ..= 1 (right).
        pan: f32,
        /// Playback-rate multiplier.
        rate: f32,
        /// Seconds from now.
        delay: f32,
    },
    /// Start a looping sound in a slot (restarting whatever was there).
    LoopStart {
        slot: u8,
        sound: u16,
        bus: Bus,
        volume: f32,
        rate: f32,
    },
    /// Live update of a running loop.
    LoopSet {
        slot: u8,
        volume: f32,
        rate: f32,
    },
    LoopStop {
        slot: u8,
    },
    /// Start the music given to [`AudioBackend::define_music`]: intro once, then the loop, gapless.
    MusicPlay {
        volume: f32,
    },
    MusicStop {
        fade: f32,
    },
    BusVolume {
        bus: Bus,
        volume: f32,
    },
    /// Cut every sound effect, loop and announcer line (stage exit); music carries on.
    StopSounds,
}

/// What a backend can say about itself for logs and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackendReport {
    pub backend: &'static str,
    /// Output device and format, or why there is none.
    pub device: String,
    /// True while an output stream is open and running.
    pub running: bool,
    pub sample_rate: u32,
    pub channels: u16,
    /// What was rendered (software-mixed backends only).
    pub mix: Option<MixSnapshot>,
    /// Callbacks that found the mixer busy and wrote silence.
    pub lock_misses: u64,
    /// Callbacks that came later than three buffer lengths after the previous one: the device ran dry.
    pub late_callbacks: u64,
    /// Errors reported by the output stream (device lost, underrun where the system reports it).
    pub stream_errors: u64,
    /// Times the output stream was (re)opened.
    pub opens: u32,
}

impl BackendReport {
    pub fn summary(&self) -> String {
        let mut s = format!(
            "audio: {} | {} | running {} | {} Hz x{} | opens {} | stream errors {} | late callbacks {} | lock misses {}",
            self.backend,
            self.device,
            self.running,
            self.sample_rate,
            self.channels,
            self.opens,
            self.stream_errors,
            self.late_callbacks,
            self.lock_misses
        );
        if let Some(m) = &self.mix {
            s.push_str(&format!(
                "\naudio mix: {} callbacks, {} frames ({:.2} s), peak {:.4}, rms {:.4}, clipped {}, messages {} (dropped {}), voices max {}",
                m.callbacks,
                m.frames,
                m.frames as f64 / f64::from(self.sample_rate.max(1)),
                m.peak,
                m.rms,
                m.clipped,
                m.messages,
                m.dropped,
                m.max_voices
            ));
        }
        s
    }
}

pub trait AudioBackend {
    /// Register a decoded sound effect under its MusyX id. Called at load time only.
    fn define_sound(&mut self, id: u16, sound: Arc<Sound>);
    /// Hand over the current music (replacing and releasing the previous track). Load time only.
    fn define_music(&mut self, intro: Option<Arc<Sound>>, body: Arc<Sound>);
    /// Offer the current music as the disc's own streams (DTK ADPCM, see `mb_audio::adp`) before it is
    /// decoded. A backend that keeps music in its own buffers (Web Audio) decodes it piece by piece into
    /// them and returns `Ok(true)`, so the track never exists as 16-bit PCM in the engine's memory as well
    /// (25 MB for the longest one, in a wasm memory that cannot shrink again). The default declines and is
    /// given [`AudioBackend::define_music`] instead.
    fn define_music_streams(
        &mut self,
        _intro: Option<&[u8]>,
        _body: &[u8],
    ) -> Result<bool, String> {
        Ok(false)
    }
    /// Release the music buffers.
    fn clear_music(&mut self);
    /// The requests of one frame, in order.
    fn submit(&mut self, commands: &[Command]);
    /// Called once per frame even when there is nothing to submit (device upkeep).
    fn frame(&mut self) {}
    /// The app went to the background / came back: silence and stop work, or resume.
    fn set_suspended(&mut self, suspended: bool);
    fn report(&self) -> BackendReport;
}

/// No output at all (headless runs, tests that do not care).
#[derive(Default)]
pub struct NullBackend;

impl AudioBackend for NullBackend {
    fn define_sound(&mut self, _id: u16, _sound: Arc<Sound>) {}
    fn define_music(&mut self, _intro: Option<Arc<Sound>>, _body: Arc<Sound>) {}
    fn clear_music(&mut self) {}
    fn submit(&mut self, _commands: &[Command]) {}
    fn set_suspended(&mut self, _suspended: bool) {}
    fn report(&self) -> BackendReport {
        BackendReport {
            backend: "none",
            device: "no audio output".into(),
            ..BackendReport::default()
        }
    }
}

/// One thing a [`RecordingBackend`] saw.
#[cfg(any(test, feature = "dev"))]
#[derive(Clone, Debug, PartialEq)]
pub enum Recorded {
    DefineSound(u16),
    DefineMusic {
        intro_frames: Option<usize>,
        body_frames: usize,
    },
    ClearMusic,
    Command(Command),
    Suspended(bool),
}

/// Logs everything it is asked to do; for tests.
#[cfg(any(test, feature = "dev"))]
pub struct RecordingBackend {
    log: Rc<RefCell<Vec<Recorded>>>,
}

#[cfg(any(test, feature = "dev"))]
impl RecordingBackend {
    pub fn new() -> (Self, Rc<RefCell<Vec<Recorded>>>) {
        let log = Rc::new(RefCell::new(Vec::new()));
        (Self { log: log.clone() }, log)
    }
}

#[cfg(any(test, feature = "dev"))]
impl AudioBackend for RecordingBackend {
    fn define_sound(&mut self, id: u16, _sound: Arc<Sound>) {
        self.log.borrow_mut().push(Recorded::DefineSound(id));
    }
    fn define_music(&mut self, intro: Option<Arc<Sound>>, body: Arc<Sound>) {
        self.log.borrow_mut().push(Recorded::DefineMusic {
            intro_frames: intro.map(|s| s.frames()),
            body_frames: body.frames(),
        });
    }
    fn clear_music(&mut self) {
        self.log.borrow_mut().push(Recorded::ClearMusic);
    }
    fn submit(&mut self, commands: &[Command]) {
        self.log
            .borrow_mut()
            .extend(commands.iter().copied().map(Recorded::Command));
    }
    fn set_suspended(&mut self, suspended: bool) {
        self.log.borrow_mut().push(Recorded::Suspended(suspended));
    }
    fn report(&self) -> BackendReport {
        BackendReport {
            backend: "recording",
            device: "request log".into(),
            ..BackendReport::default()
        }
    }
}

/// Game-thread half of the software-mixed backends: keeps the decoded sounds and turns [`Command`]s into
/// queue messages for a `mb_audio::driver::MixDriver` running wherever the audio is pulled from.
pub struct MixBackend {
    handle: MixHandle,
    sounds: Vec<Option<Arc<Sound>>>,
    music: Option<(Option<Arc<Sound>>, Arc<Sound>)>,
    /// Buffers no longer wanted that the mixer may still be reading; freed here, on the game thread,
    /// once it has let go of them.
    retired: Vec<Arc<Sound>>,
}

impl MixBackend {
    pub fn new(handle: MixHandle) -> Self {
        handle.send(MixMsg::MasterVolume(MASTER_GAIN));
        Self {
            handle,
            sounds: Vec::new(),
            music: None,
            retired: Vec::new(),
        }
    }

    pub fn handle(&self) -> &MixHandle {
        &self.handle
    }

    fn sound(&self, id: u16) -> Option<Arc<Sound>> {
        self.sounds.get(id as usize).cloned().flatten()
    }

    fn retire_music(&mut self) {
        if let Some((intro, body)) = self.music.take() {
            self.retired.extend(intro);
            self.retired.push(body);
        }
        self.collect();
    }

    /// Free retired buffers the mixer no longer references.
    fn collect(&mut self) {
        self.retired.retain(|s| Arc::strong_count(s) > 1);
    }

    pub fn snapshot(&self) -> MixSnapshot {
        self.handle.stats().snapshot()
    }
}

impl AudioBackend for MixBackend {
    fn define_sound(&mut self, id: u16, sound: Arc<Sound>) {
        let i = id as usize;
        if self.sounds.len() <= i {
            self.sounds.resize(i + 1, None);
        }
        self.sounds[i] = Some(sound);
    }

    fn define_music(&mut self, intro: Option<Arc<Sound>>, body: Arc<Sound>) {
        self.handle.send(MixMsg::MusicStop { fade_seconds: 0.0 });
        self.retire_music();
        self.music = Some((intro, body));
    }

    fn clear_music(&mut self) {
        self.retire_music();
    }

    fn submit(&mut self, commands: &[Command]) {
        for c in commands {
            let msg = match *c {
                Command::Play {
                    sound,
                    bus,
                    volume,
                    pan,
                    rate,
                    delay,
                } => match self.sound(sound) {
                    Some(sound) => MixMsg::Play {
                        sound,
                        bus,
                        volume,
                        pan,
                        rate,
                        delay_seconds: delay,
                    },
                    None => continue,
                },
                Command::LoopStart {
                    slot,
                    sound,
                    bus,
                    volume,
                    rate,
                } => match self.sound(sound) {
                    Some(sound) => MixMsg::LoopStart {
                        slot,
                        sound,
                        bus,
                        volume,
                        rate,
                    },
                    None => continue,
                },
                Command::LoopSet { slot, volume, rate } => MixMsg::LoopSet { slot, volume, rate },
                Command::LoopStop { slot } => MixMsg::LoopStop { slot },
                Command::MusicPlay { volume } => match &self.music {
                    Some((intro, body)) => MixMsg::Music {
                        intro: intro.clone(),
                        body: body.clone(),
                        volume,
                    },
                    None => continue,
                },
                Command::MusicStop { fade } => MixMsg::MusicStop { fade_seconds: fade },
                Command::BusVolume { bus, volume } => MixMsg::BusVolume { bus, volume },
                Command::StopSounds => {
                    self.handle.send(MixMsg::StopBus(Bus::Sfx));
                    MixMsg::StopBus(Bus::Announcer)
                }
            };
            self.handle.send(msg);
        }
    }

    fn frame(&mut self) {
        if !self.retired.is_empty() {
            self.collect();
        }
    }

    fn set_suspended(&mut self, suspended: bool) {
        self.handle.send(MixMsg::Pause(suspended));
    }

    fn report(&self) -> BackendReport {
        BackendReport {
            backend: "software mixer",
            device: "offline".into(),
            mix: Some(self.snapshot()),
            ..BackendReport::default()
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Loading (the only code that reads the disc for audio).

/// The MusyX banks the main game's sounds live in.
pub const SFX_BANKS: [&str; 2] = ["allse", "comn"];

/// Disc paths [`load_sfx`] fetches.
pub fn sfx_paths() -> Vec<String> {
    SFX_BANKS
        .iter()
        .flat_map(|bank| {
            ["proj", "pool", "sdir", "samp"]
                .iter()
                .map(move |ext| format!("test/snd/mkb/{bank}.{ext}"))
        })
        .collect()
}

/// One decoded sound effect.
pub struct LoadedSfx {
    pub sfx: Sfx,
    pub musyx_id: u16,
    /// Linear gain of the velocity the game plays it at.
    pub gain: f32,
    pub sound: Arc<Sound>,
}

/// What [`load_sfx`] produced.
#[derive(Default)]
pub struct SfxSet {
    pub sounds: Vec<LoadedSfx>,
    /// Sounds that could not be resolved to a sample in the banks.
    pub missing: Vec<Sfx>,
    /// Whether ids came from the executable's table (false: the built-in USA ids were used).
    pub from_table: bool,
}

impl SfxSet {
    /// Bytes of decoded 16-bit PCM, counting a sample shared by several sounds once.
    pub fn pcm_bytes(&self) -> usize {
        let mut seen: Vec<u16> = Vec::new();
        let mut total = 0;
        for s in &self.sounds {
            if !seen.contains(&s.musyx_id) {
                seen.push(s.musyx_id);
                total += s.sound.byte_len();
            }
        }
        total
    }
}

/// Read the banks named by [`sfx_paths`] through `fetch` and decode exactly the sounds in [`Sfx`].
/// `main_dol` is the game executable when the content has one: its sound table maps request ids to bank
/// entries; without it the ids of the USA disc are used.
pub fn load_sfx(
    fetch: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
    main_dol: Option<&[u8]>,
) -> Result<SfxSet, String> {
    let mut banks = Vec::new();
    for bank in SFX_BANKS {
        let mut get = |ext: &str| {
            let path = format!("test/snd/mkb/{bank}.{ext}");
            fetch(&path).map_err(|e| format!("{path}: {e}"))
        };
        let (proj, pool, sdir, samp) = (get("proj")?, get("pool")?, get("sdir")?, get("samp")?);
        banks
            .push(MusyxBank::parse(&proj, &pool, &sdir, samp).map_err(|e| format!("{bank}: {e}"))?);
    }
    let lib = SfxLibrary::new(banks);
    let table = main_dol.and_then(SoundTable::from_dol);
    let mut set = SfxSet {
        from_table: table.is_some(),
        ..SfxSet::default()
    };
    let mut decoded: Vec<(u16, Arc<Sound>)> = Vec::new();
    for sfx in Sfx::all() {
        let spec = sfx.spec();
        let resolved = table.as_ref().and_then(|t| match spec.source {
            Source::Request(id) => t.resolve(id, 0).map(|r| (r.musyx_id, r.velocity)),
            Source::Named(name) => t.by_name(name).map(|id| (id, spec.usa.1)),
        });
        let (musyx_id, velocity) = resolved.unwrap_or(spec.usa);
        let sound = match decoded.iter().find(|(id, _)| *id == musyx_id) {
            Some((_, s)) => Some(s.clone()),
            None => match lib.render(musyx_id) {
                Ok(Some(r)) => {
                    let s = Arc::new(r.to_sound());
                    decoded.push((musyx_id, s.clone()));
                    Some(s)
                }
                Ok(None) => None,
                Err(e) => return Err(format!("sound {musyx_id}: {e}")),
            },
        };
        match sound {
            Some(sound) => set.sounds.push(LoadedSfx {
                sfx,
                musyx_id,
                gain: velocity_amplitude(velocity),
                sound,
            }),
            None => set.missing.push(sfx),
        }
    }
    Ok(set)
}

/// A music stream as it is on the disc: its path and bytes.
type Stream = (String, Vec<u8>);

/// Read one stage's music streams (`music.paths()`) as they are on the disc: the optional intro and the
/// loop, each with its path.
fn read_music(
    fetch: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
    music: &StageMusic,
) -> Result<(Option<Stream>, Stream), String> {
    let mut streams = Vec::new();
    for path in music.paths() {
        let bytes = fetch(&path).map_err(|e| format!("{path}: {e}"))?;
        streams.push((path, bytes));
    }
    let body = streams.pop().ok_or("music has no stream")?;
    Ok((streams.pop(), body))
}

fn decode_stream((path, bytes): &Stream) -> Result<Sound, String> {
    decode_adp(bytes)
        .map(Sound::from_adp_owned)
        .map_err(|e| format!("{path}: {e}"))
}

/// Read and decode one stage's music (`music.paths()`): the optional intro and the loop, at the streams'
/// native 48 kHz.
pub fn load_music(
    fetch: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
    music: &StageMusic,
) -> Result<(Option<Sound>, Sound), String> {
    let (intro, body) = read_music(fetch, music)?;
    let intro = intro.as_ref().map(decode_stream).transpose()?;
    Ok((intro, decode_stream(&body)?))
}

/// Read one stage's music and hand it to `backend`: as streams when the backend takes those, decoded
/// otherwise. Returns the bytes of 16-bit PCM the engine now holds for it (none in the first case).
fn define_music(
    backend: &mut dyn AudioBackend,
    fetch: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
    music: &StageMusic,
) -> Result<usize, String> {
    let (intro, body) = read_music(fetch, music)?;
    if backend.define_music_streams(intro.as_ref().map(|(_, b)| b.as_slice()), &body.1)? {
        return Ok(0);
    }
    let intro_sound = intro.as_ref().map(decode_stream).transpose()?;
    // The stream is not needed once it is decoded: never hold both for longer than one of them takes.
    drop(intro);
    let body = decode_stream(&body)?;
    let bytes = body.byte_len() + intro_sound.as_ref().map_or(0, Sound::byte_len);
    backend.define_music(intro_sound.map(Arc::new), Arc::new(body));
    Ok(bytes)
}

// ---------------------------------------------------------------------------------------------------------
// The handle.

/// Load timings and sizes, for logs and `docs/AUDIO.md`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadReport {
    pub sfx_loaded: bool,
    pub sfx_ms: f64,
    pub sfx_count: usize,
    pub sfx_pcm_bytes: usize,
    pub sfx_from_table: bool,
    pub sfx_missing: Vec<Sfx>,
    /// Last music load.
    pub music_key: Option<&'static str>,
    pub music_ms: f64,
    /// 16-bit PCM the engine holds for the track; zero when the backend took the streams themselves.
    pub music_pcm_bytes: usize,
    pub music_loads: u32,
    /// Messages for the log (load failures).
    pub errors: Vec<String>,
}

struct Inner {
    backend: Box<dyn AudioBackend>,
    queue: Vec<Command>,
    /// MusyX id and gain per [`Sfx::index`]; `None` until loaded or when missing from the banks.
    table: [Option<(u16, f32)>; Sfx::COUNT],
    sfx_tried: bool,
    /// Category volumes 0..=1 by [`Bus`] index.
    volumes: [f32; 3],
    /// Track handed to the backend.
    music: Option<StageMusic>,
    music_playing: bool,
    roll_on: bool,
    suspended: bool,
    report: LoadReport,
}

/// Cheap to clone; every clone is the same audio system. Single-threaded (the game thread).
#[derive(Clone)]
pub struct Audio {
    inner: Rc<RefCell<Inner>>,
}

impl std::fmt::Debug for Audio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Audio")
    }
}

impl Default for Audio {
    fn default() -> Self {
        Self::silent()
    }
}

/// `smoothstep`: the game's volume curves for the roll layers are two-key Hermite splines with zero tangents.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Gains of the three roll layers and their shared playback rate for the game's roll volume (0..=127) and
/// pitch (0..=68) values.
///
/// The original (`0x8002ca5c`) keeps three looping sounds and sets, per layer, MIDI volume
/// `vol / 127 * curve(vol)` with curves 0 -> 127, constant 127, 0 -> 127, and a 14-bit pitch bend from the
/// curve 4608 -> 16383 over the pitch value; no macro changes the bend range, so MusyX's default of two
/// semitones applies.
pub fn roll_layers(vol: f32, pitch: f32) -> ([f32; 3], f32) {
    let v = (vol / 127.0).clamp(0.0, 1.0);
    let swell = v * smooth(v);
    let gains = [swell * swell, v * v, swell * swell];
    let bend = 4608.0 + (16383.0 - 4608.0) * smooth(pitch / 127.0);
    let semitones = (bend - 8192.0) / 8192.0 * 2.0;
    (gains, 2f32.powf(semitones / 12.0))
}

impl Audio {
    pub fn new(backend: Box<dyn AudioBackend>) -> Self {
        // The reference's default slider positions; the backend hears about them with the first flush.
        let volumes = [0.5, 0.3, 0.3];
        let mut queue = Vec::with_capacity(64);
        for bus in [Bus::Music, Bus::Sfx, Bus::Announcer] {
            queue.push(Command::BusVolume {
                bus,
                volume: volumes[bus as usize],
            });
        }
        Self {
            inner: Rc::new(RefCell::new(Inner {
                backend,
                queue,
                table: [None; Sfx::COUNT],
                sfx_tried: false,
                volumes,
                music: None,
                music_playing: false,
                roll_on: false,
                suspended: false,
                report: LoadReport::default(),
            })),
        }
    }

    /// An audio system that makes no sound and loads nothing.
    pub fn silent() -> Self {
        let a = Self::new(Box::new(NullBackend));
        a.inner.borrow_mut().sfx_tried = true;
        a
    }

    /// An audio system that logs its requests (tests).
    #[cfg(any(test, feature = "dev"))]
    pub fn recording() -> (Self, Rc<RefCell<Vec<Recorded>>>) {
        let (backend, log) = RecordingBackend::new();
        (Self::new(Box::new(backend)), log)
    }

    /// Load the sound effects once (menu load). A failure is recorded and leaves the game silent.
    pub fn ensure_sfx(
        &self,
        fetch: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>,
        main_dol: Option<&[u8]>,
    ) {
        let mut inner = self.inner.borrow_mut();
        if inner.sfx_tried {
            return;
        }
        inner.sfx_tried = true;
        let start = Instant::now();
        match load_sfx(fetch, main_dol) {
            Ok(set) => {
                let inner = &mut *inner;
                inner.report.sfx_pcm_bytes = set.pcm_bytes();
                inner.report.sfx_count = set.sounds.len();
                inner.report.sfx_from_table = set.from_table;
                inner.report.sfx_missing = set.missing;
                for s in set.sounds {
                    inner.table[s.sfx.index()] = Some((s.musyx_id, s.gain));
                    inner.backend.define_sound(s.musyx_id, s.sound);
                }
                inner.report.sfx_loaded = true;
            }
            Err(e) => inner.report.errors.push(format!("sound effects: {e}")),
        }
        inner.report.sfx_ms = start.elapsed().as_secs_f64() * 1000.0;
    }

    /// Apply the saved category volumes (0..=100 each). A category at zero stops being played at all.
    pub fn set_volumes(&self, music: u8, sfx: u8, announcer: u8) {
        let mut guard = self.inner.borrow_mut();
        let inner = &mut *guard;
        let new = [
            (Bus::Music, music),
            (Bus::Sfx, sfx),
            (Bus::Announcer, announcer),
        ];
        for (bus, percent) in new {
            let v = f32::from(percent.min(100)) / 100.0;
            let slot = &mut inner.volumes[bus as usize];
            if *slot == v {
                continue;
            }
            *slot = v;
            inner.queue.push(Command::BusVolume { bus, volume: v });
            if v > 0.0 {
                continue;
            }
            match bus {
                Bus::Music => {
                    if inner.music_playing {
                        inner.queue.push(Command::MusicStop { fade: 0.0 });
                        inner.music_playing = false;
                    }
                    if inner.music.take().is_some() {
                        inner.backend.clear_music();
                    }
                }
                Bus::Sfx => {
                    if inner.roll_on {
                        inner.roll_on = false;
                        for slot in ROLL_SLOTS {
                            inner.queue.push(Command::LoopStop { slot });
                        }
                    }
                }
                Bus::Announcer => {}
            }
        }
    }

    /// Play a sound at the velocity the game uses.
    pub fn play(&self, sfx: Sfx) {
        self.play_with(sfx, 1.0, 0.0);
    }

    /// Play a sound with its gain scaled by `scale`, `delay` seconds from now.
    pub fn play_with(&self, sfx: Sfx, scale: f32, delay: f32) {
        let mut inner = self.inner.borrow_mut();
        let Some((sound, gain)) = inner.table[sfx.index()] else {
            return;
        };
        let bus = sfx.bus();
        if inner.volumes[bus as usize] <= 0.0 {
            return;
        }
        inner.queue.push(Command::Play {
            sound,
            bus,
            volume: gain * scale,
            pan: 0.0,
            rate: 1.0,
            delay,
        });
    }

    /// Set the ball-roll loop from the game's roll volume (0..=127, 0 = stop) and pitch values.
    pub fn roll(&self, vol: f32, pitch: f32) {
        let mut guard = self.inner.borrow_mut();
        let inner = &mut *guard;
        let audible = vol > 0.0 && inner.volumes[Bus::Sfx as usize] > 0.0;
        if !audible {
            if inner.roll_on {
                inner.roll_on = false;
                for slot in ROLL_SLOTS {
                    inner.queue.push(Command::LoopStop { slot });
                }
            }
            return;
        }
        let (gains, rate) = roll_layers(vol, pitch);
        let start = !inner.roll_on;
        for (layer, slot) in ROLL_SLOTS.into_iter().enumerate() {
            let Some((sound, _)) = inner.table[Sfx::Roll(layer as u8).index()] else {
                continue;
            };
            inner.roll_on = true;
            inner.queue.push(if start {
                Command::LoopStart {
                    slot,
                    sound,
                    bus: Bus::Sfx,
                    volume: gains[layer],
                    rate,
                }
            } else {
                Command::LoopSet {
                    slot,
                    volume: gains[layer],
                    rate,
                }
            });
        }
    }

    /// Cut every sound effect, loop and announcer line (leaving a stage). Music is not touched.
    pub fn stop_sounds(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.roll_on = false;
        // Anything queued this frame and not yet sent would otherwise start after the cut.
        inner
            .queue
            .retain(|c| !matches!(c, Command::Play { .. } | Command::LoopStart { .. }));
        inner.queue.push(Command::StopSounds);
    }

    /// Whether [`Audio::stage_music`] would read the streams of background `bg` (a track other than the
    /// loaded one, with the music volume up): the stage loader fetches them only then.
    pub fn needs_music_files(&self, bg: &str) -> bool {
        let inner = self.inner.borrow();
        stage_music(bg)
            .filter(|_| inner.volumes[Bus::Music as usize] > 0.0)
            .is_some_and(|track| !inner.music.is_some_and(|m| m.key == track.key))
    }

    /// Stage load: make sure the music for background `bg` is playing. The same track as before carries
    /// on untouched; a different one is read through `fetch`, decoded and started (intro, then loop).
    /// With the music volume at zero nothing is read or decoded.
    pub fn stage_music(&self, bg: &str, fetch: &mut dyn FnMut(&str) -> io::Result<Vec<u8>>) {
        let mut guard = self.inner.borrow_mut();
        let inner = &mut *guard;
        let wanted = stage_music(bg).filter(|_| inner.volumes[Bus::Music as usize] > 0.0);
        let Some(track) = wanted else {
            drop(guard);
            self.stop_music();
            return;
        };
        if inner.music.is_some_and(|m| m.key == track.key) && inner.music_playing {
            return;
        }
        if !inner.music.is_some_and(|m| m.key == track.key) {
            let start = Instant::now();
            // Whatever was queued for the old track must not act on the new one.
            inner
                .queue
                .retain(|c| !matches!(c, Command::MusicPlay { .. } | Command::MusicStop { .. }));
            match define_music(inner.backend.as_mut(), fetch, &track) {
                Ok(pcm_bytes) => {
                    inner.report.music_pcm_bytes = pcm_bytes;
                    inner.music = Some(track);
                    inner.report.music_key = Some(track.key);
                    inner.report.music_loads += 1;
                }
                Err(e) => {
                    inner.report.errors.push(format!("music: {e}"));
                    drop(guard);
                    self.stop_music();
                    return;
                }
            }
            inner.report.music_ms = start.elapsed().as_secs_f64() * 1000.0;
        }
        inner.queue.push(Command::MusicPlay {
            volume: track.gain(),
        });
        inner.music_playing = true;
    }

    /// Fade the music out and release it (back in the menus).
    pub fn stop_music(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.music_playing {
            inner.music_playing = false;
            // A start that was never sent is simply withdrawn.
            inner
                .queue
                .retain(|c| !matches!(c, Command::MusicPlay { .. }));
            inner.queue.push(Command::MusicStop { fade: MUSIC_FADE });
        }
        if inner.music.take().is_some() {
            // The backend keeps a fading track alive until it has ended.
            inner.backend.clear_music();
        }
    }

    /// Key of the track that is playing (`st1`, ...).
    pub fn music_key(&self) -> Option<&'static str> {
        let inner = self.inner.borrow();
        inner.music.filter(|_| inner.music_playing).map(|m| m.key)
    }

    /// Hand this frame's requests to the backend. Call once per frame.
    pub fn flush(&self) {
        let mut guard = self.inner.borrow_mut();
        let inner = &mut *guard;
        if !inner.queue.is_empty() {
            inner.backend.submit(&inner.queue);
            inner.queue.clear();
        }
        inner.backend.frame();
    }

    /// The app went to the background (window minimized, tab hidden) or came back.
    pub fn set_suspended(&self, suspended: bool) {
        let mut inner = self.inner.borrow_mut();
        if inner.suspended != suspended {
            inner.suspended = suspended;
            inner.backend.set_suspended(suspended);
        }
    }

    pub fn load_report(&self) -> LoadReport {
        self.inner.borrow().report.clone()
    }

    pub fn backend_report(&self) -> BackendReport {
        self.inner.borrow().backend.report()
    }

    /// Load and device summary for the log.
    pub fn summary(&self) -> String {
        let l = self.load_report();
        let mut s = self.backend_report().summary();
        s.push_str(&format!(
            "\naudio load: {} sound effects ({:.1} MB PCM, ids from {}) in {:.0} ms; music {} ({:.1} MB PCM) in {:.0} ms, {} track loads",
            l.sfx_count,
            l.sfx_pcm_bytes as f64 / 1e6,
            if l.sfx_from_table { "main.dol" } else { "built-in USA table" },
            l.sfx_ms,
            l.music_key.unwrap_or("none"),
            l.music_pcm_bytes as f64 / 1e6,
            l.music_ms,
            l.music_loads
        ));
        if !l.sfx_missing.is_empty() {
            s.push_str(&format!("\naudio: not in the banks: {:?}", l.sfx_missing));
        }
        for e in &l.errors {
            s.push_str(&format!("\naudio error: {e}"));
        }
        s
    }
}

// ---------------------------------------------------------------------------------------------------------
// Simulation -> sounds.

/// `HURRY_UP_FRAMES`
const HURRY_UP_FRAMES: i32 = 11 * 60;
/// `COUNTDOWN_START_FRAMES`
const COUNTDOWN_START_FRAMES: i32 = 10 * 60;
/// Frames between two impact sounds (`frameCount - lastImpactFrame < 6`).
const IMPACT_GAP_FRAMES: i32 = 6;
/// The announcer's "GOAL" follows the tape by half a second (`playAnnouncerGoal(0.5)`; 30 frames in the
/// original's goal state).
const GOAL_CALL_DELAY: f32 = 0.5;
/// Velocity of the clock tick outside the last ten seconds, relative to its full one (request `0x3d806`
/// is 95 against 100: `(95 / 100)^2`).
const TICK_QUIET: f32 = 0.9025;

/// Sound triggers of one stage attempt: a port of the audio call sites in the reference's `game.ts`
/// (`updateRollingSound`, `playImpactForBall`, `consumeBallEvents` and the announcer calls), fed one
/// simulation tick at a time.
#[derive(Debug, Clone)]
pub struct StageSounds {
    ready_played: bool,
    go_played: bool,
    hurry_played: bool,
    last_impact_frame: i32,
    rolling_vol: f32,
    prev_intro: i32,
    prev_stage_timer: i32,
}

impl StageSounds {
    /// Start of an attempt. On a retry the announcer says "READY" at once (`!isFirstAttempt`).
    pub fn new(sim: &Sim, retry: bool, audio: &Audio) -> Self {
        if retry {
            audio.play(Sfx::Ready);
        }
        Self {
            ready_played: retry,
            go_played: false,
            hurry_played: false,
            last_impact_frame: -9999,
            rolling_vol: 0.0,
            prev_intro: sim.intro_timer,
            prev_stage_timer: sim.stage_timer_frames,
        }
    }

    /// Call after every `Sim::step`.
    pub fn tick(&mut self, sim: &Sim, audio: &Audio) {
        let intro = sim.intro_timer;
        if !self.ready_played && self.prev_intro > 120 && intro <= 120 {
            self.ready_played = true;
            audio.play(Sfx::Ready);
        }
        if !self.go_played && self.prev_intro > 0 && intro == 0 {
            self.go_played = true;
            audio.play(Sfx::Go);
        }
        self.prev_intro = intro;

        for event in &sim.events {
            match *event {
                SimEvent::BananaCollected { bunch } => {
                    audio.play(if bunch { Sfx::BananaBunch } else { Sfx::Banana })
                }
                SimEvent::BumperHit => audio.play(Sfx::Bumper),
                SimEvent::GoalEntered(_) => {
                    audio.play(Sfx::GoalTape);
                    audio.play(Sfx::GoalTapeCut);
                    audio.play_with(Sfx::Goal, 1.0, GOAL_CALL_DELAY);
                }
                SimEvent::GoalLaunch => audio.play(Sfx::GoalFly),
                SimEvent::FallOut => {
                    audio.play(Sfx::FallOut);
                    audio.play(if sim.cfg.bonus_stage {
                        Sfx::BonusFinish
                    } else {
                        Sfx::FallOutCall
                    });
                }
                SimEvent::TimeOver => audio.play(Sfx::TimeOver),
                SimEvent::BonusClear => audio.play(Sfx::Perfect),
            }
        }

        let frame = sim.runtime.timer_frames;
        let on_ground = sim.ball.audio.last_coli_flags & COLI_OCCURRED != 0;
        self.update_roll(sim, audio, frame, on_ground);
        self.impact(sim, audio, frame, on_ground);
        self.timer(sim, audio);
    }

    /// `updateRollingSound`, with the original's constants (volume up to 127; the reference caps at 100).
    fn update_roll(&mut self, sim: &Sim, audio: &Audio, frame: i32, on_ground: bool) {
        if frame & 7 != 0 && self.rolling_vol != 0.0 {
            return;
        }
        let speed = sim.ball.speed as f32 * 216.0;
        let (vol, pitch) = if on_ground && speed > 10.0 {
            (
                ((speed - 10.0) * 1.5).min(127.0),
                ((speed - 10.0) * 15.0).min(80.0) * 0.85,
            )
        } else {
            (0.0, 0.0)
        };
        self.rolling_vol = vol;
        audio.roll(vol, pitch);
    }

    /// `playImpactForBall`
    fn impact(&mut self, sim: &Sim, audio: &Audio, frame: i32, on_ground: bool) {
        if !on_ground {
            return;
        }
        let impact = (-sim.ball.audio.last_coli_speed).max(0.0);
        if impact <= 0.05 || frame - self.last_impact_frame < IMPACT_GAP_FRAMES {
            return;
        }
        self.last_impact_frame = frame;
        if impact > 0.186_999_998_986_721_04 {
            audio.play(Sfx::HitHard);
        } else if impact > 0.142_999_999_225_139_62 {
            audio.play(Sfx::HitMedium);
        } else if impact > 0.098_999_999_463_558_2 {
            audio.play(Sfx::HitSoft);
        }
    }

    /// Hurry-up call and countdown (reference), clock ticks (original game, `SMD_GAME_PLAY_MAIN`).
    fn timer(&mut self, sim: &Sim, audio: &Audio) {
        let ran = sim.stage_timer_frames != self.prev_stage_timer;
        self.prev_stage_timer = sim.stage_timer_frames;
        let limit = sim.cfg.time_limit_frames;
        if !ran || limit <= 0 {
            return;
        }
        let left = limit - sim.stage_timer_frames;
        if left == HURRY_UP_FRAMES && !self.hurry_played {
            self.hurry_played = true;
            audio.play(Sfx::HurryUp);
        }
        let last_ten = left <= COUNTDOWN_START_FRAMES;
        if left >= 60 && left % 60 == 0 {
            if last_ten {
                audio.play(Sfx::Count((left / 60 - 1) as u8));
                audio.play(Sfx::TimerTick);
            } else {
                audio.play_with(Sfx::TimerTick, TICK_QUIET, 0.0);
            }
        }
        // The ticking doubles in the last ten seconds and triples in the last five.
        if left > 0 && ((last_ten && left % 60 == 52) || (left <= 300 && left % 60 == 44)) {
            audio.play(Sfx::TimerTick);
        }
    }

    /// The simulation is not running (pause menu): no roll loop meanwhile.
    pub fn hold(&mut self, audio: &Audio) {
        if self.rolling_vol != 0.0 {
            self.rolling_vol = 0.0;
            audio.roll(0.0, 0.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(log: &Rc<RefCell<Vec<Recorded>>>) -> Vec<Command> {
        log.borrow()
            .iter()
            .filter_map(|r| match r {
                Recorded::Command(c) => Some(*c),
                _ => None,
            })
            .collect()
    }

    /// An audio system whose sounds are "loaded" with the USA ids, without a disc.
    fn loaded() -> (Audio, Rc<RefCell<Vec<Recorded>>>) {
        let (audio, log) = Audio::recording();
        {
            let mut inner = audio.inner.borrow_mut();
            inner.sfx_tried = true;
            for sfx in Sfx::all() {
                let (id, velocity) = sfx.spec().usa;
                inner.table[sfx.index()] = Some((id, velocity_amplitude(velocity)));
            }
        }
        // The initial category volumes are not what these tests look at.
        audio.flush();
        log.borrow_mut().clear();
        (audio, log)
    }

    #[test]
    fn sfx_table_is_dense_and_consistent() {
        let all: Vec<Sfx> = Sfx::all().collect();
        assert_eq!(all.len(), Sfx::COUNT);
        for (i, s) in all.iter().enumerate() {
            assert_eq!(s.index(), i, "{s:?}");
        }
        assert_eq!(Sfx::Count(3).spec().usa.0, 3);
        assert_eq!(Sfx::Ready.bus(), Bus::Announcer);
        assert_eq!(Sfx::Banana.bus(), Bus::Sfx);
        assert_eq!(sfx_paths().len(), 8);
        assert!(sfx_paths().contains(&"test/snd/mkb/comn.samp".to_string()));
    }

    #[test]
    fn requests_are_batched_until_flush() {
        let (audio, log) = loaded();
        audio.play(Sfx::Banana);
        audio.play_with(Sfx::Goal, 0.5, 0.5);
        assert!(log.borrow().is_empty());
        audio.flush();
        let c = commands(&log);
        assert_eq!(c.len(), 2);
        assert_eq!(
            c[0],
            Command::Play {
                sound: 206,
                bus: Bus::Sfx,
                volume: velocity_amplitude(110),
                pan: 0.0,
                rate: 1.0,
                delay: 0.0
            }
        );
        assert!(matches!(
            c[1],
            Command::Play { sound: 160, bus: Bus::Announcer, delay, volume, .. }
                if delay == 0.5 && (volume - velocity_amplitude(110) * 0.5).abs() < 1e-6
        ));
        audio.flush();
        assert_eq!(commands(&log).len(), 2);
    }

    #[test]
    fn unloaded_sounds_are_dropped() {
        let (audio, log) = Audio::recording();
        audio.play(Sfx::Go);
        audio.roll(100.0, 10.0);
        audio.flush();
        // Only the initial category volumes went out.
        assert_eq!(
            commands(&log),
            vec![
                Command::BusVolume {
                    bus: Bus::Music,
                    volume: 0.5
                },
                Command::BusVolume {
                    bus: Bus::Sfx,
                    volume: 0.3
                },
                Command::BusVolume {
                    bus: Bus::Announcer,
                    volume: 0.3
                },
            ]
        );
        // The silent system never tries to load.
        let silent = Audio::silent();
        silent.ensure_sfx(&mut |_| panic!("silent audio must not read the disc"), None);
        silent.stage_music("bg_jun", &mut |_| {
            Err(io::Error::new(io::ErrorKind::NotFound, "no disc"))
        });
        assert_eq!(silent.music_key(), None);
        assert_eq!(silent.load_report().errors.len(), 1);
    }

    #[test]
    fn roll_starts_updates_and_stops_three_layers() {
        let (audio, log) = loaded();
        audio.roll(0.0, 0.0);
        audio.roll(64.0, 20.0);
        audio.roll(127.0, 68.0);
        audio.roll(0.0, 0.0);
        audio.roll(0.0, 0.0);
        audio.flush();
        let c = commands(&log);
        assert_eq!(c.len(), 9);
        assert!(matches!(
            c[0],
            Command::LoopStart {
                slot: 0,
                sound: 248,
                ..
            }
        ));
        assert!(matches!(
            c[1],
            Command::LoopStart {
                slot: 1,
                sound: 273,
                ..
            }
        ));
        assert!(matches!(
            c[2],
            Command::LoopStart {
                slot: 2,
                sound: 274,
                ..
            }
        ));
        assert!(matches!(c[3], Command::LoopSet { slot: 0, volume, .. } if volume == 1.0));
        assert!(matches!(c[5], Command::LoopSet { slot: 2, .. }));
        assert_eq!(c[6], Command::LoopStop { slot: 0 });
        assert_eq!(c[8], Command::LoopStop { slot: 2 });
    }

    #[test]
    fn roll_curves_match_the_original_tables() {
        let (g, rate) = roll_layers(0.0, 0.0);
        assert_eq!(g, [0.0; 3]);
        // Pitch value 0 bends down to 4608 / 16384: -0.875 semitones.
        assert!((rate - 2f32.powf(-0.875 / 12.0)).abs() < 1e-5);
        let (g, rate) = roll_layers(127.0, 127.0);
        assert_eq!(g, [1.0; 3]);
        assert!((rate - 2f32.powf(2.0 / 12.0)).abs() < 1e-3);
        // Half volume: the middle layer is plain, the outer ones swell in later.
        let (g, _) = roll_layers(63.5, 0.0);
        assert!((g[1] - 0.25).abs() < 1e-6);
        assert!((g[0] - 0.0625).abs() < 1e-6 && g[0] == g[2]);
        // The game's pitch value tops out at 68.
        let (_, rate) = roll_layers(127.0, 68.0);
        assert!(rate > 1.0 && rate < 1.06);
    }

    #[test]
    fn muted_categories_do_no_work() {
        let (audio, log) = loaded();
        audio.set_volumes(50, 0, 30);
        audio.play(Sfx::Banana);
        audio.roll(100.0, 10.0);
        audio.play(Sfx::Ready);
        audio.flush();
        let c = commands(&log);
        assert_eq!(
            c[0],
            Command::BusVolume {
                bus: Bus::Sfx,
                volume: 0.0
            }
        );
        assert_eq!(c.len(), 2, "{c:?}");
        assert!(matches!(c[1], Command::Play { sound: 170, .. }));
        // Muting while the roll is running stops its loops.
        audio.set_volumes(50, 30, 30);
        audio.roll(100.0, 10.0);
        audio.set_volumes(50, 0, 30);
        audio.flush();
        let c = commands(&log);
        assert_eq!(c[c.len() - 1], Command::LoopStop { slot: 2 });
        // Music at zero: nothing is fetched.
        audio.set_volumes(0, 30, 30);
        audio.stage_music("bg_jun", &mut |p| panic!("read {p} with music muted"));
        assert_eq!(audio.music_key(), None);
    }

    /// A stream of `frames` silent stereo frames in the disc's ADP layout (32-byte frames of 28 samples).
    fn adp(frames: usize) -> Vec<u8> {
        let mut v = vec![0u8; frames.div_ceil(28) * 32];
        // A non-zero sample in every frame, so nothing is trimmed as padding.
        for f in v.chunks_mut(32) {
            f[4] = 0x11;
        }
        v
    }

    #[test]
    fn stage_music_changes_only_with_the_track() {
        let (audio, log) = loaded();
        let mut reads = Vec::new();
        let mut fetch = |p: &str| {
            reads.push(p.to_string());
            Ok(adp(56))
        };
        audio.stage_music("bg_nig", &mut fetch);
        audio.stage_music("bg_nig", &mut fetch);
        audio.flush();
        assert_eq!(audio.music_key(), Some("st3"));
        audio.stage_music("bg_jun", &mut fetch);
        audio.stage_music("bg_pil", &mut fetch);
        audio.flush();
        assert_eq!(audio.music_key(), None);
        assert_eq!(
            reads,
            vec![
                "test/snd/adp/st3_int.adp",
                "test/snd/adp/st3_lp.adp",
                "test/snd/adp/st1.adp"
            ]
        );
        let log = log.borrow();
        let music: Vec<&Recorded> = log
            .iter()
            .filter(|r| !matches!(r, Recorded::Command(Command::BusVolume { .. })))
            .collect();
        assert_eq!(
            music,
            vec![
                &Recorded::DefineMusic {
                    intro_frames: Some(56),
                    body_frames: 56
                },
                &Recorded::Command(Command::MusicPlay {
                    volume: 155.0 / 255.0
                }),
                &Recorded::DefineMusic {
                    intro_frames: None,
                    body_frames: 56
                },
                &Recorded::ClearMusic,
                // The jungle track was replaced before its start was ever sent; only the stop remains.
                &Recorded::Command(Command::MusicStop { fade: MUSIC_FADE }),
            ]
        );
        assert_eq!(audio.load_report().music_loads, 2);
    }

    #[test]
    fn leaving_a_stage_cuts_sounds_but_not_music() {
        let (audio, log) = loaded();
        audio.stage_music("bg_jun", &mut |_| Ok(adp(28)));
        audio.roll(100.0, 10.0);
        audio.play(Sfx::Bumper);
        audio.stop_sounds();
        audio.flush();
        let c = commands(&log);
        assert_eq!(
            c,
            vec![
                Command::MusicPlay {
                    volume: 140.0 / 255.0
                },
                Command::StopSounds
            ]
        );
        // The roll starts afresh afterwards.
        audio.roll(100.0, 10.0);
        audio.flush();
        assert!(matches!(
            commands(&log)[2],
            Command::LoopStart { slot: 0, .. }
        ));
        audio.set_suspended(true);
        audio.set_suspended(true);
        audio.set_suspended(false);
        let suspends = log
            .borrow()
            .iter()
            .filter(|r| matches!(r, Recorded::Suspended(_)))
            .count();
        assert_eq!(suspends, 2);
    }

    #[test]
    fn mix_backend_translates_commands() {
        let (handle, mut driver) = mb_audio::driver::mix_channel(1000);
        let mut b = MixBackend::new(handle);
        b.define_sound(5, Arc::new(Sound::mono(1000, vec![16384; 4], None)));
        b.define_music(None, Arc::new(Sound::mono(1000, vec![8192; 2], None)));
        b.submit(&[
            Command::Play {
                sound: 5,
                bus: Bus::Sfx,
                volume: 1.0,
                pan: 0.0,
                rate: 1.0,
                delay: 0.0,
            },
            // Unknown sounds are skipped.
            Command::Play {
                sound: 6,
                bus: Bus::Sfx,
                volume: 1.0,
                pan: 0.0,
                rate: 1.0,
                delay: 0.0,
            },
            Command::MusicPlay { volume: 1.0 },
        ]);
        let mut out = [0.0f32; 4];
        driver.process(&mut out);
        // (0.5 + 0.25) * master 0.9
        assert!((out[0] - 0.675).abs() < 1e-6);
        b.submit(&[Command::StopSounds]);
        driver.process(&mut out);
        assert!((out[0] - 0.225).abs() < 1e-6);
        // The old track stays alive until the mixer lets go of it, then is freed on this side.
        b.define_music(None, Arc::new(Sound::mono(1000, vec![0; 2], None)));
        assert_eq!(b.retired.len(), 1);
        driver.process(&mut out);
        b.frame();
        assert!(b.retired.is_empty());
        assert_eq!(out[0], 0.0);
        let r = b.report();
        assert_eq!(r.mix.unwrap().callbacks, 3);
        assert!(r.summary().contains("3 callbacks"));
    }
}
