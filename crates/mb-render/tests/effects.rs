//! Offscreen tests of instanced models, the world-space quad batch and the projected shadow. They skip
//! (pass) when no adapter is available. Every device is requested with the WebGL2 limits, so anything
//! beyond `downlevel_webgl2_defaults` fails here; `the_gl_backend_draws_the_same` repeats the scene on
//! the GL backend (the browser's WebGL2 path) where the machine has one.

use mb_gma::{material_flags, CullMode, Rgba8};
use mb_render::glam::{Mat4, Vec3};
use mb_render::material::{SamplerKey, WrapKey};
use mb_render::{
    BlendMode, Camera, DrawOrder, InstanceStyle, Layer, Lighting, ModelId, OffscreenTarget, Quad,
    Renderer, Shadow, TextureId,
};

const SIZE: u32 = 64;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

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
    r: Renderer,
    target: OffscreenTarget,
    camera: Camera,
}

impl Scene {
    /// A renderer with a camera on +Z looking at the origin; clear colour 0.1 grey.
    fn new(backends: wgpu::Backends) -> Option<Self> {
        let (device, queue) = device(backends)?;
        let mut r = Renderer::new(device.clone(), queue, FORMAT);
        r.clear_color = [0.1, 0.1, 0.1, 1.0];
        Some(Self {
            target: OffscreenTarget::new(&device, SIZE, SIZE, FORMAT),
            camera: Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y),
            r,
        })
    }

    fn render(&mut self) -> Frame {
        Frame {
            px: self.r.render_to_rgba(&self.target, &self.camera),
            clip: self
                .camera
                .projection(1.0, self.camera.near, self.camera.far)
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
    fn at(&self, world: Vec3) -> [u8; 4] {
        let ndc = self.clip.project_point3(world);
        let x = ((ndc.x * 0.5 + 0.5) * SIZE as f32) as usize;
        let y = ((0.5 - ndc.y * 0.5) * SIZE as f32) as usize;
        let i = (y.min(SIZE as usize - 1) * SIZE as usize + x.min(SIZE as usize - 1)) * 4;
        self.px[i..i + 4].try_into().unwrap()
    }
}

#[track_caller]
fn assert_rgb(actual: [u8; 4], expected: [f32; 3], what: &str) {
    for c in 0..3 {
        let want = (expected[c] * 255.0).round();
        assert!(
            (actual[c] as f32 - want).abs() <= 2.0,
            "{what}: got {actual:?}, expected about {:?}",
            expected.map(|v| (v * 255.0).round())
        );
    }
}

const CLEAR: [f32; 3] = [0.1, 0.1, 0.1];

/// An unlit, untextured NaomiLib square of half side `half` in the XY plane: it draws its material
/// colour times the object or instance colour, without blending.
fn nl_square(r: &mut Renderer, half: f32) -> ModelId {
    let positions = vec![
        [-half, -half, 0.0],
        [half, -half, 0.0],
        [-half, half, 0.0],
        [half, half, 0.0],
    ];
    let mesh = mb_gma::NlMesh {
        flags: 0,
        tex_flags: 0,
        tex_idx: None,
        mesh_type: mb_gma::mesh_type::UNLIT_CONST_MAT_COLOR,
        ambient_color_scale: 1.0,
        material_color: [1.0; 4],
        cull: CullMode::None,
        // LessEqual in the reference's table.
        z_compare: 4,
        depth_write: true,
        vertices: mb_gma::Vertices {
            normals: vec![[0.0, 0.0, 1.0]; 4],
            colors: vec![[255; 4]; 4],
            uv0: vec![[0.0; 2]; 4],
            uv1: vec![[0.0; 2]; 4],
            uv2: vec![[0.0; 2]; 4],
            positions,
        },
        indices: vec![0, 1, 2, 2, 1, 3],
    };
    r.upload_nl_model(
        &mb_gma::NlModel {
            id: 0,
            flags: 0,
            kind: mb_gma::VertexKind::B,
            bound_sphere_center: [0.0; 3],
            bound_sphere_radius: half * 1.5,
            meshes: vec![mesh],
        },
        &[],
    )
}

/// A GMA model of one quad with a normal: `flags` and `alpha` go into its (untextured) material, which
/// blends like every GMA material.
fn gma_quad(
    r: &mut Renderer,
    corners: [[f32; 3]; 4],
    normal: [f32; 3],
    flags: u32,
    color: [u8; 3],
    alpha: f32,
) -> ModelId {
    // One GX quad, vertex format 0 (floats): position and normal per vertex.
    let mut data = vec![0x80u8, 0, 4];
    for corner in corners {
        for v in corner.into_iter().chain(normal) {
            data.extend_from_slice(&v.to_be_bytes());
        }
    }
    let rgba = |c: [u8; 3]| Rgba8 {
        r: c[0],
        g: c[1],
        b: c[2],
        a: 255,
    };
    let material = mb_gma::Material {
        flags: flags | material_flags::SIMPLE_MATERIAL,
        material_color: rgba(color),
        ambient_color: rgba([255; 3]),
        specular_color: rgba([0; 3]),
        alpha,
        tev_layer_count: 0,
        unk_14: 0,
        unk_15: 0,
        tev_layer_idxs: [-1; 3],
        vtx_attrs: (1 << mb_gma::attr::POS) | (1 << mb_gma::attr::NRM),
        blend_factors: 0,
    };
    let model = mb_gma::Model {
        name: "quad".into(),
        id: 0,
        flags: 0,
        bound_sphere_center: [0.0; 3],
        bound_sphere_radius: 10.0,
        opaque_shape_count: 1,
        translucent_shape_count: 0,
        matrices: Vec::new(),
        tev_layers: Vec::new(),
        shapes: vec![mb_gma::Shape {
            material,
            origin: [0.0; 3],
            translucent: false,
            dlists: vec![mb_gma::Dlist {
                data,
                cull: CullMode::None,
            }],
        }],
        skipped_nbt_shapes: 0,
    };
    r.upload_model(&model, &[]).expect("synthetic model")
}

const UNTILTED: InstanceStyle = InstanceStyle {
    tilted: false,
    ..InstanceStyle::OPAQUE
};

fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

#[test]
fn instances_draw_in_one_call_at_their_places_and_colours() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let square = nl_square(&mut s.r, 1.0);
    let batch = s.r.create_instance_batch(square, UNTILTED);
    let colors = [
        [1.0, 0.0, 0.0, 1.0],
        [0.0, 1.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 1.0],
    ];
    let places = [-3.0, 0.0, 3.0];
    for (x, color) in places.into_iter().zip(colors) {
        assert!(s.r.push_instance(batch, &at(x, 0.0, 0.0), color));
    }
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 1, "three instances, one draw");
    assert_eq!(s.r.stats.instance_draws, 1);
    assert_eq!(s.r.stats.instances, 3);
    assert_eq!(s.r.stats.total_draws(), 1);
    for (x, color) in places.into_iter().zip(colors) {
        let rgb = [color[0], color[1], color[2]];
        assert_rgb(
            frame.at(Vec3::new(x, 0.0, 0.0)),
            rgb,
            "centre of an instance",
        );
        assert_rgb(
            frame.at(Vec3::new(x + 0.7, 0.7, 0.0)),
            rgb,
            "corner of an instance",
        );
    }
    assert_rgb(
        frame.at(Vec3::new(1.5, 0.0, 0.0)),
        CLEAR,
        "between instances",
    );
    assert_rgb(frame.at(Vec3::new(0.0, 2.0, 0.0)), CLEAR, "above the row");

    // The queue is per frame: nothing pushed, nothing drawn.
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 0);
    assert_eq!(s.r.stats.instances, 0);
    assert_rgb(frame.at(Vec3::ZERO), CLEAR, "empty frame");
}

