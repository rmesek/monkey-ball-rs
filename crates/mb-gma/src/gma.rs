//! GMA ("GCMF") model container parser. Ports `gma.ts` and `noclip/SuperMonkeyBall/Gma.ts`.

use std::collections::HashMap;

use crate::gx::{self, attr, CullMode, DecodedList};
use crate::reader::Reader;
use crate::Error;

const SHAPE_BASE_SIZE: usize = 0x60;

/// `Material::flags` bits.
pub mod material_flags {
    pub const UNLIT: u32 = 1 << 0;
    pub const DOUBLE_SIDED: u32 = 1 << 1;
    pub const NO_FOG: u32 = 1 << 2;
    pub const CUSTOM_MAT_AMB_COLORS: u32 = 1 << 3;
    pub const CUSTOM_BLEND_SRC: u32 = 1 << 5;
    pub const CUSTOM_BLEND_DEST: u32 = 1 << 6;
    /// Synthesised: material has no TEV layers.
    pub const SIMPLE_MATERIAL: u32 = 1 << 7;
    /// Synthesised: material's attributes include CLR0.
    pub const VERT_COLORS: u32 = 1 << 8;
}

/// `TevLayer::flags` bits.
pub mod tev_layer_flags {
    pub const TYPE_VIEW_SPECULAR: u32 = 1 << 0;
    pub const TYPE3: u32 = 1 << 1;
    pub const DO_EDGE_LOD: u32 = 1 << 6;
    pub const MAGFILT_NEAR: u32 = 1 << 11;
    pub const TYPE_ALPHA_BLEND: u32 = 1 << 13;
    pub const TYPE_WORLD_SPECULAR: u32 = 1 << 15;
    pub const UNK16: u32 = 1 << 16;
    pub const ENABLE_UV_SCROLL: u32 = 1 << 17;
}

/// `Model::flags` bits.
pub mod model_flags {
    /// Display lists use VTXFMT1 (16-bit fixed point) for some commands.
    pub const VAT_16_BIT: u32 = 0x01;
    pub const STITCHING: u32 = 0x04;
    pub const SKIN: u32 = 0x08;
    pub const EFFECTIVE: u32 = 0x10;
}

/// GX wrap mode as stored in TEV layer flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapMode {
    Clamp,
    Repeat,
    Mirror,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    fn unpack(v: u32) -> Self {
        Self {
            r: (v >> 24) as u8,
            g: (v >> 16) as u8,
            b: (v >> 8) as u8,
            a: v as u8,
        }
    }
}

/// A model-level texture layer. `tex_idx` indexes the companion TPL (see `mb-tpl`).
#[derive(Clone, Debug, PartialEq)]
pub struct TevLayer {
    pub flags: u32,
    pub tex_idx: i16,
    pub lod_bias: i8,
    pub max_aniso: i8,
    pub unk_0c: i8,
    pub unk_10: i32,
    pub alpha_type: u8,
    pub color_type: u8,
    pub swappable: bool,
}

impl TevLayer {
    /// S wrap mode (flag bits 2-3). Raw values follow GX: 0 clamp, 1 repeat, 2 mirror.
    pub fn wrap_s(&self) -> WrapMode {
        wrap((self.flags >> 2) & 3)
    }
    /// T wrap mode (flag bits 4-5).
    pub fn wrap_t(&self) -> WrapMode {
        wrap((self.flags >> 4) & 3)
    }
    /// Raw max-LOD field (flag bits 7-10); 15 means "down to 16x16" in the reference renderer.
    pub fn max_lod_raw(&self) -> u32 {
        (self.flags >> 7) & 0xf
    }
    /// Maximum LOD as used by the reference renderer for a texture of `width` x `height`.
    pub fn max_lod(&self, width: u32, height: u32) -> f32 {
        let raw = self.max_lod_raw();
        if width != height {
            0.0
        } else if raw == 15 {
            ((width.min(height).max(1) as f32).log2() - 4.0).max(0.0)
        } else {
            raw as f32
        }
    }
    pub fn mag_filter_nearest(&self) -> bool {
        self.flags & tev_layer_flags::MAGFILT_NEAR != 0
    }
}

