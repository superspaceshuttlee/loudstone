//! Chunk storage, including the sparse sub-voxel damage layer.
//!
//! The sub-voxel constraint is the load-bearing design decision here: 8^3 = 512
//! sub-voxels per block, so storing them densely for every block would multiply
//! world memory by 512. Instead an undamaged block stores *nothing* and is treated
//! as implicitly full. Only blocks the player has chipped allocate a 64-byte
//! bitmask, and they drop out of the map again the moment they are fully carved
//! (block becomes air) or fully restored.

use crate::block::BlockId;
use crate::config::{CHUNK_SIZE, CHUNK_SIZE_I, CHUNK_VOL, SUBVOX};
use std::collections::HashMap;

/// Chunk coordinate in chunk space (multiply by CHUNK_SIZE for world blocks).
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// World-space block coordinate of this chunk's minimum corner.
    pub fn origin(self) -> (i32, i32, i32) {
        (
            self.x * CHUNK_SIZE_I,
            self.y * CHUNK_SIZE_I,
            self.z * CHUNK_SIZE_I,
        )
    }
}

/// Number of bytes in one block's sub-voxel bitmask (512 bits).
pub const MASK_BYTES: usize = SUBVOX * SUBVOX * SUBVOX / 8;

/// A 512-bit occupancy mask over one block's sub-voxels. A set bit means solid.
#[derive(Copy, Clone)]
pub struct SubMask(pub [u8; MASK_BYTES]);

impl SubMask {
    /// A fully solid block.
    pub fn full() -> Self {
        SubMask([0xFF; MASK_BYTES])
    }

    /// A fully carved block. Only useful for tests -- a live chunk drops the
    /// entry and turns the block to air the moment it empties.
    pub fn empty() -> Self {
        SubMask([0x00; MASK_BYTES])
    }

    #[inline]
    fn bit_index(x: usize, y: usize, z: usize) -> usize {
        (y * SUBVOX + z) * SUBVOX + x
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> bool {
        let i = Self::bit_index(x, y, z);
        self.0[i >> 3] & (1 << (i & 7)) != 0
    }

    #[inline]
    pub fn clear(&mut self, x: usize, y: usize, z: usize) {
        let i = Self::bit_index(x, y, z);
        self.0[i >> 3] &= !(1 << (i & 7));
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize) {
        let i = Self::bit_index(x, y, z);
        self.0[i >> 3] |= 1 << (i & 7);
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&b| b == 0)
    }

    pub fn is_full(&self) -> bool {
        self.0.iter().all(|&b| b == 0xFF)
    }

    /// Count of sub-voxels still solid.
    pub fn solid_count(&self) -> u32 {
        self.0.iter().map(|b| b.count_ones()).sum()
    }

    /// Fraction of sub-voxels still solid, 0.0..=1.0.
    pub fn fill_ratio(&self) -> f32 {
        self.solid_count() as f32 / (SUBVOX * SUBVOX * SUBVOX) as f32
    }
}

#[derive(Clone)]
pub struct Chunk {
    pub pos: ChunkPos,
    blocks: Box<[BlockId; CHUNK_VOL]>,
    /// Sparse: only blocks that have been chipped appear here.
    damage: HashMap<u16, SubMask>,
    /// Count of non-air blocks. Lets the streamer skip empty chunks entirely.
    solid: u32,
    /// Count of blocks that do *not* occlude their neighbours (air, water,
    /// leaves). Zero means the chunk is a solid opaque block of rock, which is
    /// the fast path that lets the mesher discard buried chunks without work.
    non_opaque: u32,
    /// True once terrain generation has filled this chunk.
    pub generated: bool,
}

#[inline]
pub fn local_index(x: usize, y: usize, z: usize) -> u16 {
    ((y * CHUNK_SIZE + z) * CHUNK_SIZE + x) as u16
}

/// Inverse of [`local_index`].
#[inline]
pub fn local_coords(i: u16) -> (usize, usize, usize) {
    let i = i as usize;
    (
        i % CHUNK_SIZE,
        i / (CHUNK_SIZE * CHUNK_SIZE),
        (i / CHUNK_SIZE) % CHUNK_SIZE,
    )
}