#[test]
fn hundreds_of_instances_of_several_models_cost_one_draw_per_model() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let small = nl_square(&mut s.r, 0.1);
    let big = nl_square(&mut s.r, 0.5);
    let small_batch = s.r.create_instance_batch(small, UNTILTED);
    let big_batch = s.r.create_instance_batch(big, UNTILTED);
    let unused = s.r.create_instance_batch(big, UNTILTED);
    // 300 small squares on a grid behind, pushed interleaved with two big ones in front.
    let mut pushed = 0;
    for i in 0..300 {
        let (col, row) = ((i % 20) as f32, (i / 20) as f32);
        let m = at(col * 0.5 - 4.75, row * 0.5 - 3.5, -1.0);
        pushed += s.r.push_instance(small_batch, &m, [1.0, 1.0, 0.0, 1.0]) as usize;
        if i == 100 || i == 200 {
            let x = if i == 100 { -2.0 } else { 2.0 };
            assert!(s
                .r
                .push_instance(big_batch, &at(x, 0.0, 0.0), [0.0, 1.0, 1.0, 1.0]));
        }
    }
    assert_eq!(pushed, 300);
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 2, "one draw per batch that has instances");
    assert_eq!(s.r.stats.instance_draws, 2);
    assert_eq!(s.r.stats.instances, 302);
    // First and last small square, and a big one over the grid.
    assert_rgb(
        frame.at(Vec3::new(-4.75, -3.5, -1.0)),
        [1.0, 1.0, 0.0],
        "first",
    );
    assert_rgb(
        frame.at(Vec3::new(4.75, 3.5, -1.0)),
        [1.0, 1.0, 0.0],
        "last",
    );
    assert_rgb(frame.at(Vec3::new(4.75, 4.2, -1.0)), CLEAR, "past the grid");
    assert_rgb(
        frame.at(Vec3::new(-2.0, 0.0, 0.0)),
        [0.0, 1.0, 1.0],
        "big one",
    );
    assert_rgb(
        frame.at(Vec3::new(2.0, 0.0, 0.0)),
        [0.0, 1.0, 1.0],
        "other big one",
    );

    // The buffer is fixed: what does not fit is refused, and the frame draws the rest.
    let fits = (0..600)
        .filter(|_| s.r.push_instance(unused, &at(0.0, 0.0, 0.0), [1.0; 4]))
        .count();
    assert_eq!(fits, 512);
    s.render();
    assert_eq!(s.r.stats.instances, 512);
    assert_eq!(s.r.stats.draws, 1);
    s.r.reserve_instances(700);
    let fits = (0..800)
        .filter(|_| s.r.push_instance(unused, &at(0.0, 0.0, 0.0), [1.0; 4]))
        .count();
    assert_eq!(fits, 700, "an explicit reserve grows it");
    s.render();
    assert_eq!(s.r.stats.instances, 700);
}

