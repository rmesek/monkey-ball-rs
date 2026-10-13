//! Offscreen tests of the mirror floors: the capture through the reflected camera and the mirror surfaces
//! drawn with it. They skip (pass) when no adapter is available. Every device is requested with the
//! WebGL2 limits; `the_gl_backend_draws_the_same` repeats everything on the GL backend (the browser's
//! WebGL2 path and naga's GLSL ES output) where the machine has one.
//!
//! The scenes: a camera above and in front of a floor in the plane y = 0 that is a mirror surface, and
//! unlit squares standing in the plane z = 0 above (and below) it.

use mb_gma::CullMode;
use mb_render::glam::{Mat4, Vec3};
use mb_render::{
    BlendMode, Camera, Distortion, DrawOrder, GpuCounters, InstanceStyle, Layer, Mirror, ModelId,
    ObjectId, OffscreenTarget, Quad, Renderer, WaveParams,
};

const SIZE: u32 = 64;
const UNORM: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const CLEAR: [f32; 3] = [0.1, 0.1, 0.1];
const RED: [f32; 3] = [1.0, 0.0, 0.0];
const GREEN: [f32; 3] = [0.0, 1.0, 0.0];
const BLUE: [f32; 3] = [0.0, 0.0, 1.0];
const YELLOW: [f32; 3] = [1.0, 1.0, 0.0];
const CYAN: [f32; 3] = [0.0, 1.0, 1.0];
const MAGENTA: [f32; 3] = [1.0, 0.0, 1.0];
/// The floor's own colour, which a mirror of full opacity never shows.
const FLOOR: [f32; 3] = [0.0, 0.0, 0.5];

fn device(backends: wgpu::Backends) -> Option<(wgpu::Device, wgpu::Queue)> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    let instance = wgpu::Instance::new(descriptor);
    for fallback in [false, true] {
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                force_fallback_adapter: fallback,
                ..Default::default()
            }))
        else {
            continue;
        };
        // What the browser's WebGL2 backend offers, and nothing more.
        let required_limits =
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
        if let Ok(dq) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits,
            ..Default::default()
        })) {
            let info = adapter.get_info();
            eprintln!("adapter: {} ({:?})", info.name, info.backend);
            return Some(dq);
        }
    }
    eprintln!("skipped: no GPU adapter for {backends:?}");
    None
}

struct Scene {
    device: wgpu::Device,
    r: Renderer,
    target: OffscreenTarget,
    camera: Camera,
}

impl Scene {
    fn with_format(backends: wgpu::Backends, format: wgpu::TextureFormat) -> Option<Self> {
        let (device, queue) = device(backends)?;
        let mut r = Renderer::new(device.clone(), queue, format);
        r.clear_color = [0.1, 0.1, 0.1, 1.0];
        Some(Self {
            target: OffscreenTarget::new(&device, SIZE, SIZE, format),
            camera: Camera::look_at(Vec3::new(0.0, 6.0, 10.0), Vec3::ZERO, Vec3::Y),
            r,
            device,
        })
    }

    /// Clear colour 0.1 grey; the capture is as large as the frame unless a test says otherwise, so
    /// that a pixel of the reflection is one texel of the capture.
    fn new(backends: wgpu::Backends) -> Option<Self> {
        let mut s = Self::with_format(backends, UNORM)?;
        s.r.set_mirror_scale(1.0);
        Some(s)
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.target = OffscreenTarget::new(&self.device, width, height, self.target.format());
    }

    fn render(&mut self) -> Frame {
        let (w, h) = self.target.size();
        Frame {
            px: self.r.render_to_rgba(&self.target, &self.camera),
            clip: self
                .camera
                .projection(w as f32 / h as f32, self.camera.near, self.camera.far)
                * self.camera.view,
        }
    }
}

struct Frame {
    px: Vec<u8>,
    clip: Mat4,
}

impl Frame {
    /// The pixel a world-space point lands on.
    fn pixel(&self, world: Vec3) -> (usize, usize) {
        let ndc = self.clip.project_point3(world);
        let x = ((ndc.x * 0.5 + 0.5) * SIZE as f32) as usize;
        let y = ((0.5 - ndc.y * 0.5) * SIZE as f32) as usize;
        (x.min(SIZE as usize - 1), y.min(SIZE as usize - 1))
    }

    fn get(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * SIZE as usize + x) * 4;
        self.px[i..i + 4].try_into().unwrap()
    }

    fn at(&self, world: Vec3) -> [u8; 4] {
        let (x, y) = self.pixel(world);
        self.get(x, y)
    }

    /// Pixels within `tolerance` of `rgb`.
    fn count(&self, rgb: [f32; 3], tolerance: f32) -> usize {
        (self.px.as_chunks::<4>().0.iter())
            .filter(|p| near(&p[..], rgb, tolerance))
            .count()
    }
}

fn near(actual: &[u8], expected: [f32; 3], tolerance: f32) -> bool {
    (0..3).all(|c| (actual[c] as f32 - (expected[c] * 255.0).round()).abs() <= tolerance)
}

#[track_caller]
fn assert_rgb(actual: [u8; 4], expected: [f32; 3], what: &str) {
    assert!(
        near(&actual, expected, 2.0),
        "{what}: got {actual:?}, expected about {:?}",
        expected.map(|v| (v * 255.0).round())
    );
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] * (1.0 - t) + b[i] * t)
}

/// An unlit, untextured NaomiLib quad drawing `color`, both faces, without blending. `corners` are
/// (low, low), (high, low), (low, high), (high, high); `normal` and the vertex alpha only matter to the
/// mirror overlay.
fn nl_quad(
    r: &mut Renderer,
    corners: [[f32; 3]; 4],
    normal: [f32; 3],
    color: [f32; 3],
    vertex_alpha: u8,
) -> ModelId {
    let mesh = mb_gma::NlMesh {
        flags: 0,
        tex_flags: 0,
        tex_idx: None,
        mesh_type: mb_gma::mesh_type::UNLIT_CONST_MAT_COLOR,
        ambient_color_scale: 1.0,
        material_color: [color[0], color[1], color[2], 1.0],
        cull: CullMode::None,
        // LessEqual in the reference's table.
        z_compare: 4,
        depth_write: true,
        vertices: mb_gma::Vertices {
            normals: vec![normal; 4],
            colors: vec![[255, 255, 255, vertex_alpha]; 4],
            uv0: vec![[0.0; 2]; 4],
            uv1: vec![[0.0; 2]; 4],
            uv2: vec![[0.0; 2]; 4],
            positions: corners.to_vec(),
        },
        indices: vec![0, 1, 2, 2, 1, 3],
    };
    let reach = (corners.iter())
        .map(|c| Vec3::from(*c).length())
        .fold(0.0, f32::max);
    r.upload_nl_model(
        &mb_gma::NlModel {
            id: 0,
            flags: 0,
            kind: mb_gma::VertexKind::B,
            bound_sphere_center: [0.0; 3],
            bound_sphere_radius: reach,
            meshes: vec![mesh],
        },
        &[],
    )
}

