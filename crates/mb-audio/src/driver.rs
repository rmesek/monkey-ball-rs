//! The mixer behind a message queue: the game thread sends [`MixMsg`]s through a [`MixHandle`], the output
//! thread owns a [`MixDriver`] and calls [`MixDriver::process`] from its callback.
//!
//! The queue is a bounded `std::sync::mpsc::sync_channel`: its buffer is allocated once, `try_send` and
//! `try_recv` never block, and neither side allocates per message. [`MixDriver::process`] does no allocation
//! and no I/O; buffers arrive already decoded as `Arc<Sound>`. The sender keeps its own reference to every
//! sound it queues (see [`crate::mixer::Mixer`]), so dropping a finished voice on the output thread only
//! decrements a counter.
//!
//! [`MixStats`] is the honest telemetry of what was rendered: callbacks, frames, peak and RMS of the final
//! signal, clipped samples, messages lost to a full queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use crate::mixer::{Bus, Mixer, PlayParams, Sound, VoiceId};

/// Looping sounds addressed by slot (the ball roll uses three layers).
pub const LOOP_SLOTS: usize = 8;
/// Messages the queue holds; a frame of gameplay sends a handful.
pub const QUEUE_CAPACITY: usize = 512;
/// Glide time for live loop volume updates.
const LOOP_RAMP_SECONDS: f32 = 0.03;

#[derive(Debug, Clone)]
pub enum MixMsg {
    Play {
        sound: Arc<Sound>,
        bus: Bus,
        volume: f32,
        pan: f32,
        rate: f32,
        delay_seconds: f32,
    },
    /// Start (or restart) the looping sound in `slot`.
    LoopStart {
        slot: u8,
        sound: Arc<Sound>,
        bus: Bus,
        volume: f32,
        rate: f32,
    },
    LoopSet {
        slot: u8,
        volume: f32,
        rate: f32,
    },
    LoopStop {
        slot: u8,
    },
    /// Replace the music: optional intro once, then `body` looping.
    Music {
        intro: Option<Arc<Sound>>,
        body: Arc<Sound>,
        volume: f32,
    },
    MusicStop {
        fade_seconds: f32,
    },
    BusVolume {
        bus: Bus,
        volume: f32,
    },
    MasterVolume(f32),
    /// Stop every voice on a bus (loops included).
    StopBus(Bus),
    Pause(bool),
}

/// Counters shared between the output thread (writer) and anyone who wants to look.
#[derive(Debug, Default)]
pub struct MixStats {
    callbacks: AtomicU64,
    frames: AtomicU64,
    /// Bits of the largest absolute output sample (non-negative `f32`s order like their bits).
    peak_bits: AtomicU32,
    /// Sum of squares of both channels, in units of 2^-24 (so it fits an integer).
    energy: AtomicU64,
    clipped: AtomicU64,
    messages: AtomicU64,
    dropped: AtomicU64,
    voices: AtomicU32,
    max_voices: AtomicU32,
}

/// A copy of [`MixStats`] at one moment.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MixSnapshot {
    /// Times [`MixDriver::process`] ran.
    pub callbacks: u64,
    /// Stereo frames rendered.
    pub frames: u64,
    /// Largest absolute sample before clamping (1.0 = full scale).
    pub peak: f32,
    /// RMS over everything rendered so far.
    pub rms: f32,
    /// Samples that had to be clamped to [-1, 1].
    pub clipped: u64,
    /// Messages applied by the output thread.
    pub messages: u64,
    /// Messages lost because the queue was full or the driver was gone.
    pub dropped: u64,
    pub voices: u32,
    pub max_voices: u32,
}

impl MixStats {
    pub fn snapshot(&self) -> MixSnapshot {
        let frames = self.frames.load(Relaxed);
        let energy = self.energy.load(Relaxed) as f64 / (1u64 << 24) as f64;
        MixSnapshot {
            callbacks: self.callbacks.load(Relaxed),
            frames,
            peak: f32::from_bits(self.peak_bits.load(Relaxed)),
            rms: if frames > 0 {
                (energy / (frames as f64 * 2.0)).sqrt() as f32
            } else {
                0.0
            },
            clipped: self.clipped.load(Relaxed),
            messages: self.messages.load(Relaxed),
            dropped: self.dropped.load(Relaxed),
            voices: self.voices.load(Relaxed),
            max_voices: self.max_voices.load(Relaxed),
        }
    }
}