#[test]
fn additive_instances_add_up_and_leave_depth_alone() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let square = nl_square(&mut s.r, 1.0);
    let small = nl_square(&mut s.r, 0.4);
    let untilted = |style| InstanceStyle {
        tilted: false,
        ..style
    };
    // Batches of one order draw in creation order.
    let solid =
        s.r.create_instance_batch(square, untilted(InstanceStyle::SORTED));
    let glow =
        s.r.create_instance_batch(square, untilted(InstanceStyle::GLOW));
    let flash =
        s.r.create_instance_batch(square, untilted(InstanceStyle::FLASH));
    let behind =
        s.r.create_instance_batch(small, untilted(InstanceStyle::SORTED));
    // An ordinary opaque object in front: seen from the camera it covers x -5..-2.5 of the z = 0 plane.
    let wall = s.r.add_object(square, Layer::World, at(-3.0, 0.0, 2.0));
    s.r.set_color_mul(wall, [0.0, 0.0, 1.0, 1.0]);

    // Two glows at half alpha, covering x -3.5..-1.5 and -2.5..-0.5.
    for x in [-2.5, -1.5] {
        s.r.push_instance(glow, &at(x, 0.0, 0.0), [0.5, 0.25, 0.0, 0.5]);
    }
    // A flash on a solid square at the same depth.
    s.r.push_instance(solid, &at(3.0, 3.0, 0.0), [0.0, 0.5, 0.0, 1.0]);
    s.r.push_instance(flash, &at(3.0, 3.0, 0.0), [0.5, 0.0, 0.0, 1.0]);
    // Drawn after the glow and behind it: shows, because the glow wrote no depth.
    s.r.push_instance(behind, &at(-1.5, 0.0, -1.0), [0.0, 0.0, 0.5, 1.0]);
    // Behind a solid square, which did write depth: hidden.
    s.r.push_instance(solid, &at(3.0, -3.0, 0.0), [0.0, 0.5, 0.0, 1.0]);
    s.r.push_instance(behind, &at(3.0, -3.0, -1.0), [1.0, 1.0, 1.0, 1.0]);

    let frame = s.render();
    assert_eq!(s.r.stats.draws, 5, "the wall and four batches");
    assert_eq!(s.r.stats.instance_draws, 4);
    assert_eq!(s.r.stats.instances, 7);
    // SrcAlpha, One: each glow adds colour * 0.5.
    assert_rgb(
        frame.at(Vec3::new(-1.0, 0.7, 0.0)),
        [0.1 + 0.25, 0.1 + 0.125, 0.1],
        "one glow over the clear colour",
    );
    assert_rgb(
        frame.at(Vec3::new(-2.0, 0.6, 0.0)),
        [0.1 + 0.5, 0.1 + 0.25, 0.1],
        "two glows add up",
    );
    assert_rgb(frame.at(Vec3::new(-3.0, 0.0, 2.0)), [0.0, 0.0, 1.0], "wall");
    assert_rgb(
        frame.at(Vec3::new(-3.0, 0.5, 0.0)),
        [0.0, 0.0, 1.0],
        "glow behind the wall is depth tested away",
    );
    assert_rgb(
        frame.at(Vec3::new(-1.5, 0.0, -1.0)),
        [0.0, 0.0, 0.5],
        "a later, farther batch is not rejected by the glow",
    );
    assert_rgb(
        frame.at(Vec3::new(3.0, 3.0, 0.0)),
        [0.5, 0.5, 0.0],
        "flash adds to the square it lies on",
    );
    assert_rgb(
        frame.at(Vec3::new(3.0, -3.0, 0.0)),
        [0.0, 0.5, 0.0],
        "depth-writing batch hides what is behind it",
    );
}

