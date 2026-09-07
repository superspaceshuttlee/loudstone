//! Two-channel voxel lighting: block light from emitters, sky light from open sky.
//!
//! The model is Minecraft 1.12's, minus the dimensions that do not exist here.
//! Every block carries two 4-bit levels in the range 0..=15:
//!
//!  * **Block light** comes from emitters (a torch is 14) and falls off by one
//!    per block travelled in any direction.
//!  * **Sky light** enters a column at 15 wherever the sky is open and falls
//!    *straight down at full strength* through transparent blocks, so a shaft
//!    is bright all the way to its floor. Sideways it decays like block light.
//!
//! Sky light is scaled by the time of day at the point it is turned into a
//! brightness; block light is not, which is the whole reason a torch is worth
//! placing. The two are combined with `max`, never summed.
//!
//! ## Where the work happens
//!
//! Initial lighting of a freshly generated chunk is done by [`seed_chunk`] on
//! the rayon worker that generated it, from the terrain heightmap rather than a
//! flood fill from the world's ceiling. That is the difference between lighting
//! being free during streaming and lighting doubling the load time.
//!
//! Everything after that is incremental: [`update_for_block_change`] runs a
//! bounded remove-then-refill flood around one edited block. The remove pass is
//! the part that is easy to get wrong -- a propagator that only ever *adds*
//! light leaves stale bright patches behind a wall you just built, so removal
//! walks the region the old source was responsible for, zeroes it, and re-seeds
//! from the independent sources it met at the edge.
//!
//! ## Sub-voxel carving
//!
//! A partially carved block **still blocks light completely**. Only destroying
//! the block outright (its mask emptying, so the block becomes air) opens it up.
//! This is deliberate: it keeps the propagator's opacity test a pure function of
//! the block id, so it never has to consult the sparse damage map, and it means
//! chipping -- which happens many times a second while mining -- costs no
//! lighting work at all. The mesher agrees with it: light for a carved block's
//! faces is sampled from its neighbours, never from the block itself.

use crate::config::{
    CHUNK_SIZE, CHUNK_SIZE_I, LIGHT_AMBIENT, LIGHT_GAMMA, MAX_LIGHT, NIGHT_SKY_SUBTRACT,
    TORCH_LIGHT,
};
use crate::content::block::BlockId;
use crate::world::chunk::Chunk;
use crate::world::worldgen::TerrainGen;
use std::collections::VecDeque;

/// The six face directions, in the order the flood fill visits them.
pub const DIRS: [(i32, i32, i32); 6] = [
    (0, 1, 0),
    (0, -1, 0),
    (1, 0, 0),
    (-1, 0, 0),
    (0, 0, 1),
    (0, 0, -1),
];

// ---------------------------------------------------------------------------
// Per-block light properties
//
// These two tables belong on `BlockId` in `block.rs`. They live here because
// `block.rs` was owned by another agent when lighting was written; moving them
// is a mechanical change and is called out in the integration note.
// ---------------------------------------------------------------------------

/// Block light a block emits by itself, 0..=15.
#[inline]
pub fn emission(id: BlockId) -> u8 {
    match id {
        BlockId::TORCH => TORCH_LIGHT,
        _ => 0,
    }
}

/// How much light a block eats. 0 is perfectly clear, `MAX_LIGHT` stops it dead.
///
/// Anything that occludes a face also stops light, so a block type added later
/// blocks light correctly without being listed here.
#[inline]
pub fn opacity(id: BlockId) -> u8 {
    if id.is_opaque() {
        return MAX_LIGHT;
    }
    if id.is_leaves() {
        // A canopy shades what is under it without blacking it out.
        return 1;
    }
    match id {
        // Water dims quickly with depth.
        BlockId::WATER => 3,
        // Torches and the ground-cover plants are perfectly clear.
        _ => 0,
    }
}

