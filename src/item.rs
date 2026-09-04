//! Item identities: what the player carries, what blocks drop, and the tool-tier
//! table that gates mining.
//!
//! Numbering rule, and it matters because save files store raw ids: item ids
//! `1..=17` are *block items* and share the exact numeric id of the block they
//! place, so `ItemId(n).places() == BlockId(n)`. Non-block items start at 100.
//! Never renumber an existing id; only append.

use crate::block::BlockId;

// --- block ids this module needs that `block.rs` does not define yet ---------
//
// The contract says TORCH / CRAFTING_TABLE / FURNACE are appended to `BlockId` by
// the agent that owns `block.rs`, in this order. They are declared here so that
// crafting compiles before that lands. Once `block.rs` defines them, these
// constants stay numerically identical and can be swapped for `BlockId::TORCH`
// and friends at leisure -- nothing breaks either way.

/// Placed torch. Expected `BlockId::TORCH`.
pub const BLOCK_TORCH: BlockId = BlockId(15);
/// Placed crafting table. Expected `BlockId::CRAFTING_TABLE`.
pub const BLOCK_CRAFTING_TABLE: BlockId = BlockId(16);
/// Placed furnace. Expected `BlockId::FURNACE`.
pub const BLOCK_FURNACE: BlockId = BlockId(17);

// --- tools -------------------------------------------------------------------

/// What a tool is shaped like. Determines which blocks it speeds up.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Sword,
}

/// Material tier of a tool. Ordering is meaningful: `Wood < Stone < Iron`.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum ToolTier {
    Wood,
    Stone,
    Iron,
}

impl ToolTier {
    /// Numeric rank, for gating comparisons.
    pub fn rank(self) -> u8 {
        match self {
            ToolTier::Wood => 1,
            ToolTier::Stone => 2,
            ToolTier::Iron => 3,
        }
    }

    /// How much faster this tier mines a block it is effective against.
    pub fn speed(self) -> f32 {
        match self {
            ToolTier::Wood => 2.0,
            ToolTier::Stone => 4.0,
            ToolTier::Iron => 6.0,
        }
    }

    /// Uses before the tool breaks.
    pub fn durability(self) -> u16 {
        match self {
            ToolTier::Wood => 60,
            ToolTier::Stone => 132,
            ToolTier::Iron => 251,
        }
    }

    /// The item this tier is crafted from.
    pub fn material(self) -> ItemId {
        match self {
            ToolTier::Wood => ItemId::PLANKS,
            ToolTier::Stone => ItemId::COBBLESTONE,
            ToolTier::Iron => ItemId::IRON_INGOT,
        }
    }
}

// --- item ids ----------------------------------------------------------------

/// A carryable item. Block items share their `BlockId` number (see module docs).
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
#[repr(transparent)]
pub struct ItemId(pub u16);

impl ItemId {
    // Block items -- numerically identical to their BlockId.
    pub const STONE: ItemId = ItemId(1);
    pub const DIRT: ItemId = ItemId(2);
    pub const GRASS: ItemId = ItemId(3);
    pub const SAND: ItemId = ItemId(4);
    pub const WOOD: ItemId = ItemId(5);
    pub const LEAVES: ItemId = ItemId(6);
    pub const PLANKS: ItemId = ItemId(7);
    pub const COBBLESTONE: ItemId = ItemId(8);
    pub const COAL_ORE: ItemId = ItemId(9);
    pub const IRON_ORE: ItemId = ItemId(10);
    pub const GOLD_ORE: ItemId = ItemId(11);
    pub const DIAMOND_ORE: ItemId = ItemId(12);
    pub const TORCH: ItemId = ItemId(15);
    pub const CRAFTING_TABLE: ItemId = ItemId(16);
    pub const FURNACE: ItemId = ItemId(17);

    // Non-block items.
    pub const STICK: ItemId = ItemId(100);
    pub const COAL: ItemId = ItemId(101);
    pub const RAW_IRON: ItemId = ItemId(102);
    pub const IRON_INGOT: ItemId = ItemId(103);