#[test]
fn instanced_and_plain_draws_of_a_lit_model_match() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    s.r.lighting = Lighting {
        ambient: [0.3, 0.3, 0.3],
        light_color: [0.7, 0.6, 0.5],
        // From the camera's side and above.
        light_rot_x: 4000,
        light_rot_y: 32768 + 3000,
        ..Lighting::default()
    };
    let corners = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let lit = gma_quad(&mut s.r, corners, [0.0, 0.0, 1.0], 0, [255, 200, 150], 1.0);
    // Rotation, non-uniform scale and a mirror: everything the normal matrix has to get right.
    let poses = [
        (
            at(-3.0, 2.0, 0.0) * Mat4::from_rotation_y(0.6),
            [1.0, 1.0, 1.0, 1.0],
        ),
        (
            at(0.0, 2.0, 0.0)
                * Mat4::from_rotation_x(-0.7)
                * Mat4::from_scale(Vec3::new(1.2, 0.4, 3.0)),
            [0.5, 1.0, 0.25, 1.0],
        ),
        (
            at(3.0, 2.0, 0.0)
                * Mat4::from_rotation_y(-0.5)
                * Mat4::from_scale(Vec3::new(-1.0, 1.0, 0.02)),
            [1.0, 0.5, 1.0, 1.0],
        ),
        (
            at(0.0, -2.0, 0.0)
                * Mat4::from_rotation_z(0.4)
                * Mat4::from_rotation_x(0.9)
                * Mat4::from_scale(Vec3::splat(0.004))
                * Mat4::from_scale(Vec3::splat(300.0)),
            [1.0, 1.0, 1.0, 1.0],
        ),
    ];
    let tilt = Mat4::from_rotation_z(0.1) * at(0.2, 0.0, 0.0);

    // Plain objects.
    let objects: Vec<_> = poses
        .iter()
        .map(|(m, color)| {
            let o = s.r.add_object(lit, Layer::World, *m);
            s.r.set_color_mul(o, *color);
            s.r.set_tilted(o, true);
            o
        })
        .collect();
    s.r.set_tilt(tilt);
    let plain = s.render();
    assert_eq!(s.r.stats.draws, 4);

    // The same as one instanced draw.
    for o in objects {
        s.r.set_visible(o, false);
    }
    let batch = s.r.create_instance_batch(lit, InstanceStyle::OPAQUE);
    for (m, color) in &poses {
        assert!(s.r.push_instance(batch, m, *color));
    }
    let instanced = s.render();
    assert_eq!(s.r.stats.draws, 1);
    assert_eq!(s.r.stats.instances, 4);

    let lit_pixels = plain.px.chunks(4).filter(|p| p[0] > 60).count();
    assert!(lit_pixels > 200, "the scene shows something: {lit_pixels}");
    let worst = (plain.px.iter().zip(&instanced.px))
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(worst <= 1, "instanced and plain frames differ by {worst}");
}

#[test]
fn instance_alpha_blends_with_the_material_in_the_sorted_place() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let corners = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let white = gma_quad(
        &mut s.r,
        corners,
        [0.0, 0.0, 1.0],
        material_flags::UNLIT,
        [255; 3],
        1.0,
    );
    let batch = s.r.create_instance_batch(
        white,
        InstanceStyle {
            tilted: false,
            ..InstanceStyle::SORTED
        },
    );
    let alpha_override = s.r.create_instance_batch(
        white,
        InstanceStyle {
            blend: Some(BlendMode::Alpha),
            depth_write: Some(false),
            tilted: false,
            ..InstanceStyle::SORTED
        },
    );
    s.r.push_instance(batch, &at(-2.0, 0.0, 0.0), [1.0, 0.0, 0.0, 0.25]);
    s.r.push_instance(alpha_override, &at(2.0, 0.0, 0.0), [0.0, 1.0, 0.0, 0.5]);
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 2);
    // SrcAlpha, OneMinusSrcAlpha over the 0.1 clear colour.
    assert_rgb(
        frame.at(Vec3::new(-2.0, 0.0, 0.0)),
        [0.25 + 0.075, 0.075, 0.075],
        "material blending with the instance alpha",
    );
    assert_rgb(
        frame.at(Vec3::new(2.0, 0.0, 0.0)),
        [0.05, 0.5 + 0.05, 0.05],
        "alpha override",
    );
}

#[test]
fn tilted_batches_follow_the_stage_tilt() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let square = nl_square(&mut s.r, 0.5);
    let tilted = s.r.create_instance_batch(square, InstanceStyle::OPAQUE);
    let fixed = s.r.create_instance_batch(square, UNTILTED);
    s.r.set_tilt(at(0.0, 3.0, 0.0));
    s.r.push_instance(tilted, &at(-2.0, 0.0, 0.0), [1.0, 0.0, 0.0, 1.0]);
    s.r.push_instance(fixed, &at(2.0, 0.0, 0.0), [0.0, 1.0, 0.0, 1.0]);
    let frame = s.render();
    assert_rgb(
        frame.at(Vec3::new(-2.0, 3.0, 0.0)),
        [1.0, 0.0, 0.0],
        "tilted",
    );
    assert_rgb(frame.at(Vec3::new(-2.0, 0.0, 0.0)), CLEAR, "tilted away");
    assert_rgb(
        frame.at(Vec3::new(2.0, 0.0, 0.0)),
        [0.0, 1.0, 0.0],
        "not tilted",
    );
}

fn quad_at(x: f32, y: f32, z: f32, half: f32, color: [u8; 4]) -> Quad {
    Quad::new(
        [
            Vec3::new(x - half, y + half, z),
            Vec3::new(x + half, y + half, z),
            Vec3::new(x - half, y - half, z),
            Vec3::new(x + half, y - half, z),
        ],
        color,
    )
}

/// 2x2 texture: the top row is white, the bottom row half-transparent white.
fn two_row_texture(r: &mut Renderer) -> TextureId {
    let top = [255u8, 255, 255, 255];
    let bottom = [255u8, 255, 255, 128];
    let texels: Vec<u8> = [top, top, bottom, bottom].concat();
    r.upload_rgba(2, 2, &texels)
}