impl Chunk {
    pub fn new(pos: ChunkPos) -> Self {
        Self {
            pos,
            blocks: Box::new([BlockId::AIR; CHUNK_VOL]),
            damage: HashMap::new(),
            solid: 0,
            non_opaque: CHUNK_VOL as u32,
            generated: false,
        }
    }

    /// Build from a filled block array, recomputing the occupancy summary once.
    /// Generation uses this instead of hammering `set` 4,096 times.
    pub fn from_blocks(pos: ChunkPos, blocks: Box<[BlockId; CHUNK_VOL]>) -> Self {
        let mut solid = 0u32;
        let mut non_opaque = 0u32;
        for b in blocks.iter() {
            if !b.is_air() {
                solid += 1;
            }
            if !b.is_opaque() {
                non_opaque += 1;
            }
        }
        Self {
            pos,
            blocks,
            damage: HashMap::new(),
            solid,
            non_opaque,
            generated: true,
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        self.blocks[local_index(x, y, z) as usize]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        let i = local_index(x, y, z);
        let old = self.blocks[i as usize];
        if old == id {
            // Still drop any carving: re-placing the same block heals it.
            self.damage.remove(&i);
            return;
        }
        if old.is_air() && !id.is_air() {
            self.solid += 1;
        } else if !old.is_air() && id.is_air() {
            self.solid -= 1;
        }
        if old.is_opaque() && !id.is_opaque() {
            self.non_opaque += 1;
        } else if !old.is_opaque() && id.is_opaque() {
            self.non_opaque -= 1;
        }
        self.blocks[i as usize] = id;
        // Changing a block's identity resets any carving on it.
        self.damage.remove(&i);
    }

    /// Raw block slice, for the mesher and for serialization.
    #[inline]
    pub fn blocks(&self) -> &[BlockId; CHUNK_VOL] {
        &self.blocks
    }

    /// The carve mask for a block, or `None` if it is undamaged (implicitly full).
    #[inline]
    pub fn mask(&self, x: usize, y: usize, z: usize) -> Option<&SubMask> {
        self.damage.get(&local_index(x, y, z))
    }

    pub fn damage_map(&self) -> &HashMap<u16, SubMask> {
        &self.damage
    }

    /// True when any block in this chunk is partially carved.
    pub fn has_damage(&self) -> bool {
        !self.damage.is_empty()
    }

    /// No solid blocks at all: nothing to mesh, nothing to collide with.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.solid == 0
    }

    /// Every block occludes. Such a chunk shows no faces of its own unless a
    /// neighbour is see-through, and a chunk whose six neighbours are all like
    /// this is provably invisible.
    #[inline]
    pub fn is_uniform_opaque(&self) -> bool {
        self.non_opaque == 0 && self.damage.is_empty()
    }

    /// Clear a single sub-voxel. Returns true if the block was fully destroyed.
    pub fn carve(&mut self, x: usize, y: usize, z: usize, sx: usize, sy: usize, sz: usize) -> bool {
        let i = local_index(x, y, z);
        let id = self.blocks[i as usize];
        if id.is_air() {
            return false;
        }
        let mask = self.damage.entry(i).or_insert_with(SubMask::full);
        if !mask.get(sx, sy, sz) {
            // Already gone. Nothing changed, so do not report progress.
            if mask.is_full() {
                self.damage.remove(&i);
            }
            return false;
        }
        mask.clear(sx, sy, sz);
        if mask.is_empty() {
            self.damage.remove(&i);
            self.blocks[i as usize] = BlockId::AIR;
            self.solid -= 1;
            if id.is_opaque() {
                self.non_opaque += 1;
            }
            return true;
        }
        false
    }

    /// How intact a block is, 1.0 when untouched.
    #[inline]
    pub fn fill_ratio(&self, x: usize, y: usize, z: usize) -> f32 {
        if self.blocks[local_index(x, y, z) as usize].is_air() {
            return 0.0;
        }
        match self.damage.get(&local_index(x, y, z)) {
            Some(m) => m.fill_ratio(),
            None => 1.0,
        }
    }

