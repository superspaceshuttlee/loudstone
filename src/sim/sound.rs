//! Sound propagation -- the game's defining mechanic.
//!
//! Mining emits noise. Noise floods outward through the voxel grid: cheap through
//! open air, heavily damped per solid block crossed, plus ordinary distance falloff.
//! Mobs listen to the resulting field and converge on what they hear.
//!
//! Chipping a block sub-voxel by sub-voxel is quiet (`LOUDNESS_CHIP`); smashing a
//! whole block out at once is loud (`LOUDNESS_SMASH`). That is the speed-versus-safety
//! bet the whole game hangs on, so the numbers below are the numbers that matter.
//!
//! Performance shape: a flood is computed **once** when an event is accepted, its
//! attenuation field is cached, and it is then merely *scaled down* every tick as the
//! sound decays. Floods are rate limited per tick and hard capped by a visit budget,
//! so a burst of mining can never stall the frame.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use glam::{IVec3, Vec3};

use crate::content::block::BlockId;

// =============================================================================
// ============================  TUNING BLOCK  =================================
//   Everything dialable about sound lives here. Nothing below this block is a
//   magic number. Turn these knobs, not the code.
// =============================================================================

/// Loudness of one sub-voxel chip. Quiet: slow mining, few visitors.
pub const LOUDNESS_CHIP: f32 = 0.25;
/// Loudness of a full-block smash. Loud: fast mining, the neighbourhood hears it.
pub const LOUDNESS_SMASH: f32 = 1.0;
/// Loudness of a creeper explosion. Wakes up the whole cave system.
pub const LOUDNESS_EXPLOSION: f32 = 3.0;

/// Attenuation charged for crossing one block of open air. Low: air is cheap.
pub const AIR_ATTENUATION_PER_BLOCK: f32 = 0.02;
/// Extra attenuation charged for crossing one *fully solid* block. High: rock kills sound.
/// Scaled by `fill_ratio`, so a half-carved block damps half as much -- your own
/// tunnel is a sound highway, which is the point.
pub const SOLID_ATTENUATION_PER_BLOCK: f32 = 0.85;
/// Effective density of non-solid but non-air blocks (water). Between the two above.
pub const FLUID_DENSITY: f32 = 0.35;

/// Distance at which plain geometric falloff has halved the sound, in blocks.
/// Falloff is `1 / (1 + (d / this)^2)`.
pub const DISTANCE_HALF_POWER: f32 = 8.0;

/// Below this audibility a mob hears nothing at all. The single most important
/// number in the file: it sets how far a chip carries versus a smash.
pub const HEARING_THRESHOLD: f32 = 0.07;

/// Hard cap on lattice nodes visited by one flood. Bounds worst-case frame cost.
pub const VISIT_BUDGET: usize = 4096;
/// Flood lattice spacing, in blocks. 2 means we store every 2nd voxel (8x fewer
/// nodes) while still charging attenuation for *every* block crossed, so a
/// one-block wall is never missed. 1 is exact and 8x more expensive.
pub const FLOOD_STRIDE: i32 = 2;

/// How fast a sound fades from memory, per second (exponential).
pub const DECAY_RATE: f32 = 0.9;
/// A decayed sound is dropped once its current strength falls under this.
pub const RETIRE_STRENGTH: f32 = 0.02;

/// Max simultaneous remembered sounds. Beyond this the weakest is evicted.
pub const MAX_ACTIVE_SOUNDS: usize = 8;
/// Max floods computed per tick. A burst of events queues instead of stalling.
pub const MAX_FLOODS_PER_TICK: usize = 2;
/// Max queued events awaiting a flood. Beyond this the quietest are dropped.
pub const MAX_PENDING_EVENTS: usize = 16;
/// New events within this many blocks of a remembered sound refresh it instead of
/// starting a second flood. Keeps continuous chipping to one flood.
pub const MERGE_RADIUS: f32 = 2.5;

// =============================================================================
// ==========================  END TUNING BLOCK  ===============================
// =============================================================================