#[test]
fn quads_batch_by_texture_and_blend_and_are_depth_tested() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let white = s.r.white_texture();
    let rows = two_row_texture(&mut s.r);
    s.r.reserve_quads(8);
    s.r.warm_quad_texture(white);
    s.r.warm_quad_texture(rows);
    let square = nl_square(&mut s.r, 1.0);
    // Seen from the camera the wall covers x -5..-2.5 of the z = 0 plane.
    let wall = s.r.add_object(square, Layer::World, at(-3.0, 0.0, 2.0));
    s.r.set_color_mul(wall, [0.0, 0.0, 1.0, 1.0]);

    let (add, alpha, after) = (
        BlendMode::Additive,
        BlendMode::Alpha,
        DrawOrder::AfterSorted,
    );
    // Run 1: two additive quads, the nearer one first; on screen they overlap on x -2.5..-1.67.
    assert!(s.r.push_quad(
        white,
        add,
        after,
        &quad_at(-2.5, 0.0, 1.0, 1.0, [96, 48, 0, 255])
    ));
    assert!(s.r.push_quad(
        white,
        add,
        after,
        &quad_at(-1.5, 0.0, 0.0, 1.0, [96, 48, 0, 255])
    ));
    // Run 2: one alpha quad with per-corner colours left out, half opaque green.
    assert!(s.r.push_quad(
        white,
        alpha,
        after,
        &quad_at(2.0, 2.5, 0.0, 1.0, [0, 255, 0, 128])
    ));
    // Run 3: a textured additive quad; the texture's bottom row halves it.
    assert!(s.r.push_quad(
        rows,
        add,
        after,
        &quad_at(2.0, -2.5, 0.0, 1.0, [255, 0, 0, 255])
    ));
    // Run 4: back to the first state, with a gradient from red (head) to dark (tail).
    let mut streak = quad_at(0.0, 4.0, 0.0, 0.5, [255, 0, 0, 255]);
    streak.colors[2] = [0, 0, 0, 255];
    streak.colors[3] = [0, 0, 0, 255];
    assert!(s.r.push_quad(white, add, after, &streak));

    let frame = s.render();
    assert_eq!(s.r.stats.quad_draws, 4, "one draw per run");
    assert_eq!(s.r.stats.quads, 5);
    assert_eq!(s.r.stats.draws, 1, "the wall");
    assert_eq!(s.r.stats.total_draws(), 5);

    let orange = [96.0 / 255.0, 48.0 / 255.0, 0.0];
    assert_rgb(
        frame.at(Vec3::new(-1.0, 0.6, 0.0)),
        [0.1 + orange[0], 0.1 + orange[1], 0.1],
        "one additive quad",
    );
    // The nearer quad was drawn first: the farther one still adds, so neither wrote depth.
    assert_rgb(
        frame.at(Vec3::new(-2.1, 0.5, 0.0)),
        [0.1 + 2.0 * orange[0], 0.1 + 2.0 * orange[1], 0.1],
        "two additive quads add up",
    );
    assert_rgb(frame.at(Vec3::new(-3.0, 0.0, 2.0)), [0.0, 0.0, 1.0], "wall");
    assert_rgb(
        frame.at(Vec3::new(-3.0, 0.5, 0.0)),
        [0.0, 0.0, 1.0],
        "a quad behind the wall is depth tested away",
    );
    let half = 128.0 / 255.0;
    assert_rgb(
        frame.at(Vec3::new(2.0, 2.5, 0.0)),
        [
            0.1 * (1.0 - half),
            half + 0.1 * (1.0 - half),
            0.1 * (1.0 - half),
        ],
        "alpha quad",
    );
    assert_rgb(
        frame.at(Vec3::new(2.0, -1.7, 0.0)),
        [1.0, 0.1, 0.1],
        "top of the textured quad: opaque texel",
    );
    assert_rgb(
        frame.at(Vec3::new(2.0, -3.3, 0.0)),
        [0.1 + half, 0.1, 0.1],
        "bottom of the textured quad: half-transparent texel",
    );
    let (head, tail) = (
        frame.at(Vec3::new(0.0, 4.4, 0.0)),
        frame.at(Vec3::new(0.0, 3.6, 0.0)),
    );
    assert!(
        head[0] > 200 && tail[0] < 110,
        "gradient: {head:?} {tail:?}"
    );

    // Capacity is fixed: the ninth quad of a frame is refused.
    let fits = (0..20)
        .filter(|_| {
            s.r.push_quad(white, add, after, &quad_at(0.0, 0.0, 0.0, 0.1, [255; 4]))
        })
        .count();
    assert_eq!(fits, 8);
    s.render();
    assert_eq!((s.r.stats.quads, s.r.stats.quad_draws), (8, 1));
}

