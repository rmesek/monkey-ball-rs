# TPL (AV texture container) — `mb-tpl`

Ports `WebMonkeyBall/src/tpl.ts` + `noclip/SuperMonkeyBall/AVTpl.ts`. Std-only; takes decompressed bytes
(`.tpl.lz` is handled by `mb_assets::lzss` in the caller).

Layout (big-endian): `u32 count`, then `count` x 0x10-byte headers
`{u32 format, u32 data_offset, u16 w, u16 h, u16 mips, u16 magic=0x1234}`. Entries with w=h=mips=0 are empty
(index gap). Mip levels are stored back to back at `data_offset`, each padded to whole tiles, sizes halving (min 1).

Formats decoded (as in the reference): I4, I8, IA4, IA8, RGB565, RGB5A3, RGBA8, CMPR. C4/C8/C14X2 are
recognised but return `UnsupportedFormat` (the reference returns null; no palettes in AV TPLs; none on the disc).

Fidelity notes: colour maths uses f64 then truncation (JS `Uint8Array` store semantics), CMPR interpolates the
un-truncated floats, and textures smaller than a tile clip like the reference (out-of-range writes dropped).

Corrupt input (the disc reader verifies no hashes): nothing panics, and a header alone cannot make the
decoder allocate. All entries share one copy of the file (`Arc<[u8]>`). `decode_rgba` fails with
`BadDimensions` for a texture that is empty in one direction or larger than 1024 (`MAX_DIMENSION`, the GX
limit, below what WebGL2 guarantees), and with `Truncated` when the file does not hold the level (the
reference would read zeros). `level_count` never goes past the 1x1 level. `parse` fails with `Overlapping`
when the headers reference more than four times the file's size in texture data. `tests/robust.rs` covers
every format at hostile sizes and mip counts plus mutation campaigns; `tests/golden.rs` pins every decoded
level of the disc to a hash recorded before these checks.

`test/preview/*.tpl` on the disc are raw RGB565 frame dumps, not TPL containers.

Tests: `cargo test -p mb-tpl --release` (unit tests always; `tests/disc.rs` and `tests/golden.rs` need the
disc). Until 0.2.0 a Deno harness also compared the decoders with the reference's; `tests/golden.rs` holds
what it found equal.
