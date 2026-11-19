struct Globals {
    resolution: vec2<f32>,
    time: f32,
    padding: f32,
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
    let wave = 0.30 * sin(p.x * 2.0 + t) + 0.16 * cos(p.x * 4.5 - t * 1.5);
    let ribbon = exp(-abs(p.y - wave) * 4.8);
    let glow = exp(-dot(p, p) * 1.15);
    let color = vec3(0.012, 0.021, 0.052)
        + ribbon * vec3(0.027, 0.29, 0.33)
        + glow * vec3(0.075, 0.046, 0.16);
    return vec4(color, 1.0);
}
