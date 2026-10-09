// GX-style material shader for SMB1 models. One uber-shader: the per-material differences (lighting channel,
// vertex colours, up to three texture layers of diffuse / alpha-blend / view-specular / world-specular kind)
// are plain uniforms, so a pipeline only differs by blend factors, cull mode and which textures it samples.
// Texture coordinates are generated per vertex, as GX does: a layer's own UV set, optionally through the
// object's texture matrix, or a sphere map on the normal for the two specular kinds.

// Texture fetches the pipeline needs: bit i = layer i. Set per pipeline, so a one-texture material pays for
// one fetch, not three.
override SAMPLE_MASK: u32 = 7u;

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

struct Material {
    mat_color: vec4<f32>,   // rgb: material colour, a: material alpha
    amb_mul: vec4<f32>,     // material ambient (white unless CustomMatAmbColors)
    k0: vec4<f32>,          // specular konst colour
    // x: lit, y: vertex colours, z: layer count,
    // w: 0 = GMA layer pipeline, 1 = NaomiLib untextured, 2..5 = NaomiLib texture combine mode 0..3
    flags: vec4<u32>,
    // xyz: layer kinds: 0 diffuse, 1 alpha blend, 2 view specular, 3 world specular
    // w: bits 0..2 = layer takes the object's texture matrix, bit 4 / 5 = a view / world specular layer exists
    layer_kinds: vec4<u32>,
    // x: V of NaomiLib meshes: 1 = `1 - v`, 2 = `-v`
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
    @location(4) view_depth: f32,
    @location(5) shadow_pos: vec3<f32>,
    // x: z of the unit normal in shadow space, y: the vertex has a normal, z: the shadow applies here
    @location(6) shadow_n: vec3<f32>,
};

// The x and y rows of gl-matrix `lookAt(origin, aim, up)`, which is all the reference's specular post
// matrices keep (`applySpecularPostTransform`). `material::look_rows` is the same on the CPU.
struct Rows {
    x: vec3<f32>,
    y: vec3<f32>,
};

fn look_rows(aim: vec3<f32>, up: vec3<f32>) -> Rows {
    var r: Rows;
    r.x = vec3<f32>(1.0, 0.0, 0.0);
    r.y = vec3<f32>(0.0, 1.0, 0.0);
    let len = length(aim);
    if (len > 1e-6) {
        let z = -aim / len;
        var x = cross(up, z);
        let xl = length(x);
        if (xl > 0.0) { x = x / xl; } else { x = vec3<f32>(0.0); }
        var y = cross(z, x);
        let yl = length(y);
        if (yl > 0.0) { y = y / yl; } else { y = vec3<f32>(0.0); }
        r.x = x;
        r.y = y;
    }
    return r;
}

// s = 0.5 + 0.5 * x.n, t = 0.5 - 0.5 * y.n (the post matrix halves the rows, flips t and adds a half).
fn spec_uv(r: Rows, n: vec3<f32>) -> vec2<f32> {
    return vec2<f32>(0.5 + 0.5 * dot(r.x, n), 0.5 - 0.5 * dot(r.y, n));
}

// GX tex gen of one layer: its UV set, through TEXMTX1 when asked, or the sphere map of its specular kind.
fn tex_gen(uv: vec2<f32>, kind: u32, scroll: bool, view_spec: vec2<f32>, world_spec: vec2<f32>) -> vec2<f32> {
    if (kind == 2u) { return view_spec; }
    if (kind == 3u) { return world_spec; }
    if (scroll) {
        let v = vec4<f32>(uv, 1.0, 1.0);
        return vec2<f32>(dot(object.tex_mtx0, v), dot(object.tex_mtx1, v));
    }
    return uv;
}

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

