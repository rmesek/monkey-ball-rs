//! Screen-space textured quads for the HUD. Coordinates are target pixels with the origin at the top-left.

use std::collections::HashMap;

use crate::gpu_types::SpriteVertex;
use crate::renderer::{GpuCounters, GpuTexture, TextureId};

#[derive(Clone, Copy, Debug)]
pub struct Sprite {
    pub texture: TextureId,
    /// `[x, y, width, height]` in target pixels.
    pub dst: [f32; 4],
    /// `[u0, v0, u1, v1]` texture coordinates.
    pub uv: [f32; 4],
    /// Multiplied with the texel (straight alpha, blended over the frame).
    pub color: [f32; 4],
}

impl Sprite {
    pub fn new(texture: TextureId, x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            texture,
            dst: [x, y, w, h],
            uv: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
        }
    }
}

/// How a sprite goes onto the frame ([`crate::Renderer::draw_sprite_blended`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SpriteBlend {
    /// Texel times colour, blended over the frame by its alpha: what [`crate::Renderer::draw_sprite`]
    /// does.
    #[default]
    Alpha,
    /// Texel times colour, scaled by its alpha and added to the frame (`SrcAlpha, One`): a flash that
    /// brightens what is under it.
    Additive,
    /// The sprite's colour as it is, in the shape of the texel's alpha, blended over the frame: the
    /// silhouette the reference's HUD lays over the `GOAL` banner (`drawSpriteSolidTint`).
    Solid,
}

impl SpriteBlend {
    /// Solid sprites use the alpha pipeline; the shader tells them by the sign of the vertex alpha.
    fn additive(self) -> bool {
        self == SpriteBlend::Additive
    }
}

/// Vertices of a sprite drawn with `blend` (see [`quad_vertices`]).
pub fn blended_vertices(s: &Sprite, blend: SpriteBlend) -> [SpriteVertex; 6] {
    let mut v = quad_vertices(s);
    if blend == SpriteBlend::Solid {
        // The shader reads a negative alpha as "silhouette". Zero stays zero: nothing to draw.
        let a = -s.color[3].max(0.0);
        for vertex in &mut v {
            vertex.color[3] = a;
        }
    }
    v
}

/// Expand sprites into two triangles each (6 vertices).
pub fn quad_vertices(s: &Sprite) -> [SpriteVertex; 6] {
    let [x, y, w, h] = s.dst;
    let [u0, v0, u1, v1] = s.uv;
    let v = |px: f32, py: f32, u: f32, vv: f32| SpriteVertex {
        pos: [px, py],
        uv: [u, vv],
        color: s.color,
    };
    let a = v(x, y, u0, v0);
    let b = v(x + w, y, u1, v0);
    let c = v(x + w, y + h, u1, v1);
    let d = v(x, y + h, u0, v1);
    [a, b, c, a, c, d]
}

#[derive(PartialEq)]
struct Batch {
    texture: usize,
    additive: bool,
    first: u32,
    count: u32,
}

pub(crate) struct SpriteRenderer {
    /// Alpha blending and additive.
    pipelines: [wgpu::RenderPipeline; 2],
    texture_layout: wgpu::BindGroupLayout,
    screen_buffer: wgpu::Buffer,
    screen_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    texture_groups: HashMap<usize, wgpu::BindGroup>,
    /// Vertex buffer, kept between frames and replaced only when it is too small.
    vertex: Option<(wgpu::Buffer, usize)>,
    /// What the vertex buffer holds; an unchanged HUD uploads nothing.
    verts: Vec<SpriteVertex>,
    scratch: Vec<SpriteVertex>,
    screen: [f32; 4],
    batches: Vec<Batch>,
    last_batches: Vec<Batch>,
}

impl SpriteRenderer {
    /// What [`Self::new`] creates.
    pub(crate) const CREATED: GpuCounters = GpuCounters {
        buffers: 1,
        textures: 0,
        pipelines: 2,
        bind_groups: 1,
        samplers: 1,
    };