/// Game-thread end of the queue.
#[derive(Debug, Clone)]
pub struct MixHandle {
    tx: SyncSender<MixMsg>,
    stats: Arc<MixStats>,
}

impl MixHandle {
    /// Queue a message; never blocks. Returns false (and counts it) when the queue is full or closed.
    pub fn send(&self, msg: MixMsg) -> bool {
        match self.tx.try_send(msg) {
            Ok(()) => true,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.stats.dropped.fetch_add(1, Relaxed);
                false
            }
        }
    }

    pub fn stats(&self) -> &Arc<MixStats> {
        &self.stats
    }
}

/// Output-thread end: applies queued messages and renders.
pub struct MixDriver {
    mixer: Mixer,
    rx: Receiver<MixMsg>,
    loops: [Option<VoiceId>; LOOP_SLOTS],
    music: Option<VoiceId>,
    stats: Arc<MixStats>,
}

/// A connected handle / driver pair rendering at `out_rate`.
pub fn mix_channel(out_rate: u32) -> (MixHandle, MixDriver) {
    let (tx, rx) = sync_channel(QUEUE_CAPACITY);
    let stats = Arc::new(MixStats::default());
    (
        MixHandle {
            tx,
            stats: stats.clone(),
        },
        MixDriver {
            mixer: Mixer::new(out_rate),
            rx,
            loops: [None; LOOP_SLOTS],
            music: None,
            stats,
        },
    )
}

impl MixDriver {
    pub fn sample_rate(&self) -> u32 {
        self.mixer.sample_rate()
    }

    /// The output device changed its rate; playing voices carry on.
    pub fn set_sample_rate(&mut self, rate: u32) {
        self.mixer.set_sample_rate(rate);
    }

    pub fn stats(&self) -> &Arc<MixStats> {
        &self.stats
    }

    fn apply(&mut self, msg: MixMsg) {
        let m = &mut self.mixer;
        match msg {
            MixMsg::Play {
                sound,
                bus,
                volume,
                pan,
                rate,
                delay_seconds,
            } => {
                let delay_frames = (delay_seconds.max(0.0) * m.sample_rate() as f32) as u32;
                m.play(
                    sound,
                    PlayParams {
                        bus,
                        volume,
                        pan,
                        rate,
                        looping: false,
                        delay_frames,
                    },
                );
            }
            MixMsg::LoopStart {
                slot,
                sound,
                bus,
                volume,
                rate,
            } => {
                let Some(entry) = self.loops.get_mut(slot as usize) else {
                    return;
                };
                if let Some(old) = entry.take() {
                    m.stop(old);
                }
                *entry = Some(m.play(
                    sound,
                    PlayParams {
                        bus,
                        volume,
                        rate,
                        looping: true,
                        ..PlayParams::default()
                    },
                ));
            }
            MixMsg::LoopSet { slot, volume, rate } => {
                if let Some(Some(id)) = self.loops.get(slot as usize) {
                    m.ramp_volume(*id, volume, LOOP_RAMP_SECONDS);
                    m.set_rate(*id, rate);
                }
            }
            MixMsg::LoopStop { slot } => {
                if let Some(id) = self.loops.get_mut(slot as usize).and_then(Option::take) {
                    // A few milliseconds of fade instead of a click.
                    m.fade_out(id, 0.01);
                }
            }
            MixMsg::Music {
                intro,
                body,
                volume,
            } => {
                if let Some(old) = self.music.take() {
                    m.stop(old);
                }
                self.music = Some(m.play_music(intro, body, volume));
            }
            MixMsg::MusicStop { fade_seconds } => {
                if let Some(id) = self.music.take() {
                    if fade_seconds > 0.0 {
                        m.fade_out(id, fade_seconds);
                    } else {
                        m.stop(id);
                    }
                }
            }
            MixMsg::BusVolume { bus, volume } => m.set_bus_volume(bus, volume),
            MixMsg::MasterVolume(v) => m.set_master_volume(v),
            MixMsg::StopBus(bus) => {
                m.stop_bus(bus);
                if bus == Bus::Music {
                    self.music = None;
                } else {
                    for slot in &mut self.loops {
                        if slot.is_some_and(|id| !m.is_playing(id)) {
                            *slot = None;
                        }
                    }
                }
            }
            MixMsg::Pause(p) => m.set_paused(p),
        }
    }

