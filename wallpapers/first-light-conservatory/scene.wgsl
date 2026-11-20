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

fn bead_layer(p: vec2<f32>, t: f32, density: f32, seed: f32) -> vec4<f32> {
    let lane = floor(p.x * density + seed);
    let speed = 0.64 + 0.85 * hash(vec2(lane, seed));
    let grid = p * vec2(density, density * 0.62) + vec2(seed, -speed * t);
    let cell = floor(grid);
    let random = hash(cell + seed);
    let center = 0.5 + vec2(hash(cell + 11.0), hash(cell - 32.0)) * 0.48 - 0.24;
    let q = fract(grid) - center;
    let glass = length(q * vec2(1.0, 1.10));
    let drop_size = 0.076 + 0.083 * random;
    let lens = (1.0 - smoothstep(drop_size - 0.032, drop_size + 0.026, glass)) * step(0.74, random);
    let bright_edge = smoothstep(0.064, 0.122, glass) * max(0.0, -q.y * 7.3);
    let thread = exp(-82.0 * abs(q.x)) * exp(-7.7 * max(0.04, q.y)) * step(0.0, q.y) * step(0.90, random);
    return vec4(-q * lens, lens * bright_edge, thread);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let p = uv * vec2(aspect, 1.0);
    let full_uv = photo_uv(uv, aspect);
    let still = textureSample(artwork, linear_sampler, full_uv).rgb;
    let greens = smoothstep(0.004, 0.112, still.g - 0.93 * still.r);
    let foliage = greens * smoothstep(0.26, 0.75, uv.y);
    let flutter = vec2(sin(p.y * 20.0 + p.x * 9.0 + t * 1.43), cos(p.y * 25.0 - p.x * 8.0 - t * 0.78));
    let plants = full_uv + foliage * flutter * 0.0064;
    let large = bead_layer(p, t * 0.77, 26.0, 7.0);
    let fine = bead_layer(p, t * 1.06, 53.0, 13.0);
    let lens = large.xy * 0.070 + fine.xy * 0.034;
    let pane = textureSample(artwork, linear_sampler, plants + lens).rgb;
    let sunshine = pow(max(0.0, 1.0 - length((uv - vec2(0.24, 0.15)) * vec2(1.33, 1.14))), 2.0);
    let beams = smoothstep(0.62, 0.78, grain(vec2(p.x * 7.0 - p.y * 8.6 + t * 0.078, p.y * 0.66)));
    let warmth = vec3(0.24, 0.13, 0.047) * sunshine * (0.25 + 0.40 * beams);
    let flash = vec3(0.36, 0.46, 0.37) * (large.z * 0.76 + fine.z * 0.55 + 0.095 * (large.w + fine.w));
    return vec4(pane * vec3(0.91, 0.90, 0.80) + warmth + flash, 1.0);
}
