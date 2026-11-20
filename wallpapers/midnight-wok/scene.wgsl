struct Globals {
    resolution: vec2<f32>,
    time: f32,
    padding: f32,
    cursor: vec2<f32>,
    cursor_present: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(1) var linear_sampler: sampler;
@group(1) @binding(2) var heat_map: texture_2d<f32>;
@group(1) @binding(0) var pan_photo: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let triangle = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(triangle[index], 0.0, 1.0);
}

fn hash(cell: vec2<f32>) -> f32 {
    return fract(sin(dot(cell, vec2(127.1, 311.7))) * 43758.5453);
}

fn oil_noise(p: vec2<f32>) -> f32 {
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
    let p = uv * vec2(aspect, 1.0);
    let photo_scale = min(vec2(1.0), vec2(aspect * 0.5625, 1.777778 / aspect));
    let photo_uv = (uv - 0.5) * photo_scale + 0.5;
    let pan = textureSample(pan_photo, linear_sampler, photo_uv).rgb;
    let field = textureSample(heat_map, linear_sampler, uv);
    let heat = smoothstep(0.06, 0.78, field.r);
    let contact = smoothstep(0.06, 0.38, field.a);
    let d_cursor = length((uv - globals.cursor) * vec2(aspect, 1.0));
    let cursor_draft = globals.cursor_present * exp(-15.0 * d_cursor);
    let oil = oil_noise(p * 164.0 + vec2(0.0, -t * 1.1));
    let grease = smoothstep(0.46, 0.87, oil) * heat;
    let surface = pan * (0.94 + 0.38 * heat)
        + vec3(0.060, 0.024, 0.007) * (heat + 0.83 * contact)
        + vec3(0.10, 0.054, 0.021) * grease;

    let lattice = p * 104.0 + 0.85 * vec2(
        sin(dot(p, vec2(89.0, 17.0))), sin(dot(p, vec2(23.0, 78.0))),
    );
    let cell = floor(lattice);
    let random = hash(cell);
    let phase = random * 6.2831853;
    let jitter = vec2(hash(cell + 17.0), hash(cell - 23.0)) * 0.56 - 0.28;
    let tumble = 0.052 * vec2(cos(t * 4.7 + phase), sin(t * 4.3 + phase))
        * (heat + cursor_draft);
    let q = fract(lattice) - 0.5 - jitter + tumble;
    let angle = phase + 0.16 * sin(t * 2.1 + phase);
    let along = dot(q, vec2(cos(angle), sin(angle)));
    let across = dot(q, vec2(-sin(angle), cos(angle)));
    let kinds = array(
        vec3(0.80, 0.16, 0.042),
        vec3(0.21, 0.51, 0.042),
        vec3(0.78, 0.53, 0.10),
        vec3(0.50, 0.19, 0.105),
        vec3(0.69, 0.65, 0.38),
    );
    let ingredient = u32(min(floor(field.g * 5.0), 4.0));
    let seasoned = (ingredient + select(0u, 2u, random > 0.88)) % 5u;
    let sliver = select(1.65, 2.65, seasoned == 1u || seasoned == 4u);
    let shape = pow(abs(along) * 1.12, 1.46) + pow(abs(across) * sliver, 1.46);
    let crust = smoothstep(0.21, 0.37, shape);
    let piece = 1.0 - smoothstep(0.29, 0.42, shape);
    let ingredient_density = smoothstep(0.33, 0.79, field.b + 0.37 * contact);
    let populated = smoothstep(0.1, 0.24, ingredient_density * 1.5 - random * 0.75);
    let dark_edge = mix(0.83, 0.41, crust);
    let seared = kinds[seasoned] * (dark_edge + 0.22 * oil + 0.09 * random)
        + vec3(0.13, 0.079, 0.018) * grease;
    let cooked = mix(surface, seared, piece * populated * (0.68 + 0.32 * heat));

    let steam_source = textureSample(
        heat_map, linear_sampler, uv + vec2(-cursor_draft * 0.018, 0.020),
    ).r;
    let steam = smoothstep(0.45, 0.85, oil_noise(p * vec2(36.0, 13.0) + vec2(t * 0.23, t * 0.64)))
        * steam_source * (0.069 + 0.06 * contact);
    let spark = smoothstep(0.90, 1.0, sin(t * 6.0 + phase))
        * (1.0 - smoothstep(0.02, 0.14, length(q))) * heat * step(0.964, random);
    return vec4(cooked + vec3(0.78, 0.67, 0.58) * steam
        + vec3(0.94, 0.31, 0.019) * spark, 1.0);
}