    /// Apply everything queued, then render `out.len() / 2` interleaved stereo frames, clamped to [-1, 1].
    pub fn process(&mut self, out: &mut [f32]) {
        let mut applied = 0u64;
        while let Ok(msg) = self.rx.try_recv() {
            self.apply(msg);
            applied += 1;
        }
        self.mixer.mix(out);

        let mut peak = 0f32;
        let mut energy = 0f64;
        let mut clipped = 0u64;
        for s in out.iter_mut() {
            let a = s.abs();
            peak = peak.max(a);
            energy += f64::from(a) * f64::from(a);
            if a > 1.0 {
                clipped += 1;
                *s = s.clamp(-1.0, 1.0);
            }
        }
        let st = &self.stats;
        st.callbacks.fetch_add(1, Relaxed);
        st.frames.fetch_add(out.len() as u64 / 2, Relaxed);
        st.peak_bits.fetch_max(peak.to_bits(), Relaxed);
        st.energy
            .fetch_add((energy * (1u64 << 24) as f64) as u64, Relaxed);
        st.clipped.fetch_add(clipped, Relaxed);
        st.messages.fetch_add(applied, Relaxed);
        let voices = self.mixer.active_voices() as u32;
        st.voices.store(voices, Relaxed);
        st.max_voices.fetch_max(voices, Relaxed);
    }

