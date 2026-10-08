//! NaomiLib ("NL") object archives (`*_p.lz` once decompressed). Ports the parsing half of
//! `noclip/SuperMonkeyBall/NaomiLib.ts`.
//!
//! The archive is a table of absolute model offsets starting at byte 4 (terminated by 0). Each
//! model holds a list of meshes (a material plus display lists of strip / triangle / quad vertices).

use std::collections::HashMap;

use crate::gx::{CullMode, Primitive, Vertices};
use crate::reader::{Budget, Reader};
use crate::Error;
use std::mem::size_of;

/// Decoded bytes allowed per input byte (see [`Budget`]). The largest ratio on the retail disc is 3.5.
const BUDGET_PER_BYTE: usize = 16;
/// Decoded size of one vertex (all six attribute arrays) and of the indices one strip vertex can add.
const VERTEX_COST: usize = 52 + 12;

const VTX_SIZE: usize = 0x20;
const VTX_OFFSET_DESC_SIZE: usize = 0x8;
const DISP_LIST_HEADER_SIZE: usize = 8;

/// Display-list flag bits.
pub mod dl_flags {
    pub const QUADS: u32 = 1 << 2;
    pub const TRIANGLES: u32 = 1 << 3;
    pub const TRIANGLE_STRIP: u32 = 1 << 4;
}

/// `NlMesh::tex_flags` bits.
pub mod tex_flags {
    pub const SCALE_FILTER_NEAR: u32 = (1 << 13) | (1 << 14);
    pub const T_CLAMP: u32 = 1 << 15;
    pub const S_CLAMP: u32 = 1 << 16;
    pub const T_MIRROR: u32 = 1 << 17;
    pub const S_MIRROR: u32 = 1 << 18;
}

/// Mesh flag bit 24: do not write depth.
pub const MESH_DISABLE_DEPTH_WRITE: u32 = 1 << 24;

/// Negative `mesh_type` values; non-negative types are lit with vertex normals.
pub mod mesh_type {
    pub const UNLIT_CONST_MAT_COLOR: i32 = -1;
    pub const LIT_CONST_MAT_COLOR: i32 = -2;
    pub const UNLIT_VERT_MAT_COLOR: i32 = -3;
}

/// Vertex layout shared by every mesh of a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VertexKind {
    /// Position, per-vertex material color, uv; no normals; always unlit.
    A,
    /// Position, normal, uv.
    B,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NlMesh {
    pub flags: u32,
    pub tex_flags: u32,
    /// TPL texture index, or `None` for untextured.
    pub tex_idx: Option<u32>,
    pub mesh_type: i32,
    pub ambient_color_scale: f32,
    /// R, G, B, A in 0..1.
    pub material_color: [f32; 4],
    /// Cull mode of the first display list (the reference applies it to the whole mesh).
    pub cull: CullMode,
    /// Raw 3-bit depth compare selector (`flags >> 29`).
    pub z_compare: u32,
    pub depth_write: bool,
    /// Vertex data over all display lists. Type A meshes fill `colors`; type B meshes fill `normals`
    /// (the other keeps defaults: white / +Z). `uv1`/`uv2` mirror `uv0`.
    pub vertices: Vertices,
    /// Triangle list indices into `vertices`.
    pub indices: Vec<u32>,
}

