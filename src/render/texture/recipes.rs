//! One function per tile: what stone looks like, what a pickaxe looks like.
//!
//! These are the actual art. Each is written against the primitives in
//! [`super::paint`] and knows nothing about the atlas it will be packed into.

use super::*;

// ---------------------------------------------------------------------------
// Tile recipes
// ---------------------------------------------------------------------------

pub fn paint_tile(id: TileId, t: &mut Tex) {
    match id {
        T_WHITE => t.fill(rgb(255, 255, 255)),
        T_STONE => stone(t, STONE_BASE, 0x5701),
        T_DIRT => dirt(t, DIRT_BASE, 0x5702),
        T_GRASS_TOP => grass_top(t, rgb(94, 152, 62), 0x5703),
        T_GRASS_SIDE => grass_side(t, rgb(94, 152, 62), 0x5704),
        T_GRASS_TOP_COLD => grass_top(t, rgb(101, 137, 100), 0x5705),
        T_GRASS_SIDE_COLD => grass_side(t, rgb(101, 137, 100), 0x5706),
        T_GRASS_TOP_DRY => grass_top(t, rgb(160, 163, 82), 0x5707),
        T_GRASS_SIDE_DRY => grass_side(t, rgb(160, 163, 82), 0x5708),
        T_GRASS_TOP_SWAMP => grass_top(t, rgb(80, 112, 60), 0x5709),
        T_GRASS_SIDE_SWAMP => grass_side(t, rgb(80, 112, 60), 0x570A),
        T_PODZOL_TOP => podzol_top(t),
        T_PODZOL_SIDE => podzol_side(t),
        T_SAND => sand(t),
        T_SANDSTONE_TOP => sandstone_top(t),
        T_SANDSTONE_SIDE => sandstone_side(t),
        T_SANDSTONE_BOTTOM => sandstone_bottom(t),
        T_GRAVEL => gravel(t),
        T_CLAY => clay(t),
        T_SNOW => snow(t),
        T_ICE => ice(t),
        T_GRANITE => speckled_stone(
            t,
            rgb(155, 111, 96),
            rgb(196, 152, 133),
            rgb(112, 76, 66),
            0x6101,
        ),
        T_DIORITE => speckled_stone(
            t,
            rgb(196, 196, 192),
            rgb(232, 232, 228),
            rgb(146, 146, 144),
            0x6102,
        ),
        T_ANDESITE => speckled_stone(
            t,
            rgb(140, 145, 140),
            rgb(172, 177, 172),
            rgb(108, 113, 108),
            0x6103,
        ),
        T_COBBLESTONE => cobbles(t, rgb(126, 126, 132), 0x6201, 7, 1.15, 0.44),
        T_BEDROCK => bedrock(t),
        T_PLANKS => planks(t, PLANK_BASE, 0x6301),
        T_OAK_LOG_SIDE => bark(t, WOOD_BASE, 0x6401),
        T_OAK_LOG_TOP => log_end(t, rgb(176, 143, 92), rgb(140, 110, 68), WOOD_BASE, 0x6402),
        T_BIRCH_LOG_SIDE => birch_bark(t),
        T_BIRCH_LOG_TOP => log_end(
            t,
            rgb(216, 205, 178),
            rgb(186, 172, 143),
            rgb(206, 200, 184),
            0x6404,
        ),
        T_SPRUCE_LOG_SIDE => bark(t, rgb(70, 49, 30), 0x6405),
        T_SPRUCE_LOG_TOP => log_end(
            t,
            rgb(140, 106, 62),
            rgb(106, 78, 44),
            rgb(70, 49, 30),
            0x6406,
        ),
        T_OAK_LEAVES => leaves(t, rgb(64, 124, 48), rgb(44, 94, 34), 0x6501, 0.38),
        T_BIRCH_LEAVES => leaves(t, rgb(122, 160, 72), rgb(92, 128, 52), 0x6502, 0.40),
        T_SPRUCE_LEAVES => needles(t, rgb(44, 84, 56), rgb(28, 60, 40), 0x6503),
        T_COAL_ORE => ore(t, rgb(34, 34, 38), rgb(58, 58, 64), 0x6601, false),
        T_IRON_ORE => ore(t, rgb(206, 160, 124), rgb(166, 122, 92), 0x6602, false),
        T_GOLD_ORE => ore(t, rgb(248, 214, 78), rgb(198, 158, 44), 0x6603, false),
        T_DIAMOND_ORE => ore(t, rgb(110, 226, 226), rgb(66, 176, 186), 0x6604, true),
        T_WATER => water(t),
        T_BUCKET => bucket(t, false),
        T_WATER_BUCKET => bucket(t, true),
        T_CACTUS_SIDE => cactus_side(t),
        T_CACTUS_TOP => cactus_top(t),
        T_CACTUS_BOTTOM => cactus_bottom(t),
        T_TORCH => torch(t),
        T_TABLE_TOP => table_top(t),
        T_TABLE_SIDE => table_side(t),
        T_TABLE_FRONT => table_front(t),
        T_FURNACE_TOP => furnace_top(t),
        T_FURNACE_SIDE => furnace_side(t),
        T_FURNACE_FRONT => furnace_front(t),
        T_TALL_GRASS => tall_grass(t),
        T_FLOWER_RED => flower(t, rgb(196, 64, 58), rgb(232, 108, 96), rgb(240, 214, 96)),
        T_FLOWER_YELLOW => flower(t, rgb(226, 190, 52), rgb(248, 226, 118), rgb(150, 104, 32)),
        T_DEAD_BUSH => dead_bush(t),

        T_STICK => stick(t),
        T_COAL => nugget(t, rgb(38, 38, 42), rgb(74, 74, 82), rgb(18, 18, 20)),
        T_RAW_IRON => nugget(t, rgb(198, 156, 122), rgb(232, 198, 168), rgb(140, 104, 78)),
        T_IRON_INGOT => ingot(t),
        T_PICK_WOOD => tool(t, &PICKAXE_ART, TIER_WOOD),
        T_PICK_STONE => tool(t, &PICKAXE_ART, TIER_STONE),
        T_PICK_IRON => tool(t, &PICKAXE_ART, TIER_IRON),
        T_AXE_WOOD => tool(t, &AXE_ART, TIER_WOOD),
        T_AXE_STONE => tool(t, &AXE_ART, TIER_STONE),
        T_AXE_IRON => tool(t, &AXE_ART, TIER_IRON),
        T_SWORD_WOOD => tool(t, &SWORD_ART, TIER_WOOD),
        T_SWORD_STONE => tool(t, &SWORD_ART, TIER_STONE),
        T_SWORD_IRON => tool(t, &SWORD_ART, TIER_IRON),

        T_ZOMBIE_FACE => zombie_face(t),
        T_ZOMBIE_HEAD => mob_skin(t, rgb(76, 140, 82), rgb(58, 112, 64), 0x7101),
        T_ZOMBIE_BODY => zombie_body(t),
        T_SKELETON_FACE => skeleton_face(t),
        T_SKELETON_HEAD => mob_skin(t, rgb(210, 210, 200), rgb(176, 176, 166), 0x7102),
        T_SKELETON_BODY => skeleton_body(t),
        T_CREEPER_FACE => creeper_face(t),
        T_CREEPER_HEAD => mob_skin(t, rgb(72, 176, 78), rgb(48, 134, 56), 0x7103),
        T_CREEPER_BODY => creeper_body(t),
        T_PIG_FACE => pig_face(t),
        T_PIG_HEAD => mob_skin(t, rgb(230, 152, 154), rgb(206, 126, 130), 0x7104),
        T_PIG_BODY => pig_body(t),
        T_ARROW => arrow(t),
        _ => missing(t),
    }
}

