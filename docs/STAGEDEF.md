# SMB1 stagedef layout (as parsed by `mb-stage`)

Input: decompressed `STAGENNN.lz`. Big-endian, pointers are absolute file offsets (0 = NULL). Mirrors
`WebMonkeyBall/src/stage.ts` `StageParser`; record sizes are the `pub const *_SIZE` in `mb-stage/src/parse.rs`.

## Header
| Off | Field |
|---|---|
| 0x00/0x04 | loop start/end seconds (i32) |
| 0x08/0x0c | anim group count / ptr (records of 0xc4) |
| 0x10 | start positions ptr (count = (fallout_ptr - start_ptr) / 0x14, min 1) |
| 0x14 | fallout ptr (f32 Y) |
| 0x18/0x1c | goals (0x14) |
| 0x28/0x2c | bumpers (0x20), 0x30/0x34 jamabars (0x20), 0x38/0x3c bananas (0x10) |
| 0x40/0x44 cones (0x20), 0x48/0x4c spheres (0x14), 0x50/0x54 cylinders (0x1c) |
| 0x58/0x5c | anim-group models (0x0c) |
| 0x68/0x6c, 0x70/0x74 | bg / fg objects (0x38; anim 0x60, flipbooks 0x10) |
| 0x80/0x84 | mirror names (8-byte entries, name ptr first) |
| 0x78, 0x7c, 0x88, 0x90 | unknown, exposed raw |

## Anim group (0xc4)
0x00 pos, 0x0c rot, 0x12 loop type (0 loop, 1 once, 2 seesaw), 0x14 anim header (6 tracks of
`count,ptr`, keyframes 0x14), 0x18 name list, 0x1c triangles (0x40), 0x20 grid cell table, 0x24..0x38 grid
origin x/z, step x/z, cell counts x/z, then per-group counts/ptrs for goals (0x3c), bumpers (0x4c), jamabars
(0x54), bananas (0x5c), cones (0x64), spheres (0x6c), cylinders (0x74), models (0x7c), fallout boxes (0x84),
mirror names (0x8c). The per-group lists are the stage-wide lists split between groups (verified on all discs
stages: sums match).

The triangle count is not stored: it is the highest index referenced by any grid cell plus one. Grid cells are
`ptr` to an `i16` list terminated by a negative value; NULL = empty. Cell (x, z) = `cells[z * count_x + x]`.

## Not implemented
SMB2 stagedefs (detected via magic `0, 0x447a0000|0x42c80000`) return `StageError::Unsupported`. Fog,
texture-scroll and wormhole/switch data are SMB2-only in the reference and are not parsed.