    /// Keep time without an output device: apply queued messages and advance `frames` frames, discarding
    /// the audio, so one-shots end and the music is where it should be when a device appears.
    pub fn advance(&mut self, frames: usize, scratch: &mut [f32]) {
        let mut left = frames;
        let chunk = scratch.len() / 2;
        if chunk == 0 {
            return;
        }
        while left > 0 {
            let n = left.min(chunk);
            self.process(&mut scratch[..n * 2]);
            left -= n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, frames: usize, level: i16) -> Arc<Sound> {
        Arc::new(Sound::mono(rate, vec![level; frames], None))
    }

    fn render(d: &mut MixDriver, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        d.process(&mut out);
        out
    }

    #[test]
    fn messages_are_applied_in_order_before_rendering() {
        let (h, mut d) = mix_channel(1000);
        assert!(h.send(MixMsg::BusVolume {
            bus: Bus::Sfx,
            volume: 0.5
        }));
        assert!(h.send(MixMsg::Play {
            sound: tone(1000, 4, 16384),
            bus: Bus::Sfx,
            volume: 1.0,
            pan: 0.0,
            rate: 1.0,
            delay_seconds: 0.002,
        }));
        let out = render(&mut d, 8);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(l, vec![0.0, 0.0, 0.25, 0.25, 0.25, 0.25, 0.0, 0.0]);
        let s = h.stats().snapshot();
        assert_eq!((s.callbacks, s.frames, s.messages, s.dropped), (1, 8, 2, 0));
        assert_eq!(s.peak, 0.25);
        assert!((s.rms - (0.25f32 * 0.25 * 8.0 / 16.0).sqrt()).abs() < 1e-4);
        assert_eq!(s.clipped, 0);
    }

    #[test]
    fn loops_follow_their_slot() {
        let (h, mut d) = mix_channel(1000);
        h.send(MixMsg::LoopStart {
            slot: 1,
            sound: tone(1000, 10, 8192),
            bus: Bus::Sfx,
            volume: 1.0,
            rate: 1.0,
        });
        assert!(render(&mut d, 50).iter().all(|&x| x == 0.25));
        // A live update glides (30 ms = 30 frames here).
        h.send(MixMsg::LoopSet {
            slot: 1,
            volume: 0.0,
            rate: 2.0,
        });
        let out = render(&mut d, 40);
        assert!(out[0] < 0.25 && out[0] > 0.2 && out[70] == 0.0);
        assert_eq!(h.stats().snapshot().voices, 1);
        // Restarting a slot replaces its voice; stopping fades it out and frees the slot.
        h.send(MixMsg::LoopStart {
            slot: 1,
            sound: tone(1000, 10, 16384),
            bus: Bus::Sfx,
            volume: 1.0,
            rate: 1.0,
        });
        assert_eq!(render(&mut d, 1)[0], 0.5);
        assert_eq!(h.stats().snapshot().voices, 1);
        h.send(MixMsg::LoopStop { slot: 1 });
        render(&mut d, 20);
        assert_eq!(h.stats().snapshot().voices, 0);
        // Unknown slots are ignored.
        h.send(MixMsg::LoopStop { slot: 200 });
        h.send(MixMsg::LoopSet {
            slot: 200,
            volume: 1.0,
            rate: 1.0,
        });
        assert!(render(&mut d, 4).iter().all(|&x| x == 0.0));
    }

    #[test]
    fn music_replaces_stops_and_survives_sfx_stop() {
        let (h, mut d) = mix_channel(1000);
        h.send(MixMsg::Music {
            intro: Some(tone(1000, 3, 8192)),
            body: tone(1000, 2, 16384),
            volume: 1.0,
        });
        let out = render(&mut d, 6);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(l, vec![0.25, 0.25, 0.25, 0.5, 0.5, 0.5]);
        h.send(MixMsg::StopBus(Bus::Sfx));
        assert_eq!(render(&mut d, 1)[0], 0.5);
        h.send(MixMsg::Music {
            intro: None,
            body: tone(1000, 2, 4096),
            volume: 1.0,
        });
        assert_eq!(render(&mut d, 1)[0], 0.125);
        assert_eq!(h.stats().snapshot().voices, 1);
        h.send(MixMsg::Pause(true));
        assert_eq!(render(&mut d, 4), vec![0.0; 8]);
        h.send(MixMsg::Pause(false));
        h.send(MixMsg::MusicStop { fade_seconds: 0.0 });
        assert_eq!(render(&mut d, 1)[0], 0.0);
        assert_eq!(h.stats().snapshot().voices, 0);
    }

    #[test]
    fn output_is_clamped_and_clipping_is_counted() {
        let (h, mut d) = mix_channel(1000);
        for _ in 0..3 {
            h.send(MixMsg::Play {
                sound: tone(1000, 2, 20000),
                bus: Bus::Sfx,
                volume: 1.0,
                pan: 0.0,
                rate: 1.0,
                delay_seconds: 0.0,
            });
        }
        let out = render(&mut d, 2);
        assert_eq!(out, vec![1.0; 4]);
        let s = h.stats().snapshot();
        assert_eq!(s.clipped, 4);
        assert!(s.peak > 1.8);
    }

    #[test]
    fn a_full_queue_drops_and_counts_instead_of_blocking() {
        let (h, mut d) = mix_channel(1000);
        let mut sent = 0;
        for _ in 0..QUEUE_CAPACITY + 10 {
            if h.send(MixMsg::MasterVolume(1.0)) {
                sent += 1;
            }
        }
        assert_eq!(sent, QUEUE_CAPACITY);
        assert_eq!(h.stats().snapshot().dropped, 10);
        render(&mut d, 1);
        assert_eq!(h.stats().snapshot().messages, QUEUE_CAPACITY as u64);
        // With the driver gone, sending still returns at once.
        drop(d);
        assert!(!h.send(MixMsg::MasterVolume(1.0)));
    }

    #[test]
    fn advance_keeps_time_without_a_device() {
        let (h, mut d) = mix_channel(1000);
        h.send(MixMsg::Play {
            sound: tone(1000, 100, 8192),
            bus: Bus::Sfx,
            volume: 1.0,
            pan: 0.0,
            rate: 1.0,
            delay_seconds: 0.0,
        });
        let mut scratch = [0.0; 32];
        d.advance(90, &mut scratch);
        assert_eq!(h.stats().snapshot().voices, 1);
        d.advance(20, &mut scratch);
        assert_eq!(h.stats().snapshot().voices, 0);
        d.set_sample_rate(2000);
        assert_eq!(d.sample_rate(), 2000);
    }
}