fn wrap(v: u32) -> WrapMode {
    match v {
        0 => WrapMode::Clamp,
        2 => WrapMode::Mirror,
        _ => WrapMode::Repeat,
    }
}

/// GX blend factor (`GX.BlendFactor`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    SrcClr,
    InvSrcClr,
    SrcAlpha,
    InvSrcAlpha,
    DstAlpha,
    InvDstAlpha,
}

impl BlendFactor {
    fn from_raw(v: u32) -> Option<Self> {
        use BlendFactor::*;
        [
            Zero,
            One,
            SrcClr,
            InvSrcClr,
            SrcAlpha,
            InvSrcAlpha,
            DstAlpha,
            InvDstAlpha,
        ]
        .get(v as usize)
        .copied()
    }
}

/// Per-shape material (the first 0x60 bytes of a shape).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Raw flags plus the synthesised `VERT_COLORS` / `SIMPLE_MATERIAL` bits.
    pub flags: u32,
    pub material_color: Rgba8,
    pub ambient_color: Rgba8,
    pub specular_color: Rgba8,
    /// 0.0..=1.0.
    pub alpha: f32,
    pub tev_layer_count: u8,
    pub unk_14: u8,
    pub unk_15: u8,
    /// Indices into the model's `tev_layers` (-1 = unused).
    pub tev_layer_idxs: [i16; 3],
    /// Bitmask of present vertex attributes, bit = `gx::attr::*`.
    pub vtx_attrs: u32,
    /// Raw blend factor word (src in low nibble, dest in the next).
    pub blend_factors: u32,
}

impl Material {
    pub fn has_flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }
    /// Source blend factor; default `SrcAlpha` unless `CUSTOM_BLEND_SRC`.
    pub fn src_blend(&self) -> BlendFactor {
        if self.has_flag(material_flags::CUSTOM_BLEND_SRC) {
            BlendFactor::from_raw(self.blend_factors & 0xf).unwrap_or(BlendFactor::SrcAlpha)
        } else {
            BlendFactor::SrcAlpha
        }
    }
    /// Destination blend factor; default `InvSrcAlpha` unless `CUSTOM_BLEND_DEST`.
    pub fn dest_blend(&self) -> BlendFactor {
        if self.has_flag(material_flags::CUSTOM_BLEND_DEST) {
            BlendFactor::from_raw((self.blend_factors >> 4) & 0xf)
                .unwrap_or(BlendFactor::InvSrcAlpha)
        } else {
            BlendFactor::InvSrcAlpha
        }
    }
    pub fn has_attr(&self, a: u32) -> bool {
        self.vtx_attrs & (1 << a) != 0
    }
}

/// One raw display list of a shape together with its default cull mode.
#[derive(Clone, Debug, PartialEq)]
pub struct Dlist {
    pub data: Vec<u8>,
    pub cull: CullMode,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub material: Material,
    /// Reference point used for depth sorting.
    pub origin: [f32; 3],
    /// True if this shape is in the model's translucent group (after the opaque ones).
    pub translucent: bool,
    pub dlists: Vec<Dlist>,
}

impl Shape {
    /// Decode display list `i` into vertex/index buffers.
    pub fn decode_dlist(&self, i: usize) -> Result<DecodedList, Error> {
        let d = self
            .dlists
            .get(i)
            .ok_or(Error::Invalid("display list index"))?;
        gx::decode_display_list(&d.data, self.material.vtx_attrs)
    }
}

