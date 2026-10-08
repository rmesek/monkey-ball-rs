// GX-style material shader for SMB1 models. One uber-shader: the per-material differences (lighting channel,
// vertex colours, up to three texture layers of diffuse / alpha-blend / view-specular kind) are plain
// uniforms, so a pipeline only differs by blend factors, cull mode and which textures it samples.

// Texture fetches the pipeline needs: bits 0..2 = layers 0..2 through their own UVs, bits 3..5 = layers 0..2
// through the sphere map. Set per pipeline, so a one-texture material pays for one fetch, not six.
override SAMPLE_MASK: u32 = 63u;

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
};

struct Object {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    color_mul: vec4<f32>,
    // x: object alpha, y: 1 = apply scene.tilt
    params: vec4<f32>,
};

struct Material {
    mat_color: vec4<f32>,   // rgb: material colour, a: material alpha
    amb_mul: vec4<f32>,     // material ambient (white unless CustomMatAmbColors)
    k0: vec4<f32>,          // specular konst colour
    // x: lit, y: vertex colours, z: layer count,
    // w: 0 = GMA layer pipeline, 1 = NaomiLib untextured, 2..5 = NaomiLib texture combine mode 0..3
    flags: vec4<u32>,
    // layer kinds: 0 diffuse, 1 alpha blend, 2 view specular, 3 world specular (unsupported => skipped)
    layer_kinds: vec4<u32>,
    // x: flip V (NaomiLib)
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> scene: Scene;
@group(1) @binding(0) var<uniform> object: Object;
@group(2) @binding(0) var<uniform> material: Material;
@group(2) @binding(1) var tex0: texture_2d<f32>;
@group(2) @binding(2) var tex1: texture_2d<f32>;
@group(2) @binding(3) var tex2: texture_2d<f32>;
@group(2) @binding(4) var samp0: sampler;
@group(2) @binding(5) var samp1: sampler;
@group(2) @binding(6) var samp2: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv0: vec2<f32>,
    @location(4) uv1: vec2<f32>,
    @location(5) uv2: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) uv1: vec2<f32>,
    @location(3) uv2: vec2<f32>,
    @location(4) view_normal: vec3<f32>,
    @location(5) view_depth: f32,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    var world = object.model * vec4<f32>(in.position, 1.0);
    var n = (object.normal_mat * vec4<f32>(in.normal, 0.0)).xyz;
    if (object.params.y > 0.5) {
        world = scene.tilt * world;
        n = (scene.tilt * vec4<f32>(n, 0.0)).xyz;
    }
    let view_pos = scene.view * world;
    out.clip = scene.proj * view_pos;
    out.view_depth = -view_pos.z;

    let nl = length(n);
    if (nl > 1e-6) {
        n = n / nl;
    } else {
        n = vec3<f32>(0.0, 0.0, 1.0);
    }
    out.view_normal = (scene.view * vec4<f32>(n, 0.0)).xyz;

    // GX rasterised colour (channel 0), computed per vertex like the hardware does.
    let lit = material.flags.x != 0u;
    let vert = material.flags.y != 0u;
    var rgb: vec3<f32>;
    var a: f32;
    if (lit) {
        let diff = max(dot(n, scene.light_dir.xyz), 0.0);
        let light = clamp(scene.ambient.rgb * material.amb_mul.rgb + scene.light_color.rgb * diff,
                          vec3<f32>(0.0), vec3<f32>(1.0));
        if (vert) {
            rgb = light * in.color.rgb;
            a = in.color.a;
        } else {
            rgb = light * material.mat_color.rgb;
            a = material.mat_color.a;
        }
    } else {
        if (vert) {
            rgb = in.color.rgb;
            a = in.color.a;
        } else {
            rgb = material.mat_color.rgb;
            a = material.mat_color.a;
        }
    }
    rgb = rgb * object.color_mul.rgb;
    a = a * object.params.x * object.color_mul.a;
    out.color = vec4<f32>(rgb, a);
    var uv0 = in.uv0;
    if (material.extra.x > 0.5) {
        uv0.y = 1.0 - uv0.y;
    }
    out.uv0 = uv0;
    out.uv1 = in.uv1;
    out.uv2 = in.uv2;
    return out;
}

