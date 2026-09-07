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
//! # Live entities and containers
//!
//! Two things are *not* derivable from the seed plus the edit log: the mobs
//! standing around the player, and what is inside the containers the player
//! filled. Both are written too, from version 2 on.
//!
//! Mobs are stored as body state only -- kind, position, velocity, facing,
//! health. AI internals (paths, patience timers, a creeper's lit fuse) are
//! deliberately dropped: a reloaded mob starts idle and re-acquires the player
//! the same way a freshly spawned one does. Only mobs inside
//! [`MOB_SAVE_RADIUS`] of the player are written, which is the same radius the
//! mob system despawns at -- a mob further out would have been deleted anyway.
//!
//! Containers are stored generically: *a container at position P, of type T,
//! holding these slots and these numeric fields*. A furnace is
//! [`ContainerKind::FURNACE`] with three slots and three fields (burn left,
//! burn total, smelt progress). A chest is the same record with more slots and
//! no fields, so adding one needs no format change -- see
//! [`ContainerKind::CHEST`].
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
//! --- version 2 and later end here; version 1 files stop above ---
//! u32                          mob count
//!   u8                           kind (0 zombie, 1 skeleton, 2 creeper, 3 pig)
//!   f32 f32 f32                  position
//!   f32 f32 f32                  velocity
//!   f32                          yaw
//!   f32                          health
//!   u8                           flags: bit 0 = on ground
//! u32                          container count
//!   i32 i32 i32                  block position
//!   u16                          container kind
//!   u8                           slot count
//!   u8                           occupied slot count
//!     u8 u16 u8 u16                slot index, item id, count, durability
//!   u8                           numeric field count
//!     f32                          field value
//! ```
//!
//! ## Versioning
//!
//! A version 1 file has no mob or container sections at all; it loads with both
//! collections empty rather than being refused. Anything outside
//! [`MIN_VERSION`]`..=`[`VERSION`] is refused with
//! [`SaveError::UnsupportedVersion`] rather than misparsed.

use crate::content::block::BlockId;
use crate::content::crafting::{Furnace, FurnaceState};
use crate::content::inventory::{Inventory, ItemStack, SLOT_COUNT};
use crate::content::item::ItemId;
use crate::sim::mob::{DESPAWN_DISTANCE, Mob, MobKind, MobManager};
use glam::Vec3;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// File magic. Loading anything else is refused outright.
pub const MAGIC: [u8; 4] = *b"LDST";
/// Current format version -- what a save is written as. Bump on any layout
/// change, and teach [`load_from_bytes`] to read every older version.
pub const VERSION: u32 = 2;
/// Oldest format version this build still reads.
///
/// * **1** -- seed, player, inventory, world edits.
/// * **2** -- adds the mob and container sections.
pub const MIN_VERSION: u32 = 1;

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
    pub fn note_carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
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

// --- mobs --------------------------------------------------------------------

/// How far from the player a mob is still worth saving.
///
/// Deliberately the mob system's own despawn distance: a mob further out than
/// this is deleted on the next tick anyway, so writing it would only resurrect
/// something the game had already thrown away.
pub const MOB_SAVE_RADIUS: f32 = DESPAWN_DISTANCE;

/// One mob's body state, with none of its AI.
///
/// Paths, patience, chase memory and a creeper's lit fuse are all left out on
/// purpose. They are a fraction of a second of thinking that the mob redoes for
/// free, and persisting a half-burnt fuse across a reload would be a way to
/// blow the player up on the loading screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MobSave {
    pub kind: MobKind,
    /// Feet centre, in block units.
    pub pos: Vec3,
    pub vel: Vec3,
    /// Facing, radians, 0 along +X.
    pub yaw: f32,
    /// Always greater than zero; a dead mob is never written.
    pub health: f32,
    pub on_ground: bool,
}

impl MobSave {
    /// Snapshot a live mob.
    pub fn from_mob(m: &Mob) -> Self {
        Self {
            kind: m.kind,
            pos: m.pos,
            vel: m.vel,
            yaw: m.yaw,
            health: m.health,
            on_ground: m.on_ground,
        }
    }

    /// Put this mob back into a manager. Returns its new id -- ids are not
    /// persisted, because nothing outside a single run refers to one.
    pub fn spawn_into(&self, mobs: &mut MobManager) -> u32 {
        mobs.spawn_restored(
            self.kind,
            self.pos,
            self.vel,
            self.yaw,
            self.health,
            self.on_ground,
        )
    }
}

/// Snapshot every mob worth persisting: alive, and within [`MOB_SAVE_RADIUS`]
/// of the player.
pub fn nearby_mobs(mobs: &[Mob], player_pos: Vec3) -> Vec<MobSave> {
    mobs.iter()
        .filter(|m| m.health > 0.0 && (m.pos - player_pos).length() <= MOB_SAVE_RADIUS)
        .map(MobSave::from_mob)
        .collect()
}

/// Stable on-disk numbering for mob kinds. Owned here rather than in `mob.rs`
/// so reordering the enum cannot silently turn saved pigs into creepers.
fn mob_kind_code(kind: MobKind) -> u8 {
    match kind {
        MobKind::Zombie => 0,
        MobKind::Skeleton => 1,
        MobKind::Creeper => 2,
        MobKind::Pig => 3,
    }
}

