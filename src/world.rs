//! The live chunk map: threaded generation, streaming in and out, remeshing,
//! and every query gameplay code needs against the voxel grid.
//!
//! Threading model, in one paragraph. Chunks live behind `Arc` so the main
//! thread can hand a worker a snapshot with 27 refcount bumps and no copying.
//! Generation and meshing both run on the rayon pool and report back over
//! channels; the main thread only drains those channels and creates GPU
//! buffers. Nothing that touches wgpu ever leaves the main thread.
//!
//! Queueing model, in one more. The wanted-chunk set is rebuilt only when the
//! player crosses a chunk boundary, and then drained incrementally from a
//! `VecDeque`. There is no per-frame allocate-and-sort of the whole world.
//! Player edits jump the queue by pushing to the front.

use crate::block::BlockId;
use crate::chunk::{Chunk, ChunkPos, SubMask};
use crate::config::*;
use crate::mesh::{mesh_chunk, Neighborhood, Vertex, NEIGHBOR_COUNT};
use crate::worldgen::{ColumnBounds, TerrainGen};
use glam::Vec3;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

/// World-space block coordinate to (chunk, local) coordinates.
#[inline]
pub fn split_coord(v: i32) -> (i32, usize) {
    let c = v.div_euclid(CHUNK_SIZE_I);
    let l = v.rem_euclid(CHUNK_SIZE_I) as usize;
    (c, l)
}

/// A sound the world emitted. Mining pushes these; the mob AI drains them and
/// floods the loudness outward to decide who comes looking.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NoiseEvent {
    pub pos: Vec3,
    pub loudness: f32,
}

/// What a ray from the camera struck.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RayHit {
    /// Block coordinate that was hit.
    pub block: (i32, i32, i32),
    /// Sub-voxel within that block, each component 0..8.
    pub sub: (usize, usize, usize),
    /// Outward normal of the face entered, e.g. `[0, 1, 0]` for a floor.
    pub face: [i32; 3],
    /// Exact world-space point of contact.
    pub point: Vec3,
    /// Distance from the ray origin, in blocks.
    pub distance: f32,
}

impl RayHit {
    /// The block coordinate a placement against this face would occupy.
    pub fn adjacent(&self) -> (i32, i32, i32) {
        (
            self.block.0 + self.face[0],
            self.block.1 + self.face[1],
            self.block.2 + self.face[2],
        )
    }
}

/// A finished mesh waiting to become GPU buffers.
pub struct ChunkMeshData {
    pub pos: ChunkPos,
    pub verts: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// What one `World::stream` call produced for the renderer to apply.
#[derive(Default)]
pub struct StreamResult {
    pub dropped: Vec<ChunkPos>,
    pub ready: Vec<ChunkMeshData>,
}

#[derive(Default, Copy, Clone)]
pub struct WorldStats {
    pub gen_queued: usize,
    pub gen_in_flight: usize,
    pub mesh_queued: usize,
    pub mesh_in_flight: usize,
    pub chunks_generated: u64,
    pub chunks_meshed: u64,
}

pub struct World {
    pub chunks: HashMap<ChunkPos, Arc<Chunk>>,
    pub terrain: Arc<TerrainGen>,

    /// Cached conservative surface bounds per chunk-column.
    bounds: HashMap<(i32, i32), ColumnBounds>,
    center: Option<ChunkPos>,

    gen_queue: VecDeque<ChunkPos>,
    gen_queued: HashSet<ChunkPos>,
    generating: HashSet<ChunkPos>,
    gen_tx: Sender<(ChunkPos, Chunk)>,
    gen_rx: Receiver<(ChunkPos, Chunk)>,

    mesh_queue: VecDeque<ChunkPos>,
    mesh_queued: HashSet<ChunkPos>,
    meshing: HashSet<ChunkPos>,
    /// Chunks edited while their mesh job was already in flight.
    remesh_after: HashSet<ChunkPos>,
    /// Chunks that must be meshed even if they look provably invisible, because
    /// a player edit may have turned an existing mesh into nothing.
    force_mesh: HashSet<ChunkPos>,
    mesh_tx: Sender<ChunkMeshData>,
    mesh_rx: Receiver<ChunkMeshData>,

    noise: Vec<NoiseEvent>,
    pub stats: WorldStats,
}

impl World {
    pub fn new(seed: u32) -> Self {
        let (gen_tx, gen_rx) = channel();
        let (mesh_tx, mesh_rx) = channel();
        Self {
            chunks: HashMap::new(),
            terrain: Arc::new(TerrainGen::new(seed)),
            bounds: HashMap::new(),
            center: None,
            gen_queue: VecDeque::new(),
            gen_queued: HashSet::new(),
            generating: HashSet::new(),
            gen_tx,
            gen_rx,
            mesh_queue: VecDeque::new(),
            mesh_queued: HashSet::new(),
            meshing: HashSet::new(),
            remesh_after: HashSet::new(),
            force_mesh: HashSet::new(),
            mesh_tx,
            mesh_rx,
            noise: Vec::new(),
            stats: WorldStats::default(),
        }
    }

    // -----------------------------------------------------------------------
    // Frozen contract API
    // -----------------------------------------------------------------------