// -- ground -----------------------------------------------------------------

pub fn stone(t: &mut Tex, base: Rgba, seed: u32) {
    noisy_fill(t, base, seed, 0.11, 0.07);
    // A handful of darker pits and one or two pale flecks give the surface a
    // sense of scale that pure noise does not.
    for i in 0..7 {
        let x = (rand01(i, 1, seed) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 2, seed) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.78);
        if i % 3 == 0 {
            t.darken((x + 1) % TILE, y, 0.84);
        }
    }
    for i in 0..3 {
        let x = (rand01(i, 3, seed ^ 0x99) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 4, seed ^ 0x99) * TILE as f32) as usize % TILE;
        t.darken(x, y, 1.16);
    }
}

pub fn dirt(t: &mut Tex, base: Rgba, seed: u32) {
    noisy_fill(t, base, seed, 0.16, 0.13);
    // Small pebbles and root flecks.
    for i in 0..9 {
        let x = (rand01(i, 5, seed) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 6, seed) * TILE as f32) as usize % TILE;
        let f = if i % 2 == 0 { 0.74 } else { 1.2 };
        t.darken(x, y, f);
    }
}

pub fn grass_top(t: &mut Tex, base: Rgba, seed: u32) {
    noisy_fill(t, base, seed, 0.13, 0.16);
    // Short blade strokes, one or two texels long, in a lighter and a darker
    // green. Without them the top face is just noise.
    for i in 0..14 {
        let x = (rand01(i, 7, seed) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 8, seed) * TILE as f32) as usize % TILE;
        let f = if i % 2 == 0 { 1.22 } else { 0.78 };
        t.darken(x, y, f);
        t.darken(x, (y + 1) % TILE, f * 0.96);
    }
}

/// Dirt with a grass fringe hanging over the top edge, ragged per column.
pub fn grass_side(t: &mut Tex, green: Rgba, seed: u32) {
    dirt(t, DIRT_BASE, seed ^ 0x2222);
    for x in 0..TILE {
        let d = 3 + (rand01(x as i32, 21, seed) * 3.0) as usize; // 3..=5
        for y in 0..d {
            let f = 1.0 + (rand01(x as i32, y as i32, seed ^ 0x33) - 0.5) * 0.24;
            t.set(x, y, shade(green, f));
        }
        // Darker lip where the fringe meets the soil, and the odd blade that
        // hangs a texel lower.
        t.set(x, d - 1, shade(green, 0.82));
        if rand01(x as i32, 41, seed) > 0.62 {
            t.set(x, d, shade(green, 0.7));
        }
    }
}

pub fn podzol_top(t: &mut Tex) {
    noisy_fill(t, rgb(96, 68, 34), 0x5801, 0.2, 0.16);
    for i in 0..18 {
        let x = (rand01(i, 9, 0x5801) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 10, 0x5801) * TILE as f32) as usize % TILE;
        let c = if i % 3 == 0 {
            rgb(142, 104, 48)
        } else {
            rgb(70, 48, 24)
        };
        t.set(x, y, c);
    }
}

pub fn podzol_side(t: &mut Tex) {
    dirt(t, DIRT_BASE, 0x5802);
    for x in 0..TILE {
        let d = 2 + (rand01(x as i32, 22, 0x5802) * 3.0) as usize;
        for y in 0..d {
            let f = 1.0 + (rand01(x as i32, y as i32, 0x5803) - 0.5) * 0.3;
            t.set(x, y, shade(rgb(96, 68, 34), f));
        }
    }
}

pub fn sand(t: &mut Tex) {
    noisy_fill(t, SAND_BASE, 0x5901, 0.05, 0.1);
    for i in 0..10 {
        let x = (rand01(i, 12, 0x5901) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 13, 0x5901) * TILE as f32) as usize % TILE;
        t.darken(x, y, if i % 2 == 0 { 0.9 } else { 1.07 });
    }
}

pub fn sandstone_top(t: &mut Tex) {
    noisy_fill(t, rgb(216, 202, 150), 0x5A01, 0.05, 0.05);
    for x in 0..TILE {
        t.darken(x, 0, 0.9);
        t.darken(x, TILE - 1, 0.94);
    }
}

