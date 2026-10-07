//! The wgpu renderer: resource upload, scene objects and the frame loop.
//!
//! The caller owns the window/surface; it hands in a `Device`, `Queue` and the colour format of whatever it
//! renders to, and later a `TextureView` to draw into. Drawing order follows the reference: background
//! layer first (own depth range, depth cleared afterwards), then the world layer, opaque shapes first and
//! translucent shapes sorted back to front, then screen-space sprites.

use std::collections::HashMap;

use glam::{Mat3, Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::camera::Camera;
use crate::gpu_types::{MaterialUniform, ObjectUniform, SceneUniform, Vertex};
use crate::lighting::Lighting;
use crate::material::{
    material_uniform, naomi_material_uniform, naomi_pipeline_key, naomi_sampler_key, pipeline_key,
    sampler_key, LayerKind, PipelineKey, SamplerKey, WrapKey,
};
use crate::offscreen::OffscreenTarget;
use crate::sprite::{Sprite, SpriteRenderer};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Handle to a texture owned by the renderer (also usable for sprites).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub(crate) usize);

/// A TPL uploaded to the GPU: `textures[tpl_index]`.
pub type TextureSet = Vec<Option<TextureId>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModelId(usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ObjectId(usize);

/// Which pass an object is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// Backdrop: drawn first with a very deep depth range; the depth buffer is cleared afterwards.
    Background,
    /// Stage geometry and everything dynamic.
    World,
}

#[derive(Clone, Copy, Debug)]
pub struct Fog {
    pub color: [f32; 3],
    pub start: f32,
    pub end: f32,
}

pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
}

struct GpuMesh {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
    key: PipelineKey,
    material: wgpu::BindGroup,
    translucent: bool,
    origin: Vec3,
}

struct GpuModel {
    meshes: Vec<GpuMesh>,
}