/// A square of half side `half` standing in the plane z = 0 about the model's origin.
fn square(r: &mut Renderer, half: f32, color: [f32; 3]) -> ModelId {
    let corners = [
        [-half, -half, 0.0],
        [half, -half, 0.0],
        [-half, half, 0.0],
        [half, half, 0.0],
    ];
    nl_quad(r, corners, [0.0, 0.0, 1.0], color, 255)
}

/// The floor: 24 units square in the plane y = 0, in [`FLOOR`]'s colour.
fn floor_model(r: &mut Renderer, normal: [f32; 3], vertex_alpha: u8) -> ModelId {
    let h = 12.0;
    let corners = [[-h, 0.0, -h], [h, 0.0, -h], [-h, 0.0, h], [h, 0.0, h]];
    nl_quad(r, corners, normal, FLOOR, vertex_alpha)
}

/// The usual floor as a mirror surface.
fn mirror_floor(r: &mut Renderer) -> ObjectId {
    let model = floor_model(r, [0.0, 1.0, 0.0], 255);
    let floor = r.add_object(model, Layer::World, Mat4::IDENTITY);
    r.set_mirror_surface(floor, true);
    floor
}

fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

/// `p` reflected in the plane through `origin` facing `normal`.
fn reflect(p: Vec3, origin: Vec3, normal: Vec3) -> Vec3 {
    let n = normal.normalize();
    p - 2.0 * n.dot(p - origin) * n
}

fn delta(after: GpuCounters, before: GpuCounters) -> GpuCounters {
    GpuCounters {
        buffers: after.buffers - before.buffers,
        textures: after.textures - before.textures,
        pipelines: after.pipelines - before.pipelines,
        bind_groups: after.bind_groups - before.bind_groups,
        samplers: after.samplers - before.samplers,
    }
}

// ---- a flat mirror ----------------------------------------------------------------------------------

fn flat_mirror_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    let floor = mirror_floor(&mut s.r);
    let red = square(&mut s.r, 1.0, RED);
    s.r.add_object(red, Layer::World, at(0.0, 2.0, 0.0));
    // Stands through the floor: from y = -3 to y = 1.
    let green = square(&mut s.r, 2.0, GREEN);
    s.r.add_object(green, Layer::World, at(-4.0, -1.0, 0.0));
    // Wholly under the floor.
    let blue = square(&mut s.r, 1.0, BLUE);
    s.r.add_object(blue, Layer::World, at(4.0, -3.0, 0.0));
    // Part of the backdrop, far behind.
    let yellow = square(&mut s.r, 2.0, YELLOW);
    s.r.add_object(yellow, Layer::Background, at(6.0, 5.0, -20.0));

    // No mirror: the floor hides what is under it and shows its own colour.
    let plain = s.render();
    let before = s.r.stats;
    assert_eq!((before.draws, before.mirror_draws), (5, 0));
    assert_rgb(plain.at(Vec3::new(0.0, -2.0, 0.0)), FLOOR, "no mirror");
    let empty_floor = Vec3::new(5.0, 0.0, 3.0);
    assert_rgb(plain.at(empty_floor), FLOOR, "no mirror");

    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    let frame = s.render();
    // The picture's own draws are what they were; the capture adds the three objects that reach above
    // the plane (the blue one is culled, the floor is the mirror) and the overlay one for the floor.
    let stats = s.r.stats;
    assert_eq!(stats.draws, before.draws);
    assert_eq!(stats.translucent_draws, before.translucent_draws);
    assert_eq!((stats.mirror_draws, stats.mirror_surface_draws), (4, 1));
    assert_eq!(stats.total_draws(), before.total_draws() + 4);

    assert_rgb(frame.at(Vec3::new(0.0, 2.0, 0.0)), RED, "the square itself");
    assert_rgb(
        frame.at(Vec3::new(0.0, -2.0, 0.0)),
        RED,
        "its reflection, as far below the floor",
    );
    assert_rgb(
        frame.at(Vec3::new(0.7, -2.7, 0.0)),
        RED,
        "a corner of the reflection",
    );
    assert_rgb(
        frame.at(Vec3::new(0.0, -3.5, 0.0)),
        CLEAR,
        "past the reflection: the empty sky of the capture",
    );
    assert_rgb(
        frame.at(empty_floor),
        CLEAR,
        "the mirror is not in its own picture",
    );
    assert_eq!(frame.count(FLOOR, 40.0), 0, "the overlay covers the floor");
    assert_rgb(
        frame.at(Vec3::new(6.0, -5.0, -20.0)),
        YELLOW,
        "the backdrop is reflected too",
    );
    // The part of the green square above the floor is reflected; the part below it is cut off at the
    // plane (unclipped it would show where its mirror image is, above the real one).
    assert_rgb(
        frame.at(Vec3::new(-4.0, 0.5, 0.0)),
        GREEN,
        "above the floor",
    );
    assert_rgb(
        frame.at(Vec3::new(-4.0, -0.5, 0.0)),
        GREEN,
        "reflection of the part above",
    );
    assert_rgb(
        frame.at(Vec3::new(-4.0, -2.0, 0.0)),
        CLEAR,
        "the reflection ends where the square meets the floor",
    );
    assert_rgb(
        frame.at(Vec3::new(-4.0, 2.0, 0.0)),
        CLEAR,
        "nothing from below the plane",
    );
    assert_rgb(
        frame.at(Vec3::new(4.0, 3.0, 0.0)),
        CLEAR,
        "nothing from below the plane",
    );
    assert_eq!(frame.count(BLUE, 40.0), 0);

    // Against the same square drawn at the mirrored pose, pixel by pixel.
    let Some(mut twin) = Scene::new(backends) else {
        return;
    };
    let red = square(&mut twin.r, 1.0, RED);
    twin.r.add_object(
        red,
        Layer::World,
        at(0.0, -2.0, 0.0) * Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)),
    );
    let posed = twin.render();
    let (lo, hi) = (
        posed.pixel(Vec3::new(-1.5, -0.6, 0.0)),
        posed.pixel(Vec3::new(1.5, -3.4, 0.0)),
    );
    let (mut reds, mut differing) = (0, 0);
    for y in lo.1..=hi.1 {
        for x in lo.0..=hi.0 {
            let (a, b) = (frame.get(x, y), posed.get(x, y));
            reds += near(&b, RED, 2.0) as usize;
            differing += (0..3).any(|c| a[c].abs_diff(b[c]) > 2) as usize;
        }
    }
    assert!(reds > 40, "{reds} pixels of the posed square");
    assert_eq!(differing, 0, "of {reds}");

    // The default capture has half the side; the reflection is the same picture, coarser.
    s.r.set_mirror_scale(0.5);
    let frame = s.render();
    assert_eq!(s.r.mirror_size(), Some((SIZE / 2, SIZE / 2)));
    assert_rgb(frame.at(Vec3::new(0.0, -2.0, 0.0)), RED, "half resolution");
    assert_rgb(frame.at(empty_floor), CLEAR, "half resolution");
    assert_rgb(
        frame.at(Vec3::new(-4.0, 2.0, 0.0)),
        CLEAR,
        "half resolution",
    );

    // No mirror surface in view: no capture, no overlay.
    s.r.set_visible(floor, false);
    s.render();
    assert_eq!(s.r.stats.mirror_draws, 0);
    assert_eq!(s.r.stats.draws, before.draws - 1);
}

