# Audio

Sound on desktop and in the browser, from the user's own disc: nothing audible is stored in the repo.
Three layers:

| Layer | Where | What |
|---|---|---|
| Decoders, mixer | `crates/mb-audio` (std only) | ADP music, MusyX banks, the executable's sound table, software mixer + queue |
| Game audio | `crates/mb-app/src/audio.rs` | `Sfx`, `Audio` handle, `AudioBackend` trait, `StageSounds` (simulation -> sounds), loading |
| Backends | `mb-app/src/audio_cpal.rs`, `crates/mb-web/src/audio.rs` | device output: cpal (native), Web Audio (browser) |

Reference: WebMonkeyBall `src/audio.ts` and its call sites in `game.ts`. The reference plays `.wav` / `.ogg`
files it does not ship (`ball_roll.wav`, `an_ready.wav`, `jungle_loop.ogg`, ...); here every sound is the
original's, found through the game's own tables (below).

## Architecture

```
game states --(Sfx, roll, stage music, volumes)--> Audio --flush(), once per frame--> AudioBackend
                                                                                       |
   MixBackend --MixMsg, bounded queue--> MixDriver { Mixer } <-- pulled by --  cpal callback (native)
                                                                              offline render (tests)
   WebAudio   --AudioBufferSourceNode / GainNode graph, mixed by the browser
   RecordingBackend (tests, `dev` feature), NullBackend (no device wanted)
```

* **`Audio`** (`Rc`, cheap clone; game thread only). States call `play(Sfx)`, `play_with(sfx, gain, delay)`,
  `roll(vol, pitch)`, `stage_music(bg, fetch)`, `stop_music()`, `stop_sounds()`, `set_volumes(music, sfx,
  announcer)`. Requests are collected and handed to the backend in one `submit(&[Command])` by `flush()`,
  which the top state calls at the end of its update. A frame with nothing to say submits nothing.
  `Menu::with_audio(audio)` / `mb_app::initial_game_with_audio(storage, audio)` attach one; the default is
  `Audio::silent()`.
* **`Command`**: `Play { sound, bus, volume, pan, rate, delay }`, `LoopStart / LoopSet / LoopStop { slot }`,
  `MusicPlay { volume }`, `MusicStop { fade }`, `BusVolume { bus, volume }`, `StopSounds`. Sounds are named
  by MusyX SFX id. Buses: `Music`, `Sfx`, `Announcer`, each with the saved volume; master gain 0.9 (reference).
