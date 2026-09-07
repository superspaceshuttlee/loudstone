//! Mob skin sheets.
//!
//! See the note below: a mob is a handful of boxes, and what makes it read as a
//! creature rather than a stack of crates is entirely in its skin.

use super::*;

// ---------------------------------------------------------------------------
// Mob skins
// ---------------------------------------------------------------------------
//
// A mob is a handful of boxes, and everything that makes it read as a zombie
// rather than a stack of crates lives in its *skin* -- a face on the front of
// the head, a torn shirt on the torso, blood down one arm. So mobs do not get a
// tile per part like blocks do; they get a 64x64 skin laid out exactly the way
// Minecraft lays one out, and every box face samples its own rectangle of it.
//
// The skin sits in a reserved corner of the block atlas rather than a texture of
// its own, so entities keep using the terrain pipeline and its single bind
// group. The atlas has 256 tiles and blocks use 82, so there is room to spare.

/// Top-left texel of the mob skin sheet inside the atlas.
pub const SKIN_ORIGIN: (usize, usize) = (0, 192);
/// Skins are the standard 64x64 sheet.
pub const SKIN_SIZE: usize = 64;

/// Which mob a skin belongs to. Ordered like `mob::MobKind::ALL`.
pub const SKIN_ZOMBIE: usize = 0;
pub const SKIN_SKELETON: usize = 1;
pub const SKIN_CREEPER: usize = 2;
pub const SKIN_PIG: usize = 3;
pub const SKIN_COUNT: usize = 4;

/// Where one skin sheet starts in the atlas. Four sheets sit side by side.
pub fn skin_origin(skin: usize) -> (usize, usize) {
    (SKIN_ORIGIN.0 + skin * SKIN_SIZE, SKIN_ORIGIN.1)
}

/// The six faces of a box in the mesher's face order (top, bottom, +Z, -Z, +X, -X),
/// as texel rectangles inside a skin sheet.
///
/// This is Minecraft's box-UV layout: given the sheet coordinate `uv` and the box
/// dimensions `(w, h, d)` in texels, the unwrapped faces sit in a fixed cross
/// arrangement. Following it exactly means a skin drawn to Minecraft's template
/// maps correctly with no per-face bookkeeping in the model file.
pub fn box_face_rects(uv: (usize, usize), size: (usize, usize, usize)) -> [[usize; 4]; 6] {
    let (u, v) = uv;
    let (w, h, d) = size;
    [
        [u + d, v, w, d],             // top
        [u + d + w, v, w, d],         // bottom
        [u + d + w + d, v + d, w, h], // +Z, the back
        [u + d, v + d, w, h],         // -Z, the face
        [u, v + d, d, h],             // +X
        [u + d + w, v + d, d, h],     // -X
    ]
}

/// Normalised atlas UV rect for a texel rect inside a skin sheet.
pub fn skin_uv(skin: usize, rect: [usize; 4]) -> [f32; 4] {
    let (ox, oy) = skin_origin(skin);
    let e = 0.25;
    [
        ((ox + rect[0]) as f32 + e) / ATLAS_W as f32,
        ((oy + rect[1]) as f32 + e) / ATLAS_H as f32,
        ((ox + rect[0] + rect[2]) as f32 - e) / ATLAS_W as f32,
        ((oy + rect[1] + rect[3]) as f32 - e) / ATLAS_H as f32,
    ]
}

/// A 64x64 skin sheet being painted.
pub struct Skin {
    px: Vec<Rgba>,
}

impl Skin {
    fn new() -> Self {
        Skin {
            px: vec![[0, 0, 0, 0]; SKIN_SIZE * SKIN_SIZE],
        }
    }
    fn set(&mut self, x: usize, y: usize, c: Rgba) {
        if x < SKIN_SIZE && y < SKIN_SIZE {
            self.px[y * SKIN_SIZE + x] = c;
        }
    }
    fn get(&self, x: usize, y: usize) -> Rgba {
        self.px[y * SKIN_SIZE + x]
    }
    /// Fill a texel rectangle with a base colour plus per-texel noise, which is
    /// what stops a flat box reading as plastic.
    fn panel(&mut self, r: [usize; 4], base: Rgba, jitter: i32, seed: u32) {
        for y in 0..r[3] {
            for x in 0..r[2] {
                let n = hash2(r[0] as u32 + x as u32, r[1] as u32 + y as u32, seed);
                let d = ((n % 512) as i32 - 256) * jitter / 256;
                self.set(r[0] + x, r[1] + y, shade_i(base, d));
            }
        }
    }
    /// Every face of a box, so no part of a skin is ever left transparent.
    fn box_all(&mut self, uv: (usize, usize), size: (usize, usize, usize), c: Rgba, seed: u32) {
        for r in box_face_rects(uv, size) {
            self.panel(r, c, 26, seed);
        }
    }
}

