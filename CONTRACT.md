# Loudstone — frozen API contract

Several agents build this crate in parallel. **These signatures are frozen.** Code against
them exactly. Do not change, rename, or "improve" anything in this file. If you believe a
signature is wrong, write the problem into your integration note instead of changing it.

## Coordinates

- World-space block coordinates are `i32` triples `(x, y, z)`. `y` is up, range `0..256`.
- Chunks are 16x16x16. `ChunkPos` is in chunk space.
- Sub-voxel coordinates within a block are `usize` in `0..8` on each axis.
- Entity/player positions are `glam::Vec3` in block units (a block spans 1.0).

## `crate::block::BlockId`

```rust
pub struct BlockId(pub u8);

impl BlockId {
    pub const AIR: BlockId;         // 0
    pub const STONE: BlockId;       // 1
    pub const DIRT: BlockId;        // 2
    pub const GRASS: BlockId;       // 3
    pub const SAND: BlockId;        // 4
    pub const WOOD: BlockId;        // 5
    pub const LEAVES: BlockId;      // 6
    pub const PLANKS: BlockId;      // 7
    pub const COBBLESTONE: BlockId; // 8
    pub const COAL_ORE: BlockId;    // 9
    pub const IRON_ORE: BlockId;    // 10
    pub const GOLD_ORE: BlockId;    // 11
    pub const DIAMOND_ORE: BlockId; // 12
    pub const BEDROCK: BlockId;     // 13
    pub const WATER: BlockId;       // 14

    pub fn is_air(self) -> bool;
    pub fn is_opaque(self) -> bool;
    pub fn is_solid(self) -> bool;   // player collides with it
    pub fn color(self) -> [f32; 3];
    pub fn hardness(self) -> f32;    // higher is slower to mine; BEDROCK is INFINITY
}
```

New block ids may be appended (TORCH, CRAFTING_TABLE, FURNACE) by the agent that owns
`block.rs`. Do not renumber existing ones — save files depend on the numbering.

## `crate::world::World`

```rust
impl World {
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId;
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, id: BlockId);
    /// Clear one sub-voxel. Returns true if that emptied the block entirely.
    pub fn carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool;
    /// Ground height of a column.
    pub fn surface_y(&self, x: i32, z: i32) -> i32;
    /// Fraction of a block still solid, 1.0 when untouched.
    pub fn fill_ratio(&self, x: i32, y: i32, z: i32) -> f32;
    /// True when that sub-voxel is still solid. Untouched blocks report true everywhere.
    pub fn sub_solid(&self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool;
}
```

`fill_ratio` and `sub_solid` may not exist yet — the engine agent adds them. Code against
them as specified.

## Rules for every agent

1. **Stay in your lane.** Only create or edit the files listed as yours. Never edit a file
   owned by another agent, and never edit `main.rs` or `Cargo.toml` unless you own them.
2. **No new dependencies** unless your brief says otherwise. Available: `winit` 0.30,
   `wgpu` 30, `glam` 0.33, `noise` 0.9, `rayon` 1.12, `bytemuck` 1.25, `rand` 0.10,
   `pollster` 1.0. Edition 2024, so `gen` is a reserved keyword — do not use it as an
   identifier.
3. **Pure logic modules must not depend on wgpu or winit.** Gameplay code takes `&mut World`
   and plain data, and returns plain data. Rendering is somebody else's job.
4. **Leave the crate compiling.** `cargo check` must pass when you finish.
5. **Write unit tests** for anything with real logic (crafting resolution, sound falloff,
   pathfinding, save round-trip). `cargo test` must pass.
6. **Visuals are not the priority.** Flat colours. No textures, no external assets, no
   asset loading of any kind.
7. Finish by writing `INTEGRATION_<yourname>.md` at the repo root: the exact functions the
   main loop must call, in what order, with what arguments, and any state the `App` struct
   needs to hold. Assume the integrator has not read your code.