#[test]
fn a_flat_mirror_shows_what_is_above_it_at_the_mirrored_place() {
    flat_mirror_on(wgpu::Backends::all());
}

// ---- what the capture leaves out ----------------------------------------------------------------------

fn capture_contents_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    let floor = mirror_floor(&mut s.r);
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    let model = square(&mut s.r, 0.8, RED);
    let hidden = s.r.add_object(model, Layer::World, at(-5.0, 2.0, 0.0));
    s.r.set_in_mirror(hidden, false);
    // A second mirror model, standing: not in the capture either.
    let model = square(&mut s.r, 0.8, MAGENTA);
    let standing = s.r.add_object(model, Layer::World, at(5.0, 2.0, 0.0));
    s.r.set_mirror_surface(standing, true);
    let white = square(&mut s.r, 0.8, [1.0; 3]);
    let untilted = |order| InstanceStyle {
        tilted: false,
        order,
        ..InstanceStyle::OPAQUE
    };
    let confetti =
        s.r.create_instance_batch(white, untilted(DrawOrder::Opaque));
    let star =
        s.r.create_instance_batch(white, untilted(DrawOrder::AfterSorted));
    let glow = s.r.create_instance_batch(
        white,
        InstanceStyle {
            tilted: false,
            ..InstanceStyle::GLOW
        },
    );
    let texture = s.r.white_texture();
    s.r.warm_quad_texture(texture);
    let push = |r: &mut Renderer| {
        r.push_instance(confetti, &at(0.0, 2.0, 0.0), [0.0, 1.0, 1.0, 1.0]);
        r.push_instance(star, &at(2.5, 2.0, 0.0), [1.0, 1.0, 0.0, 1.0]);
        r.push_instance(glow, &at(0.0, 4.5, 0.0), [0.0, 0.5, 0.0, 1.0]);
        let h = 0.8;
        let corner = |x: f32, y: f32| Vec3::new(-2.5 + x * h, 2.0 + y * h, 0.0);
        let quad = Quad::new(
            [
                corner(-1.0, 1.0),
                corner(1.0, 1.0),
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
            ],
            [0, 64, 0, 255],
        );
        for order in [DrawOrder::BeforeSorted, DrawOrder::AfterSorted] {
            r.push_quad(texture, BlendMode::Additive, order, &quad);
        }
    };
    push(&mut s.r);
    let frame = s.render();
    // All of them are in the picture.
    assert_rgb(frame.at(Vec3::new(-5.0, 2.0, 0.0)), RED, "object");
    assert_rgb(frame.at(Vec3::new(0.0, 2.0, 0.0)), CYAN, "opaque batch");
    assert_rgb(frame.at(Vec3::new(2.5, 2.0, 0.0)), YELLOW, "sorted batch");
    assert_rgb(
        frame.at(Vec3::new(0.0, 4.5, 0.0)),
        [0.1, 0.6, 0.1],
        "additive batch",
    );
    assert_rgb(
        frame.at(Vec3::new(-2.5, 2.0, 0.0)),
        [0.1, 0.1 + 2.0 * 64.0 / 255.0, 0.1],
        "two additive quads",
    );
    // Only the opaque batch (confetti) is in the mirror.
    assert_rgb(
        frame.at(Vec3::new(0.0, -2.0, 0.0)),
        CYAN,
        "opaque batch, reflected",
    );
    for (x, y, what) in [
        (-5.0, -2.0, "an object taken out of the mirror"),
        (5.0, -2.0, "a mirror surface"),
        (2.5, -2.0, "a sorted batch"),
        (0.0, -4.5, "an additive batch"),
        (-2.5, -2.0, "quads"),
    ] {
        assert_rgb(frame.at(Vec3::new(x, y, 0.0)), CLEAR, what);
    }
    assert_eq!(
        frame.count(MAGENTA, 40.0),
        0,
        "it shows the capture instead"
    );
    let stats = s.r.stats;
    assert_eq!((stats.draws, stats.instance_draws), (6, 3));
    assert_eq!(stats.quad_draws, 2);
    // The capture draws the one batch; the overlay the floor and the standing square.
    assert_eq!((stats.mirror_draws, stats.mirror_surface_draws), (3, 2));

    // Back in the mirror.
    s.r.set_in_mirror(hidden, true);
    s.r.set_mirror_surface(standing, false);
    push(&mut s.r);
    let frame = s.render();
    assert_rgb(frame.at(Vec3::new(-5.0, -2.0, 0.0)), RED, "in the mirror");
    assert_rgb(frame.at(Vec3::new(5.0, 2.0, 0.0)), MAGENTA, "itself again");
    assert_rgb(
        frame.at(Vec3::new(5.0, -2.0, 0.0)),
        MAGENTA,
        "in the mirror",
    );
    assert_eq!(
        (s.r.stats.mirror_draws, s.r.stats.mirror_surface_draws),
        (4, 1)
    );

    // An object of the sorted pass is in the capture as well (these meshes do not blend).
    s.r.set_alpha(hidden, 0.5);
    push(&mut s.r);
    let frame = s.render();
    assert_eq!(s.r.stats.translucent_draws, 1);
    assert_eq!(s.r.stats.mirror_draws, 4);
    assert_rgb(frame.at(Vec3::new(-5.0, 2.0, 0.0)), RED, "sorted");
    assert_rgb(
        frame.at(Vec3::new(-5.0, -2.0, 0.0)),
        RED,
        "sorted, reflected",
    );

    // Without the mirror the same frame costs what it did.
    s.r.set_mirror(None);
    s.r.set_mirror_surface(floor, true);
    push(&mut s.r);
    let frame = s.render();
    assert_eq!(s.r.stats.mirror_draws, 0);
    assert_eq!(s.r.stats.total_draws(), 6 + 2);
    assert_rgb(frame.at(Vec3::new(0.0, -2.0, 0.0)), FLOOR, "a plain floor");
}

#[test]
fn the_capture_leaves_out_mirrors_flagged_objects_and_effects() {
    capture_contents_on(wgpu::Backends::all());
}

// ---- alpha ------------------------------------------------------------------------------------------

