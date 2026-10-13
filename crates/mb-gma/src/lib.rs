//! GMA/NaomiLib model parsers and GX display-list decoding.
//!
//! Std-only, no GPU types. Inputs are already-decompressed bytes (use `mb_assets::lzss` for `.lz`).
//! See `docs/GMA.md` for the formats and API overview.
//!
//! ```no_run
//! let bytes = std::fs::read("st001.gma").unwrap();
//! let gma = mb_gma::parse_gma(&bytes).unwrap();
//! for model in gma.iter() {
//!     for mesh in model.meshes().unwrap() {
//!         let material = &model.shapes[mesh.shape_index].material;
//!         // upload mesh.vertices / mesh.indices, bind textures via model.shape_tev_layers(..)
//!         let _ = material;
//!     }
//! }
//! ```

mod gma;
mod gx;
mod naomi;
mod reader;

pub use gma::{
    BlendFactor, Dlist, Gma, Material, Mesh, Model, Rgba8, Shape, SkippedModel, TevLayer, WrapMode,
    material_flags, model_flags, parse_gma, tev_layer_flags,
};
pub use gx::{
    CullMode, DecodedList, Primitive, S16_SHIFT, Vertices, attr, decode_display_list, triangulate,
};
pub use naomi::{
    MESH_DISABLE_DEPTH_WRITE, NlMesh, NlModel, NlObj, VertexKind, build_obj_name_map, dl_flags,
    mesh_type, parse_naomi_obj, tex_flags,
};

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A read past the end of the input.
    OutOfBounds { offset: usize, len: usize },
    /// Structurally invalid data.
    Invalid(&'static str),
    /// Display list contains an opcode that is not a draw command.
    UnknownGxCommand(u8),
    /// Display list uses a vertex format other than VTXFMT0/1.
    UnsupportedVertexFormat(u8),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OutOfBounds { offset, len } => {
                write!(f, "read of {len} bytes at {offset:#x} is out of bounds")
            }
            Error::Invalid(what) => write!(f, "invalid data: {what}"),
            Error::UnknownGxCommand(c) => write!(f, "unknown GX command {c:#04x}"),
            Error::UnsupportedVertexFormat(v) => write!(f, "unsupported vertex format {v}"),
        }
    }
}

impl std::error::Error for Error {}