    /// Block at a world coordinate. Chunks that are not resident fall back to
    /// generation, which keeps chunk seams correct while neighbours stream in
    /// and stops the player falling through terrain that has not loaded.
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
        let pos = ChunkPos::new(cx, cy, cz);
        if let Some(arc) = self.chunks.get_mut(&pos) {
            Arc::make_mut(arc).set(lx, ly, lz, id);
            self.dirty_edit(pos, lx, ly, lz);
        }
    }

    /// Carve one sub-voxel out of a block. Returns true if the block was destroyed.
    pub fn carve(&mut self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        let (cx, lx) = split_coord(x);
        let (cy, ly) = split_coord(y);
        let (cz, lz) = split_coord(z);
        let pos = ChunkPos::new(cx, cy, cz);
        let Some(arc) = self.chunks.get_mut(&pos) else {
            return false;
        };
        let destroyed = Arc::make_mut(arc).carve(lx, ly, lz, sx, sy, sz);
        self.dirty_edit(pos, lx, ly, lz);
        destroyed
    }

    /// Ground height of a column.
    pub fn surface_y(&self, x: i32, z: i32) -> i32 {
        self.terrain.height_at(x, z)
    }

    /// Fraction of a block still solid, 1.0 when untouched.
    pub fn fill_ratio(&self, x: i32, y: i32, z: i32) -> f32 {
        let (cx, lx) = split_coord(x);
        let (cy, ly) = split_coord(y);
        let (cz, lz) = split_coord(z);
        match self.chunks.get(&ChunkPos::new(cx, cy, cz)) {
            Some(c) => c.fill_ratio(lx, ly, lz),
            // Not resident, so nothing has been carved out of it.
            None => {
                if self.terrain.block_at(x, y, z).is_air() {
                    0.0
                } else {
                    1.0
                }
            }
        }
    }

    /// True when that sub-voxel is still solid. Untouched blocks report true
    /// everywhere; air blocks report false everywhere.
    pub fn sub_solid(&self, x: i32, y: i32, z: i32, sx: usize, sy: usize, sz: usize) -> bool {
        let (cx, lx) = split_coord(x);
        let (cy, ly) = split_coord(y);
        let (cz, lz) = split_coord(z);
        match self.chunks.get(&ChunkPos::new(cx, cy, cz)) {
            Some(c) => c.sub_solid(lx, ly, lz, sx, sy, sz),
            None => !self.terrain.block_at(x, y, z).is_air(),
        }
    }

    // -----------------------------------------------------------------------
    // Noise queue -- the hinge the whole game design turns on
    // -----------------------------------------------------------------------

    pub fn push_noise(&mut self, event: NoiseEvent) {
        self.noise.push(event);
    }

    pub fn drain_noise(&mut self) -> Vec<NoiseEvent> {
        std::mem::take(&mut self.noise)
    }

    /// Peek without consuming, for HUD or debug overlays.
    pub fn pending_noise(&self) -> &[NoiseEvent] {
        &self.noise
    }

    // -----------------------------------------------------------------------
    // Sub-voxel-aware queries
    // -----------------------------------------------------------------------

    /// The carve mask of a block, if it is resident and damaged.
    pub fn damage_mask(&self, x: i32, y: i32, z: i32) -> Option<&SubMask> {
        let (cx, lx) = split_coord(x);
        let (cy, ly) = split_coord(y);
        let (cz, lz) = split_coord(z);
        self.chunks.get(&ChunkPos::new(cx, cy, cz))?.mask(lx, ly, lz)
    }

