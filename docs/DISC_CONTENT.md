# What is on the SMB1 (USA, GMBE8P) disc

As listed by the project's own disc reader (`mb_assets::disc::DiscPack::list`) on a USA RVZ: 897 files, all
under `test/` except `opening.bnr`; the executable is exposed as `sys/main.dol`. None of this data is stored
in the repository; these notes describe where things are and what the game reads.

| Disc folder | Files | Contents | Read by the game |
|---|---|---|---|
| `test/stNNN/` | 555 | per stage: `STAGENNN.lz` (stagedef), `stNNN.gma`, `stNNN.tpl`; NaomiLib stages also `stNNN_p.lz` and `stNNN.lz` | the 109 course stages |
| `test/init/` | 35 | `common.gma/.tpl`, `common_p.lz`, `common.lz` (ball, bananas, goal, clock digits); minigame models | the four `common` files |
| `test/bg/` | 46 | background models and textures (`bg_*.gma/.tpl`) | the backgrounds the courses use |
| `test/bmp/` | 15 | HUD / UI texture sheets (`bmp_com.tpl`, `bmp_nml.tpl`, ...), a TPL variant | `bmp_com`, `bmp_nml` |
| `test/snd/adp/` | 112 | music streams, DTK ADPCM (`*_int` + `*_lp` pairs, `m` = mono mixdown) | the 10 stage tracks |
| `test/snd/mkb/` | about 44 | MusyX sound banks, `.proj/.pool/.sdir/.samp` per group | `allse`, `comn` |
| `sys/main.dol` | 1 | the executable | trig tables, sound table |
| `test/spr_bg/`, `test/ape`, `kid_*`, `gor_*`, `test/recdata`, `test/preview`, ... | | menu backgrounds, character models, replay data, previews | not read |

The exact list of what the game reads is code: `mb_app::content::all_files()` (382 files, 144.1 MB), built
from the per-stage list `StageFiles`. A test checks that an imported pack contains exactly that and that
every stage loads from it without reading anything else (`crates/mb-app/tests/pack.rs`).

Formats: `docs/STAGEDEF.md`, `docs/GMA.md`, `docs/TPL.md`, `docs/AUDIO.md`.
