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
    // xyz = unit vector toward the sun, world space. w = its strength, 0 at
    // night and 1 at noon.
    sun: vec4<f32>,
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

// The face normal, recovered from how world position changes across the
// triangle rather than stored per vertex.
//
// Two neighbouring pixels on a flat quad differ in world position only along
// that quad's plane, so the cross product of those two differences is the
// plane's normal. Getting it this way costs two instructions and, crucially,
// no vertex data: the alternative is a normal on every vertex, which is another
// twelve bytes across a million triangles and a re-mesh of the world to add.
//
// Back faces are culled, so the visible side always points at the camera, and
// flipping it to face the camera makes the double-sided plant quads work too.
fn face_normal(world_pos: vec3<f32>) -> vec3<f32> {
    let n = normalize(cross(dpdx(world_pos), dpdy(world_pos)));
    let to_cam = camera.cam_pos - world_pos;
    return select(-n, n, dot(n, to_cam) >= 0.0);
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

    // --- directional sun ----------------------------------------------------
    //
    // `in.light` is what the mesher baked: per-axis face shade, ambient
    // occlusion and voxel light. It says how much light reaches a surface but
    // nothing about which way that light comes from, which is why a mountainside
    // used to read as flat noise -- every slope of the same block came out the
    // same brightness whatever its aspect. The sun term below is what turns a
    // slope into a slope, and because the direction comes from the time of day,
    // the relief moves across the land as the day passes.
    let n = face_normal(in.world_pos);
    let sun_dir = normalize(camera.sun.xyz);
    let day = camera.sun.w;
    let direct = max(dot(n, sun_dir), 0.0) * day;
    // Hemisphere ambient: upward faces see more sky than downward ones. This is
    // what keeps a shadowed face from going flat black, the way a single
    // directional light with no ambient always does.
    let sky_amt = 0.5 + 0.5 * n.y;
    let shade = 0.72 + 0.20 * sky_amt + 0.34 * direct;

    let lit = albedo * in.light * shade;

    // --- atmosphere ---------------------------------------------------------
    //
    // Horizontal-only distance, so looking up or down does not slide the fog band.
    let d = length(in.world_pos.xz - camera.cam_pos.xz);
    let fog = clamp((d - camera.fog_start) / (camera.fog_end - camera.fog_start), 0.0, 1.0);

    // Distance haze picks up the sun's colour when you look toward it, which is
    // most of what makes far-off terrain feel far off rather than merely faded.
    let view = normalize(in.world_pos - camera.cam_pos);
    let toward_sun = pow(max(dot(view, sun_dir), 0.0), 8.0) * day;
    let fog_color = camera.sky_color * mix(vec3<f32>(1.0), vec3<f32>(1.35, 1.18, 0.92), toward_sun);

    var rgb = mix(lit, fog_color, fog);

    // A gentle shoulder on the highlights. Without it, bright snow in full sun
    // clips to flat white and loses the shape the sun term just gave it.
    rgb = rgb / (1.0 + rgb * 0.16);

    if (camera.params.x > 0.5) {
        rgb = linear_to_srgb(rgb);
    }
    return vec4<f32>(rgb, 1.0);
}
