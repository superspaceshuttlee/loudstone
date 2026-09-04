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

    /// Fraction of sub-voxels still solid, 0.0..=1.0.
    pub fn fill_ratio(&self) -> f32 {
        let bits: u32 = self.0.iter().map(|b| b.count_ones()).sum();
        bits as f32 / (SUBVOX * SUBVOX * SUBVOX) as f32
    }
}

pub struct Chunk {
    pub pos: ChunkPos,
    blocks: Box<[BlockId; CHUNK_VOL]>,
    /// Sparse: only blocks that have been chipped appear here.
    damage: HashMap<u16, SubMask>,
    /// Set when the block or damage data changed and the mesh is stale.
    pub dirty: bool,
    /// True once terrain generation has filled this chunk.
    pub generated: bool,
}

#[inline]
pub fn local_index(x: usize, y: usize, z: usize) -> u16 {
    ((y * CHUNK_SIZE + z) * CHUNK_SIZE + x) as u16
}

impl Chunk {
    pub fn new(pos: ChunkPos) -> Self {
        Self {
            pos,
            blocks: Box::new([BlockId::AIR; CHUNK_VOL]),
            damage: HashMap::new(),
            dirty: true,
            generated: false,
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        self.blocks[local_index(x, y, z) as usize]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) {
        let i = local_index(x, y, z);
        self.blocks[i as usize] = id;
        // Changing a block's identity resets any carving on it.
        self.damage.remove(&i);
        self.dirty = true;
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

    /// Clear a single sub-voxel. Returns true if the block was fully destroyed.
    pub fn carve(&mut self, x: usize, y: usize, z: usize, sx: usize, sy: usize, sz: usize) -> bool {
        let i = local_index(x, y, z);
        if self.blocks[i as usize].is_air() {
            return false;
        }
        let mask = self.damage.entry(i).or_insert_with(SubMask::full);
        mask.clear(sx, sy, sz);
        self.dirty = true;
        if mask.is_empty() {
            self.damage.remove(&i);
            self.blocks[i as usize] = BlockId::AIR;
            return true;
        }
        false
    }

    /// How intact a block is, 1.0 when untouched.
    #[inline]
    pub fn fill_ratio(&self, x: usize, y: usize, z: usize) -> f32 {
        match self.damage.get(&local_index(x, y, z)) {
            Some(m) => m.fill_ratio(),
            None => 1.0,
        }
    }
}