fn alpha_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    // Left half of the floor with full vertex alpha, right half with 128.
    let h = 12.0;
    let left = nl_quad(
        &mut s.r,
        [[-h, 0.0, -h], [0.0, 0.0, -h], [-h, 0.0, h], [0.0, 0.0, h]],
        [0.0, 1.0, 0.0],
        FLOOR,
        255,
    );
    let right = nl_quad(
        &mut s.r,
        [[0.0, 0.0, -h], [h, 0.0, -h], [0.0, 0.0, h], [h, 0.0, h]],
        [0.0, 1.0, 0.0],
        FLOOR,
        128,
    );
    for model in [left, right] {
        let o = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
        s.r.set_mirror_surface(o, true);
    }
    let red = square(&mut s.r, 1.0, RED);
    s.r.add_object(red, Layer::World, at(-3.0, 2.0, 0.0));
    s.r.add_object(red, Layer::World, at(3.0, 2.0, 0.0));
    let (l, r) = (Vec3::new(-3.0, -2.0, 0.0), Vec3::new(3.0, -2.0, 0.0));
    let (l_empty, r_empty) = (Vec3::new(-4.0, 0.0, 4.0), Vec3::new(4.0, 0.0, 4.0));
    let va = 128.0 / 255.0;

    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    let frame = s.render();
    assert_rgb(frame.at(l), RED, "full alpha");
    assert_rgb(frame.at(l_empty), CLEAR, "full alpha");
    assert_rgb(frame.at(r), mix(FLOOR, RED, va), "vertex alpha");
    assert_rgb(frame.at(r_empty), mix(FLOOR, CLEAR, va), "vertex alpha");

    // `WAVY_MIRROR_ALPHA` without the distortion.
    let alpha = Mirror::WAVY_ALPHA;
    s.r.set_mirror(Some(Mirror {
        alpha,
        ..Mirror::flat(Mat4::IDENTITY)
    }));
    assert_eq!(s.r.mirror().map(|m| m.alpha), Some(alpha));
    let frame = s.render();
    assert_rgb(frame.at(l), mix(FLOOR, RED, alpha), "mirror alpha");
    assert_rgb(frame.at(l_empty), mix(FLOOR, CLEAR, alpha), "mirror alpha");
    assert_rgb(frame.at(r), mix(FLOOR, RED, alpha * va), "both");
    assert_rgb(frame.at(r_empty), mix(FLOOR, CLEAR, alpha * va), "both");
    assert_eq!(s.r.stats.mirror_surface_draws, 2);

    // Nothing shows at alpha zero, and the frame's own alpha stays one throughout.
    s.r.set_mirror(Some(Mirror {
        alpha: 0.0,
        ..Mirror::flat(Mat4::IDENTITY)
    }));
    let frame = s.render();
    assert_rgb(frame.at(l), FLOOR, "alpha zero");
    assert!(frame.px.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
}

#[test]
fn the_reflection_is_blended_by_mirror_alpha_times_vertex_alpha() {
    alpha_on(wgpu::Backends::all());
}

// ---- a moving plane -----------------------------------------------------------------------------------

fn moving_plane_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    let floor = mirror_floor(&mut s.r);
    let red = square(&mut s.r, 1.0, RED);
    let p = Vec3::new(0.0, 2.5, 0.0);
    s.r.add_object(red, Layer::World, Mat4::from_translation(p));
    // A frame before the mirror exists, as a stage load has: the target is made with the mirror.
    s.render();
    let loaded = s.r.counters();
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    assert_eq!(s.r.mirror_size(), Some((SIZE, SIZE)));
    let made = delta(s.r.counters(), loaded);
    assert_eq!(
        made,
        GpuCounters {
            buffers: 3,
            textures: 2,
            pipelines: 1,
            bind_groups: 3,
            samplers: 0,
        },
        "two scene uniforms and the overlay's, colour and depth, the overlay pipeline, three groups"
    );
    let ready = s.r.counters();

    // The mirror rides on a transform that sinks and rocks, like an anim group's.
    let pose = |i: usize| {
        let t = i as f32 / 60.0;
        at(0.0, -t, 0.0) * Mat4::from_rotation_x(0.12 * (t * 6.0).sin())
    };
    let mut draws = None;
    for i in 0..=60 {
        let m = pose(i);
        s.r.set_transform(floor, m);
        s.r.set_mirror_plane(m);
        let frame = s.render();
        let image = reflect(
            p,
            m.transform_point3(Vec3::ZERO),
            m.transform_vector3(Vec3::Y),
        );
        assert_rgb(frame.at(image), RED, &format!("frame {i}: {image}"));
        assert_rgb(frame.at(p), RED, "the square itself");
        // The capture's two scene uniforms, the overlay's and the floor's object slot, nothing else.
        assert!(s.r.stats.upload_bytes > 0);
        assert_eq!(
            *draws.get_or_insert(s.r.stats.total_draws()),
            s.r.stats.total_draws()
        );
        assert_eq!(s.r.stats.mirror_draws, 2);
    }
    assert_eq!(s.r.counters(), ready, "a moving mirror creates nothing");
    assert_eq!(s.r.mirror().map(|m| m.plane), Some(pose(60)));

    // A frame with nothing moved is not drawn again.
    let size = s.target.size();
    assert!(!s.r.prepare(size, &s.camera), "identical frame");
    assert_eq!(s.r.stats.upload_bytes, 0);
    assert_eq!(
        s.r.stats.mirror_draws, 2,
        "the counts of the frame on screen"
    );
    // Moving only the plane is a change.
    s.r.set_mirror_plane(pose(30));
    assert!(s.r.prepare(size, &s.camera));
    s.r.draw(s.target.view());

    // A plane the camera is under, or none at all, is survived; so is a mirror nobody set.
    s.r.set_mirror_plane(at(0.0, 50.0, 0.0));
    s.render();
    s.r.set_mirror_plane(Mat4::ZERO);
    s.render();
    assert_eq!(s.r.stats.mirror_draws, 0, "no plane, no mirror");
    s.r.set_mirror(None);
    s.r.set_mirror_plane(Mat4::IDENTITY);
    s.render();
    assert_eq!(s.r.counters(), ready);
}

#[test]
fn a_moving_plane_moves_the_reflection_and_creates_nothing() {
    moving_plane_on(wgpu::Backends::all());
}

// ---- the stage tilt -----------------------------------------------------------------------------------

fn tilt_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    let floor = mirror_floor(&mut s.r);
    s.r.set_tilted(floor, true);
    let red = square(&mut s.r, 1.0, RED);
    let stage = Vec3::new(-3.0, 2.0, 0.0);
    let o =
        s.r.add_object(red, Layer::World, Mat4::from_translation(stage));
    s.r.set_tilted(o, true);
    // Not part of the stage (the ball): it stays where it is while the stage tilts under it.
    let cyan = square(&mut s.r, 1.0, CYAN);
    let free = Vec3::new(3.0, 2.0, 0.0);
    s.r.add_object(cyan, Layer::World, Mat4::from_translation(free));
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));

    let pivot = Vec3::new(1.0, 0.5, 2.0);
    for (rx, rz) in [(0.0, 0.0), (0.15, 0.0), (-0.1, 0.12)] {
        let tilt = Mat4::from_translation(pivot)
            * Mat4::from_rotation_x(rx)
            * Mat4::from_rotation_z(rz)
            * Mat4::from_translation(-pivot);
        s.r.set_tilt(tilt);
        let frame = s.render();
        let what = format!("tilt {rx} {rz}");
        // The stage's reflection tilts with it: the reference's view * tilt * reflection.
        assert_rgb(frame.at(tilt.transform_point3(stage)), RED, &what);
        let image = tilt.transform_point3(reflect(stage, Vec3::ZERO, Vec3::Y));
        assert_rgb(frame.at(image), RED, &what);
        // What does not tilt is reflected in the mirror as it is shown.
        let shown = (
            tilt.transform_point3(Vec3::ZERO),
            tilt.transform_vector3(Vec3::Y),
        );
        assert_rgb(frame.at(free), CYAN, &what);
        assert_rgb(frame.at(reflect(free, shown.0, shown.1)), CYAN, &what);
        assert_eq!(frame.count(FLOOR, 40.0), 0, "{what}");
        assert_eq!(s.r.stats.mirror_draws, 3);
    }
}

#[test]
fn the_mirror_tilts_with_the_stage() {
    tilt_on(wgpu::Backends::all());
}

