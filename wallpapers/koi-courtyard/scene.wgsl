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

fn koi(q: vec2<f32>, phase: f32, shade: vec3<f32>) -> vec4<f32> {
    let spine = q.y + sin(q.x * 2.4 + phase) * max(0.0, 0.2 - q.x) * 0.23;
    let body = length(vec2(q.x, spine * 3.0));
    let tail_x = q.x + 1.17;
    let tail_span = 0.09 + 0.45 * smoothstep(0.02, 0.58, -tail_x);
    let tail = (1.0 - smoothstep(tail_span - 0.06, tail_span, abs(spine)))
        * smoothstep(-0.60, -0.46, tail_x) * (1.0 - smoothstep(0.15, 0.31, tail_x));
    let fins = exp(-24.0 * abs(q.x + 0.13 + 0.26 * abs(spine))) * exp(-4.1 * abs(spine));
    let core = 1.0 - smoothstep(0.91, 1.04, body);
    let scales = grain(q * 11.5);
    let spots = smoothstep(0.53, 0.63, grain(q * vec2(3.0, 6.0) + phase * 0.033));
    let pigment = mix(shade, vec3(0.63, 0.66, 0.57), spots);
    let ridge = exp(-6.2 * abs(spine)) * (0.12 + 0.25 * scales);
    return vec4(pigment * (0.64 + ridge) + 0.08 * scales, max(core, 0.44 * tail + 0.32 * fins));
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let ratio = vec2(aspect, 1.0);
    let p = uv * ratio;
    let from_cursor = (uv - globals.cursor) * ratio;
    let cursor_distance = length(from_cursor);
    let ripple = globals.cursor_present * exp(-11.0 * cursor_distance) * sin(116.0 * cursor_distance - t * 5.0);
    let refl = vec2(sin(p.y * 31.0 + t * 0.77), cos(p.x * 25.0 - t * 0.72));
    let water_uv = photo_uv(uv, aspect) + refl * 0.0025 + ripple * 0.005;
    let photo = textureSample(artwork, linear_sampler, water_uv).rgb;
    var water = photo * vec3(0.79, 1.01, 0.89) + vec3(0.002, 0.013, 0.016);
    water += vec3(0.024, 0.068, 0.054) * ripple;

    // Sample the layout at each fish's position and steer away from dense beds.
    for (var i = 0u; i < 4u; i++) {
        let id = f32(i);
        let phase = t * (0.33 - id * 0.041) + id * 1.67;
        let home = vec2(0.50, 0.52) + vec2(0.30 * cos(phase), 0.29 * sin(phase * 1.09 + id));
        let land = textureSampleLevel(icons, linear_sampler, home, 0.0).r;
        let detour = vec2(0.47, 0.49) + vec2(0.21 * sin(phase + 1.32), 0.18 * cos(phase));
        let fish_uv = mix(home, detour, 0.82 * smoothstep(0.15, 0.78, land));
        let direction = normalize(vec2(-sin(phase) * aspect, 1.13 * cos(phase * 1.09 + id)) + 0.00001);
        let delta = (uv - fish_uv) * ratio;
        let q = vec2(dot(delta, direction), dot(delta, vec2(-direction.y, direction.x)))
            / (0.040 - 0.0042 * id);
        if dot(q, q) < 4.0 {
            let pigment = mix(vec3(0.82, 0.20, 0.036), vec3(0.43, 0.077, 0.027), fract(id * 0.63));
            let fish = koi(q, phase * 12.0, pigment);
            let depth = 0.64 + 0.15 * sin(phase);
            water = mix(water, fish.rgb * depth + photo * 0.21, fish.a * 0.91);
        }
    }

    let pad_grid = p * 40.0;
    let cell = floor(pad_grid);
    let random = hash(cell);
    let center = (cell + 0.5) / (40.0 * ratio);
    let bed = textureSample(icons, linear_sampler, center);
    let q = fract(pad_grid) - (0.36 + 0.28 * vec2(hash(cell + 12.0), hash(cell - 23.0)));
    let angle = random * 6.28 + 0.12 * sin(t * 0.9 + random * 11.0);
    let dir = vec2(cos(angle), sin(angle));
    let leaf_uv = vec2(dot(q, dir), dot(q, vec2(-dir.y, dir.x)));
    let leaf_size = 0.27 + random * 0.22 + 0.06 * smoothstep(0.16, 0.61, bed.a);
    let leaf_radius = length(leaf_uv * vec2(1.0, 1.10));
    let cleft = smoothstep(0.035, 0.11, abs(leaf_uv.y) + min(0.0, leaf_uv.x) * 0.29);
    let shape = (1.0 - smoothstep(leaf_size - 0.044, leaf_size, leaf_radius)) * select(1.0, cleft, leaf_uv.x > 0.01);
    let vegetation = smoothstep(0.16 + random * 0.45, 0.29 + random * 0.45, bed.r);
    let ribs = pow(max(0.0, cos(atan2(leaf_uv.y, leaf_uv.x) * 15.0)), 5.0);
    let leaf = mix(vec3(0.018, 0.077, 0.022), vec3(0.17, 0.34, 0.048), random)
        * (0.77 + 0.28 * ribs + 0.27 * leaf_uv.x);
    return vec4(mix(water, leaf, shape * vegetation * 0.96), 1.0);
}
