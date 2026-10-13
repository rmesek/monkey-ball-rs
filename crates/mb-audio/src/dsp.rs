//! GameCube DSP-ADPCM (the sample codec inside MusyX `.samp` data).
//!
//! 8-byte frames of 14 samples. Byte 0: `coef_index = hdr >> 4` (0..=7), `scale = 1 << (hdr & 15)`. Bytes
//! 1..8 hold 14 signed nibbles, high nibble first. Each sample is
//! `clamp(((nibble * scale) << 11) + 1024 + c0*h1 + c1*h2) >> 11` where `(c0, c1) = coefs[2*idx..]`.

use crate::{AudioError, Result};

pub const DSP_FRAME_BYTES: usize = 8;
pub const DSP_FRAME_SAMPLES: usize = 14;

/// Number of bytes needed to hold `samples` DSP-ADPCM samples.
pub fn dsp_byte_len(samples: usize) -> usize {
    samples.div_ceil(DSP_FRAME_SAMPLES) * DSP_FRAME_BYTES
}

/// Stateful DSP-ADPCM decoder (coefficient table and two-sample history).
#[derive(Debug, Clone)]
pub struct DspDecoder {
    pub coefs: [i16; 16],
    pub hist1: i32,
    pub hist2: i32,
}

impl DspDecoder {
    pub fn new(coefs: [i16; 16], hist1: i16, hist2: i16) -> Self {
        Self {
            coefs,
            hist1: i32::from(hist1),
            hist2: i32::from(hist2),
        }
    }

    /// Decode one 8-byte frame into 14 samples.
    pub fn decode_frame(&mut self, frame: &[u8; DSP_FRAME_BYTES]) -> [i16; DSP_FRAME_SAMPLES] {
        let idx = usize::from(frame[0] >> 4) & 7;
        let scale = 1i32 << (frame[0] & 15);
        let c0 = i32::from(self.coefs[idx * 2]);
        let c1 = i32::from(self.coefs[idx * 2 + 1]);
        let mut out = [0i16; DSP_FRAME_SAMPLES];
        for (n, slot) in out.iter_mut().enumerate() {
            let byte = frame[1 + n / 2];
            let nib = if n % 2 == 0 { byte >> 4 } else { byte & 15 };
            let nib = i32::from(((nib << 4) as i8) >> 4);
            let v = ((nib * scale) << 11) + 1024 + c0 * self.hist1 + c1 * self.hist2;
            let s = (v >> 11).clamp(-32768, 32767);
            *slot = s as i16;
            self.hist2 = self.hist1;
            self.hist1 = s;
        }
        out
    }
}

/// Decode `num_samples` samples from `data` (which must hold at least [`dsp_byte_len`] bytes).
pub fn decode_dsp_adpcm(
    data: &[u8],
    coefs: &[i16; 16],
    num_samples: usize,
    hist1: i16,
    hist2: i16,
) -> Result<Vec<i16>> {
    let need = dsp_byte_len(num_samples);
    if data.len() < need {
        return Err(AudioError::Truncated {
            what: "DSP-ADPCM data",
            need,
            have: data.len(),
        });
    }
    let mut dec = DspDecoder::new(*coefs, hist1, hist2);
    let mut out = Vec::with_capacity(num_samples + DSP_FRAME_SAMPLES);
    for frame in data[..need].as_chunks::<DSP_FRAME_BYTES>().0 {
        out.extend_from_slice(&dec.decode_frame(frame));
    }
    out.truncate(num_samples);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_computed_frame() {
        // coef pair 0 = (2048, 0): prediction == previous sample (2048/2048). Header 0x00: scale 1.
        let mut coefs = [0i16; 16];
        coefs[0] = 2048;
        // nibbles: 1, 2, F(-1), 7, 0, 0, ... packed high nibble first.
        let frame = [0x00, 0x12, 0xF7, 0, 0, 0, 0, 0];
        let out = decode_dsp_adpcm(&frame, &coefs, 14, 0, 0).unwrap();
        // s0 = (1*2048 + 1024 + 0) >> 11 = 3072 >> 11 = 1
        // s1 = (2*2048 + 1024 + 2048*1) >> 11 = 7168 >> 11 = 3
        // s2 = (-2048 + 1024 + 2048*3) >> 11 = 5120 >> 11 = 2
        // s3 = (7*2048 + 1024 + 2048*2) >> 11 = 19456 >> 11 = 9
        // then zero nibbles: (1024 + 2048*h) >> 11 = h, so the value holds.
        assert_eq!(&out[..6], &[1, 3, 2, 9, 9, 9]);
        assert_eq!(out.len(), 14);
    }

    #[test]
    fn scale_and_history_and_truncate() {
        let mut coefs = [0i16; 16];
        coefs[2] = 1024; // idx 1: c0 = 0.5 * h1
        coefs[3] = 1024; // c1 = 0.5 * h2
        // header 0x13: idx 1, scale 8. nibble 4 -> 32.
        let frame = [0x13, 0x40, 0x00, 0, 0, 0, 0, 0];
        let out = decode_dsp_adpcm(&frame, &coefs, 3, 100, -20).unwrap();
        // s0 = (4*8*2048 + 1024 + 1024*100 + 1024*-20) >> 11 = (65536+1024+81920) >> 11 = 148480>>11 = 72
        // s1 = (0 + 1024 + 1024*72 + 1024*100) >> 11 = 177152 >> 11 = 86
        // s2 = (1024 + 1024*86 + 1024*72) >> 11 = 162816 >> 11 = 79
        assert_eq!(out, vec![72, 86, 79]);
    }

    #[test]
    fn truncated_input_errors() {
        assert!(decode_dsp_adpcm(&[0u8; 8], &[0; 16], 15, 0, 0).is_err());
    }

    #[test]
    fn clamps() {
        let mut coefs = [0i16; 16];
        coefs[0] = 4096; // 2x previous
        let frame = [0x0F, 0x77, 0x77, 0x77, 0x77, 0x77, 0x77, 0x77];
        let out = decode_dsp_adpcm(&frame, &coefs, 14, 0, 0).unwrap();
        assert_eq!(*out.last().unwrap(), i16::MAX);
    }
}
