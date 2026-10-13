//! GX constants and display-list decoding (port of `gx.ts` / `gx_displaylist.ts` and the noclip VtxLoader
//! behaviour used for GMA shapes).

use crate::Error;
use crate::reader::Reader;

/// Vertex attribute indices (bit positions in a material's `vtx_attrs`).
pub mod attr {
    pub const PNMTXIDX: u32 = 0;
    pub const TEX0MTXIDX: u32 = 1;
    pub const POS: u32 = 9;
    pub const NRM: u32 = 10;
    pub const CLR0: u32 = 11;
    pub const CLR1: u32 = 12;
    pub const TEX0: u32 = 13;
    pub const TEX1: u32 = 14;
    pub const TEX2: u32 = 15;
    pub const TEX7: u32 = 20;
    /// Pseudo attribute: normals are stored as normal+binormal+tangent (9 components).
    pub const NBT: u32 = 25;
}

/// Fixed-point shift for 16-bit vertex data (VTXFMT1).
pub const S16_SHIFT: u32 = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CullMode {
    None,
    Front,
    Back,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Primitive {
    Quads,
    QuadStrip,
    Triangles,
    TriangleStrip,
    TriangleFan,
    Lines,
    LineStrip,
    Points,
}

impl Primitive {
    /// Decode the opcode part (`cmd & 0xf8`) of a GX draw command.
    pub fn from_opcode(opcode: u8) -> Option<Self> {
        Some(match opcode {
            0x80 => Self::Quads,
            0x88 => Self::QuadStrip,
            0x90 => Self::Triangles,
            0x98 => Self::TriangleStrip,
            0xa0 => Self::TriangleFan,
            0xa8 => Self::Lines,
            0xb0 => Self::LineStrip,
            0xb8 => Self::Points,
            _ => return None,
        })
    }

    /// Append triangle indices for `count` consecutive vertices starting at index `base`.
    /// Winding matches the reference (`emitTrianglesForStrip` etc.). Lines/points emit nothing.
    pub fn triangulate(self, base: u32, count: u32, out: &mut Vec<u32>) {
        let n = count;
        let v = |i: u32| base + i;
        match self {
            Self::Triangles => {
                let mut i = 0;
                while i + 2 < n {
                    out.extend_from_slice(&[v(i), v(i + 1), v(i + 2)]);
                    i += 3;
                }
            }
            Self::TriangleStrip => {
                let mut i = 0;
                while i + 2 < n {
                    if i & 1 == 1 {
                        out.extend_from_slice(&[v(i + 1), v(i), v(i + 2)]);
                    } else {
                        out.extend_from_slice(&[v(i), v(i + 1), v(i + 2)]);
                    }
                    i += 1;
                }
            }
            Self::TriangleFan => {
                let mut i = 1;
                while i + 1 < n {
                    out.extend_from_slice(&[v(0), v(i), v(i + 1)]);
                    i += 1;
                }
            }
            Self::Quads => {
                let mut i = 0;
                while i + 3 < n {
                    out.extend_from_slice(&[v(i), v(i + 1), v(i + 2), v(i), v(i + 2), v(i + 3)]);
                    i += 4;
                }
            }
            Self::QuadStrip => {
                let mut i = 0;
                while i + 3 < n {
                    out.extend_from_slice(&[
                        v(i),
                        v(i + 1),
                        v(i + 2),
                        v(i + 1),
                        v(i + 3),
                        v(i + 2),
                    ]);
                    i += 2;
                }
            }
            Self::Lines | Self::LineStrip | Self::Points => {}
        }
    }
}

/// Convenience wrapper around [`Primitive::triangulate`] returning triangles.
pub fn triangulate(prim: Primitive, vertex_count: u32) -> Vec<[u32; 3]> {
    let mut flat = Vec::new();
    prim.triangulate(0, vertex_count, &mut flat);
    flat.chunks(3).map(|c| [c[0], c[1], c[2]]).collect()
}

/// Struct-of-arrays vertex data, one entry per decoded GX vertex.
///
/// Missing attributes use the reference defaults: normal `(0,0,1)`, color white, uv `(0,0)`;
/// `uv1`/`uv2` copy `uv0` when the material has no TEX1/TEX2 attribute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Vertices {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// RGBA8 vertex color (CLR0).
    pub colors: Vec<[u8; 4]>,
    pub uv0: Vec<[f32; 2]>,
    pub uv1: Vec<[f32; 2]>,
    pub uv2: Vec<[f32; 2]>,
}

