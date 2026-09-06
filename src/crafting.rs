//! Crafting grids and furnace smelting.
//!
//! A crafting grid is a flat, row-major slice of slots: 4 long for the 2x2
//! inventory grid, 9 long for the 3x3 crafting table. [`resolve`] takes one and
//! reports what it would produce, without touching it. [`craft`] does the same
//! and then spends the ingredients.
//!
//! Shaped recipes match anywhere inside the grid and in either horizontal
//! mirroring, so a pickaxe crafted in the right-hand columns still works.
//! Shapeless recipes ignore position entirely.

use crate::block::registry;
use crate::inventory::ItemStack;
use crate::item::ItemId;

/// Slot count of the inventory's 2x2 grid.
pub const GRID_2X2: usize = 4;
/// Slot count of the crafting table's 3x3 grid.
pub const GRID_3X3: usize = 9;

/// How a recipe's ingredients are arranged.
///
/// Built by the registry from `assets/data/recipes.ron`, where a shaped recipe
/// is drawn as rows of text with a character key.
#[derive(Clone, Debug)]
pub enum Pattern {
    /// Ingredients in any arrangement. The grid must hold exactly these items,
    /// one per occupied cell, and nothing else.
    Shapeless(Vec<ItemId>),
    /// A `width` x `height` block of cells that must appear somewhere in the
    /// grid, with every cell outside it empty.
    Shaped {
        width: usize,
        height: usize,
        cells: Vec<Option<ItemId>>,
    },
}

/// One craftable result.
#[derive(Clone, Debug)]
pub struct Recipe {
    pub pattern: Pattern,
    pub output: ItemId,
    pub count: u8,
}

/// Every crafting recipe in the game, in the order `recipes.ron` lists them.
/// Order matters only in that the first match wins; no two recipes should be
/// able to match the same grid.
pub fn recipes() -> &'static [Recipe] {
    registry::get().recipes()
}

/// Grid dimensions for a slice length, or `None` for a length that is not a
/// supported grid.
fn grid_dims(len: usize) -> Option<(usize, usize)> {
    match len {
        GRID_2X2 => Some((2, 2)),
        GRID_3X3 => Some((3, 3)),
        _ => None,
    }
}

/// What this grid would produce, or `None` if the arrangement is not a recipe.
/// Does not modify the grid.
pub fn resolve(grid: &[Option<ItemStack>]) -> Option<ItemStack> {
    let (w, h) = grid_dims(grid.len())?;
    if grid.iter().all(|c| c.is_none()) {
        return None;
    }
    let recipe = recipes().iter().find(|r| matches_grid(r, grid, w, h))?;
    Some(ItemStack::new(recipe.output, recipe.count))
}

/// Resolve, then spend one item from every occupied cell. Returns the result,
/// leaving the grid untouched if nothing matched.
pub fn craft(grid: &mut [Option<ItemStack>]) -> Option<ItemStack> {
    let out = resolve(grid)?;
    consume(grid);
    Some(out)
}

/// Spend one item from every occupied cell, emptying cells that run out. This
/// is what a successful craft costs; call [`resolve`] first.
pub fn consume(grid: &mut [Option<ItemStack>]) {
    for cell in grid.iter_mut() {
        let Some(stack) = cell else { continue };
        stack.count -= 1;
        if stack.count == 0 {
            *cell = None;
        }
    }
}

/// Every recipe this grid could satisfy if the player had the materials --
/// useful for a recipe book. Kept separate from `resolve` so the hot path stays
/// a single pass.
pub fn recipe_for(grid: &[Option<ItemStack>]) -> Option<&'static Recipe> {
    let (w, h) = grid_dims(grid.len())?;
    recipes().iter().find(|r| matches_grid(r, grid, w, h))
}

fn matches_grid(recipe: &Recipe, grid: &[Option<ItemStack>], w: usize, h: usize) -> bool {
    match &recipe.pattern {
        Pattern::Shapeless(items) => matches_shapeless(grid, items),
        Pattern::Shaped {
            width,
            height,
            cells,
        } => matches_shaped(grid, w, h, *width, *height, cells),
    }
}

