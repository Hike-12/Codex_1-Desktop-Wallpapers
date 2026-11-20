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

fn strip(distance: f32, thickness: f32) -> f32 {
    return 1.0 - smoothstep(thickness, thickness + 0.045, distance);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let ratio = vec2(aspect, 1.0);
    let p = uv * ratio;
    let map = textureSample(artwork, linear_sampler, photo_uv(uv, aspect)).rgb;

    // A district illuminates whole buildings as the icon field reaches their center.
    let grid = p * 29.0;
    let cell = floor(grid);
    let building_uv = (cell + 0.5) / (29.0 * ratio);
    let urban = textureSample(icons, linear_sampler, building_uv);
    let here = textureSample(icons, linear_sampler, uv);
    let lot = (fract(grid) - 0.5) * 2.0;
    let lot_random = hash(cell);
    let roofs = max(abs(lot.x), abs(lot.y));
    let lights = smoothstep(0.21, 0.73, urban.r);
    let roof = strip(roofs, 0.60 + 0.16 * lot_random);
    let rim = roof * smoothstep(0.53, 0.70, roofs);
    let slats = vec2(7.0, 9.0) * (lot + 1.0);
    let lamp = vec2(1.0) - 2.0 * abs(fract(slats) - 0.5);
    let window = smoothstep(0.42, 0.72, min(lamp.x, lamp.y));
    let rented = step(0.29, hash(floor(slats) + cell * 19.2));
    let glow = mix(vec3(0.025, 0.38, 0.65), vec3(0.81, 0.27, 0.08), urban.g);
    let office = roof * (vec3(0.009, 0.021, 0.046) + glow * window * rented * 0.34);
    let street = textureSample(links, linear_sampler, uv);
    let traffic_position = fract(street.g * 2.0 - t * 0.51 + grain(p * 4.0) * 0.64);
    let traffic = pow(traffic_position, 12.0) * smoothstep(0.32, 0.75, street.r);
    let crosswalk = step(0.83, fract(street.g * 13.0)) * street.r;
    let cursor = globals.cursor_present * exp(-8.5 * length((uv - globals.cursor) * ratio));
    let avenues = vec3(0.014, 0.090, 0.15) * street.b
        + vec3(0.045, 0.21, 0.33) * street.r
        + vec3(0.73, 0.35, 0.12) * (traffic + 0.12 * crosswalk);

    var night = map * vec3(0.61, 0.72, 1.04);
    night *= 1.0 - 0.52 * roof * lights;
    night += lights * (office + rim * glow * 0.19)
        + avenues * (0.9 + cursor)
        + glow * (0.057 * here.r + 0.055 * here.a);
    return vec4(night, 1.0);
}
