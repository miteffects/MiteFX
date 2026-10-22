// mite-ui effect passes: bloom (glow) and frosted glass, on top of Vello's frames.
//
// Every pass draws one full-screen triangle. Intermediate textures hold linear, premultiplied
// colour in f16; Vello's own textures are sRGB-encoded and unpremultiplied.

struct Params {
    // Source pixels per destination pixel (2 = reading a texture twice this one's size).
    map: vec2<f32>,
    // Blur direction in source texels (1,0) or (0,1).
    dir: vec2<f32>,
    // Gaussian standard deviation in source texels, and the tap radius.
    sigma: f32,
    radius: f32,
    // Bloom weights of the tight and the wide blur.
    k0: f32,
    k1: f32,
    // Glass: rounded rect (x0, y0, x1, y1) in destination pixels, corner radius, saturation.
    rect: vec4<f32>,
    corner: f32,
    saturate: f32,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var src_a: texture_2d<f32>;
@group(0) @binding(2) var src_b: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn uv_of(t: texture_2d<f32>, dst: vec2<f32>) -> vec2<f32> {
    return dst * p.map / vec2<f32>(textureDimensions(t));
}

// Vello's glow strip (sRGB, unpremultiplied, half size, `dir` rows down) → linear premultiplied.
@fragment
fn fs_prep(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src_a, vec2<i32>(pos.xy + p.dir), 0);
    return vec4<f32>(to_linear(c.rgb) * c.a, c.a);
}

// Bilinear downsample (a 2×2 box when `map` is 2). Reads an sRGB view or a linear texture.
@fragment
fn fs_down(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureSampleLevel(src_a, smp, uv_of(src_a, pos.xy), 0.0);
}

// One direction of a separable Gaussian.
@fragment
fn fs_blur(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let dims = vec2<f32>(textureDimensions(src_a));
    let centre = pos.xy * p.map;
    let r = i32(p.radius);
    let inv = -0.5 / (p.sigma * p.sigma);
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = -r; i <= r; i++) {
        let x = f32(i);
        let w = exp(x * x * inv);
        sum += w * textureSampleLevel(src_a, smp, (centre + p.dir * x) / dims, 0.0);
        total += w;
    }
    return sum / total;
}

// The tight (src_a, half size) and wide (src_b, quarter size) blurs, weighted.
@fragment
fn fs_bloom(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let a = textureSampleLevel(src_a, smp, pos.xy * 0.5 / vec2<f32>(textureDimensions(src_a)), 0.0);
    let b = textureSampleLevel(src_b, smp, pos.xy * 0.25 / vec2<f32>(textureDimensions(src_b)), 0.0);
    let c = clamp(a * p.k0 + b * p.k1, vec4<f32>(0.0), vec4<f32>(1.0));
    return vec4<f32>(min(c.rgb, vec3<f32>(c.a)), c.a);
}

fn rounded_rect_distance(q: vec2<f32>, rect: vec4<f32>, r: f32) -> f32 {
    let centre = (rect.xy + rect.zw) * 0.5;
    let half_size = (rect.zw - rect.xy) * 0.5;
    let d = abs(q - centre) - half_size + vec2<f32>(r);
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

// The blurred backdrop, saturated, inside the rounded rect (anti-aliased edge).
@fragment
fn fs_glass(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let mask = clamp(0.5 - rounded_rect_distance(pos.xy, p.rect, p.corner), 0.0, 1.0);
    let c = textureSampleLevel(src_a, smp, uv_of(src_a, pos.xy), 0.0);
    let lum = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let rgb = max(mix(vec3<f32>(lum), c.rgb, p.saturate), vec3<f32>(0.0));
    return vec4<f32>(rgb, 1.0) * mask;
}

// A Vello layer (sRGB, unpremultiplied, from the top left), premultiplied for an sRGB-space "over",
// which is how Vello blends inside one scene.
@fragment
fn fs_over(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src_a, vec2<i32>(pos.xy), 0);
    return vec4<f32>(c.rgb * c.a, c.a);
}