/// Position-independent match: the occupied cells must be exactly the recipe's
/// ingredient multiset.
fn matches_shapeless(grid: &[Option<ItemStack>], ingredients: &[ItemId]) -> bool {
    let mut present: Vec<ItemId> = grid.iter().flatten().map(|s| s.item).collect();
    if present.len() != ingredients.len() {
        return false;
    }
    for want in ingredients {
        match present.iter().position(|p| p == want) {
            Some(i) => {
                present.swap_remove(i);
            }
            None => return false,
        }
    }
    true
}

/// Slide the pattern over every position it fits, in both horizontal mirrorings.
fn matches_shaped(
    grid: &[Option<ItemStack>],
    gw: usize,
    gh: usize,
    pw: usize,
    ph: usize,
    cells: &[Option<ItemId>],
) -> bool {
    if pw > gw || ph > gh {
        return false;
    }
    for oy in 0..=(gh - ph) {
        for ox in 0..=(gw - pw) {
            for mirror in [false, true] {
                if window_matches(grid, gw, gh, ox, oy, pw, ph, cells, mirror) {
                    return true;
                }
            }
        }
    }
    false
}

/// True when the whole grid equals the pattern placed at `(ox, oy)` and is
/// empty everywhere else.
#[allow(clippy::too_many_arguments)]
fn window_matches(
    grid: &[Option<ItemStack>],
    gw: usize,
    gh: usize,
    ox: usize,
    oy: usize,
    pw: usize,
    ph: usize,
    cells: &[Option<ItemId>],
    mirror: bool,
) -> bool {
    for gy in 0..gh {
        for gx in 0..gw {
            let inside = gx >= ox && gx < ox + pw && gy >= oy && gy < oy + ph;
            let want = if inside {
                let mut px = gx - ox;
                if mirror {
                    px = pw - 1 - px;
                }
                cells[(gy - oy) * pw + px]
            } else {
                None
            };
            if grid[gy * gw + gx].map(|s| s.item) != want {
                return false;
            }
        }
    }
    true
}

// --- smelting ----------------------------------------------------------------

/// One furnace conversion.
#[derive(Copy, Clone, Debug)]
pub struct SmeltRecipe {
    pub input: ItemId,
    pub output: ItemId,
    /// Seconds of burning needed to convert one item.
    pub seconds: f32,
}

/// Everything a furnace can smelt, from the `smelting:` section of
/// `recipes.ron`.
pub fn smelting() -> &'static [SmeltRecipe] {
    registry::get().smelting()
}

/// The smelting recipe for an input item, if it has one.
pub fn smelt_recipe(input: ItemId) -> Option<&'static SmeltRecipe> {
    smelting().iter().find(|r| r.input == input)
}

/// What one of `input` smelts into.
pub fn smelt_result(input: ItemId) -> Option<ItemId> {
    smelt_recipe(input).map(|r| r.output)
}

/// How many seconds of furnace burn one of this item provides, or `None` if it
/// is not a fuel. The `fuel:` field in `items.ron`; coal is the workhorse and
/// wood burns because it must.
pub fn fuel_seconds(item: ItemId) -> Option<f32> {
    item.fuel_seconds()
}

/// A placed furnace's three slots and its burn state. The caller ticks it with
/// the frame delta; it spawns nothing and owns no timers of its own.
#[derive(Clone, Debug, Default)]
pub struct Furnace {
    pub input: Option<ItemStack>,
    pub fuel: Option<ItemStack>,
    pub output: Option<ItemStack>,
    /// Seconds of burn left in the fuel item currently alight.
    burn_left: f32,
    /// What that fuel item was worth, for the flame gauge.
    burn_total: f32,
    /// Seconds of smelting accumulated on the current input.
    progress: f32,
}

/// Complete durable state of a furnace. Kept as a value type so save code can
/// snapshot and restore a furnace without reaching into its simulation internals.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FurnaceState {
    pub input: Option<ItemStack>,
    pub fuel: Option<ItemStack>,
    pub output: Option<ItemStack>,
    pub burn_left: f32,
    pub burn_total: f32,
    pub progress: f32,
}

