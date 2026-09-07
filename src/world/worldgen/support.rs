//! The pieces the generator is assembled from: deterministic hashing, splines,
//! the biome table, tree shapes, and the per-thread column memo.
//!
//! Nothing here knows the shape of the world. Each part answers one small
//! question -- what does this seed and coordinate hash to, which biome does this
//! climate land in -- so the generator itself reads as the decisions it makes
//! rather than the arithmetic underneath them.

use super::*;

use tuning::{ClimateBiome, TreeKind};

// ---------------------------------------------------------------------------
// Hashing. Deterministic pseudo-randomness keyed on the seed and a coordinate:
// never an RNG whose state depends on the order chunks happen to be generated.
// ---------------------------------------------------------------------------

#[inline]
pub fn mix(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

#[inline]
pub fn hash2(seed: u32, x: i32, z: i32, salt: u32) -> u32 {
    let mut h = seed ^ 0x9E37_79B9;
    h ^= (x as u32).wrapping_mul(0x85EB_CA6B);
    h = h.rotate_left(13);
    h ^= (z as u32).wrapping_mul(0x27D4_EB2F);
    h = h.rotate_left(7);
    h ^= salt.wrapping_mul(0x1656_67B1);
    mix(h)
}

#[inline]
pub fn hash3(seed: u32, x: i32, y: i32, z: i32, salt: u32) -> u32 {
    let mut h = seed ^ 0x9E37_79B9;
    h ^= (x as u32).wrapping_mul(0x85EB_CA6B);
    h = h.rotate_left(13);
    h ^= (y as u32).wrapping_mul(0xC2B2_AE35);
    h = h.rotate_left(11);
    h ^= (z as u32).wrapping_mul(0x27D4_EB2F);
    h = h.rotate_left(7);
    h ^= salt.wrapping_mul(0x1656_67B1);
    mix(h)
}

/// A hash word as a float in `0.0..1.0`.
#[inline]
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn smoothstep64(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Piecewise-linear lookup. Linear rather than smooth on purpose: smoothing the
/// joins flattens the derivative at every knot, which shows up in the world as
/// terraces at fixed heights.
pub fn spline(points: &[(f64, f64)], t: f64) -> f64 {
    if t <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if t <= x1 {
            let f = (t - x0) / (x1 - x0);
            return y0 + (y1 - y0) * f;
        }
    }
    points[points.len() - 1].1
}

// ---------------------------------------------------------------------------
// Biomes
// ---------------------------------------------------------------------------

/// What a column reads as. The first three are derived from height and water
/// rather than from climate; the rest come from the [`tuning::CLIMATE`] table,
/// and `Mountains`/`SnowyPeaks` are an altitude overlay on top of any of them.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Biome {
    Ocean,
    Beach,
    River,
    #[default]
    Plains,
    Forest,
    Desert,
    Savanna,
    Taiga,
    SnowyTundra,
    Swamp,
    Mountains,
    SnowyPeaks,
}

impl Biome {
    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "ocean",
            Biome::Beach => "beach",
            Biome::River => "river",
            Biome::Plains => "plains",
            Biome::Forest => "forest",
            Biome::Desert => "desert",
            Biome::Savanna => "savanna",
            Biome::Taiga => "taiga",
            Biome::SnowyTundra => "snowy tundra",
            Biome::Swamp => "swamp",
            Biome::Mountains => "mountains",
            Biome::SnowyPeaks => "snowy peaks",
        }
    }
}

/// Everything about one world column that does not depend on `y`.
///
/// Hoisting this out of the inner loop is worth roughly 16x on chunk
/// generation: without it, every one of a chunk's 4,096 blocks would re-run
/// two dozen octaves of 2D noise for values that are constant down the column.
#[derive(Copy, Clone, Debug, Default)]
pub struct Column {
    /// y of the topmost solid ground block.
    pub surface: i32,
    /// Temperature at the surface, 0..1, already reduced by altitude.
    pub temp: f32,
    pub humid: f32,
    /// How alpine this column is, 0..1. Drives bare rock and cliff-side caves.
    pub rock: f32,
    pub biome: Biome,
    /// Block placed at `surface`.
    pub top: BlockId,
    /// Block placed for the next `filler_depth` blocks below `surface`.
    pub filler: BlockId,
    pub filler_depth: i32,
    /// The tree this column would grow, if a tree cell lands here.
    pub tree: TreeKind,
    pub tree_chance: f32,
    pub plant_chance: f32,
    pub flower_chance: f32,
    /// Ravine strength, 0 when this column is nowhere near one.
    pub ravine: f32,
}

