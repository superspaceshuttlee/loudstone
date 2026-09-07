//! The player's carried items: a 9-slot hotbar plus a 27-slot main grid.
//!
//! Slot indices are flat, `0..36`. `0..9` is the hotbar (index 0 is the leftmost
//! key, `1`), `9..36` is the main grid. Every slot-taking method uses this flat
//! index so the UI never has to translate.

use crate::content::item::{ItemId, MAX_STACK};

/// Hotbar slots, mapped to number keys 1-9.
pub const HOTBAR_SIZE: usize = 9;
/// Main inventory slots, shown when the inventory screen is open.
pub const MAIN_SIZE: usize = 27;
/// Total addressable slots.
pub const SLOT_COUNT: usize = HOTBAR_SIZE + MAIN_SIZE;

/// One occupied slot. A slot with zero items is represented as `None`, never as
/// an `ItemStack` with `count == 0`.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct ItemStack {
    pub item: ItemId,
    pub count: u8,
    /// Remaining uses. Meaningful only for tools; always 0 otherwise.
    pub durability: u16,
}

impl ItemStack {
    /// A stack of `count` items, clamped to what actually fits. Tools come out
    /// at full durability.
    pub fn new(item: ItemId, count: u8) -> Self {
        Self {
            item,
            count: count.clamp(1, item.max_stack()),
            durability: item.max_durability(),
        }
    }

    /// A single item. The usual way to make a tool.
    pub fn one(item: ItemId) -> Self {
        Self::new(item, 1)
    }

    /// A tool with partial wear, for load and for tests.
    pub fn worn(item: ItemId, durability: u16) -> Self {
        Self {
            item,
            count: 1,
            durability: durability.min(item.max_durability()),
        }
    }

    pub fn is_tool(self) -> bool {
        self.item.is_tool()
    }

    pub fn max_stack(self) -> u8 {
        self.item.max_stack()
    }

    pub fn is_full(self) -> bool {
        self.count >= self.max_stack()
    }

    /// Room left in this stack.
    pub fn space(self) -> u8 {
        self.max_stack().saturating_sub(self.count)
    }

    /// Whether `other` can be poured into this stack. Tools never stack, even
    /// two identical ones, because each carries its own durability.
    pub fn stacks_with(self, other: ItemStack) -> bool {
        self.item == other.item && !self.is_tool() && !other.is_tool()
    }

    /// Pour `other` in. Returns whatever did not fit.
    pub fn merge(&mut self, mut other: ItemStack) -> Option<ItemStack> {
        if !self.stacks_with(other) {
            return Some(other);
        }
        let moved = self.space().min(other.count);
        self.count += moved;
        other.count -= moved;
        if other.count == 0 { None } else { Some(other) }
    }

    /// Take `amount` items off this stack. Returns `None` if `amount` is 0 or
    /// would empty the stack (use the slot-level take for that).
    pub fn split(&mut self, amount: u8) -> Option<ItemStack> {
        if amount == 0 || amount >= self.count {
            return None;
        }
        self.count -= amount;
        Some(ItemStack {
            item: self.item,
            count: amount,
            durability: self.durability,
        })
    }

    /// Take the larger half, the usual right-click-a-stack gesture.
    pub fn split_half(&mut self) -> Option<ItemStack> {
        self.split(self.count / 2)
    }

    /// Spend `amount` durability. Returns true if the tool broke, in which case
    /// the caller must discard the stack.
    pub fn damage(&mut self, amount: u16) -> bool {
        if !self.is_tool() {
            return false;
        }
        self.durability = self.durability.saturating_sub(amount);
        self.durability == 0
    }

    /// Remaining durability as 0.0..=1.0, for a wear bar. Non-tools read 1.0.
    pub fn wear(self) -> f32 {
        let max = self.item.max_durability();
        if max == 0 {
            1.0
        } else {
            self.durability as f32 / max as f32
        }
    }
}

