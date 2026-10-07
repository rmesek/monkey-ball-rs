//! Mapping from GMA materials / TEV layers to GPU state. Everything here is pure CPU logic so it can be
//! unit-tested without a GPU.
//!
//! Approximation of the reference TEV setup (`noclip/SuperMonkeyBall/Material.ts`):
//! * lighting channel: lit / unlit x vertex-colour / material-colour, one directional light, per-vertex;
//! * texture layers modulate the previous stage (`tex * prev`), alpha-blend layers multiply alpha by the
//!   texture's R channel, view-specular layers add `konst0 * tex(matcap)`;
//! * alpha test `> 0`, blend `src_factor / dst_factor` from the material (default SrcAlpha / InvSrcAlpha),
//!   depth test LEQUAL with depth writes on.

use crate::gpu_types::MaterialUniform;
use mb_gma::{
    material_flags, tev_layer_flags, BlendFactor, CullMode, Material, TevLayer, WrapMode,
};

/// Kind of a texture layer, in the order the shader expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum LayerKind {
    Diffuse = 0,
    AlphaBlend = 1,
    ViewSpecular = 2,
    /// Not emulated yet; the shader skips it.
    WorldSpecular = 3,
}

impl LayerKind {
    /// Same precedence as the reference: alpha-blend, then view specular, then world specular.
    pub fn from_flags(flags: u32) -> Self {
        if flags & tev_layer_flags::TYPE_ALPHA_BLEND != 0 {
            LayerKind::AlphaBlend
        } else if flags & tev_layer_flags::TYPE_VIEW_SPECULAR != 0 {
            LayerKind::ViewSpecular
        } else if flags & tev_layer_flags::TYPE_WORLD_SPECULAR != 0 {
            LayerKind::WorldSpecular
        } else {
            LayerKind::Diffuse
        }
    }
}

/// Everything that selects a render pipeline. Kept small on purpose: all other material differences are
/// uniforms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipelineKey {
    /// `None` = blending disabled (NaomiLib meshes).
    pub blend: Option<(wgpu::BlendFactor, wgpu::BlendFactor)>,
    /// `None` = draw both faces.
    pub cull: Option<wgpu::Face>,
    pub depth_compare: wgpu::CompareFunction,
    pub depth_write: bool,
}

/// Source-side GX factor -> wgpu. GX `SRCCLR` as a *source* factor means the destination colour.
pub fn src_factor(f: BlendFactor) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as W;
    match f {
        BlendFactor::Zero => W::Zero,
        BlendFactor::One => W::One,
        BlendFactor::SrcClr => W::Dst,
        BlendFactor::InvSrcClr => W::OneMinusDst,
        BlendFactor::SrcAlpha => W::SrcAlpha,
        BlendFactor::InvSrcAlpha => W::OneMinusSrcAlpha,
        BlendFactor::DstAlpha => W::DstAlpha,
        BlendFactor::InvDstAlpha => W::OneMinusDstAlpha,
    }
}

/// Destination-side GX factor -> wgpu. `SRCCLR` as a destination factor is the source colour.
pub fn dst_factor(f: BlendFactor) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as W;
    match f {
        BlendFactor::SrcClr => W::Src,
        BlendFactor::InvSrcClr => W::OneMinusSrc,
        other => src_factor(other),
    }
}

pub fn cull_face(c: CullMode) -> Option<wgpu::Face> {
    match c {
        CullMode::Front => Some(wgpu::Face::Front),
        CullMode::Back => Some(wgpu::Face::Back),
        // `All` never occurs in the data; treat like none rather than drawing nothing silently.
        CullMode::None | CullMode::All => None,
    }
}

pub fn pipeline_key(material: &Material, cull: CullMode) -> PipelineKey {
    PipelineKey {
        blend: Some((
            src_factor(material.src_blend()),
            dst_factor(material.dest_blend()),
        )),
        cull: cull_face(cull),
        depth_compare: wgpu::CompareFunction::LessEqual,
        depth_write: true,
    }
}

/// Texture sampler state of a layer (cache key).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SamplerKey {
    pub wrap_s: WrapKey,
    pub wrap_t: WrapKey,
    /// Maximum LOD in whole mip levels (0 = no mip filtering, as in the reference).
    pub max_lod: u32,
    /// Point sampling (NaomiLib meshes without the filter flags).
    pub nearest: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WrapKey {
    Clamp,
    Repeat,
    Mirror,
}

impl From<WrapMode> for WrapKey {
    fn from(w: WrapMode) -> Self {
        match w {
            WrapMode::Clamp => WrapKey::Clamp,
            WrapMode::Repeat => WrapKey::Repeat,
            WrapMode::Mirror => WrapKey::Mirror,
        }
    }
}

