//! Mapping from GMA materials / TEV layers to GPU state. Everything here is pure CPU logic so it can be
//! unit-tested without a GPU.
//!
//! The TEV setup of the reference (`noclip/SuperMonkeyBall/Material.ts`), as the shader evaluates it:
//! * lighting channel: lit / unlit x vertex-colour / material-colour, one directional light, per vertex;
//! * a diffuse layer multiplies the previous stage (`tex * prev`, colour and alpha), an alpha-blend layer
//!   multiplies alpha by the texture's R channel;
//! * a view-specular layer adds `K0 * tex`, a world-specular layer `K1 * tex` (`K1 = K0 *` the scene's
//!   specular scale), each clamped like a TEV stage. Their texture coordinates are generated per vertex
//!   from the normal ([`view_specular_rows`], [`world_specular_rows`]);
//! * layers flagged `EnableUvScroll` take the object's texture matrix (GX `TEXMTX1`), the others identity;
//! * alpha test `> 0`, blend `src_factor / dst_factor` from the material (default SrcAlpha / InvSrcAlpha),
//!   depth test LEQUAL with depth writes on.
//!
//! `docs/GMA.md` ("How `mb-render` draws the materials") lists where this differs from the reference.

use crate::gpu_types::MaterialUniform;
use glam::{Mat4, Vec3};
use mb_gma::{
    BlendFactor, CullMode, Material, TevLayer, WrapMode, material_flags, tev_layer_flags,
};

/// Kind of a texture layer, in the order the shader expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum LayerKind {
    Diffuse = 0,
    AlphaBlend = 1,
    /// Sphere-mapped on the normal around the direction from the eye to the model: adds `K0 * tex`.
    ViewSpecular = 2,
    /// Sphere-mapped around the half vector of the light and the eye: adds `K1 * tex`.
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

/// What the shader needs to know about one texture layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LayerInfo {
    pub kind: LayerKind,
    /// `TevLayerFlags.EnableUvScroll`: the layer's texture coordinates go through the object's texture
    /// matrix (GX `TEXMTX1`) instead of the identity (`TEXMTX2`). Specular layers generate their own
    /// coordinates and ignore it, as in the reference.
    pub uv_scroll: bool,
}

impl LayerInfo {
    pub fn from_flags(flags: u32) -> Self {
        Self {
            kind: LayerKind::from_flags(flags),
            uv_scroll: flags & tev_layer_flags::ENABLE_UV_SCROLL != 0,
        }
    }
}

impl From<LayerKind> for LayerInfo {
    fn from(kind: LayerKind) -> Self {
        Self {
            kind,
            uv_scroll: false,
        }
    }
}

/// Bits of `MaterialUniform::layer_kinds[3]` above the three scroll bits.
pub const HAS_VIEW_SPECULAR: u32 = 1 << 4;
pub const HAS_WORLD_SPECULAR: u32 = 1 << 5;

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

