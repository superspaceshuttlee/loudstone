# Integration note — mobs and containers in the save file

Only `src/save.rs` changed, plus one new method on `MobManager` in `src/mob.rs`.
`cargo test` passes (331 tests, 16 of them new) and `cargo clippy` reports nothing in
either file.

Assume you have not read the code. Everything you need is below.

---

## 0. What is new, in one paragraph

`save::SaveData` gained two fields:

```rust
pub struct SaveData {
    pub seed: u32,
    pub player: PlayerSave,
    pub inventory: Inventory,
    pub edits: ChangeTracker,

    /// NEW. Mobs near the player when the save was written.
    pub mobs: Vec<save::MobSave>,
    /// NEW. Container contents, keyed by the block the container occupies.
    pub containers: std::collections::HashMap<(i32, i32, i32), save::ContainerSave>,
}
```

`save::VERSION` is now **2**. `save::MIN_VERSION` is **1**. A version 1 file — anything
written before today — still loads: it comes back with `mobs` empty and `containers`
empty, because that is exactly what it described. Nothing else about the old layout
moved, and the two new sections cost eight bytes when empty, so an untouched world saves
in 53 bytes instead of 45. Versions outside `1..=2` are still refused with
`SaveError::UnsupportedVersion` rather than misparsed.

Everything below is main-loop work. There are four call sites.

---

## 1. Mobs

### 1.1 Before writing a save

One line in `App::save_now`, next to the lines that sync the camera:

```rust
fn save_now(&mut self) {
    self.data.player.pos = self.player.pos;
    self.data.player.yaw = self.camera.yaw;
    self.data.player.pitch = self.camera.pitch;

    // NEW: snapshot the mobs.
    self.data.capture_mobs(self.mobs.mobs(), self.player.pos);
    self.sync_containers();          // section 2.1

    match save::save_to_file(&self.save_path, &self.data) { /* unchanged */ }
}
```

`capture_mobs` filters for you: it keeps only mobs that are alive and within
`save::MOB_SAVE_RADIUS` of the player. That constant **is** `mob::DESPAWN_DISTANCE`
(128.0), so it saves exactly the set the mob system has not already thrown away. Do not
pass the camera position — pass the player's feet, the same thing `PlayerState::new` gets,
or the radius check is off by the eye height.

A `MobSave` holds kind, position, velocity, yaw, health and the on-ground flag. It
deliberately does **not** hold pathfinding state, patience timers, chase memory, or a
creeper's lit fuse. A reloaded mob starts idle and re-acquires the player the ordinary
way. That is a design decision, not an omission: restoring a half-burnt fuse would let a
creeper detonate on the loading screen.

### 1.2 After loading — and where in the startup sequence

**Restore mobs only once the terrain around the player is resident.** Mobs run gravity and
collision against `World` on their very first tick. A world whose chunks have not
streamed in yet reports air everywhere, so a mob restored too early falls out of the
world before the ground arrives underneath it.

`App::update` already has the exact hook — the branch that fires when the initial
streaming settles:

```rust
if self.loading && self.world.is_idle() {
    self.loading = false;

    // NEW: the ground exists now, so the mobs can stand on it.
    let restored = std::mem::take(&mut self.data.mobs);
    for m in &restored {
        m.spawn_into(&mut self.mobs);
    }
    println!("[loudstone] restored {} mobs", restored.len());

    // ... the existing "world ready" logging ...
}
```

`std::mem::take` (the same idiom `replay_chunk` already uses two blocks above) means the
list is consumed, so the restore cannot run twice however that branch is reached. The list
is rebuilt from the live manager at the next `capture_mobs`, so taking it loses nothing.

`MobSave::spawn_into(&mut MobManager) -> u32` returns the mob's new id. Ids are **not**
persisted — nothing outside a single run refers to one — so the manager hands out fresh
ones starting at 1.

### 1.3 The one thing added to `mob.rs`

`MobManager::spawn` can only place a mob at full health, facing +X, standing still, so
there was no way to bring a wounded zombie back as a wounded zombie. One additive method,
next to `spawn`, with no change to anything that existed:

```rust
pub fn spawn_restored(
    &mut self, kind: MobKind, pos: Vec3, vel: Vec3, yaw: f32, health: f32, on_ground: bool,
) -> u32
```

It is `spawn` plus four field assignments. `health` is clamped to the kind's maximum. You
do not need to call it directly; `MobSave::spawn_into` is the wrapper.

---

## 2. Containers

The section is generic on purpose. A record is *a container at position P, of kind T,
holding these slots and these numeric fields* — **not** "a furnace". When chests arrive
they are the same record with 27 slots and no fields, and the file format does not change.

```rust
save::ContainerKind(pub u16)          // FURNACE = 1, CHEST = 2 (reserved)
save::ContainerSave { kind, slots: Vec<Option<ItemStack>>, fields: Vec<f32> }
```

A furnace is `ContainerKind::FURNACE` with three slots and three fields. Index them with
the constants — never with bare numbers:

| Constant | |
|---|---|
| `save::FURNACE_INPUT` (0) | the ore |
| `save::FURNACE_FUEL` (1) | the coal |
| `save::FURNACE_OUTPUT` (2) | the ingots |
| `save::FURNACE_BURN_LEFT` (0) | seconds of burn left in the lit fuel |
| `save::FURNACE_BURN_TOTAL` (1) | what that fuel was worth when lit |
| `save::FURNACE_PROGRESS` (2) | seconds of smelting banked on the input |

`ContainerSave` has `slot(i) -> Option<ItemStack>`, `field(i) -> f32`, and the three named
shortcuts `burn_left()`, `burn_total()`, `progress()`.

### 2.1 Before writing a save

```rust
fn sync_containers(&mut self) {
    self.data.containers.clear();
    for (&pos, f) in &self.furnaces {
        self.data.set_container(
            pos,
            save::ContainerSave::furnace(
                f.input,
                f.fuel,
                f.output,
                f.burn_left(),     // <-- see section 4, these three do not exist yet
                f.burn_total(),
                f.progress(),
            ),
        );
    }
}
```

`SaveData::set_container` drops a container that holds nothing and is doing nothing, so an
empty furnace the player opened once costs zero bytes. Clearing first means a furnace the
player mined out stops being written.

### 2.2 After loading

Containers need no world and no streaming, so restore them in `App::new`, right after the
`SaveData` is in hand and before the struct literal:

```rust
let mut furnaces: HashMap<(i32, i32, i32), crafting::Furnace> = HashMap::new();
for (&pos, c) in &data.containers {
    if c.kind != save::ContainerKind::FURNACE {
        continue;          // a kind this build does not know; leave it be
    }
    furnaces.insert(
        pos,
        crafting::Furnace::restore(          // <-- see section 4
            c.slot(save::FURNACE_INPUT),
            c.slot(save::FURNACE_FUEL),
            c.slot(save::FURNACE_OUTPUT),
            c.burn_left(),
            c.burn_total(),
            c.progress(),
        ),
    );
}
```

then `furnaces,` instead of `furnaces: HashMap::new(),` in the `Self { .. }` below.

Unrecognised container kinds load rather than erroring, so a save written by a later build
with dispensers in it still opens here. Skipping them on load and then re-saving would
lose them, but the version check refuses such a file long before that matters.

### 2.3 One existing bug this exposes

`App` inserts a furnace with `self.furnaces.entry((bx, by, bz)).or_default()` when the
player right-clicks one, but nothing removes the entry when the block is mined out. Today
that is a small leak; once furnaces are persisted it becomes a save that grows forever and
resurrects the contents of furnaces that no longer exist. Add
`self.furnaces.remove(&(bx, by, bz));` wherever a block is broken.

---

## 3. Order of operations, end to end

| When | Do |
|---|---|
| `App::new`, after `load_from_file` | build the `furnaces` map from `data.containers` (2.2) |
| `App::new` | `World::new(data.seed)` — unchanged |
| Each chunk becomes resident | `edits.replay_chunk(...)` — unchanged |
| `self.loading && world.is_idle()` | restore mobs from `data.mobs` (1.2) — **after** the ground exists |
| `save_now()` | sync player, then `capture_mobs`, then `sync_containers`, then `save_to_file` |