/// Lighten or darken by a signed amount, for skin noise and shading.
pub fn shade_i(c: Rgba, d: i32) -> Rgba {
    [
        (c[0] as i32 + d).clamp(0, 255) as u8,
        (c[1] as i32 + d).clamp(0, 255) as u8,
        (c[2] as i32 + d).clamp(0, 255) as u8,
        c[3],
    ]
}

pub fn hash2(x: u32, y: u32, seed: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9E37_79B9) ^ y.wrapping_mul(0x85EB_CA6B) ^ seed;
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    h
}

/// The standard Minecraft humanoid layout, in texels.
/// (uv origin, box size) per part, in the order the model file declares them.
pub const HEAD_UV: (usize, usize) = (0, 0);
pub const HEAD_SIZE: (usize, usize, usize) = (8, 8, 8);
pub const BODY_UV: (usize, usize) = (16, 16);
pub const BODY_SIZE: (usize, usize, usize) = (8, 12, 4);
pub const ARM_R_UV: (usize, usize) = (40, 16);
pub const ARM_L_UV: (usize, usize) = (32, 48);
pub const ARM_SIZE: (usize, usize, usize) = (4, 12, 4);
pub const LEG_R_UV: (usize, usize) = (0, 16);
pub const LEG_L_UV: (usize, usize) = (16, 48);
pub const LEG_SIZE: (usize, usize, usize) = (4, 12, 4);

// A four-legged mob unwraps to completely different rectangles from a humanoid:
// a 10x8x16 barrel needs a 52x24 patch, which does not remotely fit where an
// 8x12x4 torso lives. Pointing the pig's body at the humanoid `BODY_UV` ran its
// unwrap off the end of that rectangle and into the arm and leg regions, so it
// was literally wearing scraps of other limbs. Each mob owns its own 64x64
// sheet, so a quadruped simply gets its own arrangement of it.
pub const QUAD_HEAD_UV: (usize, usize) = (0, 0);
pub const QUAD_HEAD_SIZE: (usize, usize, usize) = (8, 8, 8);
pub const QUAD_BODY_UV: (usize, usize) = (0, 16);
pub const QUAD_BODY_SIZE: (usize, usize, usize) = (10, 8, 16);
pub const QUAD_SNOUT_UV: (usize, usize) = (34, 44);
pub const QUAD_SNOUT_SIZE: (usize, usize, usize) = (4, 3, 1);
pub const QUAD_LEG_UV: (usize, usize) = (0, 44);
pub const QUAD_LEG_SIZE: (usize, usize, usize) = (4, 6, 4);

