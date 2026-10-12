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

## How `mb-render` draws the materials
`crates/mb-render/src/material.rs` (CPU mapping, unit-tested) and `shader.wgsl` follow
`noclip/SuperMonkeyBall/Material.ts`. One shader; a pipeline differs only by blend factors, cull mode, depth
state and how many layer textures it fetches.

- **Lighting channel**, per vertex as on GX: lit materials get `mat * clamp(amb + light * max(n.l, 0))`,
  unlit ones the material register or the vertex colour (`VERT_COLORS`). The material colour applies only
  with `CUSTOM_MAT_AMB_COLORS` or `SIMPLE_MATERIAL`, else white; alpha is `material.alpha` times the
  object's. The object colour (`colorMul`) multiplies the material register and, for GMA materials, the
  ambient register too.
- **Layers** (at most three; layer `i` uses UV set `i` and texture slot `i`): diffuse `prev * tex`, alpha
  blend `prev.a * tex.r`, view specular `clamp(prev + K0 * tex)`, world specular `clamp(prev + K1 * tex)`.
  `K0` is the specular colour (white when it is black), `K1 = K0 *` the light's specular scale
  (`Lighting::specular_scale`, the light colour in the reference). The reference's second world-specular
  stage reads a 16x4 ramp at `t = 0`, where it is white, so the ramp is the constant one.
- **Specular coordinates** are generated per vertex from the normal: `TEXMTX0` (model-view without
  translation), normalised, then the x and y rows of a look-at: around the direction from the eye to the
  far side of the model's bounding sphere (view), or around the half vector of that and the light (world).
  `material::{view_specular_rows, world_specular_rows, specular_uv}` are the same maths on the CPU.
- **Texture matrix**: layers flagged `ENABLE_UV_SCROLL` (bit 17) go through the object's matrix
  (`Renderer::set_tex_matrix`, GX `TEXMTX1`), the others through the identity. **No layer on the SMB1 disc
  has the flag** (all 12 088 shapes of its GMA files were scanned), so the reference's sunset cloud scroll does nothing on
  this data; `Renderer::set_tex_matrix_every_layer` applies the matrix to every diffuse and alpha layer
  of an object instead.
- **Per object**, as in the reference's `RenderParams`: alpha, colour, texture matrix, specular off
  (`set_specular_disabled`, its hack for the inner ball shells), world specular off
  (`set_world_specular_disabled`, its background light groups), sort offset (`set_depth_offset`).
- **State**: alpha test `> 0`, depth LEQUAL with writes, blend factors from the material. A mesh whose
  alpha is provably one is drawn without blending only when its factors are the default ones (or
  `One / Zero`): the several hundred `One / One`, `SrcAlpha / One`, `SrcClr / One` and `Zero / InvSrcClr`
  materials on the disc blend whatever their alpha.