    // Tools.
    pub const WOODEN_PICKAXE: ItemId = ItemId(110);
    pub const STONE_PICKAXE: ItemId = ItemId(111);
    pub const IRON_PICKAXE: ItemId = ItemId(112);
    pub const WOODEN_AXE: ItemId = ItemId(120);
    pub const STONE_AXE: ItemId = ItemId(121);
    pub const IRON_AXE: ItemId = ItemId(122);
    pub const WOODEN_SWORD: ItemId = ItemId(130);
    pub const STONE_SWORD: ItemId = ItemId(131);
    pub const IRON_SWORD: ItemId = ItemId(132);

    /// Every item that exists, for UI listings and exhaustive tests.
    pub const ALL: &'static [ItemId] = &[
        ItemId::STONE,
        ItemId::DIRT,
        ItemId::GRASS,
        ItemId::SAND,
        ItemId::WOOD,
        ItemId::LEAVES,
        ItemId::PLANKS,
        ItemId::COBBLESTONE,
        ItemId::COAL_ORE,
        ItemId::IRON_ORE,
        ItemId::GOLD_ORE,
        ItemId::DIAMOND_ORE,
        ItemId::TORCH,
        ItemId::CRAFTING_TABLE,
        ItemId::FURNACE,
        ItemId::STICK,
        ItemId::COAL,
        ItemId::RAW_IRON,
        ItemId::IRON_INGOT,
        ItemId::WOODEN_PICKAXE,
        ItemId::STONE_PICKAXE,
        ItemId::IRON_PICKAXE,
        ItemId::WOODEN_AXE,
        ItemId::STONE_AXE,
        ItemId::IRON_AXE,
        ItemId::WOODEN_SWORD,
        ItemId::STONE_SWORD,
        ItemId::IRON_SWORD,
    ];

    /// True if this id is one the game actually defines.
    pub fn is_valid(self) -> bool {
        ItemId::ALL.contains(&self)
    }

    /// The block this item places when right-clicked, if it places anything.
    pub fn places(self) -> Option<BlockId> {
        match self {
            ItemId::STONE
            | ItemId::DIRT
            | ItemId::GRASS
            | ItemId::SAND
            | ItemId::WOOD
            | ItemId::LEAVES
            | ItemId::PLANKS
            | ItemId::COBBLESTONE
            | ItemId::COAL_ORE
            | ItemId::IRON_ORE
            | ItemId::GOLD_ORE
            | ItemId::DIAMOND_ORE
            | ItemId::TORCH
            | ItemId::CRAFTING_TABLE
            | ItemId::FURNACE => Some(BlockId(self.0 as u8)),
            _ => None,
        }
    }

    /// The item form of a block, for blocks that have one.
    pub fn from_block(block: BlockId) -> Option<ItemId> {
        let id = ItemId(block.0 as u16);
        if id.places() == Some(block) { Some(id) } else { None }
    }

    /// Tool kind and tier, or `None` for anything that is not a tool.
    pub fn tool(self) -> Option<(ToolKind, ToolTier)> {
        let out = match self {
            ItemId::WOODEN_PICKAXE => (ToolKind::Pickaxe, ToolTier::Wood),
            ItemId::STONE_PICKAXE => (ToolKind::Pickaxe, ToolTier::Stone),
            ItemId::IRON_PICKAXE => (ToolKind::Pickaxe, ToolTier::Iron),
            ItemId::WOODEN_AXE => (ToolKind::Axe, ToolTier::Wood),
            ItemId::STONE_AXE => (ToolKind::Axe, ToolTier::Stone),
            ItemId::IRON_AXE => (ToolKind::Axe, ToolTier::Iron),
            ItemId::WOODEN_SWORD => (ToolKind::Sword, ToolTier::Wood),
            ItemId::STONE_SWORD => (ToolKind::Sword, ToolTier::Stone),
            ItemId::IRON_SWORD => (ToolKind::Sword, ToolTier::Iron),
            _ => return None,
        };
        Some(out)
    }

    /// The tool item for a kind and tier. Inverse of [`ItemId::tool`].
    pub fn tool_item(kind: ToolKind, tier: ToolTier) -> ItemId {
        match (kind, tier) {
            (ToolKind::Pickaxe, ToolTier::Wood) => ItemId::WOODEN_PICKAXE,
            (ToolKind::Pickaxe, ToolTier::Stone) => ItemId::STONE_PICKAXE,
            (ToolKind::Pickaxe, ToolTier::Iron) => ItemId::IRON_PICKAXE,
            (ToolKind::Axe, ToolTier::Wood) => ItemId::WOODEN_AXE,
            (ToolKind::Axe, ToolTier::Stone) => ItemId::STONE_AXE,
            (ToolKind::Axe, ToolTier::Iron) => ItemId::IRON_AXE,
            (ToolKind::Sword, ToolTier::Wood) => ItemId::WOODEN_SWORD,
            (ToolKind::Sword, ToolTier::Stone) => ItemId::STONE_SWORD,
            (ToolKind::Sword, ToolTier::Iron) => ItemId::IRON_SWORD,
        }
    }

    /// True for tools, which never stack and carry durability.
    pub fn is_tool(self) -> bool {
        self.tool().is_some()
    }

    /// How many of this item fit in one stack. Tools are always 1.
    pub fn max_stack(self) -> u8 {
        if self.is_tool() { 1 } else { MAX_STACK }
    }

    /// Full durability for a tool; 0 for everything else.
    pub fn max_durability(self) -> u16 {
        match self.tool() {
            Some((_, tier)) => tier.durability(),
            None => 0,
        }
    }

    /// Melee damage this item deals when swung. Swords are the point; everything
    /// else is a fist with extra steps.
    pub fn attack_damage(self) -> f32 {
        match self.tool() {
            Some((ToolKind::Sword, tier)) => 2.0 + tier.rank() as f32 * 2.0,
            Some((ToolKind::Axe, tier)) => 1.0 + tier.rank() as f32,
            Some((ToolKind::Pickaxe, _)) => 2.0,
            None => 1.0,
        }
    }

    /// Display name for the hotbar and inventory UI.
    pub fn name(self) -> &'static str {
        match self {
            ItemId::STONE => "Stone",
            ItemId::DIRT => "Dirt",
            ItemId::GRASS => "Grass Block",
            ItemId::SAND => "Sand",
            ItemId::WOOD => "Wood",
            ItemId::LEAVES => "Leaves",
            ItemId::PLANKS => "Planks",
            ItemId::COBBLESTONE => "Cobblestone",
            ItemId::COAL_ORE => "Coal Ore",
            ItemId::IRON_ORE => "Iron Ore",
            ItemId::GOLD_ORE => "Gold Ore",
            ItemId::DIAMOND_ORE => "Diamond Ore",
            ItemId::TORCH => "Torch",
            ItemId::CRAFTING_TABLE => "Crafting Table",
            ItemId::FURNACE => "Furnace",
            ItemId::STICK => "Stick",
            ItemId::COAL => "Coal",
            ItemId::RAW_IRON => "Raw Iron",
            ItemId::IRON_INGOT => "Iron Ingot",
            ItemId::WOODEN_PICKAXE => "Wooden Pickaxe",
            ItemId::STONE_PICKAXE => "Stone Pickaxe",
            ItemId::IRON_PICKAXE => "Iron Pickaxe",
            ItemId::WOODEN_AXE => "Wooden Axe",
            ItemId::STONE_AXE => "Stone Axe",
            ItemId::IRON_AXE => "Iron Axe",
            ItemId::WOODEN_SWORD => "Wooden Sword",
            ItemId::STONE_SWORD => "Stone Sword",
            ItemId::IRON_SWORD => "Iron Sword",
            _ => "Unknown",
        }
    }

    /// Flat colour for the hotbar icon. Block items borrow their block's colour.
    pub fn color(self) -> [f32; 3] {
        match self.places() {
            Some(b) => b.color(),
            None => match self {
                ItemId::STICK => [0.55, 0.40, 0.22],
                ItemId::COAL => [0.14, 0.14, 0.16],
                ItemId::RAW_IRON => [0.78, 0.64, 0.52],
                ItemId::IRON_INGOT => [0.86, 0.86, 0.88],
                _ => match self.tool() {
                    Some((_, ToolTier::Wood)) => [0.63, 0.48, 0.29],
                    Some((_, ToolTier::Stone)) => [0.50, 0.50, 0.53],
                    Some((_, ToolTier::Iron)) => [0.86, 0.86, 0.88],
                    None => [1.0, 0.0, 1.0],
                },
            },
        }
    }
}

