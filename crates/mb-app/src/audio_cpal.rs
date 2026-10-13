//! Native audio output through cpal (WASAPI, CoreAudio, ALSA): the device's callback
//! pulls stereo PCM from `mb-audio`'s mixer.
//!
//! Threads: the game thread owns [`CpalBackend`] and sends `MixMsg`s through the mixer's bounded queue
//! (never blocks). The output thread's callback takes the mixer with `try_lock`, drains the queue and
//! renders; the lock is uncontended while a stream runs, because the game thread only touches the mixer
//! when there is no running stream (to keep time, or to re-target its sample rate while reopening). The
//! callback does not allocate or do I/O.
//!
//! The mixer resamples every voice to the device's rate, whatever it is, and the callback folds stereo to
//! the device's channel count (mono: average; more than two: front pair, rest silent).
//!
//! No device, or a device that goes away, is not an error for the game: the backend stays silent, keeps
//! the mixer's clock running so the music is in the right place later, and retries every two seconds.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use mb_audio::driver::{MixDriver, mix_channel};
use mb_audio::mixer::Sound;

use crate::audio::{AudioBackend, BackendReport, Command, MixBackend};

/// Rate the mixer runs at until a device says otherwise.
const DEFAULT_RATE: u32 = 48_000;
/// Frames rendered per `process` call inside one callback (bounds the stack/scratch use).
const CHUNK_FRAMES: usize = 1024;
const RETRY_EVERY: Duration = Duration::from_secs(2);
/// Longest stretch of silent time kept in one frame while there is no device.
const MAX_SILENT_STEP: f64 = 0.25;

#[derive(Default)]
struct Counters {
    lock_misses: AtomicU64,
    late_callbacks: AtomicU64,
    stream_errors: AtomicU64,
    /// Set by the stream's error callback: the stream is dead and must be reopened.
    failed: AtomicBool,
}

struct Open {
    stream: cpal::Stream,
    device: String,
    rate: u32,
    channels: u16,
}

pub struct CpalBackend {
    mix: MixBackend,
    driver: Arc<Mutex<MixDriver>>,
    counters: Arc<Counters>,
    open: Option<Open>,
    /// Why there is no stream.
    last_error: String,
    opens: u32,
    suspended: bool,
    next_retry: Instant,
    last_frame: Instant,
    /// Fraction of a frame of silent time not yet rendered.
    silent_debt: f64,
    scratch: Vec<f32>,
}

/// Write `stereo` (interleaved pairs) into a device buffer of `channels` channels per frame.
fn write_frames<T>(out: &mut [T], channels: usize, stereo: &[f32])
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    for (frame, lr) in out.chunks_mut(channels).zip(stereo.as_chunks::<2>().0) {
        match frame {
            [mono] => *mono = T::from_sample((lr[0] + lr[1]) * 0.5),
            [l, r, rest @ ..] => {
                *l = T::from_sample(lr[0]);
                *r = T::from_sample(lr[1]);
                for s in rest {
                    *s = T::EQUILIBRIUM;
                }
            }
            [] => {}
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    driver: Arc<Mutex<MixDriver>>,
    counters: Arc<Counters>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let rate = config.sample_rate.0.max(1);
    let mut scratch = vec![0.0f32; CHUNK_FRAMES * 2];
    let mut last: Option<Instant> = None;
    let errors = counters.clone();
    let data = move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
        let now = Instant::now();
        let frames = out.len() / channels;
        if let Some(prev) = last {
            // A callback three buffer lengths (and at least 30 ms) after the previous one means the
            // device had nothing to play in between.
            let period = frames as f64 / f64::from(rate);
            if (now - prev).as_secs_f64() > (period * 3.0).max(0.03) {
                counters.late_callbacks.fetch_add(1, Relaxed);
            }
        }
        last = Some(now);
        let Ok(mut driver) = driver.try_lock() else {
            counters.lock_misses.fetch_add(1, Relaxed);
            out.fill(T::EQUILIBRIUM);
            return;
        };
        for chunk in out.chunks_mut(CHUNK_FRAMES * channels) {
            let n = chunk.len() / channels;
            let stereo = &mut scratch[..n * 2];
            driver.process(stereo);
            write_frames(chunk, channels, stereo);
        }
    };
    let error = move |_e: cpal::StreamError| {
        errors.stream_errors.fetch_add(1, Relaxed);
        errors.failed.store(true, Relaxed);
    };
    device
        .build_output_stream(config, data, error, None)
        .map_err(|e| format!("output stream: {e}"))
}

impl CpalBackend {
    /// Never fails: without a usable device the backend is silent and keeps retrying.
    pub fn new() -> Self {
        let (handle, driver) = mix_channel(DEFAULT_RATE);
        let now = Instant::now();
        let mut backend = Self {
            mix: MixBackend::new(handle),
            driver: Arc::new(Mutex::new(driver)),
            counters: Arc::new(Counters::default()),
            open: None,
            last_error: String::new(),
            opens: 0,
            suspended: false,
            next_retry: now,
            last_frame: now,
            silent_debt: 0.0,
            scratch: vec![0.0; CHUNK_FRAMES * 2],
        };
        backend.try_open();
        backend
    }

    fn try_open(&mut self) {
        self.next_retry = Instant::now() + RETRY_EVERY;
        self.counters.failed.store(false, Relaxed);
        match self.open_stream() {
            Ok(open) => {
                self.opens += 1;
                self.last_error.clear();
                self.open = Some(open);
            }
            Err(e) => {
                self.open = None;
                self.last_error = e;
            }
        }
    }

