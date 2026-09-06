// Loudstone terrain shader. Samples the generated block atlas, modulates it by
// baked face shade, ambient occlusion and voxel light, then fades into the sky.
//
// All mixing happens in LINEAR space. Blending fog in sRGB space is the classic
// way to get a washed-out, milky horizon, because a 50% mix of two sRGB values
// is much brighter than the true half-way colour.

struct CameraUniform {
    view_proj: mat4x4<f32>,
    cam_pos: vec3<f32>,
    fog_start: f32,
    sky_color: vec3<f32>,
    fog_end: f32,
    // x = 1.0 when this shader must encode linear -> sRGB itself, because the
    // swapchain format is not an sRGB one and the hardware will not do it.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: CameraUniform;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_samp: sampler;

struct VertexInput {
    @location(0) pos: vec3<f32>,
    @location(1) color: vec3<f32>,
    @location(2) light: f32,
    @location(3) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) light: f32,
    @location(2) world_pos: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_pos = camera.view_proj * vec4<f32>(in.pos, 1.0);
    out.color = in.color;
    out.light = in.light;
    out.world_pos = in.pos;
    out.uv = in.uv;
    return out;
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The atlas is an sRGB texture, so the sample arrives already linear.
    let tex = textureSample(atlas_tex, atlas_samp, in.uv);

    // Cut-out transparency for leaves and plants. Discarding rather than
    // blending keeps the terrain pass opaque, so nothing needs sorting.
    if (tex.a < 0.5) {
        discard;
    }

    // Texture MULTIPLIES the vertex colour, so per-block tinting (biome grass,
    // for one) still works, and light still scales the result.
    let albedo = tex.rgb * in.color;
    let lit = albedo * in.light;

    // Horizontal-only distance, so looking up or down does not slide the fog band.
    let d = length(in.world_pos.xz - camera.cam_pos.xz);
    let fog = clamp((d - camera.fog_start) / (camera.fog_end - camera.fog_start), 0.0, 1.0);

    var rgb = mix(lit, camera.sky_color, fog);
    if (camera.params.x > 0.5) {
        rgb = linear_to_srgb(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