/// Decoded geometry of one display list, ready for upload.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    /// Index into `Model::shapes`; `model.shapes[shape_index].material` is this mesh's material.
    pub shape_index: usize,
    /// Which of the shape's display lists this came from.
    pub dlist_index: usize,
    pub cull: CullMode,
    pub translucent: bool,
    pub vertices: gx::Vertices,
    /// Triangle list indices into `vertices`.
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    pub name: String,
    /// Entry index in the GMA table (what stagedefs reference by id).
    pub id: usize,
    pub flags: u32,
    pub bound_sphere_center: [f32; 3],
    pub bound_sphere_radius: f32,
    pub opaque_shape_count: usize,
    pub translucent_shape_count: usize,
    /// 3x4 row-major matrices (12 floats each) as stored in the file.
    pub matrices: Vec<[f32; 12]>,
    pub tev_layers: Vec<TevLayer>,
    /// Shapes in file order, opaque first. Shapes using NBT normals are skipped like the reference does.
    pub shapes: Vec<Shape>,
    /// Number of shapes dropped because they use NBT normals.
    pub skipped_nbt_shapes: usize,
}

impl Model {
    /// Decode every display list of every shape (one `Mesh` each, in shape then dlist order).
    pub fn meshes(&self) -> Result<Vec<Mesh>, Error> {
        let mut out = Vec::new();
        for (si, shape) in self.shapes.iter().enumerate() {
            for (di, dl) in shape.dlists.iter().enumerate() {
                let d = gx::decode_display_list(&dl.data, shape.material.vtx_attrs)?;
                out.push(Mesh {
                    shape_index: si,
                    dlist_index: di,
                    cull: dl.cull,
                    translucent: shape.translucent,
                    vertices: d.vertices,
                    indices: d.indices,
                });
            }
        }
        Ok(out)
    }

    pub fn uses_vat16(&self) -> bool {
        self.flags & model_flags::VAT_16_BIT != 0
    }

    /// Texture layers referenced by `shape`'s material, resolved against this model.
    pub fn shape_tev_layers<'a>(
        &'a self,
        shape: &'a Shape,
    ) -> impl Iterator<Item = &'a TevLayer> + 'a {
        shape
            .material
            .tev_layer_idxs
            .iter()
            .take(shape.material.tev_layer_count as usize)
            .filter_map(move |&i| usize::try_from(i).ok().and_then(|i| self.tev_layers.get(i)))
    }
}

/// A model table entry that was not parsed (unsupported model type).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedModel {
    pub id: usize,
    pub name: String,
    pub flags: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Gma {
    /// Parsed models in table order.
    pub models: Vec<Model>,
    /// Entries skipped because they are skinned / stitched / effective models (unsupported, as in the reference).
    pub skipped: Vec<SkippedModel>,
    by_name: HashMap<String, usize>,
    by_id: HashMap<usize, usize>,
}

impl Gma {
    /// Look a model up by name; if names repeat the last one wins (reference `Map.set` semantics).
    pub fn model(&self, name: &str) -> Option<&Model> {
        self.by_name.get(name).map(|&i| &self.models[i])
    }
    /// Look a model up by its table index.
    pub fn model_by_id(&self, id: usize) -> Option<&Model> {
        self.by_id.get(&id).map(|&i| &self.models[i])
    }
    pub fn iter(&self) -> impl Iterator<Item = &Model> {
        self.models.iter()
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.models.iter().map(|m| m.name.as_str())
    }
}

fn parse_tev_layer(r: &Reader, o: usize) -> Result<TevLayer, Error> {
    let ty = r.u8(o + 0x13)?;
    Ok(TevLayer {
        flags: r.u32(o)?,
        tex_idx: r.i16(o + 0x04)?,
        lod_bias: r.i8(o + 0x06)?,
        max_aniso: r.i8(o + 0x07)?,
        unk_0c: r.i8(o + 0x0c)?,
        swappable: r.u8(o + 0x0d)? != 0,
        unk_10: r.i32(o + 0x10)?,
        alpha_type: (ty >> 4) & 7,
        color_type: ty & 0xf,
    })
}

fn parse_matrix(r: &Reader, o: usize) -> Result<[f32; 12], Error> {
    let mut m = [0.0; 12];
    for (i, v) in m.iter_mut().enumerate() {
        *v = r.f32(o + i * 4)?;
    }
    Ok(m)
}

