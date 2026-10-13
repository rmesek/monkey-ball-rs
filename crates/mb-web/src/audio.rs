//! Web Audio backend: the browser mixes.
//!
//! Every sound effect becomes an `AudioBuffer` once, at load; playing one is an `AudioBufferSourceNode`
//! through a `GainNode` (and a `StereoPannerNode` when panned) into its category's gain node. Music is two
//! buffers scheduled on the audio clock: the intro at `t`, the looping body at `t + intro.duration`, so
//! the hand-off is gapless whatever the main thread is doing. Nothing is mixed in wasm, and a frame that
//! requests nothing makes no JS call at all.
//!
//! ```text
//! source -> gain [-> panner] -> bus (music | sfx | announcer) -> master (0.9) -> destination
//!                                                                        \-> analyser (debug only)
//! ```
//!
//! Autoplay policy: the context is created when the game starts (after the user picked the disc image, so
//! normally with user activation). If the browser still starts it suspended, the first key, pointer or
//! touch on the page resumes it; sounds scheduled meanwhile are on a clock that does not run, so they play
//! from that moment. A hidden tab suspends the context (music pauses, nothing is processed) and showing
//! it again resumes.
//!
//! Memory: an `AudioBuffer` is 32-bit float, twice the size of 16-bit PCM (up to 54 MB for the longest
//! track, held by the browser, not in wasm memory). Only the current stage's music is kept, and it is decoded
//! from the disc's ADPCM stream straight into its buffer a block at a time, so the track never exists as
//! PCM in wasm memory (which cannot shrink again). See `audio_debug_json` for the live numbers and
//! `docs/AUDIO.md` for measurements.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use mb_app::audio::{Audio, AudioBackend, BackendReport, Bus, Command, MASTER_GAIN};
use mb_app::mb_audio::adp::{
    ADP_FRAME_BYTES, ADP_FRAME_SAMPLES, ADP_SAMPLE_RATE, AdpDecoder, adp_frame_count,
};
use mb_app::mb_audio::mixer::Sound;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use web_sys::{
    AnalyserNode, AudioBuffer, AudioBufferSourceNode, AudioContext, AudioContextState, AudioNode,
    AudioScheduledSourceNode, GainNode,
};

/// Loop slots (the ball roll uses three).
const LOOP_SLOTS: usize = 8;
/// Frames converted per `copyToChannel` call; bounds the scratch buffer.
const COPY_FRAMES: usize = 32_768;
/// Music starts this far ahead on the audio clock (as the reference does).
const MUSIC_LEAD: f64 = 0.01;
/// Time constant of live loop volume changes, and of the fade when a loop stops.
const LOOP_GLIDE: f64 = 0.015;
/// One-shots tracked for [`Command::StopSounds`] are pruned once the list is this long.
const PRUNE_AT: usize = 24;
/// Samples the debug analyser looks at.
const ANALYSER_SIZE: u32 = 2048;

struct Voice {
    source: AudioBufferSourceNode,
    /// Audio-clock time after which the source has ended by itself.
    ends: f64,
}

struct Looping {
    source: AudioBufferSourceNode,
    gain: GainNode,
}

struct Music {
    intro: Option<AudioBufferSourceNode>,
    body: AudioBufferSourceNode,
    gain: GainNode,
}

/// Numbers for `audio_debug_json`, updated as things happen (no polling).
#[derive(Default)]
struct Stats {
    sound_buffers: Cell<u32>,
    sound_bytes: Cell<u64>,
    music_bytes: Cell<u64>,
    /// Milliseconds spent filling `AudioBuffer`s (the PCM decode itself is in the load report).
    buffer_ms: Cell<f64>,
    /// `Date.now()` of the last buffer creation: nothing may happen here during play.
    last_buffer_at: Cell<f64>,
    buffers_made: Cell<u32>,
    sources_started: Cell<u32>,
    loops_started: Cell<u32>,
    music_starts: Cell<u32>,
    music_playing: Cell<bool>,
    resumes: Cell<u32>,
}

