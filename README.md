# Loudstone

A voxel survival game in Rust. Mine, craft, build, fight, survive the night —
with two mechanics wired into each other:

**Blocks are 8×8×8 grids of sub-voxels.** Mining chips material away a grain at a
time, so walls end up with real craters and half-eaten faces instead of blocks
popping out whole.

**Mining makes noise, and mobs hear it.** Sound floods through open air and is
heavily damped by rock. Chipping is slow and quiet; smashing a whole block out is
fast and loud. Every mining decision is a bet on speed against safety — and if you
stand still and stay quiet, mobs lose you, because they walk to *where the sound
was*, not to where you are.

## Playing it

Double-click **Loudstone** on the Desktop, or `play.bat` in this folder. The first
run compiles the game (about a minute); after that it starts immediately.

| Key | |
|---|---|
| **W A S D** | move |
| **Space** | jump |
| **Left Ctrl** | sprint |
| **Mouse** | look |
| **Left click** | chip — slow, quiet, carves sub-voxels |
| **Alt + left click** | smash — fast, LOUD, takes the whole block |
| **Right click** | place a block, or open a crafting table / furnace |
| **1**–**9**, mouse wheel | select hotbar slot |
| **E** | inventory (2×2 crafting) |
| **F2** | save a screenshot to `shots/` |
| **F** | noclip, for debugging |
| **Esc** | release the mouse |

Tools are 3×3 recipes, so you need a **crafting table** for anything beyond planks,
sticks and torches. Iron ore needs a stone pickaxe or better; diamond needs iron.
Raw iron becomes an ingot in a **furnace**, with coal as fuel.

The world autosaves every two minutes and on exit, to `saves/world.lsw`. Only blocks
you actually changed are stored, so an untouched world saves in about 50 bytes.

## Building and testing

```bash
cargo run --release        # play
cargo test                 # 204 tests
```

Two flags exist so the rendered output can be checked without a human at the
keyboard — they render one settled frame, write a PNG, and exit:

```bash
cargo run --release -- --shot shots/x.png            # spawn view
cargo run --release -- --demo --shot shots/x.png     # crater + one of each mob
cargo run --release -- --ui table --shot shots/x.png # a panel, stocked
```

## Layout

| File | |
|---|---|
| `config.rs` | every tunable number — look, feel, physics, mining rates |
| `world.rs` | chunk map, threaded streaming, raycast, carving |
| `mesh.rs` | face culling, ambient occlusion, sub-voxel geometry |
| `worldgen.rs` | seeded terrain, caves, depth-weighted ores |
| `sound.rs` | noise propagation — the tuning block is at the top |
| `mob.rs` | four mob kinds and their AI |
| `pathfind.rs` | A* over the voxel grid |
| `item.rs` `inventory.rs` `crafting.rs` | items, tiers, recipes, smelting |
| `save.rs` | binary saves; only player edits are written |
| `hud.rs` | overlay, with a hand-coded 5×7 bitmap font |
| `screenshot.rs` | dependency-free PNG writer |

Visuals are deliberately not the point: flat per-block colours with ambient
occlusion, no textures, no asset files anywhere.