impl Furnace {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_state(state: FurnaceState) -> Self {
        Self {
            input: state.input,
            fuel: state.fuel,
            output: state.output,
            burn_left: state.burn_left.max(0.0),
            burn_total: state.burn_total.max(0.0),
            progress: state.progress.max(0.0),
        }
    }

    pub fn state(&self) -> FurnaceState {
        FurnaceState {
            input: self.input,
            fuel: self.fuel,
            output: self.output,
            burn_left: self.burn_left,
            burn_total: self.burn_total,
            progress: self.progress,
        }
    }

    pub fn is_burning(&self) -> bool {
        self.burn_left > 0.0
    }

    /// Remaining fuel as 0.0..=1.0, for the flame icon.
    pub fn burn_fraction(&self) -> f32 {
        if self.burn_total <= 0.0 {
            0.0
        } else {
            (self.burn_left / self.burn_total).clamp(0.0, 1.0)
        }
    }

    /// Smelt progress as 0.0..=1.0, for the arrow gauge.
    pub fn progress_fraction(&self) -> f32 {
        match self.input.and_then(|s| smelt_recipe(s.item)) {
            Some(r) if r.seconds > 0.0 => (self.progress / r.seconds).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    /// Advance by `dt` seconds. Lights fuel only when there is something to
    /// smelt and somewhere to put it.
    pub fn tick(&mut self, dt: f32) {
        let recipe = self.input.and_then(|s| smelt_recipe(s.item)).copied();
        let has_work = match (recipe, self.output) {
            (Some(r), Some(out)) => out.item == r.output && !out.is_full(),
            (Some(_), None) => true,
            (None, _) => false,
        };

        if self.burn_left > 0.0 {
            self.burn_left = (self.burn_left - dt).max(0.0);
        }
        if self.burn_left <= 0.0 && has_work {
            self.light_fuel();
        }

        match recipe {
            Some(r) if has_work && self.burn_left > 0.0 => {
                self.progress += dt;
                if self.progress >= r.seconds {
                    self.progress -= r.seconds;
                    self.finish(r.output);
                }
            }
            // Progress cools off when the fire goes out or the input is pulled.
            _ => self.progress = (self.progress - dt * 2.0).max(0.0),
        }
    }

    /// Consume one fuel item and start it burning.
    fn light_fuel(&mut self) {
        let Some(fuel) = self.fuel else { return };
        let Some(seconds) = fuel_seconds(fuel.item) else {
            return;
        };
        self.burn_left = seconds;
        self.burn_total = seconds;
        let mut fuel = fuel;
        fuel.count -= 1;
        self.fuel = if fuel.count == 0 { None } else { Some(fuel) };
    }

    /// Move one finished item from input to output.
    fn finish(&mut self, output: ItemId) {
        if let Some(mut input) = self.input {
            input.count -= 1;
            self.input = if input.count == 0 { None } else { Some(input) };
        }
        match self.output.as_mut() {
            Some(out) if out.item == output && !out.is_full() => out.count += 1,
            Some(_) => {}
            None => self.output = Some(ItemStack::new(output, 1)),
        }
    }

    /// Take the finished goods out.
    pub fn take_output(&mut self) -> Option<ItemStack> {
        self.output.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shorthand for the grids below. `EM` is an empty cell.
    const EM: Option<ItemId> = None;
    const PL: Option<ItemId> = Some(ItemId::PLANKS);
    const ST: Option<ItemId> = Some(ItemId::STICK);
    const CB: Option<ItemId> = Some(ItemId::COBBLESTONE);
    const IN: Option<ItemId> = Some(ItemId::IRON_INGOT);
    const CO: Option<ItemId> = Some(ItemId::COAL);

    /// Build a 3x3 grid from item shorthand.
    fn g3(cells: [Option<ItemId>; 9]) -> Vec<Option<ItemStack>> {
        cells.iter().map(|c| c.map(ItemStack::one)).collect()
    }

    fn g2(cells: [Option<ItemId>; 4]) -> Vec<Option<ItemStack>> {
        cells.iter().map(|c| c.map(ItemStack::one)).collect()
    }

    const WD: Option<ItemId> = Some(ItemId::WOOD);

    #[test]
    fn planks_from_wood_anywhere_in_either_grid() {
        for slot in 0..9 {
            let mut cells = [EM; 9];
            cells[slot] = WD;
            let out = resolve(&g3(cells)).expect("wood should always give planks");
            assert_eq!(out.item, ItemId::PLANKS);
            assert_eq!(out.count, 4);
        }
        let out = resolve(&g2([EM, WD, EM, EM])).unwrap();
        assert_eq!((out.item, out.count), (ItemId::PLANKS, 4));
    }

    #[test]
    fn sticks_from_two_planks_stacked() {
        let out = resolve(&g3([EM, EM, EM, EM, PL, EM, EM, PL, EM])).unwrap();
        assert_eq!((out.item, out.count), (ItemId::STICK, 4));
        // Also works in the 2x2 grid.
        let out = resolve(&g2([EM, PL, EM, PL])).unwrap();
        assert_eq!((out.item, out.count), (ItemId::STICK, 4));
        // Side by side is not the recipe.
        assert!(resolve(&g2([PL, PL, EM, EM])).is_none());
    }

    #[test]
    fn crafting_table_from_four_planks() {
        let out = resolve(&g2([PL, PL, PL, PL])).unwrap();
        assert_eq!((out.item, out.count), (ItemId::CRAFTING_TABLE, 1));
        // And in the corner of a 3x3.
        let out = resolve(&g3([EM, PL, PL, EM, PL, PL, EM, EM, EM])).unwrap();
        assert_eq!(out.item, ItemId::CRAFTING_TABLE);
        // Three planks is not a table.
        assert!(resolve(&g2([PL, PL, PL, EM])).is_none());
    }

    #[test]
    fn furnace_from_eight_cobblestone() {
        let out = resolve(&g3([CB, CB, CB, CB, EM, CB, CB, CB, CB])).unwrap();
        assert_eq!((out.item, out.count), (ItemId::FURNACE, 1));
        // A filled centre is a near miss.
        assert!(resolve(&g3([CB, CB, CB, CB, CB, CB, CB, CB, CB])).is_none());
        // A missing corner is a near miss.
        assert!(resolve(&g3([EM, CB, CB, CB, EM, CB, CB, CB, CB])).is_none());
    }

    #[test]
    fn torches_from_coal_over_a_stick() {
        let out = resolve(&g2([CO, EM, ST, EM])).unwrap();
        assert_eq!((out.item, out.count), (ItemId::TORCH, 4));
        // Upside down is not the recipe.
        assert!(resolve(&g2([ST, EM, CO, EM])).is_none());
    }

    #[test]
    fn every_pickaxe_tier() {
        for (mat, want) in [
            (PL, ItemId::WOODEN_PICKAXE),
            (CB, ItemId::STONE_PICKAXE),
            (IN, ItemId::IRON_PICKAXE),
        ] {
            let out = resolve(&g3([mat, mat, mat, EM, ST, EM, EM, ST, EM])).unwrap();
            assert_eq!(out.item, want);
            assert_eq!(out.count, 1);
        }
    }

    #[test]
    fn every_axe_tier_in_both_handednesses() {
        for (mat, want) in [
            (PL, ItemId::WOODEN_AXE),
            (CB, ItemId::STONE_AXE),
            (IN, ItemId::IRON_AXE),
        ] {
            let right = resolve(&g3([mat, mat, EM, mat, ST, EM, EM, ST, EM])).unwrap();
            assert_eq!(right.item, want);
            let left = resolve(&g3([EM, mat, mat, EM, ST, mat, EM, ST, EM])).unwrap();
            assert_eq!(left.item, want);
        }
    }

    #[test]
    fn every_sword_tier() {
        for (mat, want) in [
            (PL, ItemId::WOODEN_SWORD),
            (CB, ItemId::STONE_SWORD),
            (IN, ItemId::IRON_SWORD),
        ] {
            let out = resolve(&g3([EM, mat, EM, EM, mat, EM, EM, ST, EM])).unwrap();
            assert_eq!(out.item, want);
            // Shifted into the left column.
            let out = resolve(&g3([mat, EM, EM, mat, EM, EM, ST, EM, EM])).unwrap();
            assert_eq!(out.item, want);
        }
    }

    #[test]
    fn the_table_holds_about_a_dozen_recipes() {
        let n = recipes().len();
        assert!(n >= 12, "expected ~12 recipes, found {n}");
    }

    /// Every recipe the data file lists must be buildable from items that
    /// exist. A typo in `recipes.ron` should never reach a player as a recipe
    /// that silently never matches.
    #[test]
    fn every_recipe_names_items_that_exist() {
        for r in recipes() {
            assert!(
                r.output.is_valid(),
                "recipe makes an item that does not exist"
            );
            assert!(r.count > 0 && r.count <= 64);
            match &r.pattern {
                Pattern::Shapeless(items) => {
                    assert!(!items.is_empty());
                    for i in items {
                        assert!(i.is_valid(), "{i:?} is not a real item");
                    }
                }
                Pattern::Shaped {
                    width,
                    height,
                    cells,
                } => {
                    assert_eq!(cells.len(), width * height);
                    assert!(*width <= 3 && *height <= 3, "pattern will never fit a grid");
                    for i in cells.iter().flatten() {
                        assert!(i.is_valid(), "{i:?} is not a real item");
                    }
                }
            }
        }
        for s in smelting() {
            assert!(s.input.is_valid() && s.output.is_valid());
            assert!(s.seconds > 0.0);
        }
    }

    #[test]
    fn near_misses_resolve_to_none() {
        // Empty grid.
        assert!(resolve(&g3([EM; 9])).is_none());
        // Pickaxe with a stray extra item.
        assert!(resolve(&g3([PL, PL, PL, PL, ST, EM, EM, ST, EM])).is_none());
        // Pickaxe with a missing head piece.
        assert!(resolve(&g3([PL, PL, EM, EM, ST, EM, EM, ST, EM])).is_none());
        // Sticks where the head should be.
        assert!(resolve(&g3([ST, ST, ST, EM, ST, EM, EM, ST, EM])).is_none());
        // Mixed-material pickaxe.
        assert!(resolve(&g3([PL, CB, PL, EM, ST, EM, EM, ST, EM])).is_none());
        // Sword pattern split across a gap.
        assert!(resolve(&g3([PL, EM, EM, EM, EM, EM, ST, EM, EM])).is_none());
        // Unsupported grid sizes.
        assert!(resolve(&[]).is_none());
        assert!(resolve(&[None; 6]).is_none());
    }

    #[test]
    fn a_shapeless_recipe_rejects_extra_ingredients() {
        // Two wood is not the one-wood recipe.
        assert!(resolve(&g2([WD, WD, EM, EM])).is_none());
    }

    #[test]
    fn stack_counts_do_not_affect_matching() {
        // A 64-stack of wood in one cell still yields exactly 4 planks.
        let grid = vec![Some(ItemStack::new(ItemId::WOOD, 64)), None, None, None];
        let out = resolve(&grid).unwrap();
        assert_eq!((out.item, out.count), (ItemId::PLANKS, 4));
    }

    #[test]
    fn craft_spends_one_from_each_cell() {
        let mut grid = vec![
            Some(ItemStack::new(ItemId::PLANKS, 3)),
            None,
            Some(ItemStack::new(ItemId::PLANKS, 1)),
            None,
        ];
        // Planks in slots 0 and 2 is the vertical stick pattern in a 2x2 grid.
        let out = craft(&mut grid).unwrap();
        assert_eq!((out.item, out.count), (ItemId::STICK, 4));
        assert_eq!(grid[0].unwrap().count, 2);
        assert!(grid[2].is_none());
    }

    #[test]
    fn craft_on_a_non_recipe_changes_nothing() {
        let mut grid = vec![Some(ItemStack::new(ItemId::DIRT, 4)), None, None, None];
        assert!(craft(&mut grid).is_none());
        assert_eq!(grid[0].unwrap().count, 4);
    }

    #[test]
    fn a_tool_in_the_grid_never_matches() {
        let mut cells = [EM; 9];
        cells[0] = Some(ItemId::WOODEN_PICKAXE);
        assert!(resolve(&g3(cells)).is_none());
    }

    #[test]
    fn recipe_for_reports_the_matched_recipe() {
        let r = recipe_for(&g2([PL, PL, PL, PL])).unwrap();
        assert_eq!(r.output, ItemId::CRAFTING_TABLE);
        assert!(recipe_for(&g2([EM; 4])).is_none());
    }

    // --- smelting ---

    #[test]
    fn raw_iron_smelts_to_an_ingot() {
        assert_eq!(smelt_result(ItemId::RAW_IRON), Some(ItemId::IRON_INGOT));
        assert_eq!(smelt_result(ItemId::COBBLESTONE), None);
        assert_eq!(smelt_result(ItemId::IRON_INGOT), None);
    }

    #[test]
    fn coal_is_fuel_and_iron_is_not() {
        assert_eq!(fuel_seconds(ItemId::COAL), Some(80.0));
        assert_eq!(fuel_seconds(ItemId::IRON_INGOT), None);
        assert_eq!(fuel_seconds(ItemId::RAW_IRON), None);
        assert!(fuel_seconds(ItemId::PLANKS).unwrap() < fuel_seconds(ItemId::COAL).unwrap());
    }

    #[test]
    fn furnace_smelts_while_fuelled() {
        let mut f = Furnace::new();
        f.input = Some(ItemStack::new(ItemId::RAW_IRON, 3));
        f.fuel = Some(ItemStack::new(ItemId::COAL, 1));
        assert!(!f.is_burning());
        f.tick(0.5);
        assert!(f.is_burning());
        assert_eq!(f.fuel, None, "one coal is consumed on lighting");
        // Ten seconds per ingot.
        for _ in 0..19 {
            f.tick(0.5);
        }
        assert_eq!(f.output.unwrap().item, ItemId::IRON_INGOT);
        assert_eq!(f.output.unwrap().count, 1);
        assert_eq!(f.input.unwrap().count, 2);
        // The rest of the coal finishes the other two.
        for _ in 0..40 {
            f.tick(0.5);
        }
        assert_eq!(f.output.unwrap().count, 3);
        assert!(f.input.is_none());
        assert_eq!(f.take_output().unwrap().count, 3);
        assert!(f.output.is_none());
    }

    #[test]
    fn furnace_does_not_burn_fuel_with_nothing_to_smelt() {
        let mut f = Furnace::new();
        f.fuel = Some(ItemStack::new(ItemId::COAL, 4));
        for _ in 0..100 {
            f.tick(1.0);
        }
        assert_eq!(f.fuel.unwrap().count, 4);
        assert!(!f.is_burning());
        assert_eq!(f.burn_fraction(), 0.0);
    }

    #[test]
    fn furnace_stalls_without_fuel_and_progress_decays() {
        let mut f = Furnace::new();
        f.input = Some(ItemStack::new(ItemId::RAW_IRON, 1));
        for _ in 0..40 {
            f.tick(1.0);
        }
        assert!(f.output.is_none());
        assert_eq!(f.input.unwrap().count, 1);
        assert_eq!(f.progress_fraction(), 0.0);
    }

    #[test]
    fn furnace_refuses_a_mismatched_output_slot() {
        let mut f = Furnace::new();
        f.input = Some(ItemStack::new(ItemId::RAW_IRON, 1));
        f.fuel = Some(ItemStack::new(ItemId::COAL, 1));
        f.output = Some(ItemStack::new(ItemId::DIRT, 1));
        for _ in 0..40 {
            f.tick(1.0);
        }
        assert_eq!(f.output.unwrap().item, ItemId::DIRT);
        assert_eq!(f.input.unwrap().count, 1);
        assert!(
            !f.is_burning(),
            "fuel should not be spent on a blocked furnace"
        );
    }

    #[test]
    fn burn_and_progress_gauges_stay_in_range() {
        let mut f = Furnace::new();
        f.input = Some(ItemStack::new(ItemId::RAW_IRON, 2));
        f.fuel = Some(ItemStack::new(ItemId::COAL, 1));
        for _ in 0..30 {
            f.tick(0.5);
            assert!((0.0..=1.0).contains(&f.burn_fraction()));
            assert!((0.0..=1.0).contains(&f.progress_fraction()));
        }
    }
}
