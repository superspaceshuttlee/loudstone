//! Procedural pixel art: the block/item/mob texture atlas, generated in code.
//!
//! Nothing here is loaded from disk. Every tile is 16x16 texels painted by a
//! small recipe -- seeded value noise, Voronoi cobbles, hand-placed pixel art --
//! so the atlas is byte-identical on every run and on every machine.
//!
//! # Layout
//!
//! Tiles are packed into a [`ATLAS_COLS`] x [`ATLAS_ROWS`] grid of 16x16 tiles,
//! giving a 256x256 RGBA image. Tile `n` lives at column `n % ATLAS_COLS`, row
//! `n / ATLAS_COLS`.
//!
//! # Why there is no padding between tiles
//!
//! The usual reason to pad an atlas is that mip generation averages across tile
//! borders. That cannot happen here: the tile size and the atlas size are both
//! powers of two and tile-aligned, so a 2x2 box filter never straddles a tile
//! boundary until a tile is down to a single texel. The chain therefore stops at
//! [`MIP_LEVELS`] = 5 (16, 8, 4, 2, 1 texels per tile) and every level is still
//! exactly one tile per grid cell. Within a level, `shader.wgsl` converts the
//! tile-local UV into an atlas UV and clamps it strictly inside the tile, and
//! the sampler magnifies and minifies with nearest neighbour, so no filtering
//! can reach a neighbouring tile either.

#![allow(clippy::needless_range_loop)]

use crate::block::BlockId;
use crate::item::{ItemId, ToolKind, ToolTier};

// ---------------------------------------------------------------------------
// Geometry of the atlas
// ---------------------------------------------------------------------------

/// Edge length of one tile, in texels.
pub const TILE: usize = 16;
/// Tiles across the atlas.
pub const ATLAS_COLS: usize = 16;
/// Tiles down the atlas.
pub const ATLAS_ROWS: usize = 16;
/// Atlas width in texels.
pub const ATLAS_W: usize = TILE * ATLAS_COLS;
/// Atlas height in texels.
pub const ATLAS_H: usize = TILE * ATLAS_ROWS;
/// Mip levels generated. Level 4 is one texel per tile; going further would
/// blend neighbouring tiles together.
pub const MIP_LEVELS: u32 = 5;

// ---------------------------------------------------------------------------
// Tile ids
// ---------------------------------------------------------------------------

/// Index of a tile in the atlas.
pub type TileId = u16;

pub const T_WHITE: TileId = 0;
pub const T_STONE: TileId = 1;
pub const T_DIRT: TileId = 2;
pub const T_GRASS_TOP: TileId = 3;
pub const T_GRASS_SIDE: TileId = 4;
pub const T_GRASS_TOP_COLD: TileId = 5;
pub const T_GRASS_SIDE_COLD: TileId = 6;
pub const T_GRASS_TOP_DRY: TileId = 7;
pub const T_GRASS_SIDE_DRY: TileId = 8;
pub const T_GRASS_TOP_SWAMP: TileId = 9;
pub const T_GRASS_SIDE_SWAMP: TileId = 10;
pub const T_PODZOL_TOP: TileId = 11;
pub const T_PODZOL_SIDE: TileId = 12;
pub const T_SAND: TileId = 13;
pub const T_SANDSTONE_TOP: TileId = 14;
pub const T_SANDSTONE_SIDE: TileId = 15;
pub const T_SANDSTONE_BOTTOM: TileId = 16;
pub const T_GRAVEL: TileId = 17;
pub const T_CLAY: TileId = 18;
pub const T_SNOW: TileId = 19;
pub const T_ICE: TileId = 20;
pub const T_GRANITE: TileId = 21;
pub const T_DIORITE: TileId = 22;
pub const T_ANDESITE: TileId = 23;
pub const T_COBBLESTONE: TileId = 24;
pub const T_BEDROCK: TileId = 25;
pub const T_PLANKS: TileId = 26;
pub const T_OAK_LOG_SIDE: TileId = 27;
pub const T_OAK_LOG_TOP: TileId = 28;
pub const T_BIRCH_LOG_SIDE: TileId = 29;
pub const T_BIRCH_LOG_TOP: TileId = 30;
pub const T_SPRUCE_LOG_SIDE: TileId = 31;
pub const T_SPRUCE_LOG_TOP: TileId = 32;
pub const T_OAK_LEAVES: TileId = 33;
pub const T_BIRCH_LEAVES: TileId = 34;
pub const T_SPRUCE_LEAVES: TileId = 35;
pub const T_COAL_ORE: TileId = 36;
pub const T_IRON_ORE: TileId = 37;
pub const T_GOLD_ORE: TileId = 38;
pub const T_DIAMOND_ORE: TileId = 39;
pub const T_WATER: TileId = 40;
pub const T_CACTUS_SIDE: TileId = 41;
pub const T_CACTUS_TOP: TileId = 42;
pub const T_CACTUS_BOTTOM: TileId = 43;
pub const T_TORCH: TileId = 44;
pub const T_TABLE_TOP: TileId = 45;
pub const T_TABLE_SIDE: TileId = 46;
pub const T_TABLE_FRONT: TileId = 47;
pub const T_FURNACE_TOP: TileId = 48;
pub const T_FURNACE_SIDE: TileId = 49;
pub const T_FURNACE_FRONT: TileId = 50;
pub const T_TALL_GRASS: TileId = 51;
pub const T_FLOWER_RED: TileId = 52;
pub const T_FLOWER_YELLOW: TileId = 53;
pub const T_DEAD_BUSH: TileId = 54;

