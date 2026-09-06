//! Item identities: what the player carries, what blocks drop, and the tool-tier
//! table that gates mining.
//!
//! Like `block.rs`, this file is the API and not the content. Names, stack
//! sizes, tool tiers, durability, damage, what each block drops and what tool
//! it demands all come from `assets/data/items.ron` and `assets/data/blocks.ron`
//! by way of [`crate::block::registry`]. The ids stay here, frozen, because
//! save files and `match` patterns both need them at compile time.
//!
//! Numbering rule, and it matters because save files store raw ids: item ids
//! `1..=17` are *block items* and share the exact numeric id of the block they
//! place, so `ItemId(n).places() == BlockId(n)`. The registry refuses to load
//! data that breaks that. Non-block items start at 100. Never renumber an
//! existing id; only append.

use crate::block::BlockId;
use crate::block::registry;

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
///
/// Spelled in the data files as `tool: Pickaxe` and `Some((Axe, "iron"))`.
/// This is an enum rather than data because `texture.rs` matches on it to pick
/// an icon: the *set* of tool shapes is code, everything about them is data.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Hash, serde::Deserialize)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Sword,
}

impl ToolKind {
    /// Every tool shape, for exhaustive lookups.
    pub const ALL: [ToolKind; 3] = [ToolKind::Pickaxe, ToolKind::Axe, ToolKind::Sword];
}

/// Material tier of a tool. Ordering is meaningful: `Wood < Stone < Iron`, and
/// `can_harvest` gates on it, so the registry checks that the `rank` numbers in
/// `items.ron` increase in this same order.
///
/// The names `"wood"`, `"stone"` and `"iron"` are what `items.ron` and
/// `blocks.ron` refer to. Like [`ToolKind`], the set is code (because
/// `texture.rs` matches on it) and the numbers are data.
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum ToolTier {
    Wood,
    Stone,
    Iron,
}

impl ToolTier {
    /// Every tier, weakest first.
    pub const ALL: [ToolTier; 3] = [ToolTier::Wood, ToolTier::Stone, ToolTier::Iron];

    /// Numeric rank, for gating comparisons.
    pub fn rank(self) -> u8 {
        registry::get().tier(self).rank
    }

    /// How much faster this tier mines a block it is effective against.
    pub fn speed(self) -> f32 {
        registry::get().tier(self).speed
    }

    /// Uses before the tool breaks.
    pub fn durability(self) -> u16 {
        registry::get().tier(self).durability
    }

    /// The item this tier is crafted from.
    pub fn material(self) -> ItemId {
        registry::get().tier(self).material
    }

    /// The name this tier goes by in the data files.
    #[allow(dead_code)]
    pub fn data_name(self) -> &'static str {
        registry::get().tier(self).name.as_str()
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

    /// Every item this *build* knows by name, for UI listings, the texture
    /// atlas and exhaustive tests. It is a constant because `texture.rs`
    /// iterates it to generate icons, and art is code.
    ///
    /// Items added purely in `items.ron` are absent from here but present
    /// everywhere else -- `is_valid`, `name`, crafting -- and draw with the
    /// missing-texture tile. Use [`crate::block::registry::Registry::item_ids`]
    /// for everything the registry knows.
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

    /// True if this id is one the loaded data actually defines. `save.rs` uses
    /// it to reject a corrupt inventory.
    pub fn is_valid(self) -> bool {
        registry::get().item(self).is_some()
    }

    /// The block this item places when right-clicked, if it places anything.
    /// `places` in `items.ron`, which the registry checks carries the same
    /// number as the block it names.
    pub fn places(self) -> Option<BlockId> {
        registry::get().item(self).and_then(|d| d.places)
    }

    /// The item form of a block, for blocks that have one.
    pub fn from_block(block: BlockId) -> Option<ItemId> {
        let id = ItemId(block.0 as u16);
        if id.places() == Some(block) {
            Some(id)
        } else {
            None
        }
    }

    /// Tool kind and tier, or `None` for anything that is not a tool.
    /// `tool: Some((Pickaxe, "iron"))` in `items.ron`.
    pub fn tool(self) -> Option<(ToolKind, ToolTier)> {
        registry::get().item(self).and_then(|d| d.tool)
    }

    /// The tool item for a kind and tier. Inverse of [`ItemId::tool`]. The
    /// registry refuses to load data that leaves a combination unfilled or
    /// claimed twice, so this is always a real item.
    pub fn tool_item(kind: ToolKind, tier: ToolTier) -> ItemId {
        registry::get().tool_item(kind, tier)
    }

    /// True for tools, which never stack and carry durability.
    pub fn is_tool(self) -> bool {
        self.tool().is_some()
    }

