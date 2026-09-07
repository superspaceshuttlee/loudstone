//! Menus, panels, and the rules for moving a stack of items with the cursor.
//!
//! Two things live here and they are deliberately kept apart from the game.
//!
//! The **layout** functions are pure: given a window size they say where a slot
//! is, and nothing else. Click handling and drawing both ask them, so a slot can
//! never be drawn in one place and clicked in another -- which is the classic
//! way inventory UIs rot, with two copies of the same arithmetic drifting apart.
//!
//! The **stack-moving** functions take plain `&mut Option<ItemStack>` rather
//! than reaching into the inventory. That is what lets the fiddly rules -- split
//! on right-click, merge only up to the stack limit, never merge two damaged
//! tools -- be tested directly, without a window or a world.

use crate::Ui;
use crate::content::item::ItemId;
use crate::render::texture::{self, TileId};

/// Item art is drawn at full brightness; the tint exists for biome-coloured
/// blocks, which is a world concern the inventory does not share.
const TINT: [f32; 3] = [1.0, 1.0, 1.0];

/// The atlas tile that stands for one item in a slot.
fn item_art(item: ItemId) -> TileId {
    texture::item_tile(item)
}
use crate::content::crafting;
use crate::content::inventory::{self, ItemStack};
use crate::render::gfx;
use crate::render::hud::{self, Slot};

// --- title screen -----------------------------------------------------------

pub const MENU_BUTTON_W: f32 = 360.0;
pub const MENU_BUTTON_H: f32 = 52.0;
pub const MENU_BUTTON_GAP: f32 = 14.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TitleAction {
    Continue,
    NewWorld,
    Quit,
}

pub fn menu_button_rect(w: f32, h: f32, index: usize) -> (f32, f32, f32, f32) {
    let total_h = MENU_BUTTON_H * 3.0 + MENU_BUTTON_GAP * 2.0;
    (
        (w - MENU_BUTTON_W) * 0.5,
        h * 0.55 - total_h * 0.5 + index as f32 * (MENU_BUTTON_H + MENU_BUTTON_GAP),
        MENU_BUTTON_W,
        MENU_BUTTON_H,
    )
}

pub fn point_in_rect(point: (f32, f32), rect: (f32, f32, f32, f32)) -> bool {
    point.0 >= rect.0 && point.0 < rect.0 + rect.2 && point.1 >= rect.1 && point.1 < rect.1 + rect.3
}

pub fn title_action(w: f32, h: f32, cursor: (f32, f32), has_save: bool) -> Option<TitleAction> {
    if has_save && point_in_rect(cursor, menu_button_rect(w, h, 0)) {
        Some(TitleAction::Continue)
    } else if point_in_rect(cursor, menu_button_rect(w, h, 1)) {
        Some(TitleAction::NewWorld)
    } else if point_in_rect(cursor, menu_button_rect(w, h, 2)) {
        Some(TitleAction::Quit)
    } else {
        None
    }
}

