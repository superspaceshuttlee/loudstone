//! 2D HUD overlay renderer.
//!
//! Self-contained: one pipeline, one growable vertex buffer, one bind group.
//! Geometry is rebuilt from plain data every frame -- there is no retained
//! widget tree and no state the caller has to keep in sync.
//!
//! Usage per frame:
//!
//! ```ignore
//! hud.begin(w, h);
//! hud.crosshair();
//! hud.hotbar(&slots, selected);
//! hud.health(hp, max_hp);
//! hud.readout(player_pos.into(), fps, "Stone");
//! // ... inside the render pass, after terrain:
//! hud.draw(&device, &queue, &mut pass);
//! ```
//!
//! Coordinates are physical pixels with the origin at the top-left. Depth
//! testing is off and alpha blending is on, so the overlay draws over whatever
//! the terrain pass left in the colour attachment.

// This module is a self-contained API surface rather than glue: the game does
// not have to call every entry point for the rest of them to be useful.
#![allow(dead_code)]

// ---------------------------------------------------------------------------
// Public data
// ---------------------------------------------------------------------------

/// Depth format the HUD pipeline declares so it is compatible with the
/// terrain pass's depth attachment. Must match the renderer's depth texture.
/// The HUD neither tests nor writes depth; it only has to be *compatible*.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Number of hotbar slots.
pub const HOTBAR_SLOTS: usize = 9;

/// Edge length of a hotbar slot, in pixels.
pub const SLOT_SIZE: f32 = 46.0;
/// Gap between adjacent slots, in pixels.
pub const SLOT_GAP: f32 = 4.0;
/// Margin between the HUD and the window edge, in pixels.
pub const MARGIN: f32 = 12.0;
/// Default text size (glyph cell height) in pixels.
pub const TEXT_SIZE: f32 = 14.0;

/// One inventory cell, as the HUD needs to see it.
///
/// Gameplay code owns the real item stack; this is the flattened view of it.
/// `color` is normally `BlockId::color()` of the held block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// Which atlas tile to draw. For a block item this is an isometric cube
    /// built from that block's own faces; for anything else it is the item's
    /// own icon.
    pub tile: crate::render::texture::TileId,
    /// Tint multiplied over the art. Left white for most things; biome-tinted
    /// grass uses it the same way the world does.
    pub color: [f32; 3],
    /// Stack count. Drawn in the corner when greater than one.
    pub count: u16,
}

impl Slot {
    /// Convenience constructor.
    pub fn new(tile: crate::render::texture::TileId, color: [f32; 3], count: u16) -> Self {
        Self { tile, color, count }
    }
}

// Palette. Deliberately flat and dark -- visuals are not this project's point.
const C_PANEL: [f32; 4] = [0.05, 0.05, 0.07, 0.78];
const C_PANEL_EDGE: [f32; 4] = [0.75, 0.75, 0.80, 0.35];
const C_SLOT: [f32; 4] = [0.17, 0.17, 0.21, 0.85];
const C_SLOT_EDGE: [f32; 4] = [0.02, 0.02, 0.03, 0.9];
const C_SELECT: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
const C_TEXT: [f32; 4] = [0.94, 0.94, 0.96, 1.0];
const C_SHADOW: [f32; 4] = [0.0, 0.0, 0.0, 0.85];
const C_CROSSHAIR: [f32; 4] = [1.0, 1.0, 1.0, 0.85];
const C_CROSSHAIR_EDGE: [f32; 4] = [0.0, 0.0, 0.0, 0.55];
const C_HEART: [f32; 4] = [0.86, 0.18, 0.20, 1.0];
const C_HEART_EMPTY: [f32; 4] = [0.10, 0.10, 0.12, 0.85];
const C_DIM: [f32; 4] = [0.0, 0.0, 0.0, 0.55];

// ---------------------------------------------------------------------------
// Bitmap font
// ---------------------------------------------------------------------------

// 5x7 glyphs for ASCII 32..=126, written as pixel art so they can be read and
// corrected by eye. `build_font` folds them into a bit table at compile time,
// and a wrong-length row is a compile error rather than a runtime surprise.
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const GLYPH_ROW_STRIDE: usize = GLYPH_W + 1; // 5 pixels plus the '/' separator
const FONT_ART_LEN: usize = GLYPH_H * GLYPH_ROW_STRIDE - 1; // 41