/// Maximum items in one stack of anything stackable.
pub const MAX_STACK: u8 = 64;

// --- the tier gating table ---------------------------------------------------

/// How a block responds to tools. This is the single place tool gating is
/// decided; nothing else should hardcode a tier comparison.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct HarvestRule {
    /// The tool kind that mines this block faster. `None` means no tool helps.
    pub effective: Option<ToolKind>,
    /// Minimum tool needed to get a drop at all. `None` means bare hands work.
    pub required: Option<(ToolKind, ToolTier)>,
}

impl HarvestRule {
    const fn free(effective: Option<ToolKind>) -> Self {
        Self {
            effective,
            required: None,
        }
    }

    const fn gated(kind: ToolKind, tier: ToolTier) -> Self {
        Self {
            effective: Some(kind),
            required: Some((kind, tier)),
        }
    }
}

/// THE tier table. Read it top to bottom to see exactly what each block demands.
pub fn harvest_rule(block: BlockId) -> HarvestRule {
    use ToolKind::{Axe, Pickaxe, Sword};
    use ToolTier::{Iron, Stone, Wood};
    match block {
        // Stone family: any pickaxe drops it, better pickaxes go faster.
        BlockId::STONE | BlockId::COBBLESTONE => HarvestRule::gated(Pickaxe, Wood),
        BlockId::COAL_ORE => HarvestRule::gated(Pickaxe, Wood),
        // Iron needs stone tier or better.
        BlockId::IRON_ORE => HarvestRule::gated(Pickaxe, Stone),
        // Gold and diamond need iron tier.
        BlockId::GOLD_ORE | BlockId::DIAMOND_ORE => HarvestRule::gated(Pickaxe, Iron),
        // Crafted stone furniture behaves like stone.
        BLOCK_FURNACE => HarvestRule::gated(Pickaxe, Wood),
        // Wood family: an axe is faster but hands still work.
        BlockId::WOOD | BlockId::PLANKS | BLOCK_CRAFTING_TABLE => HarvestRule::free(Some(Axe)),
        // Leaves shear fastest with a sword.
        BlockId::LEAVES => HarvestRule::free(Some(Sword)),
        // Soft ground and torches: no tool matters.
        _ => HarvestRule::free(None),
    }
}

