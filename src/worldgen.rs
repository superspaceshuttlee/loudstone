//! Deterministic terrain generation. Everything here is a pure function of the
//! seed and the block coordinate, so a chunk generated on any thread at any time
//! produces identical results.

use crate::block::BlockId;
use crate::chunk::{Chunk, ChunkPos};
use crate::config::{CHUNK_SIZE, WORLD_HEIGHT};
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
        let h = tuning::SEA_LEVEL
            + continent * tuning::CONTINENT_AMP
            + hills * tuning::HILL_AMP;
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

    /// The block that generation places at a world coordinate.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        if y <= 0 {
            return BlockId::BEDROCK;
        }
        if y >= WORLD_HEIGHT {
            return BlockId::AIR;
        }
        let surface = self.height_at(x, z);
        if y > surface {
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

    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::new(pos);
        let (ox, oy, oz) = pos.origin();

        // Skip the whole chunk when it is entirely above the local terrain.
        let mut max_surface = i32::MIN;
        let mut min_surface = i32::MAX;
        for z in 0..CHUNK_SIZE as i32 {
            for x in 0..CHUNK_SIZE as i32 {
                let h = self.height_at(ox + x, oz + z);
                max_surface = max_surface.max(h);
                min_surface = min_surface.min(h);
            }
        }
        if oy > max_surface {
            chunk.generated = true;
            chunk.dirty = true;
            return chunk;
        }

        for y in 0..CHUNK_SIZE as i32 {
            for z in 0..CHUNK_SIZE as i32 {
                for x in 0..CHUNK_SIZE as i32 {
                    let id = self.block_at(ox + x, oy + y, oz + z);
                    if !id.is_air() {
                        chunk.set(x as usize, y as usize, z as usize, id);
                    }
                }
            }
        }
        chunk.generated = true;
        chunk.dirty = true;
        chunk
    }
}
