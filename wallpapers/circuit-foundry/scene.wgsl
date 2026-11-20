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
    let p = uv * vec2(aspect, 1.0);
    let substrate = textureSample(artwork, linear_sampler, photo_uv(uv, aspect)).rgb;
    let field = textureSample(icons, linear_sampler, uv);
    let wire = textureSample(links, linear_sampler, uv);
    let touch = globals.cursor_present * exp(-18.0 * length((uv - globals.cursor) * vec2(aspect, 1.0)));

    let high = smoothstep(0.66, 0.92, field.b);
    let chassis = smoothstep(0.81, 0.86, field.b);
    let face = smoothstep(0.88, 0.93, field.b);
    let gasket = max(0.0, chassis - face);
    let heatsink = 0.68 + 0.32 * sin(p.y * 1160.0);
    let module_grid = fract(p * 430.0);
    let microvias = 1.0 - smoothstep(0.085, 0.187, length(module_grid - 0.5));
    let color = mix(vec3(0.021, 0.44, 0.37), vec3(0.48, 0.25, 0.08), field.g);
    let chip = vec3(0.014, 0.026, 0.034) * heatsink + 0.14 * microvias * color;
    let steel = vec3(0.29, 0.35, 0.37) * (0.70 + 0.25 * grain(p * 93.0));
    let spark = pow(fract(wire.g * 2.5 - t * 0.94 + grain(p * 5.0)), 13.0);
    let return_pulse = pow(fract((1.0 - wire.g) * 3.6 - t * 0.61), 21.0);
    let trace = smoothstep(0.38, 0.73, wire.r);
    let seam = smoothstep(0.04, 0.47, wire.r) - 0.73 * trace;
    let gold = vec3(0.44, 0.235, 0.071);
    let signal = trace * (vec3(0.14, 0.82, 0.67) * spark + vec3(0.81, 0.42, 0.12) * return_pulse);
    let etch = smoothstep(0.53, 0.78, field.r) * microvias * 0.08;

    var surface = substrate * vec3(0.66, 0.91, 1.0) + vec3(0.005, 0.014, 0.019);
    surface += gold * (0.71 * trace + 0.3 * seam)
        + vec3(0.028, 0.19, 0.15) * wire.b
        + signal * (1.1 + touch)
        + color * (high * 0.10 + etch);
    surface = mix(surface, steel + color * (0.12 + 0.26 * touch), gasket);
    surface = mix(surface, chip + color * (0.1 + 0.12 * sin(t * 2.0 + field.g * 8.0)), face);
    return vec4(surface, 1.0);
}