/// How much faster `tool` mines `block` than bare hands. Always >= 1.0.
///
/// Pass `None` for an empty hand. A tool of the wrong kind is worth no more than
/// a fist -- an iron pickaxe does not fell trees quickly.
pub fn mining_speed_multiplier(tool: Option<ItemId>, block: BlockId) -> f32 {
    let rule = harvest_rule(block);
    let Some((kind, tier)) = tool.and_then(ItemId::tool) else {
        return 1.0;
    };
    if rule.effective == Some(kind) {
        tier.speed()
    } else {
        1.0
    }
}

/// Whether mining `block` with `tool` yields a drop.
///
/// Breaking is always allowed (the block still disappears); this only decides
/// whether the player gets anything for it. Bedrock, air and water are never
/// harvestable regardless of tool.
pub fn can_harvest(tool: Option<ItemId>, block: BlockId) -> bool {
    if block.is_air() || block == BlockId::WATER || block == BlockId::BEDROCK {
        return false;
    }
    match harvest_rule(block).required {
        None => true,
        Some((need_kind, need_tier)) => match tool.and_then(ItemId::tool) {
            Some((kind, tier)) => kind == need_kind && tier >= need_tier,
            None => false,
        },
    }
}

/// What `block` drops when mined with a tool that [`can_harvest`] it.
///
/// Returns `None` for blocks that drop nothing. Callers must still check
/// `can_harvest` first -- this function does not know what tool was used.
pub fn block_drop(block: BlockId) -> Option<ItemId> {
    match block {
        // Stone shatters into cobblestone.
        BlockId::STONE => Some(ItemId::COBBLESTONE),
        // Grass strips to plain dirt.
        BlockId::GRASS => Some(ItemId::DIRT),
        // Ores drop their raw material rather than the block.
        BlockId::COAL_ORE => Some(ItemId::COAL),
        BlockId::IRON_ORE => Some(ItemId::RAW_IRON),
        // Leaves drop nothing; saplings are out of scope.
        BlockId::LEAVES => None,
        BlockId::AIR | BlockId::WATER | BlockId::BEDROCK => None,
        // Everything else drops itself.
        other => ItemId::from_block(other),
    }
}

