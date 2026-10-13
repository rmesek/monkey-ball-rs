//! Offscreen tests of the material model (specular layers, texture matrices, blend factors), the vertex
//! wave, blended sprites and the creation counters. They skip (pass) when no adapter is available. Every
//! device is requested with the WebGL2 limits; `the_gl_backend_draws_the_same` repeats every scene on the
//! GL backend (the browser's WebGL2 path, GLSL ES through naga) where the machine has one.
//!
//! Geometry and textures are built here; expected colours come from the CPU twins of the shader maths in
//! `mb_render::material` and `mb_render::wave`, which unit tests tie to the reference's matrices.

use mb_gma::{CullMode, Rgba8, material_flags, tev_layer_flags};
use mb_render::glam::{Mat4, Vec3};
use mb_render::material::{model_dir_view, specular_uv, view_specular_rows, world_specular_rows};
use mb_render::{
    Camera, GpuCounters, InstanceStyle, Layer, Lighting, ModelId, OffscreenTarget, Renderer,
    Sprite, SpriteBlend, TextureId, WaveParams,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const CLEAR: [f32; 3] = [0.1, 0.1, 0.1];

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

/// One scene at a time. Devices of several backends created and dropped from many test threads at once
/// now and then deadlock inside the drivers (seen with the GL backend on Windows, about one run in
/// thirty); the product only ever has one device.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Scene {
    r: Renderer,
    target: OffscreenTarget,
    camera: Camera,
    size: u32,
    // Declared last: released after the renderer and its device are gone.
    _gpu: std::sync::MutexGuard<'static, ()>,
}

impl Scene {
    /// A square target of `size` pixels and a camera on +Z looking at the origin; clear colour 0.1 grey.
    fn new(backends: wgpu::Backends, size: u32) -> Option<Self> {
        let gpu = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (device, queue) = device(backends)?;
        let mut r = Renderer::new(device.clone(), queue, FORMAT);
        r.clear_color = [0.1, 0.1, 0.1, 1.0];
        Some(Self {
            target: OffscreenTarget::new(&device, size, size, FORMAT),
            camera: Camera::look_at(Vec3::new(0.0, 0.0, 10.0), Vec3::ZERO, Vec3::Y),
            r,
            size,
            _gpu: gpu,
        })
    }

    fn render(&mut self) -> Frame {
        Frame {
            px: self.r.render_to_rgba(&self.target, &self.camera),
            clip: self
                .camera
                .projection(1.0, self.camera.near, self.camera.far)
                * self.camera.view,
            size: self.size as usize,
        }
    }
}

struct Frame {
    px: Vec<u8>,
    clip: Mat4,
    size: usize,
}

impl Frame {
    fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y.min(self.size - 1) * self.size + x.min(self.size - 1)) * 4;
        self.px[i..i + 4].try_into().unwrap()
    }

    /// The pixel a world-space point lands on.
    fn at(&self, world: Vec3) -> [u8; 4] {
        let ndc = self.clip.project_point3(world);
        let x = ((ndc.x * 0.5 + 0.5) * self.size as f32) as usize;
        let y = ((0.5 - ndc.y * 0.5) * self.size as f32) as usize;
        self.pixel(x, y)
    }
}

#[track_caller]
fn assert_rgb_within(actual: [u8; 4], expected: [f32; 3], tolerance: f32, what: &str) {
    for c in 0..3 {
        let want = (expected[c].clamp(0.0, 1.0) * 255.0).round();
        assert!(
            (actual[c] as f32 - want).abs() <= tolerance,
            "{what}: got {actual:?}, expected about {:?}",
            expected.map(|v| (v.clamp(0.0, 1.0) * 255.0).round())
        );
    }
}

#[track_caller]
fn assert_rgb(actual: [u8; 4], expected: [f32; 3], what: &str) {
    assert_rgb_within(actual, expected, 3.0, what);
}

fn rgba(c: [u8; 3]) -> Rgba8 {
    Rgba8 {
        r: c[0],
        g: c[1],
        b: c[2],
        a: 255,
    }
}

fn layer(flags: u32, tex_idx: i16) -> mb_gma::TevLayer {
    mb_gma::TevLayer {
        // Clamped both ways, top mip level only.
        flags,
        tex_idx,
        lod_bias: 0,
        max_aniso: 0,
        unk_0c: 0,
        unk_10: 0,
        alpha_type: 0,
        color_type: 0,
        swappable: false,
    }
}

/// What a test says about its model: GX quads of position, normal and one UV set, one material.
struct Quads {
    /// Four corners per quad, in GX quad order.
    corners: Vec<[[f32; 3]; 4]>,
    normal: [f32; 3],
    uv: [f32; 2],
    flags: u32,
    material_color: [u8; 3],
    specular_color: [u8; 3],
    alpha: f32,
    blend_factors: u32,
    layers: Vec<mb_gma::TevLayer>,
    bound_radius: f32,
}

impl Quads {
    /// One quad of half side `half` in the XY plane, facing +Z, unlit and white.
    fn facing(half: f32) -> Self {
        Self {
            corners: vec![square_xy(0.0, 0.0, half)],
            normal: [0.0, 0.0, 1.0],
            uv: [0.5, 0.5],
            flags: material_flags::UNLIT,
            material_color: [255; 3],
            specular_color: [0; 3],
            alpha: 1.0,
            blend_factors: 0,
            layers: Vec::new(),
            bound_radius: 1.0,
        }
    }

