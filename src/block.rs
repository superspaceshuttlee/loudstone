//! Block identities and their static properties.
//!
//! # Where the properties live
//!
//! Nothing in this file decides what a block *is*. Colour, hardness, the
//! booleans, the light it gives off, what it drops and what tool it demands all
//! come from `assets/data/blocks.ron` by way of [`registry`]. Adding a block is
//! one entry in that file plus a texture; it is not five `match` arms and a
//! recompile.
//!
//! What stays here is the API the rest of the crate calls -- `BlockId::STONE`,
//! `id.is_opaque()`, `id.hardness()` -- and the frozen numbers behind it.
//!
//! # Numbering
//!
//! Ids 0..=17 are **frozen**. `save.rs` writes raw block numbers into world
//! files and `item.rs` derives item ids from them, so renumbering an existing
//! block silently rewrites every save that mentions it. New blocks are appended
//! from 18 upward and never inserted in the middle.
//!
//! The associated constants below stay constants deliberately: half the crate
//! writes `match id { BlockId::STONE => ... }`, and a match pattern must be a
//! compile-time constant. They are a frozen index into the registry, and
//! `registry_ids_match_the_constants` fails the build if the data ever
//! disagrees with them.

// `main.rs` owns the crate's module list and is not this agent's file to edit,
// so the procedural texture module is attached here with an explicit path. It is
// pure data -- no wgpu, no file loading -- which is why it hangs off the block
// table rather than off the renderer. The integrator can promote it to a plain
// `mod texture;` in `main.rs` and delete these two lines; nothing else changes
// except the `crate::block::texture` paths.
#[path = "texture.rs"]
pub mod texture;

// The content registry is attached the same way and for the same reason: it is
// pure data with no renderer in it, and `main.rs` is not this agent's file to
// edit. The integrator can promote it to a plain `mod registry;` in `main.rs`
// and delete these two lines; only the `crate::block::registry` paths change.
#[path = "registry.rs"]
pub mod registry;

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct BlockId(pub u8);

/// How the mesher and the renderer must treat a block's geometry.
///
/// Spelled in `blocks.ron` as `render: Solid` and friends.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, serde::Deserialize)]
pub enum RenderKind {
    /// A full cube, fully opaque texels, drawn in the depth-writing pass.
    #[default]
    Solid,
    /// A full cube whose texture has holes: leaves. Alpha-cut, drawn with
    /// back faces kept so a canopy is not see-through.
    Cutout,
    /// Two intersecting vertical quads: tall grass, flowers, dead bush.
    Cross,
    /// A thin standing post, textured from a narrow column of its tile.
    Post,
    /// Translucent, blended, drawn last and without writing depth.
    Water,
}

impl BlockId {
    pub const AIR: BlockId = BlockId(0);
    pub const STONE: BlockId = BlockId(1);
    pub const DIRT: BlockId = BlockId(2);
    pub const GRASS: BlockId = BlockId(3);
    pub const SAND: BlockId = BlockId(4);
    pub const WOOD: BlockId = BlockId(5);
    pub const LEAVES: BlockId = BlockId(6);
    pub const PLANKS: BlockId = BlockId(7);
    pub const COBBLESTONE: BlockId = BlockId(8);
    pub const COAL_ORE: BlockId = BlockId(9);
    pub const IRON_ORE: BlockId = BlockId(10);
    pub const GOLD_ORE: BlockId = BlockId(11);
    pub const DIAMOND_ORE: BlockId = BlockId(12);
    pub const BEDROCK: BlockId = BlockId(13);
    pub const WATER: BlockId = BlockId(14);
    // 15..17 are fixed by the save format: item.rs and every existing save file
    // store raw block numbers, so these must never be renumbered.
    pub const TORCH: BlockId = BlockId(15);
    pub const CRAFTING_TABLE: BlockId = BlockId(16);
    pub const FURNACE: BlockId = BlockId(17);

    // -----------------------------------------------------------------------
    // Worldgen blocks, appended from 18. Ids below this line may grow, never
    // shift.
    // -----------------------------------------------------------------------