/// What the page-level listeners and the debug export need.
struct Shared {
    ctx: AudioContext,
    master: GainNode,
    stats: Stats,
    /// The app itself asked for silence (not the tab's visibility).
    app_suspended: Cell<bool>,
    analyser: RefCell<Option<AnalyserNode>>,
}

impl Shared {
    fn hidden() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden())
    }

    /// Run or stop the audio clock according to the app state and the tab's visibility.
    fn sync_running(&self) {
        let want = !self.app_suspended.get() && !Self::hidden();
        match (want, self.ctx.state()) {
            (true, AudioContextState::Suspended) => {
                self.stats.resumes.set(self.stats.resumes.get() + 1);
                let _ = self.ctx.resume();
            }
            (false, AudioContextState::Running) => {
                let _ = self.ctx.suspend();
            }
            _ => {}
        }
    }
}

thread_local! {
    static SHARED: RefCell<Option<Rc<Shared>>> = const { RefCell::new(None) };
    static AUDIO: RefCell<Option<Audio>> = const { RefCell::new(None) };
}

pub struct WebAudio {
    shared: Rc<Shared>,
    buses: [GainNode; 3],
    sounds: Vec<Option<AudioBuffer>>,
    music_buffers: Option<(Option<AudioBuffer>, AudioBuffer)>,
    music: Option<Music>,
    loops: [Option<Looping>; LOOP_SLOTS],
    voices: Vec<Voice>,
    scratch: Vec<f32>,
    /// Decoded block of a music stream on its way into an `AudioBuffer` (interleaved, reused).
    pcm: Vec<i16>,
}

/// End a source at `when` on the audio clock (0 = now). Stopping one that already ended is harmless.
fn stop_at(source: &AudioScheduledSourceNode, when: f64) {
    let _ = source.stop_with_when(when);
}

fn connect(from: &AudioNode, to: &AudioNode) -> Result<(), JsValue> {
    from.connect_with_audio_node(to).map(|_| ())
}

impl WebAudio {
    /// `None` when the browser has no Web Audio (the game then runs silent).
    pub fn new() -> Option<Self> {
        let ctx = AudioContext::new().ok()?;
        let master = ctx.create_gain().ok()?;
        master.gain().set_value(MASTER_GAIN);
        connect(&master, &ctx.destination()).ok()?;
        let bus = || -> Option<GainNode> {
            let g = ctx.create_gain().ok()?;
            connect(&g, &master).ok()?;
            Some(g)
        };
        let buses = [bus()?, bus()?, bus()?];
        let shared = Rc::new(Shared {
            ctx,
            master,
            stats: Stats::default(),
            app_suspended: Cell::new(false),
            analyser: RefCell::new(None),
        });
        Self::listen(&shared);
        SHARED.with(|s| *s.borrow_mut() = Some(shared.clone()));
        Some(Self {
            shared,
            buses,
            sounds: Vec::new(),
            music_buffers: None,
            music: None,
            loops: Default::default(),
            voices: Vec::new(),
            scratch: Vec::new(),
            pcm: Vec::new(),
        })
    }

    /// Page listeners, for the lifetime of the page: a user gesture resumes a context the autoplay policy
    /// held back, and the tab's visibility pauses and resumes it.
    fn listen(shared: &Rc<Shared>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        for event in ["pointerdown", "keydown", "touchend"] {
            let s = shared.clone();
            let on_gesture = Closure::<dyn FnMut()>::new(move || s.sync_running());
            let _ =
                window.add_event_listener_with_callback(event, on_gesture.as_ref().unchecked_ref());
            on_gesture.forget();
        }
        if let Some(document) = window.document() {
            let s = shared.clone();
            let on_visibility = Closure::<dyn FnMut()>::new(move || s.sync_running());
            let _ = document.add_event_listener_with_callback(
                "visibilitychange",
                on_visibility.as_ref().unchecked_ref(),
            );
            on_visibility.forget();
        }
    }

    fn ctx(&self) -> &AudioContext {
        &self.shared.ctx
    }