#[rustfmt::skip]
const FONT_ART: [&str; 95] = [
    "...../...../...../...../...../...../.....", // space
    "..#../..#../..#../..#../..#../...../..#..", // !
    ".#.#./.#.#./...../...../...../...../.....", // "
    ".#.#./.#.#./#####/.#.#./#####/.#.#./.#.#.", // #
    "..#../.####/#.#../.###./..#.#/####./..#..", // $
    "##.../##..#/...#./..#../.#.../#..##/...##", // %
    ".##../#..#./#.#../.#.../#.#.#/#..#./.##.#", // &
    "..#../..#../...../...../...../...../.....", // '
    "...#./..#../.#.../.#.../.#.../..#../...#.", // (
    ".#.../..#../...#./...#./...#./..#../.#...", // )
    "...../#.#.#/.###./#####/.###./#.#.#/.....", // *
    "...../..#../..#../#####/..#../..#../.....", // +
    "...../...../...../...../..##./..#../.#...", // ,
    "...../...../...../#####/...../...../.....", // -
    "...../...../...../...../...../.##../.##..", // .
    "....#/...#./..#../..#../.#.../#..../.....", // /
    ".###./#...#/#..##/#.#.#/##..#/#...#/.###.", // 0
    "..#../.##../..#../..#../..#../..#../.###.", // 1
    ".###./#...#/....#/...#./..#../.#.../#####", // 2
    "#####/...#./..#../...#./....#/#...#/.###.", // 3
    "...#./..##./.#.#./#..#./#####/...#./...#.", // 4
    "#####/#..../####./....#/....#/#...#/.###.", // 5
    "..##./.#.../#..../####./#...#/#...#/.###.", // 6
    "#####/....#/...#./..#../.#.../.#.../.#...", // 7
    ".###./#...#/#...#/.###./#...#/#...#/.###.", // 8
    ".###./#...#/#...#/.####/....#/...#./.##..", // 9
    "...../.##../.##../...../.##../.##../.....", // :
    "...../.##../.##../...../.##../..#../.#...", // ;
    "...#./..#../.#.../#..../.#.../..#../...#.", // <
    "...../...../#####/...../#####/...../.....", // =
    ".#.../..#../...#./....#/...#./..#../.#...", // >
    ".###./#...#/....#/...#./..#../...../..#..", // ?
    ".###./#...#/....#/#.##./#.#.#/#.#.#/.####", // @
    "..#../.#.#./#...#/#...#/#####/#...#/#...#", // A
    "####./#...#/#...#/####./#...#/#...#/####.", // B
    ".###./#...#/#..../#..../#..../#...#/.###.", // C
    "###../#..#./#...#/#...#/#...#/#..#./###..", // D
    "#####/#..../#..../####./#..../#..../#####", // E
    "#####/#..../#..../####./#..../#..../#....", // F
    ".###./#...#/#..../#.###/#...#/#...#/.####", // G
    "#...#/#...#/#...#/#####/#...#/#...#/#...#", // H
    ".###./..#../..#../..#../..#../..#../.###.", // I
    "..###/...#./...#./...#./...#./#..#./.##..", // J
    "#...#/#..#./#.#../##.../#.#../#..#./#...#", // K
    "#..../#..../#..../#..../#..../#..../#####", // L
    "#...#/##.##/#.#.#/#.#.#/#...#/#...#/#...#", // M
    "#...#/#...#/##..#/#.#.#/#..##/#...#/#...#", // N
    ".###./#...#/#...#/#...#/#...#/#...#/.###.", // O
    "####./#...#/#...#/####./#..../#..../#....", // P
    ".###./#...#/#...#/#...#/#.#.#/#..#./.##.#", // Q
    "####./#...#/#...#/####./#.#../#..#./#...#", // R
    ".####/#..../#..../.###./....#/....#/####.", // S
    "#####/..#../..#../..#../..#../..#../..#..", // T
    "#...#/#...#/#...#/#...#/#...#/#...#/.###.", // U
    "#...#/#...#/#...#/#...#/#...#/.#.#./..#..", // V
    "#...#/#...#/#...#/#.#.#/#.#.#/##.##/#...#", // W
    "#...#/#...#/.#.#./..#../.#.#./#...#/#...#", // X
    "#...#/#...#/.#.#./..#../..#../..#../..#..", // Y
    "#####/....#/...#./..#../.#.../#..../#####", // Z
    ".###./.#.../.#.../.#.../.#.../.#.../.###.", // [
    "#..../.#.../.#.../..#../...#./...#./....#", // backslash
    ".###./...#./...#./...#./...#./...#./.###.", // ]
    "..#../.#.#./#...#/...../...../...../.....", // ^
    "...../...../...../...../...../...../#####", // _
    ".#.../..#../...../...../...../...../.....", // `
    "...../...../.###./....#/.####/#...#/.####", // a
    "#..../#..../####./#...#/#...#/#...#/####.", // b
    "...../...../.####/#..../#..../#..../.####", // c
    "....#/....#/.####/#...#/#...#/#...#/.####", // d
    "...../...../.###./#...#/#####/#..../.###.", // e
    "..##./.#..#/.#.../####./.#.../.#.../.#...", // f
    "...../.####/#...#/#...#/.####/....#/.###.", // g
    "#..../#..../####./#...#/#...#/#...#/#...#", // h
    "..#../...../.##../..#../..#../..#../.###.", // i
    "...#./...../...#./...#./...#./#..#./.##..", // j
    "#..../#..../#..#./#.#../##.../#.#../#..#.", // k
    ".##../..#../..#../..#../..#../..#../.###.", // l
    "...../...../##.#./#.#.#/#.#.#/#.#.#/#.#.#", // m
    "...../...../####./#...#/#...#/#...#/#...#", // n
    "...../...../.###./#...#/#...#/#...#/.###.", // o
    "...../####./#...#/#...#/####./#..../#....", // p
    "...../.####/#...#/#...#/.####/....#/....#", // q
    "...../...../#.##./##..#/#..../#..../#....", // r
    "...../...../.####/#..../.###./....#/####.", // s
    ".#.../.#.../####./.#.../.#.../.#..#/..##.", // t
    "...../...../#...#/#...#/#...#/#...#/.####", // u
    "...../...../#...#/#...#/#...#/.#.#./..#..", // v
    "...../...../#...#/#.#.#/#.#.#/#.#.#/.#.#.", // w
    "...../...../#...#/.#.#./..#../.#.#./#...#", // x
    "...../#...#/#...#/#...#/.####/....#/.###.", // y
    "...../...../#####/...#./..#../.#.../#####", // z
    "...##/..#../..#../.#.../..#../..#../...##", // {
    "..#../..#../..#../..#../..#../..#../..#..", // |
    "##.../..#../..#../...#./..#../..#../##...", // }
    "...../...../.#..#/#.#.#/#..#./...../.....", // ~
];

/// Bit table for the font: one `u8` per glyph row, bit 4 is the leftmost
/// column. Built from `FONT_ART` at compile time.
const FONT: [[u8; GLYPH_H]; 95] = build_font();

const fn build_font() -> [[u8; GLYPH_H]; 95] {
    let mut out = [[0u8; GLYPH_H]; 95];
    let mut g = 0;
    while g < 95 {
        let art = FONT_ART[g].as_bytes();
        if art.len() != FONT_ART_LEN {
            panic!("font glyph art must be 7 rows of 5 pixels separated by '/'");
        }
        let mut r = 0;
        while r < GLYPH_H {
            let mut bits = 0u8;
            let mut c = 0;
            while c < GLYPH_W {
                if art[r * GLYPH_ROW_STRIDE + c] == b'#' {
                    bits |= 1 << (GLYPH_W - 1 - c);
                }
                c += 1;
            }
            out[g][r] = bits;
            r += 1;
        }
        g += 1;
    }
    out
}

