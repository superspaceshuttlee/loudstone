//! Block identities and their static properties.
//!
//! # Numbering
//!
//! Ids 0..=17 are **frozen**. `save.rs` writes raw block numbers into world
//! files and `item.rs` derives item ids from them, so renumbering an existing
//! block silently rewrites every save that mentions it. New blocks are appended
//! from 18 upward and never inserted in the middle.

// `main.rs` owns the crate's module list and is not this agent's file to edit,
// so the procedural texture module is attached here with an explicit path. It is
// pure data -- no wgpu, no file loading -- which is why it hangs off the block
// table rather than off the renderer. The integrator can promote it to a plain
// `mod texture;` in `main.rs` and delete these two lines; nothing else changes
// except the `crate::block::texture` paths.
#[path = "texture.rs"]
pub mod texture;

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct BlockId(pub u8);

/// How the mesher and the renderer must treat a block's geometry.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum RenderKind {
    /// A full cube, fully opaque texels, drawn in the depth-writing pass.
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
    /// Leaves and the ground-cover plants are see-through, so they never hide a
    /// face behind them. Ice is treated as opaque: with flat untextured colours
    /// a "transparent" ice cube would look identical to an opaque one while
    /// costing six extra faces per block.
    #[inline]
    pub fn is_opaque(self) -> bool {
        !matches!(
            self,
            BlockId::AIR
                | BlockId::WATER
                | BlockId::LEAVES
                | BlockId::BIRCH_LEAVES
                | BlockId::SPRUCE_LEAVES
                | BlockId::TORCH
                | BlockId::TALL_GRASS
                | BlockId::FLOWER_RED
                | BlockId::FLOWER_YELLOW
                | BlockId::DEAD_BUSH
        )
    }

    /// Whether placing a block here simply replaces what is already there.
    /// Water and small plants give way, as they do in Minecraft; anything solid
    /// does not. Without this the player cannot build in, on, or beside water,
    /// which rules out most of a shoreline.
    #[inline]
    pub fn is_replaceable(self) -> bool {
        matches!(
            self,
            BlockId::AIR
                | BlockId::WATER
                | BlockId::TALL_GRASS
                | BlockId::FLOWER_RED
                | BlockId::FLOWER_YELLOW
                | BlockId::DEAD_BUSH
        )
    }

    /// Whether the player collides with it.
    #[inline]
    pub fn is_solid(self) -> bool {
        !matches!(
            self,
            BlockId::AIR
                | BlockId::WATER
                | BlockId::TORCH
                | BlockId::TALL_GRASS
                | BlockId::FLOWER_RED
                | BlockId::FLOWER_YELLOW
                | BlockId::DEAD_BUSH
        )
    }

    /// True for the three grass-family surface blocks plus podzol, i.e. ground
    /// a plant or a tree will root in.
    #[inline]
    pub fn is_grassy(self) -> bool {
        matches!(
            self,
            BlockId::GRASS
                | BlockId::GRASS_COLD
                | BlockId::GRASS_DRY
                | BlockId::GRASS_SWAMP
                | BlockId::PODZOL
        )
    }

    /// True for any of the tree trunk variants.
    #[inline]
    pub fn is_log(self) -> bool {
        matches!(
            self,
            BlockId::WOOD | BlockId::BIRCH_LOG | BlockId::SPRUCE_LOG
        )
    }

    /// True for any of the leaf variants.
    #[inline]
    pub fn is_leaves(self) -> bool {
        matches!(
            self,
            BlockId::LEAVES | BlockId::BIRCH_LEAVES | BlockId::SPRUCE_LEAVES
        )
    }

    /// How this block is built into geometry and which pass draws it.
    ///
    /// Ground cover is a cross of two quads rather than a cube -- a flower
    /// rendered as a full block is a slab of pink, which is what this replaces.
    /// Leaves stay cubes but are alpha-cut, and water is the only translucent
    /// thing in the world.
    #[inline]
    pub fn render_kind(self) -> RenderKind {
        match self {
            BlockId::WATER => RenderKind::Water,
            BlockId::LEAVES | BlockId::BIRCH_LEAVES | BlockId::SPRUCE_LEAVES => RenderKind::Cutout,
            BlockId::TALL_GRASS
            | BlockId::FLOWER_RED
            | BlockId::FLOWER_YELLOW
            | BlockId::DEAD_BUSH => RenderKind::Cross,
            BlockId::TORCH => RenderKind::Post,
            _ => RenderKind::Solid,
        }
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
    /// representative colour per block.
    pub fn color(self) -> [f32; 3] {
        match self {
            BlockId::STONE => [0.50, 0.50, 0.53],
            BlockId::DIRT => [0.42, 0.30, 0.19],
            BlockId::GRASS => [0.32, 0.58, 0.24],
            BlockId::SAND => [0.83, 0.78, 0.55],
            BlockId::WOOD => [0.38, 0.27, 0.16],
            BlockId::LEAVES => [0.20, 0.45, 0.18],
            BlockId::PLANKS => [0.63, 0.48, 0.29],
            BlockId::COBBLESTONE => [0.42, 0.42, 0.44],
            BlockId::COAL_ORE => [0.24, 0.24, 0.26],
            BlockId::IRON_ORE => [0.71, 0.57, 0.45],
            BlockId::GOLD_ORE => [0.85, 0.72, 0.24],
            BlockId::DIAMOND_ORE => [0.36, 0.80, 0.82],
            BlockId::BEDROCK => [0.15, 0.15, 0.17],
            BlockId::WATER => [0.20, 0.40, 0.75],
            BlockId::TORCH => [0.95, 0.78, 0.35],
            BlockId::CRAFTING_TABLE => [0.55, 0.40, 0.24],
            BlockId::FURNACE => [0.38, 0.38, 0.40],

            BlockId::SANDSTONE => [0.76, 0.70, 0.49],
            BlockId::GRAVEL => [0.48, 0.46, 0.45],
            BlockId::CLAY => [0.61, 0.63, 0.67],
            BlockId::SNOW => [0.93, 0.95, 0.98],
            BlockId::ICE => [0.63, 0.79, 0.93],
            BlockId::GRANITE => [0.60, 0.44, 0.38],
            BlockId::DIORITE => [0.73, 0.73, 0.71],
            BlockId::ANDESITE => [0.55, 0.57, 0.55],
            BlockId::BIRCH_LOG => [0.82, 0.80, 0.72],
            BlockId::BIRCH_LEAVES => [0.44, 0.61, 0.26],
            BlockId::SPRUCE_LOG => [0.27, 0.19, 0.12],
            BlockId::SPRUCE_LEAVES => [0.15, 0.32, 0.20],
            BlockId::CACTUS => [0.27, 0.51, 0.23],
            // Plants: near their host ground so a full-cube meadow reads as
            // texture rather than as confetti.
            BlockId::TALL_GRASS => [0.38, 0.63, 0.25],
            BlockId::FLOWER_RED => [0.68, 0.31, 0.28],
            BlockId::FLOWER_YELLOW => [0.80, 0.75, 0.32],
            BlockId::DEAD_BUSH => [0.55, 0.44, 0.25],
            BlockId::GRASS_COLD => [0.36, 0.52, 0.36],
            BlockId::GRASS_DRY => [0.60, 0.62, 0.31],
            BlockId::GRASS_SWAMP => [0.29, 0.42, 0.22],
            BlockId::PODZOL => [0.35, 0.24, 0.12],

            _ => [1.0, 0.0, 1.0], // missing-block magenta
        }
    }

    /// Relative mining hardness. Higher is slower.
    pub fn hardness(self) -> f32 {
        match self {
            BlockId::BEDROCK => f32::INFINITY,
            BlockId::STONE | BlockId::COBBLESTONE => 1.5,
            BlockId::GRANITE | BlockId::DIORITE | BlockId::ANDESITE => 1.5,
            BlockId::COAL_ORE | BlockId::IRON_ORE => 3.0,
            BlockId::GOLD_ORE | BlockId::DIAMOND_ORE => 3.5,
            BlockId::DIRT | BlockId::GRASS | BlockId::SAND => 0.6,
            BlockId::GRASS_COLD | BlockId::GRASS_DRY | BlockId::GRASS_SWAMP => 0.6,
            BlockId::PODZOL | BlockId::GRAVEL | BlockId::CLAY => 0.6,
            BlockId::SANDSTONE => 0.9,
            BlockId::SNOW => 0.15,
            BlockId::ICE => 0.5,
            BlockId::WOOD | BlockId::PLANKS => 2.0,
            BlockId::BIRCH_LOG | BlockId::SPRUCE_LOG => 2.0,
            BlockId::LEAVES | BlockId::BIRCH_LEAVES | BlockId::SPRUCE_LEAVES => 0.2,
            BlockId::CACTUS => 0.4,
            BlockId::TORCH => 0.1,
            BlockId::TALL_GRASS
            | BlockId::FLOWER_RED
            | BlockId::FLOWER_YELLOW
            | BlockId::DEAD_BUSH => 0.1,
            BlockId::CRAFTING_TABLE => 2.5,
            BlockId::FURNACE => 3.5,
            _ => 1.0,
        }
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
