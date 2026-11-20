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

fn leaflet(q: vec2<f32>, rotation: f32, width: f32) -> f32 {
    let direction = vec2(cos(rotation), sin(rotation));
    let tip = dot(q, direction);
    let side = dot(q, vec2(-direction.y, direction.x));
    let cut = length(vec2((tip - 0.17) * 2.29, side / width));
    return 1.0 - smoothstep(0.70, 0.95, cut);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let ratio = vec2(aspect, 1.0);
    let p = uv * ratio;
    let slate = textureSample(artwork, linear_sampler, photo_uv(uv, aspect)).rgb;
    let icons_here = textureSample(icons, linear_sampler, uv);
    let water = globals.cursor_present * exp(-15.0 * length((uv - globals.cursor) * ratio));
    let soil = grain(p * 39.0) * 0.60 + grain(p * 156.0) * 0.40;
    let lichen = smoothstep(0.19, 0.75, icons_here.r + (soil - 0.5) * 0.23);
    let shading = 0.82 + 0.32 * grain(p * 89.0);

    let bed = p * 79.0 + vec2(0.39 * sin(p.y * 61.0), 0.39 * sin(p.x * 60.0));
    let cell = floor(bed);
    let seed = hash(cell);
    let phase = seed * 6.2831853;
    let growth = textureSample(icons, linear_sampler, (cell + 0.5) / (79.0 * ratio));
    let shoots = smoothstep(0.20 + seed * 0.46, 0.31 + seed * 0.46, growth.r);
    let sway = sin(t * 1.6 + phase) * (0.07 + water * 0.21);
    let q = fract(bed) - vec2(0.33 + 0.33 * hash(cell + 13.0), 0.34 + 0.31 * hash(cell - 17.0));
    let crown = max(
        leaflet(q, phase + sway, 0.19),
        max(leaflet(q, phase + 2.15 - sway, 0.14), leaflet(q, phase - 1.30 + sway, 0.12)),
    );
    let pale = mix(vec3(0.043, 0.13, 0.024), vec3(0.35, 0.47, 0.075), seed);
    let root = slate * (1.0 - 0.68 * lichen) + vec3(0.041, 0.081, 0.018) * shading * lichen;
    let leaf = pale * (0.68 + 0.33 * crown + 0.24 * soil);
    let droplet = pow(max(0.0, 1.0 - length(q - vec2(0.12, -0.11)) * 15.0), 2.0)
        * step(0.89, seed) * (0.4 + water);
    let moist = vec3(0.31, 0.54, 0.41) * shoots * droplet;
    return vec4(mix(root, leaf, crown * shoots) + moist, 1.0);
}