pub fn sandstone_side(t: &mut Tex) {
    noisy_fill(t, rgb(210, 194, 140), 0x5A02, 0.07, 0.05);
    // Horizontal strata: a dark cap and floor, then a few bedding lines.
    for x in 0..TILE {
        t.set(x, 0, shade(rgb(226, 212, 162), 1.0));
        t.set(x, 1, shade(rgb(226, 212, 162), 0.96));
        t.darken(x, 2, 0.82);
        t.darken(x, 13, 0.86);
        t.set(x, 15, shade(rgb(186, 170, 120), 1.0));
    }
    for y in [5usize, 9] {
        for x in 0..TILE {
            let f = 0.9 + 0.06 * vnoise(x as f32, y as f32, 4, 0x5A03);
            t.darken(x, y, f);
        }
    }
}

pub fn sandstone_bottom(t: &mut Tex) {
    noisy_fill(t, rgb(196, 180, 128), 0x5A04, 0.08, 0.06);
    for i in 0..6 {
        let x = (rand01(i, 14, 0x5A04) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 15, 0x5A04) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.86);
    }
}

pub fn gravel(t: &mut Tex) {
    cobbles(t, rgb(126, 121, 118), 0x5B01, 15, 0.75, 0.62);
    // Extra per-texel grit so it reads as loose stone rather than paving.
    for y in 0..TILE {
        for x in 0..TILE {
            let f = 1.0 + (rand01(x as i32, y as i32, 0x5B02) - 0.5) * 0.28;
            t.darken(x, y, f);
        }
    }
}

pub fn clay(t: &mut Tex) {
    noisy_fill(t, rgb(160, 166, 178), 0x5C01, 0.07, 0.04);
    for i in 0..5 {
        let x = (rand01(i, 16, 0x5C01) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 17, 0x5C01) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.92);
        t.darken((x + 1) % TILE, y, 0.95);
    }
}

pub fn snow(t: &mut Tex) {
    noisy_fill(t, rgb(246, 249, 253), 0x5D01, 0.02, 0.035);
    for i in 0..6 {
        let x = (rand01(i, 18, 0x5D01) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 19, 0x5D01) * TILE as f32) as usize % TILE;
        t.set(x, y, rgb(226, 234, 246));
    }
}

pub fn ice(t: &mut Tex) {
    noisy_fill(t, rgb(150, 194, 236), 0x5E01, 0.09, 0.03);
    // Cracks: a few straight runs at 45 degrees, wrapped over the tile.
    for k in 0..3 {
        let sx = (rand01(k, 23, 0x5E01) * TILE as f32) as i32;
        let sy = (rand01(k, 24, 0x5E01) * TILE as f32) as i32;
        let dir = if k % 2 == 0 { 1 } else { -1 };
        for s in 0..10 {
            let x = (sx + s).rem_euclid(TILE as i32) as usize;
            let y = (sy + s * dir).rem_euclid(TILE as i32) as usize;
            t.set(x, y, rgb(196, 226, 250));
        }
    }
    for i in 0..4 {
        let x = (rand01(i, 25, 0x5E02) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 26, 0x5E02) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.9);
    }
}

pub fn speckled_stone(t: &mut Tex, base: Rgba, light: Rgba, dark: Rgba, seed: u32) {
    noisy_fill(t, base, seed, 0.08, 0.05);
    // Mineral grains: a scatter of two-texel clumps in both directions.
    for i in 0..22 {
        let x = (rand01(i, 31, seed) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 32, seed) * TILE as f32) as usize % TILE;
        let c = if rand01(i, 33, seed) > 0.5 {
            light
        } else {
            dark
        };
        t.set(x, y, c);
        if rand01(i, 34, seed) > 0.55 {
            t.set((x + 1) % TILE, y, mix(c, base, 0.4));
        }
    }
}

pub fn bedrock(t: &mut Tex) {
    noisy_fill(t, rgb(58, 58, 64), 0x5F01, 0.34, 0.16);
    // Hard-edged blocky lumps, so bedrock reads as unbreakable rather than
    // as very dark stone.
    for i in 0..12 {
        let x = (rand01(i, 35, 0x5F01) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 36, 0x5F01) * TILE as f32) as usize % TILE;
        let w = 1 + (rand01(i, 37, 0x5F01) * 3.0) as usize;
        let h = 1 + (rand01(i, 38, 0x5F01) * 3.0) as usize;
        let c = if i % 2 == 0 {
            rgb(30, 30, 34)
        } else {
            rgb(92, 92, 100)
        };
        for dy in 0..h {
            for dx in 0..w {
                t.set((x + dx) % TILE, (y + dy) % TILE, c);
            }
        }
    }
}

// -- wood -------------------------------------------------------------------

pub fn planks(t: &mut Tex, base: Rgba, seed: u32) {
    // Four horizontal boards with staggered end joints.
    let joints = [11usize, 5, 13, 3];
    for b in 0..4 {
        let y0 = b * 4;
        let tone = 1.0 + (rand01(b as i32, 51, seed) - 0.5) * 0.16;
        for y in y0..y0 + 4 {
            for x in 0..TILE {
                let g = vnoise(x as f32 * 1.4, y as f32 * 6.0, 8, seed) - 0.5;
                let mut f = tone * (1.0 + g * 0.18);
                if y == y0 {
                    f *= 1.07; // lit top edge of each board
                }
                if y == y0 + 3 {
                    f *= 0.7; // shadowed gap between boards
                }
                t.set(x, y, shade(base, f));
            }
        }
        // The butt joint between two boards.
        let jx = joints[b];
        for y in y0..y0 + 3 {
            t.set(jx, y, shade(base, 0.6));
        }
    }
    // A couple of knots.
    for k in 0..2 {
        let x = 2 + (rand01(k, 52, seed) * 11.0) as usize;
        let y = 1 + (rand01(k, 53, seed) * 13.0) as usize;
        t.set(x, y, shade(base, 0.62));
        t.set((x + 1) % TILE, y, shade(base, 0.78));
    }
}