fn parse_material(r: &Reader, o: usize) -> Result<Material, Error> {
    let tev_layer_count = r.u8(o + 0x12)?;
    let vtx_attrs = r.u32(o + 0x1c)?;
    let mut flags = r.u32(o)?;
    if vtx_attrs & (1 << attr::CLR0) != 0 {
        flags |= material_flags::VERT_COLORS;
    }
    if tev_layer_count == 0 {
        flags |= material_flags::SIMPLE_MATERIAL;
    }
    Ok(Material {
        flags,
        material_color: Rgba8::unpack(r.u32(o + 0x04)?),
        ambient_color: Rgba8::unpack(r.u32(o + 0x08)?),
        specular_color: Rgba8::unpack(r.u32(o + 0x0c)?),
        alpha: r.u8(o + 0x11)? as f32 / 255.0,
        tev_layer_count,
        unk_14: r.u8(o + 0x14)?,
        unk_15: r.u8(o + 0x15)?,
        tev_layer_idxs: [r.i16(o + 0x16)?, r.i16(o + 0x18)?, r.i16(o + 0x1a)?],
        vtx_attrs,
        blend_factors: r.u32(o + 0x40)?,
    })
}

fn dlist_bytes(r: &Reader, off: usize, size: i32) -> Result<Vec<u8>, Error> {
    let size = usize::try_from(size).map_err(|_| Error::Invalid("negative display list size"))?;
    Ok(r.bytes(off, size)?.to_vec())
}

/// Returns the shape and its total byte size.
fn parse_shape(r: &Reader, o: usize, translucent: bool) -> Result<(Shape, usize), Error> {
    let material = parse_material(r, o)?;
    let dlist_flags = r.u8(o + 0x13)?;
    let front_size = r.i32(o + 0x28)?;
    let back_size = r.i32(o + 0x2c)?;
    let origin = r.vec3(o + 0x30)?;
    let double_sided = material.flags & material_flags::DOUBLE_SIDED != 0;
    let mut cur = o + SHAPE_BASE_SIZE;
    let mut dlists = Vec::new();

    if dlist_flags & 1 != 0 {
        dlists.push(Dlist {
            data: dlist_bytes(r, cur, front_size)?,
            cull: if double_sided {
                CullMode::None
            } else {
                CullMode::Front
            },
        });
        cur += front_size as usize;
    }
    if dlist_flags & 2 != 0 {
        dlists.push(Dlist {
            data: dlist_bytes(r, cur, back_size)?,
            cull: if double_sided {
                CullMode::None
            } else {
                CullMode::Back
            },
        });
        cur += back_size as usize;
    }
    if dlist_flags & 0xc != 0 {
        let extra_front = r.i32(cur + 0x08)?;
        let extra_back = r.i32(cur + 0x0c)?;
        cur += 0x20;
        dlists.push(Dlist {
            data: dlist_bytes(r, cur, extra_front)?,
            cull: CullMode::Front,
        });
        cur += extra_front as usize;
        dlists.push(Dlist {
            data: dlist_bytes(r, cur, extra_back)?,
            cull: CullMode::Back,
        });
        cur += extra_back as usize;
    }
    Ok((
        Shape {
            material,
            origin,
            translucent,
            dlists,
        },
        cur - o,
    ))
}