#[test]
fn quads_and_instances_keep_their_place_around_the_sorted_objects() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let white = s.r.white_texture();
    let corners = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    // A half-transparent white pane in the sorted pass, nearer than the quads.
    let pane_model = gma_quad(
        &mut s.r,
        corners,
        [0.0, 0.0, 1.0],
        material_flags::UNLIT,
        [255; 3],
        1.0,
    );
    let pane = s.r.add_object(
        pane_model,
        Layer::World,
        at(0.0, 0.0, 1.0) * Mat4::from_scale(Vec3::splat(4.0)),
    );
    s.r.set_alpha(pane, 0.5);
    let red = [255, 0, 0, 255];
    let add = BlendMode::Additive;
    s.r.push_quad(
        white,
        add,
        DrawOrder::BeforeSorted,
        &quad_at(-2.0, 0.0, 0.0, 1.0, red),
    );
    s.r.push_quad(
        white,
        add,
        DrawOrder::AfterSorted,
        &quad_at(2.0, 0.0, 0.0, 1.0, red),
    );
    let frame = s.render();
    assert_eq!(s.r.stats.quad_draws, 2, "different order, different run");
    assert_eq!(s.r.stats.translucent_draws, 1);
    // Before: the pane blends over the quad. After: the pane's depth hides the quad behind it.
    assert_rgb(
        frame.at(Vec3::new(-2.0, 0.0, 0.0)),
        [0.5 + 0.5, 0.5 + 0.05, 0.5 + 0.05],
        "quad drawn before the pane shows through it",
    );
    assert_rgb(
        frame.at(Vec3::new(2.0, 0.0, 0.0)),
        [0.55, 0.55, 0.55],
        "quad drawn after the pane and behind it is hidden",
    );
}

/// A white, unlit floor piece in the XZ plane facing `normal_y`, covering `x0..x1` and z -3..3.
fn floor(r: &mut Renderer, x0: f32, x1: f32, normal_y: f32) -> ModelId {
    gma_quad(
        r,
        [
            [x0, 0.0, -3.0],
            [x1, 0.0, -3.0],
            [x1, 0.0, 3.0],
            [x0, 0.0, 3.0],
        ],
        [0.0, normal_y, 0.0],
        material_flags::UNLIT,
        [255; 3],
        1.0,
    )
}

fn mask(r: &mut Renderer, value: u8) -> (TextureId, SamplerKey) {
    let texels: Vec<u8> = (0..16).flat_map(|_| [value, value, value, 255]).collect();
    let key = SamplerKey {
        wrap_s: WrapKey::Clamp,
        wrap_t: WrapKey::Clamp,
        max_lod: 0,
        nearest: false,
    };
    (r.upload_rgba(4, 4, &texels), key)
}

#[test]
fn shadow_darkens_receivers_under_the_ball_and_costs_no_draw() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    // Looking straight down; the light is straight above.
    s.camera = Camera::look_at(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::NEG_Z);
    s.r.lighting = Lighting {
        light_rot_x: 16384,
        light_rot_y: 0,
        ..Lighting::default()
    };
    let left = floor(&mut s.r, -4.0, 0.0, 1.0);
    let right = floor(&mut s.r, 0.0, 4.0, 1.0);
    let receiver = s.r.add_object(left, Layer::World, Mat4::IDENTITY);
    let other = s.r.add_object(right, Layer::World, Mat4::IDENTITY);
    s.r.set_shadow_receiver(receiver, true);
    let ball = Vec3::new(0.0, 0.5, 0.0);
    let shadow = Shadow::projected(ball, 0.5, &s.r.lighting).unwrap();
    let white = [1.0, 1.0, 1.0];
    let under = Vec3::new(-0.35, 0.0, 0.2);
    let under_other = Vec3::new(0.35, 0.0, 0.2);
    let beside = Vec3::new(-1.2, 0.0, 0.0);

    // No mask, no shadow.
    s.r.set_shadow(Some(shadow));
    let frame = s.render();
    let baseline = s.r.stats;
    assert_eq!(baseline.draws, 2);
    assert_rgb(frame.at(under), white, "no mask yet");

    let full = mask(&mut s.r, 255);
    s.r.set_shadow_mask(Some(full));
    s.r.set_shadow(Some(shadow));
    let frame = s.render();
    assert_eq!(s.r.stats.draws, baseline.draws, "the shadow adds no draw");
    assert_eq!(s.r.stats.total_draws(), baseline.total_draws());
    // 0.4 opacity, less 0.2 per unit behind the ball's centre (0.5 here).
    let expect = 1.0 - shadow.coverage(under, Vec3::Y, 1.0);
    assert!((expect - 0.64).abs() < 1e-4);
    assert_rgb(frame.at(under), [expect; 3], "receiver under the ball");
    assert_rgb(frame.at(under_other), white, "not a receiver");
    assert_rgb(
        frame.at(beside),
        white,
        "receiver outside the projected square",
    );
    // The square is 1.4 * radius to each side.
    assert_rgb(
        frame.at(Vec3::new(-0.6, 0.0, -0.6)),
        [expect; 3],
        "inside the corner",
    );
    assert_rgb(frame.at(Vec3::new(-0.8, 0.0, 0.0)), white, "just outside");

    // The mask scales it.
    let grey = mask(&mut s.r, 128);
    s.r.set_shadow_mask(Some(grey));
    let frame = s.render();
    let expect = 1.0 - shadow.coverage(under, Vec3::Y, 128.0 / 255.0);
    assert_rgb(frame.at(under), [expect; 3], "half mask");
    s.r.set_shadow_mask(Some(full));

    // It follows the ball, and fades with the distance behind it.
    let high = Shadow::projected(Vec3::new(-2.0, 3.0, 1.0), 0.5, &s.r.lighting).unwrap();
    s.r.set_shadow(Some(high));
    let frame = s.render();
    let p = Vec3::new(-2.0, 0.0, 1.0);
    let expect = 1.0 - high.coverage(p, Vec3::Y, 1.0);
    assert!((expect - 0.84).abs() < 1e-4, "{expect}");
    assert_rgb(frame.at(p), [expect; 3], "three units below the ball");
    assert_rgb(frame.at(under), white, "the old place is clear");

    // Switched off for a frame: gone, same draws.
    s.r.set_shadow(None);
    let frame = s.render();
    assert_eq!(s.r.stats.draws, baseline.draws);
    assert_rgb(frame.at(under), white, "disabled");

    // Switching the receiver flag moves it to the other piece.
    s.r.set_shadow(Some(shadow));
    s.r.set_shadow_receiver(receiver, false);
    s.r.set_shadow_receiver(other, true);
    let frame = s.render();
    assert_rgb(frame.at(under), white, "no longer a receiver");
    assert_rgb(frame.at(under_other), [0.64; 3], "now a receiver");
}