/// True when light cannot pass through this block at all.
#[inline]
pub fn blocks_light(id: BlockId) -> bool {
    opacity(id) >= MAX_LIGHT
}

// ---------------------------------------------------------------------------
// Packing: one byte per block, sky in the high nibble, block light in the low.
// ---------------------------------------------------------------------------

#[inline]
pub fn pack(block: u8, sky: u8) -> u8 {
    (sky << 4) | (block & 0x0F)
}

#[inline]
pub fn block_of(packed: u8) -> u8 {
    packed & 0x0F
}

#[inline]
pub fn sky_of(packed: u8) -> u8 {
    packed >> 4
}

// ---------------------------------------------------------------------------
// Levels to brightness
// ---------------------------------------------------------------------------

/// How much of the sky channel the current time of day removes, 0 at noon and
/// `NIGHT_SKY_SUBTRACT` at midnight. Integer, exactly as Minecraft does it: it
/// gives the day cycle a small number of discrete steps, which is what makes
/// baking light into vertices affordable.
#[inline]
pub fn sky_subtract(daylight: f32) -> u8 {
    let d = 1.0 - daylight.clamp(0.0, 1.0);
    (d * NIGHT_SKY_SUBTRACT as f32).round() as u8
}

/// A 0..=15 light level as a brightness multiplier. Level 15 is 1.0; level 0 is
/// a dim floor rather than pure black, so an unlit cave still reads as a shape.
pub fn brightness(level: f32) -> f32 {
    let t = (level / MAX_LIGHT as f32).clamp(0.0, 1.0);
    LIGHT_AMBIENT + (1.0 - LIGHT_AMBIENT) * t.powf(LIGHT_GAMMA)
}

/// Combine the two channels for rendering. `subtract` comes from [`sky_subtract`].
/// Fractional levels are allowed so the mesher can feed it a smoothed average.
#[inline]
pub fn combine(block: f32, sky: f32, subtract: f32) -> f32 {
    block.max(sky - subtract)
}

/// The brightness multiplier a surface with these smoothed levels should get.
#[inline]
pub fn vertex_light(block: f32, sky: f32, subtract: f32) -> f32 {
    brightness(combine(block, sky, subtract))
}

/// Integer effective light, for gameplay queries such as mob spawning.
#[inline]
pub fn effective_level(block: u8, sky: u8, daylight: f32) -> u8 {
    block.max(sky.saturating_sub(sky_subtract(daylight)))
}

// ---------------------------------------------------------------------------
// Initial lighting of one freshly generated chunk
// ---------------------------------------------------------------------------

/// Sky light a position has when its chunk is not resident.
///
/// The mesher's skirt and `World`'s fallbacks both use this, and [`seed_chunk`]
/// produces exactly the same answer for a column that no cave interferes with.
/// They have to agree or every chunk edge would show a light seam while the
/// world streams in.
#[inline]
pub fn fallback_sky(surface: i32, y: i32) -> u8 {
    if y > surface { MAX_LIGHT } else { 0 }
}