    /// How many of this item fit in one stack. Tools are 1 unless `items.ron`
    /// says otherwise.
    pub fn max_stack(self) -> u8 {
        registry::get()
            .item(self)
            .map_or(MAX_STACK, |d| d.max_stack)
    }

    /// Full durability for a tool; 0 for everything else.
    pub fn max_durability(self) -> u16 {
        registry::get().item(self).map_or(0, |d| d.durability)
    }

    /// Melee damage this item deals when swung. Swords are the point; everything
    /// else is a fist with extra steps. `attack` in `items.ron`.
    pub fn attack_damage(self) -> f32 {
        registry::get().item(self).map_or(1.0, |d| d.attack)
    }

    /// Display name for the hotbar and inventory UI.
    pub fn name(self) -> &'static str {
        registry::get()
            .item(self)
            .map_or("Unknown", |d| d.display.as_str())
    }

    /// The stable data-file name, e.g. `"iron_ingot"`. Recipes refer to items
    /// by this, never by number.
    #[allow(dead_code)]
    pub fn data_name(self) -> &'static str {
        registry::get().item(self).map_or("?", |d| d.name.as_str())
    }

    /// Look an item up by the name it carries in `items.ron`.
    #[allow(dead_code)]
    pub fn from_name(name: &str) -> Option<ItemId> {
        registry::get().item_id(name)
    }

    /// Flat colour for the hotbar icon. Block items borrow their block's colour
    /// and tools their tier's, unless `items.ron` gives them one of their own.
    pub fn color(self) -> [f32; 3] {
        registry::get()
            .item(self)
            .map_or([1.0, 0.0, 1.0], |d| d.color)
    }

    /// How many seconds of furnace burn one of this item is worth, or `None` if
    /// it is not a fuel. `fuel` in `items.ron`.
    pub fn fuel_seconds(self) -> Option<f32> {
        registry::get().item(self).and_then(|d| d.fuel)
    }
}

/// Maximum items in one stack of anything stackable.
pub const MAX_STACK: u8 = 64;

// --- the tier gating table ---------------------------------------------------

/// How a block responds to tools. This is the single place tool gating is
/// decided; nothing else should hardcode a tier comparison.
///
/// Filled in from the `tool:` and `requires:` fields of `blocks.ron`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct HarvestRule {
    /// The tool kind that mines this block faster. `None` means no tool helps.
    pub effective: Option<ToolKind>,
    /// Minimum tool needed to get a drop at all. `None` means bare hands work.
    pub required: Option<(ToolKind, ToolTier)>,
    /// False when nothing can ever harvest it -- air, water, bedrock.
    /// `requires: Never` in the data.
    pub harvestable: bool,
}

impl HarvestRule {
    /// What an id no data file defines behaves like: mineable, tool-agnostic.
    const UNKNOWN: HarvestRule = HarvestRule {
        effective: None,
        required: None,
        harvestable: true,
    };
}

/// THE tier table, now read from `blocks.ron`. Open that file to see exactly
/// what each block demands.
pub fn harvest_rule(block: BlockId) -> HarvestRule {
    registry::get()
        .block(block)
        .map_or(HarvestRule::UNKNOWN, |d| d.harvest)
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
/// whether the player gets anything for it. Blocks marked `requires: Never` --
/// air, water, bedrock -- are never harvestable regardless of tool.
pub fn can_harvest(tool: Option<ItemId>, block: BlockId) -> bool {
    let rule = harvest_rule(block);
    if !rule.harvestable {
        return false;
    }
    match rule.required {
        None => true,
        Some((need_kind, need_tier)) => match tool.and_then(ItemId::tool) {
            Some((kind, tier)) => kind == need_kind && tier >= need_tier,
            None => false,
        },
    }
}

/// What `block` drops when mined with a tool that [`can_harvest`] it.
///
/// The `drops:` field of `blocks.ron`: `Itself` (the default) gives the item
/// that places the block, `Nothing` gives nothing, and `Item("coal")` names
/// something else. Callers must still check `can_harvest` first -- this
/// function does not know what tool was used.
pub fn block_drop(block: BlockId) -> Option<ItemId> {
    registry::get().block(block).and_then(|d| d.drop)
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
                assert_eq!(
                    item.0,
                    block.0 as u16,
                    "{} broke the numbering rule",
                    item.name()
                );
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
        assert!(!can_harvest(
            Some(ItemId::WOODEN_PICKAXE),
            BlockId::IRON_ORE
        ));
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
            assert_eq!(
                mining_drop(Some(ItemId::IRON_PICKAXE), ore),
                ItemId::from_block(ore)
            );
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
        assert!(!can_harvest(
            Some(ItemId::WOODEN_PICKAXE),
            BlockId::DIAMOND_ORE
        ));
    }
}