    fn open_stream(&mut self) -> Result<Open, String> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or("no audio output device")?;
        let name = device.name().unwrap_or_else(|_| "output device".into());
        let supported = device
            .default_output_config()
            .map_err(|e| format!("{name}: {e}"))?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        // No stream is running here, so this lock is free.
        match self.driver.lock() {
            Ok(mut d) => d.set_sample_rate(config.sample_rate.0),
            Err(_) => return Err("audio mixer is unusable (output thread panicked)".into()),
        }
        let (d, c) = (self.driver.clone(), self.counters.clone());
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, d, c),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, d, c),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config, d, c),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, &config, d, c),
            cpal::SampleFormat::U8 => build_stream::<u8>(&device, &config, d, c),
            other => Err(format!("unsupported sample format {other:?}")),
        }
        .map_err(|e| format!("{name}: {e}"))?;
        if !self.suspended {
            stream.play().map_err(|e| format!("{name}: {e}"))?;
        }
        Ok(Open {
            stream,
            device: format!("{name} ({format:?})"),
            rate: config.sample_rate.0,
            channels: config.channels,
        })
    }

    /// Without a running stream nobody pulls audio: advance the mixer by wall-clock time instead, so
    /// queued messages are applied, one-shots end and the music keeps its place.
    fn keep_time(&mut self, elapsed: f64) {
        let Ok(mut driver) = self.driver.try_lock() else {
            return;
        };
        let frames =
            elapsed.min(MAX_SILENT_STEP) * f64::from(driver.sample_rate()) + self.silent_debt;
        let whole = frames.floor();
        self.silent_debt = frames - whole;
        driver.advance(whole as usize, &mut self.scratch);
    }
}

impl Default for CpalBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioBackend for CpalBackend {
    fn define_sound(&mut self, id: u16, sound: Arc<Sound>) {
        self.mix.define_sound(id, sound);
    }

    fn define_music(&mut self, intro: Option<Arc<Sound>>, body: Arc<Sound>) {
        self.mix.define_music(intro, body);
    }

    fn clear_music(&mut self) {
        self.mix.clear_music();
    }

    fn submit(&mut self, commands: &[Command]) {
        self.mix.submit(commands);
    }

    fn frame(&mut self) {
        self.mix.frame();
        let now = Instant::now();
        let elapsed = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;
        if self.counters.failed.load(Relaxed) {
            // Dropping the stream stops its thread; the mixer and everything playing in it stay.
            self.open = None;
            self.last_error = "output device lost".into();
        }
        if self.open.is_some() {
            return;
        }
        if !self.suspended {
            self.keep_time(elapsed);
            if now >= self.next_retry {
                self.try_open();
            }
        }
    }

    fn set_suspended(&mut self, suspended: bool) {
        self.suspended = suspended;
        // Pausing the stream stops the callbacks: no mixing at all while in the background.
        if let Some(open) = &self.open {
            let ok = if suspended {
                open.stream.pause().is_ok()
            } else {
                open.stream.play().is_ok()
            };
            if !ok {
                self.counters.failed.store(true, Relaxed);
            }
        }
        self.last_frame = Instant::now();
    }

    fn report(&self) -> BackendReport {
        let (device, rate, channels) = match &self.open {
            Some(o) => (o.device.clone(), o.rate, o.channels),
            None => (
                format!("no output ({})", self.last_error),
                self.driver.try_lock().map_or(0, |d| d.sample_rate()),
                0,
            ),
        };
        BackendReport {
            backend: "cpal",
            device,
            running: self.open.is_some() && !self.suspended,
            sample_rate: rate,
            channels,
            mix: Some(self.mix.snapshot()),
            lock_misses: self.counters.lock_misses.load(Relaxed),
            late_callbacks: self.counters.late_callbacks.load(Relaxed),
            stream_errors: self.counters.stream_errors.load(Relaxed),
            opens: self.opens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_is_folded_to_the_device_layout() {
        let stereo = [0.5f32, -0.5, 1.0, 0.0];
        let mut mono = [0.0f32; 2];
        write_frames(&mut mono, 1, &stereo);
        assert_eq!(mono, [0.0, 0.5]);
        let mut two = [0i16; 4];
        write_frames(&mut two, 2, &stereo);
        assert_eq!(two, [16384, -16384, 32767, 0]);
        let mut six = [9.0f32; 12];
        write_frames(&mut six, 6, &stereo);
        assert_eq!(&six[..6], &[0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(&six[6..8], &[1.0, 0.0]);
        let mut u = [0u16; 2];
        write_frames(&mut u, 2, &[0.0, 0.0]);
        assert_eq!(u, [32768, 32768]);
    }

    /// Opens the machine's real default device if there is one; with none the backend must still work.
    #[test]
    fn backend_survives_whatever_the_machine_has() {
        let mut b = CpalBackend::new();
        b.define_sound(1, Arc::new(Sound::mono(22_050, vec![0; 64], None)));
        b.submit(&[Command::Play {
            sound: 1,
            bus: crate::audio::Bus::Sfx,
            volume: 0.0,
            pan: 0.0,
            rate: 1.0,
            delay: 0.0,
        }]);
        b.frame();
        b.set_suspended(true);
        b.frame();
        b.set_suspended(false);
        b.frame();
        let r = b.report();
        assert_eq!(r.backend, "cpal");
        assert!(r.running || r.device.starts_with("no output"), "{r:?}");
    }
}
