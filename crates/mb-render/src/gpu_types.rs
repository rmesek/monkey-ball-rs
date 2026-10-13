//! Plain-old-data structs shared with the WGSL shaders. Layout is checked by unit tests (no GPU needed).

use bytemuck::{Pod, Zeroable};

/// Interleaved vertex: position, normal, RGBA8 colour, three UV sets.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [u8; 4],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub uv2: [f32; 2],
}

impl Vertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Unorm8x4,
        3 => Float32x2,
        4 => Float32x2,
        5 => Float32x2,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRIBUTES,
        }
    }
}

/// Group 0: per-pass scene constants.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct SceneUniform {
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    /// World-space stage tilt for objects that ask for it (`ObjectUniform::params.y`).
    pub tilt: [[f32; 4]; 4],
    pub light_dir: [f32; 4],
    pub light_color: [f32; 4],
    pub ambient: [f32; 4],
    pub spec_scale: [f32; 4],
    /// w: the target is sRGB (the shaders then convert their output to linear).
    pub target_info: [f32; 4],
    /// World (before the stage tilt) -> the space of the projected shadow: the light looks down -Z at the caster.
    pub shadow_from_world: [[f32; 4]; 4],
    pub shadow_color: [f32; 4],
    /// x: radius, y: z of the caster in shadow space, z: fade per unit behind the caster, w: 1 = enabled.
    pub shadow_info: [f32; 4],
}

/// Group 1: per-object constants.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ObjectUniform {
    pub model: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 4],
    pub color_mul: [f32; 4],
    /// x: object alpha, y: 1 when the scene's stage tilt applies, z: 1 when the projected shadow falls on it.
    pub params: [f32; 4],
}

/// What follows [`ObjectUniform`] in an object's slot of the uniform buffer: the per-object material
/// parameters of the reference's `RenderParams` and the vertex wave.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct ObjectExtra {
    /// The two rows of GX `TEXMTX1` (`RenderParams.texMtx`): `s' = dot(row0, (s, t, 1, 1))`.
    pub tex_mtx: [[f32; 4]; 2],
    /// xyz: the model's bounding-sphere centre in model space, w: its radius (specular tex gens).
    pub bound: [f32; 4],
    /// x: view-specular factor, y: world-specular factor (0 or 1), z: 1 = the wave displaces the vertices,
    /// w: 1 = the texture matrix applies to every diffuse and alpha layer, flagged or not.
    pub spec: [f32; 4],
    /// x: amplitude at the centre, y: amplitude per unit of distance, z: angle per unit of distance,
    /// w: angle at the centre now (S16 angle units, 65536 = one turn).
    pub wave: [f32; 4],
}

impl Default for ObjectExtra {
    /// Identity texture matrix, specular on, no wave: what an object has until told otherwise.
    fn default() -> Self {
        Self {
            tex_mtx: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]],
            bound: [0.0; 4],
            spec: [1.0, 1.0, 0.0, 0.0],
            wave: [0.0; 4],
        }
    }
}

/// Group 1 as the shader sees it (`Object` in shader.wgsl): one slot of the object uniform buffer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ObjectSlot {
    pub base: ObjectUniform,
    pub extra: ObjectExtra,
}

/// Group 2: per-material constants (textures/samplers are separate bindings).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct MaterialUniform {
    pub mat_color: [f32; 4],
    pub amb_mul: [f32; 4],
    pub k0: [f32; 4],
    pub flags: [u32; 4],
    /// xyz: the kind of layers 0..2 ([`crate::material::LayerKind`]), w: bits 0..2 = the layer takes the
    /// object's texture matrix (`EnableUvScroll`), bit 4 / 5 = some layer is view / world specular.
    pub layer_kinds: [u32; 4],
    /// x: the V texture coordinate of NaomiLib meshes: 1 = `1 - v`, 2 = `-v`.
    pub extra: [f32; 4],
}

/// Sprite vertex (screen-space pixels).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct SpriteVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl SpriteVertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
}

