//! Audio decoders (ADP music, MusyX sound effects) and a pure-Rust PCM mixer.
//!
//! * [`adp`]: GameCube DTK streaming ADPCM (`test/snd/adp/*.adp`), 32-byte stereo frames.
//! * [`dsp`]: GameCube DSP-ADPCM, the sample codec used inside MusyX banks.
//! * [`musyx`]: MusyX `.proj/.pool/.sdir/.samp` bank reader (`test/snd/mkb/*`).
//! * [`music`]: groups the ADP files into intro/loop tracks and mono variants.
//! * [`mixer`]: voices with volume/pan/pitch/loop and intro-to-loop handoff, producing `f32` PCM.
//! * [`driver`]: the mixer behind a non-blocking message queue, with output telemetry.
//! * [`sound_table`]: the game's sound-request table from `main.dol` (request id -> MusyX id, velocity).
//!
//! Nothing here touches an audio device; an output backend pulls interleaved stereo `f32` from
//! [`driver::MixDriver::process`] (or [`mixer::Mixer::mix`] directly).
//! See `docs/AUDIO.md` for the byte-level formats.

pub mod adp;
pub mod driver;
pub mod dsp;
pub mod mixer;
pub mod music;
pub mod musyx;
pub mod sound_table;
pub mod wav;

use std::fmt;

/// Errors from the decoders and bank parsers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioError {
    /// Input is too short for what it declares.
    Truncated {
        what: &'static str,
        need: usize,
        have: usize,
    },
    /// Structurally invalid data.
    Invalid(String),
    /// A valid but not implemented variant (e.g. a non-DSP sample format).
    Unsupported(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::Truncated { what, need, have } => {
                write!(f, "{what}: need {need} bytes, have {have}")
            }
            AudioError::Invalid(s) => write!(f, "invalid data: {s}"),
            AudioError::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for AudioError {}

pub type Result<T> = std::result::Result<T, AudioError>;

pub(crate) fn be_u16(d: &[u8], off: usize, what: &'static str) -> Result<u16> {
    d.get(off..off + 2)
        .map(|b| u16::from_be_bytes([b[0], b[1]]))
        .ok_or(AudioError::Truncated {
            what,
            need: off + 2,
            have: d.len(),
        })
}

pub(crate) fn be_u32(d: &[u8], off: usize, what: &'static str) -> Result<u32> {
    d.get(off..off + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(AudioError::Truncated {
            what,
            need: off + 4,
            have: d.len(),
        })
}
