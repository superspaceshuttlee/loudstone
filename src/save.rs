//! Binary save and load. `std` only -- no serde, no new dependencies.
//!
//! # What actually gets written
//!
//! Terrain is deterministic from the seed, so a save stores the seed and *only
//! the player's edits on top of it*. Writing generated chunks would make a save
//! hundreds of megabytes for a world nobody touched.
//!
//! The record of those edits is [`ChangeTracker`], and it is deliberately owned
//! by this module rather than scraped out of `World` at save time. Two reasons:
//!
//! 1. Chunks stream out as the player walks away. If the truth lived only in
//!    resident chunks, walking 300 blocks would erase the house you just built.
//! 2. Diffing a live chunk against freshly generated terrain would mean running
//!    worldgen again for every chunk on every save.
//!
//! So the main loop tells the tracker about every edit as it makes it, the
//! tracker is the durable record, and [`ChangeTracker::replay_chunk`] puts the
//! edits back when a chunk streams in again.
//!
//! # Format
//!
//! Little-endian throughout.
//!
//! ```text
//! "LDST"                       magic
//! u32                          version
//! u32                          seed
//! f32 f32 f32                  player position
//! f32 f32                      player yaw, pitch
//! f32                          player health
//! u8                           selected hotbar slot
//! u32                          occupied inventory slot count
//!   u8 u16 u8 u16                slot index, item id, count, durability
//! u32                          modified chunk count
//!   i32 i32 i32                  chunk position
//!   u32                          block edit count
//!     u16 u8                       index within chunk, block id
//!   u32                          carve mask count
//!     u16 [u8; 64]                 index within chunk, 512-bit mask
//! ```

use crate::block::BlockId;
use crate::inventory::{Inventory, ItemStack, SLOT_COUNT};
use crate::item::ItemId;
use glam::Vec3;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// File magic. Loading anything else is refused outright.
pub const MAGIC: [u8; 4] = *b"LDST";
/// Current format version. Bump on any layout change.
pub const VERSION: u32 = 1;

/// Bytes in one block's 512-bit sub-voxel mask.
pub const MASK_BYTES: usize = 64;

/// Blocks per chunk edge. Must match `config::CHUNK_SIZE`.
const CHUNK_SIZE: i32 = 16;
/// Blocks per chunk.
const CHUNK_VOL: usize = 16 * 16 * 16;
/// Sub-voxels per block edge. Must match `config::SUBVOX`.
const SUBVOX: usize = 8;

/// Seconds between autosaves. The caller keeps the clock; see [`should_autosave`].
pub const AUTOSAVE_INTERVAL_SECS: f32 = 120.0;

/// Whether enough time has passed to warrant an autosave. Pure predicate: it
/// starts no threads and keeps no state, so the caller owns the timer and
/// resets it after a successful save.
pub fn should_autosave(elapsed: f32) -> bool {
    elapsed >= AUTOSAVE_INTERVAL_SECS
}

/// Default location of the world file, relative to the working directory.
pub fn default_save_path() -> PathBuf {
    PathBuf::from("saves").join("world.lsw")
}

// --- sub-voxel mask helpers --------------------------------------------------
//
// Bit layout matches `chunk::SubMask` exactly: bit `(y * 8 + z) * 8 + x`, stored
// little-endian within each byte. Masks round-trip byte for byte.

/// An undamaged block: every sub-voxel solid.
pub const FULL_MASK: [u8; MASK_BYTES] = [0xFF; MASK_BYTES];

#[inline]
fn mask_bit(sx: usize, sy: usize, sz: usize) -> usize {
    (sy * SUBVOX + sz) * SUBVOX + sx
}

/// True when that sub-voxel is still solid.
#[inline]
pub fn mask_get(mask: &[u8; MASK_BYTES], sx: usize, sy: usize, sz: usize) -> bool {
    let i = mask_bit(sx, sy, sz);
    mask[i >> 3] & (1 << (i & 7)) != 0
}

/// Carve one sub-voxel out of a mask.
#[inline]
pub fn mask_clear(mask: &mut [u8; MASK_BYTES], sx: usize, sy: usize, sz: usize) {
    let i = mask_bit(sx, sy, sz);
    mask[i >> 3] &= !(1 << (i & 7));
}

/// True when nothing solid is left.
#[inline]
pub fn mask_is_empty(mask: &[u8; MASK_BYTES]) -> bool {
    mask.iter().all(|&b| b == 0)
}

// --- chunk-local addressing --------------------------------------------------

/// A chunk position in chunk space. Plain tuple so this module stays free of
/// engine types.
pub type ChunkKey = (i32, i32, i32);

#[inline]
fn split(v: i32) -> (i32, usize) {
    (v.div_euclid(CHUNK_SIZE), v.rem_euclid(CHUNK_SIZE) as usize)
}

/// Chunk key and index-within-chunk for a world block coordinate.
#[inline]
pub fn locate(x: i32, y: i32, z: i32) -> (ChunkKey, u16) {
    let (cx, lx) = split(x);
    let (cy, ly) = split(y);
    let (cz, lz) = split(z);
    ((cx, cy, cz), local_index(lx, ly, lz))
}

/// Index layout matches `chunk::local_index`.
#[inline]
fn local_index(lx: usize, ly: usize, lz: usize) -> u16 {
    ((ly * 16 + lz) * 16 + lx) as u16
}

