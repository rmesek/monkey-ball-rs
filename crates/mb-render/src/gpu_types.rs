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
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct SceneUniform {
    pub view: [[f32; 4]; 4],
    pub proj: [[f32; 4]; 4],
    pub light_dir: [f32; 4],
    pub light_color: [f32; 4],
    pub ambient: [f32; 4],
    pub spec_scale: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog_params: [f32; 4],
}

/// Group 1: per-object constants.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
pub struct ObjectUniform {
    pub model: [[f32; 4]; 4],
    pub normal_mat: [[f32; 4]; 4],
    pub color_mul: [f32; 4],
    pub params: [f32; 4],
}

/// Group 2: per-material constants (textures/samplers are separate bindings).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub struct MaterialUniform {
    pub mat_color: [f32; 4],
    pub amb_mul: [f32; 4],
    pub k0: [f32; 4],
    pub flags: [u32; 4],
    pub layer_kinds: [u32; 4],
    /// x: flip the V texture coordinate (NaomiLib meshes).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::size_of;

    #[test]
    fn layouts_match_wgsl() {
        // WGSL uniform structs: mat4 = 64, vec4 = 16.
        assert_eq!(size_of::<SceneUniform>(), 64 * 2 + 16 * 6);
        assert_eq!(size_of::<ObjectUniform>(), 64 * 2 + 32);
        assert_eq!(size_of::<MaterialUniform>(), 16 * 6);
        assert_eq!(size_of::<Vertex>(), 12 + 12 + 4 + 8 * 3);
        assert_eq!(size_of::<SpriteVertex>(), 32);
        // Uniform sizes must be multiples of 16.
        assert_eq!(size_of::<SceneUniform>() % 16, 0);
        assert_eq!(size_of::<ObjectUniform>() % 16, 0);
        assert_eq!(size_of::<MaterialUniform>() % 16, 0);
    }

    #[test]
    fn vertex_attributes_cover_the_struct() {
        let l = Vertex::layout();
        assert_eq!(l.array_stride, size_of::<Vertex>() as u64);
        let last = Vertex::ATTRIBUTES.last().unwrap();
        assert_eq!(last.offset + 8, l.array_stride);
    }
}