/// Vertical grain, for the sides of a log.
pub fn bark(t: &mut Tex, base: Rgba, seed: u32) {
    for x in 0..TILE {
        let col = 1.0 + (rand01(x as i32, 61, seed) - 0.5) * 0.34;
        for y in 0..TILE {
            let g = vnoise(x as f32 * 4.0, y as f32 * 0.8, 8, seed) - 0.5;
            t.set(x, y, shade(base, col * (1.0 + g * 0.3)));
        }
    }
    // Deep grooves down the bark.
    for k in 0..3 {
        let x = (rand01(k, 62, seed) * TILE as f32) as usize % TILE;
        for y in 0..TILE {
            let f = 0.6 + 0.12 * vnoise(x as f32, y as f32 * 2.0, 8, seed);
            t.darken(x, y, f);
        }
    }
}

pub fn birch_bark(t: &mut Tex) {
    noisy_fill(t, rgb(214, 208, 190), 0x6403, 0.05, 0.05);
    // Horizontal lenticels: short dark dashes, the birch signature.
    for i in 0..9 {
        let y = (rand01(i, 63, 0x6403) * TILE as f32) as usize % TILE;
        let x = (rand01(i, 64, 0x6403) * TILE as f32) as usize % TILE;
        let w = 2 + (rand01(i, 65, 0x6403) * 3.0) as usize;
        for d in 0..w {
            t.set((x + d) % TILE, y, rgb(74, 68, 58));
        }
        if i % 3 == 0 {
            t.set((x + w) % TILE, y, rgb(140, 132, 118));
        }
    }
    for k in 0..2 {
        let x = (rand01(k, 66, 0x6403) * TILE as f32) as usize % TILE;
        for y in 0..TILE {
            t.darken(x, y, 0.93);
        }
    }
}

/// Concentric growth rings with a bark rim, for a log's cut end.
pub fn log_end(t: &mut Tex, pale: Rgba, dark: Rgba, rim: Rgba, seed: u32) {
    let c = 7.5f32;
    for y in 0..TILE {
        for x in 0..TILE {
            let dx = x as f32 + 0.5 - c;
            let dy = y as f32 + 0.5 - c;
            let d = (dx * dx + dy * dy).sqrt() + (vnoise(x as f32, y as f32, 4, seed) - 0.5) * 1.1;
            if d > 6.6 {
                t.set(
                    x,
                    y,
                    shade(rim, 0.9 + 0.2 * vnoise(x as f32, y as f32, 8, seed)),
                );
            } else {
                let ring = ((d * 1.35).floor() as i32) % 2 == 0;
                t.set(x, y, if ring { pale } else { dark });
            }
        }
    }
    t.set(7, 7, shade(dark, 0.8));
    t.set(8, 8, shade(dark, 0.8));
}

// -- foliage ----------------------------------------------------------------

/// Clumpy leaves with cut-out holes. `hole` is the noise threshold below which
/// a texel is punched out; higher means airier foliage.
pub fn leaves(t: &mut Tex, light: Rgba, dark: Rgba, seed: u32, hole: f32) {
    for y in 0..TILE {
        for x in 0..TILE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let n = fbm(fx, fy, seed);
            if n < hole {
                t.set(x, y, CLEAR);
                continue;
            }
            let clump = vnoise(fx * 1.5, fy * 1.5, 8, seed ^ 0x1234);
            let g = rand01(x as i32, y as i32, seed ^ 0x4321) - 0.5;
            let c = mix(dark, light, clump);
            t.set(x, y, shade(c, 1.0 + g * 0.24));
        }
    }
    // Darken the texels that border a hole, so the cut-out edge reads as depth
    // rather than as a torn sticker.
    let snapshot: Vec<Rgba> = (0..TILE * TILE)
        .map(|i| t.get(i % TILE, i / TILE))
        .collect();
    for y in 0..TILE {
        for x in 0..TILE {
            if snapshot[y * TILE + x][3] == 0 {
                continue;
            }
            let n = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dy)| {
                    let nx = (x as i32 + dx).rem_euclid(TILE as i32) as usize;
                    let ny = (y as i32 + dy).rem_euclid(TILE as i32) as usize;
                    snapshot[ny * TILE + nx][3] == 0
                });
            if n {
                t.darken(x, y, 0.78);
            }
        }
    }
}

/// Spruce foliage: darker, with vertical needle strokes instead of clumps.
pub fn needles(t: &mut Tex, light: Rgba, dark: Rgba, seed: u32) {
    leaves(t, light, dark, seed, 0.36);
    for i in 0..16 {
        let x = (rand01(i, 71, seed) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 72, seed) * TILE as f32) as usize % TILE;
        if t.get(x, y)[3] == 0 {
            continue;
        }
        let f = if i % 2 == 0 { 1.25 } else { 0.8 };
        t.darken(x, y, f);
        let y2 = (y + 1) % TILE;
        if t.get(x, y2)[3] != 0 {
            t.darken(x, y2, f);
        }
    }
}

pub fn tall_grass(t: &mut Tex) {
    let seed = 0x7201;
    let greens = [rgb(96, 154, 58), rgb(74, 132, 46), rgb(120, 176, 72)];
    for b in 0..9 {
        let base_x = (rand01(b, 81, seed) * TILE as f32) as i32;
        let top = 3 + (rand01(b, 82, seed) * 7.0) as i32; // highest texel
        let bend = if rand01(b, 83, seed) > 0.5 { 1 } else { -1 };
        let c = greens[(b as usize) % greens.len()];
        let mut x = base_x;
        for y in (top..TILE as i32).rev() {
            let xx = x.rem_euclid(TILE as i32) as usize;
            t.set(xx, y as usize, shade(c, 1.0 - (y as f32 / 40.0)));
            // Blades lean over as they rise.
            if (TILE as i32 - y) % 3 == 0 {
                x += bend;
            }
        }
        // A brighter tip.
        let xx = x.rem_euclid(TILE as i32) as usize;
        t.set(xx, top.max(0) as usize, shade(c, 1.25));
    }
}