impl Vertices {
    pub fn len(&self) -> usize {
        self.positions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }
}

/// Result of decoding one display list: shared vertex pool plus a triangle-list index buffer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DecodedList {
    pub vertices: Vertices,
    /// Triangle-list indices (multiple of 3) into `vertices`.
    pub indices: Vec<u32>,
}

const ATTR_ORDER: [u32; 21] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, // PNMTXIDX, TEX0..7MTXIDX
    9, 10, 11, 12, // POS NRM CLR0 CLR1
    13, 14, 15, 16, 17, 18, 19, 20, // TEX0..7
];

/// Component storage format of a VAT slot. GMA display lists select it with the low 3 bits of the
/// draw command: VTXFMT0 = float32, VTXFMT1 = 16-bit fixed point with shift 13.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fmt {
    F32,
    S16,
}

/// Decode a GMA shape display list.
///
/// `vtx_attrs` is the material's attribute bitmask (attributes are present inline, in GX order).
/// Parsing stops at the end of the buffer; `NOOP` (0x00) bytes are skipped. Unlike the reference's
/// global `useVat16` switch, the VAT is chosen per command from the opcode's low bits, which is what
/// the noclip loader used by the game does.
pub fn decode_display_list(data: &[u8], vtx_attrs: u32) -> Result<DecodedList, Error> {
    let r = Reader::new(data);
    let mut out = DecodedList::default();
    let has = |a: u32| vtx_attrs & (1 << a) != 0;
    let (has_tex1, has_tex2) = (has(attr::TEX1), has(attr::TEX2));
    let nbt = has(attr::NBT);
    let mut off = 0usize;
    while off < data.len() {
        let cmd = data[off];
        off += 1;
        if cmd == 0 {
            continue;
        }
        let opcode = cmd & 0xf8;
        let fmt = match cmd & 7 {
            0 => Fmt::F32,
            1 => Fmt::S16,
            other => return Err(Error::UnsupportedVertexFormat(other)),
        };
        let prim = Primitive::from_opcode(opcode).ok_or(Error::UnknownGxCommand(cmd))?;
        let count = r.u16(off)? as u32;
        off += 2;
        if count > 0 {
            if !has(attr::POS) {
                // Without a position a vertex can be zero bytes long, and one 3-byte command would then
                // expand to 65535 vertices.
                return Err(Error::Invalid("display list vertices have no position"));
            }
            let need = count as usize * vertex_size(vtx_attrs, fmt);
            if need > data.len() - off {
                return Err(Error::OutOfBounds {
                    offset: off,
                    len: need,
                });
            }
        }
        let base = out.vertices.len() as u32;
        for _ in 0..count {
            let mut pos = [0.0; 3];
            let mut nrm = [0.0, 0.0, 1.0];
            let mut clr = [255u8; 4];
            let mut uv = [[0.0f32; 2]; 3];
            for &a in ATTR_ORDER.iter() {
                if !has(a) {
                    continue;
                }
                match a {
                    0..=8 => off += 1,
                    attr::POS | attr::NRM => {
                        let v = read_vec(&r, &mut off, fmt, 3)?;
                        if a == attr::POS {
                            pos = v;
                        } else {
                            nrm = v;
                            if nbt {
                                // binormal + tangent follow; not exposed.
                                read_vec(&r, &mut off, fmt, 3)?;
                                read_vec(&r, &mut off, fmt, 3)?;
                            }
                        }
                    }
                    attr::CLR0 | attr::CLR1 => {
                        let b = r.bytes(off, 4)?;
                        off += 4;
                        if a == attr::CLR0 {
                            clr = [b[0], b[1], b[2], b[3]];
                        }
                    }
                    attr::TEX0..=attr::TEX7 => {
                        let v = read_vec(&r, &mut off, fmt, 2)?;
                        let slot = (a - attr::TEX0) as usize;
                        if slot < 3 {
                            uv[slot] = [v[0], v[1]];
                        }
                    }
                    _ => {}
                }
            }
            if !has_tex1 {
                uv[1] = uv[0];
            }
            if !has_tex2 {
                uv[2] = uv[0];
            }
            let vs = &mut out.vertices;
            vs.positions.push(pos);
            vs.normals.push(nrm);
            vs.colors.push(clr);
            vs.uv0.push(uv[0]);
            vs.uv1.push(uv[1]);
            vs.uv2.push(uv[2]);
        }
        prim.triangulate(base, count, &mut out.indices);
    }
    Ok(out)
}