/// Paint one mob's skin sheet.
pub fn paint_skin(which: usize, sk: &mut Skin) {
    // Palettes chosen to sit beside the block atlas rather than shout over it.
    let (skin_c, shirt_c, trouser_c, seed) = match which {
        SKIN_ZOMBIE => (
            [0x4C, 0x7A, 0x3F, 255],
            [0x3A, 0x4E, 0x74, 255],
            [0x2E, 0x3A, 0x52, 255],
            11,
        ),
        SKIN_SKELETON => (
            [0xC8, 0xC8, 0xBE, 255],
            [0xB4, 0xB4, 0xAA, 255],
            [0xA8, 0xA8, 0x9E, 255],
            23,
        ),
        SKIN_CREEPER => (
            [0x4F, 0xB5, 0x45, 255],
            [0x45, 0xA0, 0x3C, 255],
            [0x3C, 0x8C, 0x34, 255],
            37,
        ),
        _ => (
            [0xE6, 0x9A, 0xA0, 255],
            [0xDD, 0x8E, 0x95, 255],
            [0xC9, 0x7B, 0x82, 255],
            53,
        ),
    };

    if which == SKIN_PIG {
        paint_quadruped(sk, skin_c, shirt_c, seed);
        return;
    }

    sk.box_all(HEAD_UV, HEAD_SIZE, skin_c, seed);
    sk.box_all(BODY_UV, BODY_SIZE, shirt_c, seed ^ 1);
    sk.box_all(ARM_R_UV, ARM_SIZE, skin_c, seed ^ 2);
    sk.box_all(ARM_L_UV, ARM_SIZE, skin_c, seed ^ 3);
    sk.box_all(LEG_R_UV, LEG_SIZE, trouser_c, seed ^ 4);
    sk.box_all(LEG_L_UV, LEG_SIZE, trouser_c, seed ^ 5);

    // Darken the outer edge of every face. Adjacent limbs touch with no gap
    // between them, so without this a mob reads as one undifferentiated column
    // -- which is exactly how the first skeleton came out.
    for (uv, size) in [
        (HEAD_UV, HEAD_SIZE),
        (BODY_UV, BODY_SIZE),
        (ARM_R_UV, ARM_SIZE),
        (ARM_L_UV, ARM_SIZE),
        (LEG_R_UV, LEG_SIZE),
        (LEG_L_UV, LEG_SIZE),
    ] {
        for r in box_face_rects(uv, size) {
            for x in 0..r[2] {
                for y in 0..r[3] {
                    let edge = x == 0 || y == 0 || x + 1 == r[2] || y + 1 == r[3];
                    if edge {
                        let c = sk.get(r[0] + x, r[1] + y);
                        sk.set(r[0] + x, r[1] + y, shade_i(c, -22));
                    }
                }
            }
        }
    }

    // Sleeves: the upper third of each arm belongs to the shirt.
    for (uv, _) in [(ARM_R_UV, 0), (ARM_L_UV, 0)] {
        for r in box_face_rects(uv, ARM_SIZE) {
            if r[3] >= 12 {
                sk.panel([r[0], r[1], r[2], 4], shirt_c, 20, seed);
            }
        }
    }

    // The face. This is the single thing that decides whether a mob reads as a
    // character, so it is painted explicitly rather than left to noise.
    let face = box_face_rects(HEAD_UV, HEAD_SIZE)[3];
    let (fx, fy) = (face[0], face[1]);
    let eye_dark: Rgba = [0x10, 0x14, 0x18, 255];
    let eye_glow: Rgba = match which {
        // Pale, not green. Light-green pupils on a green head have no contrast
        // at all, which is why this face did not read while the creeper's did.
        SKIN_ZOMBIE => [0xF2, 0xF6, 0xD8, 255],
        SKIN_SKELETON => [0x30, 0x30, 0x30, 255],
        SKIN_CREEPER => [0x0A, 0x0A, 0x0A, 255],
        _ => [0x2A, 0x1C, 0x1C, 255],
    };

    if which == SKIN_CREEPER {
        // The creeper's face is its whole identity: two square eyes and a
        // frowning mouth, all hard-edged.
        for (x, y) in [
            (1, 2),
            (2, 2),
            (1, 3),
            (2, 3),
            (5, 2),
            (6, 2),
            (5, 3),
            (6, 3),
        ] {
            sk.set(fx + x, fy + y, eye_glow);
        }
        for (x, y) in [
            (3, 4),
            (4, 4),
            (3, 5),
            (4, 5),
            (2, 5),
            (5, 5),
            (2, 6),
            (3, 6),
            (4, 6),
            (5, 6),
        ] {
            sk.set(fx + x, fy + y, eye_glow);
        }
    } else {
        // An 8x8 face has room for about three marks. Sockets, a pupil in each,
        // and a short mouth -- that is the whole budget. A previous version also
        // painted a brow line and a decay smear and the result was mush at any
        // distance, because every extra mark competes with the eyes.
        for (ex, ey) in [(1usize, 3usize), (5, 3)] {
            for dx in 0..2 {
                sk.set(fx + ex + dx, fy + ey, eye_dark);
                sk.set(fx + ex + dx, fy + ey + 1, eye_dark);
            }
        }
        sk.set(fx + 2, fy + 3, eye_glow);
        sk.set(fx + 5, fy + 3, eye_glow);
        for x in 3..5 {
            sk.set(fx + x, fy + 6, shade_i(skin_c, -70));
        }
        if which == SKIN_ZOMBIE {
            // One stain, low and to one side, where it cannot crowd the eyes.
            sk.set(fx + 6, fy + 5, [0x6B, 0x2B, 0x24, 255]);
            sk.set(fx + 6, fy + 6, [0x5A, 0x24, 0x1E, 255]);
        }
    }

    if which == SKIN_SKELETON {
        // Ribs across the chest and hollow sockets: bone, drawn not modelled.
        let front = box_face_rects(BODY_UV, BODY_SIZE)[3];
        for row in 0..4 {
            let y = front[1] + 2 + row * 2;
            for x in 1..(front[2] - 1) {
                sk.set(front[0] + x, y, shade_i(shirt_c, -55));
            }
        }
        for r in [
            box_face_rects(ARM_R_UV, ARM_SIZE)[3],
            box_face_rects(ARM_L_UV, ARM_SIZE)[3],
        ] {
            for y in 0..r[3] {
                sk.set(r[0] + 1, r[1] + y, shade_i(skin_c, -45));
                sk.set(r[0] + r[2] - 2, r[1] + y, shade_i(skin_c, -45));
            }
        }
    }

    // Legs touch with no gap between them, so at any distance a pair reads as
    // one wide slab. Darkening the inner edge of each is how the silhouette
    // gets its centre line back.
    for (uv, inner_on_right) in [(LEG_R_UV, true), (LEG_L_UV, false)] {
        for r in box_face_rects(uv, LEG_SIZE) {
            for y in 0..r[3] {
                let x = if inner_on_right { r[2] - 1 } else { 0 };
                let c = sk.get(r[0] + x, r[1] + y);
                sk.set(r[0] + x, r[1] + y, shade_i(c, -45));
            }
        }
    }

    // Blood and grime on the shirt front, for the zombie only.
    if which == SKIN_ZOMBIE {
        let front = box_face_rects(BODY_UV, BODY_SIZE)[3];
        for i in 0..14u32 {
            let n = hash2(i, 7, seed);
            let x = (n % front[2] as u32) as usize;
            let y = ((n >> 8) % front[3] as u32) as usize;
            sk.set(front[0] + x, front[1] + y, [0x63, 0x24, 0x20, 255]);
        }
    }
}