// ---- the wavy mirror ----------------------------------------------------------------------------------

/// `gb_grad` stood in for by two texels: blue 88 on the left, 168 on the right.
fn grad(r: &mut Renderer) -> mb_render::TextureId {
    r.upload_rgba(2, 1, &[10, 40, 88, 255, 240, 200, 168, 255])
}

fn wavy_on(backends: wgpu::Backends) {
    // The floor's normal as the camera sees it picks the texel: the camera only pitches, so a normal
    // along +x or -x addresses the right or the left end of the gradient.
    for (normal_x, shift_x) in [(1.0, 8i32), (-1.0, -8)] {
        let Some(mut s) = Scene::new(backends) else {
            return;
        };
        let model = floor_model(&mut s.r, [normal_x, 0.0, 0.0], 255);
        let floor = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
        let red = square(&mut s.r, 1.0, RED);
        s.r.add_object(red, Layer::World, at(0.0, 2.0, 0.0));
        let green = square(&mut s.r, 1.0, GREEN);
        s.r.add_object(green, Layer::World, at(-3.5, 3.5, -2.0));
        let grad = grad(&mut s.r);
        // The offset the shader must apply, from the reference's formula on the texel addressed.
        let blue = if normal_x > 0.0 { 168.0 } else { 88.0 };
        let offset = Distortion::reference(grad).offset([0.0, 0.0, blue / 255.0]);
        assert_eq!(offset, [shift_x as f32 / SIZE as f32, 18.0 / SIZE as f32]);

        // Which pixels are floor.
        let plain = s.render();
        s.r.set_mirror_surface(floor, true);
        s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
        let flat = s.render();
        s.r.set_mirror(Some(Mirror {
            alpha: 1.0,
            ..Mirror::wavy(Mat4::IDENTITY, grad)
        }));
        let wavy = s.render();
        assert_eq!(
            (s.r.stats.mirror_draws, s.r.stats.mirror_surface_draws),
            (3, 1),
            "one pass and one draw, as the flat mirror"
        );

        // A fragment shows the capture `offset` away: 0.125 of 64 pixels sideways (the capture is
        // turned round in x) and 0.28125 of them up, which is the flat mirror's pixel 8 to the side
        // and 18 above.
        let size = SIZE as i32;
        let is_floor = |x: i32, y: i32| near(&plain.get(x as usize, y as usize), FLOOR, 2.0);
        let (mut compared, mut reflected) = (0, 0);
        for y in 0..size {
            for x in 0..size {
                let (fx, fy) = (x - shift_x, y - 18);
                if fx < 0 || fx >= size || fy < 0 || !is_floor(x, y) || !is_floor(fx, fy) {
                    continue;
                }
                let (a, b) = (
                    wavy.get(x as usize, y as usize),
                    flat.get(fx as usize, fy as usize),
                );
                assert!(
                    (0..3).all(|c| a[c].abs_diff(b[c]) <= 2),
                    "normal {normal_x}: pixel {x},{y} is {a:?}, the flat mirror at {fx},{fy} is {b:?}"
                );
                compared += 1;
                reflected += (!near(&a, CLEAR, 2.0)) as usize;
            }
        }
        assert!(compared > 500, "{compared} floor pixels compared");
        assert!(reflected > 30, "{reflected} of them show a reflection");
        assert_ne!(wavy.px, flat.px);
    }

    // The reference's wavy mirror on a floor that waves: the overlay follows the waved surface (no
    // floor shows through at full alpha) and is blended at 0x60 / 0xff.
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    // Its corners are 8.49 from the axis: at this time the wave lifts them by 0.24, above the mirror's
    // plane and above where an overlay that did not wave would be.
    let h = 6.0;
    let corners = [[-h, 0.0, -h], [h, 0.0, -h], [-h, 0.0, h], [h, 0.0, h]];
    let model = nl_quad(&mut s.r, corners, [0.0, 1.0, 0.0], FLOOR, 255);
    let floor = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
    s.r.set_mirror_surface(floor, true);
    s.r.set_wave(floor, Some(WaveParams::BONUS_WAVE));
    let time = 202.3;
    let (lifted, _) = WaveParams::BONUS_WAVE.displace(Vec3::new(h, 0.0, h), Vec3::Y, time);
    assert!(lifted.y > 0.2, "{lifted}");
    s.r.set_wave_time(time);
    let plain = s.render();
    let floor_pixels = plain.count(FLOOR, 2.0);
    assert!(floor_pixels > 600, "{floor_pixels}");
    let grad = grad(&mut s.r);
    let plane = at(0.0, Mirror::WAVY_PLANE_Y, 0.0);
    s.r.set_mirror(Some(Mirror {
        alpha: 1.0,
        ..Mirror::wavy(plane, grad)
    }));
    let frame = s.render();
    assert_eq!(
        frame.count(FLOOR, 40.0),
        0,
        "the overlay hides the waved floor"
    );
    s.r.set_mirror(Some(Mirror::wavy(plane, grad)));
    let frame = s.render();
    let expect = mix(FLOOR, CLEAR, Mirror::WAVY_ALPHA);
    assert_eq!(
        frame.count(expect, 2.0),
        floor_pixels,
        "every pixel of the floor is under the mirror"
    );
    // The wave runs on: only the floor's slot is uploaded, nothing is created.
    let before = s.r.counters();
    for i in 0..10 {
        s.r.set_wave_time(time + i as f32);
        s.render();
        assert_eq!(s.r.stats.mirror_surface_draws, 1);
    }
    assert_eq!(s.r.counters(), before);

    // A gradient that is gone takes the mirror with it instead of leaving a dangling id.
    s.r.truncate_textures(1);
    assert!(s.r.mirror().is_none());
    assert_eq!(s.r.mirror_bytes(), 0);
    s.render();
}

#[test]
fn the_wavy_mirror_offsets_the_reflection_by_the_reference_formula() {
    wavy_on(wgpu::Backends::all());
}

// ---- no mirror ----------------------------------------------------------------------------------------

