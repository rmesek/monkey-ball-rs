# Audio formats (SMB1 USA disc) and the `mb-audio` crate

All of this was derived from the bytes of the USA disc (`test/snd/**`) plus knowledge of the GameCube DTK and
MusyX formats. Nothing from the disc is stored in the repo. `crates/mb-audio` is std-only (dev-dependency on
`mb-assets` for tests/examples); it opens no audio device.

## Music: `test/snd/adp/*.adp` (112 files) — fully decoded

* 32-byte frames, always stereo, no file header. File size is a multiple of 32 KiB because of trailing
  **all-zero padding frames** (0 to ~30 KiB, i.e. up to 0.6 s). `decode_adp` drops them so loops are gapless.
* Frame: `hdrL hdrR hdrL hdrR` (header bytes repeated), then 28 data bytes. Byte `n` holds sample `n` of the
  left channel in the **low** nibble and of the right channel in the **high** nibble. 28 samples per channel
  per frame, history carried across frames.
* Header byte: `filter = hdr >> 4` (0..3), `range = hdr & 15`. Sample = CD-XA style: `(nibble << 12) >> range`
  plus predictor `(c0*h1 + c1*h2 + 32) >> 6`, pairs `(0,0) (60,0) (115,-52) (98,-55)`.
  Quirk needed for bit-exactness: the history is kept with 6 fractional bits and is not clamped (`y = x*64 +
  pred`; output `y >> 6` clamped), and the predictor term alone is clamped to `[-32768*64, 32767*64+63]`.
* Sample rate: nominal 48000 Hz (hardware DTK is ~48043 Hz; the constant is `ADP_SAMPLE_RATE`).
* **Verified**: all 112 files decode bit-identical to ffmpeg's `adpcm_dtk` (`ffmpeg -i x.adp -f s16le`),
  ignoring the zero tail. Durations 0.66 s (`st5_int`) to 105.7 s (`race`); loud masters touch full scale in
  places but fewer than 1% of samples clip.
* Naming: `X_int` + `X_lp` (intro, then loop), `X_all` + `X_lp` (`st2`, `sel2`: `_all` is the longer lead-in
  stream, `_lp` loops; the exact join is not verified byte-wise), or a single `X` that loops whole. A trailing
  `m` marks a **mono mixdown** (L == R, verified for every `m` file) of the same track. `race.adp` also exists
  in `snd/mkb/` (identical bytes).
* `music::catalog` groups stems into `MusicTrack { key, intro, body, mono }`.
* Reference mapping (`audio.ts` track names -> disc): derivable from the files alone are `jungle = st1`
  (single, no intro in the reference), `arctic = st7` (single), `bonus = stb`, `master = stm`, `ending = end`.
  **Not determinable here:** `sky mall water desert storm extra` map onto `st2 st3 st4 st5 st6 st8` in some
  order; that table lives in the game code (`main.dol`), not in `test/`.
* `test/snd/test/stream{L,R}.pcm`: raw big-endian signed 16-bit mono, 18,547,200 bytes each (9,273,600
  samples). Rate unknown (193.2 s if 48 kHz); a development test stream, unused by the game flow.
  `music::decode_pcm_s16be` reads it.

## Sound effects: `test/snd/mkb/<bank>.{proj,pool,sdir,samp}` (MusyX) — decoded

Nine banks, in MusyX group-id order: `allse`(0) `bil`(1) `bowl`(2) `golf`(3) `race`(4) `target`(5) `fight`(6)
`comn`(7) `ending`(8). All big-endian. (`allse.poo/.pro/.sam/.sdi` are older leftovers; unused.)

* **`.proj`**: chain of groups `[u32 size][u16 group_id][u16 type]` + 8 offsets (40-byte header), ended by
  `0xFFFFFFFF`. All groups here are SFX groups (`type 1`). The offset at header+0x14 is the SFX table:
  `u16 count, u16 pad`, then 10-byte entries `u16 sfx_id, u16 macro_id, u8 priority, u8 max_voices,
  u8 default_velocity(127), u8 pan(64), u8 default_key(60), u8 pad`.
  The SFX ids of the nine banks tile `0x000..=0x367` contiguously (allse 0..0x12a, bil 0x12b.., bowl 0x16b..,
  golf 0x1a9.., race 0x1de.., target 0x218.., fight 0x27c.., comn 0x2d2.., ending 0x35d..0x367).