pub fn draw_title_screen(gfx: &mut gfx::Renderer, has_save: bool, cursor: (f32, f32)) {
    let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
    gfx.hud.rect(0.0, 0.0, w, h, [0.025, 0.045, 0.075, 1.0]);

    let title = "LOUDSTONE";
    let title_size = 52.0;
    gfx.hud.text_shadowed(
        (w - hud::text_width(title_size, title)) * 0.5,
        h * 0.18,
        title_size,
        [0.93, 0.95, 0.98, 1.0],
        title,
    );
    let subtitle = "A WORLD SHAPED BY SOUND";
    gfx.hud.text(
        (w - hud::text_width(hud::TEXT_SIZE, subtitle)) * 0.5,
        h * 0.18 + 66.0,
        hud::TEXT_SIZE,
        [0.55, 0.72, 0.76, 1.0],
        subtitle,
    );

    let actions = [
        ("CONTINUE WORLD", has_save),
        ("CREATE NEW WORLD", true),
        ("QUIT GAME", true),
    ];
    for (index, (label, enabled)) in actions.into_iter().enumerate() {
        let rect = menu_button_rect(w, h, index);
        let hovered = enabled && point_in_rect(cursor, rect);
        let fill = if !enabled {
            [0.10, 0.12, 0.15, 0.96]
        } else if hovered {
            [0.22, 0.38, 0.40, 0.98]
        } else {
            [0.14, 0.20, 0.23, 0.98]
        };
        let edge = if hovered {
            [0.78, 0.92, 0.82, 1.0]
        } else {
            [0.38, 0.48, 0.50, 1.0]
        };
        gfx.hud.rect(rect.0, rect.1, rect.2, rect.3, fill);
        gfx.hud.border(rect.0, rect.1, rect.2, rect.3, 2.0, edge);
        let color = if enabled {
            [0.94, 0.96, 0.96, 1.0]
        } else {
            [0.42, 0.45, 0.46, 1.0]
        };
        let size = 18.0;
        gfx.hud.text_shadowed(
            rect.0 + (rect.2 - hud::text_width(size, label)) * 0.5,
            rect.1 + (rect.3 - size) * 0.5,
            size,
            color,
            label,
        );
    }

    let save_note = if has_save {
        "CONTINUE LOADS SAVES/WORLD.LSW"
    } else {
        "NO SAVED WORLD YET"
    };
    gfx.hud.text(
        (w - hud::text_width(hud::TEXT_SIZE, save_note)) * 0.5,
        h * 0.83,
        hud::TEXT_SIZE,
        [0.48, 0.57, 0.59, 1.0],
        save_note,
    );
}

// --- inventory screen geometry, shared by drawing and hit-testing ------------

pub const SLOT_PX: f32 = 44.0;
pub const SLOT_GAP: f32 = 4.0;

/// Top-left of the 9x4 inventory grid for a given screen size.
pub fn inv_origin(w: f32, h: f32) -> (f32, f32) {
    let gw = 9.0 * SLOT_PX + 8.0 * SLOT_GAP;
    ((w - gw) * 0.5, h * 0.5 - 40.0)
}

/// Screen rect of one flat slot index (0..36).
pub fn slot_rect(w: f32, h: f32, index: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    // Row 0 is the hotbar, drawn at the bottom of the panel with a gap.
    let (col, row) = (index % 9, index / 9);
    let y = if row == 0 {
        oy + 3.0 * (SLOT_PX + SLOT_GAP) + 14.0
    } else {
        oy + (row as f32 - 1.0) * (SLOT_PX + SLOT_GAP)
    };
    (ox + col as f32 * (SLOT_PX + SLOT_GAP), y)
}

/// Crafting cell rect. `cells` is 4 (2x2) or 9 (3x3); the grid stays centred on
/// the same column either way, so the panel does not jump when it widens.
pub fn craft_rect(w: f32, h: f32, index: usize, cells: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    let step = SLOT_PX + SLOT_GAP;
    let side = if cells == 9 { 3 } else { 2 };
    let cx = ox + 4.6 * step - side as f32 * step * 0.5;
    let cy = oy - (0.4 + side as f32) * step;
    (
        cx + (index % side) as f32 * step,
        cy + (index / side) as f32 * step,
    )
}

pub fn craft_output_rect(w: f32, h: f32, cells: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    let step = SLOT_PX + SLOT_GAP;
    let side = if cells == 9 { 3 } else { 2 };
    (ox + 6.4 * step, oy - (0.9 + side as f32 * 0.5) * step)
}

/// Furnace slots: 0 input (top), 1 fuel (below it), 2 output (to the right).
pub fn furnace_rect(w: f32, h: f32, index: usize) -> (f32, f32) {
    let (ox, oy) = inv_origin(w, h);
    let step = SLOT_PX + SLOT_GAP;
    let cx = ox + 3.2 * step;
    let cy = oy - 3.4 * step;
    match index {
        0 => (cx, cy),
        1 => (cx, cy + 2.0 * step),
        _ => (cx + 3.0 * step, cy + step),
    }
}

