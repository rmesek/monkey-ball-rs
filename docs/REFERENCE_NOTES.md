# Reference notes (WebMonkeyBall, read-only)

## Layout
`src/` game + parsers, `src/noclip/` vendored renderer and Super Monkey Ball scene code, `tools/` Python pack
builders (SMB2), `worker/` Cloudflare lobby (multiplayer signalling only, unrelated to assets).

## Formats (big-endian GameCube data)
- **LZSS `.lz`**: 8-byte little-endian header (compressed size, decompressed size); 4096-byte ring buffer, start
  position 4078; 8-bit flag groups. (`src/lzs.ts`)
- **Stagedef**: header of offsets/counts to record tables. Record sizes: triangle 0x40, goal 0x14, bumper 0x20,
  jamabar 0x20, banana 0x10, cone 0x20, sphere 0x14, cylinder 0x1c, fallout box 0x20, bg object 0x38, start 0x14,
  anim-group model 0x0c, anim group 0xC4 (SMB1) / 0x49C (SMB2). SMB2 is detected by magic values.
  (`src/stage.ts`, `src/noclip/SuperMonkeyBall/Stagedef.ts`)
- **Collision**: triangle lists with a spatial grid inside the stagedef; queries in `src/collision.ts`.
- **GMA/TPL**: models with GX display lists and texture containers (RGB565, RGB5A3, CMPR, ...). (`gma.ts`, `tpl.ts`)
- **NaomiLib**: arcade-style object format used by some SMB1 stages and common models.
- **pack.json**: `id`, `name`, `gameSource`, `version`, `basePath`, `content{stages,stageNames,stageTimeOverrides}`,
  `courses{challenge,story}`, `stageEnv{bgInfo,fog}`. (`src/pack.ts`)

## Engine
Decomp-faithful physics using S16 angles and lookup tables (`math_lut.ts`), deterministic stepping, sim hashing,
rollback and WebRTC netplay. Audio via the browser decoder from wav/ogg. HUD from PNG sprites.

For SMB1 it draws the ball's shells and no monkey, particles (`effects.ts`), confetti, a projected ball
shadow, three animated backgrounds (night, storm, sunset; the other classes of `Background.ts` only draw
their objects), the bonus wave and mirror floors, and it replays a fall-out and a goal from its rollback
states (`game.ts`, `startResultReplay`). Which of that is ported is in `docs/PLAN.md`.

## What this port does differently
The reference is given extracted files (stages, models, `.ogg` / `.wav` audio, HUD PNGs) at fixed paths. This
port reads everything from the user's own disc image at run time: HUD sheets from `bmp_*.tpl`, music from the
`.adp` streams, sound effects from the MusyX banks (`docs/DISC_CONTENT.md`). For SMB2 the reference's
`tools/smb2_pack_builder.py` derives stage environment data from `mkb2.main_loop.rel` with hard-coded US
NTSC offsets; nothing of SMB2 is ported here.

Two things in the reference do not work on the SMB1 data as it stands, and the port does not copy the
fault: its post-goal sparkle loads `assets/particle/beautifulstar.png`, which is neither in its checkout nor
on the disc, and its sunset cloud scroll needs a UV-scroll flag that no layer on the disc has. Both are
described where the port's version is (`docs/GAMEPLAY.md`).
