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
    let tremor = vec2(grain(p * 16.0 + vec2(t * 0.34, 0.0)), grain(p * 23.0 - t * 0.31)) - 0.5;
    let firebed = textureSample(icons, linear_sampler, uv + tremor * 0.024);
    let trench = textureSample(links, linear_sampler, uv + tremor * 0.037);
    let stone = textureSample(artwork, linear_sampler, photo_uv(uv, aspect)).rgb;
    let age = grain(p * 61.0 + tremor * 2.0);
    let brick = grain(p * 16.0 + age * 0.34);
    let scale = abs(grain(p * 110.0 + tremor * 1.7) - 0.48);
    let split = 1.0 - smoothstep(0.024, 0.13, scale);
    let lava = smoothstep(0.35, 0.82, firebed.r * (0.61 + 0.47 * brick) + trench.b * 0.27);
    let crack = split * lava;
    let heat = 0.69 + 0.31 * sin(t * 1.4 + brick * 4.9 + age * 2.0);
    let jet = pow(fract(trench.g * 2.0 - t * 0.58), 17.0) * pow(trench.r, 1.9);
    let wind = globals.cursor_present * exp(-16.0 * length((uv - globals.cursor) * ratio));

    let swept = p * vec2(88.0, 91.0) + vec2(-0.5 * t, t * (8.0 + wind * 4.0));
    let ember_cell = floor(swept);
    let random = hash(ember_cell);
    let speckle = length(fract(swept) - vec2(0.22 + 0.52 * hash(ember_cell + 17.0), 0.50));
    let spark = (1.0 - smoothstep(0.035, 0.18, speckle))
        * step(0.94, random) * smoothstep(0.20, 0.79, firebed.r + trench.b * 0.38);
    let blackened = stone * (0.77 + 0.38 * brick);
    let cooling = vec3(0.28, 0.048, 0.010) * (0.36 * firebed.r + 0.68 * trench.b);
    let margin = vec3(0.57, 0.085, 0.013) * (lava * 0.20 + 0.47 * crack);
    let molten = vec3(0.88, 0.34, 0.042) * (crack * 0.68 + 0.41 * pow(trench.r, 2.0)) * heat;
    let hot = vec3(1.0, 0.53, 0.12) * (0.51 * jet + (0.8 + wind) * spark);
    return vec4(blackened + cooling + margin + molten + hot, 1.0);
}