// Everything after the model transform. `params` is `Object.params`; `model` and `n_model` (the normal in
// model space) feed the specular tex gens only.
fn shade(in: VsIn, model: mat4x4<f32>, n_model: vec3<f32>, world_in: vec4<f32>, n_in: vec3<f32>,
         color_mul: vec4<f32>, params: vec4<f32>) -> VsOut {
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

    // GX rasterised colour (channel 0), computed per vertex like the hardware does.
    let lit = material.flags.x != 0u;
    let vert = material.flags.y != 0u;
    var rgb: vec3<f32>;
    var a: f32;
    if (lit) {
        let diff = max(dot(n, scene.light_dir.xyz), 0.0);
        var amb = scene.ambient.rgb * material.amb_mul.rgb;
        // The reference multiplies the ambient register of GMA materials by the object colour as well
        // as the material register (`ambientColor.r *= colorMul.r`); NaomiLib's ambient is left alone.
        if (material.flags.w == 0u) {
            amb = amb * color_mul.rgb;
        }
        let light = clamp(amb + scene.light_color.rgb * diff, vec3<f32>(0.0), vec3<f32>(1.0));
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
    if (material.extra.x > 1.5) {
        uv0.y = -uv0.y;
    } else if (material.extra.x > 0.5) {
        uv0.y = 1.0 - uv0.y;
    }
    out.uv0 = uv0;
    out.uv1 = in.uv1;
    out.uv2 = in.uv2;

    // Tex gens. Most materials have neither a scrolled nor a specular layer and stop here.
    let bits = material.layer_kinds.w;
    let every_layer = object.spec.w > 0.5;
    if (bits != 0u || every_layer) {
        var view_spec = vec2<f32>(0.5);
        var world_spec = vec2<f32>(0.5);
        if ((bits & 48u) != 0u) {
            // `TEXMTX0`: the model-view matrix without its translation, on the raw normal.
            var center = model * vec4<f32>(object.bound.xyz, 1.0);
            var tn = (model * vec4<f32>(n_model, 0.0)).xyz;
            if (params.y > 0.5) {
                center = scene.tilt * center;
                tn = (scene.tilt * vec4<f32>(tn, 0.0)).xyz;
            }
            tn = (scene.view * vec4<f32>(tn, 0.0)).xyz;
            let tl = length(tn);
            if (tl > 1e-6) { tn = tn / tl; } else { tn = vec3<f32>(0.0); }
            // From the eye to the far side of the model's bounding sphere.
            var dir = (scene.view * center).xyz;
            dir.z = dir.z - object.bound.w;
            let dl = length(dir);
            if (dl > 1e-6) { dir = dir / dl; } else { dir = vec3<f32>(0.0, 0.0, -1.0); }
            if ((bits & 16u) != 0u) {
                view_spec = spec_uv(look_rows(dir, vec3<f32>(0.0, 1.0, 0.0)), tn);
            }
            if ((bits & 32u) != 0u) {
                let light = (scene.view * vec4<f32>(scene.light_dir.xyz, 0.0)).xyz;
                let scaled = dir * -0.9;
                let aim = (light + scaled) * 0.5;
                world_spec = spec_uv(look_rows(aim, aim - scaled), tn);
            }
        }
        let kinds = material.layer_kinds;
        out.uv0 = tex_gen(uv0, kinds.x, every_layer || (bits & 1u) != 0u, view_spec, world_spec);
        out.uv1 = tex_gen(in.uv1, kinds.y, every_layer || (bits & 2u) != 0u, view_spec, world_spec);
        out.uv2 = tex_gen(in.uv2, kinds.z, every_layer || (bits & 4u) != 0u, view_spec, world_spec);
    }
    return out;
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var w: Waved;
    w.pos = in.position;
    w.normal = in.normal;
    if (object.spec.z > 0.5) {
        w = wave(in.position, in.normal);
    }
    let world = object.model * vec4<f32>(w.pos, 1.0);
    let n = (object.normal_mat * vec4<f32>(w.normal, 0.0)).xyz;
    return shade(in, object.model, w.normal, world, n, object.color_mul, object.params);
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
    let model = transpose(mat4x4<f32>(inst.row0, inst.row1, inst.row2, vec4<f32>(0.0, 0.0, 0.0, 1.0)));
    return shade(in, model, in.normal, world, n, inst.color, vec4<f32>(1.0, object.params.y, 0.0, 0.0));
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
    if ((SAMPLE_MASK & 1u) != 0u) { s0 = textureSample(tex0, samp0, in.uv0); }
    if ((SAMPLE_MASK & 2u) != 0u) { s1 = textureSample(tex1, samp1, in.uv1); }
    if ((SAMPLE_MASK & 4u) != 0u) { s2 = textureSample(tex2, samp2, in.uv2); }

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
        if (i == 1u) { t = s1; }
        if (i == 2u) { t = s2; }
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
            // out = prev + K0 * tex, clamped like every TEV stage. The object can switch it off (the
            // reference's `disableSpecular`).
            c = clamp(c + material.k0.rgb * object.spec.x * t.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        } else if (kind == 3u) {
            // out = prev + K1 * tex with K1 = K0 * the light's specular scale. The reference's first of
            // the two stages reads a ramp that is white wherever it is addressed.
            c = clamp(c + material.k0.rgb * scene.spec_scale.rgb * object.spec.y * t.rgb,
                      vec3<f32>(0.0), vec3<f32>(1.0));
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
