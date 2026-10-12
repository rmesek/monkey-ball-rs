//! Mirror floors (`noclip/Render.ts` "Mirror Capture" / "Mirror Overlay", `World.ts`
//! `prepareToRenderMirrors`): the scene is drawn once more from the camera reflected in the mirror's plane
//! into a reduced target, and the mirror models are drawn again over the opaque geometry with a program
//! that looks that picture up at each fragment's place on the screen.
//!
//! This module holds what the caller says ([`Mirror`], [`Distortion`]), the maths (reflection, the plane
//! as an oblique near plane) and the GPU objects; `renderer.rs` runs the passes.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};

use crate::gpu_types::Vertex;
use crate::renderer::TextureId;

/// The wavy mirror's distortion (`createMirrorWavyProgram`): the reflection is looked up beside each
/// fragment, by an amount read from a gradient texture addressed with the surface normal. The reference
/// draws the waved floor into a 256 x 256 target with that texture and reads the target back at the
/// fragment's place on the screen; here the mirror surface itself is drawn waved, so the texel under a
/// fragment is its own and one fetch in the overlay's shader replaces the target and its pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Distortion {
    /// `gb_grad` of `common.gma` (its first texture layer). The reference falls back to a white texel
    /// when the model is missing ([`crate::Renderer::white_texture`]).
    pub grad: TextureId,
    /// Rows of the GX indirect matrix, applied to `(alpha, blue, green) * 255`, masked to five bits and
    /// less 128, then divided by 256: `u` and `v` of the offset, `v` up the screen.
    pub ind0: [f32; 3],
    pub ind1: [f32; 3],
}

impl Distortion {
    /// `(0, 1.6, 0)` and `(1.2, 0, 0)` of `Render.ts`, halved until the larger is below one.
    pub const IND0: [f32; 3] = [0.0, 0.8, 0.0];
    pub const IND1: [f32; 3] = [0.6, 0.0, 0.0];

    /// The reference's distortion with `grad`.
    pub fn reference(grad: TextureId) -> Self {
        Self {
            grad,
            ind0: Self::IND0,
            ind1: Self::IND1,
        }
    }

    /// `IndMask(255 * v, 0xF8) - 128` of the reference's fragment shader.
    pub fn quantise(v: f32) -> f32 {
        ((v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u32 & 0xF8) as f32 - 128.0
    }

    /// What the overlay's shader adds to the mirror coordinates (`v` up the screen, in fractions of the
    /// picture) for a texel `rgb` of the gradient. The reference reads `.abg` of its distortion target,
    /// whose alpha is always one, so the first component is constant and red is not used.
    pub fn offset(&self, rgb: [f32; 3]) -> [f32; 2] {
        let c = Vec3::new(
            Self::quantise(1.0),
            Self::quantise(rgb[2]),
            Self::quantise(rgb[1]),
        );
        [
            Vec3::from(self.ind0).dot(c) / 256.0,
            Vec3::from(self.ind1).dot(c) / 256.0,
        ]
    }
}

/// A mirror floor: see [`crate::Renderer::set_mirror`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mirror {
    /// Places the mirror in stage space (before the stage tilt): its plane goes through the matrix's
    /// origin and faces along its +Y axis. What is on the +Y side is reflected.
    pub plane: Mat4,
    /// `mirrorAlpha`: the reflection's opacity over the mirror surface, times the surface's vertex alpha.
    pub alpha: f32,
    /// The wavy mirror of stage 92; `None` for a flat one.
    pub distortion: Option<Distortion>,
}

impl Mirror {
    /// `WAVY_MIRROR_ALPHA`.
    pub const WAVY_ALPHA: f32 = 96.0 / 255.0;
    /// `WAVY_MIRROR_PLANE_Y`.
    pub const WAVY_PLANE_Y: f32 = 0.02;

    /// A flat mirror at full opacity (stages 102 and 103).
    pub fn flat(plane: Mat4) -> Self {
        Self {
            plane,
            alpha: 1.0,
            distortion: None,
        }
    }

    /// The wavy mirror of stage 92 with the reference's opacity and distortion.
    pub fn wavy(plane: Mat4, grad: TextureId) -> Self {
        Self {
            plane,
            alpha: Self::WAVY_ALPHA,
            distortion: Some(Distortion::reference(grad)),
        }
    }
}