#[test]
fn shadow_skips_faces_turned_from_the_light_and_stays_in_stage_space() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    s.camera = Camera::look_at(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::NEG_Z);
    s.r.lighting = Lighting {
        light_rot_x: 16384,
        light_rot_y: 0,
        ..Lighting::default()
    };
    let facing_down = floor(&mut s.r, -4.0, 0.0, -1.0);
    let no_normal = floor(&mut s.r, 0.0, 4.0, 0.0);
    let a = s.r.add_object(facing_down, Layer::World, Mat4::IDENTITY);
    let b = s.r.add_object(no_normal, Layer::World, Mat4::IDENTITY);
    for o in [a, b] {
        s.r.set_shadow_receiver(o, true);
        s.r.set_tilted(o, true);
    }
    let full = mask(&mut s.r, 255);
    s.r.set_shadow_mask(Some(full));
    let shadow = Shadow::projected(Vec3::new(0.0, 0.5, 0.0), 0.5, &s.r.lighting).unwrap();
    s.r.set_shadow(Some(shadow));
    let frame = s.render();
    assert_rgb(
        frame.at(Vec3::new(-0.35, 0.0, 0.0)),
        [1.0; 3],
        "a face turned away from the light takes no shadow",
    );
    assert_rgb(
        frame.at(Vec3::new(0.35, 0.0, 0.0)),
        [0.64; 3],
        "a vertex without a normal is not tested",
    );

    // The tilt moves the stage on screen; the shadow is cast before it and moves along.
    s.r.set_tilt(at(0.0, 0.0, 2.0));
    let frame = s.render();
    assert_rgb(
        frame.at(Vec3::new(0.35, 0.0, 2.0)),
        [0.64; 3],
        "moved with the stage",
    );
    assert_rgb(
        frame.at(Vec3::new(0.35, 0.0, 0.0)),
        [1.0; 3],
        "not left behind",
    );
}

#[test]
fn a_frame_without_effects_draws_what_it_did_before() {
    let Some(mut s) = Scene::new(wgpu::Backends::all()) else {
        return;
    };
    let square = nl_square(&mut s.r, 0.5);
    let corners = [
        [-1.0, -1.0, 0.0],
        [1.0, -1.0, 0.0],
        [1.0, 1.0, 0.0],
        [-1.0, 1.0, 0.0],
    ];
    let pane = gma_quad(&mut s.r, corners, [0.0, 0.0, 1.0], 0, [255; 3], 0.5);
    for i in 0..5 {
        s.r.add_object(square, Layer::World, at(i as f32 - 2.0, 2.0, 0.0));
    }
    s.r.add_object(square, Layer::Background, at(0.0, -3.0, 0.0));
    s.r.add_object(pane, Layer::World, at(0.0, -1.0, 0.0));
    s.render();
    let before = s.r.stats;
    // One draw per mesh of each visible object, as ever.
    assert_eq!(before.draws, 7);
    assert_eq!(
        (
            before.instance_draws,
            before.quad_draws,
            before.sprite_draws
        ),
        (0, 0, 0)
    );
    assert_eq!(before.total_draws(), 7);

    // Everything set up for effects, nothing pushed and the shadow off: the same draws.
    let batch = s.r.create_instance_batch(square, InstanceStyle::GLOW);
    let confetti = s.r.create_instance_batch(pane, InstanceStyle::OPAQUE);
    s.r.reserve_instances(1024);
    s.r.reserve_quads(256);
    let white = s.r.white_texture();
    s.r.warm_quad_texture(white);
    let full = mask(&mut s.r, 255);
    s.r.set_shadow_mask(Some(full));
    s.r.set_shadow(None);
    s.render();
    let idle = s.r.stats;
    assert_eq!(idle.draws, before.draws);
    assert_eq!(idle.translucent_draws, before.translucent_draws);
    assert_eq!(idle.total_draws(), before.total_draws());
    assert_eq!((idle.instances, idle.quads), (0, 0));

    // With all three at once: one draw per batch mesh and per quad run on top, none for the shadow.
    let shadow = Shadow::projected(Vec3::Y, 0.5, &s.r.lighting);
    let push = |r: &mut Renderer| {
        r.set_shadow(shadow);
        for i in 0..40 {
            r.push_instance(batch, &at(i as f32 * 0.1, 0.0, 0.0), [0.1, 0.1, 0.1, 1.0]);
            r.push_instance(confetti, &at(0.0, i as f32 * 0.1, 0.0), [1.0; 4]);
            let q = quad_at(i as f32 * 0.1, 1.0, 0.0, 0.1, [255; 4]);
            r.push_quad(white, BlendMode::Additive, DrawOrder::AfterSorted, &q);
        }
    };
    push(&mut s.r);
    s.render();
    let busy = s.r.stats;
    assert_eq!(busy.draws, before.draws + 2);
    assert_eq!(busy.instance_draws, 2);
    assert_eq!(busy.quad_draws, 1);
    assert_eq!(busy.total_draws(), before.total_draws() + 3);
    assert_eq!((busy.instances, busy.quads), (80, 40));
    assert_eq!(busy.translucent_draws, before.translucent_draws);

    // The same particles again upload nothing and leave the frame unchanged (a paused game).
    push(&mut s.r);
    let size = s.target.size();
    assert!(!s.r.prepare(size, &s.camera), "identical frame");
    assert_eq!(s.r.stats.upload_bytes, 0);
    assert_eq!(s.r.stats.total_draws(), busy.total_draws());
    // A frame that is updated but not drawn drops its queue.
    push(&mut s.r);
    s.r.discard_sprites();
    assert!(
        s.r.prepare(size, &s.camera),
        "the particles are gone: redraw"
    );
    s.r.draw(s.target.view());
    assert_eq!(s.r.stats.total_draws(), before.total_draws());

    // Unloading the scene drops the batches; pushing to a stale one is refused.
    s.r.clear_scene();
    assert!(!s.r.push_instance(batch, &Mat4::IDENTITY, [1.0; 4]));
    s.render();
    assert_eq!(s.r.stats.total_draws(), 0);
}