fn mob_kind_from_code(code: u8) -> Option<MobKind> {
    Some(match code {
        0 => MobKind::Zombie,
        1 => MobKind::Skeleton,
        2 => MobKind::Creeper,
        3 => MobKind::Pig,
        _ => return None,
    })
}

// --- containers --------------------------------------------------------------

/// What kind of container sits at a block position.
///
/// An open `u16` rather than a closed enum: a build that grows a new container
/// type picks the next number and the format does not change. Kinds this build
/// does not recognise still load, so a save round-trips through an older
/// binary without losing them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContainerKind(pub u16);

impl ContainerKind {
    /// Three slots (input, fuel, output) and three fields.
    pub const FURNACE: ContainerKind = ContainerKind(1);
    /// Reserved. Slots only, no fields -- nothing else has to change when
    /// chests arrive.
    pub const CHEST: ContainerKind = ContainerKind(2);
}

/// Slots in a furnace, and what each one holds.
pub const FURNACE_SLOTS: usize = 3;
pub const FURNACE_INPUT: usize = 0;
pub const FURNACE_FUEL: usize = 1;
pub const FURNACE_OUTPUT: usize = 2;

/// Numeric fields in a furnace, in order.
pub const FURNACE_FIELDS: usize = 3;
/// Seconds of burn left in the fuel item currently alight.
pub const FURNACE_BURN_LEFT: usize = 0;
/// What that fuel item was worth when it was lit, for the flame gauge.
pub const FURNACE_BURN_TOTAL: usize = 1;
/// Seconds of smelting accumulated on the current input.
pub const FURNACE_PROGRESS: usize = 2;

/// Upper bounds, so a corrupt length cannot ask for an absurd allocation.
pub const MAX_CONTAINER_SLOTS: usize = 64;
pub const MAX_CONTAINER_FIELDS: usize = 16;

/// A block position in world space.
pub type BlockPos = (i32, i32, i32);

/// The contents of one container: some item slots and some numbers.
///
/// Intentionally not "a furnace". The slots and fields mean whatever `kind`
/// says they mean, which is what lets a chest -- or a dispenser, or a brewing
/// stand -- reuse the record without touching the file format.
#[derive(Clone, Debug, PartialEq)]
pub struct ContainerSave {
    pub kind: ContainerKind,
    /// One entry per slot, empty ones included, so slot indices stay stable.
    pub slots: Vec<Option<ItemStack>>,
    /// Free-form numbers whose meaning is fixed by `kind`.
    pub fields: Vec<f32>,
}

impl ContainerSave {
    /// An empty container with `slots` slots and `fields` zeroed numbers.
    pub fn new(kind: ContainerKind, slots: usize, fields: usize) -> Self {
        Self {
            kind,
            slots: vec![None; slots],
            fields: vec![0.0; fields],
        }
    }

    /// A furnace, laid out as [`FURNACE_INPUT`], [`FURNACE_FUEL`],
    /// [`FURNACE_OUTPUT`] and [`FURNACE_BURN_LEFT`], [`FURNACE_BURN_TOTAL`],
    /// [`FURNACE_PROGRESS`] describe. The three timings are in seconds.
    pub fn furnace(
        input: Option<ItemStack>,
        fuel: Option<ItemStack>,
        output: Option<ItemStack>,
        burn_left: f32,
        burn_total: f32,
        progress: f32,
    ) -> Self {
        let mut slots = vec![None; FURNACE_SLOTS];
        slots[FURNACE_INPUT] = input;
        slots[FURNACE_FUEL] = fuel;
        slots[FURNACE_OUTPUT] = output;

        let mut fields = vec![0.0; FURNACE_FIELDS];
        fields[FURNACE_BURN_LEFT] = burn_left;
        fields[FURNACE_BURN_TOTAL] = burn_total;
        fields[FURNACE_PROGRESS] = progress;

        Self {
            kind: ContainerKind::FURNACE,
            slots,
            fields,
        }
    }

    pub fn from_furnace(furnace: &Furnace) -> Self {
        let state = furnace.state();
        Self::furnace(
            state.input,
            state.fuel,
            state.output,
            state.burn_left,
            state.burn_total,
            state.progress,
        )
    }

    /// Convert a generic record into a live furnace. Unknown container kinds or
    /// malformed furnace layouts stay preserved in the save but are not exposed
    /// to the simulation.
    pub fn to_furnace(&self) -> Option<Furnace> {
        if self.kind != ContainerKind::FURNACE
            || self.slots.len() != FURNACE_SLOTS
            || self.fields.len() != FURNACE_FIELDS
            || self
                .fields
                .iter()
                .any(|value| !value.is_finite() || *value < 0.0)
        {
            return None;
        }
        Some(Furnace::from_state(FurnaceState {
            input: self.slot(FURNACE_INPUT),
            fuel: self.slot(FURNACE_FUEL),
            output: self.slot(FURNACE_OUTPUT),
            burn_left: self.burn_left(),
            burn_total: self.burn_total(),
            progress: self.progress(),
        }))
    }

    /// Contents of a slot, or `None` for an empty or out-of-range one.
    pub fn slot(&self, index: usize) -> Option<ItemStack> {
        self.slots.get(index).copied().flatten()
    }

    /// A numeric field, or 0.0 if this container does not have that many.
    pub fn field(&self, index: usize) -> f32 {
        self.fields.get(index).copied().unwrap_or(0.0)
    }