/// One instance of an instanced model draw: the rows of its affine model matrix and a colour multiplier.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct InstanceRaw {
    pub rows: [[f32; 4]; 3],
    pub color: [f32; 4],
}

impl InstanceRaw {
    /// After the six attributes of [`Vertex`].
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        6 => Float32x4,
        7 => Float32x4,
        8 => Float32x4,
        9 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<InstanceRaw>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRIBUTES,
        }
    }

    pub fn new(transform: &glam::Mat4, color: [f32; 4]) -> Self {
        let m = transform;
        Self {
            rows: [
                m.row(0).to_array(),
                m.row(1).to_array(),
                m.row(2).to_array(),
            ],
            color,
        }
    }
}

/// Vertex of the world-space quad batch (the reference's 24-byte streak vertex).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct QuadVertex {
    pub pos: [f32; 3],
    pub color: [u8; 4],
    pub uv: [f32; 2],
}

impl QuadVertex {
    pub const ATTRIBUTES: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Float32x2];
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn layouts_match_wgsl() {
        // WGSL uniform structs: mat4 = 64, vec4 = 16.
        assert_eq!(size_of::<SceneUniform>(), 64 * 4 + 16 * 7);
        assert_eq!(size_of::<ObjectUniform>(), 64 * 2 + 32);
        assert_eq!(size_of::<MaterialUniform>(), 16 * 6);
        assert_eq!(size_of::<Vertex>(), 12 + 12 + 4 + 8 * 3);
        assert_eq!(size_of::<SpriteVertex>(), 32);
        assert_eq!(size_of::<InstanceRaw>(), 64);
        assert_eq!(size_of::<QuadVertex>(), 24);
        // Uniform sizes must be multiples of 16.
        assert_eq!(size_of::<SceneUniform>() % 16, 0);
        assert_eq!(size_of::<ObjectUniform>() % 16, 0);
        assert_eq!(size_of::<MaterialUniform>() % 16, 0);
    }

    #[test]
    fn object_slot_is_the_object_uniform_followed_by_five_vec4() {
        // `Object` in shader.wgsl: two mat4, two vec4, then the members of `ObjectExtra`.
        assert_eq!(size_of::<ObjectExtra>(), 16 * 5);
        assert_eq!(size_of::<ObjectSlot>(), 64 * 2 + 16 * 7);
        assert_eq!(std::mem::offset_of!(ObjectSlot, extra), 160);
        assert_eq!(std::mem::offset_of!(ObjectExtra, bound), 32);
        assert_eq!(std::mem::offset_of!(ObjectExtra, spec), 48);
        assert_eq!(std::mem::offset_of!(ObjectExtra, wave), 64);
        let d = ObjectExtra::default();
        assert_eq!(d.tex_mtx, [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0]]);
        assert_eq!(d.spec, [1.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn vertex_attributes_cover_the_struct() {
        let l = Vertex::layout();
        assert_eq!(l.array_stride, size_of::<Vertex>() as u64);
        let last = Vertex::ATTRIBUTES.last().unwrap();
        assert_eq!(last.offset + 8, l.array_stride);
        let l = InstanceRaw::layout();
        let last = InstanceRaw::ATTRIBUTES.last().unwrap();
        assert_eq!(last.offset + 16, l.array_stride);
        // WebGL2 guarantees 16 vertex attributes; an instanced draw uses both layouts.
        assert!(Vertex::ATTRIBUTES.len() + InstanceRaw::ATTRIBUTES.len() <= 16);
        assert_eq!(last.shader_location, 9);
    }

    #[test]
    fn instance_rows_are_the_affine_matrix() {
        let m = glam::Mat4::from_translation(glam::Vec3::new(1.0, 2.0, 3.0))
            * glam::Mat4::from_scale(glam::Vec3::splat(2.0));
        let raw = InstanceRaw::new(&m, [1.0; 4]);
        assert_eq!(raw.rows[0], [2.0, 0.0, 0.0, 1.0]);
        assert_eq!(raw.rows[1], [0.0, 2.0, 0.0, 2.0]);
        assert_eq!(raw.rows[2], [0.0, 0.0, 2.0, 3.0]);
    }
}