---

## 4. The one thing I could not do, and the four lines that fix it

**`crafting::Furnace` keeps `burn_left`, `burn_total` and `progress` private, with no
getter and no way to set them.** `input`, `fuel` and `output` are public; the three
timings are not. I do not own `crafting.rs` — another agent is making it data-driven — so
per contract rule 1 this is a report, not a change.

The save format carries the three numbers correctly and a test proves it round-trips a
real mid-smelt furnace's gauges exactly. Only the two lines of glue in `main.rs` cannot be
written yet. Add this to `impl Furnace` in `crafting.rs`, and sections 2.1 and 2.2 compile
as written:

```rust
    /// Seconds of burn left in the fuel currently alight.
    pub fn burn_left(&self) -> f32 { self.burn_left }
    /// What that fuel item was worth when it was lit.
    pub fn burn_total(&self) -> f32 { self.burn_total }
    /// Seconds of smelting banked on the current input.
    pub fn progress(&self) -> f32 { self.progress }

    /// Rebuild a furnace from a save, timings and all.
    pub fn restore(
        input: Option<ItemStack>,
        fuel: Option<ItemStack>,
        output: Option<ItemStack>,
        burn_left: f32,
        burn_total: f32,
        progress: f32,
    ) -> Self {
        Self { input, fuel, output, burn_left, burn_total, progress }
    }
```

### 4.1 Interim version, if `crafting.rs` cannot be touched yet

Slots alone fix the reported bug — *put iron ore and coal in a furnace, quit, come back,
your ore is gone*. Write zeros for the three timings:

```rust
// saving
save::ContainerSave::furnace(f.input, f.fuel, f.output, 0.0, 0.0, 0.0)

// loading
let mut f = crafting::Furnace::new();
f.input  = c.slot(save::FURNACE_INPUT);
f.fuel   = c.slot(save::FURNACE_FUEL);
f.output = c.slot(save::FURNACE_OUTPUT);
furnaces.insert(pos, f);
```

A furnace restored this way relights itself on the next `tick`, because `Furnace::tick`
lights fuel whenever `burn_left` has run out and there is work to do. The cost is one
partly-burnt fuel item and up to ten seconds of smelting per furnace per reload. The ore
and the coal — the things the player actually loses today — are safe either way. When the
four methods above land, switch to the full version in 2.1 and 2.2; the file format does
not change, and old saves keep loading because the fields were always written.

---

## 5. What the tests cover

In `save.rs`, all passing:

- `mobs_round_trip_exactly` — four mobs, one of each kind, every field bit-identical.
- `a_reloaded_mob_comes_back_where_it_was_and_starts_idle` — through a real `MobManager`:
  position, velocity, facing and health restored; state `Idle`; `fuse_fraction() == 0`.
- `only_mobs_near_the_player_are_saved` — the despawn-radius filter, and that
  `MOB_SAVE_RADIUS == mob::DESPAWN_DISTANCE`.
- `containers_round_trip_exactly` — a busy furnace, a fuel-only furnace and a 27-slot
  chest, with empty slots keeping their indices.
- `a_furnace_saved_mid_smelt_reloads_with_its_burn_and_progress` — a genuine
  `crafting::Furnace` ticked six seconds into a smelt; the reloaded record reproduces
  `burn_fraction()` and `progress_fraction()` exactly.
- `a_version_1_file_still_loads_with_no_mobs_and_no_containers` — a hand-assembled version
  1 file, byte by byte, loaded by today's code.
- `loading_a_version_1_file_and_saving_it_upgrades_it_losslessly` — and the upgraded file
  is exactly eight bytes larger.
- `a_truncated_version_1_file_errors_instead_of_panicking`,
  `truncation_at_every_length_errors_instead_of_panicking`, `garbage_never_panics` —
  every prefix of both formats errors; random bytes never panic.
- `an_unknown_mob_kind_is_rejected_rather_than_guessed`,
  `impossible_container_data_is_rejected` — corrupt records give a named `SaveError`.
- `an_untouched_world_still_saves_in_a_few_dozen_bytes` — 53 bytes, and the last eight are
  the two empty section counts.