    fn upload(&self, r: &mut Renderer, textures: &[Option<TextureId>]) -> ModelId {
        let mut data = vec![0x80u8];
        data.extend_from_slice(&((self.corners.len() * 4) as u16).to_be_bytes());
        for quad in &self.corners {
            for corner in quad {
                let values = corner.iter().chain(&self.normal).chain(&self.uv);
                for v in values {
                    data.extend_from_slice(&v.to_be_bytes());
                }
            }
        }
        let mut idxs = [-1i16; 3];
        for (i, slot) in idxs.iter_mut().enumerate().take(self.layers.len()) {
            *slot = i as i16;
        }
        let no_layers = self.layers.is_empty();
        let material = mb_gma::Material {
            flags: self.flags
                | if no_layers {
                    material_flags::SIMPLE_MATERIAL
                } else {
                    0
                },
            material_color: rgba(self.material_color),
            ambient_color: rgba([255; 3]),
            specular_color: rgba(self.specular_color),
            alpha: self.alpha,
            tev_layer_count: self.layers.len() as u8,
            unk_14: 0,
            unk_15: 0,
            tev_layer_idxs: idxs,
            vtx_attrs: (1 << mb_gma::attr::POS)
                | (1 << mb_gma::attr::NRM)
                | (1 << mb_gma::attr::TEX0),
            blend_factors: self.blend_factors,
        };
        let model = mb_gma::Model {
            name: "quads".into(),
            id: 0,
            flags: 0,
            bound_sphere_center: [0.0; 3],
            bound_sphere_radius: self.bound_radius,
            opaque_shape_count: 1,
            translucent_shape_count: 0,
            matrices: Vec::new(),
            tev_layers: self.layers.clone(),
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
        r.upload_model(&model, textures).expect("synthetic model")
    }
}

fn square_xy(x: f32, y: f32, half: f32) -> [[f32; 3]; 4] {
    [
        [x - half, y - half, 0.0],
        [x + half, y - half, 0.0],
        [x + half, y + half, 0.0],
        [x - half, y + half, 0.0],
    ]
}

fn square_xz(x: f32, z: f32, half: f32) -> [[f32; 3]; 4] {
    [
        [x - half, 0.0, z - half],
        [x + half, 0.0, z - half],
        [x + half, 0.0, z + half],
        [x - half, 0.0, z + half],
    ]
}

/// Red, green / blue, yellow.
const TEXELS: [[f32; 3]; 4] = [
    [1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 1.0, 0.0],
];

fn four_texels(r: &mut Renderer) -> TextureId {
    let bytes: Vec<u8> = TEXELS
        .iter()
        .flat_map(|c| [c[0], c[1], c[2], 1.0].map(|v| (v * 255.0) as u8))
        .collect();
    r.upload_rgba(2, 2, &bytes)
}

/// Bilinear sample of the clamped 2x2 texture of [`four_texels`].
fn sample(uv: [f32; 2]) -> [f32; 3] {
    let fx = (uv[0] * 2.0 - 0.5).clamp(0.0, 1.0);
    let fy = (uv[1] * 2.0 - 0.5).clamp(0.0, 1.0);
    std::array::from_fn(|c| {
        let top = TEXELS[0][c] * (1.0 - fx) + TEXELS[1][c] * fx;
        let bottom = TEXELS[2][c] * (1.0 - fx) + TEXELS[3][c] * fx;
        top * (1.0 - fy) + bottom * fy
    })
}

fn at(x: f32, y: f32, z: f32) -> Mat4 {
    Mat4::from_translation(Vec3::new(x, y, z))
}

// ---- specular layers ------------------------------------------------------------------------------

/// Three quads with different normals and places, each with one specular layer of `kind` over a dark
/// material: the frame must show `material + K * texture(uv)` with the reference's tex gen.
fn specular_layer_on(backends: wgpu::Backends, kind: u32) {
    let Some(mut s) = Scene::new(backends, 64) else {
        return;
    };
    let world = kind == tev_layer_flags::TYPE_WORLD_SPECULAR;
    s.r.lighting = Lighting {
        specular_scale: [0.5, 1.0, 0.25],
        ..Lighting::default()
    };
    let texture = four_texels(&mut s.r);
    let base = [51, 51, 51];
    let specular = [255, 128, 0];
    let normals = [
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(0.5, 0.3, 0.8).normalize(),
        Vec3::new(-0.4, -0.5, 0.75).normalize(),
    ];
    let transforms = [
        at(-3.0, 2.0, 0.0),
        at(0.0, -2.0, 0.0) * Mat4::from_rotation_y(0.5),
        // A uniform scale must not change the coordinates: the tex gen normalises.
        at(3.0, 2.0, -1.0) * Mat4::from_rotation_x(-0.4) * Mat4::from_scale(Vec3::splat(1.5)),
    ];
    let before = s.r.counters().pipelines;
    let mut objects = Vec::new();
    for (normal, transform) in normals.into_iter().zip(transforms) {
        let model = Quads {
            normal: normal.to_array(),
            flags: material_flags::UNLIT | material_flags::CUSTOM_MAT_AMB_COLORS,
            material_color: base,
            specular_color: specular,
            layers: vec![layer(kind, 0)],
            bound_radius: 1.25,
            ..Quads::facing(0.8)
        }
        .upload(&mut s.r, &[Some(texture)]);
        objects.push(s.r.add_object(model, Layer::World, transform));
    }
    assert_eq!(
        s.r.counters().pipelines - before,
        2,
        "three specular materials share the blended and the unblended pipeline"
    );

    let light = s
        .camera
        .view
        .transform_vector3(s.r.lighting.direction_to_light());
    let expected: Vec<[f32; 3]> = normals
        .into_iter()
        .zip(transforms)
        .map(|(normal, transform)| {
            let view_from_model = s.camera.view * transform;
            let dir = model_dir_view(&view_from_model, Vec3::ZERO, 1.25);
            let rows = if world {
                world_specular_rows(dir, light)
            } else {
                view_specular_rows(dir)
            };
            let uv = specular_uv(rows, view_from_model.transform_vector3(normal));
            let texel = sample(uv);
            // K0 is the specular colour; K1 is K0 times the light's specular scale.
            let k: [f32; 3] = std::array::from_fn(|c| {
                let k0 = specular[c] as f32 / 255.0;
                if world {
                    k0 * s.r.lighting.specular_scale[c]
                } else {
                    k0
                }
            });
            std::array::from_fn(|c| 0.2 + k[c] * texel[c])
        })
        .collect();
    // The three quads must not all land on one texel, or the test would not see the tex gen.
    assert!(
        expected
            .iter()
            .any(|e| (e[0] - expected[0][0]).abs() > 0.05 || (e[1] - expected[0][1]).abs() > 0.05),
        "{expected:?}"
    );

    let centre = |m: &Mat4| m.transform_point3(Vec3::ZERO);
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 3, "a specular layer costs no draw");
    for (i, transform) in transforms.iter().enumerate() {
        assert_rgb(frame.at(centre(transform)), expected[i], "specular layer");
        // The coordinates are generated per vertex from one normal: flat over the quad.
        let corner = transform.transform_point3(Vec3::new(0.5, -0.5, 0.0));
        assert_rgb(frame.at(corner), expected[i], "corner of the quad");
    }