    pub(crate) fn new(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sprite shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("sprite.wgsl").into()),
        });
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite screen"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sprite layout"),
            bind_group_layouts: &[Some(&screen_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let make = |blend: wgpu::BlendState, write_mask: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("sprite pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<SpriteVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &SpriteVertex::ATTRIBUTES,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(blend),
                        write_mask,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                // Drawn inside the world pass, so the pipeline must match its depth attachment; it never
                // tests.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let add = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        };
        let pipelines = [
            make(wgpu::BlendState::ALPHA_BLENDING, wgpu::ColorWrites::ALL),
            // Light is added to the colour; the frame's alpha stays what it was.
            make(
                wgpu::BlendState {
                    color: add,
                    alpha: add,
                },
                wgpu::ColorWrites::COLOR,
            ),
        ];
        let screen_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let screen_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite screen"),
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: screen_buffer.as_entire_binding(),
            }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sprite sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipelines,
            texture_layout,
            screen_buffer,
            screen_group,
            sampler,
            texture_groups: HashMap::new(),
            vertex: None,
            verts: Vec::new(),
            scratch: Vec::new(),
            screen: [0.0; 4],
            batches: Vec::new(),
            last_batches: Vec::new(),
        }
    }

    /// Create the bind group of a texture ahead of its first sprite (one serves every blend mode).
    /// False for a texture that does not exist.
    pub(crate) fn warm_texture(
        &mut self,
        device: &wgpu::Device,
        textures: &[GpuTexture],
        texture: usize,
        counters: &mut GpuCounters,
    ) -> bool {
        let Some(tex) = textures.get(texture) else {
            return false;
        };
        let (layout, sampler) = (&self.texture_layout, &self.sampler);
        self.texture_groups.entry(texture).or_insert_with(|| {
            counters.bind_groups += 1;
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("sprite texture"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&tex.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            })
        });
        true
    }

    /// Make the vertex buffer hold at least `sprites` sprites (never fewer than the 170 a first frame
    /// gets by itself). A smaller or equal request changes nothing.
    pub(crate) fn reserve(
        &mut self,
        device: &wgpu::Device,
        sprites: usize,
        counters: &mut GpuCounters,
    ) {
        let wanted = sprites.saturating_mul(6);
        if matches!(&self.vertex, Some((_, capacity)) if *capacity >= wanted) {
            return;
        }
        let capacity = wanted.next_power_of_two().max(1024);
        counters.buffers += 1;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite vertices"),
            size: (capacity * std::mem::size_of::<SpriteVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.vertex = Some((buffer, capacity));
        // The new buffer holds nothing yet.
        self.verts.clear();
    }

    pub(crate) fn forget_textures(&mut self) {
        self.texture_groups.clear();
    }

    pub(crate) fn batch_count(&self) -> usize {
        self.batches.len()
    }

    /// Build this frame's batches and bring the vertex buffer up to date. Returns the bytes uploaded and
    /// whether the sprites differ from the last frame's in any way.
    pub(crate) fn prepare(
        &mut self,
        (device, queue): (&wgpu::Device, &wgpu::Queue),
        sprites: &[(Sprite, SpriteBlend)],
        textures: &[GpuTexture],
        size: (u32, u32),
        srgb_target: bool,
        counters: &mut GpuCounters,
    ) -> (usize, bool) {
        std::mem::swap(&mut self.batches, &mut self.last_batches);
        self.batches.clear();
        if sprites.is_empty() {
            return (0, !self.last_batches.is_empty());
        }
        let mut uploaded = 0;
        let screen = [size.0 as f32, size.1 as f32, srgb_target as u32 as f32, 0.0];
        if screen != self.screen {
            queue.write_buffer(&self.screen_buffer, 0, bytemuck::cast_slice(&screen));
            self.screen = screen;
            uploaded += 16;
        }
        let mut verts = std::mem::take(&mut self.scratch);
        verts.clear();
        for (s, blend) in sprites {
            let Some(_) = textures.get(s.texture.0) else {
                continue;
            };
            let first = verts.len() as u32;
            verts.extend_from_slice(&blended_vertices(s, *blend));
            let additive = blend.additive();
            match self.batches.last_mut() {
                Some(b)
                    if b.texture == s.texture.0
                        && b.additive == additive
                        && b.first + b.count == first =>
                {
                    b.count += 6
                }
                _ => self.batches.push(Batch {
                    texture: s.texture.0,
                    additive,
                    first,
                    count: 6,
                }),
            }
            self.warm_texture(device, textures, s.texture.0, counters);
        }
        let fits = matches!(&self.vertex, Some((_, capacity)) if *capacity >= verts.len());
        if !fits {
            self.reserve(device, verts.len().div_ceil(6), counters);
        }
        if !fits || verts != self.verts {
            if let Some((buffer, _)) = &self.vertex {
                queue.write_buffer(buffer, 0, bytemuck::cast_slice(&verts));
                uploaded += std::mem::size_of_val(verts.as_slice());
            }
            std::mem::swap(&mut self.verts, &mut verts);
        }
        self.scratch = verts;
        (uploaded, uploaded > 0 || self.batches != self.last_batches)
    }

    pub(crate) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        let (Some((vb, _)), false) = (&self.vertex, self.batches.is_empty()) else {
            return;
        };
        pass.set_bind_group(0, &self.screen_group, &[]);
        pass.set_vertex_buffer(0, vb.slice(..));
        let mut bound = None;
        for b in &self.batches {
            if bound != Some(b.additive) {
                pass.set_pipeline(&self.pipelines[b.additive as usize]);
                bound = Some(b.additive);
            }
            pass.set_bind_group(1, &self.texture_groups[&b.texture], &[]);
            pass.draw(b.first..b.first + b.count, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_covers_destination_rect() {
        let s = Sprite::new(TextureId(0), 10.0, 20.0, 30.0, 40.0);
        let q = quad_vertices(&s);
        assert_eq!(q[0].pos, [10.0, 20.0]);
        assert_eq!(q[2].pos, [40.0, 60.0]);
        assert_eq!(q[0].uv, [0.0, 0.0]);
        assert_eq!(q[2].uv, [1.0, 1.0]);
        assert_eq!(q[3], q[0]);
        assert_eq!(q[4], q[2]);
    }

    #[test]
    fn only_solid_sprites_change_their_vertices() {
        let mut s = Sprite::new(TextureId(0), 0.0, 0.0, 4.0, 4.0);
        s.color = [1.0, 0.5, 0.25, 0.75];
        assert_eq!(blended_vertices(&s, SpriteBlend::Alpha), quad_vertices(&s));
        assert_eq!(
            blended_vertices(&s, SpriteBlend::Additive),
            quad_vertices(&s)
        );
        let solid = blended_vertices(&s, SpriteBlend::Solid);
        for (a, b) in solid.iter().zip(quad_vertices(&s)) {
            assert_eq!((a.pos, a.uv), (b.pos, b.uv));
            assert_eq!(a.color, [1.0, 0.5, 0.25, -0.75]);
        }
        assert_eq!(SpriteBlend::default(), SpriteBlend::Alpha);
        assert!(SpriteBlend::Additive.additive() && !SpriteBlend::Solid.additive());
    }
}