/// The player's items. Also the crafting grid's backing store is *not* here --
/// see `crafting.rs`; this struct is only what the player carries.
#[derive(Clone, Debug)]
pub struct Inventory {
    slots: [Option<ItemStack>; SLOT_COUNT],
    selected: usize,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            slots: [None; SLOT_COUNT],
            selected: 0,
        }
    }

    // --- raw slot access ----------------------------------------------------

    pub fn slots(&self) -> &[Option<ItemStack>; SLOT_COUNT] {
        &self.slots
    }

    pub fn slot(&self, index: usize) -> Option<ItemStack> {
        self.slots.get(index).copied().flatten()
    }

    pub fn slot_mut(&mut self, index: usize) -> Option<&mut ItemStack> {
        self.slots.get_mut(index)?.as_mut()
    }

    /// Overwrite a slot wholesale. Used by load and by inventory-screen drags.
    pub fn set_slot(&mut self, index: usize, stack: Option<ItemStack>) {
        if let Some(s) = self.slots.get_mut(index) {
            *s = stack.filter(|st| st.count > 0);
        }
    }

    /// Empty a slot and hand back what was in it.
    pub fn take_slot(&mut self, index: usize) -> Option<ItemStack> {
        self.slots.get_mut(index)?.take()
    }

    /// The hotbar, left to right.
    pub fn hotbar(&self) -> &[Option<ItemStack>] {
        &self.slots[..HOTBAR_SIZE]
    }

    /// The main grid, row-major.
    pub fn main(&self) -> &[Option<ItemStack>] {
        &self.slots[HOTBAR_SIZE..]
    }

    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.is_none())
    }

    pub fn first_empty(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }

    // --- selection ----------------------------------------------------------

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Select a hotbar slot. Out-of-range indices are ignored.
    pub fn set_selected(&mut self, index: usize) {
        if index < HOTBAR_SIZE {
            self.selected = index;
        }
    }

    /// Scroll wheel: move the selection by `delta`, wrapping around the hotbar.
    pub fn cycle_selection(&mut self, delta: i32) {
        let n = HOTBAR_SIZE as i32;
        self.selected = (self.selected as i32 + delta).rem_euclid(n) as usize;
    }

    pub fn selected_stack(&self) -> Option<ItemStack> {
        self.slot(self.selected)
    }

    /// The held item, which is what mining and placing consult.
    pub fn selected_item(&self) -> Option<ItemId> {
        self.selected_stack().map(|s| s.item)
    }

    // --- bulk queries -------------------------------------------------------

    /// Total count of `item` across every slot.
    pub fn count(&self, item: ItemId) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    pub fn has(&self, item: ItemId, count: u32) -> bool {
        self.count(item) >= count
    }

    // --- adding and removing ------------------------------------------------

    /// Insert a stack. Fills partial stacks of the same item first, then empty
    /// slots. Returns whatever did not fit.
    pub fn add(&mut self, stack: ItemStack) -> Option<ItemStack> {
        let mut rest = stack;
        if rest.count == 0 {
            return None;
        }
        if !rest.is_tool() {
            for slot in self.slots.iter_mut() {
                let Some(existing) = slot else { continue };
                if !existing.stacks_with(rest) || existing.is_full() {
                    continue;
                }
                match existing.merge(rest) {
                    None => return None,
                    Some(left) => rest = left,
                }
            }
        }
        for slot in self.slots.iter_mut() {
            if slot.is_some() {
                continue;
            }
            let take = rest.count.min(rest.max_stack());
            *slot = Some(ItemStack {
                item: rest.item,
                count: take,
                durability: rest.durability,
            });
            rest.count -= take;
            if rest.count == 0 {
                return None;
            }
        }
        Some(rest)
    }

    /// Insert `count` fresh items, spilling across as many stacks as needed.
    /// Returns how many did not fit.
    pub fn add_item(&mut self, item: ItemId, count: u32) -> u32 {
        let per_stack = item.max_stack() as u32;
        let mut left = count;
        while left > 0 {
            let chunk = left.min(per_stack) as u8;
            match self.add(ItemStack::new(item, chunk)) {
                None => left -= chunk as u32,
                Some(leftover) => {
                    // Inventory is full; leftover plus everything not yet placed.
                    return left - (chunk as u32 - leftover.count as u32);
                }
            }
        }
        0
    }

    /// Remove up to `count` of `item`. Returns how many were actually removed.
    pub fn remove(&mut self, item: ItemId, count: u32) -> u32 {
        let mut left = count;
        for slot in self.slots.iter_mut() {
            if left == 0 {
                break;
            }
            let Some(stack) = slot else { continue };
            if stack.item != item {
                continue;
            }
            let take = (stack.count as u32).min(left) as u8;
            stack.count -= take;
            left -= take as u32;
            if stack.count == 0 {
                *slot = None;
            }
        }
        count - left
    }

    /// Take up to `count` items out of one specific slot.
    pub fn take_from_slot(&mut self, index: usize, count: u8) -> Option<ItemStack> {
        let stack = self.slots.get_mut(index)?.as_mut()?;
        let take = count.min(stack.count);
        if take == 0 {
            return None;
        }
        let out = ItemStack {
            item: stack.item,
            count: take,
            durability: stack.durability,
        };
        stack.count -= take;
        if stack.count == 0 {
            self.slots[index] = None;
        }
        Some(out)
    }

    // --- slot-to-slot manipulation -----------------------------------------

    /// Drag `from` onto `to`: merge if they stack, otherwise swap.
    pub fn move_stack(&mut self, from: usize, to: usize) {
        if from == to || from >= SLOT_COUNT || to >= SLOT_COUNT {
            return;
        }
        let Some(src) = self.take_slot(from) else {
            return;
        };
        match self.slots[to] {
            None => self.slots[to] = Some(src),
            Some(mut dst) => {
                if dst.stacks_with(src) {
                    let leftover = dst.merge(src);
                    self.slots[to] = Some(dst);
                    self.slots[from] = leftover;
                } else {
                    // Not compatible: swap.
                    self.slots[to] = Some(src);
                    self.slots[from] = Some(dst);
                }
            }
        }
    }

    /// Merge `from` into `to` without swapping. Returns true if anything moved.
    /// Incompatible or full destinations leave both slots untouched.
    pub fn merge_stacks(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= SLOT_COUNT || to >= SLOT_COUNT {
            return false;
        }
        let (Some(src), Some(mut dst)) = (self.slot(from), self.slot(to)) else {
            return false;
        };
        if !dst.stacks_with(src) || dst.is_full() {
            return false;
        }
        let leftover = dst.merge(src);
        self.slots[to] = Some(dst);
        self.slots[from] = leftover;
        true
    }

    /// Split half of `from` into `to`. `to` must be empty or hold the same item.
    /// Returns true if anything moved.
    pub fn split_stack(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= SLOT_COUNT || to >= SLOT_COUNT {
            return false;
        }
        let Some(mut src) = self.slot(from) else {
            return false;
        };
        if src.is_tool() {
            return false;
        }
        // A single item cannot be halved, but it can still be moved to an
        // empty slot; callers get `false` so they can fall back to move_stack.
        let Some(half) = src.split_half() else {
            return false;
        };
        match self.slot(to) {
            None => {
                self.slots[from] = Some(src);
                self.slots[to] = Some(half);
                true
            }
            Some(mut dst) if dst.stacks_with(half) && !dst.is_full() => {
                let leftover = dst.merge(half);
                // Anything that did not fit goes back where it came from.
                if let Some(back) = leftover {
                    src.count += back.count;
                }
                self.slots[from] = Some(src);
                self.slots[to] = Some(dst);
                true
            }
            _ => false,
        }
    }

    // --- durability ---------------------------------------------------------

    /// Spend durability on the tool in `index`. Returns true if it broke, in
    /// which case the slot has already been emptied.
    pub fn damage_slot(&mut self, index: usize, amount: u16) -> bool {
        let Some(stack) = self.slot_mut(index) else {
            return false;
        };
        if stack.damage(amount) {
            self.slots[index] = None;
            true
        } else {
            false
        }
    }

    /// Spend durability on the held tool. Returns true if it broke.
    pub fn damage_selected(&mut self, amount: u16) -> bool {
        self.damage_slot(self.selected, amount)
    }

    // --- death --------------------------------------------------------------

    /// Empty the inventory and hand back everything that was in it, in slot
    /// order. The player drops the lot on death.
    pub fn drop_all(&mut self) -> Vec<ItemStack> {
        let mut out = Vec::new();
        for slot in self.slots.iter_mut() {
            if let Some(stack) = slot.take() {
                out.push(stack);
            }
        }
        out
    }
}

