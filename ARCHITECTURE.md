# Loudstone — architecture and working notes

Written so somebody (or some model) who has never seen this repo can change it
safely. Read the **Hard rules** section before touching anything; every entry in
it is a bug that has already been paid for once.

---

## 0. If you are an AI assistant reading this without the repo

This document describes a codebase living at `C:\Users\aavig\dev\loudstone` on
the owner's machine. Repository access depends on the environment in which this
file is being read. Inspect the checkout and its configured remotes instead of
assuming that source access or a GitHub remote is unavailable.

Three ways to work, in increasing order of usefulness:

1. **The owner pastes in the file(s) for the task.** Best for a focused change —
   "make night darker" needs `config.rs` and `light.rs`, nothing else. Ask for
   exactly what you need.
2. **The owner grants access to the repository.** An agentic coding tool
   connected to it can then read, edit, and run the project directly.
3. **The owner runs an agentic CLI locally** in that directory, which already has
   filesystem access.

Whatever you do, verify with `cargo test` and, for anything visual,
`cargo run --release -- --shot shots/x.png` and actually look at the PNG.

### Current state, as of the last commit

Playable and complete for its scope: biomes, water, trees, caves, ores, lighting
with a day/night cycle, mining, crafting, smelting, tool tiers, four mob kinds
with sound-driven AI, saves, generated textures, and synthesised audio.

The content registry in `src/registry.rs`, backed by `assets/data/*.ron`, is the
source of truth for block and item properties, recipes, and smelting rules. Rust
keeps the stable numeric ID constants required by save compatibility and match
patterns.

Mob and furnace state crosses the runtime/save boundary in `main.rs`: live state
is captured before saves and restored when the application starts. The generic
container records remain forward-compatible with future container kinds.

---

## 1. What the game is

A voxel survival game in Rust: mine, craft, build, fight, survive the night.
Roughly Minecraft 1.12 in scope, deliberately excluding the Nether and the End.

Two mechanics define it, and they are wired into each other on purpose:

**Sub-voxel destruction.** Every block is internally an 8x8x8 grid of sub-voxels.
Mining chips material away progressively; explosions carve real craters. A block
is gone only when its last sub-voxel is.

**Sound-driven mob AI.** Mining emits noise. Noise floods outward through open
air and is heavily damped by rock. Mobs walk toward **where the sound was**, not
toward the player. Chipping is slow and quiet; smashing a whole block is fast and
loud. Every mining decision is therefore a bet on speed against safety, and
standing still genuinely loses your pursuers.

If a change makes those two stop mattering, the change is wrong even if it
compiles and the tests pass.

---

## 2. Getting around

```bash
cargo run --release            # play
cargo test                     # all tests must pass
cargo clippy --all-targets --all-features
```

Diagnostics that let you see the game without a human at the keyboard — use
these constantly, they are the difference between reasoning about the renderer
and knowing what it drew:

```bash
cargo run --release -- --shot shots/a.png             # render one settled frame, exit
cargo run --release -- --demo --shot shots/b.png      # + a carved crater and one of each mob
cargo run --release -- --ui table --shot shots/c.png  # + a stocked crafting panel
cargo run --release -- --gauntlet --seed 3 --secs 90  # randomised playtest
```

`--shot` writes a real PNG (there is a dependency-free encoder in
`screenshot.rs`). **Open it and look.** Several of the worst bugs in this
project's history were invisible to tests and obvious in a screenshot.

`play.bat` and a Desktop shortcut launch the release build for the owner.

---

## 3. Module map

| File | Responsibility |
|---|---|
| `main.rs` | Window, event loop, `App` state, and the wiring between systems |
| `cli.rs` | Command-line options, parsed once into one `Cli` value |
| `session.rs` | `Hands` (what the player is doing) and `Stats` (counters) |
| `ui.rs` | Menu and panel layout, drawing, and the stack-moving rules |
| `daylight.rs` | The day/night cycle: brightness, sky colour, sun direction |
| `config.rs` | **Every tunable number.** Look/feel, physics, mining rates, streaming budgets |
| `world.rs` | Chunk map, threaded streaming, raycast, carving, block queries |
| `chunk.rs` | Chunk storage and the sparse sub-voxel damage masks |
| `worldgen.rs` | Seeded terrain: biomes, caves, ores, trees, water |
| `mesh.rs` | Chunk meshing: face culling, ambient occlusion, sub-voxel geometry, UVs |
| `light.rs` | Block and sky light, flood propagation and de-lighting |
| `gfx.rs` | wgpu device, terrain pipeline, atlas upload, highlight, entity geometry |
| `texture.rs` | The generated texture atlas and every block/item/mob tile |
| `registry.rs` / `assets/data/*.ron` | Validated block, item, recipe, and smelting data |
| `shader.wgsl` / `hud.wgsl` | Terrain and overlay shaders |
| `hud.rs` | Overlay: crosshair, hotbar, health, panels, and a hand-coded 5x7 font |
| `camera.rs` | Camera, player body, physics, collision, step-up |
| `block.rs` `item.rs` `inventory.rs` `crafting.rs` | Content definitions and rules |
| `save.rs` | Binary save format and the durable edit log |
| `sound.rs` | Noise **propagation model** for mob AI. Makes no audible sound |
| `audio.rs` | What the player actually hears. Synthesised, no sample files |
| `mob.rs` `pathfind.rs` | Mob entities, AI state machine, A* over voxels |
| `gauntlet.rs` | The randomised property-based playtest harness |
| `screenshot.rs` | Dependency-free PNG writer |