    /// Copy 16-bit PCM into a new float `AudioBuffer` at the sound's own sample rate.
    fn buffer(&mut self, sound: &Sound) -> Option<AudioBuffer> {
        let started = js_sys::Date::now();
        let frames = sound.frames();
        let channels = sound.channels.max(1);
        let buffer = self
            .shared
            .ctx
            .create_buffer(
                channels as u32,
                frames.max(1) as u32,
                sound.sample_rate as f32,
            )
            .ok()?;
        self.scratch.resize(COPY_FRAMES, 0.0);
        for ch in 0..channels {
            let mut at = 0;
            while at < frames {
                let n = (frames - at).min(COPY_FRAMES);
                let src = sound.data[at * channels + ch..].iter().step_by(channels);
                for (out, s) in self.scratch[..n].iter_mut().zip(src) {
                    *out = f32::from(*s) / 32768.0;
                }
                buffer
                    .copy_to_channel_with_start_in_channel(&self.scratch[..n], ch as i32, at as u32)
                    .ok()?;
                at += n;
            }
        }
        let st = &self.shared.stats;
        let now = js_sys::Date::now();
        st.buffer_ms.set(st.buffer_ms.get() + (now - started));
        st.last_buffer_at.set(now);
        st.buffers_made.set(st.buffers_made.get() + 1);
        Some(buffer)
    }

    /// Decode a DTK stream into a new stereo `AudioBuffer`, a block at a time: the same samples as
    /// decoding it whole and passing the result to [`WebAudio::buffer`], without the whole track in memory.
    fn stream_buffer(&mut self, stream: &[u8]) -> Result<AudioBuffer, String> {
        /// DTK frames per block: 28 sample pairs each, so one block fits `COPY_FRAMES`.
        const BLOCK: usize = COPY_FRAMES / ADP_FRAME_SAMPLES;
        let started = js_sys::Date::now();
        let frames = adp_frame_count(stream, true).map_err(|e| e.to_string())?;
        let buffer = self
            .shared
            .ctx
            .create_buffer(
                2,
                (frames * ADP_FRAME_SAMPLES).max(1) as u32,
                ADP_SAMPLE_RATE as f32,
            )
            .map_err(|_| "the browser could not allocate the music buffer".to_string())?;
        self.scratch.resize(COPY_FRAMES, 0.0);
        let mut decoder = AdpDecoder::new();
        let mut at = 0usize;
        for block in stream[..frames * ADP_FRAME_BYTES].chunks(BLOCK * ADP_FRAME_BYTES) {
            self.pcm.clear();
            decoder.decode(block, &mut self.pcm);
            let n = self.pcm.len() / 2;
            for ch in 0..2 {
                let src = self.pcm[ch..].iter().step_by(2);
                for (out, s) in self.scratch[..n].iter_mut().zip(src) {
                    *out = f32::from(*s) / 32768.0;
                }
                buffer
                    .copy_to_channel_with_start_in_channel(&self.scratch[..n], ch as i32, at as u32)
                    .map_err(|_| "the browser refused the music data".to_string())?;
            }
            at += n;
        }
        let st = &self.shared.stats;
        let now = js_sys::Date::now();
        st.buffer_ms.set(st.buffer_ms.get() + (now - started));
        st.last_buffer_at.set(now);
        st.buffers_made.set(st.buffers_made.get() + 1);
        Ok(buffer)
    }

    fn buffer_bytes(buffer: &AudioBuffer) -> u64 {
        u64::from(buffer.length()) * u64::from(buffer.number_of_channels()) * 4
    }

    /// A source for `buffer` feeding a new gain node connected to `bus`.
    fn source(
        &self,
        buffer: &AudioBuffer,
        bus: Bus,
        volume: f32,
        rate: f32,
        pan: f32,
    ) -> Option<(AudioBufferSourceNode, GainNode)> {
        let ctx = self.ctx();
        let source = ctx.create_buffer_source().ok()?;
        source.set_buffer(Some(buffer));
        if rate != 1.0 {
            source.playback_rate().set_value(rate);
        }
        let gain = ctx.create_gain().ok()?;
        gain.gain().set_value(volume);
        connect(&source, &gain).ok()?;
        let out = &self.buses[bus as usize];
        if pan != 0.0 {
            let panner = ctx.create_stereo_panner().ok()?;
            panner.pan().set_value(pan.clamp(-1.0, 1.0));
            connect(&gain, &panner).ok()?;
            connect(&panner, out).ok()?;
        } else {
            connect(&gain, out).ok()?;
        }
        Some((source, gain))
    }