    /// True when an axis-aligned box overlaps material the player collides with.
    /// Partially carved blocks are tested against their sub-voxel mask, so a
    /// crater is genuinely walkable rather than a solid block that looks hollow.
    pub fn box_collides(&self, min: Vec3, max: Vec3) -> bool {
        let e = COLLIDE_EPSILON;
        let x0 = (min.x + e).floor() as i32;
        let x1 = (max.x - e).floor() as i32;
        let y0 = (min.y + e).floor() as i32;
        let y1 = (max.y - e).floor() as i32;
        let z0 = (min.z + e).floor() as i32;
        let z1 = (max.z - e).floor() as i32;

        for by in y0..=y1 {
            for bz in z0..=z1 {
                for bx in x0..=x1 {
                    if !self.block_at(bx, by, bz).is_solid() {
                        continue;
                    }
                    match self.damage_mask(bx, by, bz) {
                        None => return true,
                        Some(mask) => {
                            if Self::mask_overlaps_box(mask, bx, by, bz, min, max) {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    fn mask_overlaps_box(
        mask: &SubMask,
        bx: i32,
        by: i32,
        bz: i32,
        min: Vec3,
        max: Vec3,
    ) -> bool {
        let e = COLLIDE_EPSILON;
        let range = |lo: f32, hi: f32, base: i32| -> (usize, usize) {
            let a = (((lo + e) - base as f32) * SUBVOX_F).floor();
            let b = (((hi - e) - base as f32) * SUBVOX_F).floor();
            (
                a.clamp(0.0, SUBVOX_F - 1.0) as usize,
                b.clamp(0.0, SUBVOX_F - 1.0) as usize,
            )
        };
        let (sx0, sx1) = range(min.x, max.x, bx);
        let (sy0, sy1) = range(min.y, max.y, by);
        let (sz0, sz1) = range(min.z, max.z, bz);
        for sy in sy0..=sy1 {
            for sz in sz0..=sz1 {
                for sx in sx0..=sx1 {
                    if mask.get(sx, sy, sz) {
                        return true;
                    }
                }
            }
        }
        false
    }

    // -----------------------------------------------------------------------
    // Raycast
    // -----------------------------------------------------------------------

    /// Amanatides-Woo DDA stepped at **sub-voxel** resolution, so a ray that
    /// enters a hollowed-out region of a block keeps going instead of stopping
    /// at the block's outer shell. `max_dist` is in blocks.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max_dist: f32) -> Option<RayHit> {
        if dir.length_squared() < 1e-12 {
            return None;
        }
        let d = dir.normalize();
        // Work in sub-voxel units: one grid cell is 1/8 of a block.
        let p = origin * SUBVOX_F;
        let mut cell = [
            p.x.floor() as i32,
            p.y.floor() as i32,
            p.z.floor() as i32,
        ];
        let dv = [d.x, d.y, d.z];
        let pv = [p.x, p.y, p.z];

        let mut step = [0i32; 3];
        let mut t_max = [f32::INFINITY; 3];
        let mut t_delta = [f32::INFINITY; 3];
        for a in 0..3 {
            if dv[a] > 0.0 {
                step[a] = 1;
                t_max[a] = ((cell[a] + 1) as f32 - pv[a]) / dv[a];
                t_delta[a] = 1.0 / dv[a];
            } else if dv[a] < 0.0 {
                step[a] = -1;
                t_max[a] = (cell[a] as f32 - pv[a]) / dv[a];
                t_delta[a] = -1.0 / dv[a];
            }
        }

        let limit = max_dist * SUBVOX_F;
        let mut t = 0.0f32;
        // Face normal of the cell we are standing in; only meaningful once we
        // have stepped at least once.
        let mut face = [0i32; 3];

        // A generous step cap: 3 axes x the sub-cell reach, plus slack.
        let max_steps = (limit.ceil() as i32 * 3 + 8).max(8);
        for _ in 0..max_steps {
            let bx = cell[0].div_euclid(SUBVOX_I);
            let by = cell[1].div_euclid(SUBVOX_I);
            let bz = cell[2].div_euclid(SUBVOX_I);
            if (0..WORLD_HEIGHT).contains(&by) {
                let sx = cell[0].rem_euclid(SUBVOX_I) as usize;
                let sy = cell[1].rem_euclid(SUBVOX_I) as usize;
                let sz = cell[2].rem_euclid(SUBVOX_I) as usize;
                let id = self.block_at(bx, by, bz);
                if !id.is_air() && self.sub_solid(bx, by, bz, sx, sy, sz) {
                    let dist = t / SUBVOX_F;
                    return Some(RayHit {
                        block: (bx, by, bz),
                        sub: (sx, sy, sz),
                        face,
                        point: origin + d * dist,
                        distance: dist,
                    });
                }
            }

            // Advance to the next sub-cell along the shortest axis.
            let a = if t_max[0] < t_max[1] {
                if t_max[0] < t_max[2] { 0 } else { 2 }
            } else if t_max[1] < t_max[2] {
                1
            } else {
                2
            };
            if t_max[a] > limit {
                return None;
            }
            t = t_max[a];
            cell[a] += step[a];
            t_max[a] += t_delta[a];
            face = [0, 0, 0];
            // We entered through the face opposite the direction of travel.
            face[a] = -step[a];
        }
        None
    }

    // -----------------------------------------------------------------------
    // Mining and building
    // -----------------------------------------------------------------------

    /// Chip a small sphere of sub-voxels out around a hit point. `radius` is in
    /// sub-voxels. Returns how many sub-voxels were actually removed, so the
    /// caller can tell a real bite out of the wall from a wasted swing.
    pub fn chip_sphere(&mut self, hit: &RayHit, radius: f32) -> u32 {
        let (bx, by, bz) = hit.block;
        let (sx, sy, sz) = hit.sub;
        let centre = [
            bx * SUBVOX_I + sx as i32,
            by * SUBVOX_I + sy as i32,
            bz * SUBVOX_I + sz as i32,
        ];
        let r = radius.ceil() as i32;
        let r2 = radius * radius;
        let mut removed = 0u32;
        for dy in -r..=r {
            for dz in -r..=r {
                for dx in -r..=r {
                    let d2 = (dx * dx + dy * dy + dz * dz) as f32;
                    if d2 > r2 {
                        continue;
                    }
                    let (wx, wy, wz) = (centre[0] + dx, centre[1] + dy, centre[2] + dz);
                    let (b, s) = (
                        (
                            wx.div_euclid(SUBVOX_I),
                            wy.div_euclid(SUBVOX_I),
                            wz.div_euclid(SUBVOX_I),
                        ),
                        (
                            wx.rem_euclid(SUBVOX_I) as usize,
                            wy.rem_euclid(SUBVOX_I) as usize,
                            wz.rem_euclid(SUBVOX_I) as usize,
                        ),
                    );
                    if self.block_at(b.0, b.1, b.2).hardness().is_infinite() {
                        continue; // bedrock never yields
                    }
                    if !self.sub_solid(b.0, b.1, b.2, s.0, s.1, s.2) {
                        continue;
                    }
                    self.carve(b.0, b.1, b.2, s.0, s.1, s.2);
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            self.push_noise(NoiseEvent {
                pos: hit.point,
                loudness: NOISE_CHIP,
            });
        }
        removed
    }

    /// Remove a whole block at once. This is the loud option.
    pub fn smash_block(&mut self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let id = self.block_at(x, y, z);
        if id.is_air() || id.hardness().is_infinite() {
            return None;
        }
        let (cx, _) = split_coord(x);
        let (cy, _) = split_coord(y);
        let (cz, _) = split_coord(z);
        if !self.chunks.contains_key(&ChunkPos::new(cx, cy, cz)) {
            return None;
        }
        self.set_block(x, y, z, BlockId::AIR);
        self.push_noise(NoiseEvent {
            pos: Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5),
            loudness: NOISE_SMASH,
        });
        Some(id)
    }

    /// Place a block, refusing to bury the player inside it.
    pub fn place_block(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        id: BlockId,
        player_min: Vec3,
        player_max: Vec3,
    ) -> bool {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return false;
        }
        if !self.block_at(x, y, z).is_air() {
            return false;
        }
        // Block cell as an AABB; refuse if it would intersect the player.
        if id.is_solid() {
            let bmin = Vec3::new(x as f32, y as f32, z as f32);
            let bmax = bmin + Vec3::ONE;
            let overlaps = player_min.x < bmax.x
                && player_max.x > bmin.x
                && player_min.y < bmax.y
                && player_max.y > bmin.y
                && player_min.z < bmax.z
                && player_max.z > bmin.z;
            if overlaps {
                return false;
            }
        }
        let (cx, _) = split_coord(x);
        let (cy, _) = split_coord(y);
        let (cz, _) = split_coord(z);
        if !self.chunks.contains_key(&ChunkPos::new(cx, cy, cz)) {
            return false; // do not edit chunks that are not loaded
        }
        self.set_block(x, y, z, id);
        true
    }

    // -----------------------------------------------------------------------
    // Streaming
    // -----------------------------------------------------------------------

    /// Advance the streaming pipeline by one frame. The returned meshes are the
    /// only thing the caller has to push to the GPU.
    pub fn stream(&mut self, center: ChunkPos) -> StreamResult {
        let mut out = StreamResult::default();

        // The wanted set only changes when the player changes chunk. This is
        // the whole point: no rebuild-and-sort of ~7,000 positions per frame.
        if self.center != Some(center) {
            out.dropped = self.unload_far(center);
            self.rebuild_gen_queue(center);
            self.center = Some(center);
        }

        self.intake_generated();
        out.ready = self.collect_meshes();
        self.dispatch_generation();
        self.dispatch_meshing();

        self.stats.gen_queued = self.gen_queue.len();
        self.stats.gen_in_flight = self.generating.len();
        self.stats.mesh_queued = self.mesh_queue.len();
        self.stats.mesh_in_flight = self.meshing.len();
        out
    }

    /// True when everything the streamer wanted is generated, meshed and uploaded.
    pub fn is_idle(&self) -> bool {
        self.gen_queue.is_empty()
            && self.generating.is_empty()
            && self.mesh_queue.is_empty()
            && self.meshing.is_empty()
    }

    fn bounds_for(&mut self, cx: i32, cz: i32) -> ColumnBounds {
        if let Some(b) = self.bounds.get(&(cx, cz)) {
            return *b;
        }
        let b = self.terrain.column_bounds(cx, cz);
        self.bounds.insert((cx, cz), b);
        b
    }

    /// The chunks worth having resident around `center`, nearest first.
    ///
    /// See the streaming block in `config.rs` for why this is not simply the
    /// full 16-high column across the whole disc.
    fn wanted_chunks(&mut self, center: ChunkPos) -> Vec<ChunkPos> {
        let r = RENDER_DISTANCE;
        let near2 = FULL_COLUMN_RADIUS * FULL_COLUMN_RADIUS;
        let mut out: Vec<ChunkPos> = Vec::with_capacity(4096);
        for dz in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dz * dz;
                if d2 > r * r {
                    continue;
                }
                let (cx, cz) = (center.x + dx, center.z + dz);
                let b = self.bounds_for(cx, cz);
                // Nothing above the highest possible ground is ever solid.
                let top = b.hi.div_euclid(CHUNK_SIZE_I).clamp(0, CHUNK_COLUMN - 1);
                let skin_lo = b.lo.div_euclid(CHUNK_SIZE_I) - SURFACE_SKIN_DEPTH;
                let lo = if d2 <= near2 {
                    // Near the player: follow them down a mineshaft.
                    (center.y - VERTICAL_LOAD_RADIUS).min(skin_lo)
                } else {
                    // Far away: the surface skin is all that can be seen.
                    skin_lo
                }
                .clamp(0, top);
                for cy in lo..=top {
                    out.push(ChunkPos::new(cx, cy, cz));
                }
            }
        }
        out.sort_by_key(|p| {
            let dx = p.x - center.x;
            let dz = p.z - center.z;
            let dy = p.y - center.y;
            dx * dx + dz * dz + dy * dy / 2
        });
        out
    }

    fn rebuild_gen_queue(&mut self, center: ChunkPos) {
        let wanted = self.wanted_chunks(center);
        self.gen_queue.clear();
        self.gen_queued.clear();
        for p in wanted {
            if self.chunks.contains_key(&p) || self.generating.contains(&p) {
                continue;
            }
            if self.gen_queued.insert(p) {
                self.gen_queue.push_back(p);
            }
        }
    }

    /// Drop chunks outside the keep radius. Returns the positions removed.
    fn unload_far(&mut self, center: ChunkPos) -> Vec<ChunkPos> {
        let keep = RENDER_DISTANCE + UNLOAD_MARGIN;
        let r2 = keep * keep;
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
            self.mesh_queued.remove(p);
            self.remesh_after.remove(p);
            self.force_mesh.remove(p);
        }
        let Self {
            mesh_queue,
            mesh_queued,
            ..
        } = self;
        mesh_queue.retain(|p| mesh_queued.contains(p));
        self.bounds.retain(|(cx, cz), _| {
            let dx = cx - center.x;
            let dz = cz - center.z;
            dx * dx + dz * dz <= r2
        });
        doomed
    }

    fn dispatch_generation(&mut self) {
        while self.generating.len() < GEN_JOBS_IN_FLIGHT {
            let Some(pos) = self.gen_queue.pop_front() else {
                break;
            };
            self.gen_queued.remove(&pos);
            if self.chunks.contains_key(&pos) || !self.generating.insert(pos) {
                continue;
            }
            let terrain = self.terrain.clone();
            let tx = self.gen_tx.clone();
            rayon::spawn(move || {
                let chunk = terrain.generate(pos);
                let _ = tx.send((pos, chunk));
            });
        }
    }

    fn intake_generated(&mut self) {
        for _ in 0..GEN_INTAKE_PER_FRAME {
            let Ok((pos, chunk)) = self.gen_rx.try_recv() else {
                break;
            };
            self.generating.remove(&pos);
            self.stats.chunks_generated += 1;
            let empty = chunk.is_empty();
            self.chunks.insert(pos, Arc::new(chunk));
            // Note what we deliberately do *not* do here: mark the six
            // neighbours dirty. An unmodified chunk is byte-identical to what
            // the meshing skirt already sampled from the generator, so a
            // neighbour arriving cannot change an existing mesh. Skipping that
            // cascade is worth roughly 6x on the meshing work during streaming.
            if !empty {
                self.enqueue_mesh(pos, false);
            }
        }
    }

    fn enqueue_mesh(&mut self, pos: ChunkPos, urgent: bool) {
        if !self.chunks.contains_key(&pos) {
            return;
        }
        if urgent {
            self.force_mesh.insert(pos);
        }
        if self.meshing.contains(&pos) {
            self.remesh_after.insert(pos);
            return;
        }
        if self.mesh_queued.insert(pos) {
            if urgent {
                self.mesh_queue.push_front(pos);
            } else {
                self.mesh_queue.push_back(pos);
            }
        } else if urgent {
            // Already queued; move it to the front so edits feel instant.
            if let Some(i) = self.mesh_queue.iter().position(|p| *p == pos) {
                self.mesh_queue.remove(i);
            }
            self.mesh_queue.push_front(pos);
        }
    }

    /// A chunk whose six face neighbours are all solid opaque rock shows no
    /// surface at all. Underground, that is most of them.
    fn is_provably_invisible(&self, pos: ChunkPos) -> bool {
        let Some(c) = self.chunks.get(&pos) else {
            return true;
        };
        if c.is_empty() {
            return true;
        }
        if !c.is_uniform_opaque() {
            return false;
        }
        for (dx, dy, dz) in [
            (1, 0, 0),
            (-1, 0, 0),
            (0, 1, 0),
            (0, -1, 0),
            (0, 0, 1),
            (0, 0, -1),
        ] {
            match self
                .chunks
                .get(&ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz))
            {
                Some(n) if n.is_uniform_opaque() => {}
                // Absent or see-through: cannot prove it, so mesh it.
                _ => return false,
            }
        }
        true
    }

    fn gather_neighbors(&self, pos: ChunkPos) -> [Option<Arc<Chunk>>; NEIGHBOR_COUNT] {
        std::array::from_fn(|i| {
            let dx = (i % 3) as i32 - 1;
            let dz = ((i / 3) % 3) as i32 - 1;
            let dy = (i / 9) as i32 - 1;
            self.chunks
                .get(&ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz))
                .cloned()
        })
    }

    fn dispatch_meshing(&mut self) {
        while self.meshing.len() < MESH_JOBS_IN_FLIGHT {
            let Some(pos) = self.mesh_queue.pop_front() else {
                break;
            };
            self.mesh_queued.remove(&pos);
            if !self.chunks.contains_key(&pos) {
                self.force_mesh.remove(&pos);
                continue;
            }
            // An edit always meshes: it may have turned an existing mesh into
            // nothing, and only a real (empty) mesh result frees the GPU buffer.
            if !self.force_mesh.remove(&pos) && self.is_provably_invisible(pos) {
                continue;
            }
            self.meshing.insert(pos);
            let neighbors = self.gather_neighbors(pos);
            let terrain = self.terrain.clone();
            let tx = self.mesh_tx.clone();
            rayon::spawn(move || {
                let nb = Neighborhood::build(pos, &neighbors, &terrain);
                let (verts, indices) = mesh_chunk(&nb);
                let _ = tx.send(ChunkMeshData {
                    pos,
                    verts,
                    indices,
                });
            });
        }
    }

    fn collect_meshes(&mut self) -> Vec<ChunkMeshData> {
        let start = Instant::now();
        let budget = std::time::Duration::from_secs_f32(UPLOAD_BUDGET_MS / 1000.0);
        let mut out = Vec::new();
        while out.len() < UPLOAD_BUDGET_COUNT {
            let Ok(data) = self.mesh_rx.try_recv() else {
                break;
            };
            self.meshing.remove(&data.pos);
            self.stats.chunks_meshed += 1;
            if self.remesh_after.remove(&data.pos) {
                self.enqueue_mesh(data.pos, true);
            }
            if self.chunks.contains_key(&data.pos) {
                out.push(data);
            }
            if start.elapsed() >= budget {
                break;
            }
        }
        out
    }

    /// Mark a chunk (and, on a boundary block, its neighbour) for remeshing
    /// after a player edit. Edits jump the queue.
    fn dirty_edit(&mut self, pos: ChunkPos, lx: usize, ly: usize, lz: usize) {
        self.enqueue_mesh(pos, true);
        // A block on a chunk boundary changes the neighbour's visible faces too.
        let last = CHUNK_SIZE - 1;
        let mut touch = |dx: i32, dy: i32, dz: i32| {
            ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz)
        };
        let mut neighbours = Vec::new();
        if lx == 0 {
            neighbours.push(touch(-1, 0, 0));
        }
        if lx == last {
            neighbours.push(touch(1, 0, 0));
        }
        if ly == 0 {
            neighbours.push(touch(0, -1, 0));
        }
        if ly == last {
            neighbours.push(touch(0, 1, 0));
        }
        if lz == 0 {
            neighbours.push(touch(0, 0, -1));
        }
        if lz == last {
            neighbours.push(touch(0, 0, 1));
        }
        for n in neighbours {
            self.enqueue_mesh(n, true);
        }
    }