`sound.rs` and `audio.rs` are easy to confuse and are unrelated. `sound.rs` is
simulation; `audio.rs` is output.

---

## 4. Hard rules

Each of these was a real bug. Breaking one costs a day.

**Block numeric ids are frozen.** Save files store raw block numbers and
`texture.rs` maps ids to atlas tiles. Append new blocks; never renumber.

**Every world mutation needs its `note_*` twin.** `world.set_block` pairs with
`data.edits.note_set_block`, `world.carve` with `note_carve`. The `ChangeTracker`
is the durable record of player edits; an unnoted edit vanishes the moment its
chunk streams out. Direct player actions record their matching edit, while mob
mutations pass through `TrackedWorld`, which records every carve and block set.

**Nothing touching wgpu leaves the main thread.** Generation and meshing run on
rayon and report over channels; the main thread does every GPU call.

**Never allocate GPU buffers per frame.** Mob geometry is rebuilt every frame and
the selection box every time the crosshair moves. Creating a buffer for each one
exhausts the device within ~90 seconds, after which every allocation returns an
invalid buffer and panics on map. Use the growable `DynBuffer` in `gfx.rs`.

**Same-type faces between non-opaque blocks must stay culled.** Leaves, water and
plants do not occlude their neighbours, but two of the *same* non-opaque block
share an invisible interior face. Emitting those meshed every leaf inside every
tree canopy: 16.1M triangles, 904 MB of vertex data, an 8.4 s world load. With
the cull it is 1.5M triangles, 83 MB, 0.85 s. **Do not remove this.**

**Mesh vertices are in world space.** The renderer applies no per-chunk
transform. Emitting chunk-local coordinates stacks the entire world inside one
16³ box at the origin.

**Faces wind counter-clockwise seen from outside.** Clockwise side faces get
deleted by back-face culling, leaving horizontal plates with gaps between them.

**Sub-voxel storage stays sparse.** An undamaged block stores nothing and is
implicitly full; only chipped blocks allocate a 64-byte mask. Dense storage is a
512x memory blowup.

**Do not mesh a chunk before its neighbours are resident.** `Neighborhood::build`
falls back to regenerating terrain for a missing skirt, then the mesh is thrown
away when the neighbour arrives. `World::neighbors_ready` gates this.

**Call `world.set_daylight` every frame.** Sky light is baked into chunk meshes.
Without it, night changes only the sky colour and the ground stays lit as if at
noon.

**Mining is precise; explosions are messy.** `chip_block` confines the carve to
one block and anchors it to the nearest surviving material, and mining sticks to
the block the swing started on. `chip_sphere` (unconfined, spans blocks) is for
explosions only. Using the unconfined version for mining erodes a mushy bowl
across a wall and blocks pop in an order the player did not choose.

**`hud.rs` text `size` is a pixel height, not a scale factor.** `TEXT_SIZE` is
14.0. Passing 1.4 renders text about one pixel tall.

---

## 5. How the pieces move

**Streaming.** `World::stream(center)` is called once per frame. The wanted set
is rebuilt only when the player crosses a chunk boundary, then drained
incrementally from a `VecDeque` — there is no per-frame allocate-and-sort of the
world. Generation and meshing dispatch to rayon; finished meshes come back over
channels and the main thread uploads them within a budget. Player edits jump the
queue.

**Edits and saves.** A player edit mutates the live chunk *and* appends to
`ChangeTracker`. When a chunk streams back in it is regenerated from the seed and
the tracker replays its edits over it. The save file therefore contains only what
the player changed — an untouched world saves in about 50 bytes.

**Noise to mobs.** Mining pushes `NoiseEvent`s onto the world. Each frame
`main.rs` drains them into the `SoundField`, which floods loudness through the
voxel grid with a visit budget. Mobs query the loudest thing audible at their own
position and path toward its **origin**. Chase uses line of sight and decays;
investigation uses the remembered sound position. That split is what makes
staying still work.

