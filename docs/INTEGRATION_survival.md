# Integration note — survival systems

Four new modules: `src/item.rs`, `src/inventory.rs`, `src/crafting.rs`, `src/save.rs`.
They are pure logic — no `wgpu`, no `winit`, no threads, no new dependencies, and they
never name `World`. 78 unit tests live inside them; `cargo test` and `cargo clippy` are
clean.

Assume you have not read the code. Everything you need is below.

---

## 0. Two things to do first

### 0.1 Add the module declarations

```rust
mod crafting;
mod inventory;
mod item;
mod save;
```

### 0.2 Append three block ids to `block.rs`

`item.rs` needs blocks for torch, crafting table and furnace, which `block.rs` does not
define yet. The contract says the `block.rs` owner appends them, so I did **not** touch
that file. Instead `item.rs` declares the ids it expects:

```rust
pub const BLOCK_TORCH: BlockId = BlockId(15);
pub const BLOCK_CRAFTING_TABLE: BlockId = BlockId(16);
pub const BLOCK_FURNACE: BlockId = BlockId(17);
```

When `block.rs` gains `TORCH`, `CRAFTING_TABLE` and `FURNACE`, **they must be 15, 16 and
17 in that order.** Save files store raw block numbers, so a different numbering silently
turns saved torches into something else. Once they exist you can delete the three
constants from `item.rs` and swap the four uses for `BlockId::TORCH` and friends; nothing
else changes because the numbers are identical either way.

Those blocks also need `color()`, `hardness()`, `is_opaque()` and `is_solid()` entries in
`block.rs`. A torch should be non-opaque and non-solid; the other two behave like stone.

---

## 1. The one trait you must implement

Exactly one, in whichever file you like (it needs `World` in scope, so `main.rs` or
`world.rs`):

```rust
impl save::WorldEdit for world::World {
    fn set_block_at(&mut self, x: i32, y: i32, z: i32, id: block::BlockId) {
        self.set_block(x, y, z, id);
    }
    fn carve_at(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        self.carve(x, y, z, sx, sy, sz)
    }
}
```

That is the whole thing. It is only used to put saved edits back into the world after a
load.

**Optional speed-up.** The trait has a third method, `set_mask_at`, with a default that
replays `carve_at` once per carved sub-voxel — up to 512 calls per damaged block. It is
correct against the frozen `World` API, which has no way to install a mask directly. If
`world.rs` grows a `set_mask(x, y, z, SubMask)`, override it:

```rust
    fn set_mask_at(&mut self, x: i32, y: i32, z: i32, mask: &[u8; 64]) {
        self.set_mask(x, y, z, chunk::SubMask(*mask));
    }
```

A test asserts the two paths produce identical worlds, so the override is safe to add
later. The default is fine for now: loads only touch blocks the player personally chipped.

---

## 2. What `App` holds

The persistent state is already bundled for you. Hold a `save::SaveData` and read the
fields off it rather than keeping parallel copies — that way saving costs no clone.

```rust
struct App {
    // ... existing renderer / world / camera fields ...

    /// Seed, player position and health, inventory, and every world edit.
    /// This IS the save file, in memory.
    save: save::SaveData,

    /// Where that save is written. `save::default_save_path()` is "saves/world.lsw".
    save_path: std::path::PathBuf,

    /// Seconds since the last successful save. You own this clock.
    time_since_save: f32,

    /// The open crafting grid. 9 cells; use `[..4]` for the inventory's 2x2.
    craft_grid: [Option<inventory::ItemStack>; 9],

    /// Sub-voxel mining accumulator, so mining speed is frame-rate independent.
    chip_accum: f32,
}
```

Useful accessors:

| What | Where |
|---|---|
| Inventory | `app.save.inventory` (`inventory::Inventory`) |
| Health | `app.save.player.health` (f32, 20.0 is full) |
| Player position | `app.save.player.pos` (`glam::Vec3`) |
| Yaw / pitch | `app.save.player.yaw` / `.pitch` |
| Seed | `app.save.seed` |
| World edit log | `app.save.edits` (`save::ChangeTracker`) |

Keep `app.save.player.health` as the single source of truth for health; sync `pos`, `yaw`
and `pitch` from the camera only at save time (section 6).

---

## 3. Startup, in order

```rust
let save_path = save::default_save_path();

let data = if save::save_exists(&save_path) {
    match save::load_from_file(&save_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("could not load save: {e}");   // Display is written for humans
            return;                                  // or start fresh, your call
        }
    }
} else {
    save::SaveData::new(chosen_seed)
};

let mut world = World::new(data.seed);               // seed comes FROM the save
camera.pos = data.player.pos;
camera.yaw = data.player.yaw;
camera.pitch = data.player.pitch;

let app = App { save: data, save_path, time_since_save: 0.0, /* ... */ };
```