pub fn draw_panel(
    gfx: &mut gfx::Renderer,
    ui: Ui,
    inv: &inventory::Inventory,
    grid: &[Option<ItemStack>; 9],
    furnace: Option<&crafting::Furnace>,
    carried: Option<ItemStack>,
    cursor: (f32, f32),
) {
    let (w, h) = (gfx.config.width as f32, gfx.config.height as f32);
    gfx.hud.screen_dim();
    let (ox, oy) = inv_origin(w, h);
    let gw = 9.0 * SLOT_PX + 8.0 * SLOT_GAP;
    let step = SLOT_PX + SLOT_GAP;
    gfx.hud
        .panel(ox - 16.0, oy - 4.8 * step, gw + 32.0, 8.2 * step + 40.0);

    let to_slot =
        |s: Option<ItemStack>| s.map(|st| Slot::new(item_art(st.item), TINT, st.count as u16));

    // The 36 inventory slots are common to every panel.
    for i in 0..36 {
        let (x, y) = slot_rect(w, h, i);
        gfx.hud
            .slot(x, y, SLOT_PX, to_slot(inv.slot(i)), i == inv.selected());
    }

    let title = match ui {
        Ui::Furnace(_) => "FURNACE   ore above, fuel below",
        Ui::Table => "CRAFTING TABLE   3x3",
        _ => "INVENTORY   2x2, table for tools",
    };

    match ui {
        Ui::Furnace(_) => {
            let f = furnace.expect("furnace panel opened without a furnace");
            for (i, stack) in [f.input, f.fuel, f.output].iter().enumerate() {
                let (x, y) = furnace_rect(w, h, i);
                gfx.hud.slot(x, y, SLOT_PX, to_slot(*stack), false);
            }
            // Flame and progress gauges, as plain bars.
            let (fx, fy) = furnace_rect(w, h, 1);
            let burn = f.burn_fraction();
            gfx.hud.rect(
                fx + SLOT_PX + 8.0,
                fy + SLOT_PX * (1.0 - burn),
                10.0,
                SLOT_PX * burn,
                [0.95, 0.55, 0.15, 1.0],
            );
            let (px, py) = furnace_rect(w, h, 0);
            let prog = f.progress_fraction();
            gfx.hud.rect(
                px + SLOT_PX + 8.0,
                py + SLOT_PX * 0.45,
                (2.6 * step - 16.0) * prog,
                10.0,
                [0.85, 0.85, 0.9, 1.0],
            );
        }
        _ => {
            let cells = ui.craft_cells();
            for i in 0..cells {
                let (x, y) = craft_rect(w, h, i, cells);
                gfx.hud.slot(x, y, SLOT_PX, to_slot(grid[i]), false);
            }
            let (cx, cy) = craft_output_rect(w, h, cells);
            gfx.hud.slot(
                cx,
                cy,
                SLOT_PX,
                to_slot(crafting::resolve(&grid[..cells])),
                false,
            );
        }
    }

    gfx.hud.text_shadowed(
        ox,
        oy - 4.55 * step,
        hud::TEXT_SIZE,
        [0.92, 0.92, 0.95, 1.0],
        title,
    );

    // The carried stack rides the cursor so it is obvious what is in hand.
    if let Some(st) = carried {
        gfx.hud.slot(
            cursor.0 - SLOT_PX * 0.4,
            cursor.1 - SLOT_PX * 0.4,
            SLOT_PX * 0.8,
            to_slot(Some(st)),
            false,
        );
    }
}

/// Pick up, put down, merge, or split one stack against the carried one.
pub fn swap_carried(carried: &mut Option<ItemStack>, cell: &mut Option<ItemStack>, right: bool) {
    match (carried.take(), cell.take()) {
        (None, Some(s)) => {
            if right && s.count > 1 {
                // Right click takes half and leaves the rest.
                let half = s.count / 2;
                *carried = Some(ItemStack::new(s.item, half));
                *cell = Some(ItemStack::new(s.item, s.count - half));
            } else {
                *carried = Some(s);
            }
        }
        (Some(c), None) => {
            if right && c.count > 1 {
                *cell = Some(ItemStack::new(c.item, 1));
                *carried = Some(ItemStack::new(c.item, c.count - 1));
            } else {
                *cell = Some(c);
            }
        }
        (Some(c), Some(mut s)) => {
            if s.stacks_with(c) && !s.is_full() {
                *carried = s.merge(c);
                *cell = Some(s);
            } else {
                *cell = Some(c);
                *carried = Some(s);
            }
        }
        (None, None) => {}
    }
}