/// True when drawing with the material's blend factors and a source alpha of one leaves exactly the
/// source colour: only then may a mesh whose alpha is always one skip blending. Additive and
/// colour-factor materials (`One / One`, `SrcAlpha / One`, `SrcClr / One`, ...) blend whatever their alpha.
pub fn opaque_at_full_alpha(material: &Material) -> bool {
    matches!(
        (material.src_blend(), material.dest_blend()),
        (
            BlendFactor::SrcAlpha | BlendFactor::One,
            BlendFactor::InvSrcAlpha | BlendFactor::Zero
        )
    )
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

/// Build the per-material uniform from the GMA material and the kinds of its texture layers (at most 3),
/// none of which takes the object's texture matrix. See [`material_uniform_layers`].
pub fn material_uniform(material: &Material, layer_kinds: &[LayerKind]) -> MaterialUniform {
    let layers: Vec<LayerInfo> = layer_kinds.iter().map(|k| LayerInfo::from(*k)).collect();
    material_uniform_layers(material, &layers)
}

/// Build the per-material uniform from the GMA material and its texture layers (at most 3).
pub fn material_uniform_layers(material: &Material, layers: &[LayerInfo]) -> MaterialUniform {
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

    let n = if simple { 0 } else { layers.len().min(3) };
    let mut kinds = [0u32; 4];
    for (i, layer) in layers.iter().take(n).enumerate() {
        kinds[i] = layer.kind as u32;
        match layer.kind {
            LayerKind::Diffuse | LayerKind::AlphaBlend => kinds[3] |= (layer.uv_scroll as u32) << i,
            LayerKind::ViewSpecular => kinds[3] |= HAS_VIEW_SPECULAR,
            LayerKind::WorldSpecular => kinds[3] |= HAS_WORLD_SPECULAR,
        }
    }
    MaterialUniform {
        mat_color,
        amb_mul,
        k0,
        flags: [(!unlit) as u32, vert as u32, n as u32, 0],
        layer_kinds: kinds,
        extra: [0.0; 4],
    }
}

// ---- specular texture coordinates -----------------------------------------------------------------
//
// The reference generates the coordinates of both specular kinds from the normal: `TEXMTX0` (the
// model-view matrix without its translation), normalised, then a post matrix built from a look-at
// (`computeViewSpecularPostTexMtx`, `computeWorldSpecularPostTexMtx`, `applySpecularPostTransform`). The
// post matrix reduces to two rows of that look-at: `s = 0.5 + 0.5 * x.n`, `t = 0.5 - 0.5 * y.n`, `q = 1`.
// shader.wgsl holds the same maths (`look_rows`, `spec_uv`); these functions are its CPU twin.

/// How far the eye direction counts against the light direction in the world-specular half vector
/// (`vec3.scale(scaledModelDir, modelDirView, -0.9)`).
pub const WORLD_SPECULAR_EYE_WEIGHT: f32 = 0.9;

/// The x and y rows of gl-matrix `lookAt(origin, target, up)`: the identity's rows when the target is
/// the origin, zero where a cross product vanishes.
pub fn look_rows(target: Vec3, up: Vec3) -> (Vec3, Vec3) {
    let len = target.length();
    if len.is_nan() || len <= 1e-6 {
        return (Vec3::X, Vec3::Y);
    }
    let z = -target / len;
    let x = up.cross(z).normalize_or_zero();
    let y = z.cross(x).normalize_or_zero();
    (x, y)
}

/// The view-space direction the specular look-ats are built around: from the eye to the model's
/// bounding-sphere centre pushed back by its radius (`scratchVec3a` of `setOnRenderInst`).
pub fn model_dir_view(view_from_model: &Mat4, bound_center: Vec3, bound_radius: f32) -> Vec3 {
    let mut c = view_from_model.transform_point3(bound_center);
    c.z -= bound_radius;
    let len = c.length();
    if len.is_nan() || len <= 1e-6 {
        Vec3::NEG_Z
    } else {
        c / len
    }
}

/// Rows of `computeViewSpecularPostTexMtx`.
pub fn view_specular_rows(model_dir: Vec3) -> (Vec3, Vec3) {
    look_rows(model_dir, Vec3::Y)
}

/// Rows of `computeWorldSpecularPostTexMtx` (its first matrix; the second one addresses the reference's
/// 16x4 ramp texture at `t = 0`, which is white all along, so the ramp is the constant one).
/// `light_dir` is the unit view-space direction towards the light.
pub fn world_specular_rows(model_dir: Vec3, light_dir: Vec3) -> (Vec3, Vec3) {
    let scaled = model_dir * -WORLD_SPECULAR_EYE_WEIGHT;
    let target = (light_dir + scaled) * 0.5;
    look_rows(target, target - scaled)
}

/// Texture coordinates of a specular layer for a view-space normal (any length; zero gives the centre).
pub fn specular_uv(rows: (Vec3, Vec3), normal_view: Vec3) -> [f32; 2] {
    let n = normal_view.normalize_or_zero();
    [0.5 + 0.5 * rows.0.dot(n), 0.5 - 0.5 * rows.1.dot(n)]
}

/// `texMtx` applied to a texture coordinate the way GX `MTX2x4` does: the input is `(s, t, 1, 1)`.
pub fn apply_tex_matrix(rows: &[[f32; 4]; 2], uv: [f32; 2]) -> [f32; 2] {
    let v = [uv[0], uv[1], 1.0, 1.0];
    let dot = |r: &[f32; 4]| r[0] * v[0] + r[1] * v[1] + r[2] * v[2] + r[3] * v[3];
    [dot(&rows[0]), dot(&rows[1])]
}

/// The rows [`crate::Renderer::set_tex_matrix`] takes, from a column-major 4x4 (`RenderParams.texMtx`).
pub fn tex_matrix_rows(m: &Mat4) -> [[f32; 4]; 2] {
    [m.row(0).to_array(), m.row(1).to_array()]
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
    // The reference's texture matrix is `t' = -t`. Under Repeat that is `1 - t`, which also keeps
    // clamped textures authored in 0..1 on the texture; a mirrored wrap needs the true negation.
    let flip = if textured && mesh.wrap().1 == WrapMode::Mirror {
        2.0
    } else {
        1.0
    };
    MaterialUniform {
        mat_color: mesh.material_color,
        amb_mul: [s, s, s, 1.0],
        k0: [1.0; 4],
        flags: [lit as u32, vert as u32, 0, combine],
        layer_kinds: [0; 4],
        extra: [flip, 0.0, 0.0, 0.0],
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

    #[test]
    fn layer_info_reads_kind_and_scroll_flag() {
        use tev_layer_flags::*;
        let plain = LayerInfo::from_flags(0x780);
        assert_eq!((plain.kind, plain.uv_scroll), (LayerKind::Diffuse, false));
        let scrolled = LayerInfo::from_flags(ENABLE_UV_SCROLL | TYPE_ALPHA_BLEND);
        assert_eq!(
            (scrolled.kind, scrolled.uv_scroll),
            (LayerKind::AlphaBlend, true)
        );
        assert!(!LayerInfo::from(LayerKind::WorldSpecular).uv_scroll);
    }

    #[test]
    fn uniform_carries_every_layer_kind_and_its_bits() {
        let scroll = |kind| LayerInfo {
            kind,
            uv_scroll: true,
        };
        // Diffuse (scrolled), world specular, alpha (scrolled).
        let u = material_uniform_layers(
            &mat(0),
            &[
                scroll(LayerKind::Diffuse),
                LayerKind::WorldSpecular.into(),
                scroll(LayerKind::AlphaBlend),
            ],
        );
        assert_eq!(u.flags[2], 3);
        assert_eq!(&u.layer_kinds[..3], &[0, 3, 1]);
        assert_eq!(u.layer_kinds[3], 0b101 | HAS_WORLD_SPECULAR);

        // A specular layer generates its coordinates: its scroll flag is not a scroll bit.
        let u = material_uniform_layers(
            &mat(0),
            &[
                LayerKind::Diffuse.into(),
                scroll(LayerKind::ViewSpecular),
                scroll(LayerKind::WorldSpecular),
            ],
        );
        assert_eq!(&u.layer_kinds[..3], &[0, 2, 3]);
        assert_eq!(u.layer_kinds[3], HAS_VIEW_SPECULAR | HAS_WORLD_SPECULAR);

        // The old entry point is the same without scroll flags; plain materials set no bit at all.
        let kinds = [LayerKind::Diffuse, LayerKind::AlphaBlend];
        let u = material_uniform(&mat(0), &kinds);
        assert_eq!(u.layer_kinds, [0, 1, 0, 0]);
        assert_eq!(
            u,
            material_uniform_layers(&mat(0), &kinds.map(LayerInfo::from))
        );
        // At most three layers; a simple material has none and no bits.
        let four = [LayerInfo::from(LayerKind::ViewSpecular); 4];
        assert_eq!(material_uniform_layers(&mat(0), &four).flags[2], 3);
        let simple = material_uniform_layers(&mat(material_flags::SIMPLE_MATERIAL), &four);
        assert_eq!((simple.flags[2], simple.layer_kinds), (0, [0; 4]));
    }

    #[test]
    fn specular_colour_is_the_konst_colour() {
        let m = Material {
            specular_color: Rgba8 {
                r: 255,
                g: 0,
                b: 51,
                a: 7,
            },
            ..mat(0)
        };
        let u = material_uniform(&m, &[LayerKind::WorldSpecular]);
        assert_eq!(u.k0, [1.0, 0.0, 0.2, 1.0]);
    }

    #[test]
    fn only_default_like_blends_may_skip_blending() {
        let with = |factors: u32| Material {
            blend_factors: factors,
            ..mat(material_flags::CUSTOM_BLEND_SRC | material_flags::CUSTOM_BLEND_DEST)
        };
        assert!(opaque_at_full_alpha(&mat(0)), "SrcAlpha / InvSrcAlpha");
        assert!(opaque_at_full_alpha(&with(0x01)), "One / Zero");
        assert!(opaque_at_full_alpha(&with(0x54)), "explicit default");
        // What the disc uses besides the default: all of them add to or scale the frame.
        for (factors, what) in [
            (0x11, "One / One"),
            (0x14, "SrcAlpha / One"),
            (0x12, "SrcClr / One"),
            (0x30, "Zero / InvSrcClr"),
        ] {
            assert!(!opaque_at_full_alpha(&with(factors)), "{what}");
        }
    }

    /// The reference's `applySpecularPostTransform` on a look-at matrix, applied to a unit normal the
    /// way a GX `MTX3x4` tex gen with a post matrix does (`s / q`, `t / q`).
    fn reference_spec_uv(target: Vec3, up: Vec3, n: Vec3) -> [f32; 2] {
        // gl-matrix `lookAt` with the eye at the origin is glam's right-handed look-at.
        let look = if target.length() <= 1e-6 {
            Mat4::IDENTITY
        } else {
            Mat4::look_at_rh(Vec3::ZERO, target, up)
        };
        let mut m = look.to_cols_array_2d();
        let (get, set) = (
            |m: &[[f32; 4]; 4], row: usize, col: usize| m[col][row],
            |m: &mut [[f32; 4]; 4], row: usize, col: usize, v: f32| m[col][row] = v,
        );
        set(&mut m, 0, 3, 0.5);
        for col in 0..3 {
            let v = -get(&m, 1, col);
            set(&mut m, 1, col, v);
        }
        set(&mut m, 1, 3, 0.5);
        for col in 0..3 {
            set(&mut m, 2, col, 0.0);
        }
        set(&mut m, 2, 3, 1.0);
        for col in m.iter_mut().take(3) {
            for v in col.iter_mut().take(3) {
                *v *= 0.5;
            }
        }
        let out = Mat4::from_cols_array_2d(&m) * n.extend(1.0);
        [out.x / out.z, out.y / out.z]
    }

    fn close(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5
    }

    #[test]
    fn view_specular_matches_the_reference_post_matrix() {
        let view = Mat4::look_at_rh(Vec3::new(4.0, 6.0, 9.0), Vec3::new(1.0, 0.0, -2.0), Vec3::Y);
        let model = Mat4::from_translation(Vec3::new(-3.0, 1.0, 2.0)) * Mat4::from_rotation_y(0.7);
        let view_from_model = view * model;
        let dir = model_dir_view(&view_from_model, Vec3::new(0.5, 0.25, 0.0), 2.0);
        // `scratchVec3a`: the centre in view space, pushed back by the radius, normalised.
        let mut c = view_from_model.transform_point3(Vec3::new(0.5, 0.25, 0.0));
        c.z -= 2.0;
        assert!((dir - c.normalize()).length() < 1e-6);
        for normal in [Vec3::X, Vec3::Y, Vec3::new(0.3, -0.5, 0.8), Vec3::NEG_Z] {
            // `TEXMTX0` is the model-view matrix without translation; the tex gen normalises.
            let n = view_from_model.transform_vector3(normal).normalize();
            let uv = specular_uv(view_specular_rows(dir), n);
            assert!(
                close(uv, reference_spec_uv(dir, Vec3::Y, n)),
                "{normal:?}: {uv:?}"
            );
        }
        // A normal pointing back along the model direction lands in the middle of the map.
        let uv = specular_uv(view_specular_rows(dir), -dir);
        assert!(close(uv, [0.5, 0.5]), "{uv:?}");
        // Straight ahead of the camera the basis is the view axes: the plain sphere map.
        let uv = specular_uv(view_specular_rows(Vec3::NEG_Z), Vec3::new(0.6, 0.8, 0.0));
        assert!(close(uv, [0.8, 0.1]), "{uv:?}");
    }

    #[test]
    fn world_specular_matches_the_reference_post_matrix() {
        let dir = Vec3::new(0.2, -0.1, -1.0).normalize();
        let light = Vec3::new(-0.4, 0.7, 0.5).normalize();
        // `computeWorldSpecularPostTexMtx`, operation by operation.
        let scaled = dir * -0.9;
        let target = (light + scaled) * 0.5;
        let up = target - scaled;
        for normal in [Vec3::Z, Vec3::new(0.5, 0.5, 0.7).normalize(), light, -dir] {
            let uv = specular_uv(world_specular_rows(dir, light), normal);
            assert!(
                close(uv, reference_spec_uv(target, up, normal)),
                "{normal:?}: {uv:?}"
            );
        }
        // The highlight sits where the normal is the half vector of light and eye.
        let uv = specular_uv(world_specular_rows(dir, light), target.normalize());
        assert!(close(uv, [0.5, 0.5]), "{uv:?}");
        // A degenerate half vector falls back to the identity, as gl-matrix `lookAt` does.
        let rows = world_specular_rows(Vec3::NEG_Z, Vec3::NEG_Z * WORLD_SPECULAR_EYE_WEIGHT);
        assert_eq!(rows, (Vec3::X, Vec3::Y));
        // No normal: the middle, never NaN.
        assert_eq!(specular_uv(rows, Vec3::ZERO), [0.5, 0.5]);
        assert_eq!(
            model_dir_view(&Mat4::IDENTITY, Vec3::ZERO, 0.0),
            Vec3::NEG_Z
        );
    }

    #[test]
    fn tex_matrix_is_a_gx_2x4() {
        let identity = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]];
        assert_eq!(apply_tex_matrix(&identity, [0.25, 0.75]), [0.25, 0.75]);
        // `mat4.fromTranslation(texMtx, [0.5, -0.25, 9])`: the translation lands in the fourth column.
        let m = Mat4::from_translation(Vec3::new(0.5, -0.25, 9.0));
        let rows = tex_matrix_rows(&m);
        assert_eq!(rows, [[1.0, 0.0, 0.0, 0.5], [0.0, 1.0, 0.0, -0.25]]);
        assert_eq!(apply_tex_matrix(&rows, [0.25, 0.75]), [0.75, 0.5]);
        // The third column multiplies the constant one of the `(s, t, 1, 1)` input.
        let rows = [[2.0, 0.0, 0.125, 0.0], [0.0, 0.0, 0.0, 1.0]];
        assert_eq!(apply_tex_matrix(&rows, [0.25, 0.75]), [0.625, 1.0]);
    }

    fn nl_mesh(tex_flags: u32, textured: bool) -> mb_gma::NlMesh {
        mb_gma::NlMesh {
            flags: 0,
            tex_flags,
            tex_idx: textured.then_some(0),
            mesh_type: mb_gma::mesh_type::UNLIT_CONST_MAT_COLOR,
            ambient_color_scale: 0.5,
            material_color: [1.0; 4],
            cull: CullMode::None,
            z_compare: 4,
            depth_write: true,
            vertices: mb_gma::Vertices::default(),
            indices: Vec::new(),
        }
    }

    #[test]
    fn naomi_meshes_negate_v_only_under_a_mirrored_wrap() {
        // Walk the flag bits until every wrap mode of T has been seen once.
        let mut flips = Vec::new();
        for tex_flags in 0..(1u32 << 20) {
            let mesh = nl_mesh(tex_flags, true);
            let wrap_t = mesh.wrap().1;
            if flips.contains(&wrap_t) {
                continue;
            }
            flips.push(wrap_t);
            let u = naomi_material_uniform(&mesh, false, true);
            let want = if wrap_t == WrapMode::Mirror { 2.0 } else { 1.0 };
            assert_eq!(u.extra[0], want, "{wrap_t:?}");
            assert_eq!(u.amb_mul, [0.5, 0.5, 0.5, 1.0]);
            if flips.len() == 3 {
                break;
            }
        }
        assert_eq!(flips.len(), 3, "clamp, repeat and mirror all occur");
        assert_eq!(
            naomi_material_uniform(&nl_mesh(0, false), false, false).flags[3],
            1
        );
    }
}
