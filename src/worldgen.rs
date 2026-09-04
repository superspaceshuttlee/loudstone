//! Deterministic terrain generation. Everything here is a pure function of the
//! seed and the block coordinate, so a chunk generated on any thread at any time
//! produces identical results.

use crate::block::BlockId;
use crate::chunk::{Chunk, ChunkPos};
use crate::config::{
    CHUNK_SIZE, CHUNK_VOL, SURFACE_PROBE_MARGIN, SURFACE_PROBE_STRIDE, WORLD_HEIGHT,
};
use noise::{NoiseFn, Perlin};

/// Tunables for the shape of the world.
pub mod tuning {
    /// Average ground height in blocks.
    pub const SEA_LEVEL: f64 = 64.0;
    /// Broad continental shape.
    pub const CONTINENT_SCALE: f64 = 0.0018;
    pub const CONTINENT_AMP: f64 = 34.0;
    /// Local hills riding on top of the continents.
    pub const HILL_SCALE: f64 = 0.012;
    pub const HILL_AMP: f64 = 11.0;
    /// Cave carve: 3D noise above this threshold becomes air.
    pub const CAVE_SCALE: f64 = 0.035;
    pub const CAVE_THRESHOLD: f64 = 0.58;
    /// Caves fade out near the surface so the world is not a sponge.
    pub const CAVE_SURFACE_FADE: f64 = 8.0;
    /// Soil depth below the grass layer.
    pub const DIRT_DEPTH: i32 = 4;
    pub const ORE_SCALE: f64 = 0.09;
}

/// The vertical extent of solid ground in one chunk-column, bounded cheaply.
///
/// `lo`/`hi` are conservative: real terrain in the column is guaranteed to sit
/// inside them. The streamer uses this to avoid queueing sky chunks at all.
#[derive(Copy, Clone, Debug)]
pub struct ColumnBounds {
    pub lo: i32,
    pub hi: i32,
}

pub struct TerrainGen {
    continent: Perlin,
    hills: Perlin,
    cave: Perlin,
    ore: Perlin,
    pub seed: u32,
}

impl TerrainGen {
    pub fn new(seed: u32) -> Self {
        Self {
            continent: Perlin::new(seed),
            hills: Perlin::new(seed.wrapping_add(1)),
            cave: Perlin::new(seed.wrapping_add(2)),
            ore: Perlin::new(seed.wrapping_add(3)),
            seed,
        }
    }

    /// Fractal sum of a 2D noise source.
    fn fbm2(noise: &Perlin, x: f64, z: f64, scale: f64, octaves: u32) -> f64 {
        let mut sum = 0.0;
        let mut amp = 1.0;
        let mut freq = scale;
        let mut norm = 0.0;
        for _ in 0..octaves {
            sum += noise.get([x * freq, z * freq]) * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }

    /// Ground surface height at a world column.
    pub fn height_at(&self, x: i32, z: i32) -> i32 {
        let (xf, zf) = (x as f64, z as f64);
        let continent = Self::fbm2(&self.continent, xf, zf, tuning::CONTINENT_SCALE, 4);
        let hills = Self::fbm2(&self.hills, xf, zf, tuning::HILL_SCALE, 3);
        let h = tuning::SEA_LEVEL + continent * tuning::CONTINENT_AMP + hills * tuning::HILL_AMP;
        (h as i32).clamp(1, WORLD_HEIGHT - 1)
    }

    /// Whether a solid block at this position is carved out by a cave.
    fn is_cave(&self, x: i32, y: i32, z: i32, surface: i32) -> bool {
        if y < 2 {
            return false;
        }
        let depth = (surface - y) as f64;
        if depth < tuning::CAVE_SURFACE_FADE {
            return false;
        }
        let s = tuning::CAVE_SCALE;
        let v = self
            .cave
            .get([x as f64 * s, y as f64 * s * 1.6, z as f64 * s]);
        v.abs() > tuning::CAVE_THRESHOLD
    }

    /// Which ore, if any, replaces stone here. Rarer and richer with depth.
    fn ore_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let s = tuning::ORE_SCALE;
        let v = self.ore.get([x as f64 * s, y as f64 * s, z as f64 * s]);
        // Each ore occupies a narrow band of the noise range, gated by depth, so
        // veins come out as connected blobs rather than scattered specks.
        if y < 16 && v > 0.80 {
            Some(BlockId::DIAMOND_ORE)
        } else if y < 32 && v > 0.76 {
            Some(BlockId::GOLD_ORE)
        } else if y < 56 && v > 0.68 {
            Some(BlockId::IRON_ORE)
        } else if v > 0.62 {
            Some(BlockId::COAL_ORE)
        } else {
            None
        }
    }