pub const T_STICK: TileId = 55;
pub const T_COAL: TileId = 56;
pub const T_RAW_IRON: TileId = 57;
pub const T_IRON_INGOT: TileId = 58;
pub const T_PICK_WOOD: TileId = 59;
pub const T_PICK_STONE: TileId = 60;
pub const T_PICK_IRON: TileId = 61;
pub const T_AXE_WOOD: TileId = 62;
pub const T_AXE_STONE: TileId = 63;
pub const T_AXE_IRON: TileId = 64;
pub const T_SWORD_WOOD: TileId = 65;
pub const T_SWORD_STONE: TileId = 66;
pub const T_SWORD_IRON: TileId = 67;

pub const T_ZOMBIE_FACE: TileId = 68;
pub const T_ZOMBIE_HEAD: TileId = 69;
pub const T_ZOMBIE_BODY: TileId = 70;
pub const T_SKELETON_FACE: TileId = 71;
pub const T_SKELETON_HEAD: TileId = 72;
pub const T_SKELETON_BODY: TileId = 73;
pub const T_CREEPER_FACE: TileId = 74;
pub const T_CREEPER_HEAD: TileId = 75;
pub const T_CREEPER_BODY: TileId = 76;
pub const T_PIG_FACE: TileId = 77;
pub const T_PIG_HEAD: TileId = 78;
pub const T_PIG_BODY: TileId = 79;
pub const T_ARROW: TileId = 80;
pub const T_MISSING: TileId = 81;

/// Number of tiles the atlas actually paints.
pub const TILE_COUNT: usize = 82;

/// Human-readable name per tile, for test failures and the atlas dump.
pub const TILE_NAMES: [&str; TILE_COUNT] = [
    "white",
    "stone",
    "dirt",
    "grass_top",
    "grass_side",
    "grass_top_cold",
    "grass_side_cold",
    "grass_top_dry",
    "grass_side_dry",
    "grass_top_swamp",
    "grass_side_swamp",
    "podzol_top",
    "podzol_side",
    "sand",
    "sandstone_top",
    "sandstone_side",
    "sandstone_bottom",
    "gravel",
    "clay",
    "snow",
    "ice",
    "granite",
    "diorite",
    "andesite",
    "cobblestone",
    "bedrock",
    "planks",
    "oak_log_side",
    "oak_log_top",
    "birch_log_side",
    "birch_log_top",
    "spruce_log_side",
    "spruce_log_top",
    "oak_leaves",
    "birch_leaves",
    "spruce_leaves",
    "coal_ore",
    "iron_ore",
    "gold_ore",
    "diamond_ore",
    "water",
    "cactus_side",
    "cactus_top",
    "cactus_bottom",
    "torch",
    "table_top",
    "table_side",
    "table_front",
    "furnace_top",
    "furnace_side",
    "furnace_front",
    "tall_grass",
    "flower_red",
    "flower_yellow",
    "dead_bush",
    "stick",
    "coal",
    "raw_iron",
    "iron_ingot",
    "pick_wood",
    "pick_stone",
    "pick_iron",
    "axe_wood",
    "axe_stone",
    "axe_iron",
    "sword_wood",
    "sword_stone",
    "sword_iron",
    "zombie_face",
    "zombie_head",
    "zombie_body",
    "skeleton_face",
    "skeleton_head",
    "skeleton_body",
    "creeper_face",
    "creeper_head",
    "creeper_body",
    "pig_face",
    "pig_head",
    "pig_body",
    "arrow",
    "missing",
];

// ---------------------------------------------------------------------------
// Block -> tile
// ---------------------------------------------------------------------------

/// Face order, matching `mesh::FACE_NORMALS`: +Y, -Y, +Z, -Z, +X, -X.
pub const FACE_TOP: usize = 0;
pub const FACE_BOTTOM: usize = 1;
pub const FACE_NORTH: usize = 3; // -Z, the direction a furnace faces

