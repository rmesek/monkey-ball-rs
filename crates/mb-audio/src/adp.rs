//! GameCube DTK ("ADP") streaming ADPCM, as used by `test/snd/adp/*.adp`.
//!
//! The stream is a sequence of 32-byte frames, always stereo:
//!
//! ```text
//! 0      1      2      3      4 .. 31
//! hdrL   hdrR   hdrL   hdrR   28 data bytes: low nibble = left sample n, high nibble = right sample n
//! ```
//!
//! A header byte holds `filter = hdr >> 4` (0..=3) and `range = hdr & 15`. One frame yields 28 samples per
//! channel. This is the CD-XA ADPCM scheme: `s = (nibble << 12) >> range`, plus a two-tap predictor
//! `(c0*h1 + c1*h2 + 32) >> 6` with the fixed pairs `(0,0) (60,0) (115,-52) (98,-55)`, evaluated on a
//! 6-bit-fractional history (see the comment in the decoder). History carries across frames. Files are padded with all-zero frames to a 32 KiB multiple; [`decode_adp`] drops that tail.

use crate::{AudioError, Result};

/// Bytes per DTK frame.
pub const ADP_FRAME_BYTES: usize = 32;
/// Samples per channel per frame.
pub const ADP_FRAME_SAMPLES: usize = 28;
/// Nominal sample rate (the DVD audio stream runs at the 48 kHz AI rate; real hardware is ~48043 Hz).
pub const ADP_SAMPLE_RATE: u32 = 48_000;

const FILTERS: [(i64, i64); 4] = [(0, 0), (60, 0), (115, -52), (98, -55)];

/// Decoded stereo stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdpAudio {
    pub sample_rate: u32,
    /// Interleaved L,R.
    pub samples: Vec<i16>,
}

impl AdpAudio {
    pub fn channels(&self) -> u16 {
        2
    }

    /// Stereo frames (sample pairs).
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }
}

/// Decode a whole `.adp` file, dropping trailing all-zero padding frames.
pub fn decode_adp(data: &[u8]) -> Result<AdpAudio> {
    decode_adp_opts(data, true)
}

/// Like [`decode_adp`], optionally keeping the zero padding frames.
pub fn decode_adp_opts(data: &[u8], trim_padding: bool) -> Result<AdpAudio> {
    let frames = adp_frame_count(data, trim_padding)?;
    let mut out = Vec::with_capacity(frames * ADP_FRAME_SAMPLES * 2);
    AdpDecoder::new().decode(&data[..frames * ADP_FRAME_BYTES], &mut out);
    Ok(AdpAudio {
        sample_rate: ADP_SAMPLE_RATE,
        samples: out,
    })
}

/// Number of 32-byte frames in a stream, without the trailing all-zero padding frames when `trim_padding`.
/// Each frame decodes to [`ADP_FRAME_SAMPLES`] stereo sample pairs.
pub fn adp_frame_count(data: &[u8], trim_padding: bool) -> Result<usize> {
    let mut frames = data.len() / ADP_FRAME_BYTES;
    if frames == 0 {
        return Err(AudioError::Truncated {
            what: "ADP stream",
            need: ADP_FRAME_BYTES,
            have: data.len(),
        });
    }
    if trim_padding {
        while frames > 0
            && data[(frames - 1) * ADP_FRAME_BYTES..frames * ADP_FRAME_BYTES]
                .iter()
                .all(|&b| b == 0)
        {
            frames -= 1;
        }
    }
    Ok(frames)
}

/// Decodes a stream a piece at a time: the predictor history carries from one call to the next, so feeding
/// the frames in any number of pieces gives exactly the samples of [`decode_adp`]. For outputs that take
/// audio in blocks (a browser `AudioBuffer`), so a whole track never has to exist as PCM in memory.
#[derive(Debug, Clone, Default)]
pub struct AdpDecoder {
    /// Per channel `(h1, h2)`.
    hist: [(i64, i64); 2],
}