/// Light a chunk from scratch, using only the chunk itself and the generator.
///
/// Sky light is seeded per column from the terrain height rather than flooded
/// from the top of the world, then spread sideways within the chunk. Light that
/// has to cross a chunk boundary is picked up later by the world's border merge.
pub fn seed_chunk(chunk: &mut Chunk, terrain: &TerrainGen) {
    chunk.lit = true;
    // Solid rock end to end: no sky reaches it and it holds no emitters.
    if chunk.is_uniform_opaque() {
        chunk.fill_light(0);
        return;
    }

    let (_, oy, _) = chunk.pos.origin();
    let heights = terrain.column_heights(chunk.pos.x, chunk.pos.z);
    let max_h = heights.iter().copied().max().unwrap_or(0);

    // Entirely above ground: open sky everywhere, and no flood fill needed
    // because every cell already holds the maximum.
    if chunk.is_empty() && oy > max_h {
        chunk.fill_light(pack(0, MAX_LIGHT));
        return;
    }
    chunk.fill_light(0);

    // --- sky: one downward sweep per column ---
    let top = oy + CHUNK_SIZE_I - 1;
    let mut sky_seeds: VecDeque<(u8, u8, u8)> = VecDeque::new();
    for z in 0..CHUNK_SIZE {
        for x in 0..CHUNK_SIZE {
            let surface = heights[z * CHUNK_SIZE + x];
            // Nothing in this column can be lit from above at all.
            if top <= surface && !column_has_air(chunk, x, z) {
                continue;
            }
            let mut incoming = fallback_sky(surface, top);
            for y in (0..CHUNK_SIZE).rev() {
                let op = opacity(chunk.get(x, y, z));
                let level = if op >= MAX_LIGHT {
                    0
                } else if incoming == MAX_LIGHT && op == 0 {
                    // Straight down through clear air keeps full strength.
                    MAX_LIGHT
                } else {
                    incoming.saturating_sub(op.max(1))
                };
                if level > 0 {
                    chunk.set_sky_light(x, y, z, level);
                    if level > 1 {
                        sky_seeds.push_back((x as u8, y as u8, z as u8));
                    }
                }
                incoming = level;
            }
        }
    }
    spread_within(chunk, sky_seeds, Channel::Sky);

    // --- block light: only chunks that actually contain an emitter pay ---
    let mut block_seeds: VecDeque<(u8, u8, u8)> = VecDeque::new();
    for z in 0..CHUNK_SIZE {
        for y in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                let e = emission(chunk.get(x, y, z));
                if e > 0 {
                    chunk.set_block_light(x, y, z, e);
                    block_seeds.push_back((x as u8, y as u8, z as u8));
                }
            }
        }
    }
    if !block_seeds.is_empty() {
        spread_within(chunk, block_seeds, Channel::Block);
    }
}

