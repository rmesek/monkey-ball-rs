//! World-space textured quads for streaks and sparkles: one vertex buffer of fixed capacity, written at
//! most once per frame, and one draw per run of quads that share texture, blend mode and draw order.

use std::collections::HashMap;

use crate::effects::{BlendMode, DrawOrder, Quad};
use crate::gpu_types::QuadVertex;
use crate::renderer::GpuTexture;

/// Quads the buffer holds unless the caller reserves another number.
pub(crate) const DEFAULT_CAPACITY: usize = 512;
/// 16-bit indices address this many quads.
const MAX_CAPACITY: usize = 65536 / 4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Run {
    texture: usize,
    blend: BlendMode,
    order: DrawOrder,
    /// In quads.
    first: u32,
    count: u32,
}

struct Gpu {
    /// Indexed by `BlendMode as usize`.
    pipelines: [wgpu::RenderPipeline; 2],
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    capacity: usize,
}

#[derive(Default)]
pub(crate) struct QuadBatcher {
    gpu: Option<Gpu>,
    texture_groups: HashMap<usize, wgpu::BindGroup>,
    /// This frame's vertices, four per quad.
    verts: Vec<QuadVertex>,
    /// What the vertex buffer holds; unchanged quads upload nothing.
    uploaded: Vec<QuadVertex>,
    runs: Vec<Run>,
    last_runs: Vec<Run>,
}