/// Convenience: the drop from mining `block` with `tool`, `None` if it yields
/// nothing. This is the one call the mining code needs.
pub fn mining_drop(tool: Option<ItemId>, block: BlockId) -> Option<ItemId> {
    if can_harvest(tool, block) {
        block_drop(block)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_items_share_block_numbering() {
        for &item in ItemId::ALL {
            if let Some(block) = item.places() {
                assert_eq!(item.0, block.0 as u16, "{} broke the numbering rule", item.name());
                assert_eq!(ItemId::from_block(block), Some(item));
            }
        }
    }

    #[test]
    fn non_block_items_do_not_place() {
        for item in [
            ItemId::STICK,
            ItemId::COAL,
            ItemId::RAW_IRON,
            ItemId::IRON_INGOT,
            ItemId::IRON_PICKAXE,
        ] {
            assert_eq!(item.places(), None);
        }
        assert_eq!(ItemId::from_block(BlockId::AIR), None);
        assert_eq!(ItemId::from_block(BlockId::BEDROCK), None);
        assert_eq!(ItemId::from_block(BlockId::WATER), None);
    }

    #[test]
    fn tool_roundtrip() {
        for &item in ItemId::ALL {
            if let Some((kind, tier)) = item.tool() {
                assert_eq!(ItemId::tool_item(kind, tier), item);
                assert_eq!(item.max_stack(), 1);
                assert_eq!(item.max_durability(), tier.durability());
            } else {
                assert_eq!(item.max_stack(), 64);
                assert_eq!(item.max_durability(), 0);
            }
        }
    }

    #[test]
    fn stone_drops_cobblestone_only_with_a_pickaxe() {
        assert!(!can_harvest(None, BlockId::STONE));
        assert!(!can_harvest(Some(ItemId::WOODEN_AXE), BlockId::STONE));
        assert!(!can_harvest(Some(ItemId::IRON_SWORD), BlockId::STONE));
        assert!(can_harvest(Some(ItemId::WOODEN_PICKAXE), BlockId::STONE));
        assert_eq!(
            mining_drop(Some(ItemId::WOODEN_PICKAXE), BlockId::STONE),
            Some(ItemId::COBBLESTONE)
        );
        assert_eq!(mining_drop(None, BlockId::STONE), None);
    }

    #[test]
    fn iron_needs_stone_tier() {
        assert!(!can_harvest(Some(ItemId::WOODEN_PICKAXE), BlockId::IRON_ORE));
        assert!(can_harvest(Some(ItemId::STONE_PICKAXE), BlockId::IRON_ORE));
        assert!(can_harvest(Some(ItemId::IRON_PICKAXE), BlockId::IRON_ORE));
        assert_eq!(
            mining_drop(Some(ItemId::STONE_PICKAXE), BlockId::IRON_ORE),
            Some(ItemId::RAW_IRON)
        );
    }

    #[test]
    fn diamond_and_gold_need_iron_tier() {
        for ore in [BlockId::DIAMOND_ORE, BlockId::GOLD_ORE] {
            assert!(!can_harvest(Some(ItemId::WOODEN_PICKAXE), ore));
            assert!(!can_harvest(Some(ItemId::STONE_PICKAXE), ore));
            assert!(can_harvest(Some(ItemId::IRON_PICKAXE), ore));
            assert_eq!(mining_drop(Some(ItemId::IRON_PICKAXE), ore), ItemId::from_block(ore));
        }
    }

    #[test]
    fn coal_ore_drops_coal_with_any_pickaxe() {
        assert_eq!(
            mining_drop(Some(ItemId::WOODEN_PICKAXE), BlockId::COAL_ORE),
            Some(ItemId::COAL)
        );
        assert_eq!(mining_drop(None, BlockId::COAL_ORE), None);
    }

    #[test]
    fn soft_blocks_drop_bare_handed() {
        assert_eq!(mining_drop(None, BlockId::DIRT), Some(ItemId::DIRT));
        assert_eq!(mining_drop(None, BlockId::GRASS), Some(ItemId::DIRT));
        assert_eq!(mining_drop(None, BlockId::WOOD), Some(ItemId::WOOD));
        assert_eq!(mining_drop(None, BlockId::SAND), Some(ItemId::SAND));
        assert_eq!(mining_drop(None, BlockId::LEAVES), None);
    }

    #[test]
    fn bedrock_and_fluids_are_never_harvestable() {
        for block in [BlockId::BEDROCK, BlockId::AIR, BlockId::WATER] {
            assert!(!can_harvest(None, block));
            assert!(!can_harvest(Some(ItemId::IRON_PICKAXE), block));
            assert_eq!(mining_drop(Some(ItemId::IRON_PICKAXE), block), None);
        }
    }

    #[test]
    fn speed_scales_with_tier_only_for_the_right_tool() {
        assert_eq!(mining_speed_multiplier(None, BlockId::STONE), 1.0);
        assert_eq!(
            mining_speed_multiplier(Some(ItemId::WOODEN_PICKAXE), BlockId::STONE),
            2.0
        );
        assert_eq!(
            mining_speed_multiplier(Some(ItemId::STONE_PICKAXE), BlockId::STONE),
            4.0
        );
        assert_eq!(
            mining_speed_multiplier(Some(ItemId::IRON_PICKAXE), BlockId::STONE),
            6.0
        );
        // Wrong kind: no better than a fist.
        assert_eq!(
            mining_speed_multiplier(Some(ItemId::IRON_PICKAXE), BlockId::WOOD),
            1.0
        );
        assert_eq!(
            mining_speed_multiplier(Some(ItemId::IRON_AXE), BlockId::WOOD),
            6.0
        );
        assert_eq!(
            mining_speed_multiplier(Some(ItemId::WOODEN_SWORD), BlockId::LEAVES),
            2.0
        );
    }

    #[test]
    fn a_pickaxe_that_cannot_harvest_still_mines_at_speed() {
        // Wooden pickaxe on diamond: fast-ish, but yields nothing.
        assert_eq!(
            mining_speed_multiplier(Some(ItemId::WOODEN_PICKAXE), BlockId::DIAMOND_ORE),
            2.0
        );
        assert!(!can_harvest(Some(ItemId::WOODEN_PICKAXE), BlockId::DIAMOND_ORE));
    }
}