pub fn flower(t: &mut Tex, petal: Rgba, petal_hi: Rgba, centre: Rgba) {
    let stem = rgb(70, 128, 50);
    let stem_d = rgb(52, 100, 38);
    let art: [&str; TILE] = [
        "................",
        "................",
        ".....p.pp.p.....",
        "....pPPPPPPp....",
        "....pPPccPPp....",
        "....pPPccPPp....",
        "....pPPPPPPp....",
        ".....p.pp.p.....",
        ".......ss.......",
        "......s.ss......",
        "....ll.ss.......",
        "...lLl.ss.......",
        ".......ss.ll....",
        ".......ss.lLl...",
        "........ss......",
        "........s.......",
    ];
    t.art(
        &art,
        &[
            ('p', shade(petal, 0.78)),
            ('P', petal),
            ('c', centre),
            ('s', stem),
            ('S', stem_d),
            ('l', stem_d),
            ('L', shade(stem, 1.15)),
        ],
    );
    // A highlight on the upper-left petals.
    t.set(5, 3, petal_hi);
    t.set(6, 3, petal_hi);
    t.set(5, 4, petal_hi);
}

pub fn dead_bush(t: &mut Tex) {
    let a = rgb(140, 106, 52);
    let b = rgb(108, 80, 38);
    let art: [&str; TILE] = [
        "................",
        "................",
        "......a.........",
        ".....a.a...a....",
        "......a.a.a.....",
        "...a...a.a......",
        "....a...a.......",
        ".....a..a..a....",
        "......b.b.b.....",
        ".......bb.......",
        "....b..bb..b....",
        ".....b.bb.b.....",
        ".......bb.......",
        "......b.bb......",
        ".......bb.......",
        "......b..b......",
    ];
    t.art(&art, &[('a', a), ('b', b)]);
}

// -- ores -------------------------------------------------------------------

pub fn ore(t: &mut Tex, core: Rgba, edge: Rgba, seed: u32, gems: bool) {
    stone(t, STONE_BASE, 0x5701);
    if gems {
        // Diamonds read as faceted crystals, not lumps.
        let spark = rgb(232, 255, 255);
        gem(t, 4, 4, 2, core, edge, spark);
        gem(t, 11, 6, 2, core, edge, spark);
        gem(t, 6, 11, 2, core, edge, spark);
        gem(t, 12, 13, 1, core, edge, spark);
        return;
    }
    let count = 4;
    for i in 0..count {
        let cx = 1.5 + rand01(i, 91, seed) * 13.0;
        let cy = 1.5 + rand01(i, 92, seed) * 13.0;
        let r = 2.0 + rand01(i, 93, seed) * 1.7;
        ore_blob(t, cx, cy, r, seed.wrapping_add(i as u32 * 17), core, edge);
    }
}

// -- liquids and specials ---------------------------------------------------

pub fn water(t: &mut Tex) {
    for y in 0..TILE {
        for x in 0..TILE {
            let n = fbm(x as f32 * 1.0, y as f32 * 1.0, 0x7301);
            let c = mix(rgb(34, 82, 168), rgb(70, 128, 210), n);
            // Alpha varies a little with the wave pattern, so the surface is not
            // a flat sheet of glass.
            let a = (168.0 + n * 42.0) as u8;
            t.set(x, y, [c[0], c[1], c[2], a]);
        }
    }
    // Bright crests.
    for i in 0..5 {
        let x = (rand01(i, 94, 0x7301) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 95, 0x7301) * TILE as f32) as usize % TILE;
        let c = t.get(x, y);
        t.set(
            x,
            y,
            [shade(c, 1.3)[0], shade(c, 1.3)[1], shade(c, 1.3)[2], c[3]],
        );
    }
}

/// A pail: a tapered iron body with a rim and a handle.
pub fn bucket(t: &mut Tex, full: bool) {
    let body = rgb(148, 152, 160);
    for y in 4..15 {
        // Tapered, so it reads as a pail rather than a tin can.
        let inset = ((y - 4) / 5) as usize;
        for x in (3 + inset)..(13 - inset) {
            let f = 1.0 - (x as f32 - 3.0) / 12.0 * 0.35;
            t.set(x, y, shade(body, 0.82 + f * 0.4));
        }
    }
    // Rim.
    for x in 2..14 {
        t.set(x, 3, shade(body, 1.25));
        t.set(x, 4, shade(body, 1.1));
    }
    // Handle, an arc over the rim.
    for (x, y) in [(3, 2), (4, 1), (6, 0), (9, 0), (11, 1), (12, 2)] {
        t.set(x, y, shade(body, 0.7));
    }
    if full {
        // Water sits inside the rim, below it, so the pail still reads as a pail.
        for y in 5..13 {
            let inset = ((y - 4) / 5) as usize;
            for x in (4 + inset)..(12 - inset) {
                let n = fbm(x as f32, y as f32, 0x7301);
                t.set(x, y, mix(rgb(38, 92, 176), rgb(66, 126, 206), n));
            }
        }
        for x in 4..12 {
            t.set(x, 5, rgb(96, 158, 224));
        }
    }
    // A dark outline so it separates from the slot behind it.
    for y in 0..TILE {
        for x in 0..TILE {
            if t.get(x, y)[3] == 0 {
                continue;
            }
            let edge = [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dy)| {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    nx < 0
                        || ny < 0
                        || nx >= TILE as i32
                        || ny >= TILE as i32
                        || t.get(nx as usize, ny as usize)[3] == 0
                });
            if edge {
                let c = t.get(x, y);
                t.set(x, y, shade(c, 0.55));
            }
        }
    }
}

pub fn cactus_side(t: &mut Tex) {
    noisy_fill(t, rgb(66, 128, 52), 0x7401, 0.07, 0.05);
    // Ribs down the flanks, brighter in the middle where the light catches.
    for y in 0..TILE {
        t.darken(0, y, 0.7);
        t.darken(1, y, 0.82);
        t.darken(14, y, 0.82);
        t.darken(15, y, 0.7);
        t.darken(7, y, 1.1);
        t.darken(8, y, 1.1);
    }
    // Spines on a staggered lattice.
    for row in 0..4 {
        for col in 0..3 {
            let x = 3 + col * 5 + (row % 2) * 2;
            let y = 2 + row * 4;
            t.set(x % TILE, y % TILE, rgb(226, 228, 200));
            t.set((x + 1) % TILE, (y + 1) % TILE, rgb(180, 182, 150));
        }
    }
}