fn parse_model(r: &Reader, o: usize, name: String, id: usize) -> Result<Model, Error> {
    if r.bytes(o, 4)? != b"GCMF" {
        return Err(Error::Invalid("bad GCMF tag"));
    }
    let flags = r.u32(o + 4)?;
    let tev_count = r.i16(o + 0x18)?;
    let opaque = r.i16(o + 0x1a)?;
    let translucent = r.i16(o + 0x1c)?;
    let mtx_count = r.i8(o + 0x1e)?;
    let tex_mtx_size = r.u32(o + 0x20)? as usize;
    if tev_count < 0 || opaque < 0 || translucent < 0 || mtx_count < 0 {
        return Err(Error::Invalid("negative count in GCMF header"));
    }
    let (opaque, translucent) = (opaque as usize, translucent as usize);

    let mut cur = o + 0x40;
    let mut tev_layers = Vec::new();
    for _ in 0..tev_count {
        tev_layers.push(parse_tev_layer(r, cur)?);
        cur += 0x20;
    }
    let mut matrices = Vec::new();
    for _ in 0..mtx_count {
        matrices.push(parse_matrix(r, cur)?);
        cur += 0x30;
    }

    let mut shape_off = o + tex_mtx_size;
    let mut shapes = Vec::new();
    let mut skipped_nbt_shapes = 0;
    for i in 0..opaque + translucent {
        let (shape, size) = parse_shape(r, shape_off, i >= opaque)?;
        shape_off += size;
        if shape.material.has_attr(attr::NBT) {
            skipped_nbt_shapes += 1;
            continue;
        }
        shapes.push(shape);
    }

    Ok(Model {
        name,
        id,
        flags,
        bound_sphere_center: r.vec3(o + 0x08)?,
        bound_sphere_radius: r.f32(o + 0x14)?,
        opaque_shape_count: opaque,
        translucent_shape_count: translucent,
        matrices,
        tev_layers,
        shapes,
        skipped_nbt_shapes,
    })
}

