# What is on the SMB1 (USA, GMBE8P) disc

Observed with `xtask disc-ls` / `disc-cat` on a USA RVZ. 897 files; everything except `opening.bnr` is under `test/`.
The web build's `smb1_content/test/…` is exactly this `test/` folder.

| Disc folder | Files | Contents (as far as verified) |
|---|---|---|
| `test/stNNN/` | 555 | per-stage `STAGENNN.lz` (stagedef), `stNNN.gma`, `stNNN.tpl`, and more |
| `test/init/` | 35 | `common.gma/.tpl`, `common_p.lz`, `common.lz`, minigame models (`m_*.gma/.tpl`), `roll`, `pattern`, ... |
| `test/bg/` | 46 | background models/textures (`bg_*.gma/.tpl`) |
| `test/bmp/` | 15 | HUD/UI textures: `bmp_com.tpl`, `bmp_nml.tpl`, `bmp_adv`, `bmp_rnk`, ... |
| `test/spr_bg/` | 7 | `bg_*.gct` menu backgrounds |
| `test/snd/adp/` | 112 | music streams, `*.adp` (`*_int` intro + `*_lp` loop pairs, plus `m` variants) |
| `test/snd/mkb/` | ~44 | sound-effect banks: `.proj/.pool/.sdir/.samp` per group (`allse`, `comn`, `bil`, `bowl`, ...) |
| `test/ape`, `kid_*`, `gor_*`, ... | ~40 | character models/textures |
| `test/recdata`, `preview`, `stage`, `common` | | replay data, previews, shared model sets |

## Mapping to what the web build loads
- **Stages / models / textures**: read directly (Phase 2 parsers). Verified: stage-1/2 files present, all stagedefs
  decompress to their declared size, output byte-identical to the reference decoder on stages 1, 2, 50.
- **HUD**: the web build's `assets/hud/smb1/bmp_com|bmp_nml/NNN_name.png` match the disc's `bmp_com.tpl` and
  `bmp_nml.tpl`. These are TPL image containers (header differs from standard TPL; the reference parses them with
  `parseAVTpl`). So HUD sprites can be decoded in the app from the disc, no PNG files needed.
- **Music**: `snd/adp/*.adp`. The first bytes show 32-byte frames with a repeated 4-byte header, consistent with
  GameCube streaming ADPCM. Exact layout to be confirmed in Phase 3. The web build ships these as `.ogg`.
- **Sound effects**: `snd/mkb/*.proj|.pool|.sdir|.samp` look like the MusyX sound engine's project/sample banks.
  The web build uses `.wav` by id. Needs a MusyX bank reader (Phase 3, largest unknown).

None of this data is stored in the repo; these notes describe formats only.
