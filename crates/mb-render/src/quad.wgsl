// World-space textured quads (streaks, sparkles): the reference's streak program. Vertex colour times
// texel, no lighting and no fog.

// Must stay the `Scene` of shader.wgsl: both read the same uniform buffer.
struct Scene {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    tilt: mat4x4<f32>,
    light_dir: vec4<f32>,
    light_color: vec4<f32>,
    ambient: vec4<f32>,
    spec_scale: vec4<f32>,
    fog_color: vec4<f32>,
    // w: target is sRGB (convert output to linear)
    fog_params: vec4<f32>,
    shadow_from_world: mat4x4<f32>,
    shadow_color: vec4<f32>,
    shadow_info: vec4<f32>,
};

@group(0) @binding(0) var<uniform> scene: Scene;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = scene.proj * (scene.view * vec4<f32>(in.pos, 1.0));
    out.color = in.color;
    out.uv = in.uv;
    return out;
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = in.color * textureSample(tex, samp, in.uv);
    if (scene.fog_params.w > 0.5) {
        c = vec4<f32>(srgb_to_linear(c.rgb), c.a);
    }
    return c;
}