/// How far above the mirror's plane the capture is cut off. Geometry that lies in the plane itself (floor
/// pieces beside the mirror) would otherwise be clipped or not from one frame to the next.
pub(crate) const CLIP_LIFT: f32 = 0.005;

/// A plane as a unit normal and a point on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Plane {
    pub normal: Vec3,
    pub point: Vec3,
}

impl Plane {
    /// The plane of [`Mirror::plane`]; `None` when the matrix has no usable +Y axis.
    pub(crate) fn from_matrix(m: &Mat4) -> Option<Self> {
        let normal = m.transform_vector3(Vec3::Y).try_normalize()?;
        let point = m.transform_point3(Vec3::ZERO);
        (normal.is_finite() && point.is_finite()).then_some(Self { normal, point })
    }

    /// The plane after a rigid transform (the stage tilt).
    pub(crate) fn transformed(&self, m: &Mat4) -> Self {
        Self {
            normal: m.transform_vector3(self.normal).normalize_or_zero(),
            point: m.transform_point3(self.point),
        }
    }

    /// Signed distance of `p`, positive on the reflected side.
    pub(crate) fn distance(&self, p: Vec3) -> f32 {
        self.normal.dot(p - self.point)
    }

    /// `(n, d)` with `n . p + d = 0` on the plane lifted by `lift` along its normal.
    pub(crate) fn equation(&self, lift: f32) -> Vec4 {
        self.normal.extend(-self.normal.dot(self.point) - lift)
    }

    /// `computeReflectionMatrix`.
    pub(crate) fn reflection(&self) -> Mat4 {
        let n = self.normal;
        let d = -n.dot(self.point);
        Mat4::from_cols(
            Vec4::new(
                1.0 - 2.0 * n.x * n.x,
                -2.0 * n.x * n.y,
                -2.0 * n.x * n.z,
                0.0,
            ),
            Vec4::new(
                -2.0 * n.y * n.x,
                1.0 - 2.0 * n.y * n.y,
                -2.0 * n.y * n.z,
                0.0,
            ),
            Vec4::new(
                -2.0 * n.z * n.x,
                -2.0 * n.z * n.y,
                1.0 - 2.0 * n.z * n.z,
                0.0,
            ),
            Vec4::new(-2.0 * n.x * d, -2.0 * n.y * d, -2.0 * n.z * d, 1.0),
        )
    }
}

/// `mirrorFlipX`: the reflection turns every triangle over; flipping the picture turns them back, so the
/// capture is drawn with the pipelines (winding, culling) of the main picture.
pub(crate) fn flip_x() -> Mat4 {
    Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))
}

/// `proj` with its near plane replaced by `plane` (view space, `plane . v >= 0` is kept): Lengyel's
/// oblique frustum for a depth range of 0..1. WebGL2 has no clip distances, and this costs nothing per
/// fragment. Only the depth row changes, so x, y and w of every vertex stay what they were. `None` when
/// the eye is not on the cut-off side or the plane misses the frustum: the caller then draws unclipped,
/// as the reference always does.
pub(crate) fn oblique_projection(proj: &Mat4, plane: Vec4) -> Option<Mat4> {
    if plane.w.is_nan() || plane.w >= 0.0 {
        return None;
    }
    // The corner of the far plane that lies farthest along the plane's normal.
    let corner = Vec4::new(plane.x.signum(), plane.y.signum(), 1.0, 1.0);
    let q = proj.inverse() * corner;
    let reach = plane.dot(q);
    let w = proj.row(3).dot(q);
    let scale = w / reach;
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let row = plane * scale;
    let mut out = *proj;
    out.x_axis.z = row.x;
    out.y_axis.z = row.y;
    out.z_axis.z = row.z;
    out.w_axis.z = row.w;
    out.is_finite().then_some(out)
}

/// Size of the capture for a frame of `size` at `scale`, at least one pixel each way.
pub(crate) fn target_size(size: (u32, u32), scale: f32) -> (u32, u32) {
    let side = |v: u32| ((v as f32 * scale).round() as u32).clamp(1, v.max(1));
    (side(size.0), side(size.1))
}