    /// Compacted desert stone, under the sand.
    pub const SANDSTONE: BlockId = BlockId(18);
    /// Loose stone: river beds, sea floors, underground pockets.
    pub const GRAVEL: BlockId = BlockId(19);
    /// Clay banks in shallow water.
    pub const CLAY: BlockId = BlockId(20);
    /// Snow cover on cold ground and mountain caps.
    pub const SNOW: BlockId = BlockId(21);
    /// Frozen water surface in cold biomes.
    pub const ICE: BlockId = BlockId(22);
    /// Pink-grey stone variant.
    pub const GRANITE: BlockId = BlockId(23);
    /// Near-white stone variant.
    pub const DIORITE: BlockId = BlockId(24);
    /// Grey-green stone variant.
    pub const ANDESITE: BlockId = BlockId(25);
    /// Pale birch trunk.
    pub const BIRCH_LOG: BlockId = BlockId(26);
    pub const BIRCH_LEAVES: BlockId = BlockId(27);
    /// Dark spruce trunk.
    pub const SPRUCE_LOG: BlockId = BlockId(28);
    pub const SPRUCE_LEAVES: BlockId = BlockId(29);
    /// Desert cactus column.
    pub const CACTUS: BlockId = BlockId(30);
    /// Ground cover. Non-solid and non-opaque: walk straight through it.
    pub const TALL_GRASS: BlockId = BlockId(31);
    pub const FLOWER_RED: BlockId = BlockId(32);
    pub const FLOWER_YELLOW: BlockId = BlockId(33);
    pub const DEAD_BUSH: BlockId = BlockId(34);
    /// Grass "tint" variants. The mesher has one flat colour per block id, so a
    /// biome tint is spelled as a separate id rather than a per-vertex colour.
    pub const GRASS_COLD: BlockId = BlockId(35);
    pub const GRASS_DRY: BlockId = BlockId(36);
    pub const GRASS_SWAMP: BlockId = BlockId(37);
    /// Taiga forest floor.
    pub const PODZOL: BlockId = BlockId(38);

    /// Highest id this build knows about. Anything above renders magenta.
    pub const MAX: u8 = 38;

    #[inline]
    pub fn is_air(self) -> bool {
        self.0 == 0
    }

    /// Whether this block occludes the face of a neighbour (and so that face is culled).
    ///
    /// `opaque` in `blocks.ron`. Leaves and the ground-cover plants are
    /// see-through, so they never hide a face behind them. Ice is marked opaque
    /// on purpose: with flat untextured colours a "transparent" ice cube would
    /// look identical to an opaque one while costing six extra faces.
    ///
    /// This is the single hottest question in the engine -- the mesher asks it
    /// once per candidate face -- so it is a flat array index, nothing more.
    #[inline]
    pub fn is_opaque(self) -> bool {
        registry::block(self).has(registry::F_OPAQUE)
    }

    /// Whether placing a block here simply replaces what is already there.
    /// Water and small plants give way; anything solid does not. Without this
    /// the player cannot build in, on, or beside water, which rules out most of
    /// a shoreline. `replaceable` in `blocks.ron`.
    #[inline]
    pub fn is_replaceable(self) -> bool {
        registry::block(self).has(registry::F_REPLACEABLE)
    }

    /// Whether the player collides with it. `solid` in `blocks.ron`.
    #[inline]
    pub fn is_solid(self) -> bool {
        registry::block(self).has(registry::F_SOLID)
    }

    /// True for the grass-family surface blocks plus podzol, i.e. ground a
    /// plant or a tree will root in. The `"grassy"` tag in `blocks.ron`.
    #[inline]
    pub fn is_grassy(self) -> bool {
        registry::block(self).has(registry::F_GRASSY)
    }

    /// True for any of the tree trunk variants. The `"log"` tag.
    #[inline]
    pub fn is_log(self) -> bool {
        registry::block(self).has(registry::F_LOG)
    }

    /// True for any of the leaf variants. The `"leaves"` tag.
    #[inline]
    pub fn is_leaves(self) -> bool {
        registry::block(self).has(registry::F_LEAVES)
    }

    /// How this block is built into geometry and which pass draws it.
    ///
    /// Ground cover is a cross of two quads rather than a cube -- a flower
    /// rendered as a full block is a slab of pink, which is what this replaces.
    /// Leaves stay cubes but are alpha-cut, and water is the only translucent
    /// thing in the world. `render` in `blocks.ron`.
    #[inline]
    pub fn render_kind(self) -> RenderKind {
        registry::block(self).render
    }

    /// Block light this emits by itself, 0..=15. `light` in `blocks.ron`.
    ///
    /// `light.rs` still keeps its own copy of this table; a test below fails if
    /// the two ever disagree, and the copy should be deleted in favour of this.
    #[inline]
    #[allow(dead_code)]
    pub fn light_emission(self) -> u8 {
        registry::block(self).light
    }

