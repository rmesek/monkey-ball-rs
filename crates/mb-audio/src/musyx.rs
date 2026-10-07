//! MusyX sound-effect banks (`test/snd/mkb/<name>.proj|.pool|.sdir|.samp`), big-endian throughout.
//!
//! * `.proj`: chain of groups, each `u32 size` then a 40-byte header
//!   (`u32 size, u16 group_id, u16 type, 8 x u32 offsets`) and, for SFX groups (`type == 1`), a page table at
//!   `offsets[5]`: `u16 count, u16 pad`, then `count` 10-byte entries
//!   `u16 sfx_id, u16 macro_id, u8 priority, u8 max_voices, u8 default_velocity, u8 pan, u8 default_key, u8 pad`.
//!   The chain ends with `0xFFFFFFFF`.
//! * `.pool`: 16-byte header (`u32` offsets of macro/table/keymap/layer sections; only the macro section is
//!   used on this disc) and objects `u32 size, u16 id, u16 pad, 8-byte commands...` ending with `0xFFFFFFFF`.
//!   A command is two big-endian words; the opcode is the low byte of the first, the rest of that word
//!   (`word0 >> 8`) is the argument. Opcode `0x10` (start sample) carries the sample id in `word0 >> 8`.
//! * `.sdir`: 32-byte sample entries, terminated by id `0xFFFF`, followed by 40-byte DSP-ADPCM parameter blocks.
//! * `.samp`: raw DSP-ADPCM sample data, referenced by byte offset.
//!
//! Other files next to these (`allse.poo/.pro/.sam/.sdi`) are unused development leftovers.

use crate::dsp::{decode_dsp_adpcm, dsp_byte_len};
use crate::mixer::Sound;
use crate::{be_u16, be_u32, AudioError, Result};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Bank base names on the disc (file stems under `test/snd/mkb/`), in MusyX group-id order.
pub const BANK_NAMES: [&str; 9] = [
    "allse", "bil", "bowl", "golf", "race", "target", "fight", "comn", "ending",
];

/// Macro opcode: start playing a sample (id in `arg`).
pub const OP_START_SAMPLE: u8 = 0x10;