`load_from_file` never panics. It returns `save::SaveError`, whose `Display` explains the
problem in plain words: wrong magic, an unsupported version number, a truncated file, or
a specific corruption. A save written by a future build is refused with
`UnsupportedVersion { found, supported }` rather than misparsed.

### 3.1 The one hook you must add — replaying edits into streamed chunks

Terrain is regenerated from the seed at run time, so a freshly generated chunk knows
nothing about the house the player built there. **Every time a chunk becomes resident,
its saved edits must be replayed into it**, or walking away and back will erase the
player's work — during a session as well as across a load.

```rust
// Wherever a newly generated Chunk is inserted into World::chunks:
app.save.edits.replay_chunk((pos.x, pos.y, pos.z), &mut world);
```

One line, but it has to run **after** the chunk is inserted and **before** it is meshed.
The natural home is inside `world.rs`'s generation-completion path, which I do not own —
so this is yours or the engine agent's to place. If you must do it from the main loop
instead, replay each newly resident chunk and then mark it for remesh.

`replay_chunk` on an untouched chunk is a cheap no-op, so calling it for every arriving
chunk is fine.

---

## 4. Mining

Two moves, per the design: quiet chipping and a loud full-block smash. Both consult the
same two functions.

```rust
use item::{can_harvest, mining_drop, mining_speed_multiplier};
```

### 4.1 Chipping (left-click held)

```rust
let block = world.block_at(bx, by, bz);
if block == BlockId::BEDROCK { return; }            // hardness() is INFINITY

let tool = app.save.inventory.selected_item();       // Option<ItemId>, None = bare hands
let speed = mining_speed_multiplier(tool, block);    // 1.0 hand, 2/4/6 for wood/stone/iron

app.chip_accum += dt * config::CHIP_RATE * speed / block.hardness();
while app.chip_accum >= 1.0 {
    app.chip_accum -= 1.0;

    // Only record carves the world actually performed.
    if world.block_at(bx, by, bz).is_air() { break; }

    let destroyed = world.carve(bx, by, bz, sx, sy, sz);
    app.save.edits.note_carve(bx, by, bz, sx, sy, sz);   // <-- must not be forgotten

    if destroyed {
        grant_drop(app, tool, block);
        break;
    }
}
```

`note_carve` returns the same `bool` the world does, so you can assert they agree.

### 4.2 Full-block smash

```rust
let block = world.block_at(bx, by, bz);
if block == BlockId::BEDROCK { return; }
world.set_block(bx, by, bz, BlockId::AIR);
app.save.edits.note_set_block(bx, by, bz, BlockId::AIR);
grant_drop(app, tool, block);
```

### 4.3 Drops and durability

```rust
fn grant_drop(app: &mut App, tool: Option<ItemId>, block: BlockId) {
    // One function decides both "do I get anything" and "what".
    if let Some(dropped) = mining_drop(tool, block) {
        let spilled = app.save.inventory.add_item(dropped, 1);
        // `spilled` > 0 means the inventory is full; drop it on the ground or ignore.
    }
    // One point of durability per block broken, not per sub-voxel chipped.
    if app.save.inventory.damage_selected(1) {
        // Returns true when the tool broke. The slot is already empty.
        // Play a sound / flash the hotbar here.
    }
}
```

`mining_drop(tool, block)` is `can_harvest` and `block_drop` combined and is the only call
you need. If you want them separately for UI (greying out a block the player cannot
harvest), `can_harvest(tool, block) -> bool` is public.

The gating lives in exactly one function, `item::harvest_rule`. Nothing else in the
codebase should compare tool tiers.

| Block | Drops | Needs |
|---|---|---|
| Stone | Cobblestone | any pickaxe |
| Cobblestone | itself | any pickaxe |
| Coal ore | Coal | any pickaxe |
| Iron ore | Raw iron | **stone** pickaxe or better |
| Gold ore, Diamond ore | itself | **iron** pickaxe |
| Wood, Planks | itself | nothing (axe is faster) |
| Dirt, Sand | itself | nothing |
| Grass | Dirt | nothing |
| Leaves | nothing | — (sword is faster) |
| Bedrock, Water, Air | nothing | never harvestable |

A tool of the wrong kind gives no speed bonus. A pickaxe too weak to harvest still mines
at full pickaxe speed — it just yields nothing, which is the correct frustration.

---

## 5. Placing, the hotbar, and the inventory screen

### 5.1 Placing (right-click)

```rust
let Some(stack) = app.save.inventory.selected_stack() else { return };
let Some(block) = stack.item.places() else { return };   // None for sticks, tools, ore drops

let (x, y, z) = hit.adjacent();
if !world.block_at(x, y, z).is_air() { return; }
if placement_would_trap_the_player(x, y, z) { return; }

world.set_block(x, y, z, block);
app.save.edits.note_set_block(x, y, z, block);           // <-- must not be forgotten
let selected = app.save.inventory.selected();
app.save.inventory.take_from_slot(selected, 1);
```

