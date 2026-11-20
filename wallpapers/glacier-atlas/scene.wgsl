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

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let ratio = vec2(aspect, 1.0);
    let p = uv * ratio;
    let drift = vec2(sin(21.0 * p.y + t * 0.9), cos(24.0 * p.x - t * 0.6)) * 0.0026;
    let blue = textureSample(artwork, linear_sampler, photo_uv(uv, aspect) + drift).rgb;
    let snow = textureSample(icons, linear_sampler, uv);
    let wind = globals.cursor_present * exp(-14.0 * length((uv - globals.cursor) * ratio));
    let plates = grain(p * 17.0) * 0.62 + grain(p * 53.0) * 0.31 + grain(p * 141.0) * 0.07;
    let edge = snow.r + (plates - 0.5) * 0.19;
    let thaw = 0.024 * sin(p.x * 41.0 + p.y * 39.0 + t * 1.24);
    let sheet = smoothstep(0.41, 0.52, edge + thaw);
    let submerged = smoothstep(0.09, 0.44, edge + thaw);
    let water_gap = 1.0 - smoothstep(0.013, 0.088, abs(edge - 0.41));
    let strata = dot(p, vec2(37.0, -19.0)) + 7.0 * grain(p * 13.0);
    let pressure = 1.0 - smoothstep(0.014, 0.14, abs(fract(strata) - 0.48));
    let seam_position = 2.0 * snow.b - snow.r;
    let collision = (1.0 - smoothstep(0.045, 0.22, seam_position))
        * smoothstep(0.10, 0.32, snow.a);
    let fissure = pressure * (0.19 + 0.7 * collision);
    let sparkle = grain(p * 395.0) * grain(p * 193.0);
    let compact = smoothstep(0.44, 0.96, edge);
    let ice = mix(vec3(0.092, 0.24, 0.34), vec3(0.44, 0.66, 0.75), compact)
        * (0.73 + 0.24 * plates + 0.13 * sparkle);
    let fracture = vec3(0.037, 0.37, 0.59) * (0.86 + 0.21 * sin(t * 2.2 + strata) + 0.34 * wind);
    let brine = blue * vec3(0.60, 0.79, 1.02) + vec3(0.005, 0.015, 0.030)
        + submerged * vec3(0.023, 0.12, 0.16);
    let floe = mix(ice, fracture, fissure) + vec3(0.032, 0.13, 0.18) * water_gap;
    return vec4(mix(brine, floe, sheet) + vec3(0.27, 0.47, 0.51) * water_gap * 0.22, 1.0);
}