    fn play(&mut self, sound: u16, bus: Bus, volume: f32, pan: f32, rate: f32, delay: f32) {
        let Some(Some(buffer)) = self.sounds.get(sound as usize) else {
            return;
        };
        let Some((source, _gain)) = self.source(buffer, bus, volume, rate, pan) else {
            return;
        };
        let now = self.ctx().current_time();
        let when = now + f64::from(delay.max(0.0));
        if source.start_with_when(when).is_err() {
            return;
        }
        let ends = when + buffer.duration() / f64::from(rate.max(0.01)) + 0.1;
        if self.voices.len() >= PRUNE_AT {
            self.voices.retain(|v| v.ends > now);
        }
        self.voices.push(Voice { source, ends });
        let st = &self.shared.stats;
        st.sources_started.set(st.sources_started.get() + 1);
    }

    fn loop_start(&mut self, slot: u8, sound: u16, bus: Bus, volume: f32, rate: f32) {
        self.loop_stop(slot);
        let Some(Some(buffer)) = self.sounds.get(sound as usize) else {
            return;
        };
        let Some((source, gain)) = self.source(buffer, bus, volume, rate, 0.0) else {
            return;
        };
        source.set_loop(true);
        if source.start().is_err() {
            return;
        }
        if let Some(entry) = self.loops.get_mut(slot as usize) {
            *entry = Some(Looping { source, gain });
            let st = &self.shared.stats;
            st.loops_started.set(st.loops_started.get() + 1);
        }
    }

    fn loop_stop(&mut self, slot: u8) {
        let Some(l) = self.loops.get_mut(slot as usize).and_then(Option::take) else {
            return;
        };
        // A short glide to zero, then the source ends; the nodes are collected by the browser.
        let now = self.shared.ctx.current_time();
        let _ = l.gain.gain().set_target_at_time(0.0, now, LOOP_GLIDE / 3.0);
        stop_at(&l.source, now + LOOP_GLIDE * 4.0);
    }

    fn music_stop(&mut self, fade: f32) {
        let Some(m) = self.music.take() else {
            return;
        };
        self.shared.stats.music_playing.set(false);
        let now = self.ctx().current_time();
        let end = now + f64::from(fade.max(0.0));
        if fade > 0.0 {
            let g = m.gain.gain();
            let _ = g.set_value_at_time(g.value(), now);
            let _ = g.linear_ramp_to_value_at_time(0.0, end);
        }
        for source in m.intro.iter().chain(std::iter::once(&m.body)) {
            stop_at(source, end);
        }
    }

    fn music_play(&mut self, volume: f32) {
        self.music_stop(0.0);
        let Some((intro, body)) = &self.music_buffers else {
            return;
        };
        let ctx = self.ctx();
        let Ok(gain) = ctx.create_gain() else {
            return;
        };
        gain.gain().set_value(volume);
        if connect(&gain, &self.buses[Bus::Music as usize]).is_err() {
            return;
        }
        let make = |buffer: &AudioBuffer| -> Option<AudioBufferSourceNode> {
            let s = ctx.create_buffer_source().ok()?;
            s.set_buffer(Some(buffer));
            connect(&s, &gain).ok()?;
            Some(s)
        };
        let Some(body_source) = make(body) else {
            return;
        };
        body_source.set_loop(true);
        let start = ctx.current_time() + MUSIC_LEAD;
        let mut body_at = start;
        let intro_source = intro.as_ref().and_then(|buffer| {
            let s = make(buffer)?;
            s.start_with_when(start).ok()?;
            // The loop begins on the audio clock exactly where the intro ends.
            body_at = start + buffer.duration();
            Some(s)
        });
        if body_source.start_with_when(body_at).is_err() {
            return;
        }
        self.music = Some(Music {
            intro: intro_source,
            body: body_source,
            gain,
        });
        let st = &self.shared.stats;
        st.music_starts.set(st.music_starts.get() + 1);
        st.music_playing.set(true);
    }