    /// Whether one sub-voxel is still solid. Untouched blocks are solid
    /// everywhere; air blocks are solid nowhere.
    #[inline]
    pub fn sub_solid(&self, x: usize, y: usize, z: usize, sx: usize, sy: usize, sz: usize) -> bool {
        let i = local_index(x, y, z);
        if self.blocks[i as usize].is_air() {
            return false;
        }
        match self.damage.get(&i) {
            Some(m) => m.get(sx, sy, sz),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_index_round_trips() {
        for y in 0..CHUNK_SIZE {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    let i = local_index(x, y, z);
                    assert_eq!(local_coords(i), (x, y, z));
                }
            }
        }
    }

    #[test]
    fn mask_bits_are_independent() {
        let mut m = SubMask::full();
        assert!(m.is_full());
        m.clear(3, 4, 5);
        assert!(!m.get(3, 4, 5));
        assert!(m.get(3, 4, 6));
        assert_eq!(m.solid_count(), 511);
        m.set(3, 4, 5);
        assert!(m.is_full());
    }

    /// The headline sub-voxel requirement: carve all 512 and the block is gone.
    #[test]
    fn carving_every_subvoxel_destroys_the_block() {
        let mut c = Chunk::new(ChunkPos::new(0, 0, 0));
        c.set(1, 2, 3, BlockId::STONE);
        assert_eq!(c.get(1, 2, 3), BlockId::STONE);

        let mut destroyed = false;
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..SUBVOX {
                    assert!(!destroyed, "block destroyed before the last sub-voxel");
                    destroyed = c.carve(1, 2, 3, sx, sy, sz);
                }
            }
        }
        assert!(destroyed, "carving all 512 sub-voxels must destroy the block");
        assert_eq!(c.get(1, 2, 3), BlockId::AIR);
        assert!(!c.has_damage(), "the mask must be released when it empties");
        assert!(c.is_empty());
        assert_eq!(c.fill_ratio(1, 2, 3), 0.0);
    }

    #[test]
    fn partial_carve_reports_fill_and_sub_solidity() {
        let mut c = Chunk::new(ChunkPos::new(0, 0, 0));
        c.set(0, 0, 0, BlockId::STONE);
        assert_eq!(c.fill_ratio(0, 0, 0), 1.0);
        assert!(c.sub_solid(0, 0, 0, 7, 7, 7));

        assert!(!c.carve(0, 0, 0, 0, 0, 0));
        assert!(!c.sub_solid(0, 0, 0, 0, 0, 0));
        assert!(c.sub_solid(0, 0, 0, 1, 0, 0));
        assert!((c.fill_ratio(0, 0, 0) - 511.0 / 512.0).abs() < 1e-6);
        // Carving the same sub-voxel twice is not progress.
        assert!(!c.carve(0, 0, 0, 0, 0, 0));
        assert!((c.fill_ratio(0, 0, 0) - 511.0 / 512.0).abs() < 1e-6);
    }

    #[test]
    fn replacing_a_block_heals_its_damage() {
        let mut c = Chunk::new(ChunkPos::new(0, 0, 0));
        c.set(5, 5, 5, BlockId::STONE);
        c.carve(5, 5, 5, 0, 0, 0);
        assert!(c.has_damage());
        c.set(5, 5, 5, BlockId::DIRT);
        assert!(!c.has_damage());
        assert_eq!(c.fill_ratio(5, 5, 5), 1.0);
    }

    #[test]
    fn occupancy_summary_tracks_edits() {
        let mut c = Chunk::new(ChunkPos::new(0, 0, 0));
        assert!(c.is_empty());
        assert!(!c.is_uniform_opaque());

        let blocks = Box::new([BlockId::STONE; CHUNK_VOL]);
        let full = Chunk::from_blocks(ChunkPos::new(0, 0, 0), blocks);
        assert!(!full.is_empty());
        assert!(full.is_uniform_opaque());

        let mut full = full;
        full.set(0, 0, 0, BlockId::AIR);
        assert!(!full.is_uniform_opaque());
        assert!(!full.is_empty());

        c.set(0, 0, 0, BlockId::WATER);
        assert!(!c.is_empty(), "water is a block, just not an opaque one");
        assert!(!c.is_uniform_opaque());
    }
}