/// The game's sound-request ids carry flags above bit 9; the MusyX SFX index is the low 10 bits.
/// (Inferred: every id in the reference's `SMB1_SFX_IDS` maps into a populated index range this way.)
pub fn sfx_index_from_game_id(id: u32) -> u16 {
    (id & 0x3ff) as u16
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SfxEntry {
    pub sfx_id: u16,
    pub macro_id: u16,
    pub priority: u8,
    pub max_voices: u8,
    pub default_velocity: u8,
    /// 0..=127, 64 = centre.
    pub pan: u8,
    pub default_key: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub group_id: u16,
    /// 1 = SFX group (the only kind on this disc); other kinds are skipped.
    pub kind: u16,
    pub sfx: Vec<SfxEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacroCmd {
    pub opcode: u8,
    /// `word0 >> 8`.
    pub arg: u32,
    pub word1: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Macro {
    pub id: u16,
    pub cmds: Vec<MacroCmd>,
}

impl Macro {
    /// Sample id of the first start-sample command, if the macro plays one at all.
    pub fn first_sample(&self) -> Option<u16> {
        self.cmds
            .iter()
            .find(|c| c.opcode == OP_START_SAMPLE)
            .map(|c| (c.arg & 0xffff) as u16)
    }
}

/// DSP-ADPCM parameter block (`.sdir`, 40 bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DspParams {
    pub bytes_per_frame: u8,
    /// Predictor/scale byte of the first frame.
    pub initial_ps: u8,
    /// Predictor/scale byte of the frame containing the loop start.
    pub loop_ps: u8,
    /// `sample[loop_start - 1]` (most recent).
    pub loop_hist1: i16,
    /// `sample[loop_start - 2]`.
    pub loop_hist2: i16,
    pub coefs: [i16; 16],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleEntry {
    pub id: u16,
    /// Byte offset into `.samp`.
    pub offset: u32,
    /// Root key (60 = middle C everywhere on this disc).
    pub root_key: u8,
    pub sample_rate: u16,
    /// 0 = DSP-ADPCM (the only format present).
    pub format: u8,
    pub num_samples: u32,
    pub loop_start: u32,
    /// 0 = does not loop.
    pub loop_length: u32,
    pub params: DspParams,
}

/// A fully decoded mono sample.
#[derive(Debug, Clone)]
pub struct DecodedSample {
    pub sample_id: u16,
    pub sample_rate: u32,
    pub root_key: u8,
    pub pcm: Vec<i16>,
    /// Loop range in samples (`start..end`), if looping.
    pub loop_range: Option<(usize, usize)>,
}

/// A sound effect ready to hand to the mixer.
#[derive(Debug, Clone)]
pub struct RenderedSfx {
    pub sfx_id: u16,
    pub group_id: u16,
    pub sample: DecodedSample,
    /// `default_velocity / 127`.
    pub volume: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
}

impl RenderedSfx {
    pub fn to_sound(&self) -> Sound {
        Sound::mono(
            self.sample.sample_rate,
            self.sample.pcm.clone(),
            self.sample.loop_range,
        )
    }
}

#[derive(Debug, Clone)]
pub struct MusyxBank {
    pub groups: Vec<Group>,
    pub macros: BTreeMap<u16, Macro>,
    pub samples: BTreeMap<u16, SampleEntry>,
    samp: Vec<u8>,
}

impl MusyxBank {
    /// Parse a bank from its four files. `samp` is retained for later decoding.
    pub fn parse(proj: &[u8], pool: &[u8], sdir: &[u8], samp: Vec<u8>) -> Result<Self> {
        Ok(Self {
            groups: parse_proj(proj)?,
            macros: parse_pool(pool)?,
            samples: parse_sdir(sdir)?,
            samp,
        })
    }

    /// All SFX entries across groups.
    pub fn sfx_entries(&self) -> impl Iterator<Item = (&Group, &SfxEntry)> {
        self.groups
            .iter()
            .flat_map(|g| g.sfx.iter().map(move |e| (g, e)))
    }

    pub fn find_sfx(&self, sfx_id: u16) -> Option<(&Group, &SfxEntry)> {
        self.sfx_entries().find(|(_, e)| e.sfx_id == sfx_id)
    }

    /// Sample played by an SFX (first start-sample command of its macro). `None` for silent placeholder macros.
    pub fn sample_id_for_sfx(&self, sfx_id: u16) -> Option<u16> {
        let (_, e) = self.find_sfx(sfx_id)?;
        self.macros.get(&e.macro_id)?.first_sample()
    }

    /// The raw DSP-ADPCM bytes of one sample (frame-aligned length).
    pub fn raw_sample_data(&self, sample_id: u16) -> Result<&[u8]> {
        let e = self
            .samples
            .get(&sample_id)
            .ok_or_else(|| AudioError::Invalid(format!("no sample {sample_id}")))?;
        let start = e.offset as usize;
        let end = start + dsp_byte_len(e.num_samples as usize);
        self.samp.get(start..end).ok_or(AudioError::Truncated {
            what: "sample data",
            need: end,
            have: self.samp.len(),
        })
    }

    /// Decode one sample to 16-bit mono PCM.
    pub fn decode_sample(&self, sample_id: u16) -> Result<DecodedSample> {
        let e = self
            .samples
            .get(&sample_id)
            .ok_or_else(|| AudioError::Invalid(format!("no sample {sample_id}")))?;
        if e.format != 0 {
            return Err(AudioError::Unsupported(format!(
                "sample {sample_id}: format {}",
                e.format
            )));
        }
        let n = e.num_samples as usize;
        let data = self.raw_sample_data(sample_id)?;
        let pcm = decode_dsp_adpcm(data, &e.params.coefs, n, 0, 0)?;
        let loop_range = (e.loop_length > 0).then(|| {
            let s = (e.loop_start as usize).min(n);
            (s, (s + e.loop_length as usize).min(n))
        });
        Ok(DecodedSample {
            sample_id,
            sample_rate: u32::from(e.sample_rate),
            root_key: e.root_key,
            pcm,
            loop_range,
        })
    }

    /// Decode the sample for an SFX id. `Ok(None)` when the SFX is a silent macro or unknown.
    pub fn render_sfx(&self, sfx_id: u16) -> Result<Option<RenderedSfx>> {
        let Some((group, entry)) = self.find_sfx(sfx_id) else {
            return Ok(None);
        };
        let Some(sid) = self
            .macros
            .get(&entry.macro_id)
            .and_then(Macro::first_sample)
        else {
            return Ok(None);
        };
        Ok(Some(RenderedSfx {
            sfx_id,
            group_id: group.group_id,
            sample: self.decode_sample(sid)?,
            volume: f32::from(entry.default_velocity) / 127.0,
            pan: (f32::from(entry.pan) - 64.0) / 64.0,
        }))
    }
}

/// Mixer-ready SFX: sound, default volume (0..=1), default pan (-1..=1).
pub type SfxPlayback = (Arc<Sound>, f32, f32);

/// Several banks addressed by global SFX index.
#[derive(Debug, Default)]
pub struct SfxLibrary {
    banks: Vec<MusyxBank>,
    index: BTreeMap<u16, usize>,
}

impl SfxLibrary {
    pub fn new(banks: Vec<MusyxBank>) -> Self {
        let mut index = BTreeMap::new();
        for (i, b) in banks.iter().enumerate() {
            for (_, e) in b.sfx_entries() {
                index.entry(e.sfx_id).or_insert(i);
            }
        }
        Self { banks, index }
    }

    pub fn banks(&self) -> &[MusyxBank] {
        &self.banks
    }

    pub fn sfx_ids(&self) -> impl Iterator<Item = u16> + '_ {
        self.index.keys().copied()
    }

    pub fn contains(&self, sfx_id: u16) -> bool {
        self.index.contains_key(&sfx_id)
    }

    /// Render by MusyX SFX index.
    pub fn render(&self, sfx_id: u16) -> Result<Option<RenderedSfx>> {
        match self.index.get(&sfx_id) {
            Some(&b) => self.banks[b].render_sfx(sfx_id),
            None => Ok(None),
        }
    }

    /// Render by the game's sound-request id (see [`sfx_index_from_game_id`]).
    pub fn render_game_id(&self, id: u32) -> Result<Option<RenderedSfx>> {
        self.render(sfx_index_from_game_id(id))
    }

    /// Decode into mixer-ready sounds keyed by SFX index (silent macros omitted).
    pub fn to_sounds(&self) -> Result<BTreeMap<u16, SfxPlayback>> {
        let mut out = BTreeMap::new();
        for id in self.sfx_ids() {
            if let Some(r) = self.render(id)? {
                out.insert(id, (Arc::new(r.to_sound()), r.volume, r.pan));
            }
        }
        Ok(out)
    }
}

/// Load every bank in [`BANK_NAMES`] using `fetch(path)`, where `path` is relative to the disc root
/// (for example `test/snd/mkb/comn.proj`). Banks whose files cannot be fetched are an error.
pub fn load_library<F>(mut fetch: F) -> Result<SfxLibrary>
where
    F: FnMut(&str) -> std::io::Result<Vec<u8>>,
{
    let mut banks = Vec::new();
    for name in BANK_NAMES {
        let mut get = |ext: &str| -> Result<Vec<u8>> {
            let path = format!("test/snd/mkb/{name}.{ext}");
            fetch(&path).map_err(|e| AudioError::Invalid(format!("{path}: {e}")))
        };
        let (proj, pool, sdir, samp) = (get("proj")?, get("pool")?, get("sdir")?, get("samp")?);
        banks.push(MusyxBank::parse(&proj, &pool, &sdir, samp)?);
    }
    Ok(SfxLibrary::new(banks))
}

fn parse_proj(d: &[u8]) -> Result<Vec<Group>> {
    let mut groups = Vec::new();
    let mut p = 0usize;
    loop {
        let size = be_u32(d, p, "proj group size")?;
        if size == 0xffff_ffff {
            break;
        }
        if size < 0x28 {
            return Err(AudioError::Invalid(format!(
                "group size {size:#x} at {p:#x}"
            )));
        }
        let group_id = be_u16(d, p + 4, "proj group id")?;
        let kind = be_u16(d, p + 6, "proj group type")?;
        let mut sfx = Vec::new();
        if kind == 1 {
            let table = p + be_u32(d, p + 8 + 5 * 4, "proj page table off")? as usize;
            let count = usize::from(be_u16(d, table, "sfx count")?);
            for i in 0..count {
                let o = table + 4 + i * 10;
                let rec = d.get(o..o + 10).ok_or(AudioError::Truncated {
                    what: "sfx entry",
                    need: o + 10,
                    have: d.len(),
                })?;
                sfx.push(SfxEntry {
                    sfx_id: u16::from_be_bytes([rec[0], rec[1]]),
                    macro_id: u16::from_be_bytes([rec[2], rec[3]]),
                    priority: rec[4],
                    max_voices: rec[5],
                    default_velocity: rec[6],
                    pan: rec[7],
                    default_key: rec[8],
                });
            }
        }
        groups.push(Group {
            group_id,
            kind,
            sfx,
        });
        p += size as usize;
    }
    Ok(groups)
}

fn parse_pool(d: &[u8]) -> Result<BTreeMap<u16, Macro>> {
    let mut macros = BTreeMap::new();
    let mut p = be_u32(d, 0, "pool macro section")? as usize;
    loop {
        let size = be_u32(d, p, "pool object size")?;
        if size == 0xffff_ffff {
            break;
        }
        let size = size as usize;
        if size < 8 || !(size - 8).is_multiple_of(8) {
            return Err(AudioError::Invalid(format!(
                "pool object size {size:#x} at {p:#x}"
            )));
        }
        let id = be_u16(d, p + 4, "pool object id")?;
        let mut cmds = Vec::with_capacity((size - 8) / 8);
        for c in (p + 8..p + size).step_by(8) {
            let w0 = be_u32(d, c, "macro command")?;
            let w1 = be_u32(d, c + 4, "macro command")?;
            cmds.push(MacroCmd {
                opcode: (w0 & 0xff) as u8,
                arg: w0 >> 8,
                word1: w1,
            });
        }
        macros.insert(id, Macro { id, cmds });
        p += size;
    }
    Ok(macros)
}

fn parse_sdir(d: &[u8]) -> Result<BTreeMap<u16, SampleEntry>> {
    let mut out = BTreeMap::new();
    let mut p = 0usize;
    loop {
        let id = be_u16(d, p, "sdir entry id")?;
        if id == 0xffff {
            break;
        }
        let offset = be_u32(d, p + 4, "sample offset")?;
        let root_key = *d.get(p + 12).ok_or(AudioError::Truncated {
            what: "sdir entry",
            need: p + 13,
            have: d.len(),
        })?;
        let sample_rate = be_u16(d, p + 14, "sample rate")?;
        let ns = be_u32(d, p + 16, "num samples")?;
        let loop_start = be_u32(d, p + 20, "loop start")?;
        let loop_length = be_u32(d, p + 24, "loop length")?;
        let poff = be_u32(d, p + 28, "adpcm params offset")? as usize;
        let pb = d.get(poff..poff + 40).ok_or(AudioError::Truncated {
            what: "adpcm params",
            need: poff + 40,
            have: d.len(),
        })?;
        let mut coefs = [0i16; 16];
        for (i, c) in coefs.iter_mut().enumerate() {
            *c = i16::from_be_bytes([pb[8 + i * 2], pb[9 + i * 2]]);
        }
        let params = DspParams {
            bytes_per_frame: pb[1],
            initial_ps: pb[2],
            loop_ps: pb[3],
            // Stored oldest first: (sample[loop_start - 2], sample[loop_start - 1]).
            loop_hist2: i16::from_be_bytes([pb[4], pb[5]]),
            loop_hist1: i16::from_be_bytes([pb[6], pb[7]]),
            coefs,
        };
        out.insert(
            id,
            SampleEntry {
                id,
                offset,
                root_key,
                sample_rate,
                format: (ns >> 24) as u8,
                num_samples: ns & 0x00ff_ffff,
                loop_start,
                loop_length,
                params,
            },
        );
        p += 0x20;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be16(v: &mut Vec<u8>, x: u16) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn be32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    /// One SFX group with two SFX: id 5 -> macro 9 (plays sample 3), id 6 -> macro 10 (silent).
    fn proj() -> Vec<u8> {
        let mut v = Vec::new();
        be32(&mut v, 0x28 + 4 + 20 + 2); // size
        be16(&mut v, 7);
        be16(&mut v, 1);
        for off in [0x28u32, 0x28, 0x28, 0x28, 0x28, 0x28, 0, 0] {
            be32(&mut v, off);
        }
        be16(&mut v, 2); // count
        be16(&mut v, 0);
        for (sid, mid) in [(5u16, 9u16), (6, 10)] {
            be16(&mut v, sid);
            be16(&mut v, mid);
            v.extend_from_slice(&[0xff, 10, 127, 64, 60, 0]);
        }
        be16(&mut v, 0); // pad to declared size
        be32(&mut v, 0xffff_ffff);
        v
    }

    fn macro_obj(v: &mut Vec<u8>, id: u16, cmds: &[(u8, u32, u32)]) {
        be32(v, 8 + cmds.len() as u32 * 8);
        be16(v, id);
        be16(v, 0);
        for &(op, arg, w1) in cmds {
            be32(v, (arg << 8) | u32::from(op));
            be32(v, w1);
        }
    }

    fn pool() -> Vec<u8> {
        let mut v = Vec::new();
        be32(&mut v, 0x10);
        be32(&mut v, 0);
        be32(&mut v, 0);
        be32(&mut v, 0);
        macro_obj(
            &mut v,
            9,
            &[(0x38, 0, 0), (OP_START_SAMPLE, 3, 0), (0, 0, 0)],
        );
        macro_obj(&mut v, 10, &[(0x07, 0x010001, 0xffff0000), (0, 0, 0)]);
        be32(&mut v, 0xffff_ffff);
        v
    }

    fn sdir() -> Vec<u8> {
        let mut v = Vec::new();
        // entry: id 3, offset 0, key 60, rate 22050, 14 samples, loop 7..14
        be16(&mut v, 3);
        be16(&mut v, 0);
        be32(&mut v, 0);
        be32(&mut v, 0);
        v.extend_from_slice(&[60, 0]);
        be16(&mut v, 22050);
        be32(&mut v, 14);
        be32(&mut v, 7);
        be32(&mut v, 7);
        be32(&mut v, 0x24); // params right after terminator (0x20 + 4)
        be16(&mut v, 0xffff);
        be16(&mut v, 0xffff);
        // params: pad, 8, ps, loop_ps, hist1, hist2, coefs
        v.extend_from_slice(&[0, 8, 0x00, 0x00]);
        be16(&mut v, 0);
        be16(&mut v, 0);
        let mut coefs = [0u16; 16];
        coefs[0] = 2048;
        for c in coefs {
            be16(&mut v, c);
        }
        v
    }

    fn bank() -> MusyxBank {
        // frame: header 0 (scale 1, idx 0), nibbles 1,2,... see dsp tests
        let samp = vec![0x00, 0x12, 0xF7, 0, 0, 0, 0, 0];
        MusyxBank::parse(&proj(), &pool(), &sdir(), samp).unwrap()
    }

    #[test]
    fn parses_hand_built_bank() {
        let b = bank();
        assert_eq!(b.groups.len(), 1);
        assert_eq!(b.groups[0].group_id, 7);
        assert_eq!(b.groups[0].sfx.len(), 2);
        assert_eq!(b.groups[0].sfx[0].macro_id, 9);
        assert_eq!(b.groups[0].sfx[0].default_velocity, 127);
        assert_eq!(b.macros.len(), 2);
        assert_eq!(b.sample_id_for_sfx(5), Some(3));
        assert_eq!(b.sample_id_for_sfx(6), None);
        assert_eq!(b.sample_id_for_sfx(99), None);
        let s = &b.samples[&3];
        assert_eq!((s.sample_rate, s.num_samples, s.format), (22050, 14, 0));
        assert_eq!(s.params.coefs[0], 2048);
    }

    #[test]
    fn renders_sfx_with_loop() {
        let lib = SfxLibrary::new(vec![bank()]);
        assert!(lib.contains(5) && lib.contains(6) && !lib.contains(7));
        let r = lib.render(5).unwrap().unwrap();
        assert_eq!(&r.sample.pcm[..4], &[1, 3, 2, 9]);
        assert_eq!(r.sample.sample_rate, 22050);
        assert_eq!(r.sample.loop_range, Some((7, 14)));
        assert!((r.volume - 1.0).abs() < 1e-6);
        assert_eq!(r.pan, 0.0);
        assert!(lib.render(6).unwrap().is_none());
        assert!(lib.render_game_id(0x2805).unwrap().is_some()); // flags above bit 9 ignored
        let sounds = lib.to_sounds().unwrap();
        assert_eq!(sounds.len(), 1);
    }

    #[test]
    fn rejects_truncated_inputs() {
        let samp = vec![0u8; 4];
        let b = MusyxBank::parse(&proj(), &pool(), &sdir(), samp).unwrap();
        assert!(b.decode_sample(3).is_err());
        assert!(MusyxBank::parse(&proj()[..30], &pool(), &sdir(), vec![]).is_err());
        assert!(b.decode_sample(77).is_err());
    }

    #[test]
    fn sfx_index_masks_flags() {
        assert_eq!(sfx_index_from_game_id(0x281b), 0x1b);
        assert_eq!(sfx_index_from_game_id(0x3b1f4), 0x1f4);
        assert_eq!(sfx_index_from_game_id(0x0240), 0x240);
    }
}