/// The tile a block shows on one of its six faces.
///
/// This is where "grass is green on top, dirt underneath, and fringed on the
/// side" lives, along with log end grain and the furnace front.
pub fn block_tile(id: BlockId, face: usize) -> TileId {
    let top = face == FACE_TOP;
    let bottom = face == FACE_BOTTOM;
    let side = !top && !bottom;
    match id {
        BlockId::STONE => T_STONE,
        BlockId::DIRT => T_DIRT,
        BlockId::GRASS => grassy(top, side, T_GRASS_TOP, T_GRASS_SIDE),
        BlockId::GRASS_COLD => grassy(top, side, T_GRASS_TOP_COLD, T_GRASS_SIDE_COLD),
        BlockId::GRASS_DRY => grassy(top, side, T_GRASS_TOP_DRY, T_GRASS_SIDE_DRY),
        BlockId::GRASS_SWAMP => grassy(top, side, T_GRASS_TOP_SWAMP, T_GRASS_SIDE_SWAMP),
        BlockId::PODZOL => grassy(top, side, T_PODZOL_TOP, T_PODZOL_SIDE),
        BlockId::SAND => T_SAND,
        BlockId::SANDSTONE => {
            if top {
                T_SANDSTONE_TOP
            } else if bottom {
                T_SANDSTONE_BOTTOM
            } else {
                T_SANDSTONE_SIDE
            }
        }
        BlockId::GRAVEL => T_GRAVEL,
        BlockId::CLAY => T_CLAY,
        BlockId::SNOW => T_SNOW,
        BlockId::ICE => T_ICE,
        BlockId::GRANITE => T_GRANITE,
        BlockId::DIORITE => T_DIORITE,
        BlockId::ANDESITE => T_ANDESITE,
        BlockId::COBBLESTONE => T_COBBLESTONE,
        BlockId::BEDROCK => T_BEDROCK,
        BlockId::PLANKS => T_PLANKS,
        BlockId::WOOD => log(side, T_OAK_LOG_SIDE, T_OAK_LOG_TOP),
        BlockId::BIRCH_LOG => log(side, T_BIRCH_LOG_SIDE, T_BIRCH_LOG_TOP),
        BlockId::SPRUCE_LOG => log(side, T_SPRUCE_LOG_SIDE, T_SPRUCE_LOG_TOP),
        BlockId::LEAVES => T_OAK_LEAVES,
        BlockId::BIRCH_LEAVES => T_BIRCH_LEAVES,
        BlockId::SPRUCE_LEAVES => T_SPRUCE_LEAVES,
        BlockId::COAL_ORE => T_COAL_ORE,
        BlockId::IRON_ORE => T_IRON_ORE,
        BlockId::GOLD_ORE => T_GOLD_ORE,
        BlockId::DIAMOND_ORE => T_DIAMOND_ORE,
        BlockId::WATER => T_WATER,
        BlockId::CACTUS => {
            if top {
                T_CACTUS_TOP
            } else if bottom {
                T_CACTUS_BOTTOM
            } else {
                T_CACTUS_SIDE
            }
        }
        BlockId::TORCH => T_TORCH,
        BlockId::CRAFTING_TABLE => {
            if top {
                T_TABLE_TOP
            } else if bottom {
                T_PLANKS
            } else if face == 2 || face == 3 {
                T_TABLE_FRONT
            } else {
                T_TABLE_SIDE
            }
        }
        BlockId::FURNACE => {
            if top || bottom {
                T_FURNACE_TOP
            } else if face == FACE_NORTH {
                T_FURNACE_FRONT
            } else {
                T_FURNACE_SIDE
            }
        }
        BlockId::TALL_GRASS => T_TALL_GRASS,
        BlockId::FLOWER_RED => T_FLOWER_RED,
        BlockId::FLOWER_YELLOW => T_FLOWER_YELLOW,
        BlockId::DEAD_BUSH => T_DEAD_BUSH,
        _ => T_MISSING,
    }
}

#[inline]
fn grassy(top: bool, side: bool, top_tile: TileId, side_tile: TileId) -> TileId {
    if top {
        top_tile
    } else if side {
        side_tile
    } else {
        T_DIRT
    }
}

#[inline]
fn log(side: bool, bark: TileId, end: TileId) -> TileId {
    if side { bark } else { end }
}

/// The tile an inventory icon uses for one item.
pub fn item_tile(item: ItemId) -> TileId {
    if let Some(block) = item.places() {
        // Side faces read best as an icon: a grass block shows its fringe, a
        // log shows its bark, a furnace shows its mouth.
        return match block {
            BlockId::FURNACE => T_FURNACE_FRONT,
            BlockId::CRAFTING_TABLE => T_TABLE_FRONT,
            _ => block_tile(block, 2),
        };
    }
    match item {
        ItemId::STICK => T_STICK,
        ItemId::COAL => T_COAL,
        ItemId::RAW_IRON => T_RAW_IRON,
        ItemId::IRON_INGOT => T_IRON_INGOT,
        _ => match item.tool() {
            Some((ToolKind::Pickaxe, tier)) => {
                tier_tile(tier, T_PICK_WOOD, T_PICK_STONE, T_PICK_IRON)
            }
            Some((ToolKind::Axe, tier)) => tier_tile(tier, T_AXE_WOOD, T_AXE_STONE, T_AXE_IRON),
            Some((ToolKind::Sword, tier)) => {
                tier_tile(tier, T_SWORD_WOOD, T_SWORD_STONE, T_SWORD_IRON)
            }
            None => T_MISSING,
        },
    }
}

#[inline]
fn tier_tile(tier: ToolTier, wood: TileId, stone: TileId, iron: TileId) -> TileId {
    match tier {
        ToolTier::Wood => wood,
        ToolTier::Stone => stone,
        ToolTier::Iron => iron,
    }
}

/// The three tiles a mob is drawn with: `(head front, head elsewhere, body)`.
///
/// Indexed the way `mob::MobKind::ALL` is ordered: zombie, skeleton, creeper,
/// pig. `gfx.rs` cannot see `MobKind` without dragging gameplay types into the
/// renderer, so it resolves the index from the mob's flat colour instead.
pub fn mob_tiles(kind: usize) -> (TileId, TileId, TileId) {
    match kind {
        0 => (T_ZOMBIE_FACE, T_ZOMBIE_HEAD, T_ZOMBIE_BODY),
        1 => (T_SKELETON_FACE, T_SKELETON_HEAD, T_SKELETON_BODY),
        2 => (T_CREEPER_FACE, T_CREEPER_HEAD, T_CREEPER_BODY),
        _ => (T_PIG_FACE, T_PIG_HEAD, T_PIG_BODY),
    }
}