    fn stop_sounds(&mut self) {
        for v in self.voices.drain(..) {
            // Already ended sources throw nothing; a not-yet-started one is cancelled.
            stop_at(&v.source, 0.0);
        }
        for slot in 0..LOOP_SLOTS as u8 {
            self.loop_stop(slot);
        }
    }
}

impl AudioBackend for WebAudio {
    fn define_sound(&mut self, id: u16, sound: Arc<Sound>) {
        let Some(buffer) = self.buffer(&sound) else {
            return;
        };
        let st = &self.shared.stats;
        st.sound_buffers.set(st.sound_buffers.get() + 1);
        st.sound_bytes
            .set(st.sound_bytes.get() + Self::buffer_bytes(&buffer));
        let i = id as usize;
        if self.sounds.len() <= i {
            self.sounds.resize(i + 1, None);
        }
        self.sounds[i] = Some(buffer);
    }

    fn define_music(&mut self, intro: Option<Arc<Sound>>, body: Arc<Sound>) {
        // The previous track goes first, so two tracks are never held at once.
        self.music_stop(0.0);
        self.music_buffers = None;
        self.shared.stats.music_bytes.set(0);
        let intro = intro.and_then(|s| self.buffer(&s));
        let Some(body) = self.buffer(&body) else {
            return;
        };
        let bytes = Self::buffer_bytes(&body) + intro.as_ref().map_or(0, Self::buffer_bytes);
        self.shared.stats.music_bytes.set(bytes);
        self.music_buffers = Some((intro, body));
    }

    fn define_music_streams(&mut self, intro: Option<&[u8]>, body: &[u8]) -> Result<bool, String> {
        // The previous track goes first, so two tracks are never held at once.
        self.music_stop(0.0);
        self.music_buffers = None;
        self.shared.stats.music_bytes.set(0);
        let intro = intro.map(|s| self.stream_buffer(s)).transpose()?;
        let body = self.stream_buffer(body)?;
        let bytes = Self::buffer_bytes(&body) + intro.as_ref().map_or(0, Self::buffer_bytes);
        self.shared.stats.music_bytes.set(bytes);
        self.music_buffers = Some((intro, body));
        Ok(true)
    }

    fn clear_music(&mut self) {
        // Sources that are still fading out keep their buffer until they end.
        self.music_buffers = None;
        self.shared.stats.music_bytes.set(0);
    }

    fn submit(&mut self, commands: &[Command]) {
        for c in commands {
            match *c {
                Command::Play {
                    sound,
                    bus,
                    volume,
                    pan,
                    rate,
                    delay,
                } => self.play(sound, bus, volume, pan, rate, delay),
                Command::LoopStart {
                    slot,
                    sound,
                    bus,
                    volume,
                    rate,
                } => self.loop_start(slot, sound, bus, volume, rate),
                Command::LoopSet { slot, volume, rate } => {
                    if let Some(Some(l)) = self.loops.get(slot as usize) {
                        // A start time of zero means "now"; the glide avoids zipper noise.
                        let _ = l.gain.gain().set_target_at_time(volume, 0.0, LOOP_GLIDE);
                        l.source.playback_rate().set_value(rate);
                    }
                }
                Command::LoopStop { slot } => self.loop_stop(slot),
                Command::MusicPlay { volume } => self.music_play(volume),
                Command::MusicStop { fade } => self.music_stop(fade),
                Command::BusVolume { bus, volume } => {
                    self.buses[bus as usize].gain().set_value(volume)
                }
                Command::StopSounds => self.stop_sounds(),
            }
        }
    }

    fn set_suspended(&mut self, suspended: bool) {
        self.shared.app_suspended.set(suspended);
        self.shared.sync_running();
    }