    /// The block that generation places at a world coordinate, given a surface
    /// height the caller has already computed. Hoisting `height_at` out of the
    /// inner loop is worth ~16x on chunk generation, because otherwise every one
    /// of a chunk's 4,096 blocks re-runs seven octaves of 2D noise for a value
    /// that is constant down the whole column.
    #[inline]
    pub fn block_at_surface(&self, x: i32, y: i32, z: i32, surface: i32) -> BlockId {
        if y <= 0 {
            return BlockId::BEDROCK;
        }
        if y >= WORLD_HEIGHT || y > surface {
            return BlockId::AIR;
        }
        if self.is_cave(x, y, z, surface) {
            return BlockId::AIR;
        }
        if y == surface {
            return BlockId::GRASS;
        }
        if y > surface - tuning::DIRT_DEPTH {
            return BlockId::DIRT;
        }
        self.ore_at(x, y, z).unwrap_or(BlockId::STONE)
    }

    /// The block that generation places at a world coordinate.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        if y <= 0 {
            return BlockId::BEDROCK;
        }
        if y >= WORLD_HEIGHT {
            return BlockId::AIR;
        }
        self.block_at_surface(x, y, z, self.height_at(x, z))
    }

    /// The 16x16 surface heights of one chunk-column, in `local_index` x/z order
    /// (`z * CHUNK_SIZE + x`).
    pub fn column_heights(&self, cx: i32, cz: i32) -> [i32; CHUNK_SIZE * CHUNK_SIZE] {
        let ox = cx * CHUNK_SIZE as i32;
        let oz = cz * CHUNK_SIZE as i32;
        let mut out = [0i32; CHUNK_SIZE * CHUNK_SIZE];
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                out[z * CHUNK_SIZE + x] = self.height_at(ox + x as i32, oz + z as i32);
            }
        }
        out
    }

    /// Conservative vertical bounds of solid ground in a chunk-column, from a
    /// coarse probe rather than all 256 columns. Terrain cannot swing more than
    /// a couple of blocks between probe points at these noise frequencies, so
    /// the margin makes this an over-estimate, never an under-estimate.
    pub fn column_bounds(&self, cx: i32, cz: i32) -> ColumnBounds {
        let ox = cx * CHUNK_SIZE as i32;
        let oz = cz * CHUNK_SIZE as i32;
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        let mut z = 0;
        while z <= CHUNK_SIZE as i32 {
            let mut x = 0;
            while x <= CHUNK_SIZE as i32 {
                let h = self.height_at(ox + x.min(CHUNK_SIZE as i32 - 1), oz + z.min(CHUNK_SIZE as i32 - 1));
                lo = lo.min(h);
                hi = hi.max(h);
                x += SURFACE_PROBE_STRIDE;
            }
            z += SURFACE_PROBE_STRIDE;
        }
        ColumnBounds {
            lo: (lo - SURFACE_PROBE_MARGIN).max(0),
            hi: (hi + SURFACE_PROBE_MARGIN).min(WORLD_HEIGHT - 1),
        }
    }

    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let (ox, oy, oz) = pos.origin();
        let heights = self.column_heights(pos.x, pos.z);

        // Entirely above the local terrain: nothing but air, and the caller can
        // see that from `Chunk::is_empty` without touching a single block.
        let max_surface = heights.iter().copied().max().unwrap_or(0);
        if oy > max_surface {
            let mut c = Chunk::new(pos);
            c.generated = true;
            return c;
        }

        let mut blocks = Box::new([BlockId::AIR; CHUNK_VOL]);
        for y in 0..CHUNK_SIZE as i32 {
            let wy = oy + y;
            for z in 0..CHUNK_SIZE as i32 {
                let wz = oz + z;
                for x in 0..CHUNK_SIZE as i32 {
                    let surface = heights[(z * CHUNK_SIZE as i32 + x) as usize];
                    if wy > surface && wy > 0 {
                        continue; // already air
                    }
                    let id = self.block_at_surface(ox + x, wy, wz, surface);
                    if !id.is_air() {
                        blocks[crate::chunk::local_index(x as usize, y as usize, z as usize)
                            as usize] = id;
                    }
                }
            }
        }
        Chunk::from_blocks(pos, blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CHUNK_SIZE_I;

    /// Deterministic worldgen is the contract every other system leans on.
    #[test]
    fn generation_is_byte_identical_for_the_same_seed_and_position() {
        let a = TerrainGen::new(1337);
        let b = TerrainGen::new(1337);
        for pos in [
            ChunkPos::new(0, 4, 0),
            ChunkPos::new(-7, 3, 12),
            ChunkPos::new(51, 0, -33),
            ChunkPos::new(2, 15, 2),
        ] {
            let ca = a.generate(pos);
            let cb = b.generate(pos);
            assert_eq!(
                ca.blocks().as_slice(),
                cb.blocks().as_slice(),
                "chunk {pos:?} differs between two generators with the same seed"
            );
            // And a second run of the same generator must agree too.
            let cc = a.generate(pos);
            assert_eq!(ca.blocks().as_slice(), cc.blocks().as_slice());
        }
    }

    #[test]
    fn a_different_seed_produces_a_different_world() {
        let a = TerrainGen::new(1337);
        let b = TerrainGen::new(9001);
        let pos = ChunkPos::new(3, 4, 3);
        assert_ne!(
            a.generate(pos).blocks().as_slice(),
            b.generate(pos).blocks().as_slice()
        );
    }

    /// The hoisted-surface fast path must agree with the general one exactly,
    /// or generated chunks and the meshing skirt would disagree at seams.
    #[test]
    fn hoisted_surface_matches_the_general_path() {
        let g = TerrainGen::new(4242);
        for x in [-33, -1, 0, 7, 250] {
            for z in [-9, 0, 3, 128] {
                let s = g.height_at(x, z);
                for y in [0, 1, 12, 60, s - 1, s, s + 1, 200, 255] {
                    assert_eq!(
                        g.block_at(x, y, z),
                        g.block_at_surface(x, y, z, s),
                        "mismatch at {x},{y},{z}"
                    );
                }
            }
        }
    }

    /// Symptom 3 in the bug report was grass appearing far below ground. The
    /// generator itself never does that: grass exists only at `y == surface`.
    #[test]
    fn grass_only_ever_sits_on_the_surface() {
        let g = TerrainGen::new(1337);
        for cx in -2..3 {
            for cz in -2..3 {
                let heights = g.column_heights(cx, cz);
                for cy in 0..6 {
                    let c = g.generate(ChunkPos::new(cx, cy, cz));
                    if c.is_empty() {
                        continue;
                    }
                    for y in 0..CHUNK_SIZE {
                        for z in 0..CHUNK_SIZE {
                            for x in 0..CHUNK_SIZE {
                                if c.get(x, y, z) == BlockId::GRASS {
                                    let wy = cy * CHUNK_SIZE_I + y as i32;
                                    assert_eq!(
                                        wy,
                                        heights[z * CHUNK_SIZE + x],
                                        "grass at world y={wy} is not on the surface"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn column_bounds_contain_the_real_terrain() {
        let g = TerrainGen::new(1337);
        for cx in -3..4 {
            for cz in -3..4 {
                let b = g.column_bounds(cx, cz);
                for h in g.column_heights(cx, cz) {
                    assert!(
                        h >= b.lo && h <= b.hi,
                        "height {h} escapes bounds {b:?} at column {cx},{cz}"
                    );
                }
            }
        }
    }

    #[test]
    fn bedrock_floors_the_world_and_sky_is_air() {
        let g = TerrainGen::new(1337);
        assert_eq!(g.block_at(5, 0, 5), BlockId::BEDROCK);
        assert_eq!(g.block_at(5, -1, 5), BlockId::BEDROCK);
        assert_eq!(g.block_at(5, WORLD_HEIGHT, 5), BlockId::AIR);
        assert_eq!(g.block_at(5, WORLD_HEIGHT - 1, 5), BlockId::AIR);
    }
}
