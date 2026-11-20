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

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let ratio = vec2(aspect, 1.0);
    let p = uv * ratio;
    let water_uv = photo_uv(uv, aspect);
    let current = vec2(sin(p.y * 21.0 - t * 0.75), cos(p.x * 20.0 + t * 0.49));
    let photo = textureSample(artwork, linear_sampler, water_uv + current * 0.0039).rgb;
    let swell = dot(p, vec2(9.4, -16.1)) + 0.87 * sin(p.x * 4.1 + p.y * 3.0 - t * 0.38)
        + 1.59 * grain(p * 7.3 + vec2(t * 0.084, -t * 0.13));
    let crest = fract(swell * 0.47 - t * 0.24);
    let surf = smoothstep(0.72, 0.96, crest) * (1.0 - smoothstep(0.97, 0.998, crest));
    let eddies = grain(p * 39.0 + vec2(-t * 0.34, t * 0.23));
    let small_surges = smoothstep(0.68, 0.93, sin(swell * 2.92 - t * 1.91)) * 0.18;
    let plankton = smoothstep(0.39, 0.88, eddies + 0.12 * grain(p * 135.0));
    let shore_effect = smoothstep(0.09, 0.73, p.y + 0.29 * sin(p.x * 4.2));
    let wake_point = (uv - globals.cursor) * ratio;
    let wake_distance = length(wake_point);
    let wakes = globals.cursor_present * exp(-15.0 * wake_distance)
        * pow(0.5 + 0.5 * sin(wake_distance * 118.0 - t * 6.3 + eddies * 0.81), 3.0);
    let glimmer = (surf * 0.62 + small_surges) * plankton * (0.10 + 0.54 * shore_effect);
    let ocean = photo * vec3(0.62, 0.77, 0.98) + vec3(0.001, 0.007, 0.014);
    let shimmer = vec3(0.026, 0.29, 0.67) * glimmer + vec3(0.033, 0.27, 0.49) * wakes;
    let foam = vec3(0.12, 0.43, 0.56) * pow(plankton, 4.0) * (0.45 * surf + 0.21 * wakes);
    return vec4(ocean + shimmer + foam, 1.0);
}