fn no_mirror_on(backends: wgpu::Backends) {
    let build = |s: &mut Scene| -> (ObjectId, ObjectId) {
        let model = floor_model(&mut s.r, [0.0, 1.0, 0.0], 255);
        let floor = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
        let red = square(&mut s.r, 1.0, RED);
        let mover = s.r.add_object(red, Layer::World, at(0.0, 2.0, 0.0));
        let far = square(&mut s.r, 2.0, YELLOW);
        s.r.add_object(far, Layer::Background, at(0.0, 5.0, -20.0));
        (floor, mover)
    };
    // The frame counts that tests cannot take from another renderer: the picture's own.
    let frame_stats = |r: &Renderer| {
        let s = r.stats;
        (
            (s.draws, s.translucent_draws, s.instance_draws, s.quad_draws),
            (s.sprite_draws, s.state_changes, s.upload_bytes, s.objects),
        )
    };
    let (Some(mut plain), Some(mut s)) = (
        Scene::with_format(backends, UNORM),
        Scene::with_format(backends, UNORM),
    ) else {
        return;
    };
    let fresh = s.r.counters();
    assert_eq!(plain.r.counters(), fresh);
    let (_, plain_mover) = build(&mut plain);
    let (floor, mover) = build(&mut s);
    // Everything about mirrors but the mirror itself.
    s.r.set_mirror_surface(floor, true);
    s.r.set_in_mirror(mover, false);
    s.r.set_mirror_plane(at(0.0, 1.0, 0.0));
    s.r.set_mirror_scale(0.25);
    s.r.set_mirror(None);
    for i in 0..4 {
        let m = at(i as f32 * 0.5, 2.0, 0.0);
        plain.r.set_transform(plain_mover, m);
        s.r.set_transform(mover, m);
        let (a, b) = (plain.render(), s.render());
        assert_eq!(a.px, b.px, "frame {i}");
        assert_eq!(frame_stats(&plain.r), frame_stats(&s.r), "frame {i}");
        assert_eq!(s.r.stats.mirror_draws, 0);
        assert_eq!(s.r.stats.total_draws(), 3);
        assert_eq!(plain.r.counters(), s.r.counters(), "frame {i}");
    }
    assert_eq!((s.r.mirror_bytes(), s.r.mirror_size()), (0, None));
    assert!(s.r.mirror().is_none());
    let idle = s.r.counters();
    let textures = s.r.texture_bytes();

    // With the mirror the same frame gains the capture and the overlay ...
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    s.r.set_in_mirror(mover, true);
    let with = s.render();
    assert_ne!(with.px, plain.render().px);
    assert_eq!(frame_stats(&plain.r).0, frame_stats(&s.r).0);
    assert_eq!(s.r.stats.mirror_draws, 3);
    assert_eq!(s.r.mirror_size(), Some((SIZE / 4, SIZE / 4)));
    assert_eq!(s.r.mirror_bytes(), (SIZE / 4 * SIZE / 4 * 8) as usize);
    assert_eq!(s.r.texture_bytes(), textures, "the target is counted apart");
    assert!(s.r.counters().total() > idle.total());

    // ... and without it again it is the frame it was, and nothing more is ever created.
    s.r.set_mirror(None);
    let after = s.r.counters();
    for i in 0..4 {
        let m = at(-(i as f32) * 0.5, 2.0, 0.0);
        plain.r.set_transform(plain_mover, m);
        s.r.set_transform(mover, m);
        let (a, b) = (plain.render(), s.render());
        assert_eq!(a.px, b.px, "frame {i} after");
        assert_eq!(frame_stats(&plain.r), frame_stats(&s.r), "frame {i} after");
        assert_eq!(s.r.stats.total_draws(), 3);
    }
    assert_eq!(s.r.counters(), after);
    assert_eq!(s.r.mirror_bytes(), 0);

    // Unloading the stage unloads its mirror.
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    s.render();
    assert!(s.r.mirror_bytes() > 0);
    s.r.clear_scene();
    assert!(s.r.mirror().is_none());
    assert_eq!(s.r.mirror_bytes(), 0);
    s.render();
    assert_eq!(s.r.stats.total_draws(), 0);
    // The next stage's objects are no mirror surfaces because the last one's were.
    let (_, _) = build(&mut s);
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    s.render();
    assert_eq!(s.r.stats.mirror_draws, 0);
}

#[test]
fn without_a_mirror_a_frame_is_what_it_was() {
    no_mirror_on(wgpu::Backends::all());
}

// ---- the target ---------------------------------------------------------------------------------------

fn target_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::with_format(backends, UNORM) else {
        return;
    };
    mirror_floor(&mut s.r);
    let red = square(&mut s.r, 1.0, RED);
    s.r.add_object(red, Layer::World, at(0.0, 2.0, 0.0));
    // Set before any frame: the target comes with the first one.
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    assert_eq!((s.r.mirror_size(), s.r.mirror_bytes()), (None, 0));
    s.render();
    assert_eq!(
        s.r.mirror_size(),
        Some((32, 32)),
        "half the frame by default"
    );
    assert_eq!(s.r.mirror_bytes(), 32 * 32 * 8);
    assert_eq!(s.r.texture_bytes(), 4, "only the white texel is a texture");
    let first = s.r.counters();
    s.render();
    assert_eq!(s.r.counters(), first);

    // A resized frame: its depth buffer, and the mirror's colour, depth and bind group.
    for (w, h, expect) in [
        (100, 50, (50, 25)),
        (1280, 720, (640, 360)),
        (1920, 1080, (960, 540)),
        (3, 1, (2, 1)),
    ] {
        let before = s.r.counters();
        s.resize(w, h);
        s.render();
        assert_eq!(s.r.mirror_size(), Some(expect), "{w} x {h}");
        assert_eq!(s.r.mirror_bytes(), (expect.0 * expect.1 * 8) as usize);
        assert_eq!(
            delta(s.r.counters(), before),
            GpuCounters {
                textures: 3,
                bind_groups: 1,
                ..GpuCounters::default()
            },
            "{w} x {h}"
        );
        assert_eq!(s.r.stats.mirror_draws, 2);
        let settled = s.r.counters();
        s.render();
        assert_eq!(s.r.counters(), settled, "{w} x {h}: once");
    }
    assert_eq!(s.r.texture_bytes(), 4);

    // The scale: any fraction, never above the frame, never below a pixel.
    s.resize(SIZE, SIZE);
    for (scale, expect) in [
        (1.0, (64, 64)),
        (0.25, (16, 16)),
        (0.0, (1, 1)),
        (7.0, (64, 64)),
        (0.5, (32, 32)),
    ] {
        s.r.set_mirror_scale(scale);
        s.render();
        assert_eq!(s.r.mirror_size(), Some(expect), "scale {scale}");
        assert_eq!(s.r.stats.mirror_draws, 2);
    }
    s.r.set_mirror_scale(f32::NAN);
    s.render();
    assert_eq!(s.r.mirror_size(), Some((32, 32)), "not a number: ignored");

    // Changing the shadow mask rebuilds the capture's scene groups with the picture's.
    let before = s.r.counters();
    let mask = s.r.upload_rgba(1, 1, &[255; 4]);
    let key = mb_render::SamplerKey {
        wrap_s: mb_render::material::WrapKey::Clamp,
        wrap_t: mb_render::material::WrapKey::Clamp,
        max_lod: 0,
        nearest: false,
    };
    s.r.set_shadow_mask(Some((mask, key)));
    assert_eq!(delta(s.r.counters(), before).bind_groups, 4);
    let frame = s.render();
    assert_rgb(frame.at(Vec3::new(0.0, -2.0, 0.0)), RED, "still a mirror");
}

#[test]
fn the_target_follows_the_frame_and_is_counted_apart() {
    target_on(wgpu::Backends::all());
}

// ---- an sRGB frame ------------------------------------------------------------------------------------

