struct Globals {
    resolution: vec2<f32>,
    time: f32,
    padding: f32,
}

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var image: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(points[index], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(image));
    let viewport_aspect = globals.resolution.x / globals.resolution.y;
    let video_aspect = dimensions.x / dimensions.y;
    let crop = min(vec2(1.0), vec2(viewport_aspect / video_aspect, video_aspect / viewport_aspect));
    let uv = (pixel.xy / globals.resolution - 0.5) * crop + 0.5;
    return vec4(textureSample(image, image_sampler, uv).rgb, 1.0);
}