**Light.** Two 4-bit channels per block, block light and sky light, packed into
one byte. Propagation is a BFS flood; removing a source de-lights first and then
re-propagates, or stale bright patches survive behind walls. Light is baked into
vertex light at mesh time, so a change of daylight step queues a background
remesh.

---

## 6. Where the knobs are

- `config.rs` — physics, mining rate and radius, reach, render distance, fog, AO,
  face shading, streaming budgets, combat.
- Tuning block at the top of `sound.rs` — air and solid attenuation, decay,
  hearing threshold, visit budget.
- Tuning block at the top of `audio.rs` — master volume, per-sound gains,
  cooldowns, attenuation distance, voice cap.
- `worldgen.rs::tuning` — sea level, biome scales, cave thresholds, ore rates.
- `light.rs` — emission table and the brightness curve.

The owner dials look and feel by hand. When a change is about how something
*feels*, expose the number rather than guessing at it.

---

## 7. Testing

Three layers, and they catch different things.

**Unit tests.** Pure logic: recipes, inventory maths, save round-trips,
pathfinding, sound falloff, meshing invariants (a sealed chunk meshes to zero
vertices; an isolated block meshes to exactly six quads).

**Screenshots.** `--shot` and friends. Tests cannot see that the world renders
inside-out or that night never darkens.

**The gauntlet (`gauntlet.rs`).** A randomised property-based soak test. A
weighted policy plays for minutes, teleporting between distant columns; oracles
assert properties every frame; a coverage tracker fails any session that
exercised too little to prove anything. Every run prints its seed so a failure
replays with `--gauntlet --seed N`.

The oracles are the valuable part: the player is never non-finite, never outside
the world, never buried; items never appear without mining or crafting; stacks
stay legal; streaming always settles; light stays in 0..15; a save round-trip
reproduces the world. **When you add a system, add its invariant here** — that is
what stops the harness from becoming a fixed script that only finds what its
author already imagined.

---

## 8. Conventions

- **All art and audio are generated in code.** No downloaded assets, and none of
  Minecraft's. Textures are original 16x16 pixel art built from seeded noise and
  hand-placed pixels in `texture.rs`; sounds are synthesised in `audio.rs`.
- Comments explain *why*, especially where the obvious approach is wrong.
- Edition 2024: `gen` is a reserved keyword and cannot be an identifier.
- Windows/MSVC toolchain; `wgpu` 30, `winit` 0.30, `glam` 0.33, `cpal`, `rayon`.

---

## 9. Known gaps

In roughly the order they would pay off:

1. **Storage** — chests. The container save section is designed to take them.
2. **Animals and breeding** — passive mobs exist; husbandry does not.
3. **Food and farming** — deliberately out of scope so far; no hunger.
4. **Water you can swim in, and buckets.** Water renders but is inert.
5. **Armour.**
6. **Structures and villages.**
7. **Redstone.**
8. **Plants render as cubes.** `is_cross` exists on `BlockId` and the mesher does
   not yet emit crossed quads, so flowers are solid slabs.
9. **Fixed-timestep simulation.** Physics runs on frame delta, so the simulation
   is not identical across machines. Blocks replays and any future multiplayer.
10. **Shipping shell** — settings menu, keybind remapping, graphics options, CI,
    installer, exe icon, crash reporting.

Structurally, `main.rs` still holds an `App` with 39 fields. That is down from
58: launch flags became `cli.rs`, the panel layer became `ui.rs`, the day cycle
became `daylight.rs`, and the loose interaction timers and counters became
`Hands` and `Stats` in `session.rs`.

That refactor was not cosmetic, and the reason is worth keeping in mind for the
next one. `mining`, `placing` and `mining_target` were three independent-looking
booleans, but they are not independent -- releasing the button must clear the
committed target, or the next swing quietly resumes on a block the player has
walked away from. Two of the four places that released the hands got that wrong,
because nothing in a flat struct says the three fields belong together. Moving
them into `Hands` with a `stop()` gave the rule one home and fixed both sites.

What is left to group, in the order it will hurt:

- **Persistence** (`data`, `save_path`, `has_save`, `time_since_save`,
  `replayed`, `furnaces`) -- the boundary bugs found so far all lived here:
  furnace contents outliving their block, items lost on close. There is still no
  type that says "this is the set of things that must be saved together".
- **Panel state** (`ui`, `craft_grid`, `carried`, `cursor`, `cursor_locked`) --
  `carried` is a live item stack held outside the inventory, so any path that
  closes a panel without returning it destroys items.
- **Frame/loop** (`window`, `gfx`, `last_frame`, `last_frame_at`, `loading`,
  `load_frames`, `start`).

Beyond that, a real engine would use an ECS. That is a bigger decision than a
field regrouping and should wait until there is a second entity type that wants
components the mob system does not have.
