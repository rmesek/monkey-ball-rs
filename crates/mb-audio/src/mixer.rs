//! A small software mixer: plays decoded buffers with volume, pan, pitch, looping and intro-to-loop
//! handoff, and renders interleaved stereo `f32` for any output backend.
//!
//! Playback resamples with linear interpolation from each sound's rate to the mixer's output rate.
//! Pan is a balance law (centre = unity on both sides, like a gain-only Web Audio path); it attenuates the
//! far side only. A voice is up to two segments; when a non-looping segment ends the next one starts on
//! the exact output frame (sub-sample accurate), which is how `*_int` + `*_lp` music is chained.

use crate::adp::AdpAudio;
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

    /// Like [`Sound::from_adp`] without copying the samples.
    pub fn from_adp_owned(adp: AdpAudio) -> Self {
        Self::stereo(adp.sample_rate, adp.samples, None)
    }

    /// Bytes of PCM held.
    pub fn byte_len(&self) -> usize {
        self.data.len() * 2
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

/// At most an intro and a body.
const MAX_SEGS: usize = 2;

struct Voice {
    id: VoiceId,
    bus: Bus,
    /// Current gain; moves towards `vol_target` by `vol_step` per output frame.
    volume: f32,
    vol_target: f32,
    vol_step: f32,
    /// Remove the voice once a fade reaches zero.
    stop_at_zero: bool,
    pan: f32,
    rate: f32,
    delay: u32,
    segs: [Option<Segment>; MAX_SEGS],
    /// Index of the segment being played; `MAX_SEGS` when finished.
    cur: usize,
    /// Position in source frames within the current segment.
    pos: f64,
}

impl Voice {
    fn front(&self) -> Option<&Segment> {
        self.segs.get(self.cur).and_then(Option::as_ref)
    }

    fn finished(&self) -> bool {
        self.front().is_none() || (self.stop_at_zero && self.volume <= 0.0)
    }

    /// Source frames advanced per output frame for the current segment.
    fn step(&self, out_rate: u32) -> f64 {
        match self.front() {
            Some(s) => f64::from(s.sound.sample_rate) / f64::from(out_rate) * f64::from(self.rate),
            None => 0.0,
        }
    }

    /// Interpolated stereo sample at the current position, or `None` if finished.
    fn sample(&self) -> Option<(f32, f32)> {
        let seg = self.front()?;
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
        } else if let Some(next) = self.segs.get(self.cur + 1).and_then(Option::as_ref) {
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
            let Some(seg) = self.front() else { return };
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
            self.cur += 1;
            if self.front().is_none() {
                return;
            }
            self.pos = over_out * self.step(out_rate);
        }
    }

    /// Move the gain one frame along its ramp; true when it changed.
    fn ramp(&mut self) -> bool {
        if self.volume == self.vol_target {
            return false;
        }
        let d = self.vol_target - self.volume;
        if d.abs() <= self.vol_step || self.vol_step <= 0.0 {
            self.volume = self.vol_target;
        } else {
            self.volume += self.vol_step.copysign(d);
        }
        true
    }
}

fn loop_bounds(seg: &Segment) -> (usize, usize) {
    let frames = seg.sound.frames();
    match seg.sound.loop_range {
        Some((s, e)) if e > s && e <= frames => (s, e),
        _ => (0, frames.max(1)),
    }
}

/// Default voice limit; the voice list is allocated once for this many.
pub const DEFAULT_MAX_VOICES: usize = 64;

/// The mixer. Not thread-safe by itself; the output thread owns it and is fed through a queue.
///
/// [`Mixer::mix`] and every method that starts, stops or changes a voice are allocation-free: the voice list
/// is reserved up front and a voice holds its (at most two) segments inline. Removing a voice drops its
/// `Arc<Sound>`s, so whoever queued a sound must keep its own reference if freeing the buffer on the
/// output thread is to be avoided.
pub struct Mixer {
    out_rate: u32,
    master: f32,
    bus_gain: [f32; 3],
    max_voices: usize,
    voices: Vec<Voice>,
    next_id: u64,
    paused: bool,
}

