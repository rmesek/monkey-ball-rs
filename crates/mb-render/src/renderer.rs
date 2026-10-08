//! The wgpu renderer: resource upload, scene objects and the frame loop.
//!
//! The caller owns the window/surface; it hands in a `Device`, `Queue` and the colour format of whatever it
//! renders to, and later a `TextureView` to draw into. Drawing order follows the reference: background
//! layer first (own depth range, depth cleared afterwards), then the world layer, opaque shapes first and
//! translucent shapes sorted back to front, then screen-space sprites. Instance batches and world-space
//! quads (particles) go into the world layer at the place their [`DrawOrder`] names.

use std::collections::HashMap;

use glam::{Mat3, Mat4, Vec3, Vec4};
use web_time::Instant;
use wgpu::util::DeviceExt;

use crate::camera::Camera;
use crate::effects::{
    BlendMode, DrawOrder, InstanceBatchId, InstanceStyle, Quad, Shadow, DRAW_ORDERS,
};
use crate::gpu_types::{InstanceRaw, MaterialUniform, ObjectUniform, SceneUniform, Vertex};
use crate::instances::InstanceStore;
use crate::lighting::Lighting;
use crate::material::{
    material_uniform, naomi_material_uniform, naomi_pipeline_key, naomi_sampler_key, pipeline_key,
    sampler_key, LayerKind, PipelineKey, SamplerKey, WrapKey,
};
// Only the readback path (`render_to_rgba`) names the type, and that path does not exist on wasm32.
#[cfg(not(target_arch = "wasm32"))]
use crate::offscreen::OffscreenTarget;
use crate::quads::QuadBatcher;
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

impl ObjectId {
    /// Position in creation order since the last [`Renderer::clear_scene`] (dense, for side tables).
    pub fn index(self) -> usize {
        self.0
    }
}

/// Where the time of uploads went since the caller last reset it (stage loading).
#[derive(Clone, Copy, Debug, Default)]
pub struct LoadStats {
    pub textures: usize,
    /// Decoded RGBA bytes handed to the GPU, all mip levels.
    pub texture_bytes: usize,
    pub texture_decode_ms: f64,
    pub texture_upload_ms: f64,
    /// Display-list decoding, vertex packing, buffer and pipeline creation.
    pub mesh_ms: f64,
}

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
    /// Some texel of the top level is not fully opaque.
    pub alpha: bool,
}

struct GpuMesh {
    vertex: wgpu::Buffer,
    vertex_count: u32,
    index: wgpu::Buffer,
    index_count: u32,
    /// Index into `Renderer::pipelines` for the opaque pass (no blending when `solid`).
    pipeline: usize,
    /// The material's own blending pipeline, for the sorted pass.
    pipeline_blend: usize,
    /// Alpha is one everywhere (material, vertex colours, textures): blending would be a no-op.
    solid: bool,
    /// Index into `Renderer::materials`.
    material: usize,
    translucent: bool,
    origin: Vec3,
    /// What `pipeline_blend` was made from, for the variants instance batches ask for.
    key: PipelineKey,
    samples: u32,
}

/// Everything that tells two pipelines apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipelineVariant {
    key: PipelineKey,
    /// Texture fetches of the fragment shader (`SAMPLE_MASK`).
    samples: u32,
    /// Model matrix and colour come from an instance buffer.
    instanced: bool,
    /// Leave the target's alpha alone (effect overrides, like the reference's RGB write mask).
    color_only: bool,
    depth_bias: bool,
}

/// One model drawn at many transforms in one call per mesh.
struct InstanceBatch {
    model: ModelId,
    style: InstanceStyle,
    /// Per mesh of the model: index into `Renderer::pipelines`.
    pipelines: Vec<usize>,
}

struct GpuModel {
    meshes: Vec<GpuMesh>,
    has_translucent: bool,
    /// Bounding sphere of all vertices in model space, for frustum culling.
    center: Vec3,
    radius: f32,
}

struct Object {
    model: ModelId,
    layer: Layer,
    transform: Mat4,
    alpha: f32,
    color_mul: [f32; 4],
    visible: bool,
    sort_bias: Option<f32>,
    /// The stage tilt of the scene uniform is applied on top of `transform`.
    tilted: bool,
    /// The projected shadow of the scene uniform falls on it.
    receiver: bool,
    dirty: bool,
    /// Bounding sphere in world space before the tilt (refreshed with the uniform).
    center: Vec3,
    radius: f32,
    /// Outside the view frustum this frame.
    culled: bool,
}

#[derive(Clone, Copy)]
struct DrawItem {
    object: u32,
    mesh: u32,
    /// Opaque: state sort key (pipeline, material). Translucent: unused.
    key: u64,
    dist: f32,
}

/// Statistics of the last frame (the `?fps=1` overlay, tests).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameStats {
    /// Model draw calls issued (after culling), instanced ones included.
    pub draws: usize,
    pub translucent_draws: usize,
    /// Of `draws`: instanced calls, one per mesh of each batch that has instances.
    pub instance_draws: usize,
    /// Instances those calls draw.
    pub instances: usize,
    /// World-space quad runs (one draw each).
    pub quad_draws: usize,
    pub quads: usize,
    pub objects: usize,
    /// Visible objects skipped because they are outside the view frustum.
    pub culled_objects: usize,
    /// Sprite batches (one draw each).
    pub sprite_draws: usize,
    /// Pipeline, bind-group and vertex-buffer switches issued.
    pub state_changes: usize,
    /// Bytes written to uniform and vertex buffers.
    pub upload_bytes: usize,
}

impl FrameStats {
    /// Every draw call of the frame: models, instance batches, quad runs and sprite batches.
    pub fn total_draws(&self) -> usize {
        self.draws + self.quad_draws + self.sprite_draws
    }
}

/// Key of a material bind group: uniform bytes plus the three (texture, sampler) pairs. Meshes with equal
/// materials share one bind group, so sorted draws can skip the rebind.
type MaterialKey = (
    [u8; std::mem::size_of::<MaterialUniform>()],
    [(usize, SamplerKey); 3],
);

/// All per-object uniforms live in one buffer, bound with a dynamic offset: a frame uploads the changed range
/// in a single `write_buffer` instead of one call per object (each of those is a JS call in the browser).
struct ObjectBuffer {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    capacity: usize,
}

/// What a render pass has bound, to skip redundant state calls.
struct Bound {
    pipeline: usize,
    material: usize,
    object: u32,
    mesh: (usize, u32),
}

/// `Bound::object` while the untilted slot of `Renderer::instance_objects` is bound (the tilted one is one
/// less). No scene has that many objects.
const INSTANCE_OBJECT: u32 = u32::MAX - 1;

