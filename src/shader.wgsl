// Loudstone terrain shader. Flat per-block colour, modulated by baked face shade
// and ambient occlusion, then faded into the sky by distance fog.

struct CameraUniform {
    view_proj: mat4x4<f32>,
    cam_pos: vec3<f32>,
    fog_start: f32,
    sky_color: vec3<f32>,
    fog_end: f32,
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;

struct VertexInput {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) light: f32,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) light: f32,
    @location(2) world_pos: vec3<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    out.light = in.light;
    out.world_pos = in.pos;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let base = in.color * in.light;

    // Horizontal-only distance so looking up or down does not change the fog band.
    let d = length(in.world_pos.xz - camera.cam_pos.xz);
    let fog = clamp((d - camera.fog_start) / (camera.fog_end - camera.fog_start), 0.0, 1.0);

    let rgb = mix(base, camera.sky_color, fog);
    return vec4<f32>(rgb, 1.0);
}