// Atlas: 32 x 3 cells of 8x8 texels. 95 glyphs fill cells 0..=94 and cell 95 is
// solid white, which is what solid rectangles sample. 256 bytes per row keeps
// the upload trivially aligned.
const ATLAS_COLS: usize = 32;
const ATLAS_ROWS: usize = 3;
const CELL: usize = 8;
const ATLAS_W: usize = ATLAS_COLS * CELL;
const ATLAS_H: usize = ATLAS_ROWS * CELL;
const WHITE_CELL: usize = 95;

/// Rasterise the font into an R8 atlas.
fn build_atlas() -> Vec<u8> {
    let mut px = vec![0u8; ATLAS_W * ATLAS_H];
    for (g, glyph) in FONT.iter().enumerate() {
        let ox = (g % ATLAS_COLS) * CELL;
        let oy = (g / ATLAS_COLS) * CELL;
        for (r, bits) in glyph.iter().enumerate() {
            for c in 0..GLYPH_W {
                if (bits >> (GLYPH_W - 1 - c)) & 1 == 1 {
                    px[(oy + r) * ATLAS_W + ox + c] = 255;
                }
            }
        }
    }
    let ox = (WHITE_CELL % ATLAS_COLS) * CELL;
    let oy = (WHITE_CELL / ATLAS_COLS) * CELL;
    for r in 0..CELL {
        for c in 0..CELL {
            px[(oy + r) * ATLAS_W + ox + c] = 255;
        }
    }
    px
}

/// UV rect `[u0, v0, u1, v1]` covering the 5x7 glyph inside its cell.
fn glyph_uv(g: usize) -> [f32; 4] {
    let ox = (g % ATLAS_COLS) * CELL;
    let oy = (g / ATLAS_COLS) * CELL;
    [
        ox as f32 / ATLAS_W as f32,
        oy as f32 / ATLAS_H as f32,
        (ox + GLYPH_W) as f32 / ATLAS_W as f32,
        (oy + GLYPH_H) as f32 / ATLAS_H as f32,
    ]
}

/// A degenerate UV rect sitting in the middle of the solid white cell. Using a
/// single point rather than a rect means nearest sampling can never stray into
/// a neighbouring cell no matter how a rectangle is scaled.
fn white_uv() -> [f32; 4] {
    let ox = (WHITE_CELL % ATLAS_COLS) * CELL + CELL / 2;
    let oy = (WHITE_CELL / ATLAS_COLS) * CELL + CELL / 2;
    let u = (ox as f32 + 0.5) / ATLAS_W as f32;
    let v = (oy as f32 + 0.5) / ATLAS_H as f32;
    [u, v, u, v]
}

/// Glyph index for a character, falling back to `?` for anything unprintable.
fn glyph_index(c: char) -> usize {
    let u = c as u32;
    if (32..=126).contains(&u) {
        (u - 32) as usize
    } else {
        ('?' as usize) - 32
    }
}

/// Pixel size of one font pixel at the given glyph cell height.
fn font_scale(size: f32) -> f32 {
    size / GLYPH_H as f32
}

/// Width in pixels of `s` drawn at `size`, excluding the trailing gap.
pub fn text_width(size: f32, s: &str) -> f32 {
    let n = s.chars().count();
    if n == 0 {
        return 0.0;
    }
    let px = font_scale(size);
    (n as f32 * (GLYPH_W + 1) as f32 - 1.0) * px
}