    fn report(&self) -> BackendReport {
        let ctx = self.ctx();
        BackendReport {
            backend: "web audio",
            device: format!("AudioContext ({:?})", ctx.state()),
            running: ctx.state() == AudioContextState::Running,
            sample_rate: ctx.sample_rate() as u32,
            channels: ctx.destination().channel_count() as u16,
            ..BackendReport::default()
        }
    }
}

/// The game's audio system for this page: Web Audio when the browser has it, silence otherwise.
pub fn create() -> Audio {
    let audio = match WebAudio::new() {
        Some(backend) => Audio::new(Box::new(backend)),
        None => {
            web_sys::console::warn_1(&JsValue::from_str("no Web Audio: the game runs silent"));
            Audio::silent()
        }
    };
    AUDIO.with(|a| *a.borrow_mut() = Some(audio.clone()));
    audio
}

/// State of the audio system as JSON, for the developer console and automated checks:
/// context state and clock, decoded buffers and their sizes, load times, how many nodes were started, and
/// the level an `AnalyserNode` on the master output sees right now. The analyser is created by the first
/// call (so the first reading is silence) and exists only if this is ever called.
#[wasm_bindgen]
pub fn audio_debug_json() -> String {
    let Some(shared) = SHARED.with(|s| s.borrow().clone()) else {
        return "{\"backend\":\"none\"}".into();
    };
    let ctx = &shared.ctx;
    let mut analyser = shared.analyser.borrow_mut();
    if analyser.is_none()
        && let Ok(a) = ctx.create_analyser()
    {
        a.set_fft_size(ANALYSER_SIZE);
        if connect(&shared.master, &a).is_ok() {
            *analyser = Some(a);
        }
    }
    let (mut peak, mut energy) = (0f32, 0f64);
    if let Some(a) = analyser.as_ref() {
        let mut data = vec![0f32; ANALYSER_SIZE as usize];
        a.get_float_time_domain_data(&mut data);
        for s in &data {
            peak = peak.max(s.abs());
            energy += f64::from(*s) * f64::from(*s);
        }
        energy = (energy / data.len() as f64).sqrt();
    }
    let st = &shared.stats;
    let load = AUDIO.with(|a| a.borrow().as_ref().map(Audio::load_report));
    let load = load.unwrap_or_default();
    format!(
        concat!(
            "{{\"backend\":\"web audio\",\"state\":\"{:?}\",\"sampleRate\":{},\"currentTime\":{:.3},",
            "\"hidden\":{},\"resumes\":{},",
            "\"soundBuffers\":{},\"soundBytes\":{},\"musicBytes\":{},\"musicKey\":\"{}\",",
            "\"buffersMade\":{},\"bufferMs\":{:.1},\"lastBufferAt\":{:.0},\"now\":{:.0},",
            "\"sfxDecodeMs\":{:.1},\"sfxPcmBytes\":{},\"sfxFromTable\":{},\"sfxMissing\":{},",
            "\"musicDecodeMs\":{:.1},\"musicPcmBytes\":{},\"musicLoads\":{},",
            "\"sourcesStarted\":{},\"loopsStarted\":{},\"musicStarts\":{},\"musicPlaying\":{},",
            "\"errors\":{},\"analyserPeak\":{:.5},\"analyserRms\":{:.5}}}"
        ),
        ctx.state(),
        ctx.sample_rate(),
        ctx.current_time(),
        Shared::hidden(),
        st.resumes.get(),
        st.sound_buffers.get(),
        st.sound_bytes.get(),
        st.music_bytes.get(),
        load.music_key.unwrap_or(""),
        st.buffers_made.get(),
        st.buffer_ms.get(),
        st.last_buffer_at.get(),
        js_sys::Date::now(),
        load.sfx_ms,
        load.sfx_pcm_bytes,
        load.sfx_from_table,
        load.sfx_missing.len(),
        load.music_ms,
        load.music_pcm_bytes,
        load.music_loads,
        st.sources_started.get(),
        st.loops_started.get(),
        st.music_starts.get(),
        st.music_playing.get(),
        load.errors.len(),
        peak,
        energy
    )
}