/// Largest stack of anything stackable. Re-exported so UI code need not reach
/// into `item`.
pub const STACK_LIMIT: u8 = MAX_STACK;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_merges_up_to_the_limit() {
        let mut a = ItemStack::new(ItemId::COBBLESTONE, 60);
        let leftover = a.merge(ItemStack::new(ItemId::COBBLESTONE, 10));
        assert_eq!(a.count, 64);
        assert_eq!(leftover.map(|s| s.count), Some(6));
    }

    #[test]
    fn stack_merge_rejects_different_items() {
        let mut a = ItemStack::new(ItemId::COBBLESTONE, 1);
        let leftover = a.merge(ItemStack::new(ItemId::DIRT, 1));
        assert_eq!(a.count, 1);
        assert_eq!(leftover.map(|s| s.item), Some(ItemId::DIRT));
    }

    #[test]
    fn tools_never_stack() {
        let a = ItemStack::one(ItemId::IRON_PICKAXE);
        let b = ItemStack::one(ItemId::IRON_PICKAXE);
        assert!(!a.stacks_with(b));
        assert_eq!(a.count, 1);
        assert_eq!(a.max_stack(), 1);
        assert_eq!(a.durability, 251);
        // Even asking for 64 gives one.
        assert_eq!(ItemStack::new(ItemId::IRON_PICKAXE, 64).count, 1);
    }

    #[test]
    fn split_takes_the_smaller_half_and_leaves_the_rest() {
        let mut a = ItemStack::new(ItemId::STICK, 7);
        let half = a.split_half().unwrap();
        assert_eq!(half.count, 3);
        assert_eq!(a.count, 4);
        // A single item cannot be split.
        let mut single = ItemStack::new(ItemId::STICK, 1);
        assert!(single.split_half().is_none());
        assert_eq!(single.count, 1);
    }

    #[test]
    fn split_rejects_overreach() {
        let mut a = ItemStack::new(ItemId::STICK, 5);
        assert!(a.split(5).is_none());
        assert!(a.split(9).is_none());
        assert!(a.split(0).is_none());
        assert_eq!(a.count, 5);
    }

    #[test]
    fn add_overflows_into_a_second_slot_at_64() {
        let mut inv = Inventory::new();
        assert_eq!(inv.add_item(ItemId::COBBLESTONE, 100), 0);
        assert_eq!(inv.count(ItemId::COBBLESTONE), 100);
        assert_eq!(inv.slot(0).unwrap().count, 64);
        assert_eq!(inv.slot(1).unwrap().count, 36);
    }

    #[test]
    fn add_tops_up_partial_stacks_before_taking_a_new_slot() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::DIRT, 60)));
        assert_eq!(inv.add_item(ItemId::DIRT, 10), 0);
        assert_eq!(inv.slot(0).unwrap().count, 64);
        assert_eq!(inv.slot(1).unwrap().count, 6);
    }

    #[test]
    fn add_reports_what_does_not_fit_when_full() {
        let mut inv = Inventory::new();
        // Fill every slot with a different-item-proof full stack.
        for i in 0..SLOT_COUNT {
            inv.set_slot(i, Some(ItemStack::new(ItemId::SAND, 64)));
        }
        assert_eq!(inv.count(ItemId::SAND), 64 * SLOT_COUNT as u32);
        let leftover = inv.add(ItemStack::new(ItemId::DIRT, 10));
        assert_eq!(leftover.map(|s| s.count), Some(10));
        // Adding sand still fails: everything is at the limit.
        assert_eq!(inv.add_item(ItemId::SAND, 5), 5);
    }

    #[test]
    fn add_item_reports_partial_fit() {
        let mut inv = Inventory::new();
        for i in 0..SLOT_COUNT {
            inv.set_slot(i, Some(ItemStack::new(ItemId::SAND, 64)));
        }
        // Free exactly 10 spaces in one slot.
        inv.set_slot(0, Some(ItemStack::new(ItemId::SAND, 54)));
        assert_eq!(inv.add_item(ItemId::SAND, 30), 20);
        assert_eq!(inv.slot(0).unwrap().count, 64);
    }

    #[test]
    fn remove_drains_across_slots_and_reports_the_shortfall() {
        let mut inv = Inventory::new();
        inv.add_item(ItemId::COBBLESTONE, 100);
        assert_eq!(inv.remove(ItemId::COBBLESTONE, 70), 70);
        assert_eq!(inv.count(ItemId::COBBLESTONE), 30);
        assert_eq!(inv.remove(ItemId::COBBLESTONE, 50), 30);
        assert_eq!(inv.count(ItemId::COBBLESTONE), 0);
        assert!(inv.is_empty());
    }

    #[test]
    fn move_stack_merges_or_swaps() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::PLANKS, 30)));
        inv.set_slot(1, Some(ItemStack::new(ItemId::PLANKS, 20)));
        inv.move_stack(0, 1);
        assert_eq!(inv.slot(1).unwrap().count, 50);
        assert!(inv.slot(0).is_none());

        inv.set_slot(0, Some(ItemStack::new(ItemId::DIRT, 3)));
        inv.move_stack(0, 1);
        assert_eq!(inv.slot(0).unwrap().item, ItemId::PLANKS);
        assert_eq!(inv.slot(1).unwrap().item, ItemId::DIRT);
    }

    #[test]
    fn move_stack_leaves_overflow_behind() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::PLANKS, 40)));
        inv.set_slot(1, Some(ItemStack::new(ItemId::PLANKS, 40)));
        inv.move_stack(0, 1);
        assert_eq!(inv.slot(1).unwrap().count, 64);
        assert_eq!(inv.slot(0).unwrap().count, 16);
    }

    #[test]
    fn split_stack_between_slots() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::COAL, 9)));
        assert!(inv.split_stack(0, 5));
        assert_eq!(inv.slot(0).unwrap().count, 5);
        assert_eq!(inv.slot(5).unwrap().count, 4);
        // Splitting into a matching stack merges.
        assert!(inv.split_stack(0, 5));
        assert_eq!(inv.slot(0).unwrap().count, 3);
        assert_eq!(inv.slot(5).unwrap().count, 6);
    }

    #[test]
    fn split_stack_into_an_incompatible_slot_does_nothing() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::COAL, 8)));
        inv.set_slot(1, Some(ItemStack::new(ItemId::DIRT, 1)));
        assert!(!inv.split_stack(0, 1));
        assert_eq!(inv.slot(0).unwrap().count, 8);
        assert_eq!(inv.slot(1).unwrap().count, 1);
    }

    #[test]
    fn split_returns_overflow_to_the_source() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::COAL, 40)));
        inv.set_slot(1, Some(ItemStack::new(ItemId::COAL, 54)));
        // Half of 40 is 20; only 10 fits, so 10 bounces back.
        assert!(inv.split_stack(0, 1));
        assert_eq!(inv.slot(1).unwrap().count, 64);
        assert_eq!(inv.slot(0).unwrap().count, 30);
        assert_eq!(inv.count(ItemId::COAL), 94);
    }

    #[test]
    fn merge_stacks_never_swaps() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::DIRT, 5)));
        inv.set_slot(1, Some(ItemStack::new(ItemId::SAND, 5)));
        assert!(!inv.merge_stacks(0, 1));
        assert_eq!(inv.slot(0).unwrap().item, ItemId::DIRT);
        assert_eq!(inv.slot(1).unwrap().item, ItemId::SAND);
    }

    #[test]
    fn selection_wraps_the_hotbar() {
        let mut inv = Inventory::new();
        assert_eq!(inv.selected(), 0);
        inv.cycle_selection(-1);
        assert_eq!(inv.selected(), 8);
        inv.cycle_selection(3);
        assert_eq!(inv.selected(), 2);
        inv.set_selected(7);
        assert_eq!(inv.selected(), 7);
        // Main-inventory indices are not selectable.
        inv.set_selected(20);
        assert_eq!(inv.selected(), 7);
    }

    #[test]
    fn durability_wears_down_and_the_tool_vanishes() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::one(ItemId::WOODEN_PICKAXE)));
        assert_eq!(inv.slot(0).unwrap().durability, 60);
        for _ in 0..59 {
            assert!(!inv.damage_selected(1));
        }
        assert_eq!(inv.slot(0).unwrap().durability, 1);
        assert!((inv.slot(0).unwrap().wear() - 1.0 / 60.0).abs() < 1e-6);
        assert!(inv.damage_selected(1));
        assert!(inv.slot(0).is_none());
    }

    #[test]
    fn damaging_a_non_tool_does_nothing() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::DIRT, 10)));
        assert!(!inv.damage_selected(100));
        assert_eq!(inv.slot(0).unwrap().count, 10);
    }

    #[test]
    fn drop_all_empties_and_returns_everything() {
        let mut inv = Inventory::new();
        inv.add_item(ItemId::COBBLESTONE, 100);
        inv.add(ItemStack::one(ItemId::STONE_PICKAXE));
        let dropped = inv.drop_all();
        assert_eq!(dropped.len(), 3);
        assert_eq!(dropped.iter().map(|s| s.count as u32).sum::<u32>(), 101);
        assert!(inv.is_empty());
        assert_eq!(inv.count(ItemId::COBBLESTONE), 0);
    }

    #[test]
    fn take_from_slot_clears_when_emptied() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId::TORCH, 3)));
        assert_eq!(inv.take_from_slot(0, 2).unwrap().count, 2);
        assert_eq!(inv.slot(0).unwrap().count, 1);
        assert_eq!(inv.take_from_slot(0, 9).unwrap().count, 1);
        assert!(inv.slot(0).is_none());
        assert!(inv.take_from_slot(0, 1).is_none());
    }

    #[test]
    fn hotbar_and_main_partition_the_slots() {
        let inv = Inventory::new();
        assert_eq!(inv.hotbar().len(), HOTBAR_SIZE);
        assert_eq!(inv.main().len(), MAIN_SIZE);
        assert_eq!(SLOT_COUNT, 36);
        assert_eq!(STACK_LIMIT, 64);
    }
}
