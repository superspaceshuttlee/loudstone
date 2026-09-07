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

use crate::content::block::BlockId;
use crate::content::item::{ItemId, ToolKind, ToolTier};

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
pub const T_BUCKET: TileId = 82;
pub const T_WATER_BUCKET: TileId = 83;

/// First of the block-icon tiles. There is one per block id, at
/// `ICON_BASE + block.0`, and each holds an isometric cube built from that
/// block's own faces.
///
/// A block item used to show one flat face in the inventory, so a slot of stone
/// and a slot of cobblestone were two grey squares and every wood looked alike.
/// Nothing about a flat square says "this is a block you can place". The cube
/// shows the top and two sides at once, which is both what Minecraft does and
/// the reason its inventory is readable at a glance.
pub const ICON_BASE: TileId = 84;
/// One icon slot per possible block id, so the lookup is `ICON_BASE + id` with
/// no table to keep in step.
pub const ICON_SLOTS: usize = 40;

/// Number of tiles the atlas actually paints.
pub const TILE_COUNT: usize = ICON_BASE as usize + ICON_SLOTS;

/// Human-readable name per tile, for test failures and the atlas dump.
/// Names for the tiles that are painted individually. Icon tiles are composited
/// afterwards from these and are named by their block id instead.
pub const TILE_NAMES: [&str; ICON_BASE as usize] = [
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
    "bucket",
    "water_bucket",
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
        // A cube built from this block's own faces, not one flat face of it.
        if (block.0 as usize) < ICON_SLOTS {
            return ICON_BASE + block.0 as TileId;
        }
        return block_tile(block, 2);
    }
    match item {
        ItemId::STICK => T_STICK,
        ItemId::COAL => T_COAL,
        ItemId::RAW_IRON => T_RAW_IRON,
        ItemId::IRON_INGOT => T_IRON_INGOT,
        ItemId::BUCKET => T_BUCKET,
        ItemId::WATER_BUCKET => T_WATER_BUCKET,
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
// Submodules
// ---------------------------------------------------------------------------
//
// Split by what each part is *for*, not by size. `paint` is the medium,
// `recipes` are the drawings, `skins` is the one place that lays out a sheet
// instead of a tile. This file keeps the atlas geometry, the tile ids and the
// block-to-tile mapping, because those are the contract every other module in
// the game reads.

mod paint;
mod recipes;
mod skins;

pub use paint::*;
pub use recipes::*;
pub use skins::*;

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
        assert_eq!(
            TILE_NAMES.len(),
            ICON_BASE as usize,
            "TILE_NAMES covers the individually painted tiles; icons are named              by block id and composited afterwards"
        );
    }

    /// Every registered tile must actually be painted. A tile left blank shows
    /// up in the world as a flat colour, which is exactly the look this work
    /// exists to remove.
    #[test]
    fn every_tile_has_content() {
        let a = atlas();
        for t in 0..ICON_BASE as usize {
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
        // Rebuild through the real entry point rather than re-implementing it
        // here. The previous version painted tiles by hand and then compared
        // against the cached atlas, so it silently stopped covering every stage
        // the moment the builder grew one -- which is exactly what happened when
        // block icons were added.
        let a = build_atlas();
        let b = build_atlas();
        assert_eq!(a.levels.len(), b.levels.len());
        for (i, ((w, h, x), (_, _, y))) in a.levels.iter().zip(b.levels.iter()).enumerate() {
            assert_eq!(x, y, "atlas mip {i} ({w}x{h}) is not reproducible");
        }
        assert_eq!(
            a.levels[0].2,
            atlas().levels[0].2,
            "the cached atlas differs from a fresh build"
        );
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
        crate::render::screenshot::write_rgba_png(
            std::path::Path::new(&path),
            w as u32,
            h as u32,
            &out,
        )
        .expect("write atlas dump");
        println!("atlas written to {path}");
    }
}