/// World block coordinate of an index within a chunk.
#[inline]
pub fn unlocate(chunk: ChunkKey, index: u16) -> (i32, i32, i32) {
    let i = index as usize;
    let lx = (i % 16) as i32;
    let lz = ((i / 16) % 16) as i32;
    let ly = (i / 256) as i32;
    (
        chunk.0 * CHUNK_SIZE + lx,
        chunk.1 * CHUNK_SIZE + ly,
        chunk.2 * CHUNK_SIZE + lz,
    )
}

// --- the world-write trait ---------------------------------------------------

/// The only thing this module needs from the engine's world: the ability to put
/// edits back. The integrator implements it for `World` in a few delegating
/// lines; nothing here ever names a concrete world type.
pub trait WorldEdit {
    /// Set a block, as `World::set_block`.
    fn set_block_at(&mut self, x: i32, y: i32, z: i32, id: BlockId);

    /// Clear one sub-voxel, as `World::carve`. Returns true if that emptied the
    /// block entirely.
    fn carve_at(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool;

    /// Install a whole carve mask at once.
    ///
    /// The default replays `carve_at` for every cleared sub-voxel, which is
    /// correct against the frozen `World` API but does up to 512 calls per
    /// damaged block. Override it if chunk storage can drop a mask in directly.
    fn set_mask_at(&mut self, x: i32, y: i32, z: i32, mask: &[u8; MASK_BYTES]) {
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..SUBVOX {
                    if !mask_get(mask, sx, sy, sz) {
                        self.carve_at(x, y, z, sx, sy, sz);
                    }
                }
            }
        }
    }
}

// --- the edit record ---------------------------------------------------------

/// Everything the player changed inside one chunk.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkDelta {
    /// Blocks whose identity was changed, by index within the chunk.
    pub blocks: HashMap<u16, BlockId>,
    /// Partially carved blocks, by index within the chunk. A block that is
    /// fully carved leaves this map and appears in `blocks` as air.
    pub masks: HashMap<u16, [u8; MASK_BYTES]>,
}

impl ChunkDelta {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.masks.is_empty()
    }
}

/// The durable record of every player edit, and the only thing a save writes
/// about the world.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeTracker {
    chunks: HashMap<ChunkKey, ChunkDelta>,
}

impl ChangeTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of chunks holding at least one edit. This is exactly how many
    /// chunks a save writes.
    pub fn modified_chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn chunks(&self) -> impl Iterator<Item = (&ChunkKey, &ChunkDelta)> {
        self.chunks.iter()
    }

    pub fn chunk(&self, key: ChunkKey) -> Option<&ChunkDelta> {
        self.chunks.get(&key)
    }

    /// Forget everything. Only useful when starting a new world.
    pub fn clear(&mut self) {
        self.chunks.clear();
    }

    /// Record a block change. Call this right after `World::set_block`.
    ///
    /// Mirrors the engine's rule that changing a block's identity resets any
    /// carving on it.
    pub fn note_set_block(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        let (key, index) = locate(x, y, z);
        let delta = self.chunks.entry(key).or_default();
        delta.blocks.insert(index, id);
        delta.masks.remove(&index);
    }

    /// Record one carved sub-voxel. Call this only for carves the world
    /// actually performed -- i.e. when `World::carve` was called on a solid
    /// block. Returns true if this carve emptied the block, matching what
    /// `World::carve` returns.
    pub fn note_carve(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        sx: usize,
        sy: usize,
        sz: usize,
    ) -> bool {
        let (key, index) = locate(x, y, z);
        let delta = self.chunks.entry(key).or_default();
        let mask = delta.masks.entry(index).or_insert(FULL_MASK);
        mask_clear(mask, sx, sy, sz);
        if mask_is_empty(mask) {
            delta.masks.remove(&index);
            delta.blocks.insert(index, BlockId::AIR);
            true
        } else {
            false
        }
    }

    /// The carve mask of a block, or `None` if it is undamaged.
    pub fn mask_at(&self, x: i32, y: i32, z: i32) -> Option<&[u8; MASK_BYTES]> {
        let (key, index) = locate(x, y, z);
        self.chunks.get(&key)?.masks.get(&index)
    }

    /// The recorded block override at a coordinate, if the player changed it.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let (key, index) = locate(x, y, z);
        self.chunks.get(&key)?.blocks.get(&index).copied()
    }

    /// Push every recorded edit into the world. Used once after load.
    pub fn replay<W: WorldEdit + ?Sized>(&self, world: &mut W) {
        let keys: Vec<ChunkKey> = self.chunks.keys().copied().collect();
        for key in keys {
            self.replay_chunk(key, world);
        }
    }

    /// Push one chunk's edits into the world. Call this whenever a chunk
    /// finishes generating, so streaming a chunk back in does not undo the
    /// player's work.
    ///
    /// Blocks go first, then masks: setting a block clears its mask in the
    /// engine, so the reverse order would throw the carving away.
    pub fn replay_chunk<W: WorldEdit + ?Sized>(&self, key: ChunkKey, world: &mut W) {
        let Some(delta) = self.chunks.get(&key) else {
            return;
        };
        for (&index, &id) in &delta.blocks {
            let (x, y, z) = unlocate(key, index);
            world.set_block_at(x, y, z, id);
        }
        for (&index, mask) in &delta.masks {
            let (x, y, z) = unlocate(key, index);
            world.set_mask_at(x, y, z, mask);
        }
    }
}