/// The narrow slice of the world that sound, pathfinding and mob AI need.
///
/// The integrator implements this for `crate::world::World` in a few lines; every
/// method here already exists on `World` with the same signature (see CONTRACT.md).
pub trait VoxelWorld {
    fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId;
    /// True when that sub-voxel is still solid. Untouched blocks report true everywhere.
    fn sub_solid(&self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool;
    /// Fraction of a block still solid, 1.0 when untouched, 0.0 for air.
    fn fill_ratio(&self, x: i32, y: i32, z: i32) -> f32;
    /// Clear one sub-voxel. Returns true if that emptied the block entirely.
    fn carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool;
    /// Replace a whole block. An explosion uses this for blocks it fully consumes,
    /// which is 512x cheaper than carving every sub-voxel individually.
    fn set_block(&mut self, x: i32, y: i32, z: i32, id: BlockId);
}

/// One noise emission. Mining pushes these; anything else that should be heard can too.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NoiseEvent {
    pub pos: Vec3,
    pub loudness: f32,
}

impl NoiseEvent {
    pub fn chip(pos: Vec3) -> Self {
        Self {
            pos,
            loudness: LOUDNESS_CHIP,
        }
    }
    pub fn smash(pos: Vec3) -> Self {
        Self {
            pos,
            loudness: LOUDNESS_SMASH,
        }
    }
    pub fn explosion(pos: Vec3) -> Self {
        Self {
            pos,
            loudness: LOUDNESS_EXPLOSION,
        }
    }
}

/// What a listener currently hears: how loud, and -- crucially -- from *where*.
///
/// Mobs investigate `pos`. That is the position of the *sound*, never of the player.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Heard {
    pub pos: Vec3,
    pub loudness: f32,
}

/// Plain geometric falloff, independent of what the sound had to travel through.
#[inline]
fn distance_falloff(d: f32) -> f32 {
    let k = d / DISTANCE_HALF_POWER;
    1.0 / (1.0 + k * k)
}

/// `distance_falloff` expressed in the same additive units as travel attenuation,
/// so the two can be summed into one "how quiet is it here" number.
#[inline]
fn distance_penalty(d: f32) -> f32 {
    let k = d / DISTANCE_HALF_POWER;
    (1.0 + k * k).ln()
}

/// Attenuation charged for entering block `(x, y, z)`.
fn block_attenuation<W: VoxelWorld + ?Sized>(world: &W, x: i32, y: i32, z: i32) -> f32 {
    let b = world.block_at(x, y, z);
    if b.is_air() {
        return AIR_ATTENUATION_PER_BLOCK;
    }
    if !b.is_solid() {
        return AIR_ATTENUATION_PER_BLOCK + SOLID_ATTENUATION_PER_BLOCK * FLUID_DENSITY;
    }
    let fill = world.fill_ratio(x, y, z).clamp(0.0, 1.0);
    AIR_ATTENUATION_PER_BLOCK + SOLID_ATTENUATION_PER_BLOCK * fill
}