/// The flat colours `mob::MobKind::color()` returns, in `MobKind::ALL` order.
/// Duplicated here rather than imported so the renderer stays free of mob code;
/// a test in `gfx.rs` would be the place to catch them drifting apart.
pub const MOB_COLORS: [[f32; 3]; 4] = [
    [0.30, 0.55, 0.32], // zombie
    [0.82, 0.82, 0.78], // skeleton
    [0.22, 0.70, 0.28], // creeper
    [0.90, 0.60, 0.62], // pig
];

// ---------------------------------------------------------------------------
// The atlas itself
// ---------------------------------------------------------------------------

/// A generated mip chain. Level 0 is [`ATLAS_W`] x [`ATLAS_H`] RGBA8.
pub struct Atlas {
    /// `(width, height, rgba)` per mip level, level 0 first.
    pub levels: Vec<(u32, u32, Vec<u8>)>,
}

impl Atlas {
    /// Read one texel of mip level 0.
    pub fn texel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * ATLAS_W + x) * 4;
        let p = &self.levels[0].2;
        [p[i], p[i + 1], p[i + 2], p[i + 3]]
    }

    /// Read one texel of a tile, in tile-local coordinates.
    pub fn tile_texel(&self, tile: TileId, x: usize, y: usize) -> [u8; 4] {
        let (ox, oy) = tile_origin(tile);
        self.texel(ox + x, oy + y)
    }
}

/// Top-left texel of a tile in the atlas.
pub fn tile_origin(tile: TileId) -> (usize, usize) {
    let t = tile as usize;
    ((t % ATLAS_COLS) * TILE, (t / ATLAS_COLS) * TILE)
}

/// Normalised `[u0, v0, u1, v1]` of a tile, inset by a quarter texel so nearest
/// sampling can never land on a neighbouring tile.
pub fn tile_uv_rect(tile: TileId) -> [f32; 4] {
    let (ox, oy) = tile_origin(tile);
    let e = 0.25;
    [
        (ox as f32 + e) / ATLAS_W as f32,
        (oy as f32 + e) / ATLAS_H as f32,
        (ox as f32 + TILE as f32 - e) / ATLAS_W as f32,
        (oy as f32 + TILE as f32 - e) / ATLAS_H as f32,
    ]
}

static ATLAS: std::sync::OnceLock<Atlas> = std::sync::OnceLock::new();

/// The atlas, generated once per process and shared by the renderer and the HUD.
pub fn atlas() -> &'static Atlas {
    ATLAS.get_or_init(build_atlas)
}


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
        [u + d, v, w, d],                 // top
        [u + d + w, v, w, d],             // bottom
        [u + d + w + d, v + d, w, h],     // +Z, the back
        [u + d, v + d, w, h],             // -Z, the face
        [u, v + d, d, h],                 // +X
        [u + d + w, v + d, d, h],         // -X
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
struct Skin {
    px: Vec<Rgba>,
}