/// Bytes of one inline vertex with the attributes in `vtx_attrs`.
fn vertex_size(vtx_attrs: u32, fmt: Fmt) -> usize {
    let comp = if fmt == Fmt::F32 { 4 } else { 2 };
    let has = |a: u32| vtx_attrs & (1 << a) != 0;
    ATTR_ORDER
        .iter()
        .filter(|&&a| has(a))
        .map(|&a| match a {
            0..=8 => 1,
            attr::POS => 3 * comp,
            attr::NRM if has(attr::NBT) => 9 * comp,
            attr::NRM => 3 * comp,
            attr::CLR0 | attr::CLR1 => 4,
            _ => 2 * comp,
        })
        .sum()
}

fn read_vec(r: &Reader, off: &mut usize, fmt: Fmt, n: usize) -> Result<[f32; 3], Error> {
    let mut v = [0.0f32; 3];
    for (i, slot) in v.iter_mut().enumerate().take(n) {
        match fmt {
            Fmt::F32 => *slot = r.f32(*off + i * 4)?,
            Fmt::S16 => *slot = r.i16(*off + i * 2)? as f32 / (1u32 << S16_SHIFT) as f32,
        }
    }
    *off += n * if fmt == Fmt::F32 { 4 } else { 2 };
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_list(p: Primitive, n: u32) -> Vec<[u32; 3]> {
        triangulate(p, n)
    }

    #[test]
    fn strip_alternates_winding() {
        assert_eq!(
            tri_list(Primitive::TriangleStrip, 5),
            vec![[0, 1, 2], [2, 1, 3], [2, 3, 4]]
        );
        assert!(tri_list(Primitive::TriangleStrip, 2).is_empty());
    }

    #[test]
    fn fan_pivots_on_first() {
        assert_eq!(
            tri_list(Primitive::TriangleFan, 5),
            vec![[0, 1, 2], [0, 2, 3], [0, 3, 4]]
        );
    }

    #[test]
    fn quads_split_in_two() {
        assert_eq!(tri_list(Primitive::Quads, 8).len(), 4);
        assert_eq!(&tri_list(Primitive::Quads, 4), &[[0, 1, 2], [0, 2, 3]]);
        // Trailing partial quad is dropped.
        assert_eq!(tri_list(Primitive::Quads, 7).len(), 2);
    }

    #[test]
    fn quad_strip() {
        assert_eq!(
            tri_list(Primitive::QuadStrip, 6),
            vec![[0, 1, 2], [1, 3, 2], [2, 3, 4], [3, 5, 4]]
        );
    }

    #[test]
    fn triangles_ignore_remainder() {
        assert_eq!(
            tri_list(Primitive::Triangles, 7),
            vec![[0, 1, 2], [3, 4, 5]]
        );
    }

    fn be_f32(v: f32) -> [u8; 4] {
        v.to_be_bytes()
    }

    #[test]
    fn decodes_f32_pos_nrm_uv_strip_with_noops() {
        let attrs = (1 << attr::POS) | (1 << attr::NRM) | (1 << attr::TEX0);
        let mut d = vec![0u8, 0x98, 0, 4];
        for i in 0..4 {
            for f in [i as f32, 1.0, 2.0, 0.0, 1.0, 0.0, 0.25, 0.5] {
                d.extend_from_slice(&be_f32(f));
            }
        }
        d.extend_from_slice(&[0; 5]);
        let l = decode_display_list(&d, attrs).unwrap();
        assert_eq!(l.vertices.len(), 4);
        assert_eq!(l.vertices.positions[3], [3.0, 1.0, 2.0]);
        assert_eq!(l.vertices.normals[0], [0.0, 1.0, 0.0]);
        assert_eq!(l.vertices.uv0[2], [0.25, 0.5]);
        assert_eq!(l.vertices.uv2[2], [0.25, 0.5]); // copied from uv0
        assert_eq!(l.vertices.colors[0], [255; 4]);
        assert_eq!(l.indices, vec![0, 1, 2, 2, 1, 3]);
    }

    #[test]
    fn decodes_s16_vat_and_color_and_matrix_idx() {
        let attrs =
            (1 << attr::PNMTXIDX) | (1 << attr::POS) | (1 << attr::CLR0) | (1 << attr::TEX0);
        let mut d = vec![0x91u8, 0, 3]; // triangles, VTXFMT1
        for i in 0..3i16 {
            d.push(9); // matrix index
            for s in [8192 * (i + 1), -4096, 0] {
                d.extend_from_slice(&s.to_be_bytes());
            }
            d.extend_from_slice(&[10, 20, 30, 40]);
            for s in [8192i16, 2048] {
                d.extend_from_slice(&s.to_be_bytes());
            }
        }
        let l = decode_display_list(&d, attrs).unwrap();
        assert_eq!(l.vertices.positions[1], [2.0, -0.5, 0.0]);
        assert_eq!(l.vertices.colors[2], [10, 20, 30, 40]);
        assert_eq!(l.vertices.uv0[0], [1.0, 0.25]);
        assert_eq!(l.vertices.normals[0], [0.0, 0.0, 1.0]);
        assert_eq!(l.indices, vec![0, 1, 2]);
    }

    #[test]
    fn quads_and_fan_in_one_list_offset_indices() {
        let attrs = 1 << attr::POS;
        let mut d = vec![0x80u8, 0, 4];
        d.extend(std::iter::repeat_n(0u8, 48));
        d.extend_from_slice(&[0xa0, 0, 3]);
        d.extend(std::iter::repeat_n(0u8, 36));
        let l = decode_display_list(&d, attrs).unwrap();
        assert_eq!(l.vertices.len(), 7);
        assert_eq!(l.indices, vec![0, 1, 2, 0, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn truncated_and_unknown_are_errors() {
        let attrs = 1 << attr::POS;
        assert!(decode_display_list(&[0x90, 0, 1, 0, 0], attrs).is_err());
        assert!(decode_display_list(&[0x40, 0, 0], attrs).is_err());
        assert!(decode_display_list(&[0x92, 0, 0], attrs).is_err());
        assert_eq!(
            decode_display_list(&[], attrs).unwrap(),
            DecodedList::default()
        );
    }

    #[test]
    fn nbt_normals_consume_nine_floats() {
        let attrs = (1 << attr::POS) | (1 << attr::NRM) | (1 << attr::NBT);
        let mut d = vec![0x90u8, 0, 3];
        for _ in 0..3 {
            for f in 0..12 {
                d.extend_from_slice(&be_f32(f as f32));
            }
        }
        let l = decode_display_list(&d, attrs).unwrap();
        assert_eq!(l.vertices.normals[1], [3.0, 4.0, 5.0]);
    }
}