pub fn cactus_top(t: &mut Tex) {
    noisy_fill(t, rgb(84, 150, 62), 0x7402, 0.08, 0.05);
    for y in 0..TILE {
        for x in 0..TILE {
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 7.5;
            let d = (dx * dx + dy * dy).sqrt();
            if d > 6.4 {
                t.darken(x, y, 0.74);
            } else if d < 2.4 {
                t.darken(x, y, 1.14);
            }
        }
    }
    for i in 0..6 {
        let a = i as f32 / 6.0 * std::f32::consts::TAU;
        let x = (7.5 + a.cos() * 4.5) as usize % TILE;
        let y = (7.5 + a.sin() * 4.5) as usize % TILE;
        t.set(x, y, rgb(226, 228, 200));
    }
}

pub fn cactus_bottom(t: &mut Tex) {
    noisy_fill(t, rgb(58, 108, 46), 0x7403, 0.1, 0.07);
    for y in 0..TILE {
        for x in 0..TILE {
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 7.5;
            if (dx * dx + dy * dy).sqrt() < 3.0 {
                t.darken(x, y, 0.8);
            }
        }
    }
}

/// A torch, drawn to be sampled two ways: the block model takes the narrow
/// column `x 7..9`, and the inventory icon takes the whole tile.
pub fn torch(t: &mut Tex) {
    let art: [&str; TILE] = [
        "................",
        "................",
        ".......ww.......",
        "......wYYw......",
        "......YYYY......",
        "......fYYf......",
        ".......EE.......",
        ".......EE.......",
        ".......ss.......",
        ".......ss.......",
        ".......Ss.......",
        ".......Ss.......",
        ".......sS.......",
        ".......sS.......",
        ".......ss.......",
        ".......Ss.......",
    ];
    t.art(
        &art,
        &[
            ('w', rgb(255, 246, 206)),
            ('Y', rgb(255, 214, 92)),
            ('f', rgb(238, 150, 44)),
            ('E', rgb(255, 232, 150)),
            ('s', rgb(140, 100, 56)),
            ('S', rgb(104, 72, 38)),
        ],
    );
}

pub fn table_top(t: &mut Tex) {
    planks(t, PLANK_BASE, 0x7501);
    // A 3x3 grid scored into the surface.
    for i in 0..TILE {
        for &g in &[5usize, 10] {
            t.darken(g, i, 0.62);
            t.darken(i, g, 0.62);
        }
    }
    for &g in &[5usize, 10] {
        for i in 0..TILE {
            t.darken(g + 1, i, 1.1);
            t.darken(i, g + 1, 1.1);
        }
    }
    // Corner rivets.
    for (x, y) in [(1usize, 1usize), (14, 1), (1, 14), (14, 14)] {
        t.set(x, y, rgb(84, 62, 38));
    }
}

pub fn table_side(t: &mut Tex) {
    planks(t, shade(PLANK_BASE, 0.9), 0x7502);
    // A saw hanging on the panel.
    let art: [&str; TILE] = [
        "................",
        "..dddddddddd....",
        "..dLLLLLLLLd....",
        "..dddddddddd....",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
        "................",
    ];
    t.art(&art, &[('d', rgb(70, 52, 32)), ('L', rgb(198, 200, 206))]);
    // Saw teeth.
    for x in (3..13).step_by(2) {
        t.set(x, 4, rgb(198, 200, 206));
    }
}

pub fn table_front(t: &mut Tex) {
    planks(t, shade(PLANK_BASE, 0.94), 0x7503);
    let art: [&str; TILE] = [
        "................",
        "...HHHH..hhhh...",
        "...HHHH..hhhh...",
        "....dd....dd....",
        "....dd....dd....",
        "....dd....dd....",
        "....dd....dd....",
        "................",
        "................",
        "..MMMMMMMMMMMM..",
        "..MmmmmmmmmmmM..",
        "..MmMMMMMMMMmM..",
        "..MmMMMMMMMMmM..",
        "..MmmmmmmmmmmM..",
        "..MMMMMMMMMMMM..",
        "................",
    ];
    t.art(
        &art,
        &[
            ('H', rgb(176, 178, 186)),
            ('h', rgb(140, 142, 150)),
            ('d', rgb(96, 68, 38)),
            ('M', rgb(86, 62, 36)),
            ('m', rgb(126, 96, 58)),
        ],
    );
}

pub fn furnace_top(t: &mut Tex) {
    cobbles(t, rgb(118, 118, 124), 0x7601, 8, 1.0, 0.32);
    // The stoke hole.
    for y in 4..12 {
        for x in 4..12 {
            let dx = x as f32 - 7.5;
            let dy = y as f32 - 7.5;
            if (dx * dx + dy * dy).sqrt() < 3.4 {
                t.set(x, y, rgb(42, 42, 46));
            }
        }
    }
}

pub fn furnace_side(t: &mut Tex) {
    cobbles(t, rgb(112, 112, 118), 0x7602, 9, 1.0, 0.34);
    for x in 0..TILE {
        t.darken(x, 0, 1.14);
        t.darken(x, 15, 0.8);
    }
}

pub fn furnace_front(t: &mut Tex) {
    cobbles(t, rgb(112, 112, 118), 0x7603, 9, 1.0, 0.28);
    // Iron-bound mouth with a lintel above it and coals inside.
    t.rect(2, 5, 12, 2, rgb(74, 74, 80));
    t.rect(2, 7, 12, 8, rgb(30, 28, 30));
    t.rect(3, 8, 10, 6, rgb(16, 14, 16));
    for x in 3..13 {
        t.set(x, 7, rgb(58, 56, 58));
    }
    // Embers along the floor of the mouth.
    let embers = [
        (4usize, 12usize),
        (5, 13),
        (6, 12),
        (7, 13),
        (8, 12),
        (9, 13),
        (10, 12),
        (11, 13),
    ];
    for (i, (x, y)) in embers.iter().enumerate() {
        let c = if i % 3 == 0 {
            rgb(255, 196, 84)
        } else if i % 3 == 1 {
            rgb(226, 118, 34)
        } else {
            rgb(158, 58, 18)
        };
        t.set(*x, *y, c);
    }
    for x in 2..14 {
        t.set(x, 14, rgb(52, 50, 52));
    }
    // Bright rivets on the lintel.
    t.set(3, 5, rgb(158, 158, 166));
    t.set(12, 5, rgb(158, 158, 166));
}