/// Sensible baseline-to-baseline spacing for stacked lines of text.
pub fn line_height(size: f32) -> f32 {
    size * 1.45
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct HudVertex {
    /// Pixel position, origin top-left.
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    /// 0 samples the font mask, 1 samples the block atlas. Per-vertex rather
    /// than a second pipeline so the overlay is still one draw call.
    mode: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ScreenUniform {
    size: [f32; 2],
    pad: [f32; 2],
}

/// Pure-CPU overlay geometry. Split out from `Hud` so the layout logic is
/// testable without a GPU device.
struct Batch {
    verts: Vec<HudVertex>,
    screen: [f32; 2],
}

impl Batch {
    fn new() -> Self {
        Self {
            verts: Vec::with_capacity(4096),
            screen: [1.0, 1.0],
        }
    }

    fn begin(&mut self, screen_w: f32, screen_h: f32) {
        self.verts.clear();
        // A zeroed screen (minimised window) would divide by zero in the
        // vertex shader, so clamp rather than trusting the caller.
        self.screen = [screen_w.max(1.0), screen_h.max(1.0)];
    }

    fn quad(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], color: [f32; 4]) {
        self.quad_mode(x, y, w, h, uv, color, 0.0);
    }

    /// A quad sampling the block atlas rather than the font mask.
    fn tile_quad(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        tile: crate::render::texture::TileId,
        tint: [f32; 4],
    ) {
        let r = crate::render::texture::tile_uv_rect(tile);
        // Half-texel inset. Nearest sampling at a tile's exact edge can land on
        // the neighbouring tile, which shows up as a stray line of some other
        // block down one side of an icon.
        let e = 0.5 / crate::render::texture::ATLAS_W as f32;
        self.quad_mode(
            x,
            y,
            w,
            h,
            [r[0] + e, r[1] + e, r[2] - e, r[3] - e],
            tint,
            1.0,
        );
    }

    fn quad_mode(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        uv: [f32; 4],
        color: [f32; 4],
        mode: f32,
    ) {
        if w <= 0.0 || h <= 0.0 || color[3] <= 0.0 {
            return;
        }
        let (x0, y0, x1, y1) = (x, y, x + w, y + h);
        let (u0, v0, u1, v1) = (uv[0], uv[1], uv[2], uv[3]);
        let tl = HudVertex {
            pos: [x0, y0],
            uv: [u0, v0],
            color,
            mode,
        };
        let tr = HudVertex {
            pos: [x1, y0],
            uv: [u1, v0],
            color,
            mode,
        };
        let br = HudVertex {
            pos: [x1, y1],
            uv: [u1, v1],
            color,
            mode,
        };
        let bl = HudVertex {
            pos: [x0, y1],
            uv: [u0, v1],
            color,
            mode,
        };
        self.verts.extend_from_slice(&[tl, tr, br, tl, br, bl]);
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        self.quad(x, y, w, h, white_uv(), color);
    }

    /// Hollow rectangle of thickness `t`, drawn inside the given bounds.
    fn border(&mut self, x: f32, y: f32, w: f32, h: f32, t: f32, color: [f32; 4]) {
        if w <= 0.0 || h <= 0.0 || t <= 0.0 {
            return;
        }
        let t = t.min(w * 0.5).min(h * 0.5);
        self.rect(x, y, w, t, color);
        self.rect(x, y + h - t, w, t, color);
        self.rect(x, y + t, t, h - 2.0 * t, color);
        self.rect(x + w - t, y + t, t, h - 2.0 * t, color);
    }

    fn text(&mut self, x: f32, y: f32, size: f32, color: [f32; 4], s: &str) {
        if color[3] <= 0.0 || size <= 0.0 {
            return;
        }
        let px = font_scale(size);
        let advance = (GLYPH_W + 1) as f32 * px;
        let gw = GLYPH_W as f32 * px;
        let gh = GLYPH_H as f32 * px;
        let mut cursor = x;
        for c in s.chars() {
            if c == ' ' {
                cursor += advance;
                continue;
            }
            let g = glyph_index(c);
            self.quad(cursor, y, gw, gh, glyph_uv(g), color);
            cursor += advance;
        }
    }

    /// Text with a one-pixel drop shadow, so it stays readable over terrain.
    fn text_shadowed(&mut self, x: f32, y: f32, size: f32, color: [f32; 4], s: &str) {
        let off = font_scale(size).max(1.0);
        self.text(x + off, y + off, size, C_SHADOW, s);
        self.text(x, y, size, color, s);
    }

    // -- composed widgets ---------------------------------------------------

    fn crosshair(&mut self) {
        let cx = (self.screen[0] * 0.5).round();
        let cy = (self.screen[1] * 0.5).round();
        let arm = 8.0;
        let th = 2.0;
        // Dark backing one pixel larger on every side, for contrast against
        // both bright sky and dark caves.
        self.rect(
            cx - arm - 1.0,
            cy - th * 0.5 - 1.0,
            arm * 2.0 + 2.0,
            th + 2.0,
            C_CROSSHAIR_EDGE,
        );
        self.rect(
            cx - th * 0.5 - 1.0,
            cy - arm - 1.0,
            th + 2.0,
            arm * 2.0 + 2.0,
            C_CROSSHAIR_EDGE,
        );
        self.rect(cx - arm, cy - th * 0.5, arm * 2.0, th, C_CROSSHAIR);
        self.rect(cx - th * 0.5, cy - arm, th, arm * 2.0, C_CROSSHAIR);
    }

    /// Bounds of the hotbar: `(x, y, w, h)`.
    fn hotbar_bounds(&self) -> (f32, f32, f32, f32) {
        let n = HOTBAR_SLOTS as f32;
        let w = n * SLOT_SIZE + (n - 1.0) * SLOT_GAP;
        let x = ((self.screen[0] - w) * 0.5).round();
        let y = (self.screen[1] - MARGIN - SLOT_SIZE).round();
        (x, y, w, SLOT_SIZE)
    }

    /// One inventory cell: background, item swatch, stack count, selection ring.
    fn slot(&mut self, x: f32, y: f32, size: f32, slot: Option<Slot>, selected: bool) {
        self.rect(x, y, size, size, C_SLOT);
        self.border(x, y, size, size, 1.0, C_SLOT_EDGE);

        if let Some(item) = slot {
            let inset = (size * 0.13).max(3.0);
            let sw = size - inset * 2.0;
            let c = item.color;
            self.tile_quad(
                x + inset,
                y + inset,
                sw,
                sw,
                item.tile,
                [c[0], c[1], c[2], 1.0],
            );
            // No painted band under the art: the icon is a lit cube and brings
            // its own depth. The band existed only because a slot used to be a
            // flat coloured square with nothing to read as a shape.

            if item.count > 1 {
                let ts = (size * 0.26).max(9.0);
                let label = item.count.to_string();
                let tw = text_width(ts, &label);
                self.text_shadowed(x + size - 3.0 - tw, y + size - 3.0 - ts, ts, C_TEXT, &label);
            }
        }

        if selected {
            let t = (size * 0.06).max(2.0);
            self.border(x - t, y - t, size + t * 2.0, size + t * 2.0, t, C_SELECT);
        }
    }

    fn hotbar(&mut self, slots: &[Option<Slot>], selected: usize) {
        let (x0, y0, w, h) = self.hotbar_bounds();
        let pad = 4.0;
        self.rect(x0 - pad, y0 - pad, w + pad * 2.0, h + pad * 2.0, C_PANEL);

        for i in 0..HOTBAR_SLOTS {
            let x = x0 + i as f32 * (SLOT_SIZE + SLOT_GAP);
            let item = slots.get(i).copied().flatten();
            self.slot(x, y0, SLOT_SIZE, item, i == selected);
        }
    }

    /// Ten segments above the hotbar, each worth a tenth of `max_hp`.
    /// A partially-lost segment drains from the right.
    fn health(&mut self, hp: f32, max_hp: f32) {
        let (x0, y0, _, _) = self.hotbar_bounds();
        let seg = 14.0;
        let gap = 3.0;
        let y = y0 - MARGIN - seg;
        let max_hp = if max_hp > 0.0 { max_hp } else { 1.0 };
        let filled = (hp.max(0.0) / max_hp * 10.0).min(10.0);

        for i in 0..10 {
            let x = x0 + i as f32 * (seg + gap);
            self.rect(x, y, seg, seg, C_HEART_EMPTY);
            let frac = (filled - i as f32).clamp(0.0, 1.0);
            if frac > 0.0 {
                let inset = 2.0;
                let inner = seg - inset * 2.0;
                self.rect(x + inset, y + inset, inner * frac, inner, C_HEART);
            }
            self.border(x, y, seg, seg, 1.0, C_SLOT_EDGE);
        }
    }

    /// Debug readout in the top-left corner.
    fn readout(&mut self, pos: [f32; 3], fps: f32, held: &str) {
        let size = TEXT_SIZE;
        let lh = line_height(size);
        let x = MARGIN;
        let mut y = MARGIN;
        let lines = [
            format!("XYZ {:.1} {:.1} {:.1}", pos[0], pos[1], pos[2]),
            format!("FPS {fps:.0}"),
            format!("HELD {held}"),
        ];
        for line in &lines {
            self.text_shadowed(x, y, size, C_TEXT, line);
            y += lh;
        }
    }

    /// Full-screen scrim, for when a modal panel is open.
    fn screen_dim(&mut self) {
        self.rect(0.0, 0.0, self.screen[0], self.screen[1], C_DIM);
    }

    /// Translucent panel background with a light edge.
    fn panel(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rect(x, y, w, h, C_PANEL);
        self.border(x, y, w, h, 1.0, C_PANEL_EDGE);
    }

    /// A `cols` x `rows` grid of slots with its top-left corner at `origin`.
    /// `slots` is read in row-major order; a short slice leaves the remaining
    /// cells empty, so callers can pass whatever their inventory model holds.
    fn slot_grid(
        &mut self,
        origin: [f32; 2],
        cols: usize,
        rows: usize,
        cell: f32,
        gap: f32,
        slots: &[Option<Slot>],
    ) {
        for r in 0..rows {
            for c in 0..cols {
                let x = origin[0] + c as f32 * (cell + gap);
                let y = origin[1] + r as f32 * (cell + gap);
                let item = slots.get(r * cols + c).copied().flatten();
                self.slot(x, y, cell, item, false);
            }
        }
    }
}

/// Total width of a `cols`-wide slot grid, including gaps.
pub fn grid_width(cols: usize, cell: f32, gap: f32) -> f32 {
    if cols == 0 {
        0.0
    } else {
        cols as f32 * cell + (cols - 1) as f32 * gap
    }
}

/// Total height of a `rows`-tall slot grid, including gaps.
pub fn grid_height(rows: usize, cell: f32, gap: f32) -> f32 {
    grid_width(rows, cell, gap)
}

// ---------------------------------------------------------------------------
// Renderer
// ---------------------------------------------------------------------------

const VERTEX_SIZE: usize = std::mem::size_of::<HudVertex>();
const INITIAL_VERTS: usize = 4096;

/// The HUD overlay renderer.
pub struct Hud {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    screen_buf: wgpu::Buffer,
    font_tex: wgpu::Texture,
    font_uploaded: bool,
    /// The block atlas, for item art. Uploaded with the font on the first frame.
    item_tex: wgpu::Texture,
    vbuf: wgpu::Buffer,
    vbuf_cap: usize,
    batch: Batch,
}

impl Hud {
    /// Build the overlay pipeline for a colour target of `format`, assuming the
    /// pass it draws into has a [`DEPTH_FORMAT`] depth attachment (as the
    /// terrain pass does). Use [`Hud::with_depth`] for any other arrangement.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_depth(device, format, Some(DEPTH_FORMAT))
    }

    /// As [`Hud::new`], but spelling out the depth attachment of the render
    /// pass the HUD will be recorded into. Pass `None` when that pass has no
    /// depth attachment at all. The HUD never tests or writes depth either way;
    /// the pipeline only has to match the pass it is used in.
    pub fn with_depth(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hud shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hud.wgsl").into()),
        });

        let screen_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud screen uniform"),
            size: std::mem::size_of::<ScreenUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let font_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud font atlas"),
            size: wgpu::Extent3d {
                width: ATLAS_W as u32,
                height: ATLAS_H as u32,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let font_view = font_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hud font sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hud bind layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // The block atlas, so the overlay can draw item art. Only the top mip
        // is uploaded: a HUD icon is drawn at a fixed size and never minified,
        // so the rest of the chain would be dead weight.
        let (aw, ah) = (
            crate::render::texture::ATLAS_W as u32,
            crate::render::texture::ATLAS_H as u32,
        );
        let atlas_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hud item atlas"),
            size: wgpu::Extent3d {
                width: aw,
                height: ah,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let atlas_view = atlas_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hud bind group"),
            layout: &bind_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: screen_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&font_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hud pipeline layout"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let attributes = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 8,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 16,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 32,
                shader_location: 3,
            },
        ];

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("hud pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: VERTEX_SIZE as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                // Overlay quads are 2D; culling would only be a way to get
                // winding wrong.
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                ..Default::default()
            },
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hud vertices"),
            size: (INITIAL_VERTS * VERTEX_SIZE) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group,
            screen_buf,
            font_tex,
            font_uploaded: false,
            item_tex: atlas_tex,
            vbuf,
            vbuf_cap: INITIAL_VERTS,
            batch: Batch::new(),
        }
    }

    // -- per-frame geometry -------------------------------------------------

    /// Start a frame. Clears last frame's geometry and records the framebuffer
    /// size that pixel coordinates are relative to.
    pub fn begin(&mut self, screen_w: f32, screen_h: f32) {
        self.batch.begin(screen_w, screen_h);
    }

    /// Solid rectangle in pixels, origin top-left.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
        self.batch.rect(x, y, w, h, color);
    }

    /// Hollow rectangle of thickness `t`, drawn inside the given bounds.
    pub fn border(&mut self, x: f32, y: f32, w: f32, h: f32, t: f32, color: [f32; 4]) {
        self.batch.border(x, y, w, h, t, color);
    }

    /// Draw `s` with its top-left corner at `(x, y)`. `size` is the glyph cell
    /// height in pixels; use [`text_width`] to measure before placing.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: [f32; 4], s: &str) {
        self.batch.text(x, y, size, color, s);
    }

    /// As [`Hud::text`], with a drop shadow for legibility over terrain.
    pub fn text_shadowed(&mut self, x: f32, y: f32, size: f32, color: [f32; 4], s: &str) {
        self.batch.text_shadowed(x, y, size, color, s);
    }

    /// Crosshair at the centre of the screen.
    pub fn crosshair(&mut self) {
        self.batch.crosshair();
    }

    /// Nine-slot hotbar along the bottom edge, `selected` highlighted.
    pub fn hotbar(&mut self, slots: &[Option<Slot>], selected: usize) {
        self.batch.hotbar(slots, selected);
    }

    /// Ten health segments above the hotbar.
    pub fn health(&mut self, hp: f32, max_hp: f32) {
        self.batch.health(hp, max_hp);
    }

    /// Position / fps / held-item readout in the top-left corner.
    pub fn readout(&mut self, pos: [f32; 3], fps: f32, held: &str) {
        self.batch.readout(pos, fps, held);
    }

    /// Full-screen scrim to sit behind a modal panel.
    pub fn screen_dim(&mut self) {
        self.batch.screen_dim();
    }

    /// Translucent panel background with a light edge.
    pub fn panel(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.batch.panel(x, y, w, h);
    }

    /// A `cols` x `rows` grid of inventory slots at an arbitrary position and
    /// cell size. `slots` is row-major and may be shorter than `cols * rows`.
    pub fn slot_grid(
        &mut self,
        origin: [f32; 2],
        cols: usize,
        rows: usize,
        cell: f32,
        gap: f32,
        slots: &[Option<Slot>],
    ) {
        self.batch.slot_grid(origin, cols, rows, cell, gap, slots);
    }

    /// A single slot, for anything the composed widgets do not cover (a crafting
    /// output cell, the cursor-held stack, and so on).
    pub fn slot(&mut self, x: f32, y: f32, size: f32, slot: Option<Slot>, selected: bool) {
        self.batch.slot(x, y, size, slot, selected);
    }

    /// Bounds of the hotbar for the current screen size, as `(x, y, w, h)`.
    /// Useful for anchoring extra HUD elements to it.
    pub fn hotbar_bounds(&self) -> (f32, f32, f32, f32) {
        self.batch.hotbar_bounds()
    }

    /// Screen size recorded by the last [`Hud::begin`].
    pub fn screen(&self) -> [f32; 2] {
        self.batch.screen
    }

    /// Number of vertices queued this frame. Exposed for debugging.
    pub fn vertex_count(&self) -> usize {
        self.batch.verts.len()
    }

    // -- upload and draw ----------------------------------------------------

    /// Upload this frame's geometry and record the overlay draw.
    ///
    /// Call inside the frame's render pass, after the terrain draws. The pass
    /// must target the colour format given to [`Hud::new`] and have a matching
    /// depth attachment. Uploads happen through `Queue::write_*`, which lands
    /// before the encoder being recorded is submitted.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        // Neither atlas ever changes, so both are uploaded once, on the first
        // frame. Doing it here rather than in `new` keeps the constructor free
        // of a queue argument.
        if !self.font_uploaded {
            let items = crate::render::texture::atlas();
            let (iw, ih, ref ipx) = items.levels[0];
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.item_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                ipx,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(iw * 4),
                    rows_per_image: Some(ih),
                },
                wgpu::Extent3d {
                    width: iw,
                    height: ih,
                    depth_or_array_layers: 1,
                },
            );

            let pixels = build_atlas();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.font_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(ATLAS_W as u32),
                    rows_per_image: Some(ATLAS_H as u32),
                },
                wgpu::Extent3d {
                    width: ATLAS_W as u32,
                    height: ATLAS_H as u32,
                    depth_or_array_layers: 1,
                },
            );
            self.font_uploaded = true;
        }

        let count = self.batch.verts.len();
        if count == 0 {
            return;
        }

        queue.write_buffer(
            &self.screen_buf,
            0,
            bytemuck::bytes_of(&ScreenUniform {
                size: self.batch.screen,
                pad: [0.0, 0.0],
            }),
        );

        // One buffer for the whole overlay, grown in place when a frame needs
        // more room than the last one did.
        if count > self.vbuf_cap {
            let cap = count.next_power_of_two();
            self.vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("hud vertices"),
                size: (cap * VERTEX_SIZE) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vbuf_cap = cap;
        }
        queue.write_buffer(&self.vbuf, 0, bytemuck::cast_slice(&self.batch.verts));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.draw(0..count as u32, 0..1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const VERTS_PER_QUAD: usize = 6;

    fn batch(w: f32, h: f32) -> Batch {
        let mut b = Batch::new();
        b.begin(w, h);
        b
    }

    /// Mirror of the vertex shader, so the mapping is checked on the CPU.
    fn to_clip(v: HudVertex, screen: [f32; 2]) -> [f32; 2] {
        [
            v.pos[0] / screen[0] * 2.0 - 1.0,
            1.0 - v.pos[1] / screen[1] * 2.0,
        ]
    }

    #[test]
    fn font_table_covers_printable_ascii() {
        assert_eq!(FONT.len(), 95);
        // Space is blank, every other glyph has at least one lit pixel.
        assert!(FONT[0].iter().all(|&r| r == 0));
        for (i, glyph) in FONT.iter().enumerate().skip(1) {
            assert!(glyph.iter().any(|&r| r != 0), "glyph {i} is blank");
            // Only the low five bits may be set.
            assert!(
                glyph.iter().all(|&r| r < 32),
                "glyph {i} overflows 5 columns"
            );
        }
    }

    #[test]
    fn glyph_index_maps_and_falls_back() {
        assert_eq!(glyph_index(' '), 0);
        assert_eq!(glyph_index('A'), 33);
        assert_eq!(glyph_index('~'), 94);
        // Anything outside printable ASCII renders as '?'.
        assert_eq!(glyph_index('\u{e9}'), glyph_index('?'));
        assert_eq!(glyph_index('\n'), glyph_index('?'));
    }

    #[test]
    fn atlas_holds_glyphs_and_a_white_cell() {
        let px = build_atlas();
        assert_eq!(px.len(), ATLAS_W * ATLAS_H);

        // 'A' has a lit pixel in the middle of its top row.
        let g = glyph_index('A');
        let ox = (g % ATLAS_COLS) * CELL;
        let oy = (g / ATLAS_COLS) * CELL;
        assert_eq!(px[oy * ATLAS_W + ox + 2], 255);
        // The unused columns of every cell stay clear, so nearest sampling at
        // a glyph's right edge cannot pick up its neighbour.
        assert_eq!(px[oy * ATLAS_W + ox + 5], 0);

        let wx = (WHITE_CELL % ATLAS_COLS) * CELL;
        let wy = (WHITE_CELL / ATLAS_COLS) * CELL;
        for r in 0..CELL {
            for c in 0..CELL {
                assert_eq!(px[(wy + r) * ATLAS_W + wx + c], 255);
            }
        }
    }

    #[test]
    fn white_uv_samples_inside_the_white_cell() {
        let uv = white_uv();
        assert_eq!(uv[0], uv[2], "white uv must be a single point");
        assert_eq!(uv[1], uv[3]);
        let tx = (uv[0] * ATLAS_W as f32).floor() as usize;
        let ty = (uv[1] * ATLAS_H as f32).floor() as usize;
        assert_eq!(tx / CELL, WHITE_CELL % ATLAS_COLS);
        assert_eq!(ty / CELL, WHITE_CELL / ATLAS_COLS);
    }

    #[test]
    fn text_width_counts_glyphs_and_gaps() {
        // Three glyphs at size 14 -> font pixel 2 -> 3*6-1 columns * 2.
        assert!((text_width(14.0, "ABC") - 34.0).abs() < 1e-4);
        assert_eq!(text_width(14.0, ""), 0.0);
        // Spaces still advance.
        assert!(text_width(14.0, "A B") > text_width(14.0, "AB"));
    }

    #[test]
    fn begin_clears_and_clamps() {
        let mut b = batch(800.0, 600.0);
        b.rect(0.0, 0.0, 10.0, 10.0, [1.0; 4]);
        assert_eq!(b.verts.len(), VERTS_PER_QUAD);
        b.begin(0.0, 0.0);
        assert!(b.verts.is_empty());
        assert_eq!(b.screen, [1.0, 1.0]);
    }

    #[test]
    fn rect_emits_one_quad_and_skips_degenerate_ones() {
        let mut b = batch(800.0, 600.0);
        b.rect(10.0, 20.0, 30.0, 40.0, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(b.verts.len(), VERTS_PER_QUAD);
        assert_eq!(b.verts[0].pos, [10.0, 20.0]);
        assert_eq!(b.verts[2].pos, [40.0, 60.0]);

        let before = b.verts.len();
        b.rect(0.0, 0.0, 0.0, 10.0, [1.0; 4]);
        b.rect(0.0, 0.0, 10.0, -1.0, [1.0; 4]);
        b.rect(0.0, 0.0, 10.0, 10.0, [1.0, 1.0, 1.0, 0.0]);
        assert_eq!(b.verts.len(), before, "degenerate quads must be skipped");
    }

    #[test]
    fn pixel_origin_is_top_left_in_clip_space() {
        let screen = [800.0, 600.0];
        let mut b = batch(screen[0], screen[1]);
        b.rect(0.0, 0.0, screen[0], screen[1], [1.0; 4]);
        let tl = to_clip(b.verts[0], screen);
        let br = to_clip(b.verts[2], screen);
        assert!(
            (tl[0] + 1.0).abs() < 1e-5 && (tl[1] - 1.0).abs() < 1e-5,
            "{tl:?}"
        );
        assert!(
            (br[0] - 1.0).abs() < 1e-5 && (br[1] + 1.0).abs() < 1e-5,
            "{br:?}"
        );
    }

    #[test]
    fn text_emits_one_quad_per_visible_glyph() {
        let mut b = batch(800.0, 600.0);
        b.text(0.0, 0.0, 14.0, C_TEXT, "A A");
        assert_eq!(
            b.verts.len(),
            2 * VERTS_PER_QUAD,
            "spaces must not emit quads"
        );

        b.begin(800.0, 600.0);
        b.text(0.0, 0.0, 14.0, C_TEXT, "HELLO");
        assert_eq!(b.verts.len(), 5 * VERTS_PER_QUAD);

        // Glyphs advance left to right and never overlap.
        let first_left = b.verts[0].pos[0];
        let second_left = b.verts[VERTS_PER_QUAD].pos[0];
        let first_right = b.verts[2].pos[0];
        assert!(second_left >= first_right, "glyphs must not overlap");
        assert!(second_left > first_left);
    }

    #[test]
    fn text_respects_zero_alpha_and_zero_size() {
        let mut b = batch(800.0, 600.0);
        b.text(0.0, 0.0, 14.0, [1.0, 1.0, 1.0, 0.0], "HELLO");
        b.text(0.0, 0.0, 0.0, C_TEXT, "HELLO");
        assert!(b.verts.is_empty());
    }

    #[test]
    fn text_width_matches_emitted_geometry() {
        let mut b = batch(800.0, 600.0);
        let s = "XYZ 1.0";
        b.text(0.0, 0.0, 14.0, C_TEXT, s);
        let right = b.verts.iter().map(|v| v.pos[0]).fold(f32::MIN, f32::max);
        assert!((right - text_width(14.0, s)).abs() < 1e-3, "{right}");
    }

    #[test]
    fn crosshair_sits_at_the_centre() {
        let mut b = batch(800.0, 600.0);
        b.crosshair();
        assert!(!b.verts.is_empty());
        let cx = b.verts.iter().map(|v| v.pos[0]).sum::<f32>() / b.verts.len() as f32;
        let cy = b.verts.iter().map(|v| v.pos[1]).sum::<f32>() / b.verts.len() as f32;
        assert!((cx - 400.0).abs() < 0.6, "{cx}");
        assert!((cy - 300.0).abs() < 0.6, "{cy}");
    }

    #[test]
    fn hotbar_is_centred_and_on_screen() {
        let (w, h) = (1280.0, 720.0);
        let mut b = batch(w, h);
        let slots: Vec<Option<Slot>> = (0..HOTBAR_SLOTS)
            .map(|i| Some(Slot::new(1, [0.5, 0.4, 0.3], (i as u16) * 7)))
            .collect();
        b.hotbar(&slots, 3);

        let (hx, hy, hw, hh) = b.hotbar_bounds();
        assert!(
            (hx + hw * 0.5 - w * 0.5).abs() < 1.0,
            "hotbar is not centred"
        );
        assert!(
            (hy + hh + MARGIN - h).abs() < 1.0,
            "hotbar is not at the bottom"
        );

        for v in &b.verts {
            assert!(
                v.pos[0] >= 0.0 && v.pos[0] <= w,
                "x off screen: {}",
                v.pos[0]
            );
            assert!(
                v.pos[1] >= 0.0 && v.pos[1] <= h,
                "y off screen: {}",
                v.pos[1]
            );
        }
    }

    #[test]
    fn selected_slot_adds_geometry() {
        let slots: Vec<Option<Slot>> = vec![None; HOTBAR_SLOTS];

        let mut plain = batch(1280.0, 720.0);
        plain.slot(10.0, 10.0, SLOT_SIZE, None, false);
        let mut selected = batch(1280.0, 720.0);
        selected.slot(10.0, 10.0, SLOT_SIZE, None, true);
        assert!(selected.verts.len() > plain.verts.len());

        // And the highlight reaches the whole bar, not just one slot.
        let mut b = batch(1280.0, 720.0);
        b.hotbar(&slots, 0);
        let n0 = b.verts.len();
        b.begin(1280.0, 720.0);
        b.hotbar(&slots, 8);
        assert_eq!(n0, b.verts.len(), "every slot must highlight identically");
    }

    #[test]
    fn stack_counts_are_drawn_only_above_one() {
        let mut none = batch(800.0, 600.0);
        none.slot(0.0, 0.0, SLOT_SIZE, Some(Slot::new(1, [1.0; 3], 1)), false);
        let mut some = batch(800.0, 600.0);
        some.slot(0.0, 0.0, SLOT_SIZE, Some(Slot::new(1, [1.0; 3], 64)), false);
        // "64" is two glyphs, drawn twice (shadow + face).
        assert_eq!(some.verts.len(), none.verts.len() + 4 * VERTS_PER_QUAD);
    }

    #[test]
    fn health_fills_proportionally() {
        let mut full = batch(1280.0, 720.0);
        full.health(20.0, 20.0);
        let mut empty = batch(1280.0, 720.0);
        empty.health(0.0, 20.0);
        let mut half = batch(1280.0, 720.0);
        half.health(10.0, 20.0);

        // Every segment draws a backing and a border; only filled ones add a bar.
        assert_eq!(full.verts.len(), empty.verts.len() + 10 * VERTS_PER_QUAD);
        assert_eq!(half.verts.len(), empty.verts.len() + 5 * VERTS_PER_QUAD);

        // A fractional segment is narrower than a full one.
        let mut partial = batch(1280.0, 720.0);
        partial.health(1.0, 20.0);
        let widest = partial
            .verts
            .chunks(VERTS_PER_QUAD)
            .map(|q| q[2].pos[0] - q[0].pos[0])
            .fold(f32::MIN, f32::max);
        assert!(widest <= 14.0);
    }

    #[test]
    fn health_survives_nonsense_input() {
        let mut b = batch(1280.0, 720.0);
        b.health(-5.0, 0.0);
        b.health(999.0, 20.0);
        assert!(
            b.verts
                .iter()
                .all(|v| v.pos[0].is_finite() && v.pos[1].is_finite())
        );
    }

    #[test]
    fn slot_grid_covers_every_cell() {
        let mut b = batch(1280.0, 720.0);
        let items: Vec<Option<Slot>> = vec![Some(Slot::new(1, [0.2, 0.6, 0.2], 1)); 9];
        // 27 cells but only 9 items: the rest must still draw as empty slots.
        b.slot_grid([100.0, 100.0], 9, 3, 40.0, 4.0, &items);

        let empty_cell = VERTS_PER_QUAD + 4 * VERTS_PER_QUAD; // background + 4-sided border
        // One quad of item art. It used to be two -- a flat colour swatch and a
        // painted band under it faking depth -- back when the overlay could not
        // sample a texture and a slot was a coloured square.
        let filled_cell = empty_cell + VERTS_PER_QUAD;
        assert_eq!(b.verts.len(), 9 * filled_cell + 18 * empty_cell);

        assert!((grid_width(9, 40.0, 4.0) - (9.0 * 40.0 + 8.0 * 4.0)).abs() < 1e-4);
        assert_eq!(grid_width(0, 40.0, 4.0), 0.0);
        assert_eq!(grid_height(3, 40.0, 4.0), grid_width(3, 40.0, 4.0));
    }

    #[test]
    fn panel_and_dim_cover_what_they_claim() {
        let mut b = batch(800.0, 600.0);
        b.screen_dim();
        assert_eq!(b.verts[0].pos, [0.0, 0.0]);
        assert_eq!(b.verts[2].pos, [800.0, 600.0]);

        b.begin(800.0, 600.0);
        b.panel(50.0, 60.0, 200.0, 150.0);
        assert!(
            b.verts.len() > VERTS_PER_QUAD,
            "panel needs a background and an edge"
        );
        assert_eq!(b.verts[0].pos, [50.0, 60.0]);
    }

    #[test]
    fn readout_draws_three_shadowed_lines() {
        let mut b = batch(1280.0, 720.0);
        b.readout([12.34, -5.0, 700.5], 59.7, "Stone");
        // Cheap structural check: something was drawn, in the top-left, and
        // every line is shadowed so glyph quads come in pairs.
        assert!(!b.verts.is_empty());
        assert_eq!(b.verts.len() % (2 * VERTS_PER_QUAD), 0);
        assert!(b.verts.iter().all(|v| v.pos[0] < 400.0 && v.pos[1] < 120.0));
    }

    /// The vertex must have no padding, and the attribute offsets declared to
    /// wgpu must match the field offsets Rust actually chose. Getting the second
    /// one wrong does not fail to compile -- it draws garbage.
    #[test]
    fn vertex_layout_is_tightly_packed() {
        assert_eq!(VERTEX_SIZE, 2 * 4 + 2 * 4 + 4 * 4 + 4);
        assert_eq!(std::mem::size_of::<ScreenUniform>(), 16);
        let v = HudVertex {
            pos: [0.0; 2],
            uv: [0.0; 2],
            color: [0.0; 4],
            mode: 0.0,
        };
        let base = &v as *const _ as usize;
        assert_eq!(&v.pos as *const _ as usize - base, 0);
        assert_eq!(&v.uv as *const _ as usize - base, 8);
        assert_eq!(&v.color as *const _ as usize - base, 16);
        assert_eq!(&v.mode as *const _ as usize - base, 32);
    }
}