* **`AudioBackend`**: `define_sound(id, Arc<Sound>)` and `define_music(intro, body)` at load time (or
  `define_music_streams(intro, body)` for a backend that takes the disc's streams undecoded),
  `clear_music()`, `submit`, `frame()` (device upkeep), `set_suspended(bool)`, `report()`.

### Desktop: cpal (`audio_cpal.rs`)
`CpalBackend` = `MixBackend` + an output stream on the default device (WASAPI, CoreAudio, ALSA).

* The game thread sends `MixMsg`s through `std::sync::mpsc::sync_channel(512)` (`try_send`: never blocks; a
  full queue drops the message and counts it). The stream callback takes the `MixDriver` with `try_lock`,
  drains the queue, mixes and writes. The lock is uncontended while a stream runs (the game thread touches
  the mixer only when no stream is running). The callback does not allocate (voices live in a `Vec`
  reserved once, a voice holds its two segments inline) and does no I/O; all PCM arrives decoded.
  Buffers are freed on the game thread: `MixBackend` keeps a reference to everything it queued and drops a
  retired music track only after the mixer has let go of it.
* Sample rates: every voice is resampled (linear interpolation) from its own rate (11025..48000 Hz) to
  the device rate, whatever that is; a reopened device with another rate continues mid-voice
  (`Mixer::set_sample_rate`, positions are kept in source frames). Channels: mono devices get the average,
  devices with more than two channels the front pair. Formats f32 / i16 / u16 / i32 / u8.
* No device, or a lost one: not an error. The backend stays silent, advances the mixer by wall-clock time
  so queued sounds end and the music is in the right place, and retries every two seconds.
* Suspend (window minimized or occluded): the stream is paused, so no callbacks run.
* Telemetry (a `dev` build prints it: `cargo run --release --features dev -- 1 --exit-after <s>`;
  `--mute` opens no device): device name and
  format, stream opens and errors, callbacks, frames, peak and RMS of the rendered signal, clamped samples,
  messages applied / dropped, callbacks that found the mixer locked, and "late callbacks" (a gap of more
  than three buffer lengths between callbacks, i.e. the device ran dry). WASAPI does not report underruns
  itself, so the late-callback count is the measure.

### Browser: Web Audio (`mb-web/src/audio.rs`)
The browser mixes; wasm only schedules.

* Each sound effect is one `AudioBuffer` (at its native rate, made once at load). Playing = `AudioBuffer
  SourceNode -> GainNode [-> StereoPannerNode] -> bus GainNode -> master GainNode -> destination`.
  `playbackRate` is the pitch. Loops are sources with `loop = true`; live updates use
  `gain.setTargetAtTime` (15 ms) and `playbackRate`.
* Music: two buffers on the audio clock, intro at `t = currentTime + 0.01`, body (`loop = true`) at
  `t + intro.duration`. The hand-off does not depend on the main thread.
* A frame without requests makes no JS call. The roll updates three layers every eighth tick.
* Autoplay policy: the context is created when the game starts (after the disc was picked). If it starts
  suspended, the first `pointerdown` / `keydown` / `touchend` on the page resumes it; what was scheduled
  meanwhile starts then (the audio clock was not running). `visibilitychange`: a hidden tab suspends the
  context (music pauses, nothing is processed), showing it resumes.
* Developer check: `(await import('./pkg/mb_web.js')).audio_debug_json()` in the console: context state,
  clock, buffers and bytes, load times, nodes started, and peak / RMS seen by an `AnalyserNode` on the
  master output (created by the first call, never otherwise).

## Disc reads

Only these, never during play. They take a fetch function, so an asynchronous prefetch can supply the bytes:

| When | Function | Files |
|---|---|---|
| Menu load (once) | `audio::load_sfx(fetch, main_dol)`; paths: `audio::sfx_paths()` | `test/snd/mkb/{allse,comn}.{proj,pool,sdir,samp}` (about 2.3 MB) and `main.dol` (already in `Content`) |
| Stage load, when the track changes | `audio::load_music(fetch, &track)`; paths: `mb_audio::music::stage_music(bg)?.paths()` | `test/snd/adp/<intro>.adp`, `<loop>.adp` |

With the music volume at zero the music files are not read at all.

## What plays when

`StageSounds::tick` runs after every simulation tick and mirrors the reference's call sites. The simulation
reports what happened through `Sim::events` (`SimEvent`, cleared every step, not hashed) and
`ball.audio` (`BallAudio`: collision flags and speed of the last step, as the reference's `ball.audio`).

| Event (reference call) | Sound(s), request id -> MusyX id (name in the executable) | Bus |
|---|---|---|
| Stage start (`playMusicForStage`) | music of the background, see below | Music |
| Intro reaches 120 frames, or at once on a retry (`playAnnouncerReady`) | `0x4` -> 170 `SND_SED_NAR_READY` | Announcer |
| Intro ends (`playAnnouncerGo`) | `0x5` -> 158 `SND_SED_NAR_GO` | Announcer |
| Every 8th tick, at once when silent (`updateRollingSound`) | three loops 248 / 273 / 274 `SND_SE01_RUN01..03` | Sfx |
| Collision harder than 0.187 / 0.143 / 0.099, at most every 6 frames (`playImpactForBall`) | `0x2f` -> 176, `0x14` -> 185, `0x13` -> 178 | Sfx |
| `SimEvent::BumperHit` (`consumeBallEvents`) | `0x5011` -> 165 | Sfx |
| `SimEvent::BananaCollected` (`playBananaCollect`) | single `0x3` -> 206, bunch `0x39` -> 207 | Sfx |
| `SimEvent::GoalEntered` (`playGoal`, `playAnnouncerGoal(0.5)`) | `0x16` -> 211 and `0x127` -> 734 `SFXtapecut`; `0x8` -> 160 `SND_SED_NAR_GOAL` 0.5 s later | Sfx, Announcer |
| `SimEvent::GoalLaunch` (`ball_woosh`) | `0x126` -> 733 `SFXfinishfly2` | Sfx |
| `SimEvent::FallOut` (`playFallout`) | `0x15` -> 105 `SND_SED_NAR_FALL00`; bonus stage: `0x45` -> 52 `SND_SED_NAR_BONUSFIN`, otherwise `0xc` -> 108 `SND_SED_NAR_FALLOUT` | Sfx, Announcer |
| `SimEvent::TimeOver` (`playAnnouncerTimeOver`) | `0xb` -> 255 `SND_SED_NAR_TIMEOVER` | Announcer |
| `SimEvent::BonusClear` (`playAnnouncerPerfect`) | `0x48` -> 166 `SND_SED_NAR_PERFECT` | Announcer |
| 11 s left (`playAnnouncerHurryUp`) | `0x7` -> 163 `SND_SED_NAR_HURRY` | Announcer |
| Every second from 10 s left (`playAnnouncerCount`) | `0x23 + n` -> `n` `SND_SED_NAR_0..9` | Announcer |
| Every second of play (original only) | `0x6` -> 164 clock tick; twice a second in the last 10 s, three times in the last 5 | Sfx |
| Menu / pause menu: move, confirm, back (original only) | `0x6c` -> 214, `0x6a` -> 244, `0x6b` -> 175 | Sfx |

Lifecycle: the pause menu holds the roll (the simulation stands still) while music and running one-shots
play on, as in the reference, which does nothing on pause. Leaving a stage (`PlayState::on_exit`) and a
practice retry cut all effects, loops and announcer lines (`StopSounds`). Music is started at stage load
only if the track differs from the one playing, and fades out (0.3 s) and is released when a menu screen
shows. Options: music and sound volume (the announcer follows sound), 0..100 in steps of ten, saved in
`mb_game::Settings` and applied at once. The dial is a scale: 80, the default, is a gain of 0.5 for music and
0.3 for sound, the levels the mix was balanced at, and 100 is a quarter louder; a category at zero is not played at all (no voices, no music file read, no decode).

### How the sounds were identified
The banks carry no names and the reference's `.wav` names are not on the disc, so the mapping comes from the
USA `main.dol`:

* **Sound table** (`mb_audio::sound_table`, 1061 entries of 16 bytes at `0x8011057c`): `s32` MusyX SFX id,
  name pointer, category byte, flags, `s16` request id. The game's sound function (`0x8002b360`) looks up
  the low 11 bits of a request id in the *request id* column (per character for voice categories 15..19),
  walks back over `DMY_CODE` alias entries, and passes the MusyX id to `sndFXStartEx`. Bits 11..17 are a
  signed velocity offset from 100; narrator lines get +10, MusyX ids 178 / 184 / 185 +27 and 206 / 207 +10.
  MusyX maps velocity to gain as `(v / 127)^2` (its 128-entry table in the executable). At run time the
  table is read from the user's `main.dol` (`SoundTable::from_dol`); the USA ids are the fallback when a
  content directory has no executable. The earlier guess in this document (`id & 0x3ff` = MusyX index) was
  wrong and is gone.
* **Call sites** give the request ids: `SMD_GAME_PLAY_MAIN` (tick, countdown table at `0x801101c8`, hurry
  up), the ball's collision code at `0x8003ba30` (the same 0.187 / 0.143 / 0.099 thresholds as the
  reference), the banana item at `0x800690dc`, the bumper at `0x8006bfac`, the goal tape at `0x8006f7c8`,
  the ring-out and time-over states, the menu code at `0x800a34xx`.
* **Roll** (`0x8002ca5c(player, vol, pitch)`): three looping sounds; per layer MIDI volume
  `vol / 127 * curve(vol)` with two-key Hermite curves (0 -> 127, constant 127, 0 -> 127) at `0x801b3998`,
  and a 14-bit pitch bend from the curve 4608 -> 16383 at `0x801b3a04`. `roll_layers` implements this with
  MusyX's default bend range of two semitones (no macro on the disc changes it). `vol` and `pitch` are
  computed as in the reference except that the volume tops out at 127 (original) instead of 100.
* **Music**: background id -> stream table at `0x801101dc` and the stream table at `0x801b2a5c` (name,
  volume byte, length; even = intro, odd = loop):

  | Background | Reference name | Stream | Volume |
  |---|---|---|---|
  | `bg_jun` | jungle | `st1` (no intro) | 140 |
  | `bg_wat` | water | `st2_all` + `st2_lp` | 150 |
  | `bg_nig` | mall | `st3_int` + `st3_lp` | 155 |
  | `bg_sun` | sky | `st4_int` + `st4_lp` | 135 |
  | `bg_spa` | extra | `st5_int` + `st5_lp` | 150 |
  | `bg_snd` | desert | `st6_int` + `st6_lp` | 150 |
  | `bg_ice` | arctic | `st7` (no intro) | 145 |
  | `bg_stm` | storm | `st8_int` + `st8_lp` | 160 |
  | `bg_bns` | bonus | `stb_int` + `stb_lp` | 145 |
  | `bg_mst` | master | `stm_int` + `stm_lp` | 120 |
  | `bg_ending` | ending | `end_int` + `end_lp` | 145 |

  The volume byte is the stream volume out of 255 and is applied as the track's gain.

Every sound in the table above resolves to an audible sample in `allse` or `comn`; none is missing
(`cargo test -p mb-audio` checks the ids, velocities and that all 72 request ids of the reference's
`smb1_sfx_ids.ts` resolve).

## Memory and load time

Measured on the USA disc (Windows 11, release builds; browser = the desktop app's built-in Chromium).

| | 16-bit PCM | Browser (`AudioBuffer`, float) | Load |
|---|---|---|---|
| 35 sound effects | 1.74 MB | 3.48 MB | desktop 5 ms; browser 7..11 ms (2 ms of it filling buffers) |
| Music `st1` (65 s) | 12.4 MB | 24.9 MB | desktop 15 ms; browser 29..39 ms (5..8 ms filling buffers) |
| Largest track, `st2_all` + `st2_lp` (7.7 MB of streams) | 26.8 MB (computed) | 53.7 MB (computed) | not measured |

Music is decoded whole at stage load, at its native 48 kHz, on the game thread, inside the stage load that
already takes longer; only the current track is kept (the previous one is released first in the browser).
On desktop the track is held as 16-bit PCM for the mixer. In the browser it never exists as PCM: the backend
is offered the disc's streams (`AudioBackend::define_music_streams`) and decodes them 1170 frames at a time
(`mb_audio::adp::AdpDecoder`) straight into the `AudioBuffer`, so wasm memory, which cannot shrink again,
holds one stream (at most 4.9 MB, handed over without a copy by `Content::read_once`) and a 131 KB block.
That took the wasm memory during Beginner from 32 MB to 22 MB in 0.1.0 (19.1 MB in 0.2.0, for other
reasons: `docs/WEB.md`, "Measurements"). The samples are the same as a whole decode
(`decoding_in_pieces_gives_the_same_samples`), so the intro-to-loop hand-off on the audio clock is unchanged.
During play nothing is decoded: `buffersMade` and `lastBufferAt` of `audio_debug_json` do not change.

Not done, on purpose: holding the music as 16-bit in the browser (an `AudioBuffer` is float by
specification), streaming the loop in segments (buffer boundaries at 48 kHz do not fall on output samples of
a 44.1 kHz context, so a seam could click; it could not be verified sample-exact here), or lowering the rate.

## `mb-audio` formats (unchanged facts)

### Music: `test/snd/adp/*.adp` (112 files)
* 32-byte frames, always stereo, no header; trailing all-zero padding frames are dropped (`decode_adp`).
* Frame: `hdrL hdrR hdrL hdrR`, then 28 bytes; byte `n` holds sample `n` of the left channel in the low
  nibble and of the right in the high nibble. `filter = hdr >> 4`, `range = hdr & 15`; CD-XA style
  prediction with pairs `(0,0) (60,0) (115,-52) (98,-55)`, history kept with 6 fractional bits, unclamped.
* 48000 Hz nominal. All 112 files decode bit-identical to ffmpeg's `adpcm_dtk`.
* Naming: `X_int` + `X_lp`, `X_all` + `X_lp` (`st2`, `sel2`), or a single looping `X`; a trailing `m` is the
  mono mixdown. `music::catalog` groups stems; `music::stage_music` is the per-background table above.

### Sound effects: `test/snd/mkb/<bank>.{proj,pool,sdir,samp}` (MusyX)
Nine banks in group order: `allse`(0) `bil` `bowl` `golf` `race` `target` `fight` `comn`(7) `ending`; SFX
ids tile `0x000..=0x367`. The main game uses `allse` and `comn` only.
* `.proj`: groups `[u32 size][u16 id][u16 type]` + 8 offsets; SFX table at header+0x14: 10-byte entries
  `u16 sfx_id, u16 macro_id, u8 priority, u8 max_voices, u8 velocity, u8 pan, u8 key, u8 pad`.
* `.pool`: macros of 8-byte commands (opcode = low byte of word 0). Seen: `0x00` end, `0x05` loop, `0x07`
  wait, `0x10` start sample, `0x11` stop sample, `0x14`, `0x19`, `0x31`, `0x38`.
* `.sdir`: 32-byte sample entries (id, offset, root key, rate, format + length, loop) and 40-byte DSP-ADPCM
  parameter blocks. `.samp`: DSP-ADPCM, 8-byte frames of 14 samples.
* Every sample decodes; loop histories match the decoded data, which pins the decoder down bit-exactly.

## Tests

* `cargo test -p mb-audio`: mixer (resampling, loops, intro -> loop hand-off sub-sample exact, ramps, fades,
  pause, muted bus keeps time, rate change, no reallocation), the queue driver (ordering, loop slots, music,
  clamping, full queue, keeping time without a device), the sound table reader; with a disc: all ADP and
  bank checks, the sound table against the real `main.dol`, the stage music files.
* `cargo test -p mb-app`: `audio.rs` unit tests (batching, roll layers and curves, muting, music changing
  only with the track, `MixBackend`), `audio_cpal` (channel folding; opens whatever device the machine has),
  and `tests/audio.rs` with a disc: the autopilot plays Beginner floor 1 with a recording backend (request
  sequence) and with the software mixer rendering offline (levels). Nothing is written to disk unless
  `MB_AUDIO_OUT` names a directory outside the repository; then the mix is saved there as WAV files for
  listening.

## Known gaps

* Nobody has listened to the result in this workstream: correctness was established by decoding checks,
  the executable's tables, request sequences and signal measurements. Balance is the original's
  velocities and stream volumes under the reference's default category volumes (50 / 30 / 30).
* MusyX macros are reduced to "play the sample" (looping when the macro loops): the `0x14` and `0x38`
  commands (volume / envelope selection) and `0x19` are not interpreted, so attack and release shapes may
  differ from the original.
* Not played: the monkey's voices (the original adds one to most events; the reference has none), the
  softest impact tier (`0x12`, below the reference's threshold), the time-over explosion (`0x128`), the
  party-ball and goal-replay sounds, 1UP, continue / game-over lines and jingles, menu music, minigame
  banks. All of these are in the original game only: the reference plays none of them, so they are outside
  parity and not planned (`docs/PLAN.md`, "Goal"). Every call the reference's
  `audio.ts` makes for SMB1 has its sound here ("What plays when"); its `playAnnouncerTimeBonus` is empty.
* The announcer's `GO` has one take (the reference picks one of two files of its own at random).
* The "FALL OUT" call is played when the fall-out starts; the original ties it to its banner sprite's
  timer, which was not traced.
* Panning is supported by every backend but nothing pans yet (single player, as the reference).
* The resampler is linear interpolation; fine for these sources, not a band-limited design.
* Other regions' discs: sound ids come from their own `main.dol`; the music table and the roll curves are
  the USA executable's.