/// The vertical extent of solid ground in one chunk-column, bounded cheaply.
///
/// `lo`/`hi` are conservative: everything generation places in the column is
/// guaranteed to sit inside them, water surface and tree canopies included.
/// The streamer uses this to avoid queueing sky chunks at all, so an
/// *under*-estimate punches holes in the world while an over-estimate only
/// costs an empty chunk.
#[derive(Copy, Clone, Debug)]
pub struct ColumnBounds {
    pub lo: i32,
    pub hi: i32,
}

// ---------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------

/// One resolved tree. Produced identically by every caller from the cell hash,
/// which is what lets a trunk and its canopy straddle a chunk boundary without
/// the two chunks talking to each other.
#[derive(Copy, Clone, Debug)]
pub struct Tree {
    pub x: i32,
    pub z: i32,
    /// y of the lowest trunk block (one above the ground).
    pub base: i32,
    pub kind: TreeKind,
    pub height: i32,
    pub salt: u32,
}

impl Tree {
    #[inline]
    pub fn log(&self) -> BlockId {
        match self.kind {
            TreeKind::Birch => BlockId::BIRCH_LOG,
            TreeKind::Spruce => BlockId::SPRUCE_LOG,
            _ => BlockId::WOOD,
        }
    }

    #[inline]
    pub fn leaf(&self) -> BlockId {
        match self.kind {
            TreeKind::Birch => BlockId::BIRCH_LEAVES,
            TreeKind::Spruce => BlockId::SPRUCE_LEAVES,
            _ => BlockId::LEAVES,
        }
    }

    /// Lowest and highest world y this tree can occupy.
    #[inline]
    pub fn y_range(&self) -> (i32, i32) {
        (self.base, self.base + self.height)
    }

    /// The block this tree puts at a world coordinate, if any.
    #[inline]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let dy = y - self.base;
        if dy < 0 || dy > self.height {
            return None;
        }
        let dx = x - self.x;
        let dz = z - self.z;
        if self.kind == TreeKind::Cactus {
            return (dx == 0 && dz == 0 && dy < self.height).then_some(BlockId::CACTUS);
        }
        if dx == 0 && dz == 0 && dy < self.height {
            return Some(self.log());
        }
        let r = self.canopy_radius(dy);
        if r == 0 {
            // The very tip still gets a leaf directly over the trunk.
            return (dx == 0 && dz == 0).then(|| self.leaf());
        }
        if dx.abs() > r || dz.abs() > r {
            return None;
        }
        // Round the corners off, then trim the remaining outer ring at random
        // so no two canopies have the same silhouette.
        let d2 = dx * dx + dz * dz;
        if d2 > r * r + 1 {
            return None;
        }
        if dx.abs() == r
            && dz.abs() == r
            && unit(hash3(self.salt, dx, dy, dz, tuning::SALT_LEAF)) < tuning::LEAF_TRIM
        {
            return None;
        }
        Some(self.leaf())
    }

    /// Canopy half-width at a height above the trunk base.
    #[inline]
    pub fn canopy_radius(&self, dy: i32) -> i32 {
        match self.kind {
            TreeKind::Spruce => {
                // Layered cone: a wide ring every third layer going down.
                let k = self.height - dy;
                if k < 0 || dy < 2 {
                    return 0;
                }
                match k {
                    0 => 0,
                    1 => 1,
                    _ if k % 3 == 2 => 2,
                    _ => 1,
                }
            }
            TreeKind::Cactus | TreeKind::None => 0,
            // Oak and birch: two fat layers around the top of the trunk and a
            // narrow cap over them.
            _ => match self.height - dy {
                0 => 1,
                1 => 2,
                2 | 3 => 2,
                _ => 0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Per-thread column memo
//
// The same column is asked for many times: once per chunk in a 16-high stack,
// once per tree cell in range, and 18x18 times by the meshing skirt. Computing
// a column is ~25 octaves of 2D noise, so a memo turns the repeat visits into a
// load. It is a pure cache -- keyed on (seed, x, z) with the full key compared
// on hit -- so it cannot change what is generated, only how fast.
// ---------------------------------------------------------------------------

pub const CACHE_BITS: u32 = 11;
pub const CACHE_SLOTS: usize = 1 << CACHE_BITS;

#[derive(Copy, Clone)]
pub struct Slot {
    pub seed: u32,
    pub x: i32,
    pub z: i32,
    pub valid: bool,
    pub col: Column,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            seed: 0,
            x: 0,
            z: 0,
            valid: false,
            col: Column::default(),
        }
    }
}

thread_local! {
    pub static COLUMN_MEMO: RefCell<Vec<Slot>> =
        RefCell::new(vec![Slot::default(); CACHE_SLOTS]);
}

#[inline]
pub fn memo_index(seed: u32, x: i32, z: i32) -> usize {
    (hash2(seed, x, z, 0) >> (32 - CACHE_BITS)) as usize
}