// -- items ------------------------------------------------------------------

pub fn stick(t: &mut Tex) {
    let art: [&str; TILE] = [
        "................",
        "................",
        "................",
        "...........ss...",
        "..........sSs...",
        ".........sSs....",
        "........sSs.....",
        ".......sSs......",
        "......sSs.......",
        ".....sSs........",
        "....sSs.........",
        "...sSs..........",
        "...ss...........",
        "................",
        "................",
        "................",
    ];
    t.art(&art, &[('s', rgb(150, 110, 62)), ('S', rgb(108, 78, 42))]);
}

pub fn nugget(t: &mut Tex, base: Rgba, hi: Rgba, lo: Rgba) {
    let art: [&str; TILE] = [
        "................",
        "................",
        "................",
        ".....LLL........",
        "....LHHBL.......",
        "...LHHBBBL......",
        "...LHBBBBBL.....",
        "..LBBBBBBBL.....",
        "..LBBBBBBBBL....",
        "..LBBBBBBBBL....",
        "...LBBBBBBL.....",
        "....LBBBBL......",
        ".....LLLL.......",
        "................",
        "................",
        "................",
    ];
    t.art(&art, &[('B', base), ('H', hi), ('L', lo)]);
}

pub fn ingot(t: &mut Tex) {
    let base = rgb(214, 214, 220);
    let art: [&str; TILE] = [
        "................",
        "................",
        "................",
        "................",
        "................",
        "....HHHHHHHH....",
        "...HBBBBBBBBH...",
        "..HBBBBBBBBBBH..",
        "..LBBBBBBBBBBL..",
        "..LBBBBBBBBBBL..",
        "..LLLLLLLLLLLL..",
        "................",
        "................",
        "................",
        "................",
        "................",
    ];
    t.art(
        &art,
        &[
            ('B', base),
            ('H', shade(base, 1.12)),
            ('L', shade(base, 0.66)),
        ],
    );
}

/// Palette for one tool tier: `(head, head highlight, head shadow)`.
pub type TierPal = (Rgba, Rgba, Rgba);

pub const TIER_WOOD: TierPal = (rgb(163, 124, 74), rgb(198, 158, 104), rgb(112, 82, 46));
pub const TIER_STONE: TierPal = (rgb(136, 136, 142), rgb(178, 178, 184), rgb(90, 90, 96));
pub const TIER_IRON: TierPal = (rgb(214, 214, 220), rgb(244, 244, 248), rgb(148, 148, 156));

pub const PICKAXE_ART: [&str; TILE] = [
    "................",
    "..HHH.....HHHH..",
    ".HhhhHHHHHhhhhH.",
    ".HhhhhhhhhhhhhH.",
    ".HdHH..SS..HHdH.",
    "..dd...SS...dd..",
    "......SS........",
    "......SS........",
    ".....SS.........",
    ".....SS.........",
    "....SS..........",
    "....SS..........",
    "...SS...........",
    "...SS...........",
    "..SS............",
    "..S.............",
];

pub const AXE_ART: [&str; TILE] = [
    "................",
    ".......HHHH.....",
    "......HhhhhH....",
    "......HhhhhhH...",
    "......ShhhhhH...",
    "......SHhhhhH...",
    "......SSHhhhH...",
    ".....SS.HhhH....",
    ".....SS..dd.....",
    "....SS..........",
    "....SS..........",
    "...SS...........",
    "...SS...........",
    "..SS............",
    "..SS............",
    "..S.............",
];

pub const SWORD_ART: [&str; TILE] = [
    "............HH..",
    "...........HhH..",
    "..........HhhH..",
    ".........HhhH...",
    "........HhhH....",
    ".......HhhH.....",
    "......HhhH......",
    ".....HhhH.......",
    "....dHhH........",
    "...ddHH.........",
    "..dddd..........",
    "..SSdd..........",
    ".SS.d...........",
    ".SS.............",
    "SS..............",
    "S...............",
];

pub fn tool(t: &mut Tex, art: &[&str; TILE], pal: TierPal) {
    let (head, hi, lo) = pal;
    t.art(
        art,
        &[
            ('H', head),
            ('h', hi),
            ('d', lo),
            ('S', rgb(140, 100, 56)),
            ('s', rgb(104, 72, 38)),
        ],
    );
}

pub fn arrow(t: &mut Tex) {
    let art: [&str; TILE] = [
        "................",
        "................",
        "................",
        "...........HH...",
        "..........HHH...",
        ".........HHH....",
        "........SSS.....",
        ".......SSS......",
        "......SSS.......",
        ".....SSS........",
        "....FSS.........",
        "...FFS..........",
        "..FFF...........",
        "...F............",
        "................",
        "................",
    ];
    t.art(
        &art,
        &[
            ('H', rgb(216, 216, 222)),
            ('S', rgb(140, 100, 56)),
            ('F', rgb(238, 238, 240)),
        ],
    );
}

pub fn missing(t: &mut Tex) {
    for y in 0..TILE {
        for x in 0..TILE {
            let c = if ((x / 4) + (y / 4)) % 2 == 0 {
                rgb(232, 0, 232)
            } else {
                rgb(24, 24, 24)
            };
            t.set(x, y, c);
        }
    }
}

// -- mobs -------------------------------------------------------------------

