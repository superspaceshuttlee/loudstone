//! Block identities and their static properties.

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
#[repr(transparent)]
pub struct BlockId(pub u8);

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

    #[inline]
    pub fn is_air(self) -> bool {
        self.0 == 0
    }

    /// Whether this block occludes the face of a neighbour (and so that face is culled).
    #[inline]
    pub fn is_opaque(self) -> bool {
        !matches!(
            self,
            BlockId::AIR | BlockId::WATER | BlockId::LEAVES | BlockId::TORCH
        )
    }

    /// Whether the player collides with it.
    #[inline]
    pub fn is_solid(self) -> bool {
        !matches!(self, BlockId::AIR | BlockId::WATER | BlockId::TORCH)
    }

    /// Base RGB. Visuals are deliberately flat -- shading comes from AO and sun angle.
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
            _ => [1.0, 0.0, 1.0], // missing-block magenta
        }
    }

    /// Relative mining hardness. Higher is slower.
    pub fn hardness(self) -> f32 {
        match self {
            BlockId::BEDROCK => f32::INFINITY,
            BlockId::STONE | BlockId::COBBLESTONE => 1.5,
            BlockId::COAL_ORE | BlockId::IRON_ORE => 3.0,
            BlockId::GOLD_ORE | BlockId::DIAMOND_ORE => 3.5,
            BlockId::DIRT | BlockId::GRASS | BlockId::SAND => 0.6,
            BlockId::WOOD | BlockId::PLANKS => 2.0,
            BlockId::LEAVES => 0.2,
            BlockId::TORCH => 0.1,
            BlockId::CRAFTING_TABLE => 2.5,
            BlockId::FURNACE => 3.5,
            _ => 1.0,
        }
    }
}
