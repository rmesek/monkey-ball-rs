# Asset loading (mirrors the web build)

The reference serves content as plain static files and loads them with `fetch`. There is no encryption, token,
service worker or browser storage layer. The same design is used here, with the content root supplied by the user.

## Content root layout (SMB1)
```
<content root>/
  smb1_content/test/
    init/common.gma  common.tpl  common_p.lz  common.lz
    bg/<bg_name>.gma  <bg_name>.tpl
    stNNN/STAGENNN.lz  stNNN.gma  stNNN.tpl   (+ stNNN_p.lz, stNNN.lz for Naomi stages)
  audio/sfx/<name>.wav            (or 0x<id>.wav)
  audio/smb1-music-compressed/<track>[_loop].ogg
  assets/hud/smb1/{bmp_com,bmp_nml}/*.png
```
SMB2 and MB2WS use `smb2_content/test` and `mb2ws_content/test`; SMB2 music is in `smb2-music-compressed`.

## Request order for a stage (observed on the live build)
1. `stNNN/STAGENNN.lz` (stagedef, LZSS)
2. `init/common_p.lz`, `init/common.lz`
3. `stNNN/stNNN.gma`, `stNNN.tpl`
4. `init/common.gma`, `init/common.tpl`
5. `bg/<bg>.gma`, `bg/<bg>.tpl`
6. background music for the stage
7. prefetch of the next stage's files

## Rust design
- `GameSource` + `stage_files()` in `mb-assets::layout` give the per-stage path list.
- A pack-provider trait (`fetch(path) -> bytes`) with backends: directory, zip, HTTP base URL.
- A cache keyed by normalized path with in-flight dedupe, matching the reference's `fetchWithCache`.
- `xtask check-content <root> <stage> <bg>` lists missing files.
- Android: app-private directory or a user-picked folder. Web: picked folder / OPFS / HTTP base.

## Not provided by this project
The files themselves. Obtain them lawfully and place them in the content root.