fn matcap_uv(n: vec3<f32>) -> vec2<f32> {
    let nn = normalize(n);
    return vec2<f32>(nn.x * 0.5 + 0.5, 0.5 - nn.y * 0.5);
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = in.color.rgb;
    var a = in.color.a;
    let count = material.flags.z;
    let naomi = material.flags.w;

    // Textures are sampled up front (uniform control flow: the mask is a pipeline constant); the kinds then
    // pick what to use.
    var s0 = vec4<f32>(1.0);
    var s1 = vec4<f32>(1.0);
    var s2 = vec4<f32>(1.0);
    var m0 = vec4<f32>(1.0);
    var m1 = vec4<f32>(1.0);
    var m2 = vec4<f32>(1.0);
    if ((SAMPLE_MASK & 1u) != 0u) { s0 = textureSample(tex0, samp0, in.uv0); }
    if ((SAMPLE_MASK & 2u) != 0u) { s1 = textureSample(tex1, samp1, in.uv1); }
    if ((SAMPLE_MASK & 4u) != 0u) { s2 = textureSample(tex2, samp2, in.uv2); }
    if ((SAMPLE_MASK & 56u) != 0u) {
        let muv = matcap_uv(in.view_normal);
        if ((SAMPLE_MASK & 8u) != 0u) { m0 = textureSample(tex0, samp0, muv); }
        if ((SAMPLE_MASK & 16u) != 0u) { m1 = textureSample(tex1, samp1, muv); }
        if ((SAMPLE_MASK & 32u) != 0u) { m2 = textureSample(tex2, samp2, muv); }
    }

    if (naomi != 0u) {
        // NaomiLib: single texture, combine mode picked from the mesh flags.
        let ras = in.color;
        if (naomi == 2u) {
            c = s0.rgb;
            a = s0.a;
        } else if (naomi == 3u) {
            c = ras.rgb * s0.rgb;
            a = s0.a;
        } else if (naomi == 4u) {
            c = ras.rgb * (1.0 - s0.a) + s0.rgb * s0.a;
            a = ras.a;
        } else if (naomi == 5u) {
            c = s0.rgb * ras.rgb;
            a = ras.a * s0.a;
        }
    }

    for (var i = 0u; i < 3u; i = i + 1u) {
        if (i >= count || naomi != 0u) { break; }
        var t = s0;
        var m = m0;
        if (i == 1u) { t = s1; m = m1; }
        if (i == 2u) { t = s2; m = m2; }
        var kind = material.layer_kinds.x;
        if (i == 1u) { kind = material.layer_kinds.y; }
        if (i == 2u) { kind = material.layer_kinds.z; }
        if (kind == 0u) {
            // TEV: out = tex * prev (colour and alpha)
            c = c * t.rgb;
            a = a * t.a;
        } else if (kind == 1u) {
            // Swap table 1 routes the texture's R channel into alpha; colour passes through.
            a = a * t.r;
        } else if (kind == 2u) {
            // out = prev + konst0 * tex (sphere-mapped on the view-space normal)
            c = c + material.k0.rgb * m.rgb;
        }
    }

    if (a <= 0.0) {
        discard;
    }
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    if (scene.fog_params.z > 0.5) {
        let f = clamp((in.view_depth - scene.fog_params.x) / max(scene.fog_params.y - scene.fog_params.x, 1e-4), 0.0, 1.0);
        c = mix(c, scene.fog_color.rgb, f);
    }
    if (scene.fog_params.w > 0.5) {
        c = srgb_to_linear(c);
    }
    return vec4<f32>(c, a);
}