* **`.pool`**: 16-byte header (`u32` offset of macro section = 0x10; table/keymap/layer offsets are 0, so only
  macros exist), then objects `[u32 size][u16 id][u16 pad]` + 8-byte commands, ended by `0xFFFFFFFF`.
  Command = two BE words; opcode = low byte of word 0, argument = `word0 >> 8`. Opcodes seen: `0x00` end,
  `0x05`, `0x07` wait, `0x10` **start sample** (sample id = arg), `0x11` stop sample, `0x14`, `0x19`, `0x31`,
  `0x38`. Only `0x10` is interpreted; the rest are kept raw (`MacroCmd`).
* **`.sdir`**: 32-byte sample entries, terminated by id `0xFFFF`: `u16 id, u16 0, u32 samp_offset, u32 0,
  u8 root_key(60), u8 0, u16 sample_rate, u32 (format<<24 | num_samples), u32 loop_start, u32 loop_length
  (0 = none), u32 params_offset`. Format is 0 (DSP-ADPCM) for every sample. Rates: 11025, 22050, 30000, 32000,
  44100. After the table, 40-byte parameter blocks: `u8 0, u8 8 (bytes/frame), u8 ps, u8 loop_ps,
  s16 loop_hist2 (older), s16 loop_hist1 (newer), s16 coefs[16]`.
* **`.samp`**: raw DSP-ADPCM, 8-byte frames of 14 samples, samples 32-byte aligned.
* DSP-ADPCM: header byte `coef_index = hdr>>4`, `scale = 1 << (hdr&15)`; sample = `(((nibble*scale) << 11) +
  1024 + c0*h1 + c1*h2) >> 11` clamped; initial history 0.
* **Verified**: every sample of every bank decodes; the first frame's `ps` equals `initial_ps`; for all
  looping samples (15) the stored loop history equals the decoded samples before `loop_start`, which pins down the
  decoder bit-exactly; loop end == `loop_start + loop_length == num_samples`; every start-sample id exists in
  its own bank.
* **SFX id mapping:** the reference ids (`smb1_sfx_ids.ts`) carry flag bits above bit 9
  (`0x281b`, `0x3b1f4`, `0xa022`, ...). `id & 0x3ff` lands in the populated MusyX index range for all 72 of
  them (`sfx_index_from_game_id`; inferred, consistent but not proven). 51 of the 72 resolve to an audible
  sample; the other 21 resolve to **silent placeholder macros** (a macro with no `0x10` command; 114 of the 299
  `allse` entries and 42 of the 139 `comn` entries are like this). Where the original plays those
  (sequences/other mechanisms) is not recoverable from the banks.
* **Named SFX** (`ball_roll`, `goal_enter`, `banana_collect`, `bumper_hit`, `fallout`, `an_*` announcer lines,
  ...): the banks carry no names, so these cannot be mapped from the bytes. Candidates by shape: looping samples
  are the continuous sounds (`comn` samples 31, 88, 161 with ~20k-35k-frame loops; `ball_roll` is likely one
  of them). Needs listening (`examples/dump_wav`) or the game's sound-id table.

## API summary

* `adp::decode_adp(&[u8]) -> AdpAudio { sample_rate, samples: interleaved i16 }`, `decode_adp_opts(.., trim)`.
* `dsp::{DspDecoder, decode_dsp_adpcm}`.
* `musyx::{MusyxBank::parse(proj, pool, sdir, samp), SfxLibrary, load_library(fetch)}`; `render(sfx_id)` ->
  `RenderedSfx { sample (pcm, rate, loop_range), volume, pan }`; `sfx_index_from_game_id`.
* `music::{catalog, reference_music_key, decode_pcm_s16be}`.
* `mixer::{Mixer, Sound, PlayParams, Bus, VoiceId}`: `play`, `play_music(intro, loop, volume)` (sample-accurate
  handoff), `set_volume/pan/rate`, per-bus and master gain, delay, voice stealing, `mix(&mut [f32])`
  (interleaved stereo, unclamped). Linear-interpolation resampler; pan is a balance law (centre = unity).
* `examples/dump_wav`: writes WAVs of everything to a scratch directory (never commit the output).

## Open items

* Name table for the reference's named SFX and the six unresolved music themes (see above).
* Macro semantics beyond start-sample (ADSR/fade `0x14`, tremolo `0x19`, loops `0x05`): SFX are played raw at
  the sample's native rate with the group entry's velocity/pan; envelopes are not applied.
* `*_all` + `*_lp` exact join, and the PCM stream's sample rate.