pub fn merge_into_cell(cell: &mut Option<ItemStack>, stack: ItemStack) -> Option<ItemStack> {
    match cell {
        Some(existing) => existing.merge(stack),
        None => {
            *cell = Some(stack);
            None
        }
    }
}

/// Store an output transactionally. If neither the inventory nor the cursor can
/// hold it, leave both untouched so crafting or furnace output is not consumed.
pub fn store_output(
    inventory: &mut inventory::Inventory,
    carried: &mut Option<ItemStack>,
    output: ItemStack,
) -> bool {
    let mut next_inventory = inventory.clone();
    let mut next_carried = *carried;
    if let Some(rest) = next_inventory.add(output)
        && merge_into_cell(&mut next_carried, rest).is_some()
    {
        return false;
    }
    *inventory = next_inventory;
    *carried = next_carried;
    true
}

/// Return transient panel stacks without loss. A cursor stack taken from a full
/// furnace can always fall back into one of that furnace's now-empty input slots.
pub fn return_panel_items(
    inventory: &mut inventory::Inventory,
    carried: &mut Option<ItemStack>,
    craft_grid: &mut [Option<ItemStack>; 9],
    mut furnace: Option<&mut crafting::Furnace>,
) -> bool {
    let pending = carried
        .iter()
        .chain(craft_grid.iter().flatten())
        .copied()
        .collect::<Vec<_>>();
    let mut next_inventory = inventory.clone();
    let mut next_furnace = furnace.as_deref().cloned();

    for stack in pending {
        let mut rest = next_inventory.add(stack);
        if let (Some(left), Some(target)) = (rest, next_furnace.as_mut()) {
            rest = merge_into_cell(&mut target.input, left);
            if let Some(left) = rest {
                rest = merge_into_cell(&mut target.fuel, left);
            }
        }
        if rest.is_some() {
            return false;
        }
    }

    *inventory = next_inventory;
    if let (Some(target), Some(next)) = (furnace.as_deref_mut(), next_furnace) {
        *target = next;
    }
    *carried = None;
    craft_grid.fill(None);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::item::ItemId;

    fn stack(item: ItemId, n: u8) -> Option<ItemStack> {
        Some(ItemStack::new(item, n))
    }

    #[test]
    fn inventory_cursor_never_merges_or_repairs_tools() {
        let mut carried = Some(ItemStack::worn(ItemId::IRON_PICKAXE, 200));
        let mut cell = Some(ItemStack::worn(ItemId::IRON_PICKAXE, 75));

        swap_carried(&mut carried, &mut cell, false);

        assert_eq!(carried, Some(ItemStack::worn(ItemId::IRON_PICKAXE, 75)));
        assert_eq!(cell, Some(ItemStack::worn(ItemId::IRON_PICKAXE, 200)));
    }

    #[test]
    fn inventory_cursor_uses_each_items_stack_limit() {
        let mut carried = Some(ItemStack::new(ItemId::COBBLESTONE, 10));
        let mut cell = Some(ItemStack::new(ItemId::COBBLESTONE, 60));

        swap_carried(&mut carried, &mut cell, false);

        assert_eq!(cell.map(|s| s.count), Some(ItemId::COBBLESTONE.max_stack()));
        assert_eq!(carried.map(|s| s.count), Some(6));
    }

    #[test]
    fn output_collection_never_overwrites_an_incompatible_carried_stack() {
        let mut inventory = inventory::Inventory::new();
        for index in 0..inventory::SLOT_COUNT {
            inventory.set_slot(index, Some(ItemStack::new(ItemId::DIRT, 64)));
        }
        let held = ItemStack::worn(ItemId::IRON_PICKAXE, 71);
        let mut carried = Some(held);

        assert!(!store_output(
            &mut inventory,
            &mut carried,
            ItemStack::new(ItemId::IRON_INGOT, 1)
        ));
        assert_eq!(carried, Some(held));
        assert_eq!(inventory.count(ItemId::IRON_INGOT), 0);
    }
}