impl NlMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
    /// Wrap modes `(s, t)` from `tex_flags`.
    pub fn wrap(&self) -> (crate::WrapMode, crate::WrapMode) {
        use crate::WrapMode::*;
        let s = if self.tex_flags & tex_flags::S_CLAMP != 0 {
            Clamp
        } else if self.tex_flags & tex_flags::S_MIRROR != 0 {
            Mirror
        } else {
            Repeat
        };
        let t = if self.tex_flags & tex_flags::T_CLAMP != 0 {
            Clamp
        } else if self.tex_flags & tex_flags::T_MIRROR != 0 {
            Mirror
        } else {
            Repeat
        };
        (s, t)
    }
    /// Point filtering when the low two flag bits are zero (reference behaviour).
    pub fn point_filter(&self) -> bool {
        self.tex_flags & 3 == 0
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct NlModel {
    /// Index in the archive's model table.
    pub id: usize,
    pub flags: u32,
    pub kind: VertexKind,
    pub bound_sphere_center: [f32; 3],
    pub bound_sphere_radius: f32,
    pub meshes: Vec<NlMesh>,
}

#[derive(Clone, Debug, Default)]
pub struct NlObj {
    /// Valid models in table order (invalid entries are omitted but keep their ids).
    pub models: Vec<NlModel>,
    /// Name -> table index (see [`build_obj_name_map`]).
    pub names: HashMap<String, usize>,
}

impl NlObj {
    pub fn model(&self, id: usize) -> Option<&NlModel> {
        self.models.iter().find(|m| m.id == id)
    }
    /// Look up by name; `obj_` prefixed names are also reachable without the prefix.
    pub fn model_by_name(&self, name: &str) -> Option<&NlModel> {
        self.names.get(name).and_then(|&id| self.model(id))
    }
}

fn nl_cull(idx: u32) -> CullMode {
    [
        CullMode::All,
        CullMode::None,
        CullMode::Back,
        CullMode::Front,
    ][(idx & 3) as usize]
}

fn push_vertex(r: &Reader, off: usize, kind: VertexKind, out: &mut Vertices) -> Result<(), Error> {
    let pos = r.vec3(off)?;
    let (nrm, clr) = match kind {
        VertexKind::A => ([0.0, 0.0, 1.0], r.u32(off + 0x10)?.to_be_bytes()),
        VertexKind::B => (r.vec3(off + 0xc)?, [255; 4]),
    };
    let uv = [r.f32(off + 0x18)?, r.f32(off + 0x1c)?];
    out.positions.push(pos);
    out.normals.push(nrm);
    out.colors.push(clr);
    out.uv0.push(uv);
    out.uv1.push(uv);
    out.uv2.push(uv);
    Ok(())
}

fn parse_disp_lists(
    r: &Reader,
    budget: &mut Budget,
    start: usize,
    size: usize,
    kind: VertexKind,
    out: &mut Vertices,
    indices: &mut Vec<u32>,
) -> Result<Option<u32>, Error> {
    let end = start
        .checked_add(size)
        .ok_or(Error::Invalid("NL display list size"))?;
    let mut off = start;
    let mut first_flags = None;
    while off < end {
        let flags = r.u32(off)?;
        let n = r.u32(off + 4)?;
        let (prim, per_item) = if flags & dl_flags::TRIANGLE_STRIP != 0 {
            (Primitive::TriangleStrip, 1)
        } else if flags & dl_flags::TRIANGLES != 0 {
            (Primitive::Triangles, 3)
        } else if flags & dl_flags::QUADS != 0 {
            (Primitive::Quads, 4)
        } else {
            return Err(Error::Invalid("NL display list primitive type"));
        };
        // Every vertex takes at least a reference record in the file.
        let vtx_count = (n as usize)
            .checked_mul(per_item)
            .filter(|&c| c <= r.data.len() / VTX_OFFSET_DESC_SIZE)
            .ok_or(Error::Invalid("NL display list vertex count"))?;
        budget.charge_n(vtx_count, VERTEX_COST)?;
        first_flags.get_or_insert(flags);
        let base = out.len() as u32;
        let mut vo = off + DISP_LIST_HEADER_SIZE;
        for _ in 0..vtx_count {
            // The low bit of the x float marks a real vertex; otherwise an 8-byte reference to one.
            if r.u32(vo)? & 1 != 0 {
                push_vertex(r, vo, kind, out)?;
                vo += VTX_SIZE;
            } else {
                let rel = r.i32(vo + 4)? as i64;
                let actual = vo as i64 + rel + VTX_OFFSET_DESC_SIZE as i64;
                let actual =
                    usize::try_from(actual).map_err(|_| Error::Invalid("NL vertex reference"))?;
                r.bytes(actual, VTX_SIZE)?;
                push_vertex(r, actual, kind, out)?;
                vo += VTX_OFFSET_DESC_SIZE;
            }
        }
        prim.triangulate(base, vtx_count as u32, indices);
        off = vo;
    }
    Ok(first_flags)
}

fn parse_mesh_list(
    r: &Reader,
    budget: &mut Budget,
    mut off: usize,
    kind: VertexKind,
) -> Result<Vec<NlMesh>, Error> {
    let mut meshes = Vec::new();
    loop {
        let flags = r.u32(off)?;
        if flags == 0 {
            return Ok(meshes);
        }
        // Also keeps the fixed offsets below from wrapping on 32-bit targets.
        r.bytes(off, 0x50)?;
        budget.charge(size_of::<NlMesh>())?;
        let tex_idx = r.i32(off + 0x20)?;
        let size = r.u32(off + 0x4c)? as usize;
        let mut vertices = Vertices::default();
        let mut indices = Vec::new();
        let first = parse_disp_lists(
            r,
            budget,
            off + 0x50,
            size,
            kind,
            &mut vertices,
            &mut indices,
        )?;
        meshes.push(NlMesh {
            flags,
            tex_flags: r.u32(off + 8)?,
            tex_idx: u32::try_from(tex_idx).ok(),
            mesh_type: r.i32(off + 0x24)?,
            ambient_color_scale: r.f32(off + 0x28)?,
            // Stored as A, R, G, B floats.
            material_color: [
                r.f32(off + 0x30)?,
                r.f32(off + 0x34)?,
                r.f32(off + 0x38)?,
                r.f32(off + 0x2c)?,
            ],
            cull: nl_cull(first.map_or(1, |f| f & 3)),
            z_compare: flags >> 29,
            depth_write: flags & MESH_DISABLE_DEPTH_WRITE == 0,
            vertices,
            indices,
        });
        off = (off + 0x50)
            .checked_add(size)
            .ok_or(Error::Invalid("NL mesh size"))?;
    }
}

fn parse_model(
    r: &Reader,
    budget: &mut Budget,
    off: usize,
    id: usize,
) -> Result<Option<NlModel>, Error> {
    if r.i32(off)? == -1 {
        return Ok(None);
    }
    r.bytes(off, 0x18)?;
    budget.charge(size_of::<NlModel>())?;
    let flags = r.u32(off + 4)?;
    // The reference's `ModelFlags.VtxTypeA` is the first enum member (value 0), so `flags & VtxTypeA` is always
    // false and every model is parsed as type B. Retail data agrees: bit 0 can be set on models whose vertices are
    // type B (normals at +0xc).
    let kind = VertexKind::B;
    Ok(Some(NlModel {
        id,
        flags,
        kind,
        bound_sphere_center: r.vec3(off + 8)?,
        bound_sphere_radius: r.f32(off + 0x14)?,
        meshes: parse_mesh_list(r, budget, off + 0x18, kind)?,
    }))
}

/// Parse a decompressed NaomiLib archive (e.g. `init/common_p.lz`, `stNNN_p.lz`).
pub fn parse_naomi_obj(bytes: &[u8]) -> Result<NlObj, Error> {
    let r = Reader::new(bytes);
    let mut budget = Budget::new(bytes.len(), BUDGET_PER_BYTE);
    let mut obj = NlObj {
        names: name_map(bytes, &mut budget)?,
        ..Default::default()
    };
    let mut off = 4;
    for id in 0.. {
        let model_off = r.u32(off)? as usize;
        if model_off == 0 {
            break;
        }
        if let Some(m) = parse_model(&r, &mut budget, model_off, id)? {
            obj.models.push(m);
        }
        off += 4;
    }
    Ok(obj)
}

/// Name -> model index map (names live in a 8-byte header before each model). `obj_`-prefixed
/// names are registered with and without the prefix, like the reference.
///
/// A corrupt archive whose entries share one very long name yields the names read before the size cap was
/// reached ([`parse_naomi_obj`] reports that case as an error).
pub fn build_obj_name_map(bytes: &[u8]) -> HashMap<String, usize> {
    let mut budget = Budget::new(bytes.len(), BUDGET_PER_BYTE);
    let mut map = HashMap::new();
    let _ = fill_name_map(bytes, &mut budget, &mut map);
    map
}

fn name_map(bytes: &[u8], budget: &mut Budget) -> Result<HashMap<String, usize>, Error> {
    let mut map = HashMap::new();
    fill_name_map(bytes, budget, &mut map)?;
    Ok(map)
}

fn fill_name_map(
    bytes: &[u8],
    budget: &mut Budget,
    map: &mut HashMap<String, usize>,
) -> Result<(), Error> {
    let r = Reader::new(bytes);
    let mut off = 4;
    for id in 0.. {
        let Ok(model_off) = r.u32(off) else { break };
        if model_off == 0 {
            break;
        }
        off += 4;
        let model_off = model_off as usize;
        let Some(header) = model_off.checked_sub(8) else {
            continue;
        };
        let Ok(name_ptr) = r.u32(header) else {
            continue;
        };
        let name_ptr = name_ptr as usize;
        if name_ptr == 0 || name_ptr >= bytes.len() {
            continue;
        }
        let start = name_ptr + 4;
        if start >= bytes.len() {
            continue;
        }
        let name = r.cstr(start, bytes.len());
        if name.is_empty() {
            continue;
        }
        // Two keys (with and without `obj_`), each with its map slot.
        budget.charge_n(2, name.len() + size_of::<(String, usize)>())?;
        if let Some(short) = name.strip_prefix("obj_") {
            map.insert(short.to_string(), id);
        }
        map.insert(name, id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: &mut Vec<u8>, x: f32) {
        v.extend_from_slice(&x.to_be_bytes());
    }
    fn u(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_be_bytes());
    }

    fn real_vtx(v: &mut Vec<u8>, x: f32, y: f32) {
        u(v, x.to_bits() | 1);
        f(v, y);
        f(v, 0.0); // z
        u(v, 0);
        u(v, 0);
        u(v, 0);
        f(v, 0.5);
        f(v, 0.75); // u v
    }

    #[test]
    fn parses_type_b_model_with_vertex_reference() {
        // layout: [0]=pad, [4]=model offset table, then name header + model.
        let mut b = vec![0u8; 4];
        u(&mut b, 0x30); // model 0 at 0x30
        u(&mut b, 0);
        b.resize(0x28, 0);
        u(&mut b, 0x100); // name ptr at model-8
        u(&mut b, 0);
        assert_eq!(b.len(), 0x30);
        // model header
        u(&mut b, 0); // valid
        u(&mut b, 0); // flags: type B
        for _ in 0..4 {
            f(&mut b, 0.0);
        }
        // mesh
        let mut mesh = vec![0u8; 0x50];
        mesh[..4].copy_from_slice(&1u32.to_be_bytes());
        mesh[0x20..0x24].copy_from_slice(&(-1i32).to_be_bytes());
        // display list: triangles x1 (3 vertices: 2 real, 1 reference to first)
        let mut dl = Vec::new();
        u(&mut dl, (1 << 3) | 2);
        u(&mut dl, 1);
        real_vtx(&mut dl, 1.0, 2.0); // at dl+8
        real_vtx(&mut dl, 3.0, 4.0);
        // reference: located at dl offset 8+64 = 72; actual = 72 + rel + 8 = 8 -> rel = -72
        u(&mut dl, 0);
        dl.extend_from_slice(&(-72i32).to_be_bytes());
        mesh[0x4c..0x50].copy_from_slice(&(dl.len() as u32).to_be_bytes());
        b.extend_from_slice(&mesh);
        b.extend_from_slice(&dl);
        u(&mut b, 0); // end-of-mesh-list marker
        let name_at = 0x100usize;
        b.resize(name_at + 4, 0);
        b.extend_from_slice(b"obj_TEST\0");

        let obj = parse_naomi_obj(&b).unwrap();
        assert_eq!(obj.models.len(), 1);
        let m = &obj.models[0];
        assert_eq!(m.kind, VertexKind::B);
        let mesh = &m.meshes[0];
        assert_eq!(mesh.tex_idx, None);
        assert_eq!(mesh.cull, CullMode::Back);
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.vertices.positions[2][1], 2.0);
        assert_eq!(mesh.vertices.uv0[0], [0.5, 0.75]);
        assert_eq!(mesh.indices, vec![0, 1, 2]);
        assert_eq!(obj.model_by_name("TEST").unwrap().id, 0);
        assert_eq!(obj.model_by_name("obj_TEST").unwrap().id, 0);
    }
}