    /// Seconds of burn left. Meaningful for [`ContainerKind::FURNACE`].
    pub fn burn_left(&self) -> f32 {
        self.field(FURNACE_BURN_LEFT)
    }

    /// What the burning fuel item was worth, in seconds.
    pub fn burn_total(&self) -> f32 {
        self.field(FURNACE_BURN_TOTAL)
    }

    /// Seconds of smelting accumulated on the current input.
    pub fn progress(&self) -> f32 {
        self.field(FURNACE_PROGRESS)
    }

    /// Nothing in it and nothing happening -- not worth a save record.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none) && self.fields.iter().all(|&f| f == 0.0)
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
    /// Mobs near the player when the save was written. Empty in a version 1
    /// file, and empty in a fresh world.
    pub mobs: Vec<MobSave>,
    /// Container contents, keyed by the block the container occupies.
    pub containers: HashMap<BlockPos, ContainerSave>,
}

impl SaveData {
    /// A fresh world.
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            player: PlayerSave::default(),
            inventory: Inventory::new(),
            edits: ChangeTracker::new(),
            mobs: Vec::new(),
            containers: HashMap::new(),
        }
    }

    /// Replace the saved mob list with everything alive near the player.
    /// Call this immediately before writing, the same way the player's
    /// position is synced from the camera.
    pub fn capture_mobs(&mut self, mobs: &[Mob], player_pos: Vec3) {
        self.mobs = nearby_mobs(mobs, player_pos);
    }

    /// Store one container, or forget it if it holds nothing and is doing
    /// nothing -- an empty furnace the player opened once is not worth bytes.
    pub fn set_container(&mut self, pos: BlockPos, container: ContainerSave) {
        if container.is_empty() {
            self.containers.remove(&pos);
        } else {
            self.containers.insert(pos, container);
        }
    }

    /// The container saved at a block position, if any.
    pub fn container(&self, pos: BlockPos) -> Option<&ContainerSave> {
        self.containers.get(&pos)
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
    UnsupportedVersion {
        found: u32,
        supported: u32,
    },
    /// The file ended in the middle of a record.
    Truncated {
        needed: usize,
        available: usize,
    },
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
                "save format version {found} is not supported by this build \
                 (which reads versions {MIN_VERSION} to {supported})"
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
        write_slot(&mut out, index, stack);
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

    write_mobs(&mut out, &data.mobs);
    write_containers(&mut out, &data.containers);
    out
}

/// Write one item slot record: slot index, item, count, durability.
fn write_slot(out: &mut Vec<u8>, index: usize, stack: ItemStack) {
    out.push(index as u8);
    out.extend_from_slice(&stack.item.0.to_le_bytes());
    out.push(stack.count);
    out.extend_from_slice(&stack.durability.to_le_bytes());
}

fn write_mobs(out: &mut Vec<u8>, mobs: &[MobSave]) {
    out.extend_from_slice(&(mobs.len() as u32).to_le_bytes());
    for m in mobs {
        out.push(mob_kind_code(m.kind));
        for f in [
            m.pos.x, m.pos.y, m.pos.z, m.vel.x, m.vel.y, m.vel.z, m.yaw, m.health,
        ] {
            out.extend_from_slice(&f.to_le_bytes());
        }
        out.push(u8::from(m.on_ground));
    }
}