**Every world mutation needs its `note_*` twin.** `set_block` → `note_set_block`,
`carve` → `note_carve`. An edit you forget to note is an edit that vanishes when the
chunk streams out or the game is saved. There are only the two call sites above plus
mining, so this is easy to keep right.

### 5.2 Hotbar

```rust
inv.set_selected(i);         // number keys 1-9 map to i = 0..9; out-of-range ignored
inv.cycle_selection(delta);  // scroll wheel, wraps
inv.selected();              // usize, 0..9
inv.selected_stack();        // Option<ItemStack>
inv.selected_item();         // Option<ItemId>
inv.hotbar();                // &[Option<ItemStack>; first 9 slots]
inv.main();                  // &[Option<ItemStack>; the other 27]
```

Slot indices are flat and run `0..36`: `0..9` is the hotbar, `9..36` the main grid.

### 5.3 Inventory screen drags

```rust
inv.move_stack(from, to);    // merge if the same item, otherwise swap
inv.split_stack(from, to);   // right-click drag: half of `from` into `to`
inv.merge_stacks(from, to);  // merge only, never swaps; false if incompatible
inv.take_slot(i);            // pick a whole stack up
inv.set_slot(i, stack);      // put one down
inv.take_from_slot(i, n);    // take n items
```

Overflow is handled everywhere: merging 40 into 40 leaves 64 and 16, not 80.

### 5.4 Rendering a slot

`ItemStack { item: ItemId, count: u8, durability: u16 }`.
`item.name()` gives a label, `item.color()` a flat RGB for the icon, and `stack.wear()`
a 0..1 fraction for a durability bar (1.0 for anything that is not a tool). Tools always
have `count == 1` and never stack with each other, even two identical ones.

---

## 6. Saving

```rust
// Once per frame:
app.time_since_save += dt;
if save::should_autosave(app.time_since_save) {
    write_save(app);
}
```

```rust
fn write_save(app: &mut App, camera: &Camera) {
    // Sync the transient camera state into the persistent record first.
    app.save.player.pos = camera.pos;
    app.save.player.yaw = camera.yaw;
    app.save.player.pitch = camera.pitch;

    match save::save_to_file(&app.save_path, &app.save) {
        Ok(()) => app.time_since_save = 0.0,
        Err(e) => eprintln!("autosave failed: {e}"),   // do not reset the clock
    }
}
```

Call the same function on exit (`WindowEvent::CloseRequested` and whatever handles Esc-to-
quit). `should_autosave(elapsed) -> bool` is a pure predicate — it starts no threads and
keeps no clock, so you own the timer and reset it only on success.
`save::AUTOSAVE_INTERVAL_SECS` is 120.0.

`save_to_file` creates parent directories, writes to a sibling `.tmp` and renames over
the target, so a crash mid-write leaves the previous save intact rather than a
half-written one.

### What actually gets written

The seed, the player, the inventory, and **only the blocks and carve masks the player
changed** — nothing generated. A world nobody touched saves in about 50 bytes. That is
the whole point of `ChangeTracker`: it is the durable record of edits, fed by your
`note_*` calls, so it survives chunks streaming out and never needs worldgen re-run to
diff against.

---

## 7. Death and respawn

```rust
if app.save.player.health <= 0.0 {
    let dropped: Vec<ItemStack> = app.save.inventory.drop_all();
    // `dropped` is everything the player was carrying, in slot order, and the
    // inventory is now empty. Spawn item entities from it, or discard it.
    app.save.player.health = 20.0;
    camera.pos = spawn_point;
}
```

There is no item-entity system in these modules — `drop_all()` hands you the stacks and
stops there. If nobody builds ground items, discarding the vector is a complete
implementation of "full inventory drop".

---

## 8. Crafting

```rust
use crafting::{consume, craft, resolve, recipe_for};
```

`resolve(&grid) -> Option<ItemStack>` inspects a grid without touching it — use it for the
output-slot preview every frame. `grid` is a flat, row-major slice of `Option<ItemStack>`,
length **4** (2x2, in the inventory screen) or **9** (3x3, at a crafting table). Any other
length resolves to `None`.

Shaped recipes match anywhere in the grid and in either horizontal mirroring, so a
left-handed axe works and a pickaxe built in the right-hand columns still resolves.
Shapeless recipes ignore position entirely.

Crafting on click, with the full-inventory case handled correctly:

```rust
if let Some(out) = resolve(&app.craft_grid) {
    if app.save.inventory.add(out).is_none() {
        // The whole output fit, so it is safe to spend the ingredients.
        consume(&mut app.craft_grid);
    }
    // Otherwise nothing happened: no ingredients spent, no items lost.
}
```