/// Parse a decompressed GMA file.
///
/// Mirrors the reference: invalid table entries and models with zero shapes are skipped, and
/// stitching/skin/effective models are skipped but listed in [`Gma::skipped`].
pub fn parse_gma(bytes: &[u8]) -> Result<Gma, Error> {
    let r = Reader::new(bytes);
    let count = r.u32(0)? as usize;
    let gcmf_base = r.u32(4)? as usize;
    let table_end = count
        .checked_mul(8)
        .and_then(|n| n.checked_add(8))
        .ok_or(Error::Invalid("GMA model count"))?;
    if table_end > gcmf_base || gcmf_base > bytes.len() {
        return Err(Error::Invalid("GMA header offsets"));
    }
    let mut gma = Gma::default();
    for i in 0..count {
        let gcmf_offs = r.i32(8 + i * 8)?;
        let name_offs = r.i32(8 + i * 8 + 4)?;
        if gcmf_offs < 0 && name_offs <= 0 {
            continue;
        }
        let name = if name_offs >= 0 {
            r.cstr(table_end + name_offs as usize, gcmf_base)
        } else {
            String::new()
        };
        let model_off = usize::try_from(gcmf_offs)
            .ok()
            .and_then(|g| g.checked_add(gcmf_base))
            .ok_or(Error::Invalid("negative GCMF offset"))?;
        let flags = r.u32(model_off + 4)?;
        if flags & (model_flags::STITCHING | model_flags::SKIN | model_flags::EFFECTIVE) != 0 {
            gma.skipped.push(SkippedModel { id: i, name, flags });
            continue;
        }
        let model = parse_model(&r, model_off, name, i)?;
        if model.opaque_shape_count + model.translucent_shape_count < 1 {
            continue;
        }
        let idx = gma.models.len();
        gma.by_name.insert(model.name.clone(), idx);
        gma.by_id.insert(i, idx);
        gma.models.push(model);
    }
    Ok(gma)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    /// Minimal one-model GMA: one opaque shape with a single triangle display list.
    pub(crate) fn synthetic_gma() -> Vec<u8> {
        let name = b"TESTMODEL\0";
        let mut v = Vec::new();
        put32(&mut v, 2); // count
        put32(&mut v, 8 + 16 + 16); // gcmf base (names padded to 16)
                                    // entry 0: invalid, entry 1: valid
        put32(&mut v, 0xffff_ffff);
        put32(&mut v, 0);
        put32(&mut v, 0);
        put32(&mut v, 0);
        // name table
        let mut names = name.to_vec();
        names.resize(16, 0);
        v.extend_from_slice(&names);
        assert_eq!(v.len(), 8 + 16 + 16);
        // GCMF header
        let mut m = vec![0u8; 0x40];
        m[..4].copy_from_slice(b"GCMF");
        m[0x18..0x1a].copy_from_slice(&1i16.to_be_bytes()); // tev layers
        m[0x1a..0x1c].copy_from_slice(&1i16.to_be_bytes()); // opaque
        m[0x14..0x18].copy_from_slice(&2.0f32.to_be_bytes());
        m[0x20..0x24].copy_from_slice(&(0x40u32 + 0x20).to_be_bytes());
        // tev layer
        let mut t = vec![0u8; 0x20];
        t[4..6].copy_from_slice(&3i16.to_be_bytes());
        t[0..4].copy_from_slice(&((1u32 << 2) | (2 << 4)).to_be_bytes());
        t[0x13] = 0x21;
        m.extend_from_slice(&t);
        // shape
        let mut dl = vec![0u8, 0x90, 0, 3];
        for i in 0..3 {
            for f in [i as f32, 0.0, 0.0] {
                dl.extend_from_slice(&f.to_be_bytes());
            }
        }
        dl.resize(48, 0);
        let mut s = vec![0u8; 0x60];
        s[0x13] = 1;
        s[0x11] = 255;
        s[0x12] = 1;
        s[0x16..0x18].copy_from_slice(&0i16.to_be_bytes());
        s[0x18..0x1a].copy_from_slice(&(-1i16).to_be_bytes());
        s[0x1c..0x20].copy_from_slice(&(1u32 << attr::POS).to_be_bytes());
        s[0x28..0x2c].copy_from_slice(&(dl.len() as i32).to_be_bytes());
        s[0x30..0x34].copy_from_slice(&5.0f32.to_be_bytes());
        m.extend_from_slice(&s);
        m.extend_from_slice(&dl);
        v.extend_from_slice(&m);
        // fix entry 1 offsets: gcmf offset 0, name offset 0 (valid because gcmf >= 0)
        v
    }

    #[test]
    fn parses_synthetic_model() {
        let g = parse_gma(&synthetic_gma()).unwrap();
        assert_eq!(g.models.len(), 1);
        let m = g.model("TESTMODEL").unwrap();
        assert_eq!(m.id, 1);
        assert_eq!(m.bound_sphere_radius, 2.0);
        assert_eq!(m.tev_layers[0].tex_idx, 3);
        assert_eq!(m.tev_layers[0].wrap_s(), WrapMode::Repeat);
        assert_eq!(m.tev_layers[0].wrap_t(), WrapMode::Mirror);
        assert_eq!(
            (m.tev_layers[0].alpha_type, m.tev_layers[0].color_type),
            (2, 1)
        );
        let sh = &m.shapes[0];
        assert!(!sh.material.has_flag(material_flags::SIMPLE_MATERIAL));
        assert_eq!(sh.origin, [5.0, 0.0, 0.0]);
        assert_eq!(sh.dlists[0].cull, CullMode::Front);
        assert_eq!(m.shape_tev_layers(sh).count(), 1);
        let meshes = m.meshes().unwrap();
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].triangle_count(), 1);
        assert_eq!(meshes[0].vertices.positions[2], [2.0, 0.0, 0.0]);
        assert!(g.model_by_id(1).is_some() && g.model_by_id(0).is_none());
    }

    #[test]
    fn rejects_truncated_input() {
        let b = synthetic_gma();
        assert!(parse_gma(&b[..b.len() - 10]).is_err());
        assert!(parse_gma(&b[..4]).is_err());
        assert!(parse_gma(&[]).is_err());
    }

    #[test]
    fn skin_models_are_listed_not_parsed() {
        let mut b = synthetic_gma();
        let g = 8 + 16 + 16;
        b[g + 4..g + 8].copy_from_slice(&model_flags::SKIN.to_be_bytes());
        let g = parse_gma(&b).unwrap();
        assert!(g.models.is_empty());
        assert_eq!(g.skipped.len(), 1);
    }
}
