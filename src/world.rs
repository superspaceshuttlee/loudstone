//! The live chunk map: generation, streaming in and out, and remeshing.

use crate::block::BlockId;
use crate::chunk::{Chunk, ChunkPos};
use crate::config::{CHUNK_COLUMN, CHUNK_SIZE, CHUNK_SIZE_I};
use crate::mesh::{mesh_chunk, Neighborhood, Vertex};
use crate::worldgen::TerrainGen;
use std::collections::HashMap;

/// World-space block coordinate to (chunk, local) coordinates.
#[inline]
pub fn split_coord(v: i32) -> (i32, usize) {
    let c = v.div_euclid(CHUNK_SIZE_I);
    let l = v.rem_euclid(CHUNK_SIZE_I) as usize;
    (c, l)
}

pub struct World {
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub terrain: TerrainGen,
}

impl World {
    pub fn new(seed: u32) -> Self {
        Self {
            chunks: HashMap::new(),
            terrain: TerrainGen::new(seed),
        }
    }

    /// Block at a world coordinate. Chunks that are not resident fall back to
    /// generation, which keeps chunk seams correct while neighbours stream in.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        let (cx, lx) = split_coord(x);
        let (cy, ly) = split_coord(y);
        let (cz, lz) = split_coord(z);
        match self.chunks.get(&ChunkPos::new(cx, cy, cz)) {
            Some(c) => c.get(lx, ly, lz),
            None => self.terrain.block_at(x, y, z),
        }
    }

    pub fn set_block(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        let (cx, lx) = split_coord(x);
        let (cy, ly) = split_coord(y);
        let (cz, lz) = split_coord(z);
        if let Some(c) = self.chunks.get_mut(&ChunkPos::new(cx, cy, cz)) {
            c.set(lx, ly, lz, id);
            self.mark_neighbors_dirty(x, y, z, lx, ly, lz, cx, cy, cz);
        }
    }

    /// A block on a chunk boundary changes the neighbouring chunk's visible faces,
    /// so that neighbour needs remeshing too.
    #[allow(clippy::too_many_arguments)]
    fn mark_neighbors_dirty(
        &mut self,
        _x: i32,
        _y: i32,
        _z: i32,
        lx: usize,
        ly: usize,
        lz: usize,
        cx: i32,
        cy: i32,
        cz: i32,
    ) {
        let last = CHUNK_SIZE - 1;
        let mut touch = |dx: i32, dy: i32, dz: i32| {
            if let Some(c) = self.chunks.get_mut(&ChunkPos::new(cx + dx, cy + dy, cz + dz)) {
                c.dirty = true;
            }
        };
        if lx == 0 {
            touch(-1, 0, 0);
        }
        if lx == last {
            touch(1, 0, 0);
        }
        if ly == 0 {
            touch(0, -1, 0);
        }
        if ly == last {
            touch(0, 1, 0);
        }
        if lz == 0 {
            touch(0, 0, -1);
        }
        if lz == last {
            touch(0, 0, 1);
        }
    }

    /// Carve one sub-voxel out of a block. Returns true if the block was destroyed.
    pub fn carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        let (cx, lx) = split_coord(x);
        let (cy, ly) = split_coord(y);
        let (cz, lz) = split_coord(z);
        let destroyed = match self.chunks.get_mut(&ChunkPos::new(cx, cy, cz)) {
            Some(c) => c.carve(lx, ly, lz, sx, sy, sz),
            None => false,
        };
        self.mark_neighbors_dirty(x, y, z, lx, ly, lz, cx, cy, cz);
        destroyed
    }

    /// Ensure a chunk exists, generating it if not. Returns true if it generated one.
    pub fn ensure(&mut self, pos: ChunkPos) -> bool {
        if self.chunks.contains_key(&pos) {
            return false;
        }
        let chunk = self.terrain.generate(pos);
        self.chunks.insert(pos, chunk);
        true
    }

    /// Chunk positions within `radius` of a centre column, nearest first.
    pub fn wanted_chunks(&self, center: ChunkPos, radius: i32) -> Vec<ChunkPos> {
        let mut out = Vec::new();
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                // Round the load region rather than squaring it, so the horizon
                // is an even distance in every direction.
                if dx * dx + dz * dz > radius * radius {
                    continue;
                }
                for cy in 0..CHUNK_COLUMN {
                    out.push(ChunkPos::new(center.x + dx, cy, center.z + dz));
                }
            }
        }
        out.sort_by_key(|p| {
            let dx = p.x - center.x;
            let dz = p.z - center.z;
            let dy = p.y - center.y;
            dx * dx + dz * dz + dy * dy / 4
        });
        out
    }

    /// Drop chunks outside the keep radius. Returns the positions removed.
    pub fn unload_far(&mut self, center: ChunkPos, keep_radius: i32) -> Vec<ChunkPos> {
        let r2 = keep_radius * keep_radius;
        let doomed: Vec<ChunkPos> = self
            .chunks
            .keys()
            .filter(|p| {
                let dx = p.x - center.x;
                let dz = p.z - center.z;
                dx * dx + dz * dz > r2
            })
            .copied()
            .collect();
        for p in &doomed {
            self.chunks.remove(p);
        }
        doomed
    }

    /// Build a meshing snapshot for a chunk.
    pub fn neighborhood(&self, pos: ChunkPos) -> Option<Neighborhood> {
        let chunk = self.chunks.get(&pos)?;
        Some(Neighborhood::build(chunk, |x, y, z| self.block_at(x, y, z)))
    }

    /// Mesh a chunk immediately on the calling thread.
    pub fn mesh_now(&self, pos: ChunkPos) -> Option<(Vec<Vertex>, Vec<u32>)> {
        self.neighborhood(pos).map(|nb| mesh_chunk(&nb))
    }

    /// Dirty chunks, nearest to the centre first.
    pub fn dirty_chunks(&self, center: ChunkPos) -> Vec<ChunkPos> {
        let mut v: Vec<ChunkPos> = self
            .chunks
            .iter()
            .filter(|(_, c)| c.dirty && c.generated)
            .map(|(p, _)| *p)
            .collect();
        v.sort_by_key(|p| {
            let dx = p.x - center.x;
            let dz = p.z - center.z;
            let dy = p.y - center.y;
            dx * dx + dz * dz + dy * dy / 4
        });
        v
    }

    pub fn clear_dirty(&mut self, pos: ChunkPos) {
        if let Some(c) = self.chunks.get_mut(&pos) {
            c.dirty = false;
        }
    }

    /// Highest non-air block in a column, for spawn placement.
    pub fn surface_y(&self, x: i32, z: i32) -> i32 {
        self.terrain.height_at(x, z)
    }
}