- **Counts on the USA disc** (shapes): layers `D` 8322, `DV` 1230, `simple` 1082, `DW` 691, `W` 251 (the
  ball's hemispheres among them), `A` 160, `DA` 126, `DD` 109, `V` 83, others 34.

Where this still differs from the reference:

| | Reference | Here | Why |
|---|---|---|---|
| Vertex-colour materials and the object's alpha / colour | the vertex colour is the material source, so `RenderParams.alpha` and `colorMul` do not reach it (Material.ts 318-352) | multiplied in as for every other material | a fade or tint set on an object would silently do nothing on 695 shapes |
| NaomiLib V coordinate | `t' = -t` (NaomiLib.ts 73, 543) | `1 - t`, and `-t` under a mirrored wrap | identical under Repeat; `-t` would smear the 213 clamped meshes authored in 0..1 |
| NaomiLib lit meshes, alpha | the alpha channel is lit too (NaomiLib.ts 458) | material alpha | these meshes do not blend; it only matters at alpha zero |
| Instanced draws, specular | per model bounding sphere | the instance's origin, radius zero | the sphere is per object; no instanced model on the disc has a specular layer that matters |
| Opaque order | submission order | meshes without blending sorted by pipeline and material | invisible with the depth test; saves state changes |
| `lod_bias`, `max_aniso`, `MAGFILT_NEAR`, `NO_FOG` | ignored (no fog at all) | ignored | same |

### Mirror floors
`crates/mb-render/src/mirror.rs`, `mirror.wgsl`, the mirror parts of `renderer.rs`; reference `Render.ts`
("Mirror Capture", "Mirror Overlay") and `World.ts` `prepareToRenderMirrors`. The caller sets one
`Mirror` per stage (`Renderer::set_mirror`: a plane as a matrix, an alpha, optionally the wavy
distortion), flags the mirror models (`set_mirror_surface`) and takes objects out of the reflection
(`set_in_mirror`). Without a mirror none of this exists: no target, no pass, no pipeline, and a frame has
the draws, uploads and GPU objects it had (`tests/mirror.rs`, `without_a_mirror_a_frame_is_what_it_was`).

A frame in which a mirror surface is in view:

1. **Capture.** The background and world passes once more into the mirror's target, with the view
   `flipX * view * Reflect(plane as shown)` and the frame's projection. `flipX` undoes the reflection's
   change of winding, so every pipeline is used as it is. Drawn: opaque, the opaque instance batches
   (confetti) and the sorted objects, sorted again from the reflected eye; the shadow term stays.
   Skipped: mirror surfaces, objects with `set_in_mirror(false)`, objects whose bounding sphere is
   wholly behind the plane (the reference's test in `Model.ts`) or outside the reflected view, world
   quads, the other instance batches and sprites.
2. **The picture**, unchanged, mirror surfaces included with their own material.
3. **Overlay**, after the opaque geometry and before the sorted objects: every mesh of each mirror
   surface in view again with the overlay program, both faces, depth tested without writing, blended
   `SrcAlpha / InvSrcAlpha` with `alpha = mirror alpha * vertex alpha`. It shows the capture at
   `uv = clip.xy / w * 0.5 + 0.5` (clamped) of the fragment's position through the capture's clip
   matrix.

The wavy mirror (stage 92) adds the reference's indirect offset to that coordinate:
`(dot(ind0, c), dot(ind1, c)) / 256` with `c = (255 * (1, blue, green) & 0xF8) - 128` of `gb_grad`
addressed by `xz * 0.5 + 0.5` of the view-space normal, `ind0 = (0, 0.8, 0)`, `ind1 = (0.6, 0, 0)`
(`Distortion`, the same formula on the CPU). The first component is one because the reference reads the
alpha of its distortion target, which it always writes as one: the offset is `0.8 * (blue - 128) / 256`
sideways and a constant `0.28` of the picture upwards.

Cost: the capture's draws (at most those of the frame less mirrors and effects, usually fewer: nothing
below the plane) plus one per mesh of a mirror surface, reported as `FrameStats::mirror_draws` beside
`draws`; three small uniform uploads (880 bytes) in a frame whose camera moved. Made by `set_mirror`,
at load: three uniform buffers, three bind groups, the target's two textures and, once per run, one
pipeline; a resize of the frame replaces the two textures and one bind group. The target has the frame's
colour format plus `Depth32Float` at `set_mirror_scale` (default 0.5) of the frame's side: 8 bytes per
pixel, 1.84 MB under a 1280 x 720 frame and 4.15 MB under 1920 x 1080, reported by
`Renderer::mirror_bytes` and not part of `texture_bytes`. Nothing is created in a play frame, also
while the plane moves (`set_mirror_plane`).

Where it differs from the reference:

| | Reference | Here | Why |
|---|---|---|---|
| Cut at the plane | only the per-model sphere test, so a model that reaches through the mirror is reflected whole | also an oblique near plane in the capture's projection, 0.005 above the plane; the plain projection when the eye is behind the mirror | nothing from below a floor belongs in it; costs nothing per fragment and needs no clip distance (WebGL2 has none) |
| Target | fixed sizes per stage (640 x 448 down to 160 x 112), `RGBA8` and `D24` | a fraction of the frame, the frame's colour format, `Depth32Float` | the scene's pipelines are compiled for the frame's format; a second set would double them |
| Wavy mirror: the distortion | the waved floor drawn with `gb_grad` into a 256 x 256 target, read back at the fragment's place on the screen | one fetch of `gb_grad` in the overlay's fragment shader | the same texel for every fragment of the floor, without a target and a pass |
| Wavy mirror: the overlay's geometry | the floor at rest, so the crests of the waved floor hide the reflection | the waved floor (the wave function of the stage shader), looked up at the fragment's own place on the screen | what the reference shows in the troughs, on the whole surface; needed for the row above |
| What does not tilt (backdrop, ball) | drawn in the capture with the stage tilt applied to it | reflected in the mirror as it is shown | the picture does not tilt them either |
| No mirror model in view | the capture is drawn all the same | no capture | saves the pass |
| Overlay | writes the frame's alpha | colour only, and a depth bias of one step | the frame stays opaque; the surface was drawn at the same depth by another program |

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
