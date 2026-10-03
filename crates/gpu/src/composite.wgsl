// FilmCraft GPU compositor.
// Layers are drawn as transformed quads; the fragment shader samples the source (YUV planes or
// RGBA) with manual bilinear filtering (textureLoad, so any float format works) and N×N
// supersampling over the pixel footprint when minifying, converts to linear light, premultiplies
// and scales by opacity. Blending into the Rgba16Float accumulator is premultiplied "over".

struct U {
    m0: vec4<f32>,   // a b c d
    m1: vec4<f32>,   // e f out_w out_h
    src: vec4<f32>,  // src_w src_h chroma_w chroma_h
    p0: vec4<f32>,   // opacity, kind (0 rgba8 srgb straight, 1 rgba16f premul linear, 2 yuv), taps, transfer (0 srgb, 1 linear, 2 pq, 3 hlg)
    p1: vec4<f32>,   // y_off y_scale c_off c_scale (code units)
    p2: vec4<f32>,   // kr kb code_scale footprint
    fx: vec4<f32>,   // effect id, brightness, contrast, reserved
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var tex0: texture_2d<f32>;
@group(0) @binding(2) var tex1: texture_2d<f32>;
@group(0) @binding(3) var tex2: texture_2d<f32>;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) sp: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VOut {
    var corners = array<vec2<f32>, 6>(vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let c = corners[vi];
    let s = c * u.src.xy;
    let o = vec2(u.m0.x * s.x + u.m0.z * s.y + u.m1.x, u.m0.y * s.x + u.m0.w * s.y + u.m1.y);
    var out: VOut;
    out.pos = vec4(o.x / u.m1.z * 2.0 - 1.0, 1.0 - o.y / u.m1.w * 2.0, 0.0, 1.0);
    out.sp = s;
    return out;
}

fn load4(t: texture_2d<f32>, p: vec2<f32>) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(t));
    let q = p - 0.5;
    let i = vec2<i32>(floor(q));
    let f = q - floor(q);
    let a = textureLoad(t, clamp(i, vec2(0), d - 1), 0);
    let b = textureLoad(t, clamp(i + vec2(1, 0), vec2(0), d - 1), 0);
    let cc = textureLoad(t, clamp(i + vec2(0, 1), vec2(0), d - 1), 0);
    let dd = textureLoad(t, clamp(i + vec2(1, 1), vec2(0), d - 1), 0);
    return mix(mix(a, b, f.x), mix(cc, dd, f.x), f.y);
}

fn srgb_to_linear(v: vec3<f32>) -> vec3<f32> {
    let lo = v / 12.92;
    let hi = pow((max(v, vec3(0.0)) + 0.055) / 1.055, vec3(2.4));
    return select(hi, lo, v <= vec3(0.04045));
}

fn pq_eotf(e: vec3<f32>) -> vec3<f32> {
    let m1 = 0.1593017578125; let m2 = 78.84375; let c1 = 0.8359375; let c2 = 18.8515625; let c3 = 18.6875;
    let p = pow(max(e, vec3(0.0)), vec3(1.0 / m2));
    return pow(max(p - c1, vec3(0.0)) / (c2 - c3 * p), vec3(1.0 / m1)) * 100.0;
}

fn hlg_inv(e: vec3<f32>) -> vec3<f32> {
    let a = 0.17883277; let b = 0.28466892; let c = 0.55991073;
    let lo = e * e / 3.0;
    let hi = (exp((e - c) / a) + b) / 12.0;
    return select(hi, lo, e <= vec3(0.5));
}

fn to_linear(v: vec3<f32>) -> vec3<f32> {
    let t = u32(u.p0.w);
    if t == 1u { return v; }
    if t == 2u { return pq_eotf(v); }
    if t == 3u { return hlg_inv(v); }
    return srgb_to_linear(clamp(v, vec3(0.0), vec3(1.0)));
}

fn linear_to_srgb(v: vec3<f32>) -> vec3<f32> {
    let x = max(v, vec3(0.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3(0.0031308));
}

fn apply_effect(c: vec4<f32>) -> vec4<f32> {
    if u.fx.x < 0.5 || c.a <= 1e-6 {
        return c;
    }
    let encoded = linear_to_srgb(c.rgb / c.a);
    let br = u.fx.y / 100.0 * 0.4;
    let co = 1.0 + u.fx.z / 100.0;
    let corrected = clamp((encoded - vec3(0.5)) * co + vec3(0.5 + br), vec3(0.0), vec3(1.0));
    return vec4(srgb_to_linear(corrected) * c.a, c.a);
}

// One linear premultiplied sample at source position p.
fn sample(p: vec2<f32>) -> vec4<f32> {
    let kind = u32(u.p0.y);
    if kind == 0u {
        // rgba8 sRGB texture: loads are already linear, straight alpha
        let c = load4(tex0, p);
        return vec4(c.rgb * c.a, c.a);
    }
    if kind == 1u {
        return load4(tex0, p);
    }
    let cs = u.p2.z;
    let yc = load4(tex0, p).r * cs;
    let cp = p * u.src.zw / u.src.xy;
    let cb = load4(tex1, cp).r * cs;
    let cr = load4(tex2, cp).r * cs;
    let y = (yc - u.p1.x) / u.p1.y;
    let b = (cb - u.p1.z) / u.p1.w;
    let r = (cr - u.p1.z) / u.p1.w;
    let kr = u.p2.x; let kb = u.p2.y; let kg = 1.0 - kr - kb;
    let R = y + 2.0 * (1.0 - kr) * r;
    let B = y + 2.0 * (1.0 - kb) * b;
    let G = (y - kr * R - kb * B) / kg;
    return vec4(to_linear(vec3(R, G, B)), 1.0);
}

@fragment
fn fs(in: VOut) -> @location(0) vec4<f32> {
    let n = max(u32(u.p0.z), 1u);
    let fp = u.p2.w;
    var acc = vec4(0.0);
    for (var j = 0u; j < n; j++) {
        for (var i = 0u; i < n; i++) {
            let off = (vec2(f32(i), f32(j)) + 0.5) / f32(n) - 0.5;
            acc += apply_effect(sample(in.sp + off * fp));
        }
    }
    return acc / f32(n * n) * u.p0.x;
}

// ---- final pass: accumulator (linear premul) over black → sRGB target

@group(0) @binding(0) var accum: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vi << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vi & 2u) * 2.0 - 1.0;
    return vec4(x, y, 0.0, 1.0);
}

@fragment
fn fs_full(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = clamp(textureLoad(accum, vec2<i32>(pos.xy), 0).rgb, vec3(0.0), vec3(1.0));
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055;
    return vec4(select(hi, lo, c <= vec3(0.0031308)), 1.0);
}