/// Group 2 of the overlay: `MirrorParams` in mirror.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable, PartialEq)]
pub(crate) struct MirrorUniform {
    /// The displayed world (after the stage tilt) -> clip space of the capture.
    pub clip: [[f32; 4]; 4],
    /// x: mirror alpha, y: 1 = wavy.
    pub params: [f32; 4],
    pub ind0: [f32; 4],
    pub ind1: [f32; 4],
}

/// What every mirror shares, made the first time one is set.
pub(crate) struct MirrorGpu {
    pub layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
}

impl MirrorGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        scene_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
        target_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mirror shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("mirror.wgsl").into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mirror"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture(3),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mirror layout"),
            bind_group_layouts: &[Some(scene_layout), Some(object_layout), Some(&layout)],
            immediate_size: 0,
        });
        // `mirrorFlatMegaState`: blended by the source alpha, depth tested and not written, no culling.
        let blend = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mirror overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(Vertex::layout())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState {
                        color: blend,
                        alpha: blend,
                    }),
                    // The frame stays opaque, as under the other effects.
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                // The surface was just drawn at these depths by another program; one step towards the
                // camera keeps the two from fighting where a driver rounds them differently.
                bias: wgpu::DepthBiasState {
                    constant: -1,
                    slope_scale: -1.0,
                    clamp: 0.0,
                },
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { layout, pipeline }
    }
}

/// The capture: colour in the format of the frame (the scene's pipelines are compiled for it) and depth.
pub(crate) struct MirrorTarget {
    pub color: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    pub size: (u32, u32),
    // The views keep the textures alive; these are held so that dropping the target frees them at once.
    _textures: [wgpu::Texture; 2],
}

