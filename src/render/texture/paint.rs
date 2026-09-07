//! The painting primitives every tile recipe is written against: a 16x16 canvas,
//! deterministic wrapping noise, and the shared palettes.
//!
//! Everything here is a pure function of its arguments and a seed. That is what
//! lets the atlas be rebuilt byte-for-byte, which in turn is what makes it
//! testable at all -- there is a test asserting two builds are identical.

use super::*;

// ---------------------------------------------------------------------------
// Painting primitives
// ---------------------------------------------------------------------------

pub type Rgba = [u8; 4];

pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

pub const CLEAR: Rgba = [0, 0, 0, 0];

/// One 16x16 tile under construction.
pub struct Tex {
    px: [Rgba; TILE * TILE],
}

impl Tex {
    pub fn new() -> Self {
        Self {
            px: [CLEAR; TILE * TILE],
        }
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, c: Rgba) {
        if x < TILE && y < TILE {
            self.px[y * TILE + x] = c;
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> Rgba {
        self.px[y * TILE + x]
    }

    pub fn fill(&mut self, c: Rgba) {
        self.px = [c; TILE * TILE];
    }

    pub fn rect(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: Rgba) {
        for y in y0..(y0 + h).min(TILE) {
            for x in x0..(x0 + w).min(TILE) {
                self.set(x, y, c);
            }
        }
    }

    /// Multiply the RGB of one texel, keeping its alpha.
    pub fn darken(&mut self, x: usize, y: usize, f: f32) {
        let c = self.get(x, y);
        self.set(x, y, shade(c, f));
    }

    /// Paint from pixel-art rows. `'.'` leaves the texel untouched; every other
    /// character must appear in `pal`.
    pub fn art(&mut self, rows: &[&str; TILE], pal: &[(char, Rgba)]) {
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                if ch == '.' || x >= TILE {
                    continue;
                }
                let c = pal
                    .iter()
                    .find(|(k, _)| *k == ch)
                    .map(|(_, v)| *v)
                    .unwrap_or([255, 0, 255, 255]);
                self.set(x, y, c);
            }
        }
    }
}

pub fn shade(c: Rgba, f: f32) -> Rgba {
    [
        (c[0] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[1] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[2] as f32 * f).clamp(0.0, 255.0) as u8,
        c[3],
    ]
}

pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
        (a[3] as f32 + (b[3] as f32 - a[3] as f32) * t) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Deterministic noise
// ---------------------------------------------------------------------------

#[inline]
pub fn hash(x: i32, y: i32, seed: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x27D4_EB2D)
        ^ (y as u32).wrapping_mul(0x1656_67B1)
        ^ seed.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2545_F491);
    h ^= h >> 13;
    h = h.wrapping_mul(0x27D4_EB2D);
    h ^ (h >> 16)
}

/// Uniform 0..1 from a lattice point.
#[inline]
pub fn rand01(x: i32, y: i32, seed: u32) -> f32 {
    hash(x, y, seed) as f32 / u32::MAX as f32
}

#[inline]
pub fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise that wraps over the tile, so neighbouring blocks join seamlessly.
/// `period` is how many lattice cells span the 16 texels and must divide 16.
pub fn vnoise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let s = TILE as f32 / period as f32;
    let (gx, gy) = (x / s, y / s);
    let (x0, y0) = (gx.floor() as i32, gy.floor() as i32);
    let (fx, fy) = (smooth(gx - x0 as f32), smooth(gy - y0 as f32));
    let w = |i: i32, j: i32| rand01(i.rem_euclid(period), j.rem_euclid(period), seed);
    let a = w(x0, y0) + (w(x0 + 1, y0) - w(x0, y0)) * fx;
    let b = w(x0, y0 + 1) + (w(x0 + 1, y0 + 1) - w(x0, y0 + 1)) * fx;
    a + (b - a) * fy
}

/// Three octaves of wrapping value noise, 0..1.
pub fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    0.55 * vnoise(x, y, 2, seed)
        + 0.30 * vnoise(x, y, 4, seed ^ 0x9E37_79B9)
        + 0.15 * vnoise(x, y, 8, seed ^ 0x51ED_270B)
}

