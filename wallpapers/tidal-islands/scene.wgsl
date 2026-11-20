struct Globals {
    resolution: vec2<f32>,
    time: f32,
    padding: f32,
    cursor: vec2<f32>,
    cursor_present: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var water_photo: texture_2d<f32>;
@group(1) @binding(1) var linear_sampler: sampler;
@group(1) @binding(2) var geography: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let triangle = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(triangle[index], 0.0, 1.0);
}

fn noise(p: vec2<f32>) -> f32 {
    let lattice = floor(p);
    let cell = fract(p);
    let uv = cell * cell * (3.0 - 2.0 * cell);
    let ramps = fract(sin(vec4(
        dot(lattice, vec2(127.1, 311.7)),
        dot(lattice, vec2(127.1, 311.7)) + 127.1,
        dot(lattice, vec2(127.1, 311.7)) + 311.7,
        dot(lattice, vec2(127.1, 311.7)) + 438.8
    )) * 43758.5453);
    return mix(mix(ramps.x, ramps.y, uv.x), mix(ramps.z, ramps.w, uv.x), uv.y);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let p = uv * vec2(aspect, 1.0);
    let photo_scale = min(vec2(1.0), vec2(aspect * 0.5625, 1.777778 / aspect));
    let photo_uv = (uv - 0.5) * photo_scale * 0.98 + 0.5;
    let swells = vec2(
        sin(p.y * 34.0 - t * 0.68) + 0.46 * sin(p.x * 63.0 + p.y * 36.0 + t),
        cos(p.x * 29.0 - t * 0.53) + 0.41 * cos(p.y * 43.0 - p.x * 31.0 + t)
    );
    let cursor_distance = length((uv - globals.cursor) * vec2(aspect, 1.0));
    let wake = sin(115.0 * cursor_distance - t * 6.0)
        * exp(-18.0 * cursor_distance) * globals.cursor_present;
    let photo = textureSample(water_photo, linear_sampler, photo_uv + swells * 0.003 + wake * 0.005).rgb;
    let middle_depth = 0.74 + 0.26 * smoothstep(0.0, 1.0, uv.y);
    let deep = photo * vec3(0.53, 0.84, 0.97) * middle_depth
        + vec3(0.003, 0.018, 0.039) + wake * vec3(0.002, 0.028, 0.031);

    let roughness = (noise(p * 75.0) - 0.5) * 0.095 + (noise(p * 201.0) - 0.5) * 0.037;
    let field = textureSample(geography, linear_sampler, uv).r;
    let tide = 0.018 * sin(55.0 * p.y - 17.0 * p.x - t * 1.12);
    let coast = field + roughness * smoothstep(0.08, 0.42, field);
    let shoal = smoothstep(0.10, 0.52, coast);
    let shoreline = 0.44 + tide;
    let sand_coverage = smoothstep(shoreline - 0.017, shoreline + 0.045, coast);
    let wet_sand = smoothstep(0.45, 0.65, coast);
    let grain = noise(p * 830.0) * 0.55 + noise(p * 160.0) * 0.45;
    let sand = mix(vec3(0.29, 0.17, 0.079), vec3(0.63, 0.50, 0.32), wet_sand)
        * (0.81 + 0.24 * grain);
    let shallows = deep * 0.79 + vec3(0.018, 0.105, 0.092) + photo * 0.17;
    let wash = (1.0 - smoothstep(0.012, 0.093, abs(coast - shoreline + 0.04)))
        * (0.32 + 0.49 * noise(p * 117.0 + vec2(t * 1.4, -t * 0.7)));
    let foam = vec3(0.42, 0.57, 0.51) * wash;
    let lagoon = mix(deep, shallows, shoal);
    let scene = mix(lagoon, sand, sand_coverage) + foam;
    let vignette = 1.0 - 0.16 * dot(2.0 * uv - 1.0, 2.0 * uv - 1.0);
    return vec4(scene * vignette, 1.0);
}
