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

## Corrupt input
The disc reader does not verify hashes, so any bytes can arrive here. Every read is bounds-checked and returns
`StageError`; nothing panics. Because lists are found through pointers, a corrupt file can make thousands of
records share one long list (decoded size growing with the square of the file size), so the parser keeps a
budget of 24 decoded bytes per input byte plus 64 KiB (the retail maximum is 3.4) and fails with
`StageError::Invalid` once it is spent. `tests/robust.rs` runs truncation at every length, every single-byte
change, hostile words at every offset and 20,000 seeded random corruptions under a counting allocator, and
repeats the campaign on real stagedefs when a disc is present; `tests/golden.rs` pins the parse of all 200
retail stagedefs to a hash recorded before the hardening.

## Not implemented
SMB2 stagedefs (detected via magic `0, 0x447a0000|0x42c80000`) return `StageError::Unsupported`. Fog,
texture-scroll and wormhole/switch data are SMB2-only in the reference and are not parsed.

## Who reads what
Collision, goals, bumpers, jamabars, bananas and animations are read by `mb-engine`; the model lists and
the background and foreground objects with their flags and animations by `mb-app/src/stage_scene.rs`.
Since version 0.2.0 the flipbook records of background objects (`mb-app/src/backdrop.rs`) and the mirror
names (`mb-app/src/mirror.rs`) are read as well. The four header words marked unknown are read by nothing.