    // `disableSpecular`: the layer adds nothing; the others keep theirs.
    s.r.set_specular_disabled(objects[0], true);
    let frame = s.render();
    assert_rgb(frame.at(centre(&transforms[0])), [0.2; 3], "disabled");
    assert_rgb(frame.at(centre(&transforms[1])), expected[1], "untouched");

    s.r.set_specular_disabled(objects[0], false);
    let frame = s.render();
    assert_rgb(frame.at(centre(&transforms[0])), expected[0], "back on");
    assert_eq!(
        s.r.counters().pipelines - before,
        2,
        "switches are uniforms"
    );
}

#[test]
fn world_specular_layers_add_the_light_scaled_highlight() {
    specular_layer_on(wgpu::Backends::all(), tev_layer_flags::TYPE_WORLD_SPECULAR);
}

#[test]
fn view_specular_layers_follow_the_direction_to_the_model() {
    specular_layer_on(wgpu::Backends::all(), tev_layer_flags::TYPE_VIEW_SPECULAR);
}

/// Diffuse, then world specular: the specular layer reads the second UV slot's texture and its sum is
/// clamped like a TEV stage.
fn specular_over_diffuse_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 64) else {
        return;
    };
    s.r.lighting = Lighting {
        specular_scale: [1.0; 3],
        ..Lighting::default()
    };
    let tint = s.r.upload_rgba(1, 1, &[255, 128, 64, 255]);
    let texture = four_texels(&mut s.r);
    let normal = Vec3::new(0.2, 0.6, 0.77).normalize();
    let model = Quads {
        normal: normal.to_array(),
        flags: material_flags::UNLIT | material_flags::CUSTOM_MAT_AMB_COLORS,
        material_color: [204; 3],
        layers: vec![layer(0, 0), layer(tev_layer_flags::TYPE_WORLD_SPECULAR, 1)],
        ..Quads::facing(1.0)
    }
    .upload(&mut s.r, &[Some(tint), Some(texture)]);
    s.r.add_object(model, Layer::World, Mat4::IDENTITY);
    let light = s
        .camera
        .view
        .transform_vector3(s.r.lighting.direction_to_light());
    let dir = model_dir_view(&s.camera.view, Vec3::ZERO, 1.0);
    let uv = specular_uv(
        world_specular_rows(dir, light),
        s.camera.view.transform_vector3(normal),
    );
    let texel = sample(uv);
    let diffuse = [0.8, 0.8 * 128.0 / 255.0, 0.8 * 64.0 / 255.0];
    let expected: [f32; 3] = std::array::from_fn(|c| (diffuse[c] + texel[c]).min(1.0));
    let frame = s.render();
    assert_rgb(frame.at(Vec3::ZERO), expected, "diffuse plus specular");
    assert_eq!(s.r.stats.draws, 1);
}

