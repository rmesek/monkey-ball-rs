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

`test/preview/*.tpl` on the disc are raw RGB565 frame dumps, not TPL containers.

Tests: `cargo test -p mb-tpl --release` (unit tests always; `tests/disc.rs` needs the disc, the comparison also
needs `deno` and `../WebMonkeyBall`). Eyeball: `cargo run -p mb-tpl --example dump_tpl -- file.tpl [idx] [level] [out.ppm]`.