`craft(&mut grid)` does resolve-then-consume in one call if you would rather not check.
`recipe_for(&grid)` returns the matched `&Recipe` for a recipe-book UI.

The fourteen recipes, all in `crafting::RECIPES`:

| Output | Grid |
|---|---|
| 4 Planks | 1 Wood, anywhere (shapeless) |
| 4 Sticks | 2 Planks, stacked vertically |
| Crafting table | 4 Planks in a 2x2 |
| Furnace | 8 Cobblestone in a 3x3 ring, centre empty |
| 4 Torches | Coal above a Stick |
| Pickaxe (wood/stone/iron) | 3 material across the top, 2 sticks down the middle |
| Axe (wood/stone/iron) | material in an L, 2 sticks down the shaft |
| Sword (wood/stone/iron) | 2 material above 1 stick |

Tool materials are Planks, Cobblestone and Iron Ingot.

### 8.1 Furnace

`crafting::Furnace` has three public slots (`input`, `fuel`, `output`) and a `tick(dt)`.
It owns no timers and spawns nothing.

```rust
furnace.tick(dt);                       // once per frame while it exists
furnace.burn_fraction();                // 0..1, for the flame icon
furnace.progress_fraction();            // 0..1, for the arrow gauge
furnace.take_output();                  // Option<ItemStack>
```

Raw iron smelts to an iron ingot in 10 seconds. Coal burns for 80 seconds; planks, wood
and a crafting table for 15; a stick for 5 (`crafting::fuel_seconds`). Fuel is lit only
when there is something to smelt and room for the result, so a furnace left with coal and
no ore wastes nothing.

Where furnaces live is your decision — a `HashMap<(i32,i32,i32), Furnace>` on `App`
keyed by block position is the obvious shape, since these modules deliberately store no
world state.

---

## 9. Things I could not do, and notes for whoever owns those files

1. **`BlockId::TORCH` / `CRAFTING_TABLE` / `FURNACE` do not exist.** Working around it is
   section 0.2. This is the only thing blocking crafted blocks from being placeable.
2. **The frozen `World` API has no way to install a carve mask**, only to clear one
   sub-voxel at a time. `WorldEdit::set_mask_at` therefore defaults to replaying carves.
   It is correct but does up to 512 calls per damaged block on load. A `World::set_mask`
   would let you override it — see section 1. Reporting rather than changing the
   signature, per contract rule 1.
3. **`replay_chunk` needs a call site in the chunk-arrival path** (section 3.1). I cannot
   place it because I do not own `world.rs`. Without it the player's edits are lost as
   soon as a chunk streams out and back, save file or no save file.
4. **`save.rs` re-declares `CHUNK_SIZE = 16` and `SUBVOX = 8`** as private constants at
   the top of the file rather than importing `config`, so the module stays decoupled and
   the on-disk layout cannot drift when a tuning constant moves. If `config.rs` ever
   changes either value, change them in `save.rs` too and bump `save::VERSION`. Its mask
   bit layout and chunk index layout are asserted to match `chunk.rs` by unit test.
5. **No item entities on the ground.** `drop_all()` returns the stacks; spawning and
   collecting them belongs to whoever builds entities.
6. **Health, fall damage and the day/night cycle are not mine.** `save.rs` persists
   `player.health`; applying damage and the respawn rules is main-loop work (section 7).
7. **Gold ore and diamond ore drop themselves as block items.** There is no gold ingot or
   diamond item, because nothing in scope consumes them and the brief lists only sticks,
   coal, raw iron and iron ingot as non-block items.

## 10. Quick reference — everything the main loop calls

| When | Call |
|---|---|
| Startup | `save::save_exists`, `save::load_from_file` or `save::SaveData::new` |
| Chunk becomes resident | `edits.replay_chunk((x, y, z), &mut world)` |
| Mining, per frame | `item::mining_speed_multiplier(tool, block)` |
| After `world.carve` | `edits.note_carve(x, y, z, sx, sy, sz)` |
| After `world.set_block` | `edits.note_set_block(x, y, z, id)` |
| Block destroyed | `item::mining_drop(tool, block)`, `inventory.add_item`, `inventory.damage_selected(1)` |
| Right-click | `stack.item.places()`, `inventory.take_from_slot(selected, 1)` |
| Hotbar input | `inventory.set_selected` / `cycle_selection` |
| Crafting UI, per frame | `crafting::resolve(&grid)` |
| Craft clicked | `inventory.add(out)` then `crafting::consume(&mut grid)` |
| Furnace open, per frame | `furnace.tick(dt)` |
| Death | `inventory.drop_all()` |
| Every frame | `save::should_autosave(app.time_since_save)` |
| Autosave / exit | `save::save_to_file(&path, &app.save)` |
