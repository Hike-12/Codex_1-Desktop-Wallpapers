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

fn cloud_shape(p: vec2<f32>) -> f32 {
    return 0.60 * grain(p) + 0.26 * grain(p * 2.43 + vec2(17.0, 10.1))
        + 0.10 * grain(p * 5.29 - 15.0) + 0.04 * grain(p * 11.91 + 27.0);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let p = uv * vec2(aspect, 1.0);
    let landscape = textureSample(artwork, linear_sampler, photo_uv(uv, aspect)).rgb;
    let key = dot(landscape, vec3(0.24, 0.51, 0.25));
    let horizon = smoothstep(0.36, 0.64, uv.y);
    let light = pow(max(0.0, 1.0 - length((uv - vec2(0.67, 0.29)) * vec2(0.89, 1.61))), 2.0);
    let near = cloud_shape(p * vec2(4.2, 11.4) - vec2(t * 0.066, t * 0.013));
    let far = cloud_shape(p * vec2(6.1, 18.4) - vec2(t * 0.036, -t * 0.007));
    let level = 0.92 + 0.08 * cos(t * 0.12);
    let distant_mist = smoothstep(0.42, 0.72, far) * horizon * (0.15 + 0.28 * light);
    let banks = smoothstep(0.454, 0.78, near) * smoothstep(0.46, 0.93, uv.y);
    let peaks = smoothstep(0.061, 0.23, key) * 0.37;
    let cool = vec3(0.13, 0.20, 0.34);
    let lit = vec3(0.59, 0.37, 0.255);
    let atmosphere = mix(cool, lit, light * 0.62 + near * 0.22) * level;
    let ridges = landscape * vec3(0.88, 0.91, 1.0) + vec3(0.049, 0.025, 0.009) * light * level;
    let valley = mix(ridges, atmosphere, distant_mist + banks * (0.52 - peaks));
    let mist_glow = vec3(0.38, 0.25, 0.20) * smoothstep(0.69, 0.86, near) * horizon * light * 0.16;
    return vec4(valley + mist_glow, 1.0);
}