/// A four-legged skin: barrel, head with a snout, four feet.
pub fn paint_quadruped(sk: &mut Skin, hide: Rgba, belly: Rgba, seed: u32) {
    sk.box_all(QUAD_HEAD_UV, QUAD_HEAD_SIZE, hide, seed);
    sk.box_all(QUAD_BODY_UV, QUAD_BODY_SIZE, hide, seed ^ 1);
    sk.box_all(QUAD_LEG_UV, QUAD_LEG_SIZE, belly, seed ^ 2);
    sk.box_all(QUAD_SNOUT_UV, QUAD_SNOUT_SIZE, shade_i(hide, 20), seed ^ 4);
    // Nostrils on the snout's front face.
    let sn = box_face_rects(QUAD_SNOUT_UV, QUAD_SNOUT_SIZE)[3];
    sk.set(sn[0] + 1, sn[1] + 1, [0x5A, 0x35, 0x3A, 255]);
    sk.set(sn[0] + 2, sn[1] + 1, [0x5A, 0x35, 0x3A, 255]);

    // The underside is paler, as on a real animal, and it is the one cue that
    // reads the body as a barrel rather than a slab.
    let under = box_face_rects(QUAD_BODY_UV, QUAD_BODY_SIZE)[1];
    sk.panel(under, shade_i(hide, 26), 14, seed ^ 3);

    for (uv, size) in [
        (QUAD_HEAD_UV, QUAD_HEAD_SIZE),
        (QUAD_BODY_UV, QUAD_BODY_SIZE),
        (QUAD_LEG_UV, QUAD_LEG_SIZE),
    ] {
        for r in box_face_rects(uv, size) {
            for x in 0..r[2] {
                for y in 0..r[3] {
                    if x == 0 || y == 0 || x + 1 == r[2] || y + 1 == r[3] {
                        let c = sk.get(r[0] + x, r[1] + y);
                        sk.set(r[0] + x, r[1] + y, shade_i(c, -20));
                    }
                }
            }
        }
    }

    // Face: two eyes and a snout with nostrils, on the head's front.
    let f = box_face_rects(QUAD_HEAD_UV, QUAD_HEAD_SIZE)[3];
    let dark: Rgba = [0x24, 0x16, 0x1A, 255];
    for (ex, ey) in [(1usize, 2usize), (6, 2)] {
        sk.set(f[0] + ex, f[1] + ey, dark);
    }
    // No painted snout: it is a real box now, and painting one behind it only
    // showed through as a smudge.
}

/// All four skins, painted once and reused.
pub fn paint_skins_into(px: &mut [u8]) {
    for which in 0..SKIN_COUNT {
        let mut sk = Skin::new();
        paint_skin(which, &mut sk);
        let (ox, oy) = skin_origin(which);
        for y in 0..SKIN_SIZE {
            for x in 0..SKIN_SIZE {
                let c = sk.get(x, y);
                if c[3] == 0 {
                    continue;
                }
                let (ax, ay) = (ox + x, oy + y);
                if ax >= ATLAS_W || ay >= ATLAS_H {
                    continue;
                }
                let i = (ay * ATLAS_W + ax) * 4;
                px[i..i + 4].copy_from_slice(&c);
            }
        }
    }
}