#[test]
fn specular_adds_onto_the_diffuse_layer_before_it() {
    specular_over_diffuse_on(wgpu::Backends::all());
}

// ---- texture matrix -------------------------------------------------------------------------------

fn texture_matrix_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 64) else {
        return;
    };
    let texture = four_texels(&mut s.r);
    // Every vertex reads the centre of the top-left texel, so the quad is one flat colour.
    let quad = |flags: u32, r: &mut Renderer| {
        Quads {
            uv: [0.25, 0.25],
            layers: vec![layer(flags, 0)],
            ..Quads::facing(0.8)
        }
        .upload(r, &[Some(texture)])
    };
    let flagged = quad(tev_layer_flags::ENABLE_UV_SCROLL, &mut s.r);
    let plain = quad(0, &mut s.r);
    let left = s.r.add_object(flagged, Layer::World, at(-2.0, 0.0, 0.0));
    let right = s.r.add_object(plain, Layer::World, at(2.0, 0.0, 0.0));
    let (lp, rp) = (Vec3::new(-2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0));

    // Identity by default.
    let frame = s.render();
    assert_rgb(frame.at(lp), TEXELS[0], "flagged layer, no matrix");
    assert_rgb(frame.at(rp), TEXELS[0], "plain layer, no matrix");
    let draws = s.r.stats.total_draws();
    let created = s.r.counters();

    // Half a texture to the right: the flagged layer moves on to the green texel, the other stays.
    s.r.set_tex_offset(left, [0.5, 0.0]);
    s.r.set_tex_offset(right, [0.5, 0.0]);
    let frame = s.render();
    assert_rgb(frame.at(lp), TEXELS[1], "flagged layer scrolls");
    assert_rgb(frame.at(rp), TEXELS[0], "an unflagged layer keeps TEXMTX2");

    // Down instead.
    s.r.set_tex_offset(left, [0.0, 0.5]);
    assert_rgb(s.render().at(lp), TEXELS[2], "scrolled in t");

    // A full 2x4: swap s and t and shift, (0.25, 0.25) -> (0.75, 0.25) -> green.
    s.r.set_tex_matrix(left, [[0.0, 1.0, 0.25, 0.25], [1.0, 0.0, 0.0, 0.0]]);
    assert_rgb(s.render().at(lp), TEXELS[1], "third and fourth column add");

    // The switch for data without the flag.
    s.r.set_tex_matrix_every_layer(right, true);
    s.r.set_tex_offset(right, [0.5, 0.5]);
    let frame = s.render();
    assert_rgb(frame.at(rp), TEXELS[3], "every layer on request");
    s.r.set_tex_matrix_every_layer(right, false);
    assert_rgb(s.render().at(rp), TEXELS[0], "and off again");

    assert_eq!(s.r.stats.total_draws(), draws, "a uniform, not a draw");
    assert_eq!(s.r.counters(), created, "nothing is created to scroll");
}

#[test]
fn texture_matrix_moves_flagged_layers_only() {
    texture_matrix_on(wgpu::Backends::all());
}

// ---- the vertex wave ------------------------------------------------------------------------------

const FRAMES: f32 = 110.0;

fn wave_positions_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 256) else {
        return;
    };
    s.camera = Camera::look_at(Vec3::new(3.4, 0.0, 9.0), Vec3::new(3.4, 0.0, 0.0), Vec3::Y);
    let wave = WaveParams::BONUS_WAVE;
    let half = 0.12;
    // At frame 110 the front is 80 * 1092 / 16384 = 5.33 units out: the last square is beyond it.
    let xs = [0.6, 1.7, 2.9, 4.1, 6.5];
    let model = Quads {
        corners: xs.iter().map(|x| square_xy(*x, 0.0, half)).collect(),
        material_color: [255, 128, 0],
        bound_radius: 7.0,
        ..Quads::facing(half)
    }
    .upload(&mut s.r, &[]);
    let object = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
    let orange = [1.0, 128.0 / 255.0, 0.0];

    // No wave until asked: every square on the line y = 0.
    s.r.set_wave_time(FRAMES);
    let frame = s.render();
    for x in xs {
        assert_rgb(frame.at(Vec3::new(x, 0.0, 0.0)), orange, "at rest");
    }
    let created = s.r.counters();

    s.r.set_wave(object, Some(wave));
    let frame = s.render();
    assert_eq!(s.r.stats.draws, 1, "the wave is in the vertex shader");
    let mut moved = 0;
    for x in xs {
        // The displaced centre, computed on the CPU with the reference's formula.
        let (centre, _) = wave.displace(Vec3::new(x, 0.0, 0.0), Vec3::Z, FRAMES);
        let dy = centre.y;
        for inside in [-(half - 0.06), 0.0, half - 0.06] {
            assert_rgb(
                frame.at(centre + Vec3::Y * inside),
                orange,
                &format!("square at x = {x}, lifted by {dy}"),
            );
        }
        for outside in [-(half + 0.1), half + 0.1] {
            assert_rgb(
                frame.at(centre + Vec3::Y * outside),
                CLEAR,
                &format!("just outside the square at x = {x}, lifted by {dy}"),
            );
        }
        if dy.abs() > 2.0 * half + 0.1 {
            assert_rgb(frame.at(Vec3::new(x, 0.0, 0.0)), CLEAR, "left its place");
            moved += 1;
        }
    }
    assert!(
        moved >= 2,
        "the chosen frame lifts squares clear of the line"
    );
    let (beyond, _) = wave.displace(Vec3::new(6.5, 0.0, 0.0), Vec3::Z, FRAMES);
    assert_eq!(beyond.y, 0.0, "the front has not reached the last square");

    // The wave runs with the time uniform: another frame, other heights, nothing created.
    let later = FRAMES + 7.0;
    s.r.set_wave_time(later);
    let frame = s.render();
    for x in xs {
        let (centre, _) = wave.displace(Vec3::new(x, 0.0, 0.0), Vec3::Z, later);
        assert_rgb(frame.at(centre), orange, "seven frames later");
    }
    assert_eq!(s.r.counters(), created, "waving creates nothing");

    // Off again.
    s.r.set_wave(object, None);
    let frame = s.render();
    for x in xs {
        assert_rgb(frame.at(Vec3::new(x, 0.0, 0.0)), orange, "back at rest");
    }
}