    // -----------------------------------------------------------------------
    // Test / tooling helpers
    // -----------------------------------------------------------------------

    /// Generate a chunk synchronously and insert it. Used by tests and by any
    /// caller that needs a chunk resident right now.
    pub fn ensure(&mut self, pos: ChunkPos) -> bool {
        if self.chunks.contains_key(&pos) {
            return false;
        }
        let chunk = self.terrain.generate(pos);
        self.chunks.insert(pos, Arc::new(chunk));
        true
    }

    /// Mesh a chunk immediately on the calling thread.
    pub fn mesh_now(&self, pos: ChunkPos) -> Option<(Vec<Vertex>, Vec<u32>)> {
        self.chunks.get(&pos)?;
        let neighbors = self.gather_neighbors(pos);
        let nb = Neighborhood::build(pos, &neighbors, &self.terrain);
        Some(mesh_chunk(&nb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rayon::prelude::*;

    fn flat_world() -> World {
        World::new(1337)
    }

    /// Fill a cube of chunks with a solid wall so raycast tests have a target
    /// that does not depend on the noise field.
    fn world_with_wall() -> World {
        let mut w = World::new(1337);
        for cy in 0..2 {
            for cz in -1..2 {
                for cx in -1..2 {
                    let pos = ChunkPos::new(cx, cy, cz);
                    let mut c = Chunk::new(pos);
                    c.generated = true;
                    w.chunks.insert(pos, Arc::new(c));
                }
            }
        }
        // A solid stone plane at x = 10, spanning the region we shoot through.
        for y in 0..8 {
            for z in -4..8 {
                w.set_block(10, y, z, BlockId::STONE);
            }
        }
        w
    }

    #[test]
    fn raycast_hits_the_expected_block_and_face() {
        let w = world_with_wall();
        let origin = Vec3::new(4.5, 2.5, 2.5);
        let hit = w
            .raycast(origin, Vec3::X, 12.0)
            .expect("ray down +X must hit the wall at x=10");
        assert_eq!(hit.block, (10, 2, 2));
        assert_eq!(hit.face, [-1, 0, 0], "entered through the -X face");
        assert!(
            (hit.distance - 5.5).abs() < 1e-3,
            "distance was {}",
            hit.distance
        );
        assert!((hit.point.x - 10.0).abs() < 1e-3);
    }

    #[test]
    fn raycast_respects_reach() {
        let w = world_with_wall();
        let origin = Vec3::new(4.5, 2.5, 2.5);
        assert!(w.raycast(origin, Vec3::X, 5.0).is_none());
        assert!(w.raycast(origin, Vec3::X, 6.0).is_some());
    }

    #[test]
    fn raycast_hits_a_floor_from_above() {
        let mut w = world_with_wall();
        w.set_block(2, 3, 2, BlockId::STONE);
        let hit = w
            .raycast(Vec3::new(2.5, 6.0, 2.5), -Vec3::Y, 6.0)
            .expect("must hit the block below");
        assert_eq!(hit.block, (2, 3, 2));
        assert_eq!(hit.face, [0, 1, 0], "entered through the top face");
    }

    /// The load-bearing behaviour for sub-voxel destruction: a ray through a
    /// hollowed region keeps going instead of stopping at the outer shell.
    #[test]
    fn raycast_passes_through_a_carved_tunnel() {
        let mut w = world_with_wall();
        // Bore a 1-sub-voxel channel straight through the wall block at
        // (10, 2, 2), on the row the ray from y=2.5 z=2.5 travels along.
        let (sy, sz) = (4, 4); // 2.5 -> sub-voxel 4 on each axis
        for sx in 0..SUBVOX {
            w.carve(10, 2, 2, sx, sy, sz);
        }
        assert_eq!(w.block_at(10, 2, 2), BlockId::STONE, "block must survive");

        let origin = Vec3::new(4.5, 2.5 + 1.0 / 16.0, 2.5 + 1.0 / 16.0);
        let hit = w.raycast(origin, Vec3::X, 12.0);
        // The tunnel is open, so the ray must reach nothing (past the wall
        // there is only air) rather than stopping at x=10.
        assert!(
            hit.is_none() || hit.unwrap().block.0 > 10,
            "ray stopped at the shell of a hollowed block"
        );
    }

    #[test]
    fn carving_every_subvoxel_turns_the_block_to_air() {
        let mut w = world_with_wall();
        let mut destroyed = false;
        for sy in 0..SUBVOX {
            for sz in 0..SUBVOX {
                for sx in 0..SUBVOX {
                    destroyed |= w.carve(10, 2, 2, sx, sy, sz);
                }
            }
        }
        assert!(destroyed);
        assert_eq!(w.block_at(10, 2, 2), BlockId::AIR);
        assert_eq!(w.fill_ratio(10, 2, 2), 0.0);
        assert!(!w.sub_solid(10, 2, 2, 0, 0, 0));
    }

    #[test]
    fn fill_ratio_and_sub_solid_match_the_contract() {
        let mut w = world_with_wall();
        assert_eq!(w.fill_ratio(10, 2, 2), 1.0);
        assert!(w.sub_solid(10, 2, 2, 3, 3, 3));
        w.carve(10, 2, 2, 3, 3, 3);
        assert!(!w.sub_solid(10, 2, 2, 3, 3, 3));
        assert!((w.fill_ratio(10, 2, 2) - 511.0 / 512.0).abs() < 1e-6);
        // Air reports empty everywhere.
        assert_eq!(w.fill_ratio(0, 5, 0), 0.0);
        assert!(!w.sub_solid(0, 5, 0, 0, 0, 0));
    }

    #[test]
    fn chip_sphere_bites_a_crater_and_emits_quiet_noise() {
        let mut w = world_with_wall();
        let hit = w.raycast(Vec3::new(4.5, 2.5, 2.5), Vec3::X, 12.0).unwrap();
        let removed = w.chip_sphere(&hit, CHIP_RADIUS);
        assert!(removed > 0, "a chip must remove something");
        assert_eq!(w.block_at(10, 2, 2), BlockId::STONE, "one chip is not a break");
        assert!(w.fill_ratio(10, 2, 2) < 1.0);

        let events = w.drain_noise();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].loudness, NOISE_CHIP);
        assert!(w.drain_noise().is_empty(), "drain must consume");
    }

    #[test]
    fn smashing_a_block_is_loud_and_immediate() {
        let mut w = world_with_wall();
        assert_eq!(w.smash_block(10, 2, 2), Some(BlockId::STONE));
        assert_eq!(w.block_at(10, 2, 2), BlockId::AIR);
        let events = w.drain_noise();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].loudness, NOISE_SMASH);
        assert!(
            NOISE_SMASH > NOISE_CHIP * 2.0,
            "the whole design rests on smashing being much louder than chipping"
        );
    }

    #[test]
    fn bedrock_cannot_be_mined() {
        let mut w = world_with_wall();
        w.set_block(3, 1, 3, BlockId::BEDROCK);
        assert_eq!(w.smash_block(3, 1, 3), None);
        assert_eq!(w.block_at(3, 1, 3), BlockId::BEDROCK);
    }

    #[test]
    fn placement_refuses_to_bury_the_player() {
        let mut w = world_with_wall();
        let pmin = Vec3::new(2.0, 2.0, 2.0);
        let pmax = Vec3::new(2.6, 3.8, 2.6);
        // Inside the player's box.
        assert!(!w.place_block(2, 2, 2, BlockId::STONE, pmin, pmax));
        // Clear of it.
        assert!(w.place_block(5, 2, 5, BlockId::STONE, pmin, pmax));
        assert_eq!(w.block_at(5, 2, 5), BlockId::STONE);
    }

    #[test]
    fn box_collision_respects_carve_masks() {
        let mut w = world_with_wall();
        w.set_block(3, 3, 3, BlockId::STONE);
        let min = Vec3::new(3.05, 3.05, 3.05);
        let max = Vec3::new(3.2, 3.2, 3.2);
        assert!(w.box_collides(min, max), "solid block must collide");

        // Hollow out the corner the box occupies (sub-voxels 0 and 1 on each
        // axis cover 0.0..0.25 of the block).
        for sy in 0..2 {
            for sz in 0..2 {
                for sx in 0..2 {
                    w.carve(3, 3, 3, sx, sy, sz);
                }
            }
        }
        assert!(
            !w.box_collides(min, max),
            "a carved pocket must be walkable, not just look hollow"
        );
        // The rest of the block is still solid.
        assert!(w.box_collides(Vec3::new(3.5, 3.5, 3.5), Vec3::new(3.7, 3.7, 3.7)));
    }

    #[test]
    fn box_collision_ignores_air_and_water() {
        let mut w = world_with_wall();
        assert!(!w.box_collides(Vec3::new(1.1, 1.1, 1.1), Vec3::new(1.5, 1.5, 1.5)));
        w.set_block(1, 1, 1, BlockId::WATER);
        assert!(!w.box_collides(Vec3::new(1.1, 1.1, 1.1), Vec3::new(1.5, 1.5, 1.5)));
    }

    #[test]
    fn noise_queue_round_trips() {
        let mut w = flat_world();
        assert!(w.drain_noise().is_empty());
        w.push_noise(NoiseEvent {
            pos: Vec3::new(1.0, 2.0, 3.0),
            loudness: 0.5,
        });
        w.push_noise(NoiseEvent {
            pos: Vec3::ZERO,
            loudness: 1.0,
        });
        assert_eq!(w.pending_noise().len(), 2);
        let drained = w.drain_noise();
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].loudness, 0.5);
        assert!(w.pending_noise().is_empty());
    }

    /// The performance requirement, measured directly. Generation and meshing
    /// both have to be parallel for this to land.
    #[test]
    fn generating_and_meshing_512_chunks_is_fast() {
        let terrain = Arc::new(TerrainGen::new(1337));
        let mut positions = Vec::new();
        // 8 x 8 columns x 8 vertical = 512 chunks straddling the surface.
        for cx in 0..8 {
            for cz in 0..8 {
                for cy in 0..8 {
                    positions.push(ChunkPos::new(cx, cy, cz));
                }
            }
        }
        assert_eq!(positions.len(), 512);

        let start = Instant::now();
        let chunks: HashMap<ChunkPos, Arc<Chunk>> = positions
            .par_iter()
            .map(|p| (*p, Arc::new(terrain.generate(*p))))
            .collect();
        let gen_time = start.elapsed();

        let mesh_start = Instant::now();
        let total: usize = positions
            .par_iter()
            .map(|pos| {
                let neighbors: [Option<Arc<Chunk>>; NEIGHBOR_COUNT] =
                    std::array::from_fn(|i| {
                        let dx = (i % 3) as i32 - 1;
                        let dz = ((i / 3) % 3) as i32 - 1;
                        let dy = (i / 9) as i32 - 1;
                        chunks
                            .get(&ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz))
                            .cloned()
                    });
                let nb = Neighborhood::build(*pos, &neighbors, &terrain);
                mesh_chunk(&nb).0.len()
            })
            .sum();
        let mesh_time = mesh_start.elapsed();
        let total_time = start.elapsed();

        println!(
            "512 chunks: generate {:.0} ms, mesh {:.0} ms, total {:.0} ms, {} vertices",
            gen_time.as_secs_f32() * 1000.0,
            mesh_time.as_secs_f32() * 1000.0,
            total_time.as_secs_f32() * 1000.0,
            total
        );
        assert!(total > 0, "the sample region must produce some geometry");
        assert!(
            total_time.as_secs_f32() < 4.0,
            "generating and meshing 512 chunks took {:.2}s",
            total_time.as_secs_f32()
        );
    }

    #[test]
    fn streaming_converges_and_stops_asking_for_work() {
        let mut w = World::new(1337);
        let center = ChunkPos::new(0, 4, 0);
        let mut uploaded = 0usize;
        // Bounded wait: the pipeline must settle, not churn forever.
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let r = w.stream(center);
            uploaded += r.ready.len();
            if w.is_idle() {
                break;
            }
            assert!(Instant::now() < deadline, "streaming did not converge");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(w.is_idle(), "streaming never reached a steady state");
        assert!(uploaded > 0, "no meshes were produced");
        assert!(
            !w.chunks.is_empty(),
            "streaming produced no resident chunks"
        );
        // The vertical policy has to actually cut the naive column count.
        let naive = w.chunks.keys().map(|p| (p.x, p.z)).collect::<HashSet<_>>().len()
            * CHUNK_COLUMN as usize;
        println!(
            "resident chunks {} across {} columns (naive full-column would be {})",
            w.chunks.len(),
            naive / CHUNK_COLUMN as usize,
            naive
        );
        assert!(
            w.chunks.len() * 2 < naive,
            "vertical culling saved less than half"
        );
        // A second stream at the same centre must ask for nothing new.
        let r = w.stream(center);
        assert!(r.ready.is_empty() && r.dropped.is_empty());
    }

    #[test]
    fn crossing_a_chunk_boundary_unloads_the_far_side() {
        let mut w = World::new(1337);
        let deadline = Instant::now() + std::time::Duration::from_secs(120);
        loop {
            w.stream(ChunkPos::new(0, 4, 0));
            if w.is_idle() {
                break;
            }
            assert!(Instant::now() < deadline, "streaming did not converge");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let before = w.chunks.len();
        let far = ChunkPos::new(40, 4, 0);
        let r = w.stream(far);
        assert!(!r.dropped.is_empty(), "moving far away must unload chunks");
        assert!(w.chunks.len() < before);
    }

    #[test]
    fn unloaded_space_still_answers_queries_from_generation() {
        let w = World::new(1337);
        assert!(w.chunks.is_empty());
        // No chunks resident, but the world still has a floor and a surface.
        assert_eq!(w.block_at(0, 0, 0), BlockId::BEDROCK);
        let s = w.surface_y(0, 0);
        assert!(!w.block_at(0, s, 0).is_air());
        assert!(w.block_at(0, s + 1, 0).is_air());
        assert_eq!(w.fill_ratio(0, s, 0), 1.0);
        assert!(w.sub_solid(0, s, 0, 0, 0, 0));
    }
}