// --- what a save holds -------------------------------------------------------

/// Player state that survives a quit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerSave {
    pub pos: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
}

impl Default for PlayerSave {
    fn default() -> Self {
        Self {
            pos: Vec3::new(0.0, 80.0, 0.0),
            yaw: 0.0,
            pitch: 0.0,
            health: 20.0,
        }
    }
}

/// Everything a world file contains.
#[derive(Clone, Debug)]
pub struct SaveData {
    pub seed: u32,
    pub player: PlayerSave,
    pub inventory: Inventory,
    pub edits: ChangeTracker,
}

impl SaveData {
    /// A fresh world.
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            player: PlayerSave::default(),
            inventory: Inventory::new(),
            edits: ChangeTracker::new(),
        }
    }
}

// --- errors ------------------------------------------------------------------

/// Everything that can go wrong reading or writing a save. Load never panics.
#[derive(Debug)]
pub enum SaveError {
    Io(std::io::Error),
    /// The file is not a Loudstone save at all.
    BadMagic([u8; 4]),
    /// A save from a version this build does not understand.
    UnsupportedVersion { found: u32, supported: u32 },
    /// The file ended in the middle of a record.
    Truncated { needed: usize, available: usize },
    /// Structurally intact but semantically impossible.
    Corrupt(&'static str),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "save file I/O error: {e}"),
            SaveError::BadMagic(got) => write!(
                f,
                "not a Loudstone save: expected magic {:?}, found {:?}",
                String::from_utf8_lossy(&MAGIC),
                String::from_utf8_lossy(got)
            ),
            SaveError::UnsupportedVersion { found, supported } => write!(
                f,
                "save format version {found} is not supported by this build (which reads version {supported})"
            ),
            SaveError::Truncated { needed, available } => write!(
                f,
                "save file ends early: needed {needed} more bytes, {available} remain"
            ),
            SaveError::Corrupt(what) => write!(f, "corrupt save file: {what}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SaveError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}

// --- writing -----------------------------------------------------------------

/// Serialize a save into a byte buffer.
pub fn save_to_bytes(data: &SaveData) -> Vec<u8> {
    let mut out = Vec::with_capacity(1024);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&data.seed.to_le_bytes());

    let p = &data.player;
    for f in [p.pos.x, p.pos.y, p.pos.z, p.yaw, p.pitch, p.health] {
        out.extend_from_slice(&f.to_le_bytes());
    }

    out.push(data.inventory.selected() as u8);
    let occupied: Vec<(usize, ItemStack)> = data
        .inventory
        .slots()
        .iter()
        .enumerate()
        .filter_map(|(i, s)| s.map(|s| (i, s)))
        .collect();
    out.extend_from_slice(&(occupied.len() as u32).to_le_bytes());
    for (index, stack) in occupied {
        out.push(index as u8);
        out.extend_from_slice(&stack.item.0.to_le_bytes());
        out.push(stack.count);
        out.extend_from_slice(&stack.durability.to_le_bytes());
    }

    out.extend_from_slice(&(data.edits.modified_chunk_count() as u32).to_le_bytes());
    // Sorted so a save is byte-identical for identical state, which makes
    // "did anything change" checks and test diffs meaningful.
    let mut keys: Vec<ChunkKey> = data.edits.chunks().map(|(k, _)| *k).collect();
    keys.sort_unstable();
    for key in keys {
        let delta = &data.edits.chunks[&key];
        out.extend_from_slice(&key.0.to_le_bytes());
        out.extend_from_slice(&key.1.to_le_bytes());
        out.extend_from_slice(&key.2.to_le_bytes());

        let mut block_keys: Vec<u16> = delta.blocks.keys().copied().collect();
        block_keys.sort_unstable();
        out.extend_from_slice(&(block_keys.len() as u32).to_le_bytes());
        for index in block_keys {
            out.extend_from_slice(&index.to_le_bytes());
            out.push(delta.blocks[&index].0);
        }

        let mut mask_keys: Vec<u16> = delta.masks.keys().copied().collect();
        mask_keys.sort_unstable();
        out.extend_from_slice(&(mask_keys.len() as u32).to_le_bytes());
        for index in mask_keys {
            out.extend_from_slice(&index.to_le_bytes());
            out.extend_from_slice(&delta.masks[&index]);
        }
    }
    out
}

/// Write a save to disk, creating parent directories as needed.
///
/// Writes to a sibling `.tmp` file and renames over the target, so a crash
/// mid-write leaves the previous save intact rather than a half-written one.
pub fn save_to_file(path: &Path, data: &SaveData) -> Result<(), SaveError> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, save_to_bytes(data))?;
    fs::rename(&tmp, path)?;
    Ok(())
}