#[test]
fn wave_lifts_vertices_by_the_reference_formula() {
    wave_positions_on(wgpu::Backends::all());
}

fn wave_normals_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 256) else {
        return;
    };
    // From above, with a light at 45 degrees so the slope of the ripple shows in the shading.
    s.camera = Camera::look_at(
        Vec3::new(2.0, 12.0, 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::NEG_Z,
    );
    s.r.lighting = Lighting {
        ambient: [0.2; 3],
        light_color: [0.7; 3],
        light_rot_x: 8192,
        light_rot_y: 0,
        specular_scale: [1.0; 3],
    };
    let wave = WaveParams::BONUS_WAVE;
    let half = 0.1;
    let places = [
        (0.0, 0.0),
        (1.2, 0.9),
        (2.4, -1.6),
        (3.1, 2.2),
        (0.4, -3.0),
        (4.5, 4.5),
    ];
    let model = Quads {
        corners: places
            .iter()
            .map(|(x, z)| square_xz(*x, *z, half))
            .collect(),
        normal: [0.0, 1.0, 0.0],
        flags: 0,
        bound_radius: 7.0,
        ..Quads::facing(half)
    }
    .upload(&mut s.r, &[]);
    let object = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
    s.r.set_wave(object, Some(wave));
    s.r.set_wave_time(FRAMES);
    let light = s.r.lighting.direction_to_light();
    let frame = s.render();
    let mut tilted = 0;
    for (x, z) in places {
        let (centre, normal) = wave.displace(Vec3::new(x, 0.0, z), Vec3::Y, FRAMES);
        let shade = 0.2 + 0.7 * normal.dot(light).max(0.0);
        // The normal changes a little across a tile; the tolerance is a fifth of what a flat floor
        // would be off by.
        assert_rgb_within(
            frame.at(centre),
            [shade; 3],
            8.0,
            &format!("tile at ({x}, {z}) with normal {normal:?}"),
        );
        tilted += ((normal.dot(light) - Vec3::Y.dot(light)).abs() > 0.15) as usize;
    }
    assert!(tilted >= 2, "the chosen frame tilts tiles enough to tell");
}

#[test]
fn wave_tilts_normals_by_the_reference_formula() {
    wave_normals_on(wgpu::Backends::all());
}

// ---- lighting channel and blend factors -----------------------------------------------------------

#[test]
fn object_colour_scales_material_and_ambient_of_lit_materials() {
    let Some(mut s) = Scene::new(wgpu::Backends::all(), 64) else {
        return;
    };
    // Light straight along the quad's normal: diffuse term one, so the clamp is in play too.
    s.r.lighting = Lighting {
        ambient: [0.4; 3],
        light_color: [0.5; 3],
        light_rot_x: 0,
        light_rot_y: 32768,
        specular_scale: [1.0; 3],
    };
    assert!((s.r.lighting.direction_to_light() - Vec3::Z).length() < 1e-4);
    let model = Quads {
        flags: 0,
        ..Quads::facing(1.0)
    }
    .upload(&mut s.r, &[]);
    let object = s.r.add_object(model, Layer::World, Mat4::IDENTITY);
    assert_rgb(s.render().at(Vec3::ZERO), [0.9; 3], "amb + light");
    // `MAT0 *= colorMul` and `AMB0 *= colorMul`: mat * cm * (amb * cm + light).
    let cm = [0.5, 1.0, 0.25, 1.0];
    s.r.set_color_mul(object, cm);
    let expected: [f32; 3] = std::array::from_fn(|c| cm[c] * (0.4 * cm[c] + 0.5));
    assert_rgb(s.render().at(Vec3::ZERO), expected, "object colour");
}