impl WrapKey {
    pub fn to_wgpu(self) -> wgpu::AddressMode {
        match self {
            WrapKey::Clamp => wgpu::AddressMode::ClampToEdge,
            WrapKey::Repeat => wgpu::AddressMode::Repeat,
            WrapKey::Mirror => wgpu::AddressMode::MirrorRepeat,
        }
    }
}

pub fn sampler_key(layer: &TevLayer, tex_width: u32, tex_height: u32) -> SamplerKey {
    SamplerKey {
        wrap_s: layer.wrap_s().into(),
        wrap_t: layer.wrap_t().into(),
        max_lod: layer.max_lod(tex_width, tex_height).round() as u32,
        nearest: false,
    }
}

fn rgba_f32(c: mb_gma::Rgba8) -> [f32; 4] {
    [
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        c.a as f32 / 255.0,
    ]
}

/// Build the per-material uniform from the GMA material and the kinds of its texture layers (at most 3).
pub fn material_uniform(material: &Material, layer_kinds: &[LayerKind]) -> MaterialUniform {
    let f = material.flags;
    let unlit = f & material_flags::UNLIT != 0;
    let vert = f & material_flags::VERT_COLORS != 0;
    let custom_amb = f & material_flags::CUSTOM_MAT_AMB_COLORS != 0;
    let simple = f & material_flags::SIMPLE_MATERIAL != 0;

    // Material colour register: the model colour only applies for custom-colour / simple materials.
    let mut mat_color = if custom_amb || simple {
        rgba_f32(material.material_color)
    } else {
        [1.0, 1.0, 1.0, 1.0]
    };
    mat_color[3] = material.alpha;

    let amb_mul = if custom_amb {
        let a = rgba_f32(material.ambient_color);
        [a[0], a[1], a[2], 1.0]
    } else {
        [1.0; 4]
    };

    let mut k0 = rgba_f32(material.specular_color);
    if k0[0] == 0.0 && k0[1] == 0.0 && k0[2] == 0.0 {
        k0 = [1.0, 1.0, 1.0, 1.0];
    }
    k0[3] = 1.0;

    let n = layer_kinds.len().min(3);
    let mut kinds = [0u32; 4];
    for (i, k) in layer_kinds.iter().take(3).enumerate() {
        kinds[i] = *k as u32;
    }
    MaterialUniform {
        mat_color,
        amb_mul,
        k0,
        flags: [
            (!unlit) as u32,
            vert as u32,
            if simple { 0 } else { n as u32 },
            0,
        ],
        layer_kinds: kinds,
        extra: [0.0; 4],
    }
}

// ---- NaomiLib meshes ------------------------------------------------------------------------------

/// DC depth-compare selector (`mesh.flags >> 29`) -> wgpu, as in the reference table.
pub fn naomi_depth_compare(selector: u32) -> wgpu::CompareFunction {
    use wgpu::CompareFunction as C;
    [
        C::Never,
        C::GreaterEqual,
        C::Equal,
        C::GreaterEqual,
        C::LessEqual,
        C::NotEqual,
        C::LessEqual,
        C::Always,
    ][(selector & 7) as usize]
}

pub fn naomi_pipeline_key(mesh: &mb_gma::NlMesh) -> PipelineKey {
    PipelineKey {
        blend: None,
        cull: cull_face(mesh.cull),
        depth_compare: naomi_depth_compare(mesh.z_compare),
        depth_write: mesh.depth_write,
    }
}

pub fn naomi_sampler_key(mesh: &mb_gma::NlMesh) -> SamplerKey {
    let (s, t) = mesh.wrap();
    SamplerKey {
        wrap_s: s.into(),
        wrap_t: t.into(),
        max_lod: 0,
        nearest: mesh.point_filter(),
    }
}