// --- reading -----------------------------------------------------------------

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], SaveError> {
        let end = self.pos.checked_add(n).ok_or(SaveError::Corrupt("length overflow"))?;
        if end > self.data.len() {
            return Err(SaveError::Truncated {
                needed: n,
                available: self.data.len() - self.pos,
            });
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, SaveError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, SaveError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, SaveError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32, SaveError> {
        Ok(self.u32()? as i32)
    }

    fn f32(&mut self) -> Result<f32, SaveError> {
        let v = f32::from_bits(self.u32()?);
        if v.is_finite() {
            Ok(v)
        } else {
            Err(SaveError::Corrupt("non-finite float"))
        }
    }

    fn mask(&mut self) -> Result<[u8; MASK_BYTES], SaveError> {
        let bytes = self.take(MASK_BYTES)?;
        let mut out = [0u8; MASK_BYTES];
        out.copy_from_slice(bytes);
        Ok(out)
    }
}

/// Parse a save from bytes.
///
/// Rejects a wrong magic, an unknown version, a truncated file and impossible
/// values with a descriptive error. It never panics on malformed input.
pub fn load_from_bytes(bytes: &[u8]) -> Result<SaveData, SaveError> {
    let mut r = Reader::new(bytes);

    let magic = r.take(4)?;
    if magic != MAGIC {
        let mut got = [0u8; 4];
        got.copy_from_slice(magic);
        return Err(SaveError::BadMagic(got));
    }

    let version = r.u32()?;
    if version != VERSION {
        return Err(SaveError::UnsupportedVersion {
            found: version,
            supported: VERSION,
        });
    }

    let seed = r.u32()?;
    let player = PlayerSave {
        pos: Vec3::new(r.f32()?, r.f32()?, r.f32()?),
        yaw: r.f32()?,
        pitch: r.f32()?,
        health: r.f32()?,
    };

    let selected = r.u8()? as usize;
    let mut inventory = Inventory::new();
    inventory.set_selected(selected);
    let slot_records = r.u32()? as usize;
    if slot_records > SLOT_COUNT {
        return Err(SaveError::Corrupt("more inventory slots than exist"));
    }
    for _ in 0..slot_records {
        let index = r.u8()? as usize;
        let item = ItemId(r.u16()?);
        let count = r.u8()?;
        let durability = r.u16()?;
        if index >= SLOT_COUNT {
            return Err(SaveError::Corrupt("inventory slot index out of range"));
        }
        if !item.is_valid() {
            return Err(SaveError::Corrupt("unknown item id"));
        }
        if count == 0 || count > item.max_stack() {
            return Err(SaveError::Corrupt("impossible stack size"));
        }
        if inventory.slot(index).is_some() {
            return Err(SaveError::Corrupt("duplicate inventory slot"));
        }
        inventory.set_slot(
            index,
            Some(ItemStack {
                item,
                count,
                durability: durability.min(item.max_durability()),
            }),
        );
    }

    let chunk_count = r.u32()? as usize;
    let mut edits = ChangeTracker::new();
    for _ in 0..chunk_count {
        let key: ChunkKey = (r.i32()?, r.i32()?, r.i32()?);
        let mut delta = ChunkDelta::default();

        let block_count = r.u32()? as usize;
        if block_count > CHUNK_VOL {
            return Err(SaveError::Corrupt("more block edits than a chunk holds"));
        }
        for _ in 0..block_count {
            let index = r.u16()?;
            let id = BlockId(r.u8()?);
            if index as usize >= CHUNK_VOL {
                return Err(SaveError::Corrupt("block index outside chunk"));
            }
            delta.blocks.insert(index, id);
        }

        let mask_count = r.u32()? as usize;
        if mask_count > CHUNK_VOL {
            return Err(SaveError::Corrupt("more carve masks than a chunk holds"));
        }
        for _ in 0..mask_count {
            let index = r.u16()?;
            let mask = r.mask()?;
            if index as usize >= CHUNK_VOL {
                return Err(SaveError::Corrupt("mask index outside chunk"));
            }
            if mask_is_empty(&mask) {
                // An empty mask means the block is gone; it belongs in `blocks`
                // as air, not here. Reject rather than resurrect a ghost block.
                return Err(SaveError::Corrupt("empty carve mask"));
            }
            delta.masks.insert(index, mask);
        }

        if !delta.is_empty() {
            edits.chunks.insert(key, delta);
        }
    }

    Ok(SaveData {
        seed,
        player,
        inventory,
        edits,
    })
}

/// Read a save from disk.
pub fn load_from_file(path: &Path) -> Result<SaveData, SaveError> {
    let bytes = fs::read(path)?;
    load_from_bytes(&bytes)
}

/// Whether a save exists at this path.
pub fn save_exists(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::{BLOCK_TORCH, ItemId};

    /// A world that stores only what it is told, exercising the *default*
    /// `set_mask_at` (which replays carves one sub-voxel at a time).
    #[derive(Default)]
    struct CarveWorld {
        blocks: HashMap<(i32, i32, i32), BlockId>,
        masks: HashMap<(i32, i32, i32), [u8; MASK_BYTES]>,
    }

    impl WorldEdit for CarveWorld {
        fn set_block_at(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
            self.blocks.insert((x, y, z), id);
            self.masks.remove(&(x, y, z));
        }
        fn carve_at(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
            let mask = self.masks.entry((x, y, z)).or_insert(FULL_MASK);
            mask_clear(mask, sx, sy, sz);
            if mask_is_empty(mask) {
                self.masks.remove(&(x, y, z));
                self.blocks.insert((x, y, z), BlockId::AIR);
                return true;
            }
            false
        }
    }

    /// The same world, but installing masks wholesale via an override.
    #[derive(Default)]
    struct FastWorld {
        blocks: HashMap<(i32, i32, i32), BlockId>,
        masks: HashMap<(i32, i32, i32), [u8; MASK_BYTES]>,
    }

    impl WorldEdit for FastWorld {
        fn set_block_at(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
            self.blocks.insert((x, y, z), id);
            self.masks.remove(&(x, y, z));
        }
        fn carve_at(&mut self, _x: i32, _y: i32, _z: i32, _: usize, _: usize, _: usize) -> bool {
            unreachable!("FastWorld installs masks directly")
        }
        fn set_mask_at(&mut self, x: i32, y: i32, z: i32, mask: &[u8; MASK_BYTES]) {
            self.masks.insert((x, y, z), *mask);
        }
    }

    /// A save with both kinds of world edit plus a full player state.
    fn populated() -> SaveData {
        let mut data = SaveData::new(0xC0FFEE);
        data.player = PlayerSave {
            pos: Vec3::new(12.5, 71.25, -300.75),
            yaw: 1.75,
            pitch: -0.5,
            health: 13.5,
        };
        data.inventory.add_item(ItemId::COBBLESTONE, 100);
        data.inventory
            .add(ItemStack::worn(ItemId::STONE_PICKAXE, 77));
        data.inventory.add_item(ItemId::TORCH, 12);
        data.inventory.set_selected(3);

        // Whole blocks placed and removed, spread over several chunks.
        data.edits.note_set_block(5, 70, 5, BlockId::PLANKS);
        data.edits.note_set_block(5, 71, 5, BlockId::AIR);
        data.edits.note_set_block(-40, 12, 300, BLOCK_TORCH);
        data.edits.note_set_block(200, 250, -200, BlockId::COBBLESTONE);

        // A block chipped a little, and one chipped a lot but still standing.
        for sx in 0..4 {
            data.edits.note_carve(9, 64, 9, sx, 0, 0);
        }
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..7 {
                    data.edits.note_carve(-1, 33, -1, sx, sy, sz);
                }
            }
        }
        data
    }

    fn assert_same(a: &SaveData, b: &SaveData) {
        assert_eq!(a.seed, b.seed);
        assert_eq!(a.player, b.player);
        assert_eq!(a.inventory.selected(), b.inventory.selected());
        assert_eq!(a.inventory.slots(), b.inventory.slots());
        assert_eq!(a.edits, b.edits);
    }

    #[test]
    fn empty_world_round_trips() {
        let data = SaveData::new(42);
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
        assert_same(&data, &back);
        assert!(back.edits.is_empty());
        assert!(back.inventory.is_empty());
    }

    #[test]
    fn full_world_round_trips_exactly() {
        let data = populated();
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
        assert_same(&data, &back);
    }

    #[test]
    fn carve_masks_survive_bit_for_bit() {
        let data = populated();
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();

        let light = back.edits.mask_at(9, 64, 9).expect("lightly chipped block");
        for sx in 0..SUBVOX {
            assert_eq!(mask_get(light, sx, 0, 0), sx >= 4, "sub-voxel {sx} wrong");
        }
        assert_eq!(back.edits.block_at(9, 64, 9), None, "block still stands");

        let heavy = back.edits.mask_at(-1, 33, -1).expect("heavily chipped block");
        let solid = heavy.iter().map(|b| b.count_ones()).sum::<u32>();
        assert_eq!(solid, 64, "one column of 8x8 should remain");
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                assert!(mask_get(heavy, 7, sy, sz));
                assert!(!mask_get(heavy, 0, sy, sz));
            }
        }
    }

    #[test]
    fn a_fully_carved_block_becomes_air_and_drops_its_mask() {
        let mut t = ChangeTracker::new();
        let mut destroyed = false;
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..SUBVOX {
                    destroyed = t.note_carve(3, 3, 3, sx, sy, sz);
                }
            }
        }
        assert!(destroyed, "the last sub-voxel destroys the block");
        assert_eq!(t.mask_at(3, 3, 3), None);
        assert_eq!(t.block_at(3, 3, 3), Some(BlockId::AIR));

        let back = load_from_bytes(&save_to_bytes(&SaveData {
            seed: 1,
            player: PlayerSave::default(),
            inventory: Inventory::new(),
            edits: t.clone(),
        }))
        .unwrap();
        assert_eq!(back.edits, t);
    }

    #[test]
    fn placing_a_block_clears_its_carve_mask() {
        let mut t = ChangeTracker::new();
        t.note_carve(1, 2, 3, 0, 0, 0);
        assert!(t.mask_at(1, 2, 3).is_some());
        t.note_set_block(1, 2, 3, BlockId::STONE);
        assert_eq!(t.mask_at(1, 2, 3), None);
        assert_eq!(t.block_at(1, 2, 3), Some(BlockId::STONE));
    }

    #[test]
    fn only_touched_chunks_are_written() {
        let mut t = ChangeTracker::new();
        assert_eq!(t.modified_chunk_count(), 0);
        // Three edits, but only two distinct chunks.
        t.note_set_block(0, 0, 0, BlockId::STONE);
        t.note_set_block(15, 15, 15, BlockId::STONE);
        t.note_set_block(16, 0, 0, BlockId::STONE);
        assert_eq!(t.modified_chunk_count(), 2);

        let bytes = save_to_bytes(&SaveData {
            seed: 7,
            player: PlayerSave::default(),
            inventory: Inventory::new(),
            edits: t,
        });
        // Header + no inventory + two small chunk records. A save that stored
        // generated chunks would be orders of magnitude bigger than this.
        assert!(bytes.len() < 128, "save was {} bytes", bytes.len());
    }

    #[test]
    fn negative_coordinates_map_to_the_right_chunk() {
        for (x, y, z) in [
            (0, 0, 0),
            (-1, -1, -1),
            (15, 15, 15),
            (16, 32, -16),
            (-17, 200, 4095),
            (-4096, 1, -1),
        ] {
            let (key, index) = locate(x, y, z);
            assert_eq!(unlocate(key, index), (x, y, z), "round trip failed");
            assert!((index as usize) < CHUNK_VOL);
        }
    }

    #[test]
    fn replay_rebuilds_the_world_through_carves() {
        let data = populated();
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();

        let mut world = CarveWorld::default();
        back.edits.replay(&mut world);

        assert_eq!(world.blocks.get(&(5, 70, 5)), Some(&BlockId::PLANKS));
        assert_eq!(world.blocks.get(&(5, 71, 5)), Some(&BlockId::AIR));
        assert_eq!(world.blocks.get(&(200, 250, -200)), Some(&BlockId::COBBLESTONE));

        let mask = world.masks.get(&(9, 64, 9)).expect("chipped block restored");
        assert_eq!(mask, back.edits.mask_at(9, 64, 9).unwrap());
        let heavy = world.masks.get(&(-1, 33, -1)).unwrap();
        assert_eq!(heavy, back.edits.mask_at(-1, 33, -1).unwrap());
    }

    #[test]
    fn the_fast_mask_path_agrees_with_the_carve_path() {
        let data = populated();
        let mut slow = CarveWorld::default();
        let mut fast = FastWorld::default();
        data.edits.replay(&mut slow);
        data.edits.replay(&mut fast);
        assert_eq!(slow.blocks, fast.blocks);
        assert_eq!(slow.masks, fast.masks);
    }

    #[test]
    fn replay_chunk_restores_one_chunk_only() {
        let data = populated();
        let mut world = CarveWorld::default();
        data.edits.replay_chunk((0, 4, 0), &mut world);
        assert_eq!(world.blocks.get(&(5, 70, 5)), Some(&BlockId::PLANKS));
        assert!(!world.blocks.contains_key(&(200, 250, -200)));
        // An untouched chunk is a no-op, not a panic.
        data.edits.replay_chunk((999, 0, 999), &mut world);
    }

    #[test]
    fn bad_magic_is_rejected() {
        let mut bytes = save_to_bytes(&SaveData::new(1));
        bytes[0..4].copy_from_slice(b"XXXX");
        match load_from_bytes(&bytes) {
            Err(SaveError::BadMagic(got)) => assert_eq!(&got, b"XXXX"),
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }

    #[test]
    fn an_unknown_version_is_refused_with_a_clear_message() {
        let mut bytes = save_to_bytes(&SaveData::new(1));
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        let err = load_from_bytes(&bytes).unwrap_err();
        match err {
            SaveError::UnsupportedVersion { found, supported } => {
                assert_eq!(found, 99);
                assert_eq!(supported, VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        let text = err.to_string();
        assert!(text.contains("99"), "unhelpful message: {text}");
        assert!(text.contains("not supported"), "unhelpful message: {text}");
    }

    #[test]
    fn truncation_at_every_length_errors_instead_of_panicking() {
        let bytes = save_to_bytes(&populated());
        for cut in 0..bytes.len() {
            let err = load_from_bytes(&bytes[..cut]);
            assert!(err.is_err(), "a {cut}-byte prefix should not load");
        }
        assert!(load_from_bytes(&bytes).is_ok());
    }

    #[test]
    fn garbage_never_panics() {
        // Deterministic pseudo-random noise, including valid-magic prefixes.
        let mut state = 0x1234_5678u32;
        for _ in 0..500 {
            let mut buf = Vec::new();
            let len = (state % 300) as usize;
            for _ in 0..len {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                buf.push((state >> 16) as u8);
            }
            let _ = load_from_bytes(&buf);
            let mut with_magic = MAGIC.to_vec();
            with_magic.extend_from_slice(&VERSION.to_le_bytes());
            with_magic.extend_from_slice(&buf);
            let _ = load_from_bytes(&with_magic);
        }
    }

    #[test]
    fn impossible_inventory_data_is_rejected() {
        // A count of 200 cobblestone in one slot cannot happen.
        let mut data = SaveData::new(1);
        data.inventory.set_slot(0, Some(ItemStack::new(ItemId::DIRT, 64)));
        // The single slot record is the 6 bytes before the trailing chunk count:
        // index u8, item u16, count u8, durability u16.
        let record = save_to_bytes(&data).len() - 4 - 6;

        let mut bytes = save_to_bytes(&data);
        bytes[record + 3] = 200;
        assert!(matches!(
            load_from_bytes(&bytes),
            Err(SaveError::Corrupt("impossible stack size"))
        ));

        // An item id nothing defines.
        let mut bytes = save_to_bytes(&data);
        bytes[record + 1..record + 3].copy_from_slice(&9999u16.to_le_bytes());
        assert!(matches!(
            load_from_bytes(&bytes),
            Err(SaveError::Corrupt("unknown item id"))
        ));
    }

    #[test]
    fn tool_durability_survives_the_round_trip() {
        let mut data = SaveData::new(1);
        data.inventory
            .set_slot(4, Some(ItemStack::worn(ItemId::IRON_PICKAXE, 3)));
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
        let tool = back.inventory.slot(4).unwrap();
        assert_eq!(tool.item, ItemId::IRON_PICKAXE);
        assert_eq!(tool.durability, 3);
        assert_eq!(tool.count, 1);
    }

    #[test]
    fn a_save_is_byte_stable_for_identical_state() {
        let data = populated();
        assert_eq!(save_to_bytes(&data), save_to_bytes(&data));
        let round_tripped = load_from_bytes(&save_to_bytes(&data)).unwrap();
        assert_eq!(save_to_bytes(&data), save_to_bytes(&round_tripped));
    }

    #[test]
    fn file_round_trip() {
        let dir = std::env::temp_dir().join("loudstone_save_test");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("world.lsw");
        assert!(!save_exists(&path));

        let data = populated();
        save_to_file(&path, &data).unwrap();
        assert!(save_exists(&path));
        let back = load_from_file(&path).unwrap();
        assert_same(&data, &back);

        // Saving again over an existing file works and leaves no .tmp behind.
        save_to_file(&path, &data).unwrap();
        assert!(!path.with_extension("tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_an_io_error_not_a_panic() {
        let path = std::env::temp_dir().join("loudstone_definitely_not_here.lsw");
        let _ = fs::remove_file(&path);
        assert!(matches!(load_from_file(&path), Err(SaveError::Io(_))));
    }

    #[test]
    fn autosave_fires_on_the_interval() {
        assert!(!should_autosave(0.0));
        assert!(!should_autosave(AUTOSAVE_INTERVAL_SECS - 0.01));
        assert!(should_autosave(AUTOSAVE_INTERVAL_SECS));
        assert!(should_autosave(AUTOSAVE_INTERVAL_SECS * 10.0));
    }

    /// The Phase 4 acceptance criterion, end to end across all four modules:
    /// start from nothing, craft a stone pickaxe, mine iron, place a torch, die,
    /// respawn, quit, relaunch, find the world exactly as left.
    #[test]
    fn the_whole_survival_loop() {
        use crate::crafting::{Furnace, craft};
        use crate::item::{BLOCK_CRAFTING_TABLE, mining_drop};

        let mut data = SaveData::new(2024);

        // Punch a tree. Wood needs no tool.
        assert_eq!(mining_drop(None, BlockId::WOOD), Some(ItemId::WOOD));
        data.inventory.add_item(ItemId::WOOD, 2);

        // Wood -> planks.
        let mut grid = vec![data.inventory.take_from_slot(0, 1), None, None, None];
        let planks = craft(&mut grid).unwrap();
        assert_eq!((planks.item, planks.count), (ItemId::PLANKS, 4));
        data.inventory.add(planks);
        let mut grid = vec![data.inventory.take_from_slot(0, 1), None, None, None];
        data.inventory.add(craft(&mut grid).unwrap());
        assert_eq!(data.inventory.count(ItemId::PLANKS), 8);

        // Planks -> sticks, two stacked vertically in the 2x2 grid.
        data.inventory.remove(ItemId::PLANKS, 2);
        let mut grid = vec![
            Some(ItemStack::one(ItemId::PLANKS)),
            None,
            Some(ItemStack::one(ItemId::PLANKS)),
            None,
        ];
        let sticks = craft(&mut grid).unwrap();
        assert_eq!((sticks.item, sticks.count), (ItemId::STICK, 4));
        data.inventory.add(sticks);

        // Wooden pickaxe: three planks over two sticks.
        data.inventory.remove(ItemId::PLANKS, 3);
        data.inventory.remove(ItemId::STICK, 2);
        let mut grid = tool_grid(ItemId::PLANKS);
        let wooden = craft(&mut grid).unwrap();
        assert_eq!(wooden.item, ItemId::WOODEN_PICKAXE);
        assert_eq!(wooden.durability, 60);
        data.inventory.add(wooden);

        // Mine stone with it: cobblestone drops, the tool wears down.
        let pick_slot = find_slot(&data.inventory, ItemId::WOODEN_PICKAXE);
        data.inventory.set_selected(pick_slot);
        for i in 0..6 {
            let held = data.inventory.selected_item();
            let drop = mining_drop(held, BlockId::STONE).unwrap();
            assert_eq!(drop, ItemId::COBBLESTONE);
            data.inventory.add_item(drop, 1);
            assert!(
                !data.inventory.damage_selected(1),
                "a wooden pick should survive six blocks"
            );
            data.edits.note_set_block(10, 60 + i, 10, BlockId::AIR);
        }
        assert_eq!(data.inventory.slot(pick_slot).unwrap().durability, 54);
        assert_eq!(data.inventory.count(ItemId::COBBLESTONE), 6);

        // Stone pickaxe from that cobblestone.
        data.inventory.add_item(ItemId::STICK, 2);
        data.inventory.remove(ItemId::COBBLESTONE, 3);
        data.inventory.remove(ItemId::STICK, 2);
        let mut grid = tool_grid(ItemId::COBBLESTONE);
        let stone_pick = craft(&mut grid).unwrap();
        assert_eq!(stone_pick.item, ItemId::STONE_PICKAXE);
        data.inventory.add(stone_pick);

        // Iron needs that stone tier and nothing less.
        assert_eq!(
            mining_drop(Some(ItemId::WOODEN_PICKAXE), BlockId::IRON_ORE),
            None
        );
        let raw = mining_drop(Some(ItemId::STONE_PICKAXE), BlockId::IRON_ORE).unwrap();
        assert_eq!(raw, ItemId::RAW_IRON);
        data.inventory.add_item(raw, 2);
        data.edits.note_set_block(11, 40, 11, BlockId::AIR);

        // Coal, then smelt the iron into ingots.
        let coal = mining_drop(Some(ItemId::STONE_PICKAXE), BlockId::COAL_ORE).unwrap();
        data.inventory.add_item(coal, 4);
        let mut furnace = Furnace::new();
        furnace.input = Some(ItemStack::new(ItemId::RAW_IRON, 2));
        furnace.fuel = Some(ItemStack::new(ItemId::COAL, 1));
        for _ in 0..21 {
            furnace.tick(1.0);
        }
        let ingots = furnace.take_output().unwrap();
        assert_eq!((ingots.item, ingots.count), (ItemId::IRON_INGOT, 2));
        data.inventory.add(ingots);

        // Torches from coal over a stick.
        data.inventory.add_item(ItemId::STICK, 1);
        let mut grid = vec![
            Some(ItemStack::one(ItemId::COAL)),
            None,
            Some(ItemStack::one(ItemId::STICK)),
            None,
        ];
        let torches = craft(&mut grid).unwrap();
        assert_eq!((torches.item, torches.count), (ItemId::TORCH, 4));
        data.inventory.add(torches);

        // A crafting table too, so the 3x3 grid is reachable in play.
        let mut grid = vec![
            Some(ItemStack::one(ItemId::PLANKS)),
            Some(ItemStack::one(ItemId::PLANKS)),
            Some(ItemStack::one(ItemId::PLANKS)),
            Some(ItemStack::one(ItemId::PLANKS)),
        ];
        let table = craft(&mut grid).unwrap();
        assert_eq!(table.item, ItemId::CRAFTING_TABLE);
        assert_eq!(table.item.places(), Some(BLOCK_CRAFTING_TABLE));

        // Place a torch: spends one item, records one block edit.
        let torch_slot = find_slot(&data.inventory, ItemId::TORCH);
        let placed = data.inventory.take_from_slot(torch_slot, 1).unwrap();
        let block = placed.item.places().unwrap();
        assert_eq!(block, BLOCK_TORCH);
        data.edits.note_set_block(10, 60, 11, block);
        assert_eq!(data.inventory.count(ItemId::TORCH), 3);

        // Chip a wall without destroying it -- the signature mechanic.
        for sx in 0..5 {
            assert!(!data.edits.note_carve(12, 60, 12, sx, 3, 3));
        }
        assert!(data.edits.mask_at(12, 60, 12).is_some());

        // Die: the player drops everything.
        data.player.pos = Vec3::new(10.5, 61.0, 10.5);
        data.player.health = 0.0;
        let dropped = data.inventory.drop_all();
        assert!(!dropped.is_empty());
        assert!(data.inventory.is_empty(), "death empties the inventory");

        // Respawn, and pick a few things back up.
        data.player = PlayerSave {
            pos: Vec3::new(0.5, 72.0, 0.5),
            yaw: 0.0,
            pitch: 0.0,
            health: 20.0,
        };
        for stack in dropped.into_iter().take(3) {
            data.inventory.add(stack);
        }

        // Quit and relaunch.
        let reloaded = load_from_bytes(&save_to_bytes(&data)).unwrap();
        assert_same(&data, &reloaded);
        assert_eq!(reloaded.seed, 2024);
        assert_eq!(reloaded.player.health, 20.0);
        assert_eq!(reloaded.edits.block_at(10, 60, 11), Some(BLOCK_TORCH));
        assert_eq!(reloaded.edits.block_at(11, 40, 11), Some(BlockId::AIR));

        // The world comes back exactly as left, crater and all.
        let mut world = CarveWorld::default();
        reloaded.edits.replay(&mut world);
        assert_eq!(world.blocks.get(&(10, 60, 11)), Some(&BLOCK_TORCH));
        let crater = world.masks.get(&(12, 60, 12)).expect("the crater survived");
        for sx in 0..SUBVOX {
            assert_eq!(mask_get(crater, sx, 3, 3), sx >= 5);
        }
    }

    /// The standard pickaxe layout in a 3x3 grid, in `material`.
    fn tool_grid(material: ItemId) -> Vec<Option<ItemStack>> {
        vec![
            Some(ItemStack::one(material)),
            Some(ItemStack::one(material)),
            Some(ItemStack::one(material)),
            None,
            Some(ItemStack::one(ItemId::STICK)),
            None,
            None,
            Some(ItemStack::one(ItemId::STICK)),
            None,
        ]
    }

    fn find_slot(inv: &Inventory, item: ItemId) -> usize {
        inv.slots()
            .iter()
            .position(|s| s.is_some_and(|s| s.item == item))
            .unwrap_or_else(|| panic!("{} should be in the inventory", item.name()))
    }

    #[test]
    fn mask_layout_matches_the_engine() {
        // bit index (y * 8 + z) * 8 + x, little-endian within each byte.
        let mut m = FULL_MASK;
        mask_clear(&mut m, 0, 0, 0);
        assert_eq!(m[0], 0xFE);
        let mut m = FULL_MASK;
        mask_clear(&mut m, 7, 7, 7);
        assert_eq!(m[63], 0x7F);
        let mut m = FULL_MASK;
        mask_clear(&mut m, 0, 1, 0);
        assert_eq!(m[8], 0xFE, "y stride is 64 bits");
        let mut m = FULL_MASK;
        mask_clear(&mut m, 0, 0, 1);
        assert_eq!(m[1], 0xFE, "z stride is 8 bits");
    }
}