fn additive_material_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 64) else {
        return;
    };
    // An opaque red wall, and in front of it a `One / One` material whose alpha is one everywhere:
    // it must add to the wall, not replace it.
    let wall = Quads {
        flags: material_flags::UNLIT | material_flags::CUSTOM_MAT_AMB_COLORS,
        material_color: [128, 0, 0],
        ..Quads::facing(2.0)
    }
    .upload(&mut s.r, &[]);
    let glow = |factors: u32, r: &mut Renderer| {
        Quads {
            flags: material_flags::UNLIT
                | material_flags::CUSTOM_MAT_AMB_COLORS
                | material_flags::CUSTOM_BLEND_SRC
                | material_flags::CUSTOM_BLEND_DEST,
            material_color: [0, 102, 51],
            blend_factors: factors,
            ..Quads::facing(0.5)
        }
        .upload(r, &[])
    };
    let one_one = glow(0x11, &mut s.r);
    let alpha_one = glow(0x14, &mut s.r);
    // The explicit default factors are the one case where blending may be skipped.
    let default = glow(0x54, &mut s.r);
    s.r.add_object(wall, Layer::World, at(0.0, 0.0, -1.0));
    s.r.add_object(one_one, Layer::World, at(-1.2, 0.0, 0.0));
    s.r.add_object(alpha_one, Layer::World, at(0.0, 0.0, 0.0));
    s.r.add_object(default, Layer::World, at(1.2, 0.0, 0.0));
    let frame = s.render();
    let red = 128.0 / 255.0;
    let sum = [red, 0.4, 0.2];
    assert_rgb(frame.at(Vec3::new(-1.2, 0.0, 0.0)), sum, "One / One");
    assert_rgb(frame.at(Vec3::new(0.0, 0.0, 0.0)), sum, "SrcAlpha / One");
    assert_rgb(
        frame.at(Vec3::new(1.2, 0.0, 0.0)),
        [0.0, 0.4, 0.2],
        "SrcAlpha / InvSrcAlpha at alpha one",
    );
    assert_rgb(frame.at(Vec3::new(-1.2, 1.2, 0.0)), [red, 0.0, 0.0], "wall");
    assert_eq!(s.r.stats.draws, 4);
}

#[test]
fn additive_materials_blend_even_at_full_alpha() {
    additive_material_on(wgpu::Backends::all());
}

#[test]
fn depth_offset_moves_blended_meshes_back_in_the_sort() {
    let Some(mut s) = Scene::new(wgpu::Backends::all(), 64) else {
        return;
    };
    // Two half-transparent panes at the same depth test (no depth conflict: the far one is behind).
    let pane = |color: [u8; 3], r: &mut Renderer| {
        Quads {
            flags: material_flags::UNLIT | material_flags::CUSTOM_MAT_AMB_COLORS,
            material_color: color,
            alpha: 0.5,
            ..Quads::facing(1.0)
        }
        .upload(r, &[])
    };
    let red = pane([255, 0, 0], &mut s.r);
    let blue = pane([0, 0, 255], &mut s.r);
    let near = s.r.add_object(red, Layer::World, at(0.0, 0.0, 1.0));
    let far = s.r.add_object(blue, Layer::World, at(0.0, 0.0, 0.0));
    s.r.set_alpha(near, 0.999);
    s.r.set_alpha(far, 0.999);
    // Back to front: blue, then red over it.
    let both = |first: [f32; 3], second: [f32; 3]| -> [f32; 3] {
        std::array::from_fn(|c| {
            let under = CLEAR[c] * 0.5 + first[c] * 0.5;
            under * 0.5 + second[c] * 0.5
        })
    };
    let (r, b) = ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    assert_rgb_within(s.render().at(Vec3::ZERO), both(b, r), 4.0, "by distance");
    // The reference's `depthOffset`: the near pane sorts as if it were 5 units farther, so it is drawn
    // first; the far pane then fails the depth test behind it.
    s.r.set_depth_offset(near, 5.0);
    let alone: [f32; 3] = std::array::from_fn(|c| CLEAR[c] * 0.5 + r[c] * 0.5);
    assert_rgb_within(s.render().at(Vec3::ZERO), alone, 4.0, "offset");
    s.r.set_depth_offset(near, 0.0);
    assert_rgb_within(s.render().at(Vec3::ZERO), both(b, r), 4.0, "restored");
}

// ---- blended sprites ------------------------------------------------------------------------------

