//! A small software mixer: plays decoded buffers with volume, pan, pitch, looping and intro-to-loop
//! handoff, and renders interleaved stereo `f32` for any output backend.
//!
//! Playback resamples with linear interpolation from each sound's rate to the mixer's output rate.
//! Pan is a balance law (centre = unity on both sides, like a gain-only Web Audio path); it attenuates the
//! far side only. A voice is a queue of segments; when a non-looping segment ends the next one starts on
//! the exact output frame (sub-sample accurate), which is how `*_int` + `*_lp` music is chained.

use crate::adp::AdpAudio;
use std::collections::VecDeque;
use std::sync::Arc;

/// Immutable PCM buffer shared between voices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sound {
    pub sample_rate: u32,
    /// 1 or 2.
    pub channels: usize,
    /// Interleaved.
    pub data: Vec<i16>,
    /// Loop range in frames (`start..end`); used when a voice plays with `looping`.
    /// `None` loops the whole buffer.
    pub loop_range: Option<(usize, usize)>,
}

impl Sound {
    pub fn mono(sample_rate: u32, pcm: Vec<i16>, loop_range: Option<(usize, usize)>) -> Self {
        Self {
            sample_rate,
            channels: 1,
            data: pcm,
            loop_range,
        }
    }

    pub fn stereo(
        sample_rate: u32,
        interleaved: Vec<i16>,
        loop_range: Option<(usize, usize)>,
    ) -> Self {
        Self {
            sample_rate,
            channels: 2,
            data: interleaved,
            loop_range,
        }
    }

    pub fn from_adp(adp: &AdpAudio) -> Self {
        Self::stereo(adp.sample_rate, adp.samples.clone(), None)
    }

    pub fn frames(&self) -> usize {
        self.data.len() / self.channels
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }

    fn frame(&self, i: usize) -> (f32, f32) {
        if self.channels == 1 {
            let s = f32::from(self.data[i]);
            (s, s)
        } else {
            (f32::from(self.data[i * 2]), f32::from(self.data[i * 2 + 1]))
        }
    }
}

/// Independent gain groups (the reference has separate music/sfx/announcer volumes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bus {
    Music = 0,
    Sfx = 1,
    Announcer = 2,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayParams {
    pub bus: Bus,
    pub volume: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
    /// Playback-rate multiplier (pitch).
    pub rate: f32,
    pub looping: bool,
    /// Start this many output frames from now.
    pub delay_frames: u32,
}