fn pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    blend: BlendMode,
) -> wgpu::RenderPipeline {
    let (src, dst) = blend.factors();
    let component = wgpu::BlendComponent {
        src_factor: src,
        dst_factor: dst,
        operation: wgpu::BlendOperation::Add,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("quad pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<QuadVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &QuadVertex::ATTRIBUTES,
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: target_format,
                blend: Some(wgpu::BlendState {
                    color: component,
                    alpha: component,
                }),
                // The frame stays opaque whatever is added to it.
                write_mask: wgpu::ColorWrites::COLOR,
            })],
        }),
        // `cullMode: None`: a streak faces wherever its motion takes it.
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl QuadBatcher {
    /// Make room for `capacity` quads per frame. Creates buffers (and, the first time, the two pipelines),
    /// so this belongs to loading; a smaller or equal request changes nothing.
    pub(crate) fn reserve(
        &mut self,
        device: &wgpu::Device,
        scene_layout: &wgpu::BindGroupLayout,
        target_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        capacity: usize,
    ) {
        let capacity = capacity.clamp(1, MAX_CAPACITY);
        if matches!(&self.gpu, Some(g) if capacity <= g.capacity) {
            return;
        }
        let vertex = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad vertices"),
            size: (capacity * 4 * std::mem::size_of::<QuadVertex>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let indices: Vec<u16> = (0..capacity as u32)
            .flat_map(|q| [0, 1, 2, 2, 1, 3].map(|i| (q * 4 + i) as u16))
            .collect();
        let index = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("quad indices"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            },
        );
        // The new buffer holds nothing yet.
        self.uploaded.clear();
        for list in [&mut self.verts, &mut self.uploaded] {
            list.reserve((capacity * 4).saturating_sub(list.len()));
        }
        if let Some(gpu) = &mut self.gpu {
            gpu.vertex = vertex;
            gpu.index = index;
            gpu.capacity = capacity;
            return;
        }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("quad texture"),
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
            label: Some("quad layout"),
            bind_group_layouts: &[Some(scene_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let make = |blend| pipeline(device, &layout, &shader, target_format, depth_format, blend);
        // `streakSamplerDesc`: clamped, bilinear, top level only.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("quad sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            lod_max_clamp: 0.0,
            ..Default::default()
        });
        self.gpu = Some(Gpu {
            pipelines: [make(BlendMode::Alpha), make(BlendMode::Additive)],
            texture_layout,
            sampler,
            vertex,
            index,
            capacity,
        });
    }

    pub(crate) fn capacity(&self) -> usize {
        self.gpu.as_ref().map_or(0, |g| g.capacity)
    }

    /// Create the bind group of a texture ahead of its first quad. False before [`Self::reserve`] or for
    /// a texture that does not exist.
    pub(crate) fn warm_texture(
        &mut self,
        device: &wgpu::Device,
        textures: &[GpuTexture],
        texture: usize,
    ) -> bool {
        let (Some(gpu), Some(tex)) = (&self.gpu, textures.get(texture)) else {
            return false;
        };
        self.texture_groups.entry(texture).or_insert_with(|| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("quad texture"),
                layout: &gpu.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&tex.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                    },
                ],
            })
        });
        true
    }

    /// Texture ids are about to be reused.
    pub(crate) fn forget_textures(&mut self) {
        self.texture_groups.clear();
        self.discard();
    }

    /// Drop what was pushed for a frame that is not drawn.
    pub(crate) fn discard(&mut self) {
        self.verts.clear();
        self.runs.clear();
    }

    /// Queue one quad for the next frame. False when the buffer is full or the texture has no bind group
    /// (see [`Self::warm_texture`]).
    pub(crate) fn push(
        &mut self,
        texture: usize,
        blend: BlendMode,
        order: DrawOrder,
        quad: &Quad,
    ) -> bool {
        let first = (self.verts.len() / 4) as u32;
        if first as usize >= self.capacity() || !self.texture_groups.contains_key(&texture) {
            return false;
        }
        for i in 0..4 {
            self.verts.push(QuadVertex {
                pos: quad.positions[i].to_array(),
                color: quad.colors[i],
                uv: quad.uvs[i],
            });
        }
        match self.runs.last_mut() {
            Some(r) if r.texture == texture && r.blend == blend && r.order == order => r.count += 1,
            _ => self.runs.push(Run {
                texture,
                blend,
                order,
                first,
                count: 1,
            }),
        }
        true
    }

    /// Bring the vertex buffer up to date with what was pushed and start collecting the next frame.
    /// Returns the bytes uploaded and whether anything differs from the last frame.
    pub(crate) fn prepare(&mut self, queue: &wgpu::Queue) -> (usize, bool) {
        let mut uploaded = 0;
        if let Some(gpu) = &self.gpu {
            // A shorter list that is a prefix of what the buffer holds needs no upload either.
            if !self.verts.is_empty() && !self.uploaded.starts_with(&self.verts) {
                queue.write_buffer(&gpu.vertex, 0, bytemuck::cast_slice(&self.verts));
                uploaded = std::mem::size_of_val(self.verts.as_slice());
                std::mem::swap(&mut self.verts, &mut self.uploaded);
            }
        }
        self.verts.clear();
        let changed = uploaded > 0 || self.runs != self.last_runs;
        // `last_runs` is what `draw` issues until the next `prepare`.
        std::mem::swap(&mut self.runs, &mut self.last_runs);
        self.runs.clear();
        (uploaded, changed)
    }

    /// Runs (draw calls) and quads prepared for this frame.
    pub(crate) fn counts(&self) -> (usize, usize) {
        (
            self.last_runs.len(),
            self.last_runs.iter().map(|r| r.count as usize).sum(),
        )
    }

    /// Issue the runs of `order`. Bind group 0 (the scene) is expected to be set; everything else is
    /// replaced. Returns the number of draws.
    pub(crate) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, order: DrawOrder) -> usize {
        let Some(gpu) = &self.gpu else {
            return 0;
        };
        let mut draws = 0;
        let mut bound: Option<BlendMode> = None;
        for run in self.last_runs.iter().filter(|r| r.order == order) {
            let Some(group) = self.texture_groups.get(&run.texture) else {
                continue;
            };
            if draws == 0 {
                pass.set_vertex_buffer(0, gpu.vertex.slice(..));
                pass.set_index_buffer(gpu.index.slice(..), wgpu::IndexFormat::Uint16);
            }
            if bound != Some(run.blend) {
                pass.set_pipeline(&gpu.pipelines[run.blend as usize]);
                bound = Some(run.blend);
            }
            pass.set_bind_group(1, group, &[]);
            pass.draw_indexed(run.first * 6..(run.first + run.count) * 6, 0, 0..1);
            draws += 1;
        }
        draws
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_queued_before_reserve() {
        let mut q = QuadBatcher::default();
        let quad = Quad::new([glam::Vec3::ZERO; 4], [255; 4]);
        assert!(!q.push(0, BlendMode::Additive, DrawOrder::AfterSorted, &quad));
        assert_eq!(q.counts(), (0, 0));
    }
}
