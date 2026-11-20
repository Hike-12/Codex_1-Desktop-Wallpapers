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

fn snowfall(p: vec2<f32>, t: f32, density: f32, seed: f32, sway: f32) -> f32 {
    let grid = p * density + vec2(0.29 * sin(t * 0.74 + p.y * 5.1 + seed) + t * sway, -t * (0.57 + 0.023 * density)) + seed;
    let cell = floor(grid);
    let random = hash(cell + seed);
    let center = 0.5 + vec2(hash(cell + 11.0), hash(cell - 33.4)) * 0.43 - 0.215;
    let size = 0.064 + 0.088 * random;
    let distance = length((fract(grid) - center) * vec2(1.0, 0.81));
    let flake = 1.0 - smoothstep(size * 0.22, size + 0.06, distance);
    let light = 0.73 + 0.27 * sin(t * 1.67 + 43.8 * random);
    return flake * light * step(0.66, random);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let p = uv * vec2(aspect, 1.0);
    let village = textureSample(artwork, linear_sampler, photo_uv(uv, aspect)).rgb;
    let lantern = smoothstep(0.008, 0.20, village.r - village.b * 1.16);
    let pulse = 0.88 + 0.12 * sin(t * 1.22 + grain(p * 37.0) * 8.0);
    let flecks = 0.24 * snowfall(p, t * 0.64, 53.0, 3.0, 0.088)
        + 0.43 * snowfall(p, t, 29.0, 16.0, 0.19)
        + 0.47 * snowfall(p, t * 1.34, 13.0, 57.0, 0.32);
    let glow = vec3(0.32, 0.147, 0.026) * lantern * pulse;
    let darkness = 1.0 - 0.11 * length(uv - 0.5);
    let snow = mix(vec3(0.49, 0.68, 0.86), vec3(0.93, 0.63, 0.35), lantern * 0.90);
    return vec4((village * vec3(0.77, 0.87, 1.04) + glow) * darkness + snow * flecks, 1.0);
}