#[test]
fn the_gl_backend_draws_the_same() {
    // The browser's WebGL2 path goes through wgpu's GL backend: instancing without a base instance,
    // `textureGrad` for the shadow mask.
    let Some(mut s) = Scene::new(wgpu::Backends::GL) else {
        return;
    };
    s.camera = Camera::look_at(Vec3::new(0.0, 10.0, 0.0), Vec3::ZERO, Vec3::NEG_Z);
    s.r.lighting = Lighting {
        light_rot_x: 16384,
        light_rot_y: 0,
        ..Lighting::default()
    };
    let ground = floor(&mut s.r, -4.0, 4.0, 1.0);
    let receiver = s.r.add_object(ground, Layer::World, Mat4::IDENTITY);
    s.r.set_shadow_receiver(receiver, true);
    let full = mask(&mut s.r, 255);
    s.r.set_shadow_mask(Some(full));
    let shadow = Shadow::projected(Vec3::new(0.0, 0.5, 0.0), 0.5, &s.r.lighting);
    s.r.set_shadow(shadow);

    // Squares lying on the ground, facing up, in two batches so the second starts mid-buffer.
    let square = nl_square(&mut s.r, 0.4);
    let lay = Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let first = s.r.create_instance_batch(square, UNTILTED);
    let second = s.r.create_instance_batch(square, UNTILTED);
    for x in [-3.0, -2.0] {
        s.r.push_instance(first, &(at(x, 0.1, -2.0) * lay), [1.0, 0.0, 0.0, 1.0]);
    }
    for x in [2.0, 3.0] {
        s.r.push_instance(second, &(at(x, 0.1, -2.0) * lay), [0.0, 0.0, 1.0, 1.0]);
    }
    let white = s.r.white_texture();
    let q = Quad::new(
        [
            Vec3::new(-0.5, 0.1, 1.5),
            Vec3::new(0.5, 0.1, 1.5),
            Vec3::new(-0.5, 0.1, 2.5),
            Vec3::new(0.5, 0.1, 2.5),
        ],
        [0, 64, 0, 255],
    );
    s.r.push_quad(white, BlendMode::Additive, DrawOrder::AfterSorted, &q);
    s.r.push_quad(white, BlendMode::Additive, DrawOrder::AfterSorted, &q);

    let frame = s.render();
    assert_eq!(s.r.stats.draws, 3);
    assert_eq!(s.r.stats.quad_draws, 1);
    for x in [-3.0, -2.0] {
        assert_rgb(
            frame.at(Vec3::new(x, 0.1, -2.0)),
            [1.0, 0.0, 0.0],
            "first batch",
        );
    }
    for x in [2.0, 3.0] {
        assert_rgb(
            frame.at(Vec3::new(x, 0.1, -2.0)),
            [0.0, 0.0, 1.0],
            "second batch",
        );
    }
    assert_rgb(frame.at(Vec3::new(0.0, 0.0, -2.0)), [1.0; 3], "ground");
    assert_rgb(frame.at(Vec3::new(0.2, 0.0, 0.2)), [0.64; 3], "shadow");
    // White ground saturates; check the quads on their own by hiding the ground.
    s.r.set_visible(receiver, false);
    s.r.push_quad(white, BlendMode::Additive, DrawOrder::AfterSorted, &q);
    s.r.push_quad(white, BlendMode::Additive, DrawOrder::AfterSorted, &q);
    let frame = s.render();
    assert_rgb(
        frame.at(Vec3::new(0.0, 0.1, 2.0)),
        [0.1, 0.1 + 2.0 * 64.0 / 255.0, 0.1],
        "two additive quads",
    );
}
