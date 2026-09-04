// 2D HUD overlay.
//
// Vertices arrive in pixel coordinates with the origin at the top-left of the
// window; the vertex stage maps them into clip space using the screen size
// uniform, so the CPU side never has to know about NDC.
//
// Every quad -- solid rectangle or glyph -- samples the same R8 font atlas.
// Solid rectangles point at a cell that is entirely white, so one pipeline and
// one draw call cover the whole overlay.

struct Screen {
    // Framebuffer size in physical pixels.
    size: vec2<f32>,
    // Padding so the struct is a multiple of 16 bytes.
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var font_tex: texture_2d<f32>;
@group(0) @binding(2) var font_sampler: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(vin: VsIn) -> VsOut {
    var vout: VsOut;
    let ndc = vec2<f32>(
        vin.pos.x / screen.size.x * 2.0 - 1.0,
        1.0 - vin.pos.y / screen.size.y * 2.0,
    );
    // z = 0 keeps the overlay inside the depth range even though the pipeline
    // neither tests nor writes depth.
    vout.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    vout.uv = vin.uv;
    vout.color = vin.color;
    return vout;
}

@fragment
fn fs_main(vout: VsOut) -> @location(0) vec4<f32> {
    let mask = textureSample(font_tex, font_sampler, vout.uv).r;
    let a = vout.color.a * mask;
    if a <= 0.0 {
        discard;
    }
    // Straight (non-premultiplied) alpha, to match BlendState::ALPHA_BLENDING.
    return vec4<f32>(vout.color.rgb, a);
}