/// Plain mottled skin, used for the sides, back and top of a head and for the
/// limbs. Deterministic mottling makes each species read differently even from
/// behind.
pub fn mob_skin(t: &mut Tex, base: Rgba, dark: Rgba, seed: u32) {
    for y in 0..TILE {
        for x in 0..TILE {
            let n = fbm(x as f32, y as f32, seed);
            t.set(x, y, mix(dark, base, n));
        }
    }
    for i in 0..10 {
        let x = (rand01(i, 101, seed) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 102, seed) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.86);
    }
}

pub fn zombie_face(t: &mut Tex) {
    mob_skin(t, rgb(76, 140, 82), rgb(58, 112, 64), 0x7101);
    let art: [&str; TILE] = [
        "................",
        "................",
        "................",
        "................",
        "..EEE......EEE..",
        "..EppE....EppE..",
        "..EEEE....EEEE..",
        "................",
        "................",
        ".......nn.......",
        "................",
        "....mmmmmmmm....",
        "....mMMMMMMm....",
        "....mmmmmmmm....",
        "................",
        "................",
    ];
    t.art(
        &art,
        &[
            ('E', rgb(18, 26, 20)),
            ('p', rgb(126, 20, 20)),
            ('n', rgb(46, 92, 52)),
            ('m', rgb(22, 34, 24)),
            ('M', rgb(52, 70, 50)),
        ],
    );
}

pub fn zombie_body(t: &mut Tex) {
    mob_skin(t, rgb(64, 96, 132), rgb(44, 70, 102), 0x7105);
    // A ragged shirt hem and torn patches.
    for x in 0..TILE {
        t.darken(x, 13, 0.72);
        if (x + 1) % 3 == 0 {
            t.darken(x, 14, 0.66);
        }
    }
    for i in 0..5 {
        let x = (rand01(i, 103, 0x7105) * 12.0) as usize + 2;
        let y = (rand01(i, 104, 0x7105) * 9.0) as usize + 2;
        t.set(x, y, rgb(76, 140, 82));
        t.set(x + 1, y, rgb(66, 124, 74));
    }
}

pub fn skeleton_face(t: &mut Tex) {
    mob_skin(t, rgb(212, 212, 202), rgb(180, 180, 170), 0x7102);
    let art: [&str; TILE] = [
        "................",
        "................",
        "................",
        "................",
        "..EEEE....EEEE..",
        "..EEEE....EEEE..",
        "..EEEE....EEEE..",
        "................",
        "................",
        ".......dd.......",
        "................",
        "...TtTtTtTtTt...",
        "...tTtTtTtTtT...",
        "................",
        "................",
        "................",
    ];
    t.art(
        &art,
        &[
            ('E', rgb(16, 16, 18)),
            ('d', rgb(150, 150, 142)),
            ('T', rgb(240, 240, 234)),
            ('t', rgb(90, 90, 86)),
        ],
    );
}

pub fn skeleton_body(t: &mut Tex) {
    mob_skin(t, rgb(206, 206, 196), rgb(172, 172, 164), 0x7106);
    // Spine and ribs.
    for y in 1..15 {
        t.set(7, y, rgb(150, 150, 142));
        t.set(8, y, rgb(228, 228, 222));
    }
    for y in [3usize, 6, 9, 12] {
        for x in 2..14 {
            let f = if x == 7 || x == 8 { 1.0 } else { 0.7 };
            t.darken(x, y, f);
        }
    }
}

pub fn creeper_face(t: &mut Tex) {
    mob_skin(t, rgb(72, 176, 78), rgb(48, 134, 56), 0x7103);
    // Deliberately its own design: wide diamond eyes and a saw-toothed mouth.
    let art: [&str; TILE] = [
        "................",
        "................",
        "...E......E.....",
        "..EEE....EEE....",
        ".EEEEE..EEEEE...",
        "..EEE....EEE....",
        "...E......E.....",
        "................",
        "....EE....EE....",
        "...EEEE..EEEE...",
        "....EEEEEEEE....",
        ".....EEEEEE.....",
        "....EE.EE.EE....",
        "...EE...E...EE..",
        "................",
        "................",
    ];
    t.art(&art, &[('E', rgb(14, 26, 16))]);
}

pub fn creeper_body(t: &mut Tex) {
    // Camouflage: two greens in irregular vertical patches.
    for y in 0..TILE {
        for x in 0..TILE {
            let n = fbm(x as f32 * 1.4, y as f32 * 0.7, 0x7107);
            let c = if n > 0.52 {
                rgb(84, 190, 88)
            } else if n > 0.42 {
                rgb(64, 158, 70)
            } else {
                rgb(44, 122, 52)
            };
            t.set(x, y, c);
        }
    }
    for i in 0..12 {
        let x = (rand01(i, 105, 0x7107) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 106, 0x7107) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.88);
    }
}

pub fn pig_face(t: &mut Tex) {
    mob_skin(t, rgb(230, 152, 154), rgb(206, 126, 130), 0x7104);
    let art: [&str; TILE] = [
        "................",
        "................",
        "................",
        "...EE......EE...",
        "...EE......EE...",
        "................",
        "................",
        "....SSSSSSSS....",
        "...SNNNNNNNNS...",
        "...SNNhhNNhhS...",
        "...SNNhhNNhhS...",
        "...SNNNNNNNNS...",
        "....SSSSSSSS....",
        "................",
        "................",
        "................",
    ];
    t.art(
        &art,
        &[
            ('E', rgb(20, 16, 18)),
            ('S', rgb(186, 104, 110)),
            ('N', rgb(222, 138, 142)),
            ('h', rgb(120, 62, 70)),
        ],
    );
}

pub fn pig_body(t: &mut Tex) {
    mob_skin(t, rgb(228, 148, 150), rgb(202, 122, 126), 0x7108);
    // A soft belly highlight and a couple of dark spots.
    for y in 9..15 {
        for x in 3..13 {
            t.darken(x, y, 1.06);
        }
    }
    for (x, y) in [(4usize, 4usize), (11, 6), (7, 3)] {
        t.set(x, y, rgb(190, 112, 118));
        t.set(x + 1, y, rgb(196, 118, 124));
        t.set(x, y + 1, rgb(196, 118, 124));
    }
}