impl Mixer {
    pub fn new(out_rate: u32) -> Self {
        Self {
            out_rate: out_rate.max(1),
            master: 1.0,
            bus_gain: [1.0; 3],
            max_voices: DEFAULT_MAX_VOICES,
            voices: Vec::with_capacity(DEFAULT_MAX_VOICES),
            next_id: 1,
            paused: false,
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.out_rate
    }

    /// Change the output rate (the device was reopened). Voices keep their place: positions are kept in
    /// source frames.
    pub fn set_sample_rate(&mut self, out_rate: u32) {
        self.out_rate = out_rate.max(1);
    }

    pub fn set_master_volume(&mut self, v: f32) {
        self.master = v.max(0.0);
    }

    pub fn set_bus_volume(&mut self, bus: Bus, v: f32) {
        self.bus_gain[bus as usize] = v.max(0.0);
    }

    /// Voice limit. Raising it above the current capacity reallocates, so do that before playback starts.
    pub fn set_max_voices(&mut self, n: usize) {
        self.max_voices = n.max(1);
        if self.voices.capacity() < self.max_voices {
            self.voices.reserve(self.max_voices - self.voices.len());
        }
    }

    pub fn active_voices(&self) -> usize {
        self.voices.len()
    }

    /// While paused [`Mixer::mix`] writes silence and nothing advances.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Play one sound.
    pub fn play(&mut self, sound: Arc<Sound>, p: PlayParams) -> VoiceId {
        self.spawn(
            [
                Some(Segment {
                    sound,
                    looping: p.looping,
                }),
                None,
            ],
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
        let body = Some(Segment {
            sound: body,
            looping: true,
        });
        let segs = match intro {
            Some(i) => [
                Some(Segment {
                    sound: i,
                    looping: false,
                }),
                body,
            ],
            None => [body, None],
        };
        self.spawn(
            segs,
            PlayParams {
                bus: Bus::Music,
                volume,
                ..PlayParams::default()
            },
        )
    }

    fn spawn(&mut self, segs: [Option<Segment>; MAX_SEGS], p: PlayParams) -> VoiceId {
        if self.voices.len() >= self.max_voices {
            // Steal the oldest non-music voice.
            if let Some(i) = self.voices.iter().position(|v| v.bus != Bus::Music) {
                self.voices.remove(i);
            }
        }
        let id = VoiceId(self.next_id);
        self.next_id += 1;
        let volume = p.volume.max(0.0);
        self.voices.push(Voice {
            id,
            bus: p.bus,
            volume,
            vol_target: volume,
            vol_step: 0.0,
            stop_at_zero: false,
            pan: p.pan.clamp(-1.0, 1.0),
            rate: p.rate.max(0.0),
            delay: p.delay_frames,
            segs,
            cur: 0,
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

    /// Set the gain at once.
    pub fn set_volume(&mut self, id: VoiceId, volume: f32) {
        if let Some(v) = self.voice_mut(id) {
            v.volume = volume.max(0.0);
            v.vol_target = v.volume;
        }
    }

    /// Glide the gain to `volume` over `seconds` (no zipper noise on live updates).
    pub fn ramp_volume(&mut self, id: VoiceId, volume: f32, seconds: f32) {
        let frames = (seconds.max(0.0) * self.out_rate as f32).max(1.0);
        if let Some(v) = self.voice_mut(id) {
            v.vol_target = volume.max(0.0);
            v.vol_step = (v.vol_target - v.volume).abs() / frames;
        }
    }

    /// Fade a voice out over `seconds` and remove it.
    pub fn fade_out(&mut self, id: VoiceId, seconds: f32) {
        self.ramp_volume(id, 0.0, seconds);
        if let Some(v) = self.voice_mut(id) {
            v.stop_at_zero = true;
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
        if self.paused {
            return;
        }
        let out_rate = self.out_rate;
        let master = self.master;
        let bus_gain = self.bus_gain;
        for v in &mut self.voices {
            let bus = bus_gain[v.bus as usize] * master / 32768.0;
            let pan_l = 1.0 - v.pan.max(0.0);
            let pan_r = 1.0 + v.pan.min(0.0);
            let mut gl = pan_l * v.volume * bus;
            let mut gr = pan_r * v.volume * bus;
            // A muted bus only keeps time: no sample is read or interpolated.
            let silent = bus == 0.0;
            for frame in out.as_chunks_mut::<2>().0 {
                if v.delay > 0 {
                    v.delay -= 1;
                    continue;
                }
                if v.ramp() {
                    gl = pan_l * v.volume * bus;
                    gr = pan_r * v.volume * bus;
                }
                if !silent {
                    let Some((l, r)) = v.sample() else { break };
                    frame[0] += l * gl;
                    frame[1] += r * gr;
                }
                v.advance(out_rate);
                if v.finished() {
                    break;
                }
            }
        }
        self.voices.retain(|v| !v.finished());
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

    #[test]
    fn ramps_fades_pause_and_muted_bus_keep_time() {
        let mut m = Mixer::new(1000);
        let s = Arc::new(Sound::mono(1000, vec![16384; 1000], None));
        let id = m.play(
            s.clone(),
            PlayParams {
                looping: true,
                ..Default::default()
            },
        );
        // 10 ms ramp to zero gain = 10 frames of equal steps.
        m.ramp_volume(id, 0.0, 0.01);
        let out = run(&mut m, 12);
        let l: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!((l[0] - 0.45).abs() < 1e-6 && (l[4] - 0.25).abs() < 1e-6);
        assert_eq!(&l[9..], &[0.0, 0.0, 0.0]);
        assert!(m.is_playing(id), "a ramp alone does not stop the voice");
        m.set_volume(id, 1.0);
        m.fade_out(id, 0.004);
        let out = run(&mut m, 6);
        assert!(out[0] > 0.0 && out[10] == 0.0);
        assert!(!m.is_playing(id), "a fade removes the voice at zero");

        // Paused: silence, nothing advances.
        let ramp = Arc::new(Sound::mono(1000, (0..100).map(|i| i * 300).collect(), None));
        let id = m.play(ramp, PlayParams::default());
        run(&mut m, 2);
        m.set_paused(true);
        assert!(run(&mut m, 50).iter().all(|&x| x == 0.0));
        m.set_paused(false);
        let out = run(&mut m, 1);
        assert!((out[0] - 600.0 / 32768.0).abs() < 1e-6);
        // Muted bus: silent, but the voice keeps its place in time.
        m.set_bus_volume(Bus::Sfx, 0.0);
        assert!(run(&mut m, 10).iter().all(|&x| x == 0.0));
        m.set_bus_volume(Bus::Sfx, 1.0);
        let out = run(&mut m, 1);
        assert!((out[0] - 13.0 * 300.0 / 32768.0).abs() < 1e-6);
        run(&mut m, 200);
        assert!(!m.is_playing(id));
    }

    #[test]
    fn output_rate_can_change_mid_voice() {
        let mut m = Mixer::new(1000);
        let s = Arc::new(Sound::mono(1000, (0..100).map(|i| i * 100).collect(), None));
        m.play(s, PlayParams::default());
        run(&mut m, 10);
        m.set_sample_rate(2000);
        let out = run(&mut m, 2);
        assert!((out[0] - 1000.0 / 32768.0).abs() < 1e-6);
        assert!((out[2] - 1050.0 / 32768.0).abs() < 1e-6);
    }

    #[test]
    fn voice_list_never_reallocates() {
        let mut m = Mixer::new(1000);
        let cap = m.voices.capacity();
        let s = Arc::new(Sound::mono(1000, vec![1; 4], None));
        for _ in 0..(DEFAULT_MAX_VOICES * 3) {
            m.play(s.clone(), PlayParams::default());
        }
        assert_eq!(m.active_voices(), DEFAULT_MAX_VOICES);
        assert_eq!(m.voices.capacity(), cap);
    }
}
