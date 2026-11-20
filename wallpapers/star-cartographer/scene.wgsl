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
    let drift = 0.005 * vec2(sin(t * 0.07), cos(t * 0.054));
    let sky = textureSample(artwork, linear_sampler, photo_uv(uv, aspect) + drift).rgb;
    let stars = textureSample(icons, linear_sampler, uv);
    let chart = textureSample(links, linear_sampler, uv);
    let cursor = globals.cursor_present * exp(-9.0 * length((uv - globals.cursor) * ratio));
    let beating = 0.88 + 0.12 * sin(t * 1.8 + stars.g * 12.0);
    let corona = pow(stars.b, 11.0);
    let white = smoothstep(0.930, 0.994, stars.b);
    let phase = fract(chart.g * 1.7 - t * 0.31 + 0.36 * grain(p * 4.0));
    let runner = pow(phase, 25.0) * smoothstep(0.22, 0.66, chart.r);
    let dot_grid = step(0.963, hash(floor(p * 190.0)));
    let dust_point = pow(max(0.0, 1.0 - length(fract(p * 190.0) - 0.5) * 2.3), 5.0);
    let glint = sin(t * 2.1 + hash(floor(p * 190.0) + 12.0) * 24.0) * 0.5 + 0.5;
    let catalog = vec3(0.26, 0.37, 0.49) * dot_grid * dust_point * glint;
    let threads = vec3(0.11, 0.18, 0.24) * chart.b
        + vec3(0.48, 0.62, 0.70) * pow(chart.r, 3.0) * (0.77 + cursor)
        + vec3(0.91, 0.68, 0.37) * runner;
    let color = mix(vec3(0.21, 0.48, 0.83), vec3(0.89, 0.58, 0.25), stars.g);
    let asterism = color * (0.035 * stars.r + 0.62 * corona) * beating
        + vec3(0.82, 0.90, 0.99) * white;
    return vec4(sky * 0.68 + catalog + threads + asterism, 1.0);
}