/// Fill with `base`, modulated by wrapping noise and a per-texel grain.
pub fn noisy_fill(t: &mut Tex, base: Rgba, seed: u32, blotch: f32, grain: f32) {
    for y in 0..TILE {
        for x in 0..TILE {
            let n = fbm(x as f32 + 0.5, y as f32 + 0.5, seed) - 0.5;
            let g = rand01(x as i32, y as i32, seed ^ 0xABCD) - 0.5;
            let f = 1.0 + n * blotch * 2.0 + g * grain * 2.0;
            t.set(x, y, shade(base, f));
        }
    }
}

/// Distance between two coordinates on a 16-wide torus.
pub fn wrapd(a: f32, b: f32) -> f32 {
    let d = (a - b).abs();
    d.min(TILE as f32 - d)
}

/// Voronoi cell lookup: `(site index, nearest distance, second distance)`.
pub fn voronoi(x: f32, y: f32, sites: &[[f32; 2]]) -> (usize, f32, f32) {
    let (mut i0, mut d0, mut d1) = (0usize, f32::MAX, f32::MAX);
    for (i, s) in sites.iter().enumerate() {
        let dx = wrapd(x, s[0]);
        let dy = wrapd(y, s[1]);
        let d = (dx * dx + dy * dy).sqrt();
        if d < d0 {
            d1 = d0;
            d0 = d;
            i0 = i;
        } else if d < d1 {
            d1 = d;
        }
    }
    (i0, d0, d1)
}

pub fn sites(n: usize, seed: u32) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            [
                rand01(i as i32, 11, seed) * TILE as f32,
                rand01(i as i32, 29, seed) * TILE as f32,
            ]
        })
        .collect()
}

/// Cobble-style stone: irregular cells separated by dark mortar.
pub fn cobbles(t: &mut Tex, base: Rgba, seed: u32, count: usize, mortar: f32, spread: f32) {
    let pts = sites(count, seed);
    for y in 0..TILE {
        for x in 0..TILE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let (i, d0, d1) = voronoi(fx, fy, &pts);
            let tone = 1.0 + (rand01(i as i32, 7, seed) - 0.5) * spread;
            let grain = 1.0 + (rand01(x as i32, y as i32, seed ^ 0x77) - 0.5) * 0.16;
            let mut c = shade(base, tone * grain);
            // Mortar in the seam between two cells, plus a lighter lip on the
            // side of the seam nearest the camera-facing top-left.
            if d1 - d0 < mortar {
                c = shade(base, 0.44);
            } else if d1 - d0 < mortar + 0.55 {
                c = shade(c, 1.12);
            }
            t.set(x, y, c);
        }
    }
}

/// Scatter an irregular blob of ore over whatever is already painted.
pub fn ore_blob(t: &mut Tex, cx: f32, cy: f32, r: f32, seed: u32, core: Rgba, edge: Rgba) {
    for y in 0..TILE {
        for x in 0..TILE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = fx - cx;
            let dy = fy - cy;
            let d = (dx * dx + dy * dy).sqrt();
            let wobble = 0.72 + 0.55 * vnoise(fx * 2.0, fy * 2.0, 4, seed);
            let rr = r * wobble;
            if d < rr {
                // A lighter facet on the upper-left of each lump reads as a
                // rounded mineral rather than a flat sticker.
                let lit = if dx + dy < -rr * 0.25 { 1.22 } else { 1.0 };
                let c = if d < rr * 0.55 { core } else { edge };
                t.set(x, y, shade(c, lit));
            }
        }
    }
}

/// Diamond-shaped ore facet, for gems rather than lumps.
pub fn gem(t: &mut Tex, cx: i32, cy: i32, r: i32, core: Rgba, edge: Rgba, spark: Rgba) {
    for y in 0..TILE {
        for x in 0..TILE {
            let d = (x as i32 - cx).abs() + (y as i32 - cy).abs();
            if d <= r {
                t.set(x, y, if d == r { edge } else { core });
            }
        }
    }
    t.set((cx - 1).max(0) as usize, (cy - 1).max(0) as usize, spark);
}

// ---------------------------------------------------------------------------
// Palettes
// ---------------------------------------------------------------------------

pub const STONE_BASE: Rgba = rgb(128, 128, 134);
pub const DIRT_BASE: Rgba = rgb(122, 86, 55);
pub const SAND_BASE: Rgba = rgb(219, 205, 148);
pub const WOOD_BASE: Rgba = rgb(104, 76, 44);
pub const PLANK_BASE: Rgba = rgb(163, 124, 74);
