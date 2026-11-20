struct Globals {
    resolution: vec2<f32>,
    time: f32,
    padding: f32,
    cursor: vec2<f32>,
    cursor_present: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var artwork: texture_2d<f32>;
@group(1) @binding(1) var linear_sampler: sampler;
@group(1) @binding(2) var icons: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let triangle = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(triangle[index], 0.0, 1.0);
}

fn photo_uv(uv: vec2<f32>, aspect: f32) -> vec2<f32> {
    let scale = min(vec2(1.0), vec2(aspect * 0.5625, 1.777778 / aspect));
    return (uv - 0.5) * scale * 0.985 + 0.5;
}

fn hash(p: vec2<f32>) -> f32 {
    var q = fract(vec3(p.x, p.y, p.x) * vec3(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn grain(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash(i), hash(i + vec2(1.0, 0.0)), s.x),
        mix(hash(i + vec2(0.0, 1.0)), hash(i + vec2(1.0, 1.0)), s.x),
        s.y,
    );
}

@group(1) @binding(3) var links: texture_2d<f32>;

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let ratio = vec2(aspect, 1.0);
    let p = uv * ratio;
    let glass = textureSample(icons, linear_sampler, uv);
    let gutter = textureSample(links, linear_sampler, uv);
    let local_x = vec2(0.012 / aspect, 0.0);
    let left = textureSample(icons, linear_sampler, uv - local_x).b;
    let right = textureSample(icons, linear_sampler, uv + local_x).b;
    let boundary = (left - right) * 0.099;
    let cursor = globals.cursor_present * exp(-15.0 * length((uv - globals.cursor) * ratio));
    let vertical = (p.x + boundary + 0.0009 * sin(p.y * 18.0 + t * 0.7)) * 139.0;
    let col = floor(vertical);
    let rain = hash(vec2(col, 31.0));
    let streak = 1.0 - smoothstep(0.011, 0.117, abs(fract(vertical) - 0.53));
    let drain = (p.y * (4.9 + rain * 8.4) - t * (0.44 + 0.94 * rain)) + 14.0 * rain;
    let progress = fract(drain);
    let bead = smoothstep(0.47, 0.98, progress) * step(0.25, rain);
    let droplet_shape = exp(-24.0 * abs(fract(vertical) - 0.53)) * pow(progress, 12.0);
    let sheet = smoothstep(0.65, 0.86, glass.b);
    let wet_rim = smoothstep(0.26, 0.62, glass.b) - 0.96 * sheet;

    let pane_uv = photo_uv(uv, aspect);
    let normal = (vec2(fract(vertical) - 0.5, 0.1 - bead) * 0.013) * droplet_shape
        + vec2(boundary * 0.034, 0.003 * wet_rim * sin(t * 1.9 + p.x * 83.0));
    let view_plain = textureSample(artwork, linear_sampler, pane_uv).rgb;
    let view_bent = textureSample(artwork, linear_sampler, pane_uv + normal).rgb;
    let refraction = max(droplet_shape, 0.5 * wet_rim);
    let drained = pow(fract(gutter.g * 2.4 - t * 0.90), 14.0) * gutter.r;
    let flowing = (streak * bead * 0.79 + droplet_shape * 0.45) * (1.0 - 0.87 * sheet);
    let glass_reflect = vec3(0.24, 0.42, 0.59) * (0.20 * flowing + 0.12 * wet_rim);
    let catchlight = vec3(0.61, 0.72, 0.85) * (0.32 * drained + flowing * (0.08 + 0.18 * cursor));
    let fog = grain(p * 52.0 + vec2(0.0, -t * 0.35)) * 0.022;
    let city = mix(view_plain, view_bent * 1.28, refraction) * (0.86 - sheet * 0.27);
    return vec4(city + vec3(0.49, 0.68, 0.77) * (0.25 * gutter.b + fog) + glass_reflect + catchlight, 1.0);
}