impl Default for Bound {
    fn default() -> Self {
        Self {
            pipeline: usize::MAX,
            material: usize::MAX,
            object: u32::MAX,
            mesh: (usize::MAX, u32::MAX),
        }
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target_format: wgpu::TextureFormat,
    shader: wgpu::ShaderModule,
    scene_layout: wgpu::BindGroupLayout,
    object_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipelines: Vec<wgpu::RenderPipeline>,
    pipeline_index: HashMap<PipelineVariant, usize>,
    textures: Vec<GpuTexture>,
    white: TextureId,
    samplers: HashMap<SamplerKey, wgpu::Sampler>,
    materials: Vec<wgpu::BindGroup>,
    material_index: HashMap<MaterialKey, usize>,
    models: Vec<GpuModel>,
    objects: Vec<Object>,
    object_buffer: Option<ObjectBuffer>,
    /// CPU copy of the object uniform buffer (`object_stride` bytes per object).
    object_data: Vec<u8>,
    object_stride: usize,
    scene_buffers: [wgpu::Buffer; 2],
    /// One per layer: the scene uniform plus the shadow mask (rebuilt when the mask changes).
    scene_groups: Vec<wgpu::BindGroup>,
    /// Mask texture of the projected shadow and how it is sampled; `None` = no shadow is drawn.
    shadow_mask: Option<(TextureId, SamplerKey)>,
    shadow: Option<Shadow>,
    instances: InstanceStore,
    instance_batches: Vec<InstanceBatch>,
    /// Group 1 of instanced draws: two object slots that only say whether the stage tilt applies.
    instance_objects: wgpu::BindGroup,
    quads: QuadBatcher,
    /// What the scene buffers hold, to skip the upload while nothing moves.
    scene_data: [Option<SceneUniform>; 2],
    depth: Option<(wgpu::Texture, wgpu::TextureView, (u32, u32))>,
    sprites: SpriteRenderer,
    sprite_queue: Vec<Sprite>,
    /// Opaque draws per layer (see `rebuild_opaque`); rebuilt only when objects are added or change pass.
    solid: [Vec<DrawItem>; 2],
    ordered: [Vec<DrawItem>; 2],
    opaque_dirty: bool,
    /// Blended draws per layer, rebuilt and sorted back to front every frame (buffers are reused).
    translucent: [Vec<DrawItem>; 2],
    /// Rotation of the stage about the ball, applied to objects flagged with [`Renderer::set_tilted`].
    tilt: Mat4,
    /// Bytes written outside `render` since the last frame (vertex re-poses).
    pending_upload_bytes: usize,
    /// Something that shows changed without an upload (visibility, textures, the target itself).
    changed: bool,
    /// Clear colour and target size of the last drawn frame.
    drawn: Option<([f64; 4], (u32, u32))>,
    pub lighting: Lighting,
    pub clear_color: [f64; 4],
    pub fog: Option<Fog>,
    pub stats: FrameStats,
    pub load_stats: LoadStats,
}

fn ms_since(t: Instant) -> f64 {
    (Instant::now() - t).as_secs_f64() * 1000.0
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

const OBJECT_UNIFORM_SIZE: usize = std::mem::size_of::<ObjectUniform>();

fn layer_index(layer: Layer) -> usize {
    match layer {
        Layer::Background => 0,
        Layer::World => 1,
    }
}

/// The six planes of a clip matrix (depth 0..1), normalised, pointing inwards.
fn frustum_planes(clip: &Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (clip.row(0), clip.row(1), clip.row(2), clip.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| {
        let len = p.truncate().length();
        if len > 0.0 {
            p / len
        } else {
            p
        }
    })
}

/// True when the sphere lies completely outside one of the planes.
fn sphere_outside(planes: &[Vec4; 6], center: Vec3, radius: f32) -> bool {
    planes
        .iter()
        .any(|p| p.truncate().dot(center) + p.w < -radius)
}

/// Bounding sphere of a point set: centre of the bounding box, radius to the farthest point.
fn bounding_sphere<'a>(points: impl Iterator<Item = &'a [f32; 3]> + Clone) -> (Vec3, f32) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for p in points.clone() {
        let p = Vec3::from(*p);
        lo = lo.min(p);
        hi = hi.max(p);
    }
    if lo.x > hi.x {
        return (Vec3::ZERO, 0.0);
    }
    let center = (lo + hi) * 0.5;
    let radius = points
        .map(|p| (Vec3::from(*p) - center).length_squared())
        .fold(0.0f32, f32::max)
        .sqrt();
    (center, radius)
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

fn pack_vertices(v: &mb_gma::Vertices) -> Vec<Vertex> {
    (0..v.len())
        .map(|i| Vertex {
            position: v.positions[i],
            normal: v.normals[i],
            color: v.colors[i],
            uv0: v.uv0[i],
            uv1: v.uv1[i],
            uv2: v.uv2[i],
        })
        .collect()
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
            entries: &[uniform_entry(0, vf), texture_entry(1), sampler_entry(2)],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: vf,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(OBJECT_UNIFORM_SIZE as u64),
                },
                count: None,
            }],
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
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: std::mem::size_of::<SceneUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let (b0, b1) = (make_scene("scene bg"), make_scene("scene world"));
        let sprites = SpriteRenderer::new(&device, target_format, DEPTH_FORMAT);
        let align = (device.limits().min_uniform_buffer_offset_alignment as usize).max(16);
        let object_stride = OBJECT_UNIFORM_SIZE.div_ceil(align) * align;
        let mut slots = vec![0u8; object_stride * 2];
        for (i, tilted) in [0.0, 1.0].into_iter().enumerate() {
            let u = ObjectUniform {
                model: Mat4::IDENTITY.to_cols_array_2d(),
                normal_mat: Mat4::IDENTITY.to_cols_array_2d(),
                color_mul: [1.0; 4],
                params: [1.0, tilted, 0.0, 0.0],
            };
            slots[i * object_stride..i * object_stride + OBJECT_UNIFORM_SIZE]
                .copy_from_slice(bytemuck::bytes_of(&u));
        }
        let instance_object_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("instance objects"),
            contents: &slots,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let instance_objects = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("instance objects"),
            layout: &object_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &instance_object_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(OBJECT_UNIFORM_SIZE as u64),
                }),
            }],
        });
        let mut r = Self {
            device,
            queue,
            target_format,
            shader,
            scene_layout,
            object_layout,
            material_layout,
            pipeline_layout,
            pipelines: Vec::new(),
            pipeline_index: HashMap::new(),
            textures: Vec::new(),
            white: TextureId(0),
            samplers: HashMap::new(),
            materials: Vec::new(),
            material_index: HashMap::new(),
            models: Vec::new(),
            objects: Vec::new(),
            object_buffer: None,
            object_data: Vec::new(),
            object_stride,
            scene_buffers: [b0, b1],
            scene_groups: Vec::new(),
            shadow_mask: None,
            shadow: None,
            instances: InstanceStore::default(),
            instance_batches: Vec::new(),
            instance_objects,
            quads: QuadBatcher::default(),
            scene_data: [None, None],
            depth: None,
            sprites,
            sprite_queue: Vec::new(),
            solid: [Vec::new(), Vec::new()],
            ordered: [Vec::new(), Vec::new()],
            opaque_dirty: true,
            translucent: [Vec::new(), Vec::new()],
            tilt: Mat4::IDENTITY,
            pending_upload_bytes: 0,
            changed: true,
            drawn: None,
            lighting: Lighting::default(),
            clear_color: [0.0, 0.0, 0.0, 1.0],
            fog: None,
            stats: FrameStats::default(),
            load_stats: LoadStats::default(),
        };
        r.white = r.upload_rgba(1, 1, &[255, 255, 255, 255]);
        r.rebuild_scene_groups();
        r
    }

    /// Bind group 0 of both layers: the scene uniform and the shadow mask (white while there is none).
    fn rebuild_scene_groups(&mut self) {
        let fallback = SamplerKey {
            wrap_s: WrapKey::Clamp,
            wrap_t: WrapKey::Clamp,
            max_lod: 0,
            nearest: false,
        };
        let (texture, key) = self.shadow_mask.unwrap_or((self.white, fallback));
        let sampler = self.sampler(key);
        let view = &self.textures[texture.0].view;
        self.scene_groups = (self.scene_buffers.iter())
            .map(|buffer| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("scene"),
                    layout: &self.scene_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&sampler),
                        },
                    ],
                })
            })
            .collect();
        self.changed = true;
    }

    /// The 1x1 white texture the renderer uses where a material has none (untextured quads).
    pub fn white_texture(&self) -> TextureId {
        self.white
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
        let started = Instant::now();
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
        self.load_stats.textures += 1;
        self.load_stats.texture_bytes += levels
            .iter()
            .take(count as usize)
            .map(|l| l.2.len())
            .sum::<usize>();
        self.load_stats.texture_upload_ms += ms_since(started);
        self.changed = true;
        self.textures.push(GpuTexture {
            texture,
            view,
            width: w,
            height: h,
            alpha: levels[0].2.as_chunks::<4>().0.iter().any(|p| p[3] != 255),
        });
        TextureId(self.textures.len() - 1)
    }

    /// Decode and upload one TPL texture with its mip chain; `None` when it cannot be decoded. Callers
    /// that know which textures their models use upload just those (a stage shows a fraction of the
    /// textures in the background and shared containers).
    pub fn upload_tpl_texture(&mut self, tex: &mb_tpl::Texture) -> Option<TextureId> {
        let started = Instant::now();
        let levels = tex.decode_all().ok()?;
        self.load_stats.texture_decode_ms += ms_since(started);
        let refs: Vec<(u32, u32, &[u8])> = levels
            .iter()
            .map(|l| (l.width as u32, l.height as u32, l.rgba.as_slice()))
            .collect();
        Some(self.upload_rgba_mips(&refs))
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

    /// Pipeline for `key` whose fragment shader samples only the textures in `samples` (bits 0..2: layers
    /// through their own UVs, bits 3..5: layers through the sphere map). Most materials use one texture; the
    /// shader would otherwise pay for six fetches per fragment.
    fn ensure_pipeline(&mut self, key: PipelineKey, samples: u32) -> usize {
        self.ensure_variant(PipelineVariant {
            key,
            samples,
            instanced: false,
            color_only: false,
            depth_bias: false,
        })
    }

    fn ensure_variant(&mut self, variant: PipelineVariant) -> usize {
        if let Some(i) = self.pipeline_index.get(&variant) {
            return *i;
        }
        let PipelineVariant { key, samples, .. } = variant;
        let layouts = [Some(Vertex::layout()), Some(InstanceRaw::layout())];
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
                    entry_point: Some(if variant.instanced {
                        "vs_instanced"
                    } else {
                        "vs_main"
                    }),
                    compilation_options: Default::default(),
                    buffers: &layouts[..1 + variant.instanced as usize],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants: &[("SAMPLE_MASK", samples as f64)],
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.target_format,
                        blend,
                        write_mask: if variant.color_only {
                            wgpu::ColorWrites::COLOR
                        } else {
                            wgpu::ColorWrites::ALL
                        },
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
                    // `polygonOffset(1, 1)` of the reference, whose depth runs the other way.
                    bias: if variant.depth_bias {
                        wgpu::DepthBiasState {
                            constant: -1,
                            slope_scale: -1.0,
                            clamp: 0.0,
                        }
                    } else {
                        Default::default()
                    },
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
        self.pipelines.push(pipeline);
        self.pipeline_index
            .insert(variant, self.pipelines.len() - 1);
        self.pipelines.len() - 1
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
        (material, samples, solid): (usize, u32, bool),
        translucent: bool,
        origin: Vec3,
    ) -> GpuMesh {
        let pipeline_blend = self.ensure_pipeline(key, samples);
        let pipeline = if solid {
            self.ensure_pipeline(PipelineKey { blend: None, ..key }, samples)
        } else {
            pipeline_blend
        };
        let verts = pack_vertices(v);
        let vertex = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vertices"),
                contents: bytemuck::cast_slice(&verts),
                // COPY_DST so `write_vertices` can re-pose dynamic meshes (goal tape).
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
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
            vertex_count: verts.len() as u32,
            index,
            index_count: indices.len() as u32,
            pipeline,
            pipeline_blend,
            solid: solid && !translucent,
            material,
            translucent,
            origin,
            key,
            samples,
        }
    }

    /// Upload a GMA model. `textures` is the set of the TPL that belongs to the model's GMA.
    pub fn upload_model(
        &mut self,
        model: &mb_gma::Model,
        textures: &[Option<TextureId>],
    ) -> Result<ModelId, mb_gma::Error> {
        let started = Instant::now();
        let meshes = model.meshes()?;
        let mut material_groups: HashMap<usize, (usize, u32, Option<bool>)> = HashMap::new();
        let mut out = Vec::new();
        let bounds = bounding_sphere(meshes.iter().flat_map(|m| m.vertices.positions.iter()));
        for mesh in &meshes {
            if mesh.indices.is_empty() || mesh.vertices.is_empty() {
                continue;
            }
            let shape = &model.shapes[mesh.shape_index];
            let material = match material_groups.get(&mesh.shape_index) {
                Some(m) => *m,
                None => {
                    let bg = self.material_bind_group(model, shape, textures);
                    material_groups.insert(mesh.shape_index, bg);
                    bg
                }
            };
            let key = pipeline_key(&shape.material, mesh.cull);
            // `Some(uses vertex colours)`: the material itself never produces alpha below one.
            let solid = match material.2 {
                Some(true) => mesh.vertices.colors.iter().all(|c| c[3] == 255),
                Some(false) => true,
                None => false,
            };
            out.push(self.make_mesh(
                &mesh.vertices,
                &mesh.indices,
                key,
                (material.0, material.1, solid),
                mesh.translucent,
                Vec3::from(shape.origin),
            ));
        }
        self.load_stats.mesh_ms += ms_since(started);
        Ok(self.push_model(out, bounds))
    }

    /// Upload a NaomiLib model (stages such as 10, 19, 20, 30, ...). `textures` belongs to the stage
    /// NaomiLib TPL.
    pub fn upload_nl_model(
        &mut self,
        model: &mb_gma::NlModel,
        textures: &[Option<TextureId>],
    ) -> ModelId {
        let started = Instant::now();
        let kind_a = model.kind == mb_gma::VertexKind::A;
        let mut out = Vec::new();
        let bounds = bounding_sphere(
            model
                .meshes
                .iter()
                .flat_map(|m| m.vertices.positions.iter()),
        );
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
                // NaomiLib meshes never blend.
                (material, tex.is_some() as u32, true),
                false,
                Vec3::from(model.bound_sphere_center),
            ));
        }
        self.load_stats.mesh_ms += ms_since(started);
        self.push_model(out, bounds)
    }

    fn push_model(&mut self, meshes: Vec<GpuMesh>, (center, radius): (Vec3, f32)) -> ModelId {
        self.models.push(GpuModel {
            has_translucent: meshes.iter().any(|m| m.translucent),
            meshes,
            center,
            radius,
        });
        ModelId(self.models.len() - 1)
    }

    fn material_bind_group(
        &mut self,
        model: &mb_gma::Model,
        shape: &mb_gma::Shape,
        textures: &[Option<TextureId>],
    ) -> (usize, u32, Option<bool>) {
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
        let mut samples = 0;
        for (i, kind) in kinds.iter().enumerate() {
            samples |= match kind {
                LayerKind::Diffuse | LayerKind::AlphaBlend => 1 << i,
                LayerKind::ViewSpecular => 8 << i,
                LayerKind::WorldSpecular => 0,
            };
        }
        // Alpha is one unless the material, an alpha layer or a texture with an alpha channel lowers it.
        let opaque = uniform.mat_color[3] >= 1.0
            && kinds
                .iter()
                .zip(&layers)
                .all(|(kind, (tex, _))| match kind {
                    LayerKind::Diffuse => !self.textures[tex.0].alpha,
                    LayerKind::AlphaBlend => false,
                    LayerKind::ViewSpecular | LayerKind::WorldSpecular => true,
                });
        let vertex_colors = uniform.flags[1] != 0;
        (
            self.make_material_group(&uniform, &layers),
            samples,
            opaque.then_some(vertex_colors),
        )
    }

    /// Bind group 2: material uniform plus up to three (texture, sampler) pairs; unused slots get white.
    fn make_material_group(
        &mut self,
        uniform: &MaterialUniform,
        layers: &[(TextureId, SamplerKey)],
    ) -> usize {
        let fallback = SamplerKey {
            wrap_s: WrapKey::Clamp,
            wrap_t: WrapKey::Clamp,
            max_lod: 0,
            nearest: false,
        };
        let slots: [(TextureId, SamplerKey); 3] =
            std::array::from_fn(|i| layers.get(i).copied().unwrap_or((self.white, fallback)));
        let mut bytes = [0u8; std::mem::size_of::<MaterialUniform>()];
        bytes.copy_from_slice(bytemuck::bytes_of(uniform));
        let key: MaterialKey = (bytes, slots.map(|(t, s)| (t.0, s)));
        if let Some(i) = self.material_index.get(&key) {
            return *i;
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("material"),
                contents: bytemuck::bytes_of(uniform),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let mut views = Vec::new();
        let mut samplers = Vec::new();
        for (tex, key) in slots {
            views.push(tex);
            samplers.push(self.sampler(key));
        }
        let t = |i: usize| &self.textures[views[i].0].view;
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
        });
        self.materials.push(group);
        self.material_index.insert(key, self.materials.len() - 1);
        self.materials.len() - 1
    }

    // ---- objects -----------------------------------------------------------------------------------

    pub fn add_object(&mut self, model: ModelId, layer: Layer, transform: Mat4) -> ObjectId {
        self.objects.push(Object {
            model,
            layer,
            transform,
            alpha: 1.0,
            color_mul: [1.0; 4],
            visible: true,
            sort_bias: None,
            tilted: false,
            receiver: false,
            dirty: true,
            center: Vec3::ZERO,
            radius: 0.0,
            culled: false,
        });
        self.opaque_dirty = true;
        ObjectId(self.objects.len() - 1)
    }

    /// Replace the object's model-to-world transform (e.g. each frame for animated anim groups).
    pub fn set_transform(&mut self, object: ObjectId, transform: Mat4) {
        let o = &mut self.objects[object.0];
        // Callers re-pose whole scenes every frame; only what moved is uploaded again.
        if o.transform != transform {
            o.transform = transform;
            o.dirty = true;
        }
    }

    /// Objects created since the last [`Renderer::clear_scene`].
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    pub fn transform(&self, object: ObjectId) -> Mat4 {
        self.objects[object.0].transform
    }

    pub fn set_visible(&mut self, object: ObjectId, visible: bool) {
        let o = &mut self.objects[object.0];
        self.changed |= o.visible != visible;
        o.visible = visible;
    }

    /// The target no longer shows the last frame (resized, reconfigured, uncovered): the next
    /// [`Renderer::prepare`] reports a change whatever the scene did.
    pub fn invalidate(&mut self) {
        self.changed = true;
    }

    /// Object-wide opacity (1 = opaque). Objects below 1 draw all their shapes in the sorted pass.
    pub fn set_alpha(&mut self, object: ObjectId, alpha: f32) {
        let o = &mut self.objects[object.0];
        if o.alpha != alpha {
            self.opaque_dirty |= (o.alpha < 1.0) != (alpha < 1.0);
            o.alpha = alpha;
            o.dirty = true;
        }
    }

    pub fn set_color_mul(&mut self, object: ObjectId, rgba: [f32; 4]) {
        let o = &mut self.objects[object.0];
        if o.color_mul != rgba {
            self.opaque_dirty |= (o.color_mul[3] < 1.0) != (rgba[3] < 1.0);
            o.color_mul = rgba;
            o.dirty = true;
        }
    }

    /// Whether the stage tilt ([`Renderer::set_tilt`]) applies to the object, on top of its transform.
    pub fn set_tilted(&mut self, object: ObjectId, tilted: bool) {
        let o = &mut self.objects[object.0];
        if o.tilted != tilted {
            o.tilted = tilted;
            o.dirty = true;
        }
    }

    /// Whether the projected shadow ([`Renderer::set_shadow`]) falls on the object. The reference casts it
    /// on the models of the anim groups only.
    pub fn set_shadow_receiver(&mut self, object: ObjectId, receiver: bool) {
        let o = &mut self.objects[object.0];
        if o.receiver != receiver {
            o.receiver = receiver;
            o.dirty = true;
        }
    }

    /// World-space rotation shown on every tilted object of the world layer. It lives in the scene uniform,
    /// so tilting the stage re-uploads one matrix and no object.
    pub fn set_tilt(&mut self, tilt: Mat4) {
        self.tilt = tilt;
    }

    /// Draw every mesh of the object in the back-to-front pass, ordered by the distance of the object's
    /// origin plus `bias` (larger = drawn earlier). Layered shells that share a centre (the ball) need
    /// this: their per-mesh distances tie or flip. `None` restores per-mesh sorting.
    pub fn set_sort_bias(&mut self, object: ObjectId, bias: Option<f32>) {
        let o = &mut self.objects[object.0];
        self.opaque_dirty |= o.sort_bias.is_some() != bias.is_some();
        self.changed |= o.sort_bias != bias;
        o.sort_bias = bias;
    }

    /// Replace the vertices of one mesh of an uploaded model (same vertex count; the index buffer is kept).
    /// For the few meshes that are re-posed on the CPU every frame. Returns false when the mesh does not
    /// exist or the count differs.
    pub fn write_vertices(&mut self, model: ModelId, mesh: usize, v: &mb_gma::Vertices) -> bool {
        let Some(gpu_model) = self.models.get_mut(model.0) else {
            return false;
        };
        let Some(m) = gpu_model.meshes.get(mesh) else {
            return false;
        };
        if m.vertex_count as usize != v.len() {
            return false;
        }
        let verts = pack_vertices(v);
        self.queue
            .write_buffer(&m.vertex, 0, bytemuck::cast_slice(&verts));
        self.pending_upload_bytes += std::mem::size_of_val(verts.as_slice());
        // Keep the culling sphere a superset of every pose (the old sphere stays inside the new one).
        let (center, radius) = bounding_sphere(v.positions.iter());
        let reach = (center - gpu_model.center).length() + radius;
        if reach > gpu_model.radius {
            gpu_model.radius = reach;
            for o in self.objects.iter_mut().filter(|o| o.model == model) {
                o.dirty = true;
            }
        }
        true
    }

    /// Drop all models and objects (switching stages). Keeps pipelines, samplers and textures; release
    /// stage textures with [`Renderer::truncate_textures`] using a count taken before loading the stage.
    pub fn clear_scene(&mut self) {
        self.objects.clear();
        self.models.clear();
        self.materials.clear();
        self.material_index.clear();
        self.sprite_queue.clear();
        self.instance_batches.clear();
        self.instances.clear();
        self.quads.discard();
        self.shadow = None;
        self.tilt = Mat4::IDENTITY;
        self.opaque_dirty = true;
        self.changed = true;
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Free every texture created after the first `keep` (ids handed out later become invalid).
    pub fn truncate_textures(&mut self, keep: usize) {
        self.textures.truncate(keep.max(1));
        // Texture ids are reused after this, so they no longer identify a material.
        self.material_index.clear();
        self.changed = true;
        self.sprites.forget_textures();
        self.quads.forget_textures();
        if matches!(self.shadow_mask, Some((t, _)) if t.0 >= self.textures.len()) {
            self.shadow_mask = None;
            self.rebuild_scene_groups();
        }
    }

    // ---- sprites -----------------------------------------------------------------------------------

    /// Queue a screen-space quad for the next `render` call (cleared afterwards).
    pub fn draw_sprite(&mut self, sprite: Sprite) {
        self.sprite_queue.push(sprite);
    }

    /// Drop queued sprites without drawing them (a frame that was updated but is not rendered). The
    /// frame's instances and quads go with them ([`Renderer::discard_effects`]).
    pub fn discard_sprites(&mut self) {
        self.sprite_queue.clear();
        self.discard_effects();
    }

    // ---- particles: instanced models and world-space quads -----------------------------------------

    /// Draw `model` at many transforms in one call per mesh. The batch is created once, at load (this
    /// compiles its pipelines and, the first time, creates the instance buffer); each frame the caller
    /// pushes that frame's instances with [`Renderer::push_instance`]. Batches of one [`DrawOrder`] draw
    /// in creation order. Instances are not culled.
    pub fn create_instance_batch(
        &mut self,
        model: ModelId,
        style: InstanceStyle,
    ) -> InstanceBatchId {
        let capacity = (self.instances.capacity()).max(crate::instances::DEFAULT_CAPACITY);
        self.instances.reserve(&self.device, capacity);
        let meshes: Vec<(PipelineKey, u32, bool)> = self.models[model.0]
            .meshes
            .iter()
            .map(|m| (m.key, m.samples, m.solid))
            .collect();
        let pipelines = meshes
            .into_iter()
            .map(|(mut key, samples, solid)| {
                // With the opaque geometry a mesh whose alpha is always one does not blend, as there.
                if style.order == DrawOrder::Opaque && solid {
                    key.blend = None;
                }
                if let Some(blend) = style.blend {
                    key.blend = Some(blend.factors());
                }
                if let Some(write) = style.depth_write {
                    key.depth_write = write;
                }
                if style.two_sided {
                    key.cull = None;
                }
                self.ensure_variant(PipelineVariant {
                    key,
                    samples,
                    instanced: true,
                    color_only: style.blend.is_some(),
                    depth_bias: style.depth_bias,
                })
            })
            .collect();
        self.instance_batches.push(InstanceBatch {
            model,
            style,
            pipelines,
        });
        InstanceBatchId(self.instances.add_batch())
    }

    /// Room for `capacity` instances per frame over all batches (512 unless reserved). Creates the buffer:
    /// call it while loading. The buffer never grows by itself; a frame that pushes more loses the rest.
    pub fn reserve_instances(&mut self, capacity: usize) {
        self.instances.reserve(&self.device, capacity.max(1));
    }

    /// Queue one instance of a batch for the next frame: its model-to-world transform (affine) and a
    /// colour that multiplies the material's, alpha included. The queue is per frame, like sprites:
    /// push again for every frame the instance should show. False when the batch is unknown or the
    /// instance buffer is full; the instance is then not drawn.
    pub fn push_instance(
        &mut self,
        batch: InstanceBatchId,
        transform: &Mat4,
        color: [f32; 4],
    ) -> bool {
        self.instances
            .push(batch.0, InstanceRaw::new(transform, color))
    }

    /// Room for `capacity` quads per frame (512 unless reserved, at most 16384). Creates the quad buffers
    /// and, the first time, the two quad pipelines: call it while loading.
    pub fn reserve_quads(&mut self, capacity: usize) {
        self.quads.reserve(
            &self.device,
            &self.scene_layout,
            self.target_format,
            DEPTH_FORMAT,
            capacity,
        );
    }

    /// Create what quads with `texture` need (a bind group) ahead of the first one, so that pushing them
    /// during play creates nothing. Also reserves the default quad capacity if none was reserved.
    pub fn warm_quad_texture(&mut self, texture: TextureId) {
        if self.quads.capacity() == 0 {
            self.reserve_quads(crate::quads::DEFAULT_CAPACITY);
        }
        self.quads
            .warm_texture(&self.device, &self.textures, texture.0);
    }

    /// Queue one world-space quad for the next frame: depth tested, not written to depth, both faces,
    /// vertex colour times texel, no lighting and no fog. Consecutive quads with the same texture, blend
    /// mode and order share one draw call, so push them grouped. The queue is per frame, like sprites.
    /// False when the quad buffer is full or the texture does not exist; the quad is then not drawn.
    pub fn push_quad(
        &mut self,
        texture: TextureId,
        blend: BlendMode,
        order: DrawOrder,
        quad: &Quad,
    ) -> bool {
        // A lookup once warm; see `warm_quad_texture`.
        self.warm_quad_texture(texture);
        self.quads.push(texture.0, blend, order, quad)
    }

    /// Drop the instances and quads queued for a frame that is not rendered.
    pub fn discard_effects(&mut self) {
        self.instances.discard();
        self.quads.discard();
    }

    // ---- projected shadow --------------------------------------------------------------------------

    /// The mask of the projected shadow (the reference's `circle_white` texture layer) and how it is
    /// sampled, or `None` for no shadow at all. Rebuilds two bind groups: call it while loading. The
    /// mask is dropped by [`Renderer::truncate_textures`] when its texture is.
    pub fn set_shadow_mask(&mut self, mask: Option<(TextureId, SamplerKey)>) {
        let mask = mask.filter(|(t, _)| t.0 < self.textures.len());
        if self.shadow_mask != mask {
            self.shadow_mask = mask;
            self.rebuild_scene_groups();
        }
    }

    /// This frame's projected shadow, or `None` to draw none (ball hidden). It darkens the objects
    /// flagged with [`Renderer::set_shadow_receiver`] inside their own draws: no draw call is added
    /// either way. Needs a mask ([`Renderer::set_shadow_mask`]). Stays until set again or
    /// [`Renderer::clear_scene`].
    pub fn set_shadow(&mut self, shadow: Option<Shadow>) {
        self.shadow = shadow;
    }

    // ---- frame -------------------------------------------------------------------------------------

    fn ensure_depth(&mut self, size: (u32, u32)) {
        if matches!(&self.depth, Some((_, _, s)) if *s == size) {
            return;
        }
        self.changed = true;
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
        // The backdrop never tilts.
        let tilt = if bg { Mat4::IDENTITY } else { self.tilt };
        // The shadow belongs to the world layer. The radius stays non-zero: the shader divides by it.
        let (shadow_from_world, shadow_color, shadow_info) = match self.shadow {
            Some(s) if !bg && self.shadow_mask.is_some() && s.radius > 0.0 => {
                (s.from_world, s.color, [s.radius, s.depth, s.fade, 1.0])
            }
            _ => (Mat4::IDENTITY, [0.0; 4], [1.0, 0.0, 0.0, 0.0]),
        };
        SceneUniform {
            view: camera.view.to_cols_array_2d(),
            proj: camera.projection(aspect, near, far).to_cols_array_2d(),
            tilt: tilt.to_cols_array_2d(),
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
            shadow_from_world: shadow_from_world.to_cols_array_2d(),
            shadow_color,
            shadow_info,
        }
    }

    /// Bring the object uniform buffer up to date: changed objects are rewritten in the CPU copy and each
    /// run of changed slots goes to the GPU in one write. Returns the bytes uploaded.
    fn upload_objects(&mut self) -> usize {
        let stride = self.object_stride;
        let count = self.objects.len();
        if count == 0 {
            return 0;
        }
        let mut grown = false;
        if self
            .object_buffer
            .as_ref()
            .is_none_or(|b| b.capacity < count)
        {
            let capacity = count.next_power_of_two().max(64);
            let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("objects"),
                size: (capacity * stride) as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("objects"),
                layout: &self.object_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(OBJECT_UNIFORM_SIZE as u64),
                    }),
                }],
            });
            self.object_buffer = Some(ObjectBuffer {
                buffer,
                group,
                capacity,
            });
            grown = true;
        }
        self.object_data.resize(count * stride, 0);
        let buffer = &self.object_buffer.as_ref().unwrap().buffer;
        let models = &self.models;
        let mut uploaded = 0;
        // Current run of slots to upload, as a slot range.
        let mut run: Option<(usize, usize)> = None;
        // Slots between two changed ones that are cheaper to resend than to start another write.
        const MERGE_GAP: usize = 8;
        for (i, o) in self.objects.iter_mut().enumerate() {
            // Hidden objects keep their dirty flag and are written when they appear.
            let refresh = o.dirty && (o.visible || grown);
            if refresh {
                let u = ObjectUniform {
                    model: o.transform.to_cols_array_2d(),
                    normal_mat: normal_matrix(&o.transform).to_cols_array_2d(),
                    color_mul: o.color_mul,
                    params: [
                        o.alpha,
                        o.tilted as u32 as f32,
                        o.receiver as u32 as f32,
                        0.0,
                    ],
                };
                self.object_data[i * stride..i * stride + OBJECT_UNIFORM_SIZE]
                    .copy_from_slice(bytemuck::bytes_of(&u));
                let model = &models[o.model.0];
                o.center = o.transform.transform_point3(model.center);
                let scale = o
                    .transform
                    .x_axis
                    .truncate()
                    .length_squared()
                    .max(o.transform.y_axis.truncate().length_squared())
                    .max(o.transform.z_axis.truncate().length_squared())
                    .sqrt();
                o.radius = model.radius * scale;
                o.dirty = false;
            }
            if !(refresh || grown) {
                continue;
            }
            run = match run {
                Some((start, end)) if i <= end + MERGE_GAP => Some((start, i + 1)),
                Some((start, end)) => {
                    let bytes = &self.object_data[start * stride..end * stride];
                    self.queue
                        .write_buffer(buffer, (start * stride) as u64, bytes);
                    uploaded += bytes.len();
                    Some((i, i + 1))
                }
                None => Some((i, i + 1)),
            };
        }
        if let Some((start, end)) = run {
            let bytes = &self.object_data[start * stride..end * stride];
            self.queue
                .write_buffer(buffer, (start * stride) as u64, bytes);
            uploaded += bytes.len();
        }
        uploaded
    }

    /// Flag visible objects whose bounding sphere is outside the frustum of their pass. Returns how many.
    fn cull(&mut self, clip: &[Mat4; 2]) -> usize {
        let bg = frustum_planes(&clip[0]);
        let world = frustum_planes(&clip[1]);
        // Tilted objects are tested in the space before the tilt.
        let tilted = frustum_planes(&(clip[1] * self.tilt));
        let mut culled = 0;
        for o in &mut self.objects {
            if !o.visible {
                continue;
            }
            let planes = match (o.layer, o.tilted) {
                (Layer::Background, _) => &bg,
                (Layer::World, false) => &world,
                (Layer::World, true) => &tilted,
            };
            // A NaN or infinite sphere never compares as outside.
            o.culled = sphere_outside(planes, o.center, o.radius);
            culled += o.culled as usize;
        }
        culled
    }

    /// Does this object draw all its meshes in the sorted, blended pass?
    fn sorts_all(o: &Object) -> bool {
        o.alpha < 1.0 || o.color_mul[3] < 1.0 || o.sort_bias.is_some()
    }

    /// Rebuild the cached opaque lists. `solid` holds meshes whose alpha is always one: they are drawn without
    /// blending, so their order is free and they are sorted by pipeline and material. `ordered` holds the
    /// other meshes of the opaque pass; they blend at texture edges and keep the order of the stage data.
    fn rebuild_opaque(&mut self) {
        for list in self.solid.iter_mut().chain(self.ordered.iter_mut()) {
            list.clear();
        }
        for (oi, o) in self.objects.iter().enumerate() {
            if Self::sorts_all(o) {
                continue;
            }
            let layer = layer_index(o.layer);
            for (mi, m) in self.models[o.model.0].meshes.iter().enumerate() {
                if m.translucent {
                    continue;
                }
                let item = DrawItem {
                    object: oi as u32,
                    mesh: mi as u32,
                    key: ((m.pipeline as u64) << 32) | m.material as u64,
                    dist: 0.0,
                };
                if m.solid {
                    self.solid[layer].push(item);
                } else {
                    self.ordered[layer].push(item);
                }
            }
        }
        for list in &mut self.solid {
            // Stable: equal state keeps object order, so instances of one model stay together.
            list.sort_by_key(|d| d.key);
        }
        self.opaque_dirty = false;
    }

    /// Collect this frame's blended draws, farthest first.
    fn collect_translucent(&mut self, view: &Mat4) {
        let view_tilted = *view * self.tilt;
        for list in &mut self.translucent {
            list.clear();
        }
        for (oi, o) in self.objects.iter().enumerate() {
            if !o.visible || o.culled {
                continue;
            }
            let sort_all = Self::sorts_all(o);
            let model = &self.models[o.model.0];
            if !sort_all && !model.has_translucent {
                continue;
            }
            let layer = layer_index(o.layer);
            let view = if o.tilted && layer == 1 {
                &view_tilted
            } else {
                view
            };
            for (mi, m) in model.meshes.iter().enumerate() {
                if !(m.translucent || sort_all) {
                    continue;
                }
                let dist = match o.sort_bias {
                    Some(bias) => {
                        view.transform_point3(o.transform.w_axis.truncate())
                            .length()
                            + bias
                    }
                    None => view
                        .transform_point3(o.transform.transform_point3(m.origin))
                        .length(),
                };
                self.translucent[layer].push(DrawItem {
                    object: oi as u32,
                    mesh: mi as u32,
                    key: 0,
                    dist,
                });
            }
        }
        for list in &mut self.translucent {
            // Stable, like the order the reference gets for equal distances.
            list.sort_by(|a, b| b.dist.total_cmp(&a.dist));
        }
    }

    /// Issue the draws of `items`, skipping every pipeline, bind group and buffer that is already bound.
    /// `blend` picks each mesh's blending pipeline (solid meshes faded by their object need it).
    fn draw_items(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        items: &[DrawItem],
        blend: bool,
        bound: &mut Bound,
        stats: &mut FrameStats,
    ) {
        let Some(objects) = &self.object_buffer else {
            return;
        };
        for it in items {
            let obj = &self.objects[it.object as usize];
            if !obj.visible || obj.culled {
                continue;
            }
            let mesh = &self.models[obj.model.0].meshes[it.mesh as usize];
            let pipeline = if blend {
                mesh.pipeline_blend
            } else {
                mesh.pipeline
            };
            if bound.pipeline != pipeline {
                pass.set_pipeline(&self.pipelines[pipeline]);
                bound.pipeline = pipeline;
                stats.state_changes += 1;
            }
            if bound.object != it.object {
                let offset = it.object as usize * self.object_stride;
                pass.set_bind_group(1, &objects.group, &[offset as u32]);
                bound.object = it.object;
                stats.state_changes += 1;
            }
            if bound.material != mesh.material {
                pass.set_bind_group(2, &self.materials[mesh.material], &[]);
                bound.material = mesh.material;
                stats.state_changes += 1;
            }
            if bound.mesh != (obj.model.0, it.mesh) {
                pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                bound.mesh = (obj.model.0, it.mesh);
                stats.state_changes += 1;
            }
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            stats.draws += 1;
        }
    }

    /// Issue the instance batches of `order`: one draw per mesh of each batch that has instances.
    fn draw_instances(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        order: DrawOrder,
        bound: &mut Bound,
        stats: &mut FrameStats,
    ) {
        let Some(buffer) = self.instances.buffer() else {
            return;
        };
        for (bi, batch) in self.instance_batches.iter().enumerate() {
            let (bytes, count) = self.instances.range(bi);
            if batch.style.order != order || count == 0 {
                continue;
            }
            // The batch is a slice of the buffer rather than a first instance: WebGL2 has no base
            // instance.
            pass.set_vertex_buffer(1, buffer.slice(bytes));
            stats.state_changes += 1;
            let object = INSTANCE_OBJECT - batch.style.tilted as u32;
            if bound.object != object {
                let offset = batch.style.tilted as usize * self.object_stride;
                pass.set_bind_group(1, &self.instance_objects, &[offset as u32]);
                bound.object = object;
                stats.state_changes += 1;
            }
            let meshes = &self.models[batch.model.0].meshes;
            for (mi, (mesh, pipeline)) in meshes.iter().zip(&batch.pipelines).enumerate() {
                if bound.pipeline != *pipeline {
                    pass.set_pipeline(&self.pipelines[*pipeline]);
                    bound.pipeline = *pipeline;
                    stats.state_changes += 1;
                }
                if bound.material != mesh.material {
                    pass.set_bind_group(2, &self.materials[mesh.material], &[]);
                    bound.material = mesh.material;
                    stats.state_changes += 1;
                }
                if bound.mesh != (batch.model.0, mi as u32) {
                    pass.set_vertex_buffer(0, mesh.vertex.slice(..));
                    pass.set_index_buffer(mesh.index.slice(..), wgpu::IndexFormat::Uint32);
                    bound.mesh = (batch.model.0, mi as u32);
                    stats.state_changes += 1;
                }
                pass.draw_indexed(0..mesh.index_count, 0, 0..count);
                stats.draws += 1;
                stats.instance_draws += 1;
            }
        }
    }

    /// The particles of one [`DrawOrder`]: instance batches, then quad runs.
    fn draw_effects(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        order: DrawOrder,
        bound: &mut Bound,
        stats: &mut FrameStats,
    ) {
        self.draw_instances(pass, order, bound, stats);
        let quad_draws = self.quads.draw(pass, order);
        if quad_draws > 0 {
            // The quads replaced the pipeline, its layout from group 1 on and the buffers.
            *bound = Bound::default();
            stats.quad_draws += quad_draws;
        }
    }

    /// Render the scene into `target` (a view of a texture with the format given at construction).
    /// Submits the work to the queue.
    pub fn render(&mut self, target: &wgpu::TextureView, size: (u32, u32), camera: &Camera) {
        self.prepare(size, camera);
        self.draw(target);
    }

    /// First half of a frame: upload what changed since the last one and work out what to draw. Returns
    /// false when the frame would be identical to the last one drawn, in which case the caller may skip
    /// [`Renderer::draw`] and presenting altogether.
    pub fn prepare(&mut self, size: (u32, u32), camera: &Camera) -> bool {
        let size = (size.0.max(1), size.1.max(1));
        self.ensure_depth(size);
        let aspect = size.0 as f32 / size.1 as f32;
        let mut stats = FrameStats {
            objects: self.objects.len(),
            upload_bytes: std::mem::take(&mut self.pending_upload_bytes),
            ..FrameStats::default()
        };

        let mut clip = [Mat4::IDENTITY; 2];
        for (i, bg) in [true, false].into_iter().enumerate() {
            let u = self.scene_uniform(camera, aspect, bg);
            clip[i] = Mat4::from_cols_array_2d(&u.proj) * camera.view;
            // A still camera (menus, pause) uploads nothing.
            if self.scene_data[i] != Some(u) {
                self.queue
                    .write_buffer(&self.scene_buffers[i], 0, bytemuck::bytes_of(&u));
                self.scene_data[i] = Some(u);
                stats.upload_bytes += std::mem::size_of::<SceneUniform>();
            }
        }
        stats.upload_bytes += self.upload_objects();
        stats.culled_objects = self.cull(&clip);
        if self.opaque_dirty {
            self.rebuild_opaque();
        }
        self.collect_translucent(&camera.view);
        let (instance_bytes, instances_changed) = self.instances.prepare(&self.queue);
        let (quad_bytes, quads_changed) = self.quads.prepare(&self.queue);
        stats.upload_bytes += instance_bytes + quad_bytes;
        stats.instances = self.instances.count();
        stats.quads = self.quads.counts().1;
        let sprites = std::mem::take(&mut self.sprite_queue);
        let (sprite_bytes, sprites_changed) = self.sprites.prepare(
            &self.device,
            &self.queue,
            &sprites,
            &self.textures,
            size,
            self.target_format.is_srgb(),
        );
        stats.upload_bytes += sprite_bytes;
        stats.sprite_draws = self.sprites.batch_count();
        // Hand the allocation back for the next frame.
        self.sprite_queue = sprites;
        self.sprite_queue.clear();
        let look = Some((self.clear_color, size));
        let changed = std::mem::take(&mut self.changed)
            || sprites_changed
            || instances_changed
            || quads_changed
            || stats.upload_bytes > 0
            || self.drawn != look;
        self.drawn = look;
        if !changed {
            // Keep the counts of the frame that is still on screen.
            stats.draws = self.stats.draws;
            stats.translucent_draws = self.stats.translucent_draws;
            stats.instance_draws = self.stats.instance_draws;
            stats.quad_draws = self.stats.quad_draws;
            stats.state_changes = 0;
        }
        self.stats = stats;
        changed
    }

    /// Second half of a frame: encode and submit the passes for what [`Renderer::prepare`] set up.
    pub fn draw(&mut self, target: &wgpu::TextureView) {
        let mut stats = FrameStats {
            draws: 0,
            translucent_draws: 0,
            instance_draws: 0,
            quad_draws: 0,
            state_changes: 0,
            ..self.stats
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
        for layer in 0..2 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(if layer == 0 { "background" } else { "world" }),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if layer == 0 {
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
            pass.set_bind_group(0, &self.scene_groups[layer], &[]);
            let mut bound = Bound::default();
            self.draw_items(&mut pass, &self.solid[layer], false, &mut bound, &mut stats);
            self.draw_items(
                &mut pass,
                &self.ordered[layer],
                true,
                &mut bound,
                &mut stats,
            );
            // Particles live in the world layer, at the three places a `DrawOrder` names.
            let [with_opaque, before_sorted, after_sorted] = DRAW_ORDERS;
            if layer == 1 {
                self.draw_effects(&mut pass, with_opaque, &mut bound, &mut stats);
                self.draw_effects(&mut pass, before_sorted, &mut bound, &mut stats);
            }
            let opaque_draws = stats.draws;
            self.draw_items(
                &mut pass,
                &self.translucent[layer],
                true,
                &mut bound,
                &mut stats,
            );
            stats.translucent_draws += stats.draws - opaque_draws;
            if layer == 1 {
                self.draw_effects(&mut pass, after_sorted, &mut bound, &mut stats);
                self.sprites.draw(&mut pass);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        self.stats = stats;
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
    fn frustum_culls_only_what_is_outside() {
        let camera = Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y);
        let clip = camera.projection(1.0, 0.1, 100.0) * camera.view;
        let planes = frustum_planes(&clip);
        assert!(!sphere_outside(&planes, Vec3::ZERO, 1.0));
        // Behind the camera, beyond the far plane, far off to the side.
        assert!(sphere_outside(&planes, Vec3::new(0.0, 0.0, 20.0), 1.0));
        assert!(sphere_outside(&planes, Vec3::new(0.0, 0.0, -200.0), 1.0));
        assert!(sphere_outside(&planes, Vec3::new(50.0, 0.0, 0.0), 1.0));
        // Centre outside but the sphere reaches in.
        assert!(!sphere_outside(&planes, Vec3::new(8.0, 0.0, 0.0), 5.0));
        // A broken bound is never culled.
        assert!(!sphere_outside(&planes, Vec3::splat(f32::NAN), 1.0));
        assert!(!sphere_outside(
            &planes,
            Vec3::new(50.0, 0.0, 0.0),
            f32::INFINITY
        ));
    }

    #[test]
    fn bounding_sphere_contains_every_point() {
        let points = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 3.0, 0.0],
            [1.0, 0.0, -4.0],
        ];
        let (center, radius) = bounding_sphere(points.iter());
        for p in points {
            assert!((Vec3::from(p) - center).length() <= radius + 1e-6);
        }
        assert_eq!(center, Vec3::new(1.0, 1.5, -2.0));
        let none: [[f32; 3]; 0] = [];
        assert_eq!(bounding_sphere(none.iter()), (Vec3::ZERO, 0.0));
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