impl AdpDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode whole frames (`frames.len()` is taken down to a multiple of [`ADP_FRAME_BYTES`]) and append
    /// the interleaved L,R samples to `out`.
    pub fn decode(&mut self, frames: &[u8], out: &mut Vec<i16>) {
        for frame in frames.as_chunks::<ADP_FRAME_BYTES>().0 {
            let mut ch_samples = [[0i16; ADP_FRAME_SAMPLES]; 2];
            for (ch, samples) in ch_samples.iter_mut().enumerate() {
                let hdr = frame[ch];
                let filter = FILTERS[usize::from((hdr >> 4) & 3)];
                let range = u32::from(hdr & 15);
                // History is kept with 6 fractional bits and is *not* clamped; only the predictor term is
                // clamped to the 16-bit range (at that precision). This reproduces ffmpeg's `adpcm_dtk`
                // exactly.
                let (mut h1, mut h2) = self.hist[ch];
                for (n, slot) in samples.iter_mut().enumerate() {
                    let byte = frame[4 + n];
                    let nib = if ch == 0 { byte & 15 } else { byte >> 4 };
                    let nib = i64::from(((nib << 4) as i8) >> 4);
                    let pred = ((filter.0 * h1 + filter.1 * h2 + 32) >> 6)
                        .clamp(-(32768 << 6), (32767 << 6) | 63);
                    let y = (((nib << 12) >> range) << 6) + pred;
                    *slot = (y >> 6).clamp(-32768, 32767) as i16;
                    h2 = h1;
                    h1 = y;
                }
                self.hist[ch] = (h1, h2);
            }
            for (l, r) in ch_samples[0].iter().zip(&ch_samples[1]) {
                out.push(*l);
                out.push(*r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(hl: u8, hr: u8, data: [u8; 28]) -> Vec<u8> {
        let mut v = vec![hl, hr, hl, hr];
        v.extend_from_slice(&data);
        v
    }

    #[test]
    fn range12_filter0_is_raw_nibbles() {
        // range 12 => shift 0, filter 0 => sample == signed nibble.
        // byte 0x2F: left = 0xF = -1, right = 0x2 = 2. byte 0x87: left 7, right 0x8 = -8.
        let mut d = [0u8; 28];
        d[0] = 0x2F;
        d[1] = 0x87;
        let out = decode_adp_opts(&frame(0x0C, 0x0C, d), false).unwrap();
        assert_eq!(out.frames(), 28);
        assert_eq!(&out.samples[..4], &[-1, 2, 7, -8]);
    }

    #[test]
    fn range_scales_and_filter1_predicts() {
        // Left: filter 1, range 8 => nibble << 4. Right: filter 0, range 10 => nibble << 2.
        // byte0: left 3, right 1 ; byte1: 0,0 ; byte2: left 0xF (-1), right 0.
        let mut d = [0u8; 28];
        d[0] = 0x13;
        d[2] = 0x0F;
        let out = decode_adp_opts(&frame(0x18, 0x0A, d), false).unwrap();
        // L0 = 3<<4 = 48 (+ (60*0+32)>>6 = 0) = 48
        // L1 = 0 + (60*48 + 32)>>6 = 2912>>6 = 45
        // L2 = (-1<<4) + (60*45 + 32)>>6 = -16 + 2732>>6 (=42) = 26
        let l: Vec<i16> = out.samples.iter().step_by(2).take(3).copied().collect();
        assert_eq!(l, vec![48, 45, 26]);
        // R0 = 1<<2 = 4 ; R1 = 0
        assert_eq!(out.samples[1], 4);
        assert_eq!(out.samples[3], 0);
    }

    #[test]
    fn history_carries_across_frames_and_zero_tail_is_trimmed() {
        let mut d = [0u8; 28];
        d[27] = 0x0F; // L = -1 at last sample of frame 0 (range 12 filter 1)
        let mut bytes = frame(0x1C, 0x0C, d);
        // frame 1: filter 1 range 12, all-zero data: first sample = (60*h1+32)>>6 with h1 from frame 0.
        bytes.extend(frame(0x1C, 0x0C, [0u8; 28]));
        bytes.extend(vec![0u8; 64]); // padding
        let untrimmed = decode_adp_opts(&bytes, false).unwrap();
        assert_eq!(untrimmed.frames(), 4 * 28);
        let trimmed = decode_adp(&bytes).unwrap();
        assert_eq!(trimmed.frames(), 2 * 28);
        assert_eq!(trimmed.samples[27 * 2], -1);
        // Frame 1 sample 0: (60*-1 + 32)>>6 = -28>>6 = -1 (arithmetic shift).
        assert_eq!(trimmed.samples[28 * 2], -1);
    }

    #[test]
    fn full_scale_nibble_and_clamp() {
        // range 0 => shift 12: nibble 7 -> 28672, nibble -8 -> -32768.
        let d = [0x87u8; 28];
        let out = decode_adp_opts(&frame(0x00, 0x00, d), false).unwrap();
        assert_eq!(out.samples[0], 7 << 12);
        assert_eq!(out.samples[1], -8 << 12);
        // filter 2 on a sustained maximum overshoots and must clamp rather than wrap.
        let out = decode_adp_opts(&frame(0x20, 0x20, [0x77u8; 28]), false).unwrap();
        assert!(out.samples.iter().all(|&s| s > 0));
        assert_eq!(*out.samples.iter().max().unwrap(), i16::MAX);
    }

    #[test]
    fn history_keeps_fractional_bits_like_ffmpeg() {
        // Impulse of 7 through filter 1 (60/64 per step); values observed from ffmpeg's adpcm_dtk.
        let mut d = [0u8; 28];
        d[0] = 0x07;
        let out = decode_adp_opts(&frame(0x1C, 0x0C, d), false).unwrap();
        let l: Vec<i16> = out.samples.iter().step_by(2).take(10).copied().collect();
        assert_eq!(l, vec![7, 6, 6, 5, 5, 5, 4, 4, 4, 3]);
    }

    #[test]
    fn rejects_empty() {
        assert!(decode_adp(&[0u8; 31]).is_err());
        assert!(adp_frame_count(&[0u8; 31], true).is_err());
    }

    #[test]
    fn decoding_in_pieces_gives_the_same_samples() {
        // A stream that exercises every filter and range, with history that matters across frames.
        let mut bytes = Vec::new();
        let mut x = 0x2545_f491u32;
        for f in 0..40u8 {
            let mut d = [0u8; 28];
            for b in &mut d {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                *b = (x >> 8) as u8;
            }
            bytes.extend(frame(
                ((f % 4) << 4) | (f % 13),
                (((f + 1) % 4) << 4) | (f % 11),
                d,
            ));
        }
        bytes.extend(vec![0u8; 96]); // padding
        let whole = decode_adp(&bytes).unwrap();
        assert_eq!(adp_frame_count(&bytes, true).unwrap(), 40);
        assert_eq!(adp_frame_count(&bytes, false).unwrap(), 43);
        for piece in [1usize, 3, 7, 40] {
            let mut dec = AdpDecoder::new();
            let mut out = Vec::new();
            for chunk in bytes[..40 * ADP_FRAME_BYTES].chunks(piece * ADP_FRAME_BYTES) {
                dec.decode(chunk, &mut out);
            }
            assert_eq!(out, whole.samples, "pieces of {piece} frame(s)");
        }
    }
}