fn srgb_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::with_format(backends, wgpu::TextureFormat::Rgba8UnormSrgb) else {
        return;
    };
    s.r.set_mirror_scale(1.0);
    mirror_floor(&mut s.r);
    let colour = [0.5, 0.25, 0.75];
    let model = square(&mut s.r, 1.0, colour);
    s.r.add_object(model, Layer::World, at(0.0, 2.0, 0.0));
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    let frame = s.render();
    // The capture has the frame's format: what comes back is what went in.
    let (own, image) = (
        frame.at(Vec3::new(0.0, 2.0, 0.0)),
        frame.at(Vec3::new(0.0, -2.0, 0.0)),
    );
    assert_rgb(own, colour, "the square");
    assert_rgb(image, colour, "its reflection");
    // The sky above the floor and the sky in the mirror are the same clear colour.
    let (sky, mirrored) = (frame.get(0, 0), frame.at(Vec3::new(5.0, 0.0, 3.0)));
    assert!(!near(&sky, FLOOR, 40.0) && !near(&sky, colour, 40.0));
    assert!(
        (0..3).all(|c| sky[c].abs_diff(mirrored[c]) <= 2),
        "{sky:?} {mirrored:?}"
    );
}

#[test]
fn an_srgb_frame_reflects_the_colours_it_shows() {
    srgb_on(wgpu::Backends::all());
}

// ---- surfaces that are only their reflection ----------------------------------------------------------

fn surface_only_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    let model = floor_model(&mut s.r, [0.0, 1.0, 0.0], 255);
    let floor = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
    let red = square(&mut s.r, 1.0, RED);
    s.r.add_object(red, Layer::World, at(0.0, 2.0, 0.0));
    // Under the floor: seen only when the floor is not drawn.
    let blue = square(&mut s.r, 1.0, BLUE);
    s.r.add_object(blue, Layer::World, at(4.0, -3.0, 0.0));
    let (image, under, empty) = (
        Vec3::new(0.0, -2.0, 0.0),
        Vec3::new(4.0, -3.0, 0.0),
        Vec3::new(-5.0, 0.0, 3.0),
    );
    s.render();
    assert_eq!(s.r.stats.draws, 3);
    let warm = s.r.counters();

    // No mirror (reflections off): such a surface draws nothing at all.
    s.r.set_mirror_surface_only(floor, true);
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 2, "the floor has no draw of its own");
    assert_eq!(s.r.stats.total_draws(), 2);
    assert_eq!(frame.count(FLOOR, 40.0), 0);
    assert_rgb(frame.at(under), BLUE, "nothing hides it");
    assert_rgb(frame.at(empty), CLEAR, "no floor");
    assert_eq!(s.r.counters(), warm);

    // With the mirror: the overlay alone, showing the capture.
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));
    let frame = s.render();
    let stats = s.r.stats;
    assert_eq!((stats.draws, stats.translucent_draws), (2, 0));
    // The capture draws the red square (the blue one is below the plane), the overlay the floor.
    assert_eq!((stats.mirror_draws, stats.mirror_surface_draws), (2, 1));
    assert_eq!(stats.total_draws(), 4);
    assert_rgb(frame.at(image), RED, "the reflection");
    assert_rgb(frame.at(under), CLEAR, "the mirror is in front of it");
    assert_eq!(frame.count(FLOOR, 40.0), 0, "never its own material");
    assert_eq!(frame.count(BLUE, 40.0), 0);

    // Half opacity: over what is behind it, not over a floor colour.
    s.r.set_mirror(Some(Mirror {
        alpha: 0.5,
        ..Mirror::flat(Mat4::IDENTITY)
    }));
    let frame = s.render();
    assert_rgb(frame.at(under), mix(BLUE, CLEAR, 0.5), "blended");
    assert_rgb(frame.at(image), mix(CLEAR, RED, 0.5), "blended");
    s.r.set_mirror(Some(Mirror::flat(Mat4::IDENTITY)));

    // In the sorted pass it has no draw either.
    s.r.set_alpha(floor, 0.5);
    s.render();
    assert_eq!((s.r.stats.draws, s.r.stats.translucent_draws), (2, 0));
    assert_eq!(s.r.stats.mirror_surface_draws, 1);
    s.r.set_alpha(floor, 1.0);

    // Culled like any object: out of view there is no overlay and no capture.
    s.r.set_transform(floor, at(0.0, 0.0, 500.0));
    s.render();
    assert_eq!((s.r.stats.draws, s.r.stats.mirror_draws), (2, 0));
    assert_eq!(s.r.stats.culled_objects, 1);
    s.r.set_transform(floor, Mat4::IDENTITY);

    // An ordinary mirror surface again: its own material under the reflection.
    s.r.set_mirror_surface_only(floor, false);
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 3);
    assert_eq!(
        (s.r.stats.mirror_draws, s.r.stats.mirror_surface_draws),
        (2, 1)
    );
    assert_rgb(frame.at(image), RED, "still a mirror");
    assert_eq!(frame.count(BLUE, 40.0), 0, "the floor hides it");
    // `set_mirror_surface(false)` undoes both.
    s.r.set_mirror_surface_only(floor, true);
    s.r.set_mirror_surface(floor, false);
    let frame = s.render();
    assert_eq!((s.r.stats.draws, s.r.stats.mirror_draws), (3, 0));
    assert_rgb(frame.at(image), FLOOR, "a plain floor");
}

#[test]
fn a_surface_can_be_its_reflection_alone() {
    surface_only_on(wgpu::Backends::all());
}

// ---- many surfaces on one plane -----------------------------------------------------------------------