impl MirrorTarget {
    pub(crate) fn new(
        device: &wgpu::Device,
        size: (u32, u32),
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let make = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = make(
            "mirror colour",
            color_format,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = make(
            "mirror depth",
            depth_format,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        Self {
            color: color.create_view(&wgpu::TextureViewDescriptor::default()),
            depth: depth.create_view(&wgpu::TextureViewDescriptor::default()),
            size,
            _textures: [color, depth],
        }
    }

    /// Bytes of the two attachments (four per pixel each).
    pub(crate) fn bytes(&self) -> usize {
        self.size.0 as usize * self.size.1 as usize * 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor(y: f32) -> Plane {
        Plane::from_matrix(&Mat4::from_translation(Vec3::new(3.0, y, -2.0))).unwrap()
    }

    #[test]
    fn the_plane_is_the_matrix_origin_and_its_y_axis() {
        let p = floor(0.02);
        assert_eq!(p.normal, Vec3::Y);
        assert_eq!(p.point, Vec3::new(3.0, 0.02, -2.0));
        assert!((p.distance(Vec3::new(9.0, 1.02, 4.0)) - 1.0).abs() < 1e-6);
        assert!(p.distance(Vec3::ZERO) < 0.0);
        // An anim group's transform: rotated and moved.
        let m = Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0))
            * Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let p = Plane::from_matrix(&m).unwrap();
        assert!((p.normal - Vec3::NEG_X).length() < 1e-6);
        assert_eq!(p.point, Vec3::new(0.0, 5.0, 0.0));
        // The normal's length does not matter; a matrix without a y axis is no plane.
        let scaled = Plane::from_matrix(&Mat4::from_scale(Vec3::new(1.0, 7.0, 1.0))).unwrap();
        assert_eq!(scaled.normal, Vec3::Y);
        assert!(Plane::from_matrix(&Mat4::from_scale(Vec3::new(1.0, 0.0, 1.0))).is_none());
        assert!(Plane::from_matrix(&Mat4::from_scale(Vec3::splat(f32::NAN))).is_none());
    }

    #[test]
    fn reflection_is_the_reference_matrix() {
        let p = floor(2.0);
        let r = p.reflection();
        // Points on the plane stay, a point above goes as far below, twice is the identity.
        assert_eq!(
            r.transform_point3(Vec3::new(1.0, 2.0, 1.0)),
            Vec3::new(1.0, 2.0, 1.0)
        );
        assert_eq!(
            r.transform_point3(Vec3::new(1.0, 5.0, 1.0)),
            Vec3::new(1.0, -1.0, 1.0)
        );
        assert!((r * r).abs_diff_eq(Mat4::IDENTITY, 1e-6));
        assert!(r.determinant() < 0.0);
        assert!((flip_x() * r).determinant() > 0.0, "winding is kept");

        // A slanted plane, against the formula p - 2 (n . (p - o)) n.
        let m = Mat4::from_translation(Vec3::new(1.0, -2.0, 0.5))
            * Mat4::from_rotation_x(0.4)
            * Mat4::from_rotation_z(-0.7);
        let p = Plane::from_matrix(&m).unwrap();
        let v = Vec3::new(0.3, 4.0, -2.5);
        let expect = v - 2.0 * p.distance(v) * p.normal;
        assert!((p.reflection().transform_point3(v) - expect).length() < 1e-5);

        // Reflecting in the tilted plane is tilting what was reflected in the plane.
        let tilt = Mat4::from_translation(Vec3::new(0.0, 1.0, 2.0))
            * Mat4::from_rotation_x(0.1)
            * Mat4::from_rotation_z(-0.07)
            * Mat4::from_translation(Vec3::new(0.0, -1.0, -2.0));
        let a = p.transformed(&tilt).reflection() * tilt;
        let b = tilt * p.reflection();
        assert!(a.abs_diff_eq(b, 1e-5), "{a:?}\n{b:?}");
    }

    #[test]
    fn oblique_projection_cuts_at_the_plane_and_keeps_the_picture() {
        let proj = Mat4::perspective_rh(1.0, 1.5, 0.1, 3000.0);
        // The eye is 2 below a plane facing up, looking up at it at an angle.
        let view = Mat4::look_at_rh(Vec3::new(0.0, -2.0, 5.0), Vec3::new(0.0, 1.0, 0.0), Vec3::Y);
        let world = Vec4::new(0.0, 1.0, 0.0, 0.0);
        let plane = view.inverse().transpose() * world;
        assert!(plane.w < 0.0);
        let oblique = oblique_projection(&proj, plane).expect("valid");
        let depth = |p: Vec3| {
            let c = oblique * view * p.extend(1.0);
            let plain = proj * view * p.extend(1.0);
            assert!((c.x - plain.x).abs() < 1e-4 && (c.y - plain.y).abs() < 1e-4);
            assert_eq!(c.w, plain.w);
            c.z / c.w
        };
        assert!(depth(Vec3::new(0.5, 0.0, 0.0)).abs() < 1e-5, "on the plane");
        assert!(depth(Vec3::new(0.5, -0.5, 1.0)) < 0.0, "below: clipped");
        let (near, far) = (
            depth(Vec3::new(0.0, 0.5, 0.0)),
            depth(Vec3::new(0.0, 40.0, -30.0)),
        );
        assert!(0.0 < near && near < far && far <= 1.0, "{near} {far}");
        // The far corner the construction aims at lands on depth one.
        let corner = (proj * view).inverse() * Vec4::new(1.0, 1.0, 1.0, 1.0);
        let c = oblique * view * (corner / corner.w);
        assert!((c.z / c.w - 1.0).abs() < 1e-3, "{}", c.z / c.w);

        // The eye above the plane (a camera under the mirror) or no plane at all: no oblique frustum.
        assert!(oblique_projection(&proj, -plane).is_none());
        assert!(oblique_projection(&proj, Vec4::new(0.0, 1.0, 0.0, 0.0)).is_none());
        assert!(oblique_projection(&proj, Vec4::splat(f32::NAN)).is_none());
        // Looking away from the plane: nothing of the kept side is in view.
        let away = Mat4::look_at_rh(
            Vec3::new(0.0, -2.0, 5.0),
            Vec3::new(0.0, -30.0, 4.0),
            Vec3::Z,
        );
        let plane = away.inverse().transpose() * world;
        assert!(oblique_projection(&proj, plane).is_none());
    }

    #[test]
    fn the_lift_moves_the_cut_up() {
        let p = floor(1.0);
        assert_eq!(p.equation(0.0), Vec4::new(0.0, 1.0, 0.0, -1.0));
        let lifted = p.equation(CLIP_LIFT);
        assert!(lifted.dot(Vec4::new(0.0, 1.0 + CLIP_LIFT, 0.0, 1.0)).abs() < 1e-6);
        assert!(lifted.dot(Vec4::new(0.0, 1.0, 0.0, 1.0)) < 0.0);
    }

    #[test]
    fn target_size_is_scaled_rounded_and_never_empty() {
        assert_eq!(target_size((1280, 720), 0.5), (640, 360));
        assert_eq!(target_size((1920, 1080), 0.5), (960, 540));
        assert_eq!(target_size((1281, 721), 0.5), (641, 361));
        assert_eq!(target_size((64, 64), 1.0), (64, 64));
        assert_eq!(target_size((64, 64), 0.001), (1, 1));
        assert_eq!(target_size((1, 1), 0.5), (1, 1));
        assert_eq!(
            target_size((100, 50), 4.0),
            (100, 50),
            "never above the frame"
        );
        assert_eq!(target_size((100, 50), f32::NAN), (1, 1));
    }

    #[test]
    fn distortion_follows_the_reference_shader() {
        // `Render.ts`: halve (0, 1.6, 0) and (1.2, 0, 0) until the larger entry is below one.
        let (mut a, mut b) = (1.6f32, 1.2f32);
        while a.abs().max(b.abs()) >= 1.0 {
            a *= 0.5;
            b *= 0.5;
        }
        assert_eq!(Distortion::IND0, [0.0, a, 0.0]);
        assert_eq!(Distortion::IND1, [b, 0.0, 0.0]);

        // `float(int(255 * v) & 0xF8) - 128`.
        assert_eq!(Distortion::quantise(1.0), 120.0);
        assert_eq!(Distortion::quantise(0.0), -128.0);
        assert_eq!(Distortion::quantise(128.0 / 255.0), 0.0);
        assert_eq!(Distortion::quantise(135.0 / 255.0), 0.0);
        assert_eq!(Distortion::quantise(136.0 / 255.0), 8.0);
        assert_eq!(Distortion::quantise(7.0), 120.0);

        let d = Distortion::reference(TextureId(0));
        // u follows blue; v is the constant the distortion target's alpha of one gives.
        assert_eq!(d.offset([0.3, 0.9, 168.0 / 255.0]), [0.125, 0.28125]);
        assert_eq!(d.offset([1.0, 0.0, 88.0 / 255.0]), [-0.125, 0.28125]);
        assert_eq!(d.offset([0.0, 0.0, 0.0]), [-0.4, 0.28125]);
        // Other matrices reach green.
        let g = Distortion {
            ind1: [0.0, 0.0, 0.5],
            ..d
        };
        assert_eq!(g.offset([0.0, 1.0, 0.5]), [0.0, 0.5 * 120.0 / 256.0]);

        assert_eq!(Mirror::WAVY_ALPHA, 0x60 as f32 / 0xff as f32);
        let wavy = Mirror::wavy(Mat4::IDENTITY, TextureId(3));
        assert_eq!(wavy.alpha, Mirror::WAVY_ALPHA);
        assert_eq!(wavy.distortion, Some(Distortion::reference(TextureId(3))));
        assert_eq!(Mirror::flat(Mat4::IDENTITY).alpha, 1.0);
    }

    /// The lines of a top-level `struct` or `fn` of a WGSL source (whatever the checkout's line ends).
    fn item<'a>(source: &'a str, head: &str) -> Vec<&'a str> {
        let start = source.find(head).unwrap_or_else(|| panic!("{head}"));
        let end = source[start..].find("\n}").expect("closing brace");
        let text = &source[start..start + end];
        text.lines().map(|l| l.trim_end_matches('\r')).collect()
    }

    #[test]
    fn the_overlay_shader_shares_the_scene_object_and_wave_of_the_main_one() {
        // Both programs read the same uniform buffers and must put a waved vertex in the same place.
        let main = include_str!("shader.wgsl");
        let mirror = include_str!("mirror.wgsl");
        for head in [
            "struct Scene {",
            "struct Object {",
            "struct Waved {",
            "fn wave(",
        ] {
            assert_eq!(item(main, head), item(mirror, head), "{head}");
        }
        assert_eq!(std::mem::size_of::<MirrorUniform>(), 64 + 16 * 3);
    }
}