fn blended_sprites_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 64) else {
        return;
    };
    // Left half opaque teal, right half fully transparent red.
    let texture = s.r.upload_rgba(2, 1, &[0, 102, 204, 255, 255, 0, 0, 0]);
    let other = s.r.upload_rgba(1, 1, &[255, 255, 255, 255]);
    let left = |y: f32| Sprite {
        texture,
        dst: [0.0, y, 16.0, 16.0],
        uv: [0.0, 0.0, 0.5, 1.0],
        color: [1.0, 1.0, 1.0, 0.5],
    };
    let right = |y: f32| Sprite {
        uv: [0.5, 0.0, 1.0, 1.0],
        ..left(y)
    };
    let teal = [0.0, 0.4, 0.8];
    let mix = |over: [f32; 3], a: f32| -> [f32; 3] {
        std::array::from_fn(|c| CLEAR[c] * (1.0 - a) + over[c] * a)
    };

    // The default path, as before: one draw.
    s.r.draw_sprite(left(0.0));
    s.r.draw_sprite(left(16.0));
    let frame = s.render();
    assert_eq!(s.r.stats.sprite_draws, 1);
    assert_rgb(frame.pixel(4, 8), mix(teal, 0.5), "alpha sprite");
    assert_rgb(frame.pixel(40, 40), CLEAR, "no sprite");

    // Blend modes. Alpha and solid share the pipeline and a draw.
    s.r.draw_sprite_blended(left(0.0), SpriteBlend::Alpha);
    s.r.draw_sprite_blended(
        Sprite {
            color: [1.0, 1.0, 0.5, 0.5],
            ..left(16.0)
        },
        SpriteBlend::Solid,
    );
    s.r.draw_sprite_blended(right(32.0), SpriteBlend::Solid);
    let frame = s.render();
    assert_eq!(s.r.stats.sprite_draws, 1, "one run per texture");
    assert_rgb(frame.pixel(4, 8), mix(teal, 0.5), "alpha");
    // The silhouette: the sprite's own colour where the texel is opaque, nothing where it is not.
    assert_rgb(frame.pixel(4, 24), mix([1.0, 1.0, 0.5], 0.5), "solid");
    assert_rgb(frame.pixel(12, 40), CLEAR, "solid over a transparent texel");

    // Order is kept: a sprite of another texture between two of the first makes three runs.
    s.r.draw_sprite(left(0.0));
    s.r.draw_sprite(Sprite::new(other, 32.0, 32.0, 8.0, 8.0));
    s.r.draw_sprite(left(0.0));
    let frame = s.render();
    assert_eq!(s.r.stats.sprite_draws, 3);
    let once = mix(teal, 0.5);
    let twice: [f32; 3] = std::array::from_fn(|c| once[c] * 0.5 + teal[c] * 0.5);
    assert_rgb(frame.pixel(4, 8), twice, "alpha twice");
    assert_rgb(frame.pixel(36, 36), [1.0; 3], "second texture");
}

fn warm_sprites_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 64) else {
        return;
    };
    let hud = [
        s.r.upload_rgba(1, 1, &[255, 0, 0, 255]),
        s.r.upload_rgba(1, 1, &[0, 255, 0, 255]),
    ];
    let cold = s.r.upload_rgba(1, 1, &[0, 0, 255, 255]);
    // A frame without sprites sets up what every frame needs (the depth buffer).
    s.render();
    let before = s.r.counters();
    // Loading: one bind group per texture, and the sprite vertex buffer once.
    assert!(s.r.warm_sprite_texture(hud[0]));
    assert!(s.r.warm_sprite_texture(hud[1]));
    let warm = s.r.counters();
    assert_eq!(warm.bind_groups - before.bind_groups, 2);
    assert_eq!(warm.buffers - before.buffers, 1);
    assert_eq!(warm.total() - before.total(), 3);
    // Warming again is free.
    assert!(s.r.warm_sprite_texture(hud[0]));
    assert_eq!(s.r.counters(), warm);

    // Play: the first draw of each texture, in every blend mode, creates nothing.
    let modes = [SpriteBlend::Alpha, SpriteBlend::Solid];
    for (i, blend) in modes.into_iter().enumerate() {
        s.r.draw_sprite_blended(Sprite::new(hud[i % 2], 0.0, 0.0, 16.0, 16.0), blend);
        s.r.draw_sprite(Sprite::new(hud[(i + 1) % 2], 32.0, 32.0, 16.0, 16.0));
        let frame = s.render();
        assert_eq!(s.r.stats.sprite_draws, 2);
        let other = if i % 2 == 0 {
            [0, 255, 0, 255]
        } else {
            [255, 0, 0, 255]
        };
        assert_eq!(frame.pixel(40, 40), other);
        assert_eq!(s.r.counters(), warm, "{blend:?}");
    }

    // A texture that was not warmed still draws; its bind group is what a warm-up saves.
    s.r.draw_sprite(Sprite::new(cold, 0.0, 0.0, 16.0, 16.0));
    assert_eq!(s.render().pixel(8, 8), [0, 0, 255, 255]);
    assert_eq!(s.r.counters().bind_groups - warm.bind_groups, 1);
    assert_eq!(s.r.counters().buffers, warm.buffers);

    // More sprites than the buffer holds: reserved while loading, not grown in play.
    s.r.reserve_sprites(400);
    let reserved = s.r.counters();
    assert_eq!(reserved.buffers - warm.buffers, 1);
    for i in 0..400 {
        let (x, y) = ((i % 60) as f32, (i / 60) as f32);
        s.r.draw_sprite(Sprite::new(hud[0], x, y, 1.0, 1.0));
    }
    let frame = s.render();
    assert_eq!(frame.pixel(30, 3), [255, 0, 0, 255]);
    assert_eq!(s.r.stats.sprite_draws, 1);
    assert_eq!(s.r.counters(), reserved);

    // Texture ids are reused after a truncate, so the groups go; a missing texture cannot be warmed.
    s.r.truncate_textures(1);
    assert!(!s.r.warm_sprite_texture(hud[0]));
    assert_eq!(s.r.counters(), reserved);
}

#[test]
fn warmed_sprite_textures_create_nothing_at_their_first_draw() {
    warm_sprites_on(wgpu::Backends::all());
}

