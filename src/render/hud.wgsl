// 2D HUD overlay.
//
// Vertices arrive in pixel coordinates with the origin at the top-left of the
// window; the vertex stage maps them into clip space using the screen size
// uniform, so the CPU side never has to know about NDC.
//
// A quad samples one of two atlases, chosen per vertex by `mode`: the R8 font
// atlas, where the texel is an alpha mask multiplied by the vertex colour, or
// the RGBA block atlas, where the texel is the colour. Solid rectangles are the
// font path pointing at a cell that is entirely white, so one pipeline and one
// draw call still cover the whole overlay.
//
// The second atlas is why inventory icons exist at all: the overlay could
// previously only draw flat rectangles, so every item in every slot was a
// coloured square and the whole item art in the atlas was never once shown.

struct Screen {
    // Framebuffer size in physical pixels.
    size: vec2<f32>,
    // Padding so the struct is a multiple of 16 bytes.
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var font_tex: texture_2d<f32>;
@group(0) @binding(2) var font_sampler: sampler;
@group(0) @binding(3) var atlas_tex: texture_2d<f32>;
@group(0) @binding(4) var atlas_sampler: sampler;

struct VsIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // 0 samples the font mask, 1 samples the block atlas.
    @location(3) mode: f32,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) mode: f32,
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
    vout.mode = vin.mode;
    return vout;
}

@fragment
fn fs_main(vout: VsOut) -> @location(0) vec4<f32> {
    if (vout.mode > 0.5) {
        // Item art. The vertex colour tints it, so one grass tile can still be
        // recoloured per biome the way the world does it.
        let tex = textureSample(atlas_tex, atlas_sampler, vout.uv);
        let a = tex.a * vout.color.a;
        if (a <= 0.01) {
            discard;
        }
        return vec4<f32>(tex.rgb * vout.color.rgb, a);
    }
    let mask = textureSample(font_tex, font_sampler, vout.uv).r;
    let a = vout.color.a * mask;
    if a <= 0.0 {
        discard;
    }
    // Straight (non-premultiplied) alpha, to match BlendState::ALPHA_BLENDING.
    return vec4<f32>(vout.color.rgb, a);
}
