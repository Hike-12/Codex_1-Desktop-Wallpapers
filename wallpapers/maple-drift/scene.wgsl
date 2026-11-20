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

fn falling_leaf(p: vec2<f32>, t: f32, density: f32, seed: f32, wind: f32) -> vec4<f32> {
    // Each layer moves through a field of cells with stable shapes and colors.
    let descent = t * (0.73 + 0.013 * density);
    let breeze = 0.18 * sin(t * 1.4 + p.y * 6.3 + seed) + wind * 0.17;
    let grid = p * density + vec2(breeze - t * 0.15, -descent) + seed;
    let cell = floor(grid);
    let random = hash(cell + seed);
    let center = 0.5 + vec2(hash(cell + 17.5), hash(cell - 23.7)) * 0.22 - 0.11;
    let drift = vec2(0.075 * sin(t * 1.8 + random * 29.0), 0.039 * sin(t * 1.35 + random * 17.0));
    let q = fract(grid) - center + drift;
    if random < 0.82 || dot(q, q) > 0.24 {
        return vec4(0.0);
    }

    let spin = random * 23.0 + t * (0.41 + random * 0.67) + 0.45 * wind;
    let direction = vec2(cos(spin), sin(spin));
    let twist = 0.41 + 0.59 * abs(sin(t * 1.94 + random * 51.0));
    let leaf = vec2(dot(q, direction) / twist, dot(q, vec2(-direction.y, direction.x)));
    let point = leaf / (0.33 + random * 0.06);
    let angle = atan2(point.x, -point.y + 0.16);
    let lobes = pow(0.5 + 0.5 * cos(angle * 5.0), 2.3);
    let serration = pow(0.5 + 0.5 * cos(angle * 25.0), 4.0);
    let silhouette = 0.48 + 0.39 * lobes + 0.038 * serration - 0.17 * max(point.y, 0.0);
    let radius = length(point * vec2(1.0, 0.97));
    let blade = 1.0 - smoothstep(silhouette - 0.025, silhouette + 0.04, radius);
    let stem = exp(-110.0 * abs(point.x - 0.036 * point.y))
        * smoothstep(0.27, 0.47, point.y) * (1.0 - smoothstep(0.72, 0.88, point.y));
    let veins = pow(abs(cos(angle * 5.0)), 22.0) * (1.0 - 0.65 * clamp(radius, 0.0, 1.0));
    let pigment = mix(vec3(0.58, 0.057, 0.016), vec3(0.86, 0.40, 0.061), hash(cell + 2.03));
    let lit = (0.47 + twist * 0.39) * (0.77 + 0.23 * grain(point * 27.0));
    return vec4(pigment * (lit + 0.22 * veins) + vec3(0.12, 0.053, 0.009) * stem, max(blade, stem * 0.74));
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pixel.xy / globals.resolution;
    let t = globals.time;
    let aspect = globals.resolution.x / globals.resolution.y;
    let ratio = vec2(aspect, 1.0);
    let p = uv * ratio;
    let foliage = textureSample(artwork, linear_sampler, photo_uv(uv, aspect)).rgb;
    let gust = globals.cursor_present * exp(-10.0 * length((uv - globals.cursor) * ratio));
    let sun = pow(max(0.0, 1.0 - length((uv - vec2(0.77, 0.20)) * vec2(1.15, 1.6))), 3.0);
    let shimmer = 0.48 + 0.52 * grain(p * 12.0 + vec2(t * 0.086, -t * 0.04));
    var amber = foliage * vec3(0.91, 0.77, 0.68) + vec3(0.105, 0.053, 0.020) * sun * shimmer;
    let far = falling_leaf(p, t * 0.84, 27.0, 13.7, gust * 0.58);
    let middle = falling_leaf(p, t, 15.0, 29.6, gust);
    let near = falling_leaf(p, t * 1.19, 9.0, 41.8, gust);
    amber = mix(amber, far.rgb * 0.54 + 0.15 * foliage, far.a * 0.55);
    amber = mix(amber, middle.rgb * 0.76, middle.a * 0.79);
    amber = mix(amber, near.rgb, near.a * 0.86);
    return vec4(amber, 1.0);
}