#[test]
fn sprites_blend_additively_and_as_silhouettes() {
    blended_sprites_on(wgpu::Backends::all());
}

// ---- creation counters ----------------------------------------------------------------------------

fn counters_on(backends: wgpu::Backends) {
    let Some(mut s) = Scene::new(backends, 64) else {
        return;
    };
    // A new renderer has made its fixed set; the first frame adds the depth buffer.
    let start = s.r.counters();
    assert!(start.pipelines >= 1 && start.buffers >= 4 && start.textures == 1);
    assert_eq!(s.r.texture_bytes(), 4, "the white texel");

    // Each kind of upload counts.
    let texture = four_texels(&mut s.r);
    let after = s.r.counters();
    assert_eq!(after.textures - start.textures, 1);
    assert_eq!(s.r.texture_bytes(), 4 + 16);
    let wall = Quads {
        layers: vec![layer(tev_layer_flags::ENABLE_UV_SCROLL, 0)],
        ..Quads::facing(1.0)
    }
    .upload(&mut s.r, &[Some(texture)]);
    let shiny = Quads {
        layers: vec![layer(tev_layer_flags::TYPE_WORLD_SPECULAR, 0)],
        ..Quads::facing(0.5)
    }
    .upload(&mut s.r, &[Some(texture)]);
    let loaded = s.r.counters();
    // Two meshes: a vertex and an index buffer each, plus a material buffer and bind group each.
    assert_eq!(loaded.buffers - after.buffers, 6);
    assert_eq!(loaded.bind_groups - after.bind_groups, 2);
    assert!(loaded.pipelines > after.pipelines);
    // The same model again shares material, sampler and pipelines.
    Quads {
        layers: vec![layer(tev_layer_flags::ENABLE_UV_SCROLL, 0)],
        ..Quads::facing(1.0)
    }
    .upload(&mut s.r, &[Some(texture)]);
    let again = s.r.counters();
    assert_eq!(again.buffers - loaded.buffers, 2, "its two mesh buffers");
    assert_eq!(
        (again.bind_groups, again.pipelines, again.samplers),
        (loaded.bind_groups, loaded.pipelines, loaded.samplers)
    );

    let a = s.r.add_object(wall, Layer::World, at(-1.5, 0.0, 0.0));
    let b = s.r.add_object(shiny, Layer::World, at(1.5, 0.0, 0.0));
    let batch = s.r.create_instance_batch(shiny, InstanceStyle::SORTED);
    s.r.set_wave(a, Some(WaveParams::BONUS_WAVE));
    let play = |r: &mut Renderer, i: usize| {
        let t = i as f32;
        r.set_transform(b, at(1.5, (t * 0.3).sin(), 0.0));
        r.set_tex_offset(a, [t * 0.01, t * 0.02]);
        r.set_wave_time(40.0 + t);
        r.set_specular_disabled(b, i.is_multiple_of(2));
        r.set_alpha(a, 0.5 + 0.4 * (t * 0.2).cos());
        r.push_instance(batch, &at(0.0, 2.0 + t * 0.01, 0.0), [1.0; 4]);
        r.draw_sprite(Sprite::new(texture, 0.0, 0.0, 8.0, 8.0));
        r.draw_sprite_blended(Sprite::new(texture, 16.0, t, 8.0, 8.0), SpriteBlend::Solid);
    };
    // The first frame creates what frames need (depth buffer, object buffer, sprite buffer and group).
    play(&mut s.r, 0);
    s.render();
    let warm = s.r.counters();
    assert!(warm.total() > again.total());
    assert!(s.r.stats.draws >= 3);
    // Sixty frames of play with every per-frame setter in use: nothing more.
    let draws = s.r.stats.total_draws();
    for i in 1..=60 {
        play(&mut s.r, i);
        s.render();
        assert_eq!(s.r.stats.total_draws(), draws, "frame {i}");
    }
    assert_eq!(s.r.counters(), warm, "no GPU object is created during play");
    assert_eq!(
        GpuCounters::default().total(),
        0,
        "the counters start at zero and only count up"
    );

    // Unloading frees textures from the byte count but never lowers a counter.
    s.r.clear_scene();
    s.r.truncate_textures(1);
    assert_eq!(s.r.texture_bytes(), 4);
    assert_eq!(s.r.counters(), warm);
}

#[test]
fn nothing_is_created_during_play_and_the_counters_say_so() {
    counters_on(wgpu::Backends::all());
}

// ---- the browser's path ---------------------------------------------------------------------------

#[test]
fn the_gl_backend_draws_the_same() {
    // WebGL2 goes through wgpu's GL backend and naga's GLSL ES output: the new uniform members, the
    // tex gens, the wave and the sprite blend modes all have to survive that translation.
    let gl = wgpu::Backends::GL;
    specular_layer_on(gl, tev_layer_flags::TYPE_WORLD_SPECULAR);
    specular_layer_on(gl, tev_layer_flags::TYPE_VIEW_SPECULAR);
    specular_over_diffuse_on(gl);
    texture_matrix_on(gl);
    wave_positions_on(gl);
    wave_normals_on(gl);
    additive_material_on(gl);
    blended_sprites_on(gl);
    warm_sprites_on(gl);
    counters_on(gl);
}