fn write_containers(out: &mut Vec<u8>, containers: &HashMap<BlockPos, ContainerSave>) {
    out.extend_from_slice(&(containers.len() as u32).to_le_bytes());
    // Sorted, so a save stays byte-identical for identical state.
    let mut keys: Vec<BlockPos> = containers.keys().copied().collect();
    keys.sort_unstable();
    for pos in keys {
        let c = &containers[&pos];
        out.extend_from_slice(&pos.0.to_le_bytes());
        out.extend_from_slice(&pos.1.to_le_bytes());
        out.extend_from_slice(&pos.2.to_le_bytes());
        out.extend_from_slice(&c.kind.0.to_le_bytes());

        // Clamped so an oversized container cannot produce a file this build
        // would then refuse to load. Nothing in the game builds one.
        debug_assert!(c.slots.len() <= MAX_CONTAINER_SLOTS, "container too wide");
        debug_assert!(c.fields.len() <= MAX_CONTAINER_FIELDS, "too many fields");
        let slots = c.slots.len().min(MAX_CONTAINER_SLOTS);
        let fields = c.fields.len().min(MAX_CONTAINER_FIELDS);

        out.push(slots as u8);
        let occupied: Vec<(usize, ItemStack)> = c.slots[..slots]
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|s| (i, s)))
            .collect();
        out.push(occupied.len() as u8);
        for (index, stack) in occupied {
            write_slot(out, index, stack);
        }

        out.push(fields as u8);
        for &f in &c.fields[..fields] {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
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
        let end = self
            .pos
            .checked_add(n)
            .ok_or(SaveError::Corrupt("length overflow"))?;
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
    if !(MIN_VERSION..=VERSION).contains(&version) {
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
        let (index, stack) = read_slot(&mut r)?;
        if index >= SLOT_COUNT {
            return Err(SaveError::Corrupt("inventory slot index out of range"));
        }
        if inventory.slot(index).is_some() {
            return Err(SaveError::Corrupt("duplicate inventory slot"));
        }
        inventory.set_slot(index, Some(stack));
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

    // Version 1 files stop here. They load as a world with no mobs and no
    // container contents, which is exactly what they described.
    let (mobs, containers) = if version >= 2 {
        (read_mobs(&mut r)?, read_containers(&mut r)?)
    } else {
        (Vec::new(), HashMap::new())
    };

    Ok(SaveData {
        seed,
        player,
        inventory,
        edits,
        mobs,
        containers,
    })
}

/// Read one item slot record. Returns the slot index and the stack.
fn read_slot(r: &mut Reader<'_>) -> Result<(usize, ItemStack), SaveError> {
    let index = r.u8()? as usize;
    let item = ItemId(r.u16()?);
    let count = r.u8()?;
    let durability = r.u16()?;
    if !item.is_valid() {
        return Err(SaveError::Corrupt("unknown item id"));
    }
    if count == 0 || count > item.max_stack() {
        return Err(SaveError::Corrupt("impossible stack size"));
    }
    Ok((
        index,
        ItemStack {
            item,
            count,
            durability: durability.min(item.max_durability()),
        },
    ))
}

fn read_mobs(r: &mut Reader<'_>) -> Result<Vec<MobSave>, SaveError> {
    let count = r.u32()? as usize;
    // No pre-allocation from an untrusted length: the loop simply runs out of
    // bytes and reports a truncation rather than reserving gigabytes first.
    let mut mobs = Vec::new();
    for _ in 0..count {
        let kind = mob_kind_from_code(r.u8()?).ok_or(SaveError::Corrupt("unknown mob kind"))?;
        let pos = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let vel = Vec3::new(r.f32()?, r.f32()?, r.f32()?);
        let yaw = r.f32()?;
        let health = r.f32()?;
        let flags = r.u8()?;
        if flags & !1 != 0 {
            return Err(SaveError::Corrupt("unknown mob flag bits"));
        }
        if health <= 0.0 {
            return Err(SaveError::Corrupt("mob saved with no health"));
        }
        mobs.push(MobSave {
            kind,
            pos,
            vel,
            yaw,
            // A mob whose kind was nerfed between builds comes back at the new
            // maximum rather than as an unkillable relic.
            health: health.min(kind.stats().max_health),
            on_ground: flags & 1 != 0,
        });
    }
    Ok(mobs)
}

fn read_containers(r: &mut Reader<'_>) -> Result<HashMap<BlockPos, ContainerSave>, SaveError> {
    let count = r.u32()? as usize;
    let mut containers: HashMap<BlockPos, ContainerSave> = HashMap::new();
    for _ in 0..count {
        let pos: BlockPos = (r.i32()?, r.i32()?, r.i32()?);
        // An unrecognised kind is kept, not rejected: container types are an
        // open set, so a record this build cannot interpret still survives.
        let kind = ContainerKind(r.u16()?);

        let slot_count = r.u8()? as usize;
        if slot_count > MAX_CONTAINER_SLOTS {
            return Err(SaveError::Corrupt("container has more slots than possible"));
        }
        let mut slots = vec![None; slot_count];
        let occupied = r.u8()? as usize;
        if occupied > slot_count {
            return Err(SaveError::Corrupt(
                "more filled slots than the container has",
            ));
        }
        for _ in 0..occupied {
            let (index, stack) = read_slot(r)?;
            if index >= slot_count {
                return Err(SaveError::Corrupt("container slot index out of range"));
            }
            if slots[index].is_some() {
                return Err(SaveError::Corrupt("duplicate container slot"));
            }
            slots[index] = Some(stack);
        }

        let field_count = r.u8()? as usize;
        if field_count > MAX_CONTAINER_FIELDS {
            return Err(SaveError::Corrupt(
                "container has more fields than possible",
            ));
        }
        let mut fields = Vec::with_capacity(field_count);
        for _ in 0..field_count {
            fields.push(r.f32()?);
        }

        if containers
            .insert(
                pos,
                ContainerSave {
                    kind,
                    slots,
                    fields,
                },
            )
            .is_some()
        {
            return Err(SaveError::Corrupt("two containers at one block"));
        }
    }
    Ok(containers)
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
    use crate::content::item::{BLOCK_TORCH, ItemId};

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
        data.edits
            .note_set_block(200, 250, -200, BlockId::COBBLESTONE);

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

        // One of every mob kind, with velocity, facing and part-worn health.
        data.mobs = vec![
            MobSave {
                kind: MobKind::Zombie,
                pos: Vec3::new(14.5, 71.0, -298.0),
                vel: Vec3::new(0.75, -1.25, -0.5),
                yaw: 2.25,
                health: 7.5,
                on_ground: true,
            },
            MobSave {
                kind: MobKind::Skeleton,
                pos: Vec3::new(-6.0, 64.0, 12.25),
                vel: Vec3::new(-2.0, 0.0, 3.5),
                yaw: -1.5,
                health: 16.0,
                on_ground: false,
            },
            MobSave {
                kind: MobKind::Creeper,
                pos: Vec3::new(0.25, 65.5, 0.75),
                vel: Vec3::ZERO,
                yaw: 0.0,
                health: 0.5,
                on_ground: true,
            },
            MobSave {
                kind: MobKind::Pig,
                pos: Vec3::new(-33.5, 70.0, 44.0),
                vel: Vec3::new(0.0, -9.5, 0.0),
                yaw: 3.0,
                health: 10.0,
                on_ground: false,
            },
        ];

        // A furnace mid-smelt, a furnace holding only fuel, and a chest --
        // three shapes of the same record.
        data.set_container(
            (5, 70, 6),
            ContainerSave::furnace(
                Some(ItemStack::new(ItemId::RAW_IRON, 5)),
                Some(ItemStack::new(ItemId::COAL, 3)),
                Some(ItemStack::new(ItemId::IRON_INGOT, 2)),
                43.5,
                80.0,
                6.25,
            ),
        );
        data.set_container(
            (-40, 12, 301),
            ContainerSave::furnace(
                None,
                Some(ItemStack::new(ItemId::PLANKS, 1)),
                None,
                0.0,
                0.0,
                0.0,
            ),
        );
        let mut chest = ContainerSave::new(ContainerKind::CHEST, 27, 0);
        chest.slots[0] = Some(ItemStack::new(ItemId::DIRT, 64));
        chest.slots[26] = Some(ItemStack::worn(ItemId::IRON_AXE, 12));
        data.set_container((200, 250, -201), chest);
        data
    }

    fn assert_same(a: &SaveData, b: &SaveData) {
        assert_eq!(a.seed, b.seed);
        assert_eq!(a.player, b.player);
        assert_eq!(a.inventory.selected(), b.inventory.selected());
        assert_eq!(a.inventory.slots(), b.inventory.slots());
        assert_eq!(a.edits, b.edits);
        assert_eq!(a.mobs, b.mobs);
        assert_eq!(a.containers, b.containers);
    }

    /// Bytes before the first inventory slot record: magic, version, seed, six
    /// player floats, the selected slot, and the occupied-slot count.
    const HEADER_BYTES: usize = 4 + 4 + 4 + 24 + 1 + 4;

    /// Bytes in one mob record: kind, pos, vel, yaw, health, flags.
    const MOB_RECORD: usize = 1 + 12 + 12 + 4 + 4 + 1;

    /// A hand-assembled version 1 file -- the format exactly as it shipped,
    /// with no mob or container sections. Built byte by byte rather than by
    /// calling today's writer, so a future format change cannot quietly
    /// redefine what "version 1" means and let this test keep passing.
    fn version_1_bytes() -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&1u32.to_le_bytes()); // version
        b.extend_from_slice(&4242u32.to_le_bytes()); // seed
        // pos x/y/z, yaw, pitch, health
        for f in [1.5f32, 70.0, -2.25, 0.75, -0.25, 17.5] {
            b.extend_from_slice(&f.to_le_bytes());
        }
        b.push(2); // selected hotbar slot
        b.extend_from_slice(&1u32.to_le_bytes()); // one occupied inventory slot
        b.push(5); // slot index
        b.extend_from_slice(&ItemId::COBBLESTONE.0.to_le_bytes());
        b.push(30); // count
        b.extend_from_slice(&0u16.to_le_bytes()); // durability
        b.extend_from_slice(&1u32.to_le_bytes()); // one modified chunk
        let (key, index) = locate(5, 70, 5);
        b.extend_from_slice(&key.0.to_le_bytes());
        b.extend_from_slice(&key.1.to_le_bytes());
        b.extend_from_slice(&key.2.to_le_bytes());
        b.extend_from_slice(&1u32.to_le_bytes()); // one block edit
        b.extend_from_slice(&index.to_le_bytes());
        b.push(BlockId::PLANKS.0);
        b.extend_from_slice(&0u32.to_le_bytes()); // no carve masks
        b
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

        let heavy = back
            .edits
            .mask_at(-1, 33, -1)
            .expect("heavily chipped block");
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

        let mut data = SaveData::new(1);
        data.edits = t.clone();
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
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

        let mut data = SaveData::new(7);
        data.edits = t;
        let bytes = save_to_bytes(&data);
        // Header + no inventory + two small chunk records + two empty
        // sections. A save that stored generated chunks would be orders of
        // magnitude bigger than this.
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
        assert_eq!(
            world.blocks.get(&(200, 250, -200)),
            Some(&BlockId::COBBLESTONE)
        );

        let mask = world
            .masks
            .get(&(9, 64, 9))
            .expect("chipped block restored");
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
        data.inventory
            .set_slot(0, Some(ItemStack::new(ItemId::DIRT, 64)));
        // The single slot record sits immediately after the fixed header:
        // index u8, item u16, count u8, durability u16.
        let record = HEADER_BYTES;

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
        use crate::content::crafting::{Furnace, craft};
        use crate::content::item::{BLOCK_CRAFTING_TABLE, mining_drop};

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

    // --- version 1 compatibility ---------------------------------------------

    #[test]
    fn a_version_1_file_still_loads_with_no_mobs_and_no_containers() {
        let data = load_from_bytes(&version_1_bytes()).expect("version 1 must still load");
        assert_eq!(data.seed, 4242);
        assert_eq!(data.player.pos, Vec3::new(1.5, 70.0, -2.25));
        assert_eq!(data.player.health, 17.5);
        assert_eq!(data.inventory.selected(), 2);
        assert_eq!(data.inventory.slot(5).unwrap().item, ItemId::COBBLESTONE);
        assert_eq!(data.inventory.slot(5).unwrap().count, 30);
        assert_eq!(data.edits.block_at(5, 70, 5), Some(BlockId::PLANKS));

        // The two sections a version 1 file does not have.
        assert!(data.mobs.is_empty(), "version 1 describes no mobs");
        assert!(
            data.containers.is_empty(),
            "version 1 describes no containers"
        );
    }

    #[test]
    fn loading_a_version_1_file_and_saving_it_upgrades_it_losslessly() {
        let old = load_from_bytes(&version_1_bytes()).unwrap();
        let bytes = save_to_bytes(&old);
        assert_eq!(
            u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            VERSION,
            "re-saving must stamp the current version"
        );
        let new = load_from_bytes(&bytes).unwrap();
        assert_same(&old, &new);
        // Exactly eight bytes larger: one empty mob count, one empty container
        // count. Nothing else about the layout moved.
        assert_eq!(bytes.len(), version_1_bytes().len() + 8);
    }

    #[test]
    fn a_truncated_version_1_file_errors_instead_of_panicking() {
        let bytes = version_1_bytes();
        for cut in 0..bytes.len() {
            assert!(
                load_from_bytes(&bytes[..cut]).is_err(),
                "a {cut}-byte prefix of a version 1 file should not load"
            );
        }
    }

    #[test]
    fn versions_outside_the_supported_range_are_still_refused() {
        for bad in [0u32, VERSION + 1, 99, u32::MAX] {
            let mut bytes = version_1_bytes();
            bytes[4..8].copy_from_slice(&bad.to_le_bytes());
            match load_from_bytes(&bytes) {
                Err(SaveError::UnsupportedVersion { found, supported }) => {
                    assert_eq!(found, bad);
                    assert_eq!(supported, VERSION);
                }
                other => panic!("version {bad} should be refused, got {other:?}"),
            }
        }
    }

    // --- mobs ----------------------------------------------------------------

    #[test]
    fn mobs_round_trip_exactly() {
        let data = populated();
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
        assert_eq!(back.mobs.len(), 4);
        assert_eq!(back.mobs, data.mobs);
        // Order is preserved, and so is every field of every mob.
        for (before, after) in data.mobs.iter().zip(&back.mobs) {
            assert_eq!(before.kind, after.kind);
            assert_eq!(before.pos, after.pos);
            assert_eq!(before.vel, after.vel);
            assert_eq!(before.yaw, after.yaw);
            assert_eq!(before.health, after.health);
            assert_eq!(before.on_ground, after.on_ground);
        }
    }

    #[test]
    fn a_reloaded_mob_comes_back_where_it_was_and_starts_idle() {
        use crate::sim::mob::MobState;

        let data = load_from_bytes(&save_to_bytes(&populated())).unwrap();
        let mut mgr = MobManager::new(1);
        mgr.spawning_enabled = false;
        for m in &data.mobs {
            m.spawn_into(&mut mgr);
        }

        assert_eq!(mgr.len(), data.mobs.len());
        for (saved, live) in data.mobs.iter().zip(mgr.mobs()) {
            assert_eq!(live.kind, saved.kind);
            assert_eq!(live.pos, saved.pos);
            assert_eq!(live.vel, saved.vel);
            assert_eq!(live.yaw, saved.yaw);
            assert_eq!(live.health, saved.health);
            assert_eq!(live.on_ground, saved.on_ground);
            // AI is deliberately not persisted: everything wakes up idle.
            assert_eq!(live.state, MobState::Idle);
            assert_eq!(live.fuse_fraction(), 0.0, "no creeper reloads mid-fuse");
        }
        // Ids are handed out fresh; nothing outside one run refers to one.
        let ids: Vec<u32> = mgr.mobs().iter().map(|m| m.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn only_mobs_near_the_player_are_saved() {
        let here = Vec3::new(100.0, 64.0, -100.0);
        let mut dead = Mob::new(4, MobKind::Zombie, here + Vec3::X);
        dead.health = 0.0;

        let live = vec![
            Mob::new(1, MobKind::Zombie, here + Vec3::new(5.0, 0.0, 0.0)),
            Mob::new(
                2,
                MobKind::Pig,
                here + Vec3::new(0.0, 0.0, MOB_SAVE_RADIUS - 1.0),
            ),
            // Beyond the despawn radius: the mob system would delete it on the
            // next tick anyway, so saving it would resurrect a ghost.
            Mob::new(
                3,
                MobKind::Creeper,
                here + Vec3::new(0.0, 0.0, MOB_SAVE_RADIUS + 1.0),
            ),
            dead,
        ];

        let saved = nearby_mobs(&live, here);
        assert_eq!(saved.len(), 2, "only the two nearby living mobs");
        assert_eq!(saved[0].kind, MobKind::Zombie);
        assert_eq!(saved[1].kind, MobKind::Pig);
        assert_eq!(MOB_SAVE_RADIUS, crate::sim::mob::DESPAWN_DISTANCE);
    }

    #[test]
    fn capture_mobs_fills_the_save_from_the_live_manager() {
        let mut mgr = MobManager::new(3);
        mgr.spawning_enabled = false;
        mgr.spawn(MobKind::Zombie, Vec3::new(1.0, 64.0, 1.0));
        mgr.spawn(MobKind::Pig, Vec3::new(0.0, 64.0, 400.0)); // far away

        let mut data = SaveData::new(1);
        data.capture_mobs(mgr.mobs(), Vec3::new(0.0, 64.0, 0.0));
        assert_eq!(data.mobs.len(), 1);
        assert_eq!(data.mobs[0].kind, MobKind::Zombie);
        assert_eq!(data.mobs[0].health, MobKind::Zombie.stats().max_health);
    }

    #[test]
    fn an_unknown_mob_kind_is_rejected_rather_than_guessed() {
        let mut data = SaveData::new(1);
        data.mobs = vec![MobSave {
            kind: MobKind::Pig,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            yaw: 0.0,
            health: 10.0,
            on_ground: true,
        }];
        // One mob, no containers: the record sits between the two counts.
        let kind_byte = save_to_bytes(&data).len() - 4 - MOB_RECORD;

        let mut bytes = save_to_bytes(&data);
        bytes[kind_byte] = 9;
        assert!(matches!(
            load_from_bytes(&bytes),
            Err(SaveError::Corrupt("unknown mob kind"))
        ));

        // Health of zero: a dead mob was never meant to be written.
        let mut bytes = save_to_bytes(&data);
        let health_at = kind_byte + 1 + 12 + 12 + 4;
        bytes[health_at..health_at + 4].copy_from_slice(&0.0f32.to_le_bytes());
        assert!(matches!(
            load_from_bytes(&bytes),
            Err(SaveError::Corrupt("mob saved with no health"))
        ));

        // Flag bits nothing defines.
        let mut bytes = save_to_bytes(&data);
        bytes[kind_byte + MOB_RECORD - 1] = 0xF0;
        assert!(matches!(
            load_from_bytes(&bytes),
            Err(SaveError::Corrupt("unknown mob flag bits"))
        ));
    }

    #[test]
    fn a_mob_over_its_kinds_maximum_health_is_capped_not_rejected() {
        let mut data = SaveData::new(1);
        data.mobs = vec![MobSave {
            kind: MobKind::Pig,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            yaw: 0.0,
            health: 10.0,
            on_ground: true,
        }];
        let mut bytes = save_to_bytes(&data);
        let health_at = bytes.len() - 4 - MOB_RECORD + 1 + 12 + 12 + 4;
        bytes[health_at..health_at + 4].copy_from_slice(&999.0f32.to_le_bytes());
        let back = load_from_bytes(&bytes).unwrap();
        assert_eq!(back.mobs[0].health, MobKind::Pig.stats().max_health);
    }

    // --- containers ----------------------------------------------------------

    #[test]
    fn containers_round_trip_exactly() {
        let data = populated();
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
        assert_eq!(back.containers.len(), 3);
        assert_eq!(back.containers, data.containers);

        let furnace = back.container((5, 70, 6)).expect("the busy furnace");
        assert_eq!(furnace.kind, ContainerKind::FURNACE);
        assert_eq!(
            furnace.slot(FURNACE_INPUT),
            Some(ItemStack::new(ItemId::RAW_IRON, 5))
        );
        assert_eq!(
            furnace.slot(FURNACE_FUEL),
            Some(ItemStack::new(ItemId::COAL, 3))
        );
        assert_eq!(
            furnace.slot(FURNACE_OUTPUT),
            Some(ItemStack::new(ItemId::IRON_INGOT, 2))
        );

        // Empty slots keep their index: the chest's item is still in slot 26.
        let chest = back.container((200, 250, -201)).expect("the chest");
        assert_eq!(chest.kind, ContainerKind::CHEST);
        assert_eq!(chest.slots.len(), 27);
        assert_eq!(chest.slot(0), Some(ItemStack::new(ItemId::DIRT, 64)));
        assert_eq!(chest.slot(25), None);
        assert_eq!(chest.slot(26).unwrap().durability, 12);
        assert!(chest.fields.is_empty(), "a chest needs no numbers");
    }

    /// The bug this section exists for: iron ore and coal in a furnace, quit
    /// mid-smelt, come back to find the ore still there and still cooking.
    #[test]
    fn a_furnace_saved_mid_smelt_reloads_with_its_burn_and_progress() {
        use crate::content::crafting::Furnace;

        let mut f = Furnace::new();
        f.input = Some(ItemStack::new(ItemId::RAW_IRON, 2));
        f.fuel = Some(ItemStack::new(ItemId::COAL, 3));
        for _ in 0..6 {
            f.tick(1.0);
        }

        // Six one-second ticks: the first lights a coal (80s, one consumed) and
        // banks a second of progress; the other five burn and smelt.
        let (burn_left, burn_total, progress) = (75.0f32, 80.0f32, 6.0f32);
        assert_eq!(f.burn_fraction(), burn_left / burn_total);
        assert_eq!(f.progress_fraction(), progress / 10.0);
        assert_eq!(f.fuel, Some(ItemStack::new(ItemId::COAL, 2)));
        assert!(f.output.is_none(), "ten seconds are needed for one ingot");

        let mut data = SaveData::new(9);
        data.set_container(
            (8, 63, -4),
            ContainerSave::furnace(f.input, f.fuel, f.output, burn_left, burn_total, progress),
        );

        // Quit and relaunch.
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
        let c = back.container((8, 63, -4)).expect("the furnace survived");

        // The ore and the coal are still in it -- the whole point.
        assert_eq!(
            c.slot(FURNACE_INPUT),
            Some(ItemStack::new(ItemId::RAW_IRON, 2))
        );
        assert_eq!(c.slot(FURNACE_FUEL), Some(ItemStack::new(ItemId::COAL, 2)));
        assert_eq!(c.slot(FURNACE_OUTPUT), None);

        // And so is the fire, to the second.
        assert_eq!(c.burn_left(), burn_left);
        assert_eq!(c.burn_total(), burn_total);
        assert_eq!(c.progress(), progress);
        // Which is to say: the gauges read exactly what they read before.
        assert_eq!(c.burn_left() / c.burn_total(), f.burn_fraction());
        assert_eq!(c.progress() / 10.0, f.progress_fraction());
    }

    #[test]
    fn furnace_container_conversion_preserves_live_state() {
        let mut before = crate::content::crafting::Furnace::new();
        before.input = Some(ItemStack::new(ItemId::RAW_IRON, 2));
        before.fuel = Some(ItemStack::new(ItemId::COAL, 1));
        before.tick(3.25);

        let saved = ContainerSave::from_furnace(&before);
        let after = saved.to_furnace().expect("a furnace record must restore");

        assert_eq!(after.input, before.input);
        assert_eq!(after.fuel, before.fuel);
        assert_eq!(after.output, before.output);
        assert_eq!(after.burn_fraction(), before.burn_fraction());
        assert_eq!(after.progress_fraction(), before.progress_fraction());
    }

    #[test]
    fn a_container_kind_this_build_does_not_know_still_survives() {
        // The reason containers carry an open kind number: a save written by a
        // build with dispensers must not lose them when read by one without.
        let mut odd = ContainerSave::new(ContainerKind(4321), 5, 2);
        odd.slots[3] = Some(ItemStack::new(ItemId::GOLD_ORE, 7));
        odd.fields[1] = -12.5;

        let mut data = SaveData::new(1);
        data.set_container((-9, 200, 9), odd.clone());
        let back = load_from_bytes(&save_to_bytes(&data)).unwrap();
        assert_eq!(back.container((-9, 200, 9)), Some(&odd));
    }

    #[test]
    fn an_empty_container_is_not_worth_a_record() {
        let mut data = SaveData::new(1);
        data.set_container((1, 2, 3), ContainerSave::new(ContainerKind::FURNACE, 3, 3));
        assert!(
            data.containers.is_empty(),
            "an idle empty furnace is not state"
        );

        // ...but one that is merely mid-burn with empty slots is.
        data.set_container(
            (1, 2, 3),
            ContainerSave::furnace(None, None, None, 4.0, 80.0, 0.0),
        );
        assert_eq!(data.containers.len(), 1);

        // Emptying it again drops the record rather than leaving a husk.
        data.set_container((1, 2, 3), ContainerSave::new(ContainerKind::FURNACE, 3, 3));
        assert!(data.containers.is_empty());
    }

    #[test]
    fn impossible_container_data_is_rejected() {
        let mut data = SaveData::new(1);
        let container = ContainerSave::furnace(
            Some(ItemStack::new(ItemId::RAW_IRON, 1)),
            None,
            None,
            1.0,
            2.0,
            3.0,
        );
        data.set_container((1, 2, 3), container);

        let empty = save_to_bytes(&SaveData::new(1));
        let bytes = save_to_bytes(&data);
        // The container record begins right after the (now 1) container count.
        let count_at = empty.len() - 4;
        let record = bytes[empty.len()..].to_vec();
        // pos i32*3, kind u16, then the slot count byte.
        let slot_count_at = empty.len() + 12 + 2;

        // Two containers claiming the same block.
        let mut dup = bytes.clone();
        dup[count_at..count_at + 4].copy_from_slice(&2u32.to_le_bytes());
        dup.extend_from_slice(&record);
        assert!(matches!(
            load_from_bytes(&dup),
            Err(SaveError::Corrupt("two containers at one block"))
        ));

        // A container wider than anything the game has.
        let mut wide = bytes.clone();
        wide[slot_count_at] = 200;
        assert!(matches!(
            load_from_bytes(&wide),
            Err(SaveError::Corrupt("container has more slots than possible"))
        ));

        // More filled slots than slots.
        let mut over = bytes.clone();
        over[slot_count_at] = 0;
        assert!(matches!(
            load_from_bytes(&over),
            Err(SaveError::Corrupt(
                "more filled slots than the container has"
            ))
        ));

        // A slot index past the end of the container.
        let mut stray = bytes.clone();
        stray[slot_count_at + 2] = 9;
        assert!(matches!(
            load_from_bytes(&stray),
            Err(SaveError::Corrupt("container slot index out of range"))
        ));

        // An item id nothing defines.
        let mut ghost = bytes.clone();
        ghost[slot_count_at + 3..slot_count_at + 5].copy_from_slice(&4242u16.to_le_bytes());
        assert!(matches!(
            load_from_bytes(&ghost),
            Err(SaveError::Corrupt("unknown item id"))
        ));
    }

    // --- size ----------------------------------------------------------------

    #[test]
    fn an_untouched_world_still_saves_in_a_few_dozen_bytes() {
        let bytes = save_to_bytes(&SaveData::new(1337));
        assert!(
            bytes.len() <= 64,
            "an untouched world saved {} bytes; the two new sections must cost \
             one empty count each, not kilobytes",
            bytes.len()
        );
        // Precisely eight bytes more than the version 1 header cost.
        assert_eq!(bytes.len(), 53);
        assert_eq!(&bytes[bytes.len() - 8..], &[0u8; 8], "two empty sections");
        let back = load_from_bytes(&bytes).unwrap();
        assert!(back.mobs.is_empty() && back.containers.is_empty());
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