fn many_surfaces_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends) else {
        return;
    };
    // The plane of an anim group that leans and moves: its +y is not the world's.
    let pose = |i: usize| {
        let t = i as f32 / 20.0;
        at(0.3 * t, -1.0 - 0.5 * t, 0.0)
            * Mat4::from_rotation_z(0.25 + 0.1 * t)
            * Mat4::from_rotation_x(-0.2)
    };
    // 48 tiles of one model that meet edge to edge, every other one drawn by the overlay alone, and
    // four more far outside the view.
    let h = 0.5;
    let corners = [[-h, 0.0, -h], [h, 0.0, -h], [-h, 0.0, h], [h, 0.0, h]];
    let tile = nl_quad(&mut s.r, corners, [0.0, 1.0, 0.0], FLOOR, 255);
    let mut tiles = Vec::new();
    for i in 0..52 {
        let local = if i < 48 {
            at((i % 8) as f32 - 3.5, 0.0, (i / 8) as f32 - 2.5)
        } else {
            at(400.0 + i as f32, 0.0, 0.0)
        };
        let o = s.r.add_object(tile, Layer::World, pose(0) * local);
        if i % 2 == 0 {
            s.r.set_mirror_surface(o, true);
        } else {
            s.r.set_mirror_surface_only(o, true);
        }
        tiles.push((o, local));
    }
    let red = square(&mut s.r, 0.6, RED);
    let local = Vec3::new(0.5, 1.0, 0.0);
    let square = s.r.add_object(red, Layer::World, Mat4::IDENTITY);
    s.r.set_mirror(Some(Mirror::flat(pose(0))));
    s.render();
    let warm = s.r.counters();
    for i in 0..=20 {
        let m = pose(i);
        for (o, local) in &tiles {
            s.r.set_transform(*o, m * *local);
        }
        s.r.set_transform(square, m * Mat4::from_translation(local));
        s.r.set_mirror_plane(m);
        let frame = s.render();
        let stats = s.r.stats;
        // The square and the 24 tiles that have a material; the four far ones are culled.
        assert_eq!((stats.draws, stats.culled_objects), (25, 4), "frame {i}");
        // The capture draws the square; the overlay every tile in view.
        assert_eq!(
            (stats.mirror_draws, stats.mirror_surface_draws),
            (49, 48),
            "frame {i}"
        );
        let own = m.transform_point3(local);
        let image = m.transform_point3(local * Vec3::new(1.0, -1.0, 1.0));
        assert_rgb(frame.at(own), RED, &format!("frame {i}"));
        assert_rgb(frame.at(image), RED, &format!("frame {i}: reflection"));
        assert_eq!(frame.count(FLOOR, 40.0), 0, "frame {i}: all tiles mirror");
    }
    assert_eq!(s.r.counters(), warm, "nothing is created");

    // No surface in view (the camera turns to the sky): no capture pass, no overlay, no uniform of
    // the mirror written, and that is decided anew every frame.
    let toward = s.camera;
    let away = Camera::look_at(
        Vec3::new(0.0, 6.0, 10.0),
        Vec3::new(0.0, 60.0, 0.0),
        Vec3::Z,
    );
    for round in 0..3 {
        s.camera = away;
        s.render();
        let stats = s.r.stats;
        assert_eq!(
            (stats.mirror_draws, stats.mirror_surface_draws, stats.draws),
            (0, 0, 0),
            "round {round}"
        );
        assert_eq!(stats.total_draws(), 0);
        assert_eq!(stats.culled_objects, 53);
        // The plane moves on while nobody sees the mirror: nothing to upload, nothing to redraw.
        s.r.set_mirror_plane(pose(round));
        let size = s.target.size();
        assert!(!s.r.prepare(size, &s.camera), "round {round}");
        assert_eq!(s.r.stats.upload_bytes, 0);
        s.r.set_mirror_plane(pose(20));
        s.camera = toward;
        s.render();
        assert_eq!(s.r.stats.mirror_draws, 49, "round {round}: in view again");
    }
    // Hidden surfaces are no reason for a capture either.
    for (o, _) in &tiles {
        s.r.set_visible(*o, false);
    }
    s.render();
    assert_eq!((s.r.stats.mirror_draws, s.r.stats.draws), (0, 1));
    // One small surface back in view brings the whole capture back.
    s.r.set_visible(tiles[1].0, true);
    s.render();
    assert_eq!(
        (s.r.stats.mirror_draws, s.r.stats.mirror_surface_draws),
        (2, 1)
    );
    assert_eq!(s.r.counters(), warm);
}

#[test]
fn dozens_of_surfaces_share_a_leaning_plane_and_none_in_view_costs_nothing() {
    many_surfaces_on(wgpu::Backends::all());
}

// ---- the object buffer --------------------------------------------------------------------------------

fn reserve_objects_on(backends: wgpu::Backends) {
    let (Some(mut plain), Some(mut s)) = (
        Scene::with_format(backends, UNORM),
        Scene::with_format(backends, UNORM),
    ) else {
        return;
    };
    let fill = |s: &mut Scene, from: usize, to: usize| {
        let model = square(&mut s.r, 0.3, GREEN);
        for i in from..to {
            let (x, y) = ((i % 12) as f32 * 0.8 - 4.4, (i / 12) as f32 * 0.3);
            s.r.add_object(model, Layer::World, at(x, y, 0.0));
        }
    };
    assert_eq!(s.r.object_capacity(), 0);
    let fresh = s.r.counters();
    s.r.reserve_objects(100);
    assert_eq!(
        delta(s.r.counters(), fresh),
        GpuCounters {
            buffers: 1,
            bind_groups: 1,
            ..GpuCounters::default()
        },
        "the buffer and its bind group, at load"
    );
    assert_eq!(s.r.object_capacity(), 128);
    // Requests it already satisfies do nothing.
    let reserved = s.r.counters();
    for n in [0, 1, 100, 128] {
        s.r.reserve_objects(n);
    }
    assert_eq!(s.r.counters(), reserved);

    fill(&mut plain, 0, 100);
    fill(&mut s, 0, 100);
    let loaded = (plain.r.counters(), s.r.counters());
    let (a, b) = (plain.render(), s.render());
    assert_eq!(a.px, b.px);
    assert_eq!(s.r.stats.draws, 100);
    assert_eq!(plain.r.stats.upload_bytes, s.r.stats.upload_bytes);
    let depth_only = GpuCounters {
        textures: 1,
        ..GpuCounters::default()
    };
    assert_eq!(
        delta(s.r.counters(), loaded.1),
        depth_only,
        "the first frame creates the depth buffer and nothing for the objects"
    );
    assert_eq!(
        delta(plain.r.counters(), loaded.0),
        GpuCounters {
            buffers: 1,
            bind_groups: 1,
            ..depth_only
        },
        "without the reservation it creates them"
    );

    // It outlives the scene: the next stage's first frame creates nothing at all.
    s.r.clear_scene();
    assert_eq!(s.r.object_capacity(), 128);
    fill(&mut s, 0, 120);
    let loaded = s.r.counters();
    let frame = s.render();
    assert_eq!(s.r.counters(), loaded);
    assert_eq!(s.r.stats.draws, 120);
    assert_rgb(frame.at(Vec3::new(-4.4, 0.0, 0.0)), GREEN, "first object");
    assert_rgb(frame.at(Vec3::new(4.4, 2.7, 0.0)), GREEN, "last object");

    // More objects than reserved still work: the buffer grows in the frame that needs it, as before.
    fill(&mut s, 120, 150);
    let loaded = s.r.counters();
    s.render();
    assert_eq!(
        delta(s.r.counters(), loaded),
        GpuCounters {
            buffers: 1,
            bind_groups: 1,
            ..GpuCounters::default()
        }
    );
    assert_eq!((s.r.stats.draws, s.r.object_capacity()), (150, 256));

    // Reserving more while a scene is shown replaces the buffer; every object is sent to the new one.
    s.r.reserve_objects(1000);
    assert_eq!(s.r.object_capacity(), 1024);
    let size = s.target.size();
    assert!(s.r.prepare(size, &s.camera), "a new buffer: redraw");
    assert!(s.r.stats.upload_bytes > 0);
    s.r.draw(s.target.view());
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 150);
    assert_rgb(frame.at(Vec3::new(-4.4, 0.0, 0.0)), GREEN, "first object");
    assert_rgb(frame.at(Vec3::new(-0.4, 3.6, 0.0)), GREEN, "last object");
    let settled = s.r.counters();
    s.render();
    assert_eq!(s.r.counters(), settled);
}

#[test]
fn a_reserved_object_buffer_makes_the_first_frame_create_nothing() {
    reserve_objects_on(wgpu::Backends::all());
}

// ---- the browser's path -------------------------------------------------------------------------------

#[test]
fn the_gl_backend_draws_the_same() {
    // WebGL2: the overlay's shader through naga's GLSL ES output (integer masking, `textureLod`), a
    // render target that is sampled, the oblique near plane and the flipped capture.
    let gl = wgpu::Backends::GL;
    flat_mirror_on(gl);
    capture_contents_on(gl);
    alpha_on(gl);
    moving_plane_on(gl);
    tilt_on(gl);
    wavy_on(gl);
    no_mirror_on(gl);
    target_on(gl);
    srgb_on(gl);
    surface_only_on(gl);
    many_surfaces_on(gl);
    reserve_objects_on(gl);
}