/// Uniform for a NaomiLib mesh. `kind_a` meshes carry vertex colours and are always unlit.
pub fn naomi_material_uniform(
    mesh: &mb_gma::NlMesh,
    kind_a: bool,
    textured: bool,
) -> MaterialUniform {
    use mb_gma::mesh_type;
    let (lit, vert) = if kind_a {
        // mb-gma decodes "type A" models by reading normals as colours (the reference never takes this
        // branch: its flag test is `flags & 0`), so ignore those colours and draw unlit.
        (false, false)
    } else {
        match mesh.mesh_type {
            mesh_type::UNLIT_CONST_MAT_COLOR => (false, false),
            mesh_type::UNLIT_VERT_MAT_COLOR => (false, true),
            _ => (true, false),
        }
    };
    let combine = if textured {
        2 + ((mesh.tex_flags >> 6) & 3)
    } else {
        1
    };
    let s = mesh.ambient_color_scale;
    MaterialUniform {
        mat_color: mesh.material_color,
        amb_mul: [s, s, s, 1.0],
        k0: [1.0; 4],
        flags: [lit as u32, vert as u32, 0, combine],
        layer_kinds: [0; 4],
        extra: [1.0, 0.0, 0.0, 0.0],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mb_gma::Rgba8;

    fn mat(flags: u32) -> Material {
        Material {
            flags,
            material_color: Rgba8 {
                r: 255,
                g: 128,
                b: 0,
                a: 255,
            },
            ambient_color: Rgba8 {
                r: 0,
                g: 255,
                b: 0,
                a: 255,
            },
            specular_color: Rgba8 {
                r: 0,
                g: 0,
                b: 0,
                a: 0,
            },
            alpha: 0.5,
            tev_layer_count: 1,
            unk_14: 0,
            unk_15: 0,
            tev_layer_idxs: [0, -1, -1],
            vtx_attrs: 0,
            blend_factors: 0,
        }
    }

    #[test]
    fn default_blend_is_src_alpha() {
        let k = pipeline_key(&mat(0), CullMode::Back);
        assert_eq!(
            k.blend,
            Some((
                wgpu::BlendFactor::SrcAlpha,
                wgpu::BlendFactor::OneMinusSrcAlpha
            ))
        );
        assert!(k.depth_write);
        assert_eq!(k.cull, Some(wgpu::Face::Back));
    }

    #[test]
    fn custom_blend_factors_map_with_gx_colour_swap() {
        // src = SRCCLR (2), dst = INVSRCCLR (3).
        let m = Material {
            blend_factors: 0x32,
            ..mat(material_flags::CUSTOM_BLEND_SRC | material_flags::CUSTOM_BLEND_DEST)
        };
        let k = pipeline_key(&m, CullMode::None);
        assert_eq!(
            k.blend,
            Some((wgpu::BlendFactor::Dst, wgpu::BlendFactor::OneMinusSrc))
        );
        assert_eq!(k.cull, None);
    }

    #[test]
    fn one_one_additive() {
        let m = Material {
            blend_factors: 0x11,
            ..mat(material_flags::CUSTOM_BLEND_SRC | material_flags::CUSTOM_BLEND_DEST)
        };
        let k = pipeline_key(&m, CullMode::Front);
        assert_eq!(
            k.blend,
            Some((wgpu::BlendFactor::One, wgpu::BlendFactor::One))
        );
        assert_eq!(k.cull, Some(wgpu::Face::Front));
    }

    #[test]
    fn layer_kind_precedence() {
        use tev_layer_flags::*;
        assert_eq!(LayerKind::from_flags(0), LayerKind::Diffuse);
        assert_eq!(
            LayerKind::from_flags(TYPE_ALPHA_BLEND | TYPE_VIEW_SPECULAR),
            LayerKind::AlphaBlend
        );
        assert_eq!(
            LayerKind::from_flags(TYPE_VIEW_SPECULAR | TYPE_WORLD_SPECULAR),
            LayerKind::ViewSpecular
        );
        assert_eq!(
            LayerKind::from_flags(TYPE_WORLD_SPECULAR),
            LayerKind::WorldSpecular
        );
    }

    #[test]
    fn uniform_lit_plain_material_uses_white_and_scene_ambient() {
        let u = material_uniform(&mat(0), &[LayerKind::Diffuse]);
        assert_eq!(u.flags, [1, 0, 1, 0]);
        assert_eq!(u.mat_color, [1.0, 1.0, 1.0, 0.5]);
        assert_eq!(u.amb_mul, [1.0; 4]);
        assert_eq!(u.k0, [1.0; 4], "zero specular falls back to white");
    }

    #[test]
    fn uniform_custom_colours_and_unlit() {
        let u = material_uniform(
            &mat(material_flags::CUSTOM_MAT_AMB_COLORS | material_flags::UNLIT),
            &[],
        );
        assert_eq!(u.flags[0], 0);
        assert_eq!(u.mat_color[1], 128.0 / 255.0);
        assert_eq!(u.amb_mul, [0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn simple_material_has_no_layers() {
        let u = material_uniform(&mat(material_flags::SIMPLE_MATERIAL), &[LayerKind::Diffuse]);
        assert_eq!(u.flags[2], 0);
        assert_eq!(u.mat_color[0], 255.0 / 255.0);
    }

    #[test]
    fn sampler_key_from_layer() {
        let l = TevLayer {
            flags: (1 << 2) | (2 << 4) | (3 << 7),
            tex_idx: 0,
            lod_bias: 0,
            max_aniso: 0,
            unk_0c: 0,
            unk_10: 0,
            alpha_type: 0,
            color_type: 0,
            swappable: false,
        };
        let k = sampler_key(&l, 64, 64);
        assert_eq!(k.wrap_s, WrapKey::Repeat);
        assert_eq!(k.wrap_t, WrapKey::Mirror);
        assert_eq!(k.max_lod, 3);
        assert_eq!(sampler_key(&l, 64, 32).max_lod, 0);
    }
}
