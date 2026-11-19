struct Globals {
    resolution: vec2<f32>,
    time: f32,
    padding: f32,
    cursor: vec2<f32>,
    cursor_present: f32,
    input_padding: f32,
    // From bass to treble, four consecutive bands in each vector.
    audio_bands: array<vec4<f32>, 4>,
}

@group(0) @binding(0) var<uniform> globals: Globals;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(points[index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let p = (2.0 * pixel.xy - globals.resolution) / globals.resolution.y;
    let t = globals.time * 0.25;
    let bass = dot(globals.audio_bands[0], vec4(0.25));
    let highs = dot(globals.audio_bands[2], vec4(0.25));
    let wave = (0.30 + bass * 0.18) * sin(p.x * 2.0 + t) + 0.16 * cos(p.x * 4.5 - t * 1.5);
    let ribbon = exp(-abs(p.y - wave) * 4.8);
    let glow = exp(-dot(p, p) * 1.15);
    let cursor = (2.0 * globals.cursor - 1.0) * globals.resolution / globals.resolution.y;
    let offset = p - cursor;
    let pointer_glow = globals.cursor_present * exp(-dot(offset, offset) * 5.0);
    let color = vec3(0.012, 0.021, 0.052)
        + ribbon * (1.0 + 0.7 * highs) * vec3(0.027, 0.29, 0.33)
        + pointer_glow * vec3(0.025, 0.065, 0.10)
        + glow * vec3(0.075, 0.046, 0.16);
    return vec4(color, 1.0);
}