struct Object {
    model: ModelId,
    layer: Layer,
    transform: Mat4,
    alpha: f32,
    color_mul: [f32; 4],
    visible: bool,
    dirty: bool,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

struct DrawItem {
    object: usize,
    mesh: usize,
    dist: f32,
}

/// Statistics of the last frame (useful in tests and the debug HUD).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    pub draws: usize,
    pub translucent_draws: usize,
    pub objects: usize,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,
    shader: wgpu::ShaderModule,
    object_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    textures: Vec<GpuTexture>,
    white: TextureId,
    samplers: HashMap<SamplerKey, wgpu::Sampler>,
    models: Vec<GpuModel>,
    objects: Vec<Object>,
    scene_buffers: [wgpu::Buffer; 2],
    scene_groups: [wgpu::BindGroup; 2],
    depth: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    sprites: SpriteRenderer,
    sprite_queue: Vec<Sprite>,
    pub lighting: Lighting,
    pub clear_color: [f64; 4],
    pub fog: Option<Fog>,
    pub stats: FrameStats,
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// Normal matrix (inverse transpose of the upper 3x3), padded to a mat4 for the uniform.
pub fn normal_matrix(m: &Mat4) -> Mat4 {
    let m3 = Mat3::from_mat4(*m);
    let n = if m3.determinant().abs() > 1e-12 {
        m3.inverse().transpose()
    } else {
        Mat3::IDENTITY
    };
    Mat4::from_mat3(n)
}

impl Renderer {
    pub fn new(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mb-render shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &[uniform_entry(0, vf)],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[uniform_entry(0, vf)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                uniform_entry(0, vf),
                texture_entry(1),
                texture_entry(2),
                texture_entry(3),
                sampler_entry(4),
                sampler_entry(5),
                sampler_entry(6),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mb-render layout"),
            bind_group_layouts: &[
                Some(&scene_layout),
                Some(&object_layout),
                Some(&material_layout),
            ],
            immediate_size: 0,
        });
        let make_scene = |label| {
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<SceneUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &scene_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buf.as_entire_binding(),
                }],
            });
            (buf, group)
        };
        let (b0, g0) = make_scene("scene bg");
        let (b1, g1) = make_scene("scene world");
        let sprites = SpriteRenderer::new(&device, target_format, DEPTH_FORMAT);
        let mut r = Self {
            device,
            queue,
            target_format,
            shader,
            object_layout,
            material_layout,
            pipeline_layout,
            pipelines: HashMap::new(),
            textures: Vec::new(),
            white: TextureId(0),
            samplers: HashMap::new(),
            models: Vec::new(),
            objects: Vec::new(),
            scene_buffers: [b0, b1],
            scene_groups: [g0, g1],
            depth: None,
            sprites,
            sprite_queue: Vec::new(),
            lighting: Lighting::default(),
            clear_color: [0.0, 0.0, 0.0, 1.0],
            fog: None,
            stats: FrameStats::default(),
        };
        r.white = r.upload_rgba(1, 1, &[255, 255, 255, 255]);
        r
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    pub fn target_format(&self) -> wgpu::TextureFormat {
        self.target_format
    }

    // ---- textures ----------------------------------------------------------------------------------

    /// Upload one RGBA8 image (single mip level).
    pub fn upload_rgba(&mut self, width: u32, height: u32, rgba: &[u8]) -> TextureId {
        self.upload_rgba_mips(&[(width, height, rgba)])
    }

    /// Upload an RGBA8 mip chain: `(width, height, data)` per level, level 0 first.
    pub fn upload_rgba_mips(&mut self, levels: &[(u32, u32, &[u8])]) -> TextureId {
        let (w, h, _) = levels[0];
        let max_levels = 32 - w.max(h).max(1).leading_zeros();
        let count = (levels.len() as u32).min(max_levels).max(1);
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tpl texture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, (lw, lh, data)) in levels.iter().take(count as usize).enumerate() {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(lw * 4),
                    rows_per_image: Some(*lh),
                },
                wgpu::Extent3d {
                    width: *lw,
                    height: *lh,
                    depth_or_array_layers: 1,
                },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.textures.push(GpuTexture {
            texture,
            view,
            width: w,
            height: h,
        });
        TextureId(self.textures.len() - 1)
    }

    /// Decode and upload every texture of a TPL (with its mip chain). Undecodable entries become `None`
    /// (materials then fall back to a white texture).
    pub fn upload_tpl(&mut self, tpl: &mb_tpl::Tpl) -> TextureSet {
        let mut set: TextureSet = Vec::new();
        for tex in tpl.textures() {
            let Ok(levels) = tex.decode_all() else {
                continue;
            };
            let refs: Vec<(u32, u32, &[u8])> = levels
                .iter()
                .map(|l| (l.width as u32, l.height as u32, l.rgba.as_slice()))
                .collect();
            let id = self.upload_rgba_mips(&refs);
            if set.len() <= tex.index {
                set.resize(tex.index + 1, None);
            }
            set[tex.index] = Some(id);
        }
        set
    }

    pub fn texture(&self, id: TextureId) -> &GpuTexture {
        &self.textures[id.0]
    }

    fn sampler(&mut self, key: SamplerKey) -> wgpu::Sampler {
        let device = &self.device;
        self.samplers
            .entry(key)
            .or_insert_with(|| {
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("tev sampler"),
                    address_mode_u: key.wrap_s.to_wgpu(),
                    address_mode_v: key.wrap_t.to_wgpu(),
                    address_mode_w: wgpu::AddressMode::ClampToEdge,
                    mag_filter: if key.nearest {
                        wgpu::FilterMode::Nearest
                    } else {
                        wgpu::FilterMode::Linear
                    },
                    min_filter: if key.nearest {
                        wgpu::FilterMode::Nearest
                    } else {
                        wgpu::FilterMode::Linear
                    },
                    mipmap_filter: if key.max_lod == 0 {
                        wgpu::MipmapFilterMode::Nearest
                    } else {
                        wgpu::MipmapFilterMode::Linear
                    },
                    lod_min_clamp: 0.0,
                    lod_max_clamp: key.max_lod as f32,
                    ..Default::default()
                })
            })
            .clone()
    }

    // ---- models ------------------------------------------------------------------------------------

    fn ensure_pipeline(&mut self, key: PipelineKey) {
        if self.pipelines.contains_key(&key) {
            return;
        }
        let blend = key.blend.map(|(src, dst)| wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: src,
                dst_factor: dst,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("mb-render pipeline"),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(Vertex::layout())],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.target_format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    // GX treats clockwise (in screen space, after its Y-down viewport) triangles as front
                    // facing; the decoded index order keeps that winding.
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: key.cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(key.depth_write),
                    depth_compare: Some(key.depth_compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        self.pipelines.insert(key, pipeline);
    }

    fn texture_or_white(&self, textures: &[Option<TextureId>], idx: Option<usize>) -> TextureId {
        idx.and_then(|i| textures.get(i).copied().flatten())
            .unwrap_or(self.white)
    }

    fn make_mesh(
        &mut self,
        v: &mb_gma::Vertices,
        indices: &[u32],
        key: PipelineKey,
        material: wgpu::BindGroup,
        translucent: bool,
        origin: Vec3,
    ) -> GpuMesh {
        self.ensure_pipeline(key);
        let verts: Vec<Vertex> = (0..v.len())
            .map(|i| Vertex {
                position: v.positions[i],
                normal: v.normals[i],
                color: v.colors[i],
                uv0: v.uv0[i],
                uv1: v.uv1[i],
                uv2: v.uv2[i],
            })
            .collect();
        let vertex = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vertices"),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("indices"),
                contents: bytemuck::cast_slice(indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        GpuMesh {
            vertex,
            index,
            index_count: indices.len() as u32,
            key,
            material,
            translucent,
            origin,
        }
    }

    /// Upload a GMA model. `textures` is the set of the TPL that belongs to the model's GMA.
    pub fn upload_model(
        &mut self,
        model: &mb_gma::Model,
        textures: &[Option<TextureId>],
    ) -> Result<ModelId, mb_gma::Error> {
        let meshes = model.meshes()?;
        let mut material_groups: HashMap<usize, wgpu::BindGroup> = HashMap::new();
        let mut out = Vec::new();
        for mesh in &meshes {
            if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                continue;
            }
            let shape = &model.shapes[mesh.shape_index];
            let material = match material_groups.get(&mesh.shape_index) {
                Some(m) => m.clone(),
                None => {
                    let bg = self.material_bind_group(model, shape, textures);
                    material_groups.insert(mesh.shape_index, bg.clone());
                    bg
                }
            };
            let key = pipeline_key(&shape.material, mesh.cull);
            out.push(self.make_mesh(
                &mesh.vertices,
                &mesh.indices,
                key,
                material,
                mesh.translucent,
                Vec3::from(shape.origin),
            ));
        }
        self.models.push(GpuModel { meshes: out });
        Ok(ModelId(self.models.len() - 1))
    }

    /// Upload a NaomiLib model (stages such as 10, 19, 20, 30, ...). `textures` belongs to the stage
    /// NaomiLib TPL.
    pub fn upload_nl_model(
        &mut self,
        model: &mb_gma::NlModel,
        textures: &[Option<TextureId>],
    ) -> ModelId {
        let kind_a = model.kind == mb_gma::VertexKind::A;
        let mut out = Vec::new();
        for mesh in &model.meshes {
            if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                continue;
            }
            // Culling everything draws nothing, exactly like the reference.
            if mesh.cull == mb_gma::CullMode::All {
                continue;
            }
            let tex = mesh
                .tex_idx
                .map(|i| self.texture_or_white(textures, Some(i as usize)));
            let uniform = naomi_material_uniform(mesh, kind_a, tex.is_some());
            let layers = tex
                .map(|t| vec![(t, naomi_sampler_key(mesh))])
                .unwrap_or_default();
            let material = self.make_material_group(&uniform, &layers);
            out.push(self.make_mesh(
                &mesh.vertices,
                &mesh.indices,
                naomi_pipeline_key(mesh),
                material,
                false,
                Vec3::from(model.bound_sphere_center),
            ));
        }
        self.models.push(GpuModel { meshes: out });
        ModelId(self.models.len() - 1)
    }

    fn material_bind_group(
        &mut self,
        model: &mb_gma::Model,
        shape: &mb_gma::Shape,
        textures: &[Option<TextureId>],
    ) -> wgpu::BindGroup {
        let m = &shape.material;
        let mut kinds = Vec::new();
        let mut layers: Vec<(TextureId, SamplerKey)> = Vec::new();
        if m.flags & mb_gma::material_flags::SIMPLE_MATERIAL == 0 {
            for layer in model.shape_tev_layers(shape).take(3) {
                kinds.push(LayerKind::from_flags(layer.flags));
                let id = self.texture_or_white(textures, usize::try_from(layer.tex_idx).ok());
                let (w, h) = (self.textures[id.0].width, self.textures[id.0].height);
                layers.push((id, sampler_key(layer, w, h)));
            }
        }
        let uniform = material_uniform(m, &kinds);
        self.make_material_group(&uniform, &layers)
    }

    /// Bind group 2: material uniform plus up to three (texture, sampler) pairs; unused slots get white.
    fn make_material_group(
        &mut self,
        uniform: &MaterialUniform,
        layers: &[(TextureId, SamplerKey)],
    ) -> wgpu::BindGroup {
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("material"),
                contents: bytemuck::bytes_of(uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let fallback = SamplerKey {
            wrap_s: WrapKey::Clamp,
            wrap_t: WrapKey::Clamp,
            max_lod: 0,
            nearest: false,
        };
        let mut views = Vec::new();
        let mut samplers = Vec::new();
        for i in 0..3 {
            let (tex, key) = layers.get(i).copied().unwrap_or((self.white, fallback));
            views.push(tex);
            samplers.push(self.sampler(key));
        }
        let t = |i: usize| &self.textures[views[i].0].view;
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("material"),
            layout: &self.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(t(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(t(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(t(2)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&samplers[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&samplers[1]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::Sampler(&samplers[2]),
                },
            ],
        })
    }

    // ---- objects -----------------------------------------------------------------------------------

    pub fn add_object(&mut self, model: ModelId, layer: Layer, transform: Mat4) -> ObjectId {
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("object"),
            size: std::mem::size_of::<ObjectUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("object"),
            layout: &self.object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        self.objects.push(Object {
            model,
            layer,
            transform,
            alpha: 1.0,
            color_mul: [1.0; 4],
            visible: true,
            dirty: true,
            buffer,
            bind_group,
        });
        ObjectId(self.objects.len() - 1)
    }

    /// Replace the object's model-to-world transform (e.g. each frame for animated anim groups).
    pub fn set_transform(&mut self, object: ObjectId, transform: Mat4) {
        let o = &mut self.objects[object.0];
        o.transform = transform;
        o.dirty = true;
    }

    pub fn transform(&self, object: ObjectId) -> Mat4 {
        self.objects[object.0].transform
    }

    pub fn set_visible(&mut self, object: ObjectId, visible: bool) {
        self.objects[object.0].visible = visible;
    }

    /// Object-wide opacity (1 = opaque). Objects below 1 draw all their shapes in the sorted pass.
    pub fn set_alpha(&mut self, object: ObjectId, alpha: f32) {
        let o = &mut self.objects[object.0];
        o.alpha = alpha;
        o.dirty = true;
    }

    pub fn set_color_mul(&mut self, object: ObjectId, rgba: [f32; 4]) {
        let o = &mut self.objects[object.0];
        o.color_mul = rgba;
        o.dirty = true;
    }

    /// Drop all models and objects (switching stages). Keeps pipelines, samplers and textures; release
    /// stage textures with [`Renderer::truncate_textures`] using a count taken before loading the stage.
    pub fn clear_scene(&mut self) {
        self.objects.clear();
        self.models.clear();
        self.sprite_queue.clear();
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Free every texture created after the first `keep` (ids handed out later become invalid).
    pub fn truncate_textures(&mut self, keep: usize) {
        self.textures.truncate(keep.max(1));
        self.sprites.forget_textures();
    }

    // ---- sprites -----------------------------------------------------------------------------------

    /// Queue a screen-space quad for the next `render` call (cleared afterwards).
    pub fn draw_sprite(&mut self, sprite: Sprite) {
        self.sprite_queue.push(sprite);
    }

    // ---- frame -------------------------------------------------------------------------------------

    fn ensure_depth(&mut self, size: (u32, u32)) {
        if matches!(&self.depth, Some((_, _, s)) if *s == size) {
            return;
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth = Some((tex, view, size));
    }

    fn scene_uniform(&self, camera: &Camera, aspect: f32, bg: bool) -> SceneUniform {
        let (near, far) = if bg {
            (camera.bg_near, camera.bg_far)
        } else {
            (camera.near, camera.far)
        };
        let dir = self.lighting.direction_to_light();
        let l = &self.lighting;
        let (fog_color, fog_params) = match self.fog {
            Some(f) if !bg => (
                [f.color[0], f.color[1], f.color[2], 1.0],
                [f.start, f.end, 1.0],
            ),
            _ => ([0.0; 4], [0.0, 1.0, 0.0]),
        };
        SceneUniform {
            view: camera.view.to_cols_array_2d(),
            proj: camera.projection(aspect, near, far).to_cols_array_2d(),
            light_dir: [dir.x, dir.y, dir.z, 0.0],
            light_color: [l.light_color[0], l.light_color[1], l.light_color[2], 1.0],
            ambient: [l.ambient[0], l.ambient[1], l.ambient[2], 1.0],
            spec_scale: [
                l.specular_scale[0],
                l.specular_scale[1],
                l.specular_scale[2],
                1.0,
            ],
            fog_color,
            fog_params: [
                fog_params[0],
                fog_params[1],
                fog_params[2],
                self.target_format.is_srgb() as u32 as f32,
            ],
        }
    }

    fn collect(&self, layer: Layer, camera: &Camera) -> (Vec<DrawItem>, Vec<DrawItem>) {
        let mut opaque = Vec::new();
        let mut translucent = Vec::new();
        for (oi, o) in self.objects.iter().enumerate() {
            if !o.visible || o.layer != layer {
                continue;
            }
            let model = &self.models[o.model.0];
            let sort_all = o.alpha < 1.0 || o.color_mul[3] < 1.0;
            for (mi, m) in model.meshes.iter().enumerate() {
                let world = o.transform.transform_point3(m.origin);
                let dist = camera.view.transform_point3(world).length();
                let item = DrawItem {
                    object: oi,
                    mesh: mi,
                    dist,
                };
                if m.translucent || sort_all {
                    translucent.push(item);
                } else {
                    opaque.push(item);
                }
            }
        }
        translucent.sort_by(|a, b| b.dist.total_cmp(&a.dist));
        (opaque, translucent)
    }

    fn draw_items<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, items: &[DrawItem]) {
        let mut last_key: Option<PipelineKey> = None;
        for it in items {
            let obj = &self.objects[it.object];
            let mesh = &self.models[obj.model.0].meshes[it.mesh];
            if last_key != Some(mesh.key) {
                pass.set_pipeline(&self.pipelines[&mesh.key]);
                last_key = Some(mesh.key);
            }
            pass.set_bind_group(1, &obj.bind_group, &[]);
            pass.set_bind_group(2, &mesh.material, &[]);
            pass.set_vertex_buffer(0, mesh.vertex.slice(..));
            pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }

    /// Render the scene into `target` (a view of a texture with the format given at construction).
    /// Submits the work to the queue.
    pub fn render(&mut self, target: &wgpu::TextureView, size: (u32, u32), camera: &Camera) {
        let size = (size.0.max(1), size.1.max(1));
        self.ensure_depth(size);
        let aspect = size.0 as f32 / size.1 as f32;

        for (i, bg) in [true, false].into_iter().enumerate() {
            let u = self.scene_uniform(camera, aspect, bg);
            self.queue
                .write_buffer(&self.scene_buffers[i], 0, bytemuck::bytes_of(&u));
        }
        for o in &mut self.objects {
            if o.dirty {
                let u = ObjectUniform {
                    model: o.transform.to_cols_array_2d(),
                    normal_mat: normal_matrix(&o.transform).to_cols_array_2d(),
                    color_mul: o.color_mul,
                    params: [o.alpha, 0.0, 0.0, 0.0],
                };
                self.queue
                    .write_buffer(&o.buffer, 0, bytemuck::bytes_of(&u));
                o.dirty = false;
            }
        }
        let (bg_opaque, bg_translucent) = self.collect(Layer::Background, camera);
        let (w_opaque, w_translucent) = self.collect(Layer::World, camera);
        let sprites = std::mem::take(&mut self.sprite_queue);
        self.sprites.prepare(
            &self.device,
            &self.queue,
            &sprites,
            &self.textures,
            size,
            self.target_format.is_srgb(),
        );
        self.stats = FrameStats {
            draws: bg_opaque.len() + bg_translucent.len() + w_opaque.len() + w_translucent.len(),
            translucent_draws: bg_translucent.len() + w_translucent.len(),
            objects: self.objects.len(),
        };

        let (_, depth_view, _) = self.depth.as_ref().unwrap();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("mb-render frame"),
            });
        let clear = wgpu::Color {
            r: self.clear_color[0],
            g: self.clear_color[1],
            b: self.clear_color[2],
            a: self.clear_color[3],
        };
        for (pass_index, (opaque, translucent)) in
            [(&bg_opaque, &bg_translucent), (&w_opaque, &w_translucent)]
                .into_iter()
                .enumerate()
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(if pass_index == 0 {
                    "background"
                } else {
                    "world"
                }),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if pass_index == 0 {
                            wgpu::LoadOp::Clear(clear)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.scene_groups[pass_index], &[]);
            self.draw_items(&mut pass, opaque);
            self.draw_items(&mut pass, translucent);
            if pass_index == 1 {
                self.sprites.draw(&mut pass);
            }
        }
        self.queue.submit(Some(encoder.finish()));
    }

    /// Render into an offscreen target and (natively) read the pixels back as tightly packed RGBA8.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_to_rgba(&mut self, target: &OffscreenTarget, camera: &Camera) -> Vec<u8> {
        self.render(target.view(), target.size(), camera);
        target.read_rgba(&self.device, &self.queue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_matrix_of_uniform_scale_is_inverse_scale() {
        let m = Mat4::from_scale(Vec3::splat(2.0));
        let n = normal_matrix(&m);
        assert!((n.x_axis.x - 0.5).abs() < 1e-6);
    }

    #[test]
    fn normal_matrix_keeps_rotation() {
        let m = Mat4::from_rotation_y(0.5) * Mat4::from_translation(Vec3::new(5.0, 1.0, 2.0));
        let n = normal_matrix(&m);
        let v = n.transform_vector3(Vec3::X);
        let expect = Mat3::from_rotation_y(0.5) * Vec3::X;
        assert!((v - expect).length() < 1e-6);
    }

    #[test]
    fn singular_matrix_does_not_produce_nan() {
        let m = Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0));
        let n = normal_matrix(&m);
        assert!(n.to_cols_array().iter().all(|v| v.is_finite()));
    }
}
