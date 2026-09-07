# Loudstone — build specification

Build a native voxel survival game in Rust. Work through the phases in order, straight
through, without stopping for review. Each phase must end in a **runnable build** that
satisfies its acceptance criteria before you move to the next.

## Identity

Minecraft's core loop — mine, craft, build, fight, survive the night — with two mechanics
that fuse into one tension:

1. **Sub-voxel destruction.** Every block is internally an 8x8x8 grid of sub-voxels.
   Mining chips material away progressively; walls end up with real craters and partial
   faces rather than popping out whole.
2. **Sound-driven mob AI.** Digging emits noise. Noise floods through open air and is
   damped by solid rock. Hostile mobs hear it and converge on the source.

These two are the game: **chipping is slow and quiet, smashing a whole block is fast and
loud.** Every mining action is a speed-versus-safety bet. Neither mechanic is decoration —
if the player cannot feel that trade-off, the build has failed.

## Non-goals

- Visuals are explicitly **not** the priority. Flat per-block colors plus ambient occlusion.
  No textures, no external art assets, no model files, no asset pipeline.
- No multiplayer, no networking.
- No hunger, farming, armor, redstone, or alternate dimensions.
- Do not add features not listed here. Scope discipline is a hard requirement.

## Stack

- Rust 2021, `winit` 0.30, `wgpu` 30, WGSL shaders, `glam`, `noise`, `rayon`, `bytemuck`.
- No game engine. Own the render loop, the chunk meshing, and the storage layout directly.
- Target: RTX 3050 Laptop (4 GB VRAM), Ryzen 9 5900HX (8c/16t), 15.4 GB RAM. 60 fps.

## World parameters

| Parameter | Value |
|---|---|
| Chunk | 16 x 16 x 16 blocks |
| World height | 256 blocks (16 chunks vertical) |
| Horizontal extent | Infinite, streamed |
| Render distance | 12 chunks (~192 blocks) |
| Sub-voxel resolution | 8^3 per block |

## Critical constraint: sparse sub-voxel storage

8^3 = 512 sub-voxels per block. Dense storage is a 512x memory blowup and **will** kill
this build. Undamaged blocks store **nothing** — they are implicitly full. Only a damaged
block allocates, as a 64-byte bitmask (512 bits, one per sub-voxel), held in a per-chunk
`HashMap<u16, [u8; 64]>` keyed by index within the chunk. A block whose mask reaches all
zeros is removed entirely and becomes air; a block whose mask returns to full drops its
entry. Chunk meshing renders sub-voxel geometry only for blocks present in that map.

## Phases

### Phase 1 — Window, renderer, camera
winit window, wgpu surface, depth buffer, WGSL pipeline. Free-fly camera: WASD, mouse-look
with pointer capture, Space/Shift for up/down, Esc releases the cursor. Render one
hardcoded chunk of solid colored cubes.
**Accept:** window opens, cubes render with correct depth, camera flies smoothly, resize works.

### Phase 2 — World generation and streaming
Seeded terrain from layered simplex noise (continental shape + hills + a 3D-noise cave
carve). Ore veins for coal, iron, gold, diamond, depth-weighted. Chunks generate and mesh
on `rayon` worker threads and upload on the main thread. Chunks load and unload against
the 12-chunk radius as the camera moves. Face culling between adjacent solid blocks, plus
per-vertex ambient occlusion, plus frustum culling.
**Accept:** fly in any direction indefinitely with no hitching; caves and ore veins visible;
no cracks or missing faces at chunk seams.

### Phase 3 — Interaction and sub-voxel destruction
DDA voxel raycast from the camera, max 6 blocks. Highlight the targeted block. Left-click
held chips sub-voxels away in a small sphere at the hit point; the block breaks when its
mask empties. Right-click places a block against the hit face. A separate fast full-block
break (hold and release, or a modifier) removes the whole block at once. Player physics:
AABB collision, gravity, jumping, step-up, and collision against partially-chipped blocks.
**Accept:** carve a visible crater into a wall without destroying the block; walk on and
collide with partially-carved geometry; place blocks accurately.

### Phase 4 — Survival systems
~10 block types. Hotbar (9 slots) + inventory. ~12 recipes: planks, sticks, crafting table,
wood/stone/iron pickaxes/axes/swords, torches, furnace. Tool tiers gate mining speed and
what drops. Health, fall damage, respawn at spawn point on death with full inventory drop.
Day/night cycle driving sky color and light level. Save/load to a region file format under
the save directory; autosave on a timer and on exit.
**Accept:** start from nothing, craft a stone pickaxe, mine iron, place a torch, die, respawn,
quit, relaunch, and find the world exactly as left.

### Phase 5 — Mobs and sound propagation
Noise event queue: every mining action emits an event with a position and a loudness
(chipping quiet, full-block smash loud). Loudness floods outward through air with distance
falloff and a heavy per-block damping through solids, capped at a small radius budget.
Hostile mobs (zombie, skeleton, creeper analogues) spawn in darkness, path toward heard
noise, and attack in melee; one passive mob (pig analogue) wanders and drops meat.
A* or greedy pathing over the voxel grid with jump/step handling.
**Accept:** stand still and mobs lose track of you; chip quietly and few arrive; smash blocks
loudly and they converge from off-screen. The trade-off must be legible in play.

## Engineering standards

- Deterministic worldgen from a single seed.
- Meshing and generation must never block the render thread.
- Structure block behaviour behind a registry, and worldgen/mob AI behind traits, so
  systems can be extended without a rewrite.
- Expose tunable constants (noise scales, mob spawn rates, noise falloff, mining speeds)
  in one clearly marked config module rather than scattered as magic numbers.
- Prefer clear, readable code over cleverness. Comment the non-obvious parts only.
