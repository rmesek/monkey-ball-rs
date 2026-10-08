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
    // Projected shadow of the ball (the reference's shadow program, see `effects.rs`): world before the tilt
    // -> shadow space, colour and peak opacity, and x: radius, y: z of the caster in shadow space,
    // z: fade per unit behind the caster, w: enabled.
    shadow_from_world: mat4x4<f32>,
    shadow_color: vec4<f32>,
    shadow_info: vec4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    normal_mat: mat4x4<f32>,
    color_mul: vec4<f32>,
    // x: object alpha, y: 1 = apply scene.tilt, z: 1 = the projected shadow falls on it
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
@group(0) @binding(1) var shadow_tex: texture_2d<f32>;
@group(0) @binding(2) var shadow_samp: sampler;
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

// One instance of an instanced draw: the rows of its affine model matrix and a colour multiplier.
struct InstIn {
    @location(6) row0: vec4<f32>,
    @location(7) row1: vec4<f32>,
    @location(8) row2: vec4<f32>,
    @location(9) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) uv1: vec2<f32>,
    @location(3) uv2: vec2<f32>,
    @location(4) view_normal: vec3<f32>,
    @location(5) view_depth: f32,
    @location(6) shadow_pos: vec3<f32>,
    // x: z of the unit normal in shadow space, y: the vertex has a normal, z: the shadow applies here
    @location(7) shadow_n: vec3<f32>,
};

// Everything after the model transform. `params` is `Object.params`.
fn shade(in: VsIn, world_in: vec4<f32>, n_in: vec3<f32>, color_mul: vec4<f32>, params: vec4<f32>) -> VsOut {
    var out: VsOut;
    var world = world_in;
    var n = n_in;
    // The shadow is cast in stage space, before the tilt (the ball's own position does not tilt).
    out.shadow_pos = vec3<f32>(0.0);
    out.shadow_n = vec3<f32>(0.0);
    if (params.z > 0.5 && scene.shadow_info.w > 0.5) {
        out.shadow_pos = (scene.shadow_from_world * world).xyz;
        let sn = (scene.shadow_from_world * vec4<f32>(n, 0.0)).xyz;
        let sl = length(sn);
        if (sl > 1e-5) {
            out.shadow_n = vec3<f32>(sn.z / sl, 1.0, 1.0);
        } else {
            out.shadow_n = vec3<f32>(0.0, 0.0, 1.0);
        }
    }
    if (params.y > 0.5) {
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
    rgb = rgb * color_mul.rgb;
    a = a * params.x * color_mul.a;
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

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = object.model * vec4<f32>(in.position, 1.0);
    let n = (object.normal_mat * vec4<f32>(in.normal, 0.0)).xyz;
    return shade(in, world, n, object.color_mul, object.params);
}

// Instanced draws take the model matrix and the colour multiplier from the instance; the bound object only
// says whether the stage tilt applies.
@vertex
fn vs_instanced(in: VsIn, inst: InstIn) -> VsOut {
    let p = vec4<f32>(in.position, 1.0);
    let world = vec4<f32>(dot(inst.row0, p), dot(inst.row1, p), dot(inst.row2, p), 1.0);
    // Normal matrix without an inverse: the cofactors of the upper 3x3 (its inverse transpose times the
    // determinant), with the sign put back for mirrored instances.
    let c0 = vec3<f32>(inst.row0.x, inst.row1.x, inst.row2.x);
    let c1 = vec3<f32>(inst.row0.y, inst.row1.y, inst.row2.y);
    let c2 = vec3<f32>(inst.row0.z, inst.row1.z, inst.row2.z);
    let k0 = cross(c1, c2);
    var n = k0 * in.normal.x + cross(c2, c0) * in.normal.y + cross(c0, c1) * in.normal.z;
    if (dot(c0, k0) < 0.0) {
        n = -n;
    }
    // Small instances have tiny cofactors; `shade` takes anything shorter than 1e-6 for no normal.
    let nl = length(n);
    if (nl > 0.0) {
        n = n / nl;
    }
    return shade(in, world, n, inst.color, vec4<f32>(1.0, object.params.y, 0.0, 0.0));
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

    // Derivatives belong to uniform control flow; the shadow mask itself is fetched only under the ball.
    let shadow_uv = in.shadow_pos.xy / (scene.shadow_info.x * 2.0) + vec2<f32>(0.5);
    let shadow_dx = dpdx(shadow_uv);
    let shadow_dy = dpdy(shadow_uv);

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
    // The reference draws the receivers a second time with its shadow program, blended over the finished
    // (fogged) frame; the same maths here costs no draw.
    if (in.shadow_n.z > 0.5) {
        let inside = shadow_uv.x >= 0.0 && shadow_uv.x <= 1.0 && shadow_uv.y >= 0.0 && shadow_uv.y <= 1.0;
        // Faces turned away from the light take no shadow; vertices without a normal are not tested.
        let facing = in.shadow_n.y <= 0.5 || in.shadow_n.x > 0.0;
        if (inside && facing) {
            let mask = textureSampleGrad(shadow_tex, shadow_samp, shadow_uv, shadow_dx, shadow_dy);
            let behind = max(scene.shadow_info.y - in.shadow_pos.z, 0.0);
            let fade = clamp(1.0 - behind * scene.shadow_info.z, 0.0, 1.0);
            let shade_a = max(max(mask.r, mask.g), mask.b) * scene.shadow_color.a * fade;
            if (shade_a > 0.0) {
                c = mix(c, scene.shadow_color.rgb, shade_a);
            }
        }
    }
    if (scene.fog_params.w > 0.5) {
        c = srgb_to_linear(c);
    }
    return vec4<f32>(c, a);
}