impl Default for PlayParams {
    fn default() -> Self {
        Self {
            bus: Bus::Sfx,
            volume: 1.0,
            pan: 0.0,
            rate: 1.0,
            looping: false,
            delay_frames: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VoiceId(u64);

struct Segment {
    sound: Arc<Sound>,
    looping: bool,
}

struct Voice {
    id: VoiceId,
    bus: Bus,
    volume: f32,
    pan: f32,
    rate: f32,
    delay: u32,
    segs: VecDeque<Segment>,
    /// Position in source frames within the front segment.
    pos: f64,
}

impl Voice {
    /// Source frames advanced per output frame for the front segment.
    fn step(&self, out_rate: u32) -> f64 {
        match self.segs.front() {
            Some(s) => f64::from(s.sound.sample_rate) / f64::from(out_rate) * f64::from(self.rate),
            None => 0.0,
        }
    }

    /// Interpolated stereo sample at the current position, or `None` if finished.
    fn sample(&self) -> Option<(f32, f32)> {
        let seg = self.segs.front()?;
        let snd = &seg.sound;
        let frames = snd.frames();
        if frames == 0 {
            return Some((0.0, 0.0));
        }
        let i = (self.pos as usize).min(frames - 1);
        let frac = (self.pos - i as f64) as f32;
        let a = snd.frame(i);
        let (ls, le) = loop_bounds(seg);
        let b = if seg.looping && i + 1 >= le {
            snd.frame(ls)
        } else if i + 1 < frames {
            snd.frame(i + 1)
        } else if let Some(next) = self.segs.get(1) {
            if next.sound.frames() > 0 {
                next.sound.frame(0)
            } else {
                a
            }
        } else {
            a
        };
        Some((a.0 + (b.0 - a.0) * frac, a.1 + (b.1 - a.1) * frac))
    }

    fn advance(&mut self, out_rate: u32) {
        let step = self.step(out_rate);
        self.pos += step;
        loop {
            let Some(seg) = self.segs.front() else { return };
            let frames = seg.sound.frames();
            if seg.looping {
                let (ls, le) = loop_bounds(seg);
                if self.pos >= le as f64 {
                    self.pos = ls as f64 + (self.pos - le as f64) % ((le - ls) as f64);
                }
                return;
            }
            if self.pos < frames as f64 {
                return;
            }
            // Hand the leftover (in output time) to the next segment.
            let over_out = if step > 0.0 {
                (self.pos - frames as f64) / step
            } else {
                0.0
            };
            self.segs.pop_front();
            if self.segs.is_empty() {
                return;
            }
            self.pos = over_out * self.step(out_rate);
        }
    }
}

fn loop_bounds(seg: &Segment) -> (usize, usize) {
    let frames = seg.sound.frames();
    match seg.sound.loop_range {
        Some((s, e)) if e > s && e <= frames => (s, e),
        _ => (0, frames.max(1)),
    }
}

/// The mixer. Not thread-safe by itself; wrap in a mutex if the output callback runs on another thread.
pub struct Mixer {
    out_rate: u32,
    master: f32,
    bus_gain: [f32; 3],
    max_voices: usize,
    voices: Vec<Voice>,
    next_id: u64,
}

impl Mixer {
    pub fn new(out_rate: u32) -> Self {
        Self {
            out_rate,
            master: 1.0,
            bus_gain: [1.0; 3],
            max_voices: 64,
            voices: Vec::new(),
            next_id: 1,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn set_master_volume(&mut self, v: f32) {
        self.master = v.max(0.0);
    }

    pub fn set_bus_volume(&mut self, bus: Bus, v: f32) {
        self.bus_gain[bus as usize] = v.max(0.0);
    }

    pub fn set_max_voices(&mut self, n: usize) {
        self.max_voices = n.max(1);
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// Play one sound.
    pub fn play(&mut self, sound: Arc<Sound>, p: PlayParams) -> VoiceId {
        self.spawn(
            vec![Segment {
                sound,
                looping: p.looping,
            }],
            p,
        )
    }

    /// Play an optional intro once, then loop `body` forever, with no gap between them.
    pub fn play_music(
        &mut self,
        intro: Option<Arc<Sound>>,
        body: Arc<Sound>,
        volume: f32,
    ) -> VoiceId {
        let mut segs = Vec::new();
        if let Some(i) = intro {
            segs.push(Segment {
                sound: i,
                looping: false,
            });
        }
        segs.push(Segment {
            sound: body,
            looping: true,
        });
        self.spawn(
            segs,
            PlayParams {
                bus: Bus::Music,
                volume,
                ..PlayParams::default()
            },
        )
    }

    fn spawn(&mut self, segs: Vec<Segment>, p: PlayParams) -> VoiceId {
        if self.voices.len() >= self.max_voices {
            // Steal the oldest non-music voice.
            if let Some(i) = self.voices.iter().position(|v| v.bus != Bus::Music) {
                self.voices.remove(i);
            }
        }
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        self.voices.push(Voice {
            id,
            bus: p.bus,
            volume: p.volume.max(0.0),
            pan: p.pan.clamp(-1.0, 1.0),
            rate: p.rate.max(0.0),
            delay: p.delay_frames,
            segs: segs.into(),
            pos: 0.0,
        });
        id
    }

    fn voice_mut(&mut self, id: VoiceId) -> Option<&mut Voice> {
        self.voices.iter_mut().find(|v| v.id == id)
    }

    pub fn is_playing(&self, id: VoiceId) -> bool {
        self.voices.iter().any(|v| v.id == id)
    }

    pub fn stop(&mut self, id: VoiceId) {
        self.voices.retain(|v| v.id != id);
    }

    pub fn stop_bus(&mut self, bus: Bus) {
        self.voices.retain(|v| v.bus != bus);
    }

    pub fn stop_all(&mut self) {
        self.voices.clear();
    }

    pub fn set_volume(&mut self, id: VoiceId, volume: f32) {
        if let Some(v) = self.voice_mut(id) {
            v.volume = volume.max(0.0);
        }
    }

    pub fn set_pan(&mut self, id: VoiceId, pan: f32) {
        if let Some(v) = self.voice_mut(id) {
            v.pan = pan.clamp(-1.0, 1.0);
        }
    }

    pub fn set_rate(&mut self, id: VoiceId, rate: f32) {
        if let Some(v) = self.voice_mut(id) {
            v.rate = rate.max(0.0);
        }
    }

    /// Render `out.len() / 2` stereo frames (interleaved L,R), overwriting `out`. Output is not clamped.
    pub fn mix(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let out_rate = self.out_rate;
        let master = self.master;
        let bus_gain = self.bus_gain;
        for v in &mut self.voices {
            let gain = v.volume * bus_gain[v.bus as usize] * master / 32768.0;
            let gl = (1.0 - v.pan.max(0.0)) * gain;
            let gr = (1.0 + v.pan.min(0.0)) * gain;
            for frame in out.as_chunks_mut::<2>().0 {
                if v.delay > 0 {
                    v.delay -= 1;
                    continue;
                }
                let Some((l, r)) = v.sample() else { break };
                frame[0] += l * gl;
                frame[1] += r * gr;
                v.advance(out_rate);
                if v.segs.is_empty() {
                    break;
                }
            }
        }
        self.voices.retain(|v| !v.segs.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        m.mix(&mut out);
        out
    }

    fn ramp(n: usize, scale: i16) -> Arc<Sound> {
        Arc::new(Sound::mono(
            1000,
            (0..n as i16).map(|i| i * scale).collect(),
            None,
        ))
    }

    #[test]
    fn plays_once_at_unity_and_finishes() {
        let mut m = Mixer::new(1000);
        let id = m.play(ramp(4, 8192), PlayParams::default());
        let out = run(&mut m, 6);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(l, vec![0.0, 0.25, 0.5, 0.75, 0.0, 0.0]);
        assert_eq!(out[1], 0.0);
        assert_eq!(out[3], 0.25); // mono: R == L at centre
        assert!(!m.is_playing(id));
    }

    #[test]
    fn volume_pan_and_buses() {
        let mut m = Mixer::new(1000);
        let s = Arc::new(Sound::mono(1000, vec![16384; 4], None));
        m.play(
            s.clone(),
            PlayParams {
                volume: 0.5,
                pan: 1.0,
                ..Default::default()
            },
        );
        let out = run(&mut m, 1);
        assert_eq!(out, vec![0.0, 0.25]);
        m.set_bus_volume(Bus::Sfx, 0.5);
        m.set_master_volume(0.5);
        m.play(s, PlayParams::default());
        let out = run(&mut m, 1);
        // New voice 0.5*0.5*0.5 both sides, plus the first voice's second frame on the right: 0.25*0.5*0.5.
        assert_eq!(out, vec![0.125, 0.1875]);
    }

    #[test]
    fn delay_and_pitch() {
        let mut m = Mixer::new(1000);
        m.play(
            ramp(8, 4096),
            PlayParams {
                delay_frames: 2,
                rate: 2.0,
                ..Default::default()
            },
        );
        let out = run(&mut m, 5);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        // Double speed: source frames 0,2,4,6 => 0, 0.25, 0.5, 0.75.
        assert_eq!(l, vec![0.0, 0.0, 0.0, 0.25, 0.5]);
    }

    #[test]
    fn resamples_with_linear_interpolation() {
        let mut m = Mixer::new(2000);
        m.play(
            Arc::new(Sound::mono(1000, vec![0, 16384, 32767 - 16383], None)),
            PlayParams::default(),
        );
        let out = run(&mut m, 3);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(l[0], 0.0);
        assert!((l[1] - 0.25).abs() < 1e-6);
        assert!((l[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn looping_honours_loop_range() {
        let mut m = Mixer::new(1000);
        // frames 0..6 = 0,1,2,3,4,5 (x 8192/32768 = 0.25 each); loop 2..5 => 0 1 2 3 4 2 3 4 2
        let s = Arc::new(Sound::mono(
            1000,
            vec![0, 8192, 16384, 24576, 0, 0],
            Some((2, 5)),
        ));
        m.play(
            s,
            PlayParams {
                looping: true,
                ..Default::default()
            },
        );
        let out = run(&mut m, 9);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(l, vec![0.0, 0.25, 0.5, 0.75, 0.0, 0.5, 0.75, 0.0, 0.5]);
        assert_eq!(m.active_voices(), 1);
        m.stop_all();
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn intro_hands_off_to_loop_without_gap() {
        let mut m = Mixer::new(1000);
        let intro = Arc::new(Sound::mono(1000, vec![8192; 3], None));
        let body = Arc::new(Sound::mono(1000, vec![16384, 24576], None));
        let id = m.play_music(Some(intro), body, 1.0);
        let out = run(&mut m, 9);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(l, vec![0.25, 0.25, 0.25, 0.5, 0.75, 0.5, 0.75, 0.5, 0.75]);
        assert!(m.is_playing(id));
        // Music bus is independent.
        m.set_bus_volume(Bus::Music, 0.0);
        assert!(run(&mut m, 2).iter().all(|&x| x == 0.0));
        m.stop_bus(Bus::Music);
        assert_eq!(m.active_voices(), 0);
    }

    #[test]
    fn handoff_is_subsample_accurate_when_resampling() {
        // intro 2 frames at 1000 Hz into 3000 Hz output = 6 output frames, then loop starts exactly there.
        let mut m = Mixer::new(3000);
        let intro = Arc::new(Sound::mono(1000, vec![0, 0], None));
        let body = Arc::new(Sound::mono(1000, vec![32767; 2], None));
        m.play_music(Some(intro), body, 1.0);
        let out = run(&mut m, 8);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        // Linear interpolation runs from the intro's last frame towards the body's first one.
        let want = [0.0, 0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0];
        for (g, w) in l.iter().zip(want) {
            assert!((g - w).abs() < 1e-3, "{l:?}");
        }
    }

    #[test]
    fn stereo_sound_balance_and_voice_stealing() {
        let mut m = Mixer::new(1000);
        m.set_max_voices(2);
        let st = Arc::new(Sound::stereo(
            1000,
            vec![16384, -16384, 16384, -16384],
            None,
        ));
        let a = m.play(st.clone(), PlayParams::default());
        let out = run(&mut m, 1);
        assert_eq!(out, vec![0.5, -0.5]);
        m.set_pan(a, -1.0);
        m.set_volume(a, 2.0);
        let out = run(&mut m, 1);
        assert_eq!(out, vec![1.0, 0.0]);
        let b = m.play(st.clone(), PlayParams::default());
        let c = m.play(st, PlayParams::default());
        assert!(!m.is_playing(a) && m.is_playing(b) && m.is_playing(c));
    }
}