impl Skin {
    fn new() -> Self {
        Skin { px: vec![[0, 0, 0, 0]; SKIN_SIZE * SKIN_SIZE] }
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
fn shade_i(c: Rgba, d: i32) -> Rgba {
    [
        (c[0] as i32 + d).clamp(0, 255) as u8,
        (c[1] as i32 + d).clamp(0, 255) as u8,
        (c[2] as i32 + d).clamp(0, 255) as u8,
        c[3],
    ]
}

fn hash2(x: u32, y: u32, seed: u32) -> u32 {
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
pub const QUAD_SNOUT_SIZE: (usize, usize, usize) = (4, 3, 2);
pub const QUAD_LEG_UV: (usize, usize) = (0, 44);
pub const QUAD_LEG_SIZE: (usize, usize, usize) = (4, 8, 4);

/// Paint one mob's skin sheet.
fn paint_skin(which: usize, sk: &mut Skin) {
    // Palettes chosen to sit beside the block atlas rather than shout over it.
    let (skin_c, shirt_c, trouser_c, seed) = match which {
        SKIN_ZOMBIE => ([0x4C, 0x7A, 0x3F, 255], [0x3A, 0x4E, 0x74, 255], [0x2E, 0x3A, 0x52, 255], 11),
        SKIN_SKELETON => ([0xC8, 0xC8, 0xBE, 255], [0xB4, 0xB4, 0xAA, 255], [0xA8, 0xA8, 0x9E, 255], 23),
        SKIN_CREEPER => ([0x4F, 0xB5, 0x45, 255], [0x45, 0xA0, 0x3C, 255], [0x3C, 0x8C, 0x34, 255], 37),
        _ => ([0xE6, 0x9A, 0xA0, 255], [0xDD, 0x8E, 0x95, 255], [0xC9, 0x7B, 0x82, 255], 53),
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
        for (x, y) in [(1, 2), (2, 2), (1, 3), (2, 3), (5, 2), (6, 2), (5, 3), (6, 3)] {
            sk.set(fx + x, fy + y, eye_glow);
        }
        for (x, y) in [
            (3, 4), (4, 4), (3, 5), (4, 5), (2, 5), (5, 5),
            (2, 6), (3, 6), (4, 6), (5, 6),
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
        for r in [box_face_rects(ARM_R_UV, ARM_SIZE)[3], box_face_rects(ARM_L_UV, ARM_SIZE)[3]] {
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
fn paint_quadruped(sk: &mut Skin, hide: Rgba, belly: Rgba, seed: u32) {
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
fn paint_skins_into(px: &mut [u8]) {
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

fn build_atlas() -> Atlas {
    let mut px = vec![0u8; ATLAS_W * ATLAS_H * 4];
    for t in 0..TILE_COUNT {
        let mut tex = Tex::new();
        paint_tile(t as TileId, &mut tex);
        let (ox, oy) = tile_origin(t as TileId);
        for y in 0..TILE {
            for x in 0..TILE {
                let c = tex.get(x, y);
                let i = ((oy + y) * ATLAS_W + ox + x) * 4;
                px[i..i + 4].copy_from_slice(&c);
            }
        }
    }

    paint_skins_into(&mut px);

    let mut levels = vec![(ATLAS_W as u32, ATLAS_H as u32, px)];
    for _ in 1..MIP_LEVELS {
        let (w, h, ref src) = levels[levels.len() - 1];
        levels.push(downsample(w as usize, h as usize, src));
    }
    Atlas { levels }
}

/// Halve an RGBA image with a 2x2 box filter, averaging in linear light and
/// weighting colour by alpha so cut-out tiles do not grow dark fringes.
fn downsample(w: usize, h: usize, src: &[u8]) -> (u32, u32, Vec<u8>) {
    let (nw, nh) = (w / 2, h / 2);
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        for x in 0..nw {
            let mut rgb = [0.0f32; 3];
            let mut a = 0.0f32;
            let mut wsum = 0.0f32;
            let mut plain = [0.0f32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let i = ((y * 2 + dy) * w + x * 2 + dx) * 4;
                    let av = src[i + 3] as f32 / 255.0;
                    for c in 0..3 {
                        let lin = srgb_to_linear(src[i + c]);
                        rgb[c] += lin * av;
                        plain[c] += lin;
                    }
                    a += av;
                    wsum += av;
                }
            }
            let i = (y * nw + x) * 4;
            for c in 0..3 {
                let v = if wsum > 0.0 {
                    rgb[c] / wsum
                } else {
                    plain[c] / 4.0
                };
                out[i + c] = linear_to_srgb(v);
            }
            out[i + 3] = (a / 4.0 * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    (nw as u32, nh as u32, out)
}

fn srgb_to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> u8 {
    let c = if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

// ---------------------------------------------------------------------------
// Painting primitives
// ---------------------------------------------------------------------------

type Rgba = [u8; 4];

const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
    [r, g, b, 255]
}

const CLEAR: Rgba = [0, 0, 0, 0];

/// One 16x16 tile under construction.
struct Tex {
    px: [Rgba; TILE * TILE],
}

impl Tex {
    fn new() -> Self {
        Self {
            px: [CLEAR; TILE * TILE],
        }
    }

    #[inline]
    fn set(&mut self, x: usize, y: usize, c: Rgba) {
        if x < TILE && y < TILE {
            self.px[y * TILE + x] = c;
        }
    }

    #[inline]
    fn get(&self, x: usize, y: usize) -> Rgba {
        self.px[y * TILE + x]
    }

    fn fill(&mut self, c: Rgba) {
        self.px = [c; TILE * TILE];
    }

    fn rect(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: Rgba) {
        for y in y0..(y0 + h).min(TILE) {
            for x in x0..(x0 + w).min(TILE) {
                self.set(x, y, c);
            }
        }
    }

    /// Multiply the RGB of one texel, keeping its alpha.
    fn darken(&mut self, x: usize, y: usize, f: f32) {
        let c = self.get(x, y);
        self.set(x, y, shade(c, f));
    }

    /// Paint from pixel-art rows. `'.'` leaves the texel untouched; every other
    /// character must appear in `pal`.
    fn art(&mut self, rows: &[&str; TILE], pal: &[(char, Rgba)]) {
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

fn shade(c: Rgba, f: f32) -> Rgba {
    [
        (c[0] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[1] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[2] as f32 * f).clamp(0.0, 255.0) as u8,
        c[3],
    ]
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
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
fn hash(x: i32, y: i32, seed: u32) -> u32 {
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
fn rand01(x: i32, y: i32, seed: u32) -> f32 {
    hash(x, y, seed) as f32 / u32::MAX as f32
}

#[inline]
fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Value noise that wraps over the tile, so neighbouring blocks join seamlessly.
/// `period` is how many lattice cells span the 16 texels and must divide 16.
fn vnoise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
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
fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    0.55 * vnoise(x, y, 2, seed)
        + 0.30 * vnoise(x, y, 4, seed ^ 0x9E37_79B9)
        + 0.15 * vnoise(x, y, 8, seed ^ 0x51ED_270B)
}

/// Fill with `base`, modulated by wrapping noise and a per-texel grain.
fn noisy_fill(t: &mut Tex, base: Rgba, seed: u32, blotch: f32, grain: f32) {
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
fn wrapd(a: f32, b: f32) -> f32 {
    let d = (a - b).abs();
    d.min(TILE as f32 - d)
}

/// Voronoi cell lookup: `(site index, nearest distance, second distance)`.
fn voronoi(x: f32, y: f32, sites: &[[f32; 2]]) -> (usize, f32, f32) {
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

fn sites(n: usize, seed: u32) -> Vec<[f32; 2]> {
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
fn cobbles(t: &mut Tex, base: Rgba, seed: u32, count: usize, mortar: f32, spread: f32) {
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
fn ore_blob(t: &mut Tex, cx: f32, cy: f32, r: f32, seed: u32, core: Rgba, edge: Rgba) {
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
fn gem(t: &mut Tex, cx: i32, cy: i32, r: i32, core: Rgba, edge: Rgba, spark: Rgba) {
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

const STONE_BASE: Rgba = rgb(128, 128, 134);
const DIRT_BASE: Rgba = rgb(122, 86, 55);
const SAND_BASE: Rgba = rgb(219, 205, 148);
const WOOD_BASE: Rgba = rgb(104, 76, 44);
const PLANK_BASE: Rgba = rgb(163, 124, 74);

// ---------------------------------------------------------------------------
// Tile recipes
// ---------------------------------------------------------------------------

fn paint_tile(id: TileId, t: &mut Tex) {
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

fn stone(t: &mut Tex, base: Rgba, seed: u32) {
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

fn dirt(t: &mut Tex, base: Rgba, seed: u32) {
    noisy_fill(t, base, seed, 0.16, 0.13);
    // Small pebbles and root flecks.
    for i in 0..9 {
        let x = (rand01(i, 5, seed) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 6, seed) * TILE as f32) as usize % TILE;
        let f = if i % 2 == 0 { 0.74 } else { 1.2 };
        t.darken(x, y, f);
    }
}

fn grass_top(t: &mut Tex, base: Rgba, seed: u32) {
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
fn grass_side(t: &mut Tex, green: Rgba, seed: u32) {
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

fn podzol_top(t: &mut Tex) {
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

fn podzol_side(t: &mut Tex) {
    dirt(t, DIRT_BASE, 0x5802);
    for x in 0..TILE {
        let d = 2 + (rand01(x as i32, 22, 0x5802) * 3.0) as usize;
        for y in 0..d {
            let f = 1.0 + (rand01(x as i32, y as i32, 0x5803) - 0.5) * 0.3;
            t.set(x, y, shade(rgb(96, 68, 34), f));
        }
    }
}

fn sand(t: &mut Tex) {
    noisy_fill(t, SAND_BASE, 0x5901, 0.05, 0.1);
    for i in 0..10 {
        let x = (rand01(i, 12, 0x5901) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 13, 0x5901) * TILE as f32) as usize % TILE;
        t.darken(x, y, if i % 2 == 0 { 0.9 } else { 1.07 });
    }
}

fn sandstone_top(t: &mut Tex) {
    noisy_fill(t, rgb(216, 202, 150), 0x5A01, 0.05, 0.05);
    for x in 0..TILE {
        t.darken(x, 0, 0.9);
        t.darken(x, TILE - 1, 0.94);
    }
}

fn sandstone_side(t: &mut Tex) {
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

fn sandstone_bottom(t: &mut Tex) {
    noisy_fill(t, rgb(196, 180, 128), 0x5A04, 0.08, 0.06);
    for i in 0..6 {
        let x = (rand01(i, 14, 0x5A04) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 15, 0x5A04) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.86);
    }
}

fn gravel(t: &mut Tex) {
    cobbles(t, rgb(126, 121, 118), 0x5B01, 15, 0.75, 0.62);
    // Extra per-texel grit so it reads as loose stone rather than paving.
    for y in 0..TILE {
        for x in 0..TILE {
            let f = 1.0 + (rand01(x as i32, y as i32, 0x5B02) - 0.5) * 0.28;
            t.darken(x, y, f);
        }
    }
}

fn clay(t: &mut Tex) {
    noisy_fill(t, rgb(160, 166, 178), 0x5C01, 0.07, 0.04);
    for i in 0..5 {
        let x = (rand01(i, 16, 0x5C01) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 17, 0x5C01) * TILE as f32) as usize % TILE;
        t.darken(x, y, 0.92);
        t.darken((x + 1) % TILE, y, 0.95);
    }
}

fn snow(t: &mut Tex) {
    noisy_fill(t, rgb(246, 249, 253), 0x5D01, 0.02, 0.035);
    for i in 0..6 {
        let x = (rand01(i, 18, 0x5D01) * TILE as f32) as usize % TILE;
        let y = (rand01(i, 19, 0x5D01) * TILE as f32) as usize % TILE;
        t.set(x, y, rgb(226, 234, 246));
    }
}

fn ice(t: &mut Tex) {
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

fn speckled_stone(t: &mut Tex, base: Rgba, light: Rgba, dark: Rgba, seed: u32) {
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

fn bedrock(t: &mut Tex) {
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

fn planks(t: &mut Tex, base: Rgba, seed: u32) {
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
fn bark(t: &mut Tex, base: Rgba, seed: u32) {
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

fn birch_bark(t: &mut Tex) {
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
fn log_end(t: &mut Tex, pale: Rgba, dark: Rgba, rim: Rgba, seed: u32) {
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
fn leaves(t: &mut Tex, light: Rgba, dark: Rgba, seed: u32, hole: f32) {
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
    let snapshot: Vec<Rgba> = (0..TILE * TILE).map(|i| t.px[i]).collect();
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
fn needles(t: &mut Tex, light: Rgba, dark: Rgba, seed: u32) {
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

fn tall_grass(t: &mut Tex) {
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

fn flower(t: &mut Tex, petal: Rgba, petal_hi: Rgba, centre: Rgba) {
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

fn dead_bush(t: &mut Tex) {
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

fn ore(t: &mut Tex, core: Rgba, edge: Rgba, seed: u32, gems: bool) {
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

fn water(t: &mut Tex) {
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

fn cactus_side(t: &mut Tex) {
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

fn cactus_top(t: &mut Tex) {
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

fn cactus_bottom(t: &mut Tex) {
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
fn torch(t: &mut Tex) {
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

fn table_top(t: &mut Tex) {
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

fn table_side(t: &mut Tex) {
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

fn table_front(t: &mut Tex) {
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

fn furnace_top(t: &mut Tex) {
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

fn furnace_side(t: &mut Tex) {
    cobbles(t, rgb(112, 112, 118), 0x7602, 9, 1.0, 0.34);
    for x in 0..TILE {
        t.darken(x, 0, 1.14);
        t.darken(x, 15, 0.8);
    }
}

fn furnace_front(t: &mut Tex) {
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

fn stick(t: &mut Tex) {
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

fn nugget(t: &mut Tex, base: Rgba, hi: Rgba, lo: Rgba) {
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

fn ingot(t: &mut Tex) {
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
type TierPal = (Rgba, Rgba, Rgba);

const TIER_WOOD: TierPal = (rgb(163, 124, 74), rgb(198, 158, 104), rgb(112, 82, 46));
const TIER_STONE: TierPal = (rgb(136, 136, 142), rgb(178, 178, 184), rgb(90, 90, 96));
const TIER_IRON: TierPal = (rgb(214, 214, 220), rgb(244, 244, 248), rgb(148, 148, 156));

const PICKAXE_ART: [&str; TILE] = [
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

const AXE_ART: [&str; TILE] = [
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

const SWORD_ART: [&str; TILE] = [
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

fn tool(t: &mut Tex, art: &[&str; TILE], pal: TierPal) {
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

fn arrow(t: &mut Tex) {
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

fn missing(t: &mut Tex) {
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
fn mob_skin(t: &mut Tex, base: Rgba, dark: Rgba, seed: u32) {
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

fn zombie_face(t: &mut Tex) {
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

fn zombie_body(t: &mut Tex) {
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

fn skeleton_face(t: &mut Tex) {
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

fn skeleton_body(t: &mut Tex) {
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

fn creeper_face(t: &mut Tex) {
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

fn creeper_body(t: &mut Tex) {
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

fn pig_face(t: &mut Tex) {
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

fn pig_body(t: &mut Tex) {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_fits_the_grid() {
        assert!(
            TILE_COUNT <= ATLAS_COLS * ATLAS_ROWS,
            "{TILE_COUNT} tiles do not fit a {ATLAS_COLS}x{ATLAS_ROWS} atlas"
        );
        assert_eq!(TILE_NAMES.len(), TILE_COUNT);
    }

    /// Every registered tile must actually be painted. A tile left blank shows
    /// up in the world as a flat colour, which is exactly the look this work
    /// exists to remove.
    #[test]
    fn every_tile_has_content() {
        let a = atlas();
        for t in 0..TILE_COUNT {
            let name = TILE_NAMES[t];
            let first = a.tile_texel(t as TileId, 0, 0);
            let mut distinct = false;
            let mut opaque = 0;
            for y in 0..TILE {
                for x in 0..TILE {
                    let c = a.tile_texel(t as TileId, x, y);
                    if c != first {
                        distinct = true;
                    }
                    if c[3] > 0 {
                        opaque += 1;
                    }
                }
            }
            assert!(opaque > 12, "tile {name} is almost entirely transparent");
            if t as TileId != T_WHITE {
                assert!(distinct, "tile {name} is a flat colour, not a texture");
            }
        }
    }

    /// The same bytes every run, on every machine. Anything seeded from time or
    /// address space would break this.
    #[test]
    fn generation_is_deterministic() {
        let mut a = vec![0u8; ATLAS_W * ATLAS_H * 4];
        for t in 0..TILE_COUNT {
            let mut tex = Tex::new();
            paint_tile(t as TileId, &mut tex);
            let (ox, oy) = tile_origin(t as TileId);
            for y in 0..TILE {
                for x in 0..TILE {
                    let i = ((oy + y) * ATLAS_W + ox + x) * 4;
                    a[i..i + 4].copy_from_slice(&tex.get(x, y));
                }
            }
        }
        // The atlas is tiles *and* the mob skin sheets, so a faithful rebuild
        // has to paint both. Comparing tiles alone would only prove that the
        // skins exist, not that anything is reproducible.
        paint_skins_into(&mut a);
        assert_eq!(a, atlas().levels[0].2, "atlas generation is not stable");
    }

    #[test]
    fn mip_chain_is_tile_aligned() {
        let a = atlas();
        assert_eq!(a.levels.len(), MIP_LEVELS as usize);
        for (i, (w, h, px)) in a.levels.iter().enumerate() {
            assert_eq!(*w as usize, ATLAS_W >> i);
            assert_eq!(*h as usize, ATLAS_H >> i);
            assert_eq!(px.len(), (*w as usize) * (*h as usize) * 4);
            // Every level must still be a whole number of texels per tile, or
            // the filter has started mixing neighbouring tiles together.
            assert!(
                (*w as usize) % ATLAS_COLS == 0 && (*h as usize) % ATLAS_ROWS == 0,
                "mip {i} is {w}x{h}, which no longer divides into tiles"
            );
        }
    }

    /// Every block the game can place must land on a real tile on all six faces.
    #[test]
    fn every_block_maps_to_a_tile() {
        for n in 1..=BlockId::MAX {
            let id = BlockId(n);
            for face in 0..6 {
                let t = block_tile(id, face);
                assert!(
                    (t as usize) < TILE_COUNT,
                    "block {n} face {face} maps outside the atlas"
                );
                assert_ne!(t, T_MISSING, "block {n} face {face} has no texture");
            }
        }
    }

    #[test]
    fn every_item_maps_to_a_tile() {
        for &item in ItemId::ALL {
            let t = item_tile(item);
            assert!((t as usize) < TILE_COUNT);
            assert_ne!(t, T_MISSING, "item {:?} has no icon", item.name());
        }
    }

    #[test]
    fn tile_uvs_stay_inside_their_tile() {
        for t in 0..TILE_COUNT {
            let [u0, v0, u1, v1] = tile_uv_rect(t as TileId);
            let (ox, oy) = tile_origin(t as TileId);
            let (lo_u, hi_u) = (
                ox as f32 / ATLAS_W as f32,
                (ox + TILE) as f32 / ATLAS_W as f32,
            );
            let (lo_v, hi_v) = (
                oy as f32 / ATLAS_H as f32,
                (oy + TILE) as f32 / ATLAS_H as f32,
            );
            assert!(
                u0 >= lo_u && u1 <= hi_u,
                "tile {t} u range escapes its cell"
            );
            assert!(
                v0 >= lo_v && v1 <= hi_v,
                "tile {t} v range escapes its cell"
            );
            assert!(u0 < u1 && v0 < v1);
        }
    }

    #[test]
    fn pixel_art_rows_are_the_right_shape() {
        for art in [&PICKAXE_ART, &AXE_ART, &SWORD_ART] {
            assert_eq!(art.len(), TILE);
            for (i, row) in art.iter().enumerate() {
                assert_eq!(row.chars().count(), TILE, "art row {i} is the wrong width");
            }
        }
    }

    /// Cut-out tiles need enough solid area to read as a plant, and enough holes
    /// to read as cut out at all.
    #[test]
    fn cutout_tiles_have_both_solid_and_clear_texels() {
        let a = atlas();
        for t in [
            T_OAK_LEAVES,
            T_BIRCH_LEAVES,
            T_SPRUCE_LEAVES,
            T_TALL_GRASS,
            T_FLOWER_RED,
            T_FLOWER_YELLOW,
            T_DEAD_BUSH,
            T_TORCH,
        ] {
            let mut clear = 0;
            let mut solid = 0;
            for y in 0..TILE {
                for x in 0..TILE {
                    if a.tile_texel(t, x, y)[3] < 128 {
                        clear += 1;
                    } else {
                        solid += 1;
                    }
                }
            }
            let name = TILE_NAMES[t as usize];
            assert!(clear > 4, "{name} has no cut-out holes");
            assert!(solid > 12, "{name} is barely there");
        }
    }

    /// Water has to be see-through or the "translucent water" pass is pointless.
    #[test]
    fn water_is_translucent() {
        let a = atlas();
        for y in 0..TILE {
            for x in 0..TILE {
                let alpha = a.tile_texel(T_WATER, x, y)[3];
                assert!(
                    (100..250).contains(&alpha),
                    "water alpha {alpha} is not translucent"
                );
            }
        }
    }

    /// Write the atlas out as a magnified PNG so the art can be inspected by
    /// eye. Off by default; set `LOUDSTONE_DUMP_ATLAS=<path>` to enable.
    #[test]
    fn dump_atlas_png() {
        let Ok(path) = std::env::var("LOUDSTONE_DUMP_ATLAS") else {
            return;
        };
        const S: usize = 6; // magnification
        let (w, h) = (ATLAS_W * S, ATLAS_H * S);
        let a = atlas();
        let mut out = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x / S, y / S);
                let mut c = a.texel(sx, sy);
                // Checkerboard behind transparent texels so cut-outs are visible.
                if c[3] < 255 {
                    let bg = if ((x / 8) + (y / 8)) % 2 == 0 {
                        90u8
                    } else {
                        130
                    };
                    let t = c[3] as f32 / 255.0;
                    for k in 0..3 {
                        c[k] = (c[k] as f32 * t + bg as f32 * (1.0 - t)) as u8;
                    }
                    c[3] = 255;
                }
                // Tile boundaries.
                if sx % TILE == 0 && x % S == 0 {
                    c = [255, 40, 40, 255];
                }
                if sy % TILE == 0 && y % S == 0 {
                    c = [255, 40, 40, 255];
                }
                let i = (y * w + x) * 4;
                out[i..i + 4].copy_from_slice(&c);
            }
        }
        crate::screenshot::write_rgba_png(std::path::Path::new(&path), w as u32, h as u32, &out)
            .expect("write atlas dump");
        println!("atlas written to {path}");
    }
}