/// Whether a column of the chunk holds any block light can pass through. Used to
/// skip the sweep for columns buried in rock.
fn column_has_air(chunk: &Chunk, x: usize, z: usize) -> bool {
    (0..CHUNK_SIZE).any(|y| !blocks_light(chunk.get(x, y, z)))
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Channel {
    Block,
    Sky,
}

/// Flood fill confined to one chunk, in chunk-local coordinates. Used only for
/// the initial seeding, where there is nothing outside the chunk to spread to.
fn spread_within(chunk: &mut Chunk, mut q: VecDeque<(u8, u8, u8)>, ch: Channel) {
    let n = CHUNK_SIZE_I;
    while let Some((x, y, z)) = q.pop_front() {
        let (x, y, z) = (x as i32, y as i32, z as i32);
        let level = match ch {
            Channel::Block => chunk.block_light(x as usize, y as usize, z as usize),
            Channel::Sky => chunk.sky_light(x as usize, y as usize, z as usize),
        };
        if level <= 1 {
            continue;
        }
        for (dx, dy, dz) in DIRS {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if nx < 0 || ny < 0 || nz < 0 || nx >= n || ny >= n || nz >= n {
                continue;
            }
            let (ux, uy, uz) = (nx as usize, ny as usize, nz as usize);
            let op = opacity(chunk.get(ux, uy, uz));
            if op >= MAX_LIGHT {
                continue;
            }
            let target = if ch == Channel::Sky && dy == -1 && level == MAX_LIGHT && op == 0 {
                MAX_LIGHT
            } else {
                level.saturating_sub(op.max(1))
            };
            if target == 0 {
                continue;
            }
            let cur = match ch {
                Channel::Block => chunk.block_light(ux, uy, uz),
                Channel::Sky => chunk.sky_light(ux, uy, uz),
            };
            if cur >= target {
                continue;
            }
            match ch {
                Channel::Block => chunk.set_block_light(ux, uy, uz, target),
                Channel::Sky => chunk.set_sky_light(ux, uy, uz, target),
            }
            q.push_back((nx as u8, ny as u8, nz as u8));
        }
    }
}

// ---------------------------------------------------------------------------
// World-level propagation
// ---------------------------------------------------------------------------

/// The slice of the world a flood fill is allowed to touch.
///
/// `World` implements this. Meshing never does -- a mesh worker gets a frozen
/// `Neighborhood` snapshot and only ever reads light out of it.
pub trait LightVolume {
    /// Reads are allowed anywhere: outside the loaded region they answer from
    /// terrain generation, which is what keeps light continuous at the edge of
    /// what has streamed in.
    fn opacity_at(&self, x: i32, y: i32, z: i32) -> u8;
    fn emission_at(&self, x: i32, y: i32, z: i32) -> u8;
    fn block_light(&self, x: i32, y: i32, z: i32) -> u8;
    fn sky_light(&self, x: i32, y: i32, z: i32) -> u8;

    /// Writes are allowed only where a chunk is actually resident.
    fn writable(&self, x: i32, y: i32, z: i32) -> bool;
    fn set_block_light(&mut self, x: i32, y: i32, z: i32, level: u8);
    fn set_sky_light(&mut self, x: i32, y: i32, z: i32, level: u8);

    /// Note that this block's light changed, so its chunk (and, on a boundary,
    /// its neighbour) is remeshed.
    fn touch(&mut self, x: i32, y: i32, z: i32);
}

type Cell = (i32, i32, i32);

#[inline]
fn step(c: Cell, d: (i32, i32, i32)) -> Cell {
    (c.0 + d.0, c.1 + d.1, c.2 + d.2)
}

/// What level a neighbour receives from a cell holding `level`.
#[inline]
fn transmit(level: u8, op: u8, dy: i32, sky: bool) -> u8 {
    if op >= MAX_LIGHT {
        return 0;
    }
    if sky && dy == -1 && level == MAX_LIGHT && op == 0 {
        MAX_LIGHT
    } else {
        level.saturating_sub(op.max(1))
    }
}

/// Push light outward from every seed until nothing gets brighter. Only ever
/// raises a level, so it is safe to call with a mixed bag of seeds.
pub fn spread<V: LightVolume + ?Sized>(v: &mut V, mut q: VecDeque<Cell>, sky: bool) {
    while let Some(c) = q.pop_front() {
        let level = if sky {
            v.sky_light(c.0, c.1, c.2)
        } else {
            v.block_light(c.0, c.1, c.2)
        };
        if level <= 1 {
            continue;
        }
        for d in DIRS {
            let n = step(c, d);
            if !v.writable(n.0, n.1, n.2) {
                continue;
            }
            let op = v.opacity_at(n.0, n.1, n.2);
            let target = transmit(level, op, d.1, sky);
            if target == 0 {
                continue;
            }
            let cur = if sky {
                v.sky_light(n.0, n.1, n.2)
            } else {
                v.block_light(n.0, n.1, n.2)
            };
            if cur >= target {
                continue;
            }
            if sky {
                v.set_sky_light(n.0, n.1, n.2, target);
            } else {
                v.set_block_light(n.0, n.1, n.2, target);
            }
            v.touch(n.0, n.1, n.2);
            q.push_back(n);
        }
    }
}

/// Tear down the light one cell was responsible for, then refill from whatever
/// independent sources the teardown ran into.
///
/// This is the half that a naive implementation skips, and skipping it is what
/// leaves a lit ghost of a torch you removed, or a bright patch behind a wall
/// you just built.
pub fn remove<V: LightVolume + ?Sized>(v: &mut V, origin: Cell, level: u8, sky: bool) {
    if level == 0 {
        return;
    }
    let mut dark: VecDeque<(Cell, u8)> = VecDeque::new();
    let mut refill: VecDeque<Cell> = VecDeque::new();
    let mut emitters: Vec<Cell> = Vec::new();

    if sky {
        v.set_sky_light(origin.0, origin.1, origin.2, 0);
    } else {
        v.set_block_light(origin.0, origin.1, origin.2, 0);
    }
    v.touch(origin.0, origin.1, origin.2);
    dark.push_back((origin, level));

    while let Some((c, lvl)) = dark.pop_front() {
        for d in DIRS {
            let n = step(c, d);
            if !v.writable(n.0, n.1, n.2) {
                continue;
            }
            let nl = if sky {
                v.sky_light(n.0, n.1, n.2)
            } else {
                v.block_light(n.0, n.1, n.2)
            };
            if nl == 0 {
                continue;
            }
            // A cell we lit is strictly dimmer than we were -- except straight
            // down in the sky channel, where a full-strength column carries 15
            // the whole way and would otherwise look like an independent source.
            let ours = nl < lvl || (sky && d.1 == -1 && lvl == MAX_LIGHT && nl == MAX_LIGHT);
            if ours {
                if sky {
                    v.set_sky_light(n.0, n.1, n.2, 0);
                } else {
                    v.set_block_light(n.0, n.1, n.2, 0);
                    if v.emission_at(n.0, n.1, n.2) > 0 {
                        emitters.push(n);
                    }
                }
                v.touch(n.0, n.1, n.2);
                dark.push_back((n, nl));
            } else {
                refill.push_back(n);
            }
        }
    }

    // An emitter inside the torn-down region is its own source again.
    for e in emitters {
        let level = v.emission_at(e.0, e.1, e.2);
        v.set_block_light(e.0, e.1, e.2, level);
        v.touch(e.0, e.1, e.2);
        refill.push_back(e);
    }
    spread(v, refill, sky);
}

/// Bring both channels back into agreement after the block at `(x, y, z)`
/// changed identity. Safe to call for a placement, a break, or a swap.
pub fn update_for_block_change<V: LightVolume + ?Sized>(v: &mut V, x: i32, y: i32, z: i32) {
    if !v.writable(x, y, z) {
        return;
    }
    let solid = v.opacity_at(x, y, z) >= MAX_LIGHT;

    // --- block light ---
    let had = v.block_light(x, y, z);
    if had > 0 {
        remove(v, (x, y, z), had, false);
    }
    let mut q: VecDeque<Cell> = VecDeque::new();
    let emit = v.emission_at(x, y, z);
    if emit > 0 {
        v.set_block_light(x, y, z, emit);
        v.touch(x, y, z);
        q.push_back((x, y, z));
    }
    if !solid {
        // The cell may have just opened up, so let the neighbours flow in.
        for d in DIRS {
            q.push_back(step((x, y, z), d));
        }
    }
    spread(v, q, false);

    // --- sky light ---
    let had = v.sky_light(x, y, z);
    if had > 0 {
        remove(v, (x, y, z), had, true);
    }
    if !solid {
        let mut q: VecDeque<Cell> = VecDeque::new();
        for d in DIRS {
            q.push_back(step((x, y, z), d));
        }
        spread(v, q, true);
    }
    v.touch(x, y, z);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WORLD_HEIGHT;
    use crate::world::chunk::{ChunkPos, local_index};
    use std::collections::HashMap;

    /// A dense little test world: every cell is writable, nothing streams.
    struct Grid {
        blocks: HashMap<Cell, BlockId>,
        block_light: HashMap<Cell, u8>,
        sky_light: HashMap<Cell, u8>,
        touched: Vec<Cell>,
        bound: i32,
    }

    impl Grid {
        fn new(bound: i32) -> Self {
            Self {
                blocks: HashMap::new(),
                block_light: HashMap::new(),
                sky_light: HashMap::new(),
                touched: Vec::new(),
                bound,
            }
        }
        fn set(&mut self, c: Cell, id: BlockId) {
            self.blocks.insert(c, id);
        }
    }

    impl LightVolume for Grid {
        fn opacity_at(&self, x: i32, y: i32, z: i32) -> u8 {
            opacity(*self.blocks.get(&(x, y, z)).unwrap_or(&BlockId::AIR))
        }
        fn emission_at(&self, x: i32, y: i32, z: i32) -> u8 {
            emission(*self.blocks.get(&(x, y, z)).unwrap_or(&BlockId::AIR))
        }
        fn block_light(&self, x: i32, y: i32, z: i32) -> u8 {
            *self.block_light.get(&(x, y, z)).unwrap_or(&0)
        }
        fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
            *self.sky_light.get(&(x, y, z)).unwrap_or(&0)
        }
        fn writable(&self, x: i32, y: i32, z: i32) -> bool {
            x.abs() <= self.bound && y.abs() <= self.bound && z.abs() <= self.bound
        }
        fn set_block_light(&mut self, x: i32, y: i32, z: i32, level: u8) {
            self.block_light.insert((x, y, z), level);
        }
        fn set_sky_light(&mut self, x: i32, y: i32, z: i32, level: u8) {
            self.sky_light.insert((x, y, z), level);
        }
        fn touch(&mut self, x: i32, y: i32, z: i32) {
            self.touched.push((x, y, z));
        }
    }

    #[test]
    fn torch_emission_is_fourteen_and_air_emits_nothing() {
        assert_eq!(emission(BlockId::TORCH), 14);
        assert_eq!(emission(BlockId::AIR), 0);
        assert_eq!(emission(BlockId::STONE), 0);
    }

    #[test]
    fn opacity_follows_face_occlusion_with_water_and_leaves_as_exceptions() {
        assert_eq!(opacity(BlockId::AIR), 0);
        assert_eq!(opacity(BlockId::TORCH), 0);
        assert_eq!(opacity(BlockId::STONE), MAX_LIGHT);
        assert_eq!(opacity(BlockId::DIRT), MAX_LIGHT);
        assert!(opacity(BlockId::LEAVES) > 0 && opacity(BlockId::LEAVES) < MAX_LIGHT);
        assert!(opacity(BlockId::WATER) > 0 && opacity(BlockId::WATER) < MAX_LIGHT);
    }

    #[test]
    fn packing_round_trips_both_nibbles() {
        for b in 0..=15u8 {
            for s in 0..=15u8 {
                let p = pack(b, s);
                assert_eq!(block_of(p), b, "block nibble lost for {b},{s}");
                assert_eq!(sky_of(p), s, "sky nibble lost for {b},{s}");
            }
        }
    }

    /// The headline propagation rule.
    #[test]
    fn block_light_decays_by_one_per_block() {
        let mut g = Grid::new(30);
        g.set((0, 0, 0), BlockId::TORCH);
        update_for_block_change(&mut g, 0, 0, 0);
        assert_eq!(g.block_light(0, 0, 0), 14);
        for d in 1..=14 {
            assert_eq!(g.block_light(d, 0, 0), 14 - d as u8, "at distance {d}");
        }
        assert_eq!(
            g.block_light(15, 0, 0),
            0,
            "light must die out at 14 blocks"
        );
        // Diagonals decay by manhattan distance, as a flood fill should.
        assert_eq!(g.block_light(3, 2, 1), 14 - 6);
    }

    #[test]
    fn an_opaque_block_stops_light() {
        let mut g = Grid::new(30);
        // A stone pane at x = 1 with a torch behind it at x = 0.
        for y in -3..=3 {
            for z in -3..=3 {
                g.set((1, y, z), BlockId::STONE);
            }
        }
        g.set((0, 0, 0), BlockId::TORCH);
        update_for_block_change(&mut g, 0, 0, 0);
        assert_eq!(g.block_light(1, 0, 0), 0, "light must not enter the wall");
        // Straight through is blocked; it can only arrive by going around.
        assert!(
            g.block_light(2, 0, 0) < 13,
            "light leaked straight through the wall"
        );
    }

    /// The de-lighting case. A naive re-flood leaves the torch's glow behind.
    #[test]
    fn removing_a_torch_clears_every_trace_of_its_light() {
        let mut g = Grid::new(30);
        g.set((0, 0, 0), BlockId::TORCH);
        update_for_block_change(&mut g, 0, 0, 0);
        assert!(g.block_light(5, 0, 0) > 0);

        g.set((0, 0, 0), BlockId::AIR);
        update_for_block_change(&mut g, 0, 0, 0);
        for x in -20..=20 {
            for y in -6..=6 {
                for z in -6..=6 {
                    assert_eq!(
                        g.block_light(x, y, z),
                        0,
                        "stale light left at {x},{y},{z} after the torch was removed"
                    );
                }
            }
        }
    }

    #[test]
    fn two_torches_survive_one_of_them_being_removed() {
        let mut g = Grid::new(40);
        g.set((0, 0, 0), BlockId::TORCH);
        update_for_block_change(&mut g, 0, 0, 0);
        g.set((6, 0, 0), BlockId::TORCH);
        update_for_block_change(&mut g, 6, 0, 0);
        assert_eq!(g.block_light(6, 0, 0), 14);
        assert_eq!(
            g.block_light(3, 0, 0),
            11,
            "the nearer torch wins the middle"
        );

        g.set((0, 0, 0), BlockId::AIR);
        update_for_block_change(&mut g, 0, 0, 0);
        assert_eq!(g.block_light(6, 0, 0), 14, "the survivor must stay lit");
        assert_eq!(
            g.block_light(0, 0, 0),
            8,
            "and must relight what it can reach"
        );
        assert_eq!(g.block_light(-7, 0, 0), 1);
        assert_eq!(g.block_light(-8, 0, 0), 0);
    }

    #[test]
    fn sky_light_falls_at_full_strength_and_decays_sideways() {
        let mut g = Grid::new(40);
        // An open column: seed the top cell the way the world does.
        g.set_sky_light(0, 20, 0, MAX_LIGHT);
        let mut q = VecDeque::new();
        q.push_back((0, 20, 0));
        spread(&mut g, q, true);
        for y in 0..=20 {
            assert_eq!(
                g.sky_light(0, y, 0),
                15,
                "a shaft must stay bright at y={y}"
            );
        }
        // Sideways it decays like any other light.
        assert_eq!(g.sky_light(4, 10, 0), 11);
    }

    #[test]
    fn placing_a_block_darkens_what_is_behind_it() {
        let mut g = Grid::new(30);
        g.set((0, 0, 0), BlockId::TORCH);
        update_for_block_change(&mut g, 0, 0, 0);
        assert_eq!(g.block_light(4, 0, 0), 10);

        // Seal the torch inside a stone shell.
        for (dx, dy, dz) in DIRS {
            g.set((dx, dy, dz), BlockId::STONE);
            update_for_block_change(&mut g, dx, dy, dz);
        }
        for x in 1..=20 {
            assert_eq!(
                g.block_light(x, 0, 0),
                0,
                "light survived at x={x} behind a sealed wall"
            );
        }
        assert_eq!(g.block_light(0, 0, 0), 14, "the torch itself still burns");
    }

    #[test]
    fn day_subtract_runs_from_zero_at_noon_to_the_night_value() {
        assert_eq!(sky_subtract(1.0), 0);
        assert_eq!(sky_subtract(0.0), NIGHT_SKY_SUBTRACT);
        assert!(sky_subtract(0.5) > 0 && sky_subtract(0.5) < NIGHT_SKY_SUBTRACT);
        // Block light never dims with the clock.
        assert_eq!(effective_level(14, 0, 0.0), 14);
        assert_eq!(effective_level(14, 0, 1.0), 14);
        // Open sky does.
        assert_eq!(effective_level(0, 15, 1.0), 15);
        assert_eq!(effective_level(0, 15, 0.0), 15 - NIGHT_SKY_SUBTRACT);
    }

    #[test]
    fn brightness_spans_the_floor_to_one() {
        assert!((brightness(15.0) - 1.0).abs() < 1e-5);
        assert!(brightness(0.0) > 0.0 && brightness(0.0) < 0.2);
        for l in 1..=15 {
            assert!(
                brightness(l as f32) > brightness(l as f32 - 1.0),
                "brightness must be monotonic at level {l}"
            );
        }
    }

    #[test]
    fn seeding_a_surface_chunk_lights_the_sky_and_leaves_rock_dark() {
        let terrain = TerrainGen::new(1337);
        let h = terrain.height_at(0, 0);
        let cy = h.div_euclid(CHUNK_SIZE_I);
        let pos = ChunkPos::new(0, cy, 0);
        let mut c = terrain.generate(pos);
        seed_chunk(&mut c, &terrain);

        let ly = h.rem_euclid(CHUNK_SIZE_I) as usize;
        // The surface block itself is opaque, so it holds no sky light...
        assert_eq!(c.sky_light(0, ly, 0), 0);
        // ...but the air directly above it sees the whole sky, if it is in this
        // chunk at all.
        if ly + 1 < CHUNK_SIZE {
            assert_eq!(c.sky_light(0, ly + 1, 0), MAX_LIGHT);
        }
    }

    #[test]
    fn seeding_a_deep_chunk_leaves_it_pitch_dark() {
        let terrain = TerrainGen::new(1337);
        let pos = ChunkPos::new(0, 1, 0); // y 16..32, far below any surface
        let mut c = terrain.generate(pos);
        seed_chunk(&mut c, &terrain);
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    assert_eq!(
                        c.sky_light(x, y, z),
                        0,
                        "sky light reached {x},{y},{z} of a chunk 200 blocks underground"
                    );
                }
            }
        }
    }

    #[test]
    fn seeding_a_sky_chunk_is_uniformly_bright() {
        let terrain = TerrainGen::new(1337);
        let pos = ChunkPos::new(0, (WORLD_HEIGHT / CHUNK_SIZE_I) - 1, 0);
        let mut c = terrain.generate(pos);
        assert!(c.is_empty());
        seed_chunk(&mut c, &terrain);
        assert_eq!(c.sky_light(3, 3, 3), MAX_LIGHT);
        assert_eq!(c.block_light(3, 3, 3), 0);
    }

    #[test]
    fn a_torch_in_a_chunk_is_lit_by_seeding() {
        let terrain = TerrainGen::new(1337);
        let pos = ChunkPos::new(0, 1, 0);
        let mut c = terrain.generate(pos);
        c.set(8, 8, 8, BlockId::AIR);
        c.set(8, 8, 8, BlockId::TORCH);
        // Clear a little room around it so there is somewhere for light to go.
        for d in 1..=3 {
            c.set(8 + d, 8, 8, BlockId::AIR);
        }
        seed_chunk(&mut c, &terrain);
        assert_eq!(c.block_light(8, 8, 8), TORCH_LIGHT);
        assert_eq!(c.block_light(9, 8, 8), TORCH_LIGHT - 1);
        assert_eq!(c.block_light(11, 8, 8), TORCH_LIGHT - 3);
    }

    #[test]
    fn local_index_agrees_with_the_light_array() {
        // Guards against the light array and the block array disagreeing on
        // layout, which would light the wrong blocks in a way that is very hard
        // to see in a screenshot.
        let mut c = Chunk::new(ChunkPos::new(0, 0, 0));
        c.set_block_light(1, 2, 3, 9);
        assert_eq!(c.block_light(1, 2, 3), 9);
        assert_eq!(c.light_data()[local_index(1, 2, 3) as usize], pack(9, 0));
        assert_eq!(c.block_light(3, 2, 1), 0);
    }
}
