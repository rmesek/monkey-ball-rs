// The mirror overlay (the reference's `createMirrorFlatProgram` / `createMirrorWavyProgram`): a mirror
// surface drawn again over the opaque geometry, showing the capture of the reflected scene at each
// fragment's place in it. Blended by `mirror alpha * vertex alpha`.

// Must stay the `Scene` of shader.wgsl: both read the same uniform buffer.
struct Scene {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    tilt: mat4x4<f32>,      // stage tilt (a rotation about the ball), for objects with params.y set
    light_dir: vec4<f32>,   // world-space unit vector pointing towards the light
    light_color: vec4<f32>,
    ambient: vec4<f32>,
    spec_scale: vec4<f32>,
    fog_color: vec4<f32>,
    // x: fog start, y: fog end, z: fog enabled, w: target is sRGB (convert output to linear)
    fog_params: vec4<f32>,
    // Projected shadow of the ball (the reference's shadow program, see `effects.rs`): world before the tilt
    // -> shadow space, colour and peak opacity, and x: radius, y: z of the caster in shadow space,
    // z: fade per unit behind the caster, w: enabled.
    shadow_from_world: mat4x4<f32>,
    shadow_color: vec4<f32>,
    shadow_info: vec4<f32>,
};

// Must stay the `Object` of shader.wgsl.
struct Object {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    color_mul: vec4<f32>,
    // x: object alpha, y: 1 = apply scene.tilt, z: 1 = the projected shadow falls on it
    params: vec4<f32>,
    // The two rows of GX TEXMTX1 (the reference's `RenderParams.texMtx`): s' = dot(row, (s, t, 1, 1)).
    tex_mtx0: vec4<f32>,
    tex_mtx1: vec4<f32>,
    // xyz: bounding-sphere centre of the model in model space, w: its radius (specular tex gens)
    bound: vec4<f32>,
    // x: view-specular factor, y: world-specular factor, z: 1 = `wave` displaces the vertices,
    // w: 1 = the texture matrix applies to every diffuse / alpha layer, flagged or not
    spec: vec4<f32>,
    // The reference's bonus wave (`BonusWave.ts`): x: amplitude at the centre, y: amplitude per unit of
    // distance, z: angle per unit of distance, w: angle at the centre now (65536 = one turn)
    wave: vec4<f32>,
};

struct MirrorParams {
    // The displayed world (after the stage tilt) -> clip space of the capture.
    clip: mat4x4<f32>,
    // x: mirror alpha, y: 1 = wavy
    params: vec4<f32>,
    // Rows of the indirect matrix of the wavy mirror.
    ind0: vec4<f32>,
    ind1: vec4<f32>,
};

@group(0) @binding(0) var<uniform> scene: Scene;
@group(1) @binding(0) var<uniform> object: Object;
@group(2) @binding(0) var<uniform> mirror: MirrorParams;
@group(2) @binding(1) var capture_tex: texture_2d<f32>;
@group(2) @binding(2) var samp: sampler;
@group(2) @binding(3) var grad_tex: texture_2d<f32>;

// The layout of `Vertex`; the texture coordinates are not used.
struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) mirror_clip: vec4<f32>,
    @location(1) grad_uv: vec2<f32>,
    @location(2) alpha: f32,
};

struct Waved {
    pos: vec3<f32>,
    normal: vec3<f32>,
};

// `BONUS_WAVE_VERTEX_MAIN` of the reference: a ripple running outwards from the model's y axis. Where the
// front has not arrived yet (angle > 0) the vertex and its normal stay as they are.
fn wave(pos: vec3<f32>, normal: vec3<f32>) -> Waved {
    var w: Waved;
    w.pos = pos;
    w.normal = normal;
    let angle_to_rad = 0.00009587379924285257;
    let dist = length(pos.xz);
    let amp = object.wave.x + object.wave.y * dist;
    let angle = object.wave.w + object.wave.z * dist;
    if (angle <= 0.0) {
        let rad = angle * angle_to_rad;
        let s = sin(rad);
        w.pos.y = pos.y + s * amp;
        w.normal = vec3<f32>(0.0, 1.0, 0.0);
        if (dist > 0.0) {
            let dy_dr = cos(rad) * (object.wave.z * angle_to_rad) * amp + s * object.wave.y;
            w.normal = normalize(vec3<f32>(-dy_dr * pos.x / dist, 1.0, -dy_dr * pos.z / dist));
        }
    }
    return w;
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    // The position exactly as `vs_main` and `shade` of shader.wgsl compute it: the surface was drawn
    // there a moment ago and this draw is depth tested against it.
    var w: Waved;
    w.pos = in.position;
    w.normal = in.normal;
    if (object.spec.z > 0.5) {
        w = wave(in.position, in.normal);
    }
    var world = object.model * vec4<f32>(w.pos, 1.0);
    var n = (object.normal_mat * vec4<f32>(w.normal, 0.0)).xyz;
    if (object.params.y > 0.5) {
        world = scene.tilt * world;
        n = (scene.tilt * vec4<f32>(n, 0.0)).xyz;
    }
    let view_pos = scene.view * world;
    out.clip = scene.proj * view_pos;

    if (mirror.params.y > 0.5) {
        // The reference draws the wavy overlay as the flat floor, which lies in the mirror's plane: there
        // the capture's clip position is the fragment's own with x turned round. The waved surface drawn
        // here is not in the plane, so that relation is used directly.
        out.mirror_clip = vec4<f32>(-out.clip.x, out.clip.y, out.clip.z, out.clip.w);
    } else {
        out.mirror_clip = mirror.clip * world;
    }

    // `createMirrorDistortProgram`: the gradient is addressed with x and z of the view-space normal.
    var nv = (scene.view * vec4<f32>(n, 0.0)).xyz;
    let nl = length(nv);
    if (nl > 1e-6) {
        nv = nv / nl;
    } else {
        nv = vec3<f32>(0.0);
    }
    out.grad_uv = nv.xz * 0.5 + vec2<f32>(0.5);
    out.alpha = in.color.a;
    return out;
}

// `IndMask(255 * v, 0xF8) - 128`.
fn ind(v: f32) -> f32 {
    return f32(u32(floor(clamp(v, 0.0, 1.0) * 255.0 + 0.5)) & 0xF8u) - 128.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // As the reference: coordinates with v up the screen.
    var uv = clamp(in.mirror_clip.xy / in.mirror_clip.w * 0.5 + vec2<f32>(0.5), vec2<f32>(0.0), vec2<f32>(1.0));
    if (mirror.params.y > 0.5) {
        let g = textureSampleLevel(grad_tex, samp, clamp(in.grad_uv, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0);
        // The reference reads `.abg` of a distortion target whose alpha is always one.
        let c = vec3<f32>(ind(1.0), ind(g.b), ind(g.g));
        let offset = vec2<f32>(dot(mirror.ind0.xyz, c), dot(mirror.ind1.xyz, c)) * (1.0 / 256.0);
        uv = clamp(uv + offset, vec2<f32>(0.0), vec2<f32>(1.0));
    }
    // The capture is a render target: its first row is the top of the picture.
    let tex = textureSampleLevel(capture_tex, samp, vec2<f32>(uv.x, 1.0 - uv.y), 0.0);
    return vec4<f32>(tex.rgb, mirror.params.x * in.alpha);
}