/// Min-heap entry.
///
/// Ordered by `key`, which is travel attenuation *plus* distance penalty -- i.e. by
/// how quiet the node actually is, not merely by how much rock the sound crossed.
/// That ordering is what makes the visit budget useful: without it the flood spends
/// every node it has on far-away open air before it ever pushes through a wall, and
/// a mob standing three blocks behind a wall would hear nothing at all.
#[derive(Copy, Clone)]
struct Frontier {
    key: f32,
    /// Accumulated travel attenuation only (the value that gets stored).
    travel: f32,
    node: IVec3,
}
impl PartialEq for Frontier {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl Eq for Frontier {}
impl Ord for Frontier {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed: BinaryHeap is a max-heap and we want the quietest-loss node first.
        other.key.total_cmp(&self.key)
    }
}
impl PartialOrd for Frontier {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

const NEIGHBOURS: [IVec3; 6] = [
    IVec3::new(1, 0, 0),
    IVec3::new(-1, 0, 0),
    IVec3::new(0, 1, 0),
    IVec3::new(0, -1, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(0, 0, -1),
];

/// The cached attenuation field of one sound: the accumulated travel cost to every
/// lattice node the flood reached. Computed once; queried cheaply thereafter.
#[derive(Clone, Debug)]
pub struct Propagation {
    /// Block the sound started in. Lattice node `n` is block `origin_block + n * FLOOD_STRIDE`.
    origin_block: IVec3,
    origin: Vec3,
    /// Loudness the flood was pruned against. Kept for reporting only.
    source_loudness: f32,
    /// Lattice node -> accumulated attenuation cost.
    cost: HashMap<IVec3, f32>,
    /// Nodes actually expanded, for profiling and tests.
    pub visited: usize,
    /// True when the flood stopped because it hit `VISIT_BUDGET` rather than because
    /// everything further was inaudible.
    pub budget_exhausted: bool,
}

impl Propagation {
    /// Flood `loudness` outward from `origin` through `world`.
    ///
    /// Best-first (Dijkstra) over a `FLOOD_STRIDE` lattice, charging real per-block
    /// attenuation for every block crossed. Stops at `VISIT_BUDGET` nodes, or earlier
    /// once even the shortest remaining path is inaudible.
    pub fn flood<W: VoxelWorld + ?Sized>(world: &W, origin: Vec3, loudness: f32) -> Self {
        let loudness = if loudness.is_finite() {
            loudness.max(0.0)
        } else {
            0.0
        };
        let origin_block = origin.floor().as_ivec3();
        let mut cost: HashMap<IVec3, f32> = HashMap::new();
        let mut heap: BinaryHeap<Frontier> = BinaryHeap::new();

        cost.insert(IVec3::ZERO, 0.0);
        heap.push(Frontier {
            key: 0.0,
            travel: 0.0,
            node: IVec3::ZERO,
        });

        // A node is worth expanding only while the source could still be heard there.
        // `key` (travel + distance penalty) is exactly `-ln(audibility / loudness)`,
        // so the whole audibility test collapses to one comparison.
        let cutoff = HEARING_THRESHOLD / loudness;
        let max_key = if cutoff >= 1.0 { 0.0 } else { -cutoff.ln() };

        let mut visited = 0usize;
        let mut budget_exhausted = false;

        while let Some(Frontier { key, travel, node }) = heap.pop() {
            // Stale heap entry: a cheaper route to this node was already settled.
            if cost.get(&node).is_some_and(|&best| travel > best) {
                continue;
            }
            if key > max_key {
                break; // everything still queued is inaudible
            }
            if visited >= VISIT_BUDGET {
                budget_exhausted = true;
                break;
            }
            visited += 1;

            let here = origin_block + node * FLOOD_STRIDE;
            for dir in NEIGHBOURS {
                let next = node + dir;
                // Charge attenuation for every block actually crossed, not just the
                // lattice endpoint -- this is what keeps a 1-block wall from leaking.
                let mut step = 0.0;
                for s in 1..=FLOOD_STRIDE {
                    let p = here + dir * s;
                    step += block_attenuation(world, p.x, p.y, p.z);
                }
                let nt = travel + step;
                let centre = (origin_block + next * FLOOD_STRIDE).as_vec3() + Vec3::splat(0.5);
                let nk = nt + distance_penalty((centre - origin).length());
                if nk > max_key {
                    continue;
                }
                let better = match cost.get(&next) {
                    Some(&best) => nt < best,
                    None => true,
                };
                if better {
                    cost.insert(next, nt);
                    heap.push(Frontier {
                        key: nk,
                        travel: nt,
                        node: next,
                    });
                }
            }
        }

        Propagation {
            origin_block,
            origin,
            source_loudness: loudness,
            cost,
            visited,
            budget_exhausted,
        }
    }

    pub fn origin(&self) -> Vec3 {
        self.origin
    }

    pub fn source_loudness(&self) -> f32 {
        self.source_loudness
    }

    /// Fraction of the source loudness that survives the trip to `listener`, in `0..=1`.
    /// Returns 0 for listeners the flood never reached.
    pub fn transmission_at(&self, listener: Vec3) -> f32 {
        let Some(c) = self.cost_at(listener) else {
            return 0.0;
        };
        let d = (listener - self.origin).length();
        (-c).exp() * distance_falloff(d)
    }

    /// How loud this one sound is where the listener is standing.
    pub fn audibility_at(&self, listener: Vec3) -> f32 {
        self.audibility_scaled(listener, self.source_loudness)
    }

    /// As `audibility_at`, but for a source that has since decayed to `strength`.
    pub fn audibility_scaled(&self, listener: Vec3, strength: f32) -> f32 {
        strength * self.transmission_at(listener)
    }

    /// Travel cost to the lattice node nearest the listener, if the flood got there.
    ///
    /// Checks the eight lattice corners of the cell containing the listener and takes
    /// the geometrically nearest one that the flood actually reached, so a listener
    /// standing between nodes still hears something sensible.
    fn cost_at(&self, listener: Vec3) -> Option<f32> {
        let stride = FLOOD_STRIDE as f32;
        let rel = (listener.floor().as_ivec3() - self.origin_block).as_vec3() / stride;
        let base = rel.floor().as_ivec3();

        let mut best: Option<(f32, f32)> = None; // (distance^2 in lattice units, cost)
        for dx in 0..2 {
            for dy in 0..2 {
                for dz in 0..2 {
                    let n = base + IVec3::new(dx, dy, dz);
                    if let Some(&c) = self.cost.get(&n) {
                        let d2 = (n.as_vec3() - rel).length_squared();
                        if best.is_none_or(|(bd, _)| d2 < bd) {
                            best = Some((d2, c));
                        }
                    }
                }
            }
        }
        best.map(|(_, c)| c)
    }

    /// Nodes retained. Bounded by `VISIT_BUDGET` times the branching factor.
    pub fn node_count(&self) -> usize {
        self.cost.len()
    }
}

/// One remembered sound: its cached field plus how much of it is left.
#[derive(Clone, Debug)]
struct ActiveSound {
    field: Propagation,
    /// Loudness at emission, before decay.
    loudness: f32,
    /// Seconds since the sound was (re)triggered.
    age: f32,
}

impl ActiveSound {
    #[inline]
    fn strength(&self) -> f32 {
        self.loudness * (-DECAY_RATE * self.age).exp()
    }
}

/// Aggregates recent noise, decays it, and answers "what is the loudest thing this
/// mob can currently hear, and where is it?".
///
/// Lifecycle per tick: `emit` any number of events (cheap), then call `update` once
/// (bounded work), then query `loudest_at` / `audibility_at` per listener (cheap).
#[derive(Clone, Debug, Default)]
pub struct SoundField {
    active: Vec<ActiveSound>,
    pending: Vec<NoiseEvent>,
}

impl SoundField {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a noise event. Does no flooding -- safe to call from the mining code as
    /// often as you like. Nearby events merge into one another.
    pub fn emit(&mut self, ev: NoiseEvent) {
        if !(ev.loudness > 0.0) || !ev.pos.is_finite() {
            return;
        }
        // Refresh an existing sound at nearly the same place rather than re-flooding.
        for s in &mut self.active {
            if (s.field.origin() - ev.pos).length() <= MERGE_RADIUS {
                s.loudness = s.loudness.max(ev.loudness);
                s.age = 0.0;
                return;
            }
        }
        for p in &mut self.pending {
            if (p.pos - ev.pos).length() <= MERGE_RADIUS {
                p.loudness = p.loudness.max(ev.loudness);
                return;
            }
        }
        if self.pending.len() >= MAX_PENDING_EVENTS {
            // Drop the quietest queued event rather than growing without bound.
            let mut worst = 0usize;
            for (i, p) in self.pending.iter().enumerate() {
                if p.loudness < self.pending[worst].loudness {
                    worst = i;
                }
            }
            if self.pending[worst].loudness >= ev.loudness {
                return;
            }
            self.pending.swap_remove(worst);
        }
        self.pending.push(ev);
    }

    /// Convenience for the mining code.
    pub fn emit_chip(&mut self, pos: Vec3) {
        self.emit(NoiseEvent::chip(pos));
    }
    pub fn emit_smash(&mut self, pos: Vec3) {
        self.emit(NoiseEvent::smash(pos));
    }

    /// Age existing sounds and flood at most `MAX_FLOODS_PER_TICK` queued events.
    /// Call exactly once per tick, before mob AI.
    pub fn update<W: VoxelWorld + ?Sized>(&mut self, world: &W, dt: f32) {
        for s in &mut self.active {
            s.age += dt;
        }
        self.active.retain(|s| s.strength() >= RETIRE_STRENGTH);

        let floods = MAX_FLOODS_PER_TICK.min(self.pending.len());
        for _ in 0..floods {
            // Loudest queued event first -- it is the one the player cares about.
            let mut best = 0usize;
            for (i, p) in self.pending.iter().enumerate() {
                if p.loudness > self.pending[best].loudness {
                    best = i;
                }
            }
            let ev = self.pending.swap_remove(best);
            let field = Propagation::flood(world, ev.pos, ev.loudness);
            self.push_active(ActiveSound {
                field,
                loudness: ev.loudness,
                age: 0.0,
            });
        }
    }

    fn push_active(&mut self, s: ActiveSound) {
        if self.active.len() >= MAX_ACTIVE_SOUNDS {
            let mut worst = 0usize;
            for i in 1..self.active.len() {
                if self.active[i].strength() < self.active[worst].strength() {
                    worst = i;
                }
            }
            if self.active[worst].strength() >= s.strength() {
                return;
            }
            self.active.swap_remove(worst);
        }
        self.active.push(s);
    }

    /// Flood a single event immediately, bypassing the queue. Use for one-shot loud
    /// events (an explosion) that must be heard on the very tick they happen.
    pub fn emit_immediate<W: VoxelWorld + ?Sized>(&mut self, world: &W, ev: NoiseEvent) {
        if !(ev.loudness > 0.0) || !ev.pos.is_finite() {
            return;
        }
        for s in &mut self.active {
            if (s.field.origin() - ev.pos).length() <= MERGE_RADIUS {
                s.loudness = s.loudness.max(ev.loudness);
                s.age = 0.0;
                return;
            }
        }
        let field = Propagation::flood(world, ev.pos, ev.loudness);
        self.push_active(ActiveSound {
            field,
            loudness: ev.loudness,
            age: 0.0,
        });
    }

    /// Loudness of the single loudest audible sound at `listener`, or 0.
    pub fn audibility_at(&self, listener: Vec3) -> f32 {
        self.loudest_at(listener).map_or(0.0, |h| h.loudness)
    }

    /// The loudest thing this listener can currently hear, and where it came from.
    ///
    /// `None` when nothing clears `HEARING_THRESHOLD`. The returned `pos` is the
    /// *sound's* origin -- mobs must investigate that, never the player's live position.
    pub fn loudest_at(&self, listener: Vec3) -> Option<Heard> {
        self.loudest_above(listener, HEARING_THRESHOLD)
    }

    /// As `loudest_at` with a caller-supplied threshold, so a keen-eared mob kind can
    /// hear below the global floor and a deaf one above it.
    pub fn loudest_above(&self, listener: Vec3, threshold: f32) -> Option<Heard> {
        let mut best: Option<Heard> = None;
        for s in &self.active {
            let a = s.field.audibility_scaled(listener, s.strength());
            if a < threshold {
                continue;
            }
            if best.is_none_or(|b| a > b.loudness) {
                best = Some(Heard {
                    pos: s.field.origin(),
                    loudness: a,
                });
            }
        }
        best
    }

    /// Live sound sources and their current strength -- for debug HUDs.
    pub fn sources(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.active.iter().map(|s| (s.field.origin(), s.strength()))
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn clear(&mut self) {
        self.active.clear();
        self.pending.clear();
    }
}

// =============================================================================
// Test support: a mock world used by the tests in all three modules.
// =============================================================================

#[cfg(test)]
pub mod mock {
    use super::VoxelWorld;
    use crate::content::block::BlockId;
    use std::collections::HashMap;

    /// A small hand-built world with real sparse sub-voxel masks, matching the storage
    /// rules in SUPERPROMPT.md: undamaged blocks store nothing.
    #[derive(Clone, Default)]
    pub struct MockWorld {
        blocks: HashMap<(i32, i32, i32), BlockId>,
        masks: HashMap<(i32, i32, i32), [u8; 64]>,
        /// What every unset cell is.
        pub background: BlockId,
    }

    #[inline]
    fn bit_index(sx: usize, sy: usize, sz: usize) -> usize {
        (sy * 8 + sz) * 8 + sx
    }

    impl MockWorld {
        pub fn air() -> Self {
            Self {
                background: BlockId::AIR,
                ..Default::default()
            }
        }

        pub fn solid_stone() -> Self {
            Self {
                background: BlockId::STONE,
                ..Default::default()
            }
        }

        pub fn set(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
            self.masks.remove(&(x, y, z));
            self.blocks.insert((x, y, z), id);
        }

        pub fn fill(&mut self, min: (i32, i32, i32), max: (i32, i32, i32), id: BlockId) {
            for x in min.0..=max.0 {
                for y in min.1..=max.1 {
                    for z in min.2..=max.2 {
                        self.set(x, y, z, id);
                    }
                }
            }
        }

        /// A flat stone floor at `y = ground` with air above, over the given extent.
        pub fn flat_ground(extent: i32, ground: i32) -> Self {
            let mut w = Self::air();
            w.fill(
                (-extent, ground - 3, -extent),
                (extent, ground, extent),
                BlockId::STONE,
            );
            w
        }

        pub fn solid_count(&self, x: i32, y: i32, z: i32) -> u32 {
            match self.masks.get(&(x, y, z)) {
                Some(m) => m.iter().map(|b| b.count_ones()).sum(),
                None => {
                    if self.block_at(x, y, z).is_air() {
                        0
                    } else {
                        512
                    }
                }
            }
        }
    }

    impl VoxelWorld for MockWorld {
        fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
            *self.blocks.get(&(x, y, z)).unwrap_or(&self.background)
        }

        fn sub_solid(&self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
            if self.block_at(x, y, z).is_air() {
                return false;
            }
            match self.masks.get(&(x, y, z)) {
                None => true,
                Some(m) => {
                    let i = bit_index(sx, sy, sz);
                    m[i / 8] & (1 << (i % 8)) != 0
                }
            }
        }

        fn fill_ratio(&self, x: i32, y: i32, z: i32) -> f32 {
            self.solid_count(x, y, z) as f32 / 512.0
        }

        fn set_block(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
            self.blocks.insert((x, y, z), id);
            self.masks.remove(&(x, y, z));
        }

        fn carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
            if self.block_at(x, y, z).is_air() {
                return false;
            }
            let mask = self.masks.entry((x, y, z)).or_insert([0xFF; 64]);
            let i = bit_index(sx, sy, sz);
            mask[i / 8] &= !(1 << (i % 8));
            let empty = mask.iter().all(|b| *b == 0);
            if empty {
                self.masks.remove(&(x, y, z));
                self.blocks.insert((x, y, z), BlockId::AIR);
            }
            empty
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockWorld;
    use super::*;
    use crate::content::block::BlockId;

    fn v(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3::new(x, y, z)
    }

    #[test]
    fn solid_rock_is_dramatically_quieter_than_open_air() {
        let air = MockWorld::air();
        let stone = MockWorld::solid_stone();

        let src = v(0.5, 64.5, 0.5);
        let listener = v(10.5, 64.5, 0.5); // 10 blocks away

        let through_air = Propagation::flood(&air, src, LOUDNESS_SMASH).audibility_at(listener);
        let through_rock = Propagation::flood(&stone, src, LOUDNESS_SMASH).audibility_at(listener);

        assert!(
            through_air > HEARING_THRESHOLD,
            "air path should be clearly audible: {through_air}"
        );
        assert!(
            through_rock < through_air / 100.0,
            "10 blocks of stone must be dramatically quieter: air={through_air} rock={through_rock}"
        );
        assert!(
            through_rock < HEARING_THRESHOLD,
            "10 blocks of stone must be inaudible: {through_rock}"
        );
    }

    #[test]
    fn one_wall_between_muffles_but_does_not_silence() {
        let mut w = MockWorld::air();
        // A single stone pane at x = 5, with the listener just past it.
        w.fill((5, 50, -20), (5, 80, 20), BlockId::STONE);

        let src = v(0.5, 64.5, 0.5);
        let listener = v(8.5, 64.5, 0.5);

        let open =
            Propagation::flood(&MockWorld::air(), src, LOUDNESS_SMASH).audibility_at(listener);
        let walled = Propagation::flood(&w, src, LOUDNESS_SMASH).audibility_at(listener);

        assert!(
            walled < open * 0.6,
            "one wall should noticeably muffle: open={open} walled={walled}"
        );
        assert!(
            walled > 0.0,
            "one wall should not silence completely: {walled}"
        );
    }

    #[test]
    fn a_carved_tunnel_leaks_sound_that_solid_rock_would_not() {
        // Solid stone with a 1-block tunnel bored along +X: the tunnel carries sound.
        let mut tunnel = MockWorld::solid_stone();
        tunnel.fill((0, 64, 0), (14, 64, 0), BlockId::AIR);

        let src = v(0.5, 64.5, 0.5);
        let listener = v(12.5, 64.5, 0.5);

        let solid = Propagation::flood(&MockWorld::solid_stone(), src, LOUDNESS_SMASH)
            .audibility_at(listener);
        let bored = Propagation::flood(&tunnel, src, LOUDNESS_SMASH).audibility_at(listener);

        assert!(
            bored > solid * 1000.0,
            "the player's own tunnel must carry sound: solid={solid} bored={bored}"
        );
        assert!(
            bored > HEARING_THRESHOLD,
            "down a tunnel a smash is audible: {bored}"
        );
    }

    /// Two air pockets buried in solid stone, separated by a stone wall `thickness`
    /// blocks deep. Going around means crossing more stone than going through, so
    /// this isolates per-block damping from diffraction.
    fn pocket_world(thickness: i32) -> MockWorld {
        let mut w = MockWorld::solid_stone();
        w.fill((-6, 60, -4), (2, 68, 4), BlockId::AIR);
        w.fill((3 + thickness, 60, -4), (14, 68, 4), BlockId::AIR);
        w
    }

    #[test]
    fn each_solid_block_crossed_costs_far_more_than_a_block_of_air() {
        let src = v(0.5, 64.5, 0.5);
        let listener = v(8.5, 64.5, 0.5);
        let loud = LOUDNESS_EXPLOSION; // loud enough that all three walls transmit something

        let one = Propagation::flood(&pocket_world(1), src, loud).audibility_at(listener);
        let two = Propagation::flood(&pocket_world(2), src, loud).audibility_at(listener);
        let three = Propagation::flood(&pocket_world(3), src, loud).audibility_at(listener);

        assert!(
            one > two && two > three && three > 0.0,
            "{one} {two} {three}"
        );
        // Each extra block of stone should cost exactly exp(SOLID_ATTENUATION).
        let expected = SOLID_ATTENUATION_PER_BLOCK.exp();
        for ratio in [one / two, two / three] {
            assert!(
                (ratio / expected - 1.0).abs() < 0.05,
                "per-block damping is off: ratio={ratio} expected={expected}"
            );
        }
    }

    #[test]
    fn half_carved_rock_damps_less_than_untouched_rock() {
        let mut chewed = pocket_world(2);
        // Hollow out the bottom half of every block in the wall.
        for x in 3..=4 {
            for y in 55..=75 {
                for z in -8..=8 {
                    for sy in 0..4 {
                        for sz in 0..8 {
                            for sx in 0..8 {
                                chewed.carve(x, y, z, sx, sy, sz);
                            }
                        }
                    }
                }
            }
        }
        let src = v(0.5, 64.5, 0.5);
        let listener = v(8.5, 64.5, 0.5);

        let solid =
            Propagation::flood(&pocket_world(2), src, LOUDNESS_EXPLOSION).audibility_at(listener);
        let half = Propagation::flood(&chewed, src, LOUDNESS_EXPLOSION).audibility_at(listener);
        assert!(solid > 0.0, "the control case must be audible at all");
        assert!(
            half > solid * 2.0,
            "half-carved rock must damp less: solid={solid} half={half}"
        );
    }

    #[test]
    fn chip_falls_below_threshold_where_a_smash_does_not() {
        let air = MockWorld::air();
        let src = v(0.5, 64.5, 0.5);
        let listener = v(16.5, 64.5, 0.5); // 16 blocks of open air

        let chip = Propagation::flood(&air, src, LOUDNESS_CHIP).audibility_at(listener);
        let smash = Propagation::flood(&air, src, LOUDNESS_SMASH).audibility_at(listener);

        assert!(
            chip < HEARING_THRESHOLD,
            "a quiet chip must not carry 16 blocks: {chip}"
        );
        assert!(
            smash > HEARING_THRESHOLD,
            "a loud smash must carry 16 blocks: {smash}"
        );
    }

    #[test]
    fn flood_respects_the_visit_budget() {
        let air = MockWorld::air();
        // A very loud sound in wide-open air is the pathological case.
        let p = Propagation::flood(&air, v(0.5, 128.5, 0.5), 50.0);
        assert!(
            p.visited <= VISIT_BUDGET,
            "visited {} exceeds budget",
            p.visited
        );
        assert!(
            p.budget_exhausted,
            "this case should be budget bound, not audibility bound"
        );
    }

    #[test]
    fn quiet_flood_stops_early_without_using_the_whole_budget() {
        let air = MockWorld::air();
        let p = Propagation::flood(&air, v(0.5, 128.5, 0.5), LOUDNESS_CHIP);
        assert!(
            !p.budget_exhausted,
            "a chip should be audibility bound, not budget bound"
        );
        assert!(
            p.visited < VISIT_BUDGET / 2,
            "a chip flood should be cheap: {}",
            p.visited
        );
    }

    #[test]
    fn sound_field_decays_and_retires() {
        let air = MockWorld::air();
        let mut f = SoundField::new();
        let src = v(0.5, 64.5, 0.5);
        f.emit(NoiseEvent::smash(src));
        f.update(&air, 0.016);
        assert_eq!(f.active_count(), 1);

        let near = v(2.5, 64.5, 0.5);
        let fresh = f.audibility_at(near);
        for _ in 0..60 {
            f.update(&air, 1.0 / 60.0);
        }
        let older = f.audibility_at(near);
        assert!(older < fresh, "sound must decay: {fresh} -> {older}");

        for _ in 0..600 {
            f.update(&air, 1.0 / 60.0);
        }
        assert_eq!(f.active_count(), 0, "sound must eventually be forgotten");
        assert!(f.loudest_at(near).is_none());
    }

    #[test]
    fn burst_of_events_never_floods_more_than_the_per_tick_cap() {
        let air = MockWorld::air();
        let mut f = SoundField::new();
        // 200 events scattered far enough apart that none of them merge.
        for i in 0..200 {
            let x = (i % 20) as f32 * 10.0;
            let z = (i / 20) as f32 * 10.0;
            f.emit(NoiseEvent::smash(v(x, 64.5, z)));
        }
        assert!(f.pending_count() <= MAX_PENDING_EVENTS);
        f.update(&air, 0.016);
        assert!(f.active_count() <= MAX_FLOODS_PER_TICK);
        assert!(f.active_count() <= MAX_ACTIVE_SOUNDS);
    }

    #[test]
    fn repeated_chipping_in_one_spot_merges_into_one_sound() {
        let air = MockWorld::air();
        let mut f = SoundField::new();
        for _ in 0..50 {
            f.emit_chip(v(4.5, 64.5, 4.5));
            f.update(&air, 1.0 / 60.0);
        }
        assert_eq!(
            f.active_count(),
            1,
            "continuous chipping must not spawn many floods"
        );
    }

    #[test]
    fn loudest_at_reports_the_sound_position() {
        let air = MockWorld::air();
        let mut f = SoundField::new();
        let quiet = v(3.5, 64.5, 0.5);
        let loud = v(-6.5, 64.5, 0.5);
        f.emit(NoiseEvent::chip(quiet));
        f.emit(NoiseEvent::smash(loud));
        f.update(&air, 0.016);
        f.update(&air, 0.016);

        let heard = f
            .loudest_at(v(0.5, 64.5, 0.5))
            .expect("something should be audible");
        assert!(
            (heard.pos - loud).length() < 1.0,
            "the loud smash should win even though it is further: {:?}",
            heard
        );
    }

    #[test]
    fn silence_is_inaudible() {
        let air = MockWorld::air();
        let mut f = SoundField::new();
        f.update(&air, 0.016);
        assert!(f.loudest_at(v(0.0, 64.0, 0.0)).is_none());
        assert_eq!(f.audibility_at(v(0.0, 64.0, 0.0)), 0.0);
    }
}
