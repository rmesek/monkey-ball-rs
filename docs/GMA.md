# GMA / NaomiLib model parsing (`mb-gma`)

Std-only crate; input is **decompressed** bytes (use `mb_assets::lzss` for `.lz`), output is plain CPU data a
renderer can upload. Port of WebMonkeyBall `src/gma.ts`, `src/gx_displaylist.ts`, `src/gx.ts` and the data parts of
`noclip/SuperMonkeyBall/{Gma,NaomiLib,Material,TevLayer,Shape,Model}.ts`.

## Files
- GMA: `stNNN/stNNN.gma`, `init/common.gma`, `bg/bg_*.gma`, `init/m_*.gma`, `*.gma.lz` (LZSS-compressed GMA).
- NaomiLib: `init/common_p.lz`, `stNNN/stNNN_p.lz`, `bg/bg_*_p.lz`, ... (decompress, then `parse_naomi_obj`).
- Textures are not handled here: `TevLayer::tex_idx` / `NlMesh::tex_idx` index the companion TPL (`mb-tpl`).

## GMA layout (big endian)
- Header: `u32 count`, `u32 gcmf_base`; `count` entries of `(i32 gcmf_offs, i32 name_offs)`; names (NUL terminated)
  between the table and `gcmf_base`. Entries with `gcmf_offs < 0 && name_offs <= 0` are empty.
- `GCMF` model: flags `+4`, bound sphere `+8..0x18`, tev layer count `+0x18` (i16), opaque / translucent shape counts
  `+0x1a/+0x1c`, matrix count `+0x1e` (i8), `texMtxSize` `+0x20` (offset of the first shape). Then 0x20-byte TEV
  layers, 0x30-byte 3x4 matrices, shapes.
- Shape: 0x60-byte material/header (flags, colors, alpha `+0x11`, tev layer count `+0x12`, dlist flags `+0x13`, tev
  layer idxs `+0x16`, `vtx_attrs` `+0x1c`, dlist sizes `+0x28/+0x2c`, origin `+0x30`, blend factors `+0x40`) followed by
  display lists: front-culled (flag 1), back-culled (flag 2), and an optional 0x20-byte "extra" header with two more
  lists (flags 4/8). `DoubleSided` materials get `CullMode::None` on the first two.
- Display list: GX command stream. `cmd & 0xf8` = primitive, `cmd & 7` = VAT (0 = float32, 1 = 16-bit fixed point,
  shift 13), `u16` vertex count, then inline vertices holding the attributes selected by `vtx_attrs` in GX order
  (matrix indices, POS, NRM, CLR0, CLR1, TEX0..7). Padding is `0x00` (NOOP).

## Public API
```rust
let gma = mb_gma::parse_gma(&bytes)?;         // Gma { models, skipped }
let m = gma.model("STG001")?;                  // by name (last wins) / gma.model_by_id(i) / gma.iter()
for mesh in m.meshes()? {                      // one Mesh per shape display list
    let shape = &m.shapes[mesh.shape_index];   // shape.material: flags, colors, alpha, vtx_attrs, blend...
    // mesh.vertices: positions/normals/colors(u8 RGBA)/uv0/uv1/uv2 ; mesh.indices: triangle list (u32)
    // mesh.cull, mesh.translucent ; m.shape_tev_layers(shape) -> texture layers (tex_idx, wrap, lod)
}
let obj = mb_gma::parse_naomi_obj(&bytes)?;    // NlObj { models, names } ; model(id) / model_by_name(name)
```
Lower level: `decode_display_list(data, vtx_attrs)`, `Primitive::triangulate`, `triangulate(prim, n)`,
`Material::{src_blend,dest_blend}`, `TevLayer::{wrap_s,wrap_t,max_lod,mag_filter_nearest}`, flag modules
(`material_flags`, `tev_layer_flags`, `model_flags`, `attr`).

Behaviour notes (all as in the reference):
- Stitching / skin / effective models are not decoded; they are listed in `Gma::skipped`. Models with zero shapes are
  dropped. Shapes with NBT normals (`attr::NBT`) are dropped (`Model::skipped_nbt_shapes`); `decode_display_list` can
  still read them (first of the three vectors is the normal).
- Winding: strips alternate, fans pivot on vertex 0, quads split `(0,1,2),(0,2,3)`, quad strips `(i,i+1,i+2),(i+1,i+3,i+2)`.
- Missing attributes default to normal +Z, color white, uv 0; `uv1`/`uv2` copy `uv0` if the material lacks TEX1/TEX2.
- Vertices are not deduplicated across primitives; strip/fan/quad sharing is expressed through the index buffer.
- The VAT is chosen per draw command (`cmd & 7`), as the game's noclip loader does; the reference's `gx_displaylist.ts`
  uses one flag per model (`Vat16Bit`) which yields identical results on the retail data.
- The original data contains a few NaN normals (degenerate faces); positions are always finite.

## NaomiLib
Table of absolute model offsets at byte 4 (0-terminated); `-1` header marks an invalid model. Model: flags (bit 0 = vertex
type A: pos/color/uv, else type B: pos/normal/uv), bound sphere, mesh list (until `flags == 0`). Mesh: material at
`+0x08..0x3c`, tex index `+0x20` (negative = none), `u32 size` at `+0x4c`, display lists from `+0x50`: `u32 flags`
(bits 0-1 cull, 2 quads, 3 triangles, 4 strip), `u32 count`, 0x20-byte vertices. A vertex whose x-float LSB is clear is an
8-byte relative reference to an earlier/later real vertex. Cull comes from the first list; all lists of a mesh are merged
into one vertex/index buffer.

## Corrupt input
Nothing here panics on bad bytes; every failure is an `Error`.
- Decoded size is capped per input byte (GMA 8, NaomiLib 16, plus 64 KiB; retail maxima 1.3 and 3.5), because
  table entries are offsets and thousands of them can point at one model or one name:
  `Error::Invalid("entries overlap: ...")`.
- A display list is rejected when its vertices carry no position (`Error::Invalid`: a vertex could then be
  zero bytes long and one 3-byte command would expand to 65535 vertices) or when a draw command claims more
  vertices than the buffer holds (`Error::OutOfBounds`, before anything is decoded).
- Offsets are validated before fixed fields are added to them, so nothing wraps on 32-bit targets (wasm32).

## Tools / tests
- `tests/robust.rs`: mutation campaigns (truncation, single bytes, hostile words, seeded random corruption) on
  synthetic GMA / NaomiLib / display-list inputs under a counting allocator, repeated on real archives when a
  disc is present. `tests/golden.rs`: a hash over every parsed model and decoded mesh of the disc, recorded
  before the hardening.
- `crates/mb-gma/tests/disc.rs`: parses every GMA (incl. `.gma.lz`) and NaomiLib archive on the disc; compares triangle
  counts and position checksums against the TypeScript reference (via `deno`, `tests/ref/gma_ref.ts`) for 9 files.
  Env: `MB_DISC`, `MB_REF`, `MB_SCRATCH`.