    /// How much light this eats as it passes through, 0..=15.
    /// `light_opacity` in `blocks.ron`, defaulted from `opaque` and the
    /// `"leaves"` tag. The counterpart of `light.rs`'s `opacity()`.
    #[inline]
    #[allow(dead_code)]
    pub fn light_opacity(self) -> u8 {
        registry::block(self).light_opacity
    }

    /// The stable data-file name, e.g. `"crafting_table"`. `"?"` for an id no
    /// data file defines.
    #[allow(dead_code)]
    pub fn name(self) -> &'static str {
        registry::get()
            .block(self)
            .map_or("?", |d| d.name.as_str())
    }

    /// Look a block up by the name it carries in `blocks.ron`. This is how
    /// content code should refer to blocks it did not get handed.
    #[allow(dead_code)]
    pub fn from_name(name: &str) -> Option<BlockId> {
        registry::get().block_id(name)
    }

    /// True for the ground cover that draws as two crossed quads.
    #[inline]
    pub fn is_cross(self) -> bool {
        self.render_kind() == RenderKind::Cross
    }

    /// Base RGB. With textures in place this is the *tint* multiplied over the
    /// atlas sample, not the whole story: almost every block now carries its
    /// colour in its texture and tints white. The values are kept because
    /// `item.rs`, `hud.rs` and the map/debug readouts all still ask for a
    /// representative colour per block. `color` in `blocks.ron`; an id nothing
    /// defines is magenta.
    #[inline]
    pub fn color(self) -> [f32; 3] {
        registry::block(self).color
    }

    /// Relative mining hardness. Higher is slower, `f32::INFINITY` is
    /// unbreakable. `hardness` in `blocks.ron`.
    #[inline]
    pub fn hardness(self) -> f32 {
        registry::block(self).hardness
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frozen prefix. If any of these numbers move, every existing save and
    /// every stored item id is silently reinterpreted.
    #[test]
    fn frozen_ids_keep_their_numbers() {
        let frozen = [
            (BlockId::AIR, 0),
            (BlockId::STONE, 1),
            (BlockId::DIRT, 2),
            (BlockId::GRASS, 3),
            (BlockId::SAND, 4),
            (BlockId::WOOD, 5),
            (BlockId::LEAVES, 6),
            (BlockId::PLANKS, 7),
            (BlockId::COBBLESTONE, 8),
            (BlockId::COAL_ORE, 9),
            (BlockId::IRON_ORE, 10),
            (BlockId::GOLD_ORE, 11),
            (BlockId::DIAMOND_ORE, 12),
            (BlockId::BEDROCK, 13),
            (BlockId::WATER, 14),
            (BlockId::TORCH, 15),
            (BlockId::CRAFTING_TABLE, 16),
            (BlockId::FURNACE, 17),
        ];
        for (id, n) in frozen {
            assert_eq!(id.0, n, "block id {id:?} moved off its frozen number");
        }
    }

    /// The constants above and `blocks.ron` are two spellings of one numbering.
    /// If they drift, `BlockId::STONE` starts meaning something else while
    /// every save file on disk keeps meaning what it always did.
    #[test]
    fn registry_ids_match_the_constants() {
        let pairs: [(BlockId, &str); 39] = [
            (BlockId::AIR, "air"),
            (BlockId::STONE, "stone"),
            (BlockId::DIRT, "dirt"),
            (BlockId::GRASS, "grass"),
            (BlockId::SAND, "sand"),
            (BlockId::WOOD, "wood"),
            (BlockId::LEAVES, "leaves"),
            (BlockId::PLANKS, "planks"),
            (BlockId::COBBLESTONE, "cobblestone"),
            (BlockId::COAL_ORE, "coal_ore"),
            (BlockId::IRON_ORE, "iron_ore"),
            (BlockId::GOLD_ORE, "gold_ore"),
            (BlockId::DIAMOND_ORE, "diamond_ore"),
            (BlockId::BEDROCK, "bedrock"),
            (BlockId::WATER, "water"),
            (BlockId::TORCH, "torch"),
            (BlockId::CRAFTING_TABLE, "crafting_table"),
            (BlockId::FURNACE, "furnace"),
            (BlockId::SANDSTONE, "sandstone"),
            (BlockId::GRAVEL, "gravel"),
            (BlockId::CLAY, "clay"),
            (BlockId::SNOW, "snow"),
            (BlockId::ICE, "ice"),
            (BlockId::GRANITE, "granite"),
            (BlockId::DIORITE, "diorite"),
            (BlockId::ANDESITE, "andesite"),
            (BlockId::BIRCH_LOG, "birch_log"),
            (BlockId::BIRCH_LEAVES, "birch_leaves"),
            (BlockId::SPRUCE_LOG, "spruce_log"),
            (BlockId::SPRUCE_LEAVES, "spruce_leaves"),
            (BlockId::CACTUS, "cactus"),
            (BlockId::TALL_GRASS, "tall_grass"),
            (BlockId::FLOWER_RED, "flower_red"),
            (BlockId::FLOWER_YELLOW, "flower_yellow"),
            (BlockId::DEAD_BUSH, "dead_bush"),
            (BlockId::GRASS_COLD, "grass_cold"),
            (BlockId::GRASS_DRY, "grass_dry"),
            (BlockId::GRASS_SWAMP, "grass_swamp"),
            (BlockId::PODZOL, "podzol"),
        ];
        for (id, name) in pairs {
            assert_eq!(id.name(), name, "BlockId({}) is no longer {name}", id.0);
            assert_eq!(
                BlockId::from_name(name),
                Some(id),
                "\"{name}\" no longer resolves to {}",
                id.0
            );
        }
        // Nothing may be added above MAX without texture.rs learning about it.
        assert_eq!(registry::get().max_block_id(), BlockId::MAX);
    }

    /// `light.rs` still owns a private copy of the emission and opacity
    /// tables. Until it reads the registry instead, this catches the drift.
    #[test]
    fn the_light_tables_agree_with_the_registry() {
        for n in 0..=BlockId::MAX {
            let id = BlockId(n);
            assert_eq!(
                crate::light::emission(id),
                id.light_emission(),
                "block {n} emits a different amount of light in light.rs than in blocks.ron"
            );
            assert_eq!(
                crate::light::opacity(id),
                id.light_opacity(),
                "block {n} eats a different amount of light in light.rs than in blocks.ron"
            );
        }
    }

    /// Every id up to MAX must have a real colour, or it renders as magenta in
    /// the middle of the world.
    #[test]
    fn every_known_block_has_a_colour() {
        // Air is skipped: it is never drawn, so it has no colour to be wrong.
        for n in 1..=BlockId::MAX {
            let id = BlockId(n);
            assert_ne!(
                id.color(),
                [1.0, 0.0, 1.0],
                "block {n} has no colour of its own"
            );
            assert!(id.hardness() > 0.0, "block {n} has a zero hardness");
        }
    }

    /// Ground cover must not block the player or hide the ground under it.
    #[test]
    fn plants_are_neither_solid_nor_opaque() {
        for id in [
            BlockId::TALL_GRASS,
            BlockId::FLOWER_RED,
            BlockId::FLOWER_YELLOW,
            BlockId::DEAD_BUSH,
        ] {
            assert!(!id.is_solid(), "{id:?} would block the player");
            assert!(!id.is_opaque(), "{id:?} would cull the face under it");
        }
    }

    #[test]
    fn leaves_do_not_cull_but_do_collide() {
        for id in [
            BlockId::LEAVES,
            BlockId::BIRCH_LEAVES,
            BlockId::SPRUCE_LEAVES,
        ] {
            assert!(id.is_leaves());
            assert!(!id.is_opaque());
            assert!(id.is_solid(), "leaves are still something to stand on");
        }
    }

    #[test]
    fn solid_worldgen_blocks_are_opaque_and_solid() {
        for id in [
            BlockId::SANDSTONE,
            BlockId::GRAVEL,
            BlockId::CLAY,
            BlockId::SNOW,
            BlockId::ICE,
            BlockId::GRANITE,
            BlockId::DIORITE,
            BlockId::ANDESITE,
            BlockId::CACTUS,
            BlockId::PODZOL,
            BlockId::GRASS_COLD,
            BlockId::GRASS_DRY,
            BlockId::GRASS_SWAMP,
        ] {
            assert!(id.is_opaque(), "{id:?} should occlude");
            assert!(id.is_solid(), "{id:?} should be walked on, not through");
        }
    }

    #[test]
    fn grass_family_and_logs_are_recognised() {
        assert!(BlockId::GRASS.is_grassy());
        assert!(BlockId::GRASS_SWAMP.is_grassy());
        assert!(BlockId::PODZOL.is_grassy());
        assert!(!BlockId::SAND.is_grassy());
        assert!(BlockId::WOOD.is_log());
        assert!(BlockId::SPRUCE_LOG.is_log());
        assert!(!BlockId::PLANKS.is_log());
    }
}
