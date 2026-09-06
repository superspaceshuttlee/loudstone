//! Chunk meshing: face culling, per-vertex ambient occlusion, and sub-voxel geometry
//! for blocks the player has chipped.
//!
//! Meshing runs on rayon worker threads and must never touch the live world map.
//! The main thread hands a job the 3x3x3 block of `Arc<Chunk>` around the chunk
//! (27 atomic refcount bumps, no copying) plus the shared `TerrainGen`, and the
//! worker builds its own padded `Neighborhood` snapshot from that.
//!
//! Two things in here were wrong and are worth calling out, because between them
//! they accounted for most of what the world looked like:
//!
//!  * Vertices were emitted in **chunk-local** coordinates while the renderer
//!    applied no per-chunk transform, so every chunk in the world was drawn
//!    stacked inside the same 16x16x16 box at the origin.
//!  * The four side faces were wound **clockwise** seen from outside, so
//!    back-face culling deleted every vertical face and left horizontal plates
//!    with see-through gaps between them.

use crate::block::BlockId;
use crate::chunk::{local_index, Chunk, ChunkPos, SubMask};
use crate::config::{AO_STRENGTH, CARVE_SHADE, CHUNK_SIZE, FACE_SHADE, SUBVOX};
use crate::light;
use crate::worldgen::TerrainGen;
use std::collections::HashMap;
use std::sync::Arc;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    /// World space. The renderer draws every chunk with the same identity
    /// transform, so this must not be chunk-local.
    pub pos: [f32; 3],
    pub color: [f32; 3],
    /// Combined face shade and ambient occlusion, already multiplied.
    pub light: f32,
    /// Atlas coordinates. Textures multiply the colour and the light rather
    /// than replacing either, so ambient occlusion and torchlight survive.
    pub uv: [f32; 2],
}

impl Vertex {
    pub const ATTRS: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32, 3 => Float32x2];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

/// Padded edge length: the chunk plus one block of skirt on each side.
const PAD: usize = CHUNK_SIZE + 2;

/// Number of chunks in the 3x3x3 block a mesh job needs. Diagonals are included
/// because ambient occlusion samples the edge and corner neighbours of a face.
pub const NEIGHBOR_COUNT: usize = 27;

/// Index into a `[_; NEIGHBOR_COUNT]` for a chunk offset in -1..=1 on each axis.
#[inline]
pub fn neighbor_index(dx: i32, dy: i32, dz: i32) -> usize {
    debug_assert!((-1..=1).contains(&dx) && (-1..=1).contains(&dy) && (-1..=1).contains(&dz));
    (((dy + 1) * 3 + (dz + 1)) * 3 + (dx + 1)) as usize
}

/// A meshing-ready snapshot of a chunk and its immediate surroundings.
pub struct Neighborhood {
    /// Padded block ids, indexed by [(y*PAD + z)*PAD + x] with a +1 offset.
    blocks: Vec<BlockId>,
    /// Packed sky/block light, in the same padded layout as `blocks`. The light
    /// travels in the snapshot exactly like the block ids do, so a mesh worker
    /// never has to reach back into the live world for it.
    light: Vec<u8>,
    /// Sub-voxel masks for damaged blocks in the *centre* chunk only.
    damage: HashMap<u16, SubMask>,
    /// Damaged blocks do not fully occlude their neighbours' faces.
    damaged_pad: Vec<bool>,
    /// Levels the current time of day takes off the sky channel. Baked in here
    /// because the vertex format carries one combined float and the renderer's
    /// uniform block is not this module's to extend -- see the integration note.
    sky_subtract: f32,
    pub pos: ChunkPos,
    pub origin: (i32, i32, i32),
}

#[inline]
fn pad_index(x: usize, y: usize, z: usize) -> usize {
    (y * PAD + z) * PAD + x
}

impl Neighborhood {
    /// Build the padded snapshot from the 3x3x3 chunk block. Any neighbour that
    /// is absent falls back to raw terrain generation, which is exactly right:
    /// an unloaded chunk holds no player edits, so generation is the truth for
    /// it. That is what keeps chunk seams correct while the world streams in.
    pub fn build(
        pos: ChunkPos,
        neighbors: &[Option<Arc<Chunk>>; NEIGHBOR_COUNT],
        terrain: &TerrainGen,
        sky_subtract: u8,
    ) -> Self {
        let origin = pos.origin();
        let mut blocks = vec![BlockId::AIR; PAD * PAD * PAD];
        let mut light = vec![0u8; PAD * PAD * PAD];
        let mut damaged_pad = vec![false; PAD * PAD * PAD];

        // Surface heights for the 18x18 footprint, computed once and shared by
        // all 18 y-layers. Only needed when some neighbour is missing, which is
        // the minority case once the interior of the render distance is loaded.
        let need_terrain = neighbors.iter().any(|n| n.is_none());
        let heights: Vec<i32> = if need_terrain {
            let mut h = vec![0i32; PAD * PAD];
            for pz in 0..PAD {
                for px in 0..PAD {
                    h[pz * PAD + px] = terrain
                        .height_at(origin.0 + px as i32 - 1, origin.2 + pz as i32 - 1);
                }
            }
            h
        } else {
            Vec::new()
        };

        let cs = CHUNK_SIZE as i32;
        for py in 0..PAD {
            let ly = py as i32 - 1;
            let dy = chunk_offset(ly);
            let wy = origin.1 + ly;
            for pz in 0..PAD {
                let lz = pz as i32 - 1;
                let dz = chunk_offset(lz);
                let wz = origin.2 + lz;
                for px in 0..PAD {
                    let lx = px as i32 - 1;
                    let dx = chunk_offset(lx);
                    let wx = origin.0 + lx;

                    let i = pad_index(px, py, pz);
                    match &neighbors[neighbor_index(dx, dy, dz)] {
                        Some(c) => {
                            let (ix, iy, iz) = (
                                lx.rem_euclid(cs) as usize,
                                ly.rem_euclid(cs) as usize,
                                lz.rem_euclid(cs) as usize,
                            );
                            blocks[i] = c.get(ix, iy, iz);
                            light[i] = c.light(ix, iy, iz);
                            if c.has_damage() {
                                damaged_pad[i] = c.mask(ix, iy, iz).is_some();
                            }
                        }
                        None => {
                            let surface = heights[pz * PAD + px];
                            blocks[i] = terrain.block_at_surface(wx, wy, wz, surface);
                            // Must match what `light::seed_chunk` would produce
                            // for this column, or every seam against a chunk
                            // that has not streamed in yet shows a light step.
                            light[i] = light::pack(0, light::fallback_sky(surface, wy));
                        }
                    }
                }
            }
        }

        let damage = neighbors[neighbor_index(0, 0, 0)]
            .as_ref()
            .map(|c| c.damage_map().clone())
            .unwrap_or_default();

        Self {
            blocks,
            light,
            damage,
            damaged_pad,
            sky_subtract: sky_subtract as f32,
            pos,
            origin,
        }
    }

    /// Build straight from one chunk with an empty surround. Test helper.
    #[cfg(test)]
    pub fn isolated(chunk: Arc<Chunk>, terrain: &TerrainGen) -> Self {
        let pos = chunk.pos;
        let mut n: [Option<Arc<Chunk>>; NEIGHBOR_COUNT] = std::array::from_fn(|_| None);
        n[neighbor_index(0, 0, 0)] = Some(chunk);
        Self::build(pos, &n, terrain, 0)
    }

    #[inline]
    fn at(&self, x: i32, y: i32, z: i32) -> BlockId {
        // x/y/z are chunk-local, valid over -1..=CHUNK_SIZE.
        self.blocks[pad_index((x + 1) as usize, (y + 1) as usize, (z + 1) as usize)]
    }

    #[inline]
    fn occludes(&self, x: i32, y: i32, z: i32) -> bool {
        let i = pad_index((x + 1) as usize, (y + 1) as usize, (z + 1) as usize);
        self.blocks[i].is_opaque() && !self.damaged_pad[i]
    }

    /// The block at a chunk-local coordinate, valid over -1..=CHUNK_SIZE.
    #[inline]
    fn block_at_local(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.blocks[pad_index((x + 1) as usize, (y + 1) as usize, (z + 1) as usize)]
    }

    /// Packed light at a chunk-local coordinate, valid over -1..=CHUNK_SIZE.
    #[inline]
    fn light_at(&self, x: i32, y: i32, z: i32) -> u8 {
        self.light[pad_index((x + 1) as usize, (y + 1) as usize, (z + 1) as usize)]
    }

    /// Whether a cell contributes to the smooth-lighting average. Blocks that
    /// stop light hold no light of their own, so including them would drag every
    /// corner next to a wall towards black.
    #[inline]
    fn transmits(&self, x: i32, y: i32, z: i32) -> bool {
        !light::blocks_light(self.at(x, y, z))
    }

    /// Brightness at one face corner, averaged over the (up to four) cells on
    /// the lit side of the face that touch it. This is Minecraft's smooth
    /// lighting: it is what turns hard per-block light steps into a gradient.
    fn corner_light(&self, samples: [[i32; 3]; 4]) -> f32 {
        let mut block = 0.0f32;
        let mut sky = 0.0f32;
        let mut n = 0.0f32;
        for s in samples {
            if !self.transmits(s[0], s[1], s[2]) {
                continue;
            }
            let p = self.light_at(s[0], s[1], s[2]);
            block += light::block_of(p) as f32;
            sky += light::sky_of(p) as f32;
            n += 1.0;
        }
        if n == 0.0 {
            // Fully boxed in: the face is looking into solid rock.
            return light::vertex_light(0.0, 0.0, self.sky_subtract);
        }
        light::vertex_light(block / n, sky / n, self.sky_subtract)
    }

    /// Flat light for one cell, used where smoothing is not worth the cost.
    fn flat_light(&self, x: i32, y: i32, z: i32) -> f32 {
        let p = self.light_at(x, y, z);
        light::vertex_light(
            light::block_of(p) as f32,
            light::sky_of(p) as f32,
            self.sky_subtract,
        )
    }
}

/// Which neighbour chunk a padded local coordinate falls into.
#[inline]
fn chunk_offset(local: i32) -> i32 {
    if local < 0 {
        -1
    } else if local >= CHUNK_SIZE as i32 {
        1
    } else {
        0
    }
}

/// The six cube face normals, in the same order as `config::FACE_SHADE`.
pub const FACE_NORMALS: [[i32; 3]; 6] = [
    [0, 1, 0],  // +Y top
    [0, -1, 0], // -Y bottom
    [0, 0, 1],  // +Z
    [0, 0, -1], // -Z
    [1, 0, 0],  // +X
    [-1, 0, 0], // -X
];

/// Where a unit-cube corner lands in its face's texture, before the tile rect
/// is applied. `v` runs downward like an image row, so side faces map `v = 0`
/// to the top of the block -- which is what puts the grass fringe on a grass
/// side, and the bark the right way up on a log.
fn face_uv(face: usize, c: [f32; 3]) -> [f32; 2] {
    match face {
        0 => [c[0], c[2]],           // +Y top
        1 => [c[0], 1.0 - c[2]],     // -Y bottom
        2 => [1.0 - c[0], 1.0 - c[1]], // +Z
        3 => [c[0], 1.0 - c[1]],     // -Z
        4 => [c[2], 1.0 - c[1]],     // +X
        _ => [1.0 - c[2], 1.0 - c[1]], // -X
    }
}

/// Map a face-local UV into a tile of the atlas.
fn tile_uv(rect: [f32; 4], uv: [f32; 2]) -> [f32; 2] {
    [
        rect[0] + (rect[2] - rect[0]) * uv[0],
        rect[1] + (rect[3] - rect[1]) * uv[1],
    ]
}

/// Corner offsets for each face, counter-clockwise seen from **outside** the
/// cube. `unit_cube_edges` in `gfx.rs` and the winding test below both depend on
/// this staying true: cross(v1 - v0, v2 - v0) must point along the face normal.
pub const FACE_CORNERS: [[[f32; 3]; 4]; 6] = [
    // +Y
    [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
    // -Y
    [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
    // +Z
    [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
    // -Z
    [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]],
    // +X
    [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]],
    // -X
    [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
];

/// Ambient occlusion for one face corner, from the two edge-adjacent blocks and the
/// diagonal. This is the standard 0..3 voxel AO term.
fn corner_ao(side1: bool, side2: bool, corner: bool) -> f32 {
    let level = if side1 && side2 {
        0
    } else {
        3 - (side1 as u8 + side2 as u8 + corner as u8)
    };
    AO_STRENGTH + (1.0 - AO_STRENGTH) * (level as f32 / 3.0)
}

/// The two tangent axes for a face normal, used to sample AO neighbours.
fn tangents(n: [i32; 3]) -> ([i32; 3], [i32; 3]) {
    if n[1] != 0 {
        ([1, 0, 0], [0, 0, 1])
    } else if n[0] != 0 {
        ([0, 0, 1], [0, 1, 0])
    } else {
        ([1, 0, 0], [0, 1, 0])
    }
}

pub fn mesh_chunk(nb: &Neighborhood) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for y in 0..CHUNK_SIZE as i32 {
        for z in 0..CHUNK_SIZE as i32 {
            for x in 0..CHUNK_SIZE as i32 {
                let id = nb.at(x, y, z);
                if id.is_air() {
                    continue;
                }
                let li = local_index(x as usize, y as usize, z as usize);
                match nb.damage.get(&li) {
                    Some(mask) => {
                        emit_subvoxel_block(nb, &mut verts, &mut indices, x, y, z, id, mask)
                    }
                    None => emit_full_block(nb, &mut verts, &mut indices, x, y, z, id),
                }
            }
        }
    }
    (verts, indices)
}

fn push_quad(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    color: [f32; 3],
    light: [f32; 4],
    uvs: [[f32; 2]; 4],
) {
    let base = verts.len() as u32;
    for (i, c) in corners.iter().enumerate() {
        verts.push(Vertex {
            pos: *c,
            color,
            light: light[i],
            uv: uvs[i],
        });
    }
    // Flip the triangle split along the darker diagonal, otherwise strong AO
    // gradients show a visible crease across the quad. Both splits keep the
    // 0->1->2->3 winding, so face orientation is unaffected.
    if light[0] + light[2] > light[1] + light[3] {
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        indices.extend_from_slice(&[base + 1, base + 2, base + 3, base + 1, base + 3, base]);
    }
}

/// Textures now carry a block's colour, so the vertex tint must be neutral --
/// multiplying the atlas by the old flat colour would square it and turn every
/// surface muddy. The tint stays in the vertex format because biome-tinted
/// foliage will want it.
const NO_TINT: [f32; 3] = [1.0, 1.0, 1.0];

fn emit_full_block(
    nb: &Neighborhood,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    x: i32,
    y: i32,
    z: i32,
    id: BlockId,
) {
    let color = NO_TINT;
    let (ox, oy, oz) = nb.origin;
    // Leaves, water and plants are non-opaque, so they do not occlude their
    // neighbours -- but two of the SAME non-opaque block share an invisible
    // interior face, and emitting it is pure waste. Without this a tree canopy
    // meshes every face of every leaf block it contains, which is where the
    // overwhelming majority of this world's triangles were going.
    let self_culls = !id.is_opaque();
    for (f, n) in FACE_NORMALS.iter().enumerate() {
        let (nx, ny, nz) = (x + n[0], y + n[1], z + n[2]);
        if nb.occludes(nx, ny, nz) {
            continue;
        }
        if self_culls && nb.block_at_local(nx, ny, nz) == id {
            continue;
        }
        let shade = FACE_SHADE[f];
        let rect = crate::texture::tile_uv_rect(crate::texture::block_tile(id, f));
        let (t, b) = tangents(*n);
        let mut light = [0.0f32; 4];
        let mut corners = [[0.0f32; 3]; 4];
        let mut uvs = [[0.0f32; 2]; 4];
        let np = [x + n[0], y + n[1], z + n[2]];
        for (i, c) in FACE_CORNERS[f].iter().enumerate() {
            // World space, not chunk space: the renderer applies no transform.
            corners[i] = [
                (ox + x) as f32 + c[0],
                (oy + y) as f32 + c[1],
                (oz + z) as f32 + c[2],
            ];
            uvs[i] = tile_uv(rect, face_uv(f, *c));
            // Which side of each tangent axis this corner sits on.
            let du = if dot_sign(*c, t) { 1 } else { -1 };
            let dv = if dot_sign(*c, b) { 1 } else { -1 };
            let side1 = [np[0] + t[0] * du, np[1] + t[1] * du, np[2] + t[2] * du];
            let side2 = [np[0] + b[0] * dv, np[1] + b[1] * dv, np[2] + b[2] * dv];
            let diag = [
                np[0] + t[0] * du + b[0] * dv,
                np[1] + t[1] * du + b[1] * dv,
                np[2] + t[2] * du + b[2] * dv,
            ];
            let s1 = nb.occludes(side1[0], side1[1], side1[2]);
            let s2 = nb.occludes(side2[0], side2[1], side2[2]);
            let cn = nb.occludes(diag[0], diag[1], diag[2]);
            // The same four cells the AO term samples also carry the light that
            // reaches this corner, so smoothing costs no extra lookups worth
            // naming: face shade x ambient occlusion x smoothed sky/block light.
            let lit = nb.corner_light([np, side1, side2, diag]);
            light[i] = shade * corner_ao(s1, s2, cn) * lit;
        }
        push_quad(verts, indices, corners, color, light, uvs);
    }
}

/// True when the corner sits on the positive side of the given axis.
#[inline]
fn dot_sign(c: [f32; 3], axis: [i32; 3]) -> bool {
    let v = c[0] * axis[0] as f32 + c[1] * axis[1] as f32 + c[2] * axis[2] as f32;
    v > 0.5
}

/// Emit geometry for a chipped block, one small cube per surviving sub-voxel.
/// Interior sub-voxel faces are culled against their neighbours in the same mask,
/// which is what keeps a lightly-scratched block from costing 3000 triangles.
#[allow(clippy::too_many_arguments)]
fn emit_subvoxel_block(
    nb: &Neighborhood,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    x: i32,
    y: i32,
    z: i32,
    id: BlockId,
    mask: &SubMask,
) {
    let color = NO_TINT;
    let s = 1.0 / SUBVOX as f32;
    let n = SUBVOX as i32;
    let (ox, oy, oz) = nb.origin;

    // A carved block still stops light (see the module docs on `light.rs`), so
    // it holds none of its own. Its faces are lit from the neighbouring cells:
    // an outward face takes the cell it looks at, and a face inside the crater
    // takes the brightest cell adjacent to the block, which is what makes a
    // crater in a lit wall read as lit rather than as a black hole.
    let mut face_light = [0.0f32; 6];
    for (f, nrm) in FACE_NORMALS.iter().enumerate() {
        face_light[f] = nb.flat_light(x + nrm[0], y + nrm[1], z + nrm[2]);
    }
    let interior_light = face_light.iter().copied().fold(0.0f32, f32::max);

    for sy in 0..n {
        for sz in 0..n {
            for sx in 0..n {
                if !mask.get(sx as usize, sy as usize, sz as usize) {
                    continue;
                }
                for (f, nrm) in FACE_NORMALS.iter().enumerate() {
                    let (nx, ny, nz) = (sx + nrm[0], sy + nrm[1], sz + nrm[2]);
                    let inside =
                        (0..n).contains(&nx) && (0..n).contains(&ny) && (0..n).contains(&nz);
                    let hidden = if inside {
                        mask.get(nx as usize, ny as usize, nz as usize)
                    } else {
                        // Face lies on the block boundary: hide it if the neighbouring
                        // block is solid.
                        nb.occludes(x + nrm[0], y + nrm[1], z + nrm[2])
                    };
                    if hidden {
                        continue;
                    }
                    let rect = crate::texture::tile_uv_rect(crate::texture::block_tile(id, f));
                    let mut corners = [[0.0f32; 3]; 4];
                    let mut uvs = [[0.0f32; 2]; 4];
                    for (i, c) in FACE_CORNERS[f].iter().enumerate() {
                        // Block-local position, so the texture reads as one
                        // continuous surface across a chipped face rather than
                        // repeating on every sub-voxel.
                        let local = [
                            (sx as f32 + c[0]) * s,
                            (sy as f32 + c[1]) * s,
                            (sz as f32 + c[2]) * s,
                        ];
                        corners[i] = [
                            (ox + x) as f32 + local[0],
                            (oy + y) as f32 + local[1],
                            (oz + z) as f32 + local[2],
                        ];
                        uvs[i] = tile_uv(rect, face_uv(f, local));
                    }
                    // Carved surfaces take a flat shade; per-sub-voxel AO is not
                    // worth the cost at this scale.
                    let lit = if inside { interior_light } else { face_light[f] };
                    let l = FACE_SHADE[f] * CARVE_SHADE * lit;
                    push_quad(verts, indices, corners, color, [l, l, l, l], uvs);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CHUNK_VOL;

    fn empty_neighbors() -> [Option<Arc<Chunk>>; NEIGHBOR_COUNT] {
        std::array::from_fn(|_| None)
    }

    /// A flat-world generator stand-in: everything below y=0 is bedrock, so an
    /// isolated chunk high in the sky has an all-air surround.
    fn sky_terrain() -> TerrainGen {
        TerrainGen::new(7)
    }

    fn solid_chunk(pos: ChunkPos, id: BlockId) -> Arc<Chunk> {
        Arc::new(Chunk::from_blocks(pos, Box::new([id; CHUNK_VOL])))
    }

    fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    /// Regression guard for the bug that produced the see-through lattice: the
    /// four side faces were wound clockwise from outside, so back-face culling
    /// (FrontFace::Ccw + CullMode::Back) discarded every vertical face.
    #[test]
    fn every_face_is_wound_counter_clockwise_from_outside() {
        for (f, n) in FACE_NORMALS.iter().enumerate() {
            let c = FACE_CORNERS[f];
            let e1 = [c[1][0] - c[0][0], c[1][1] - c[0][1], c[1][2] - c[0][2]];
            let e2 = [c[2][0] - c[0][0], c[2][1] - c[0][1], c[2][2] - c[0][2]];
            let k = cross(e1, e2);
            let dot = k[0] * n[0] as f32 + k[1] * n[1] as f32 + k[2] * n[2] as f32;
            assert!(
                dot > 0.0,
                "face {f} (normal {n:?}) is wound backwards: cross = {k:?}"
            );
            // The second triangle of the quad must agree.
            let e3 = [c[2][0] - c[0][0], c[2][1] - c[0][1], c[2][2] - c[0][2]];
            let e4 = [c[3][0] - c[0][0], c[3][1] - c[0][1], c[3][2] - c[0][2]];
            let k2 = cross(e3, e4);
            let dot2 = k2[0] * n[0] as f32 + k2[1] * n[1] as f32 + k2[2] * n[2] as f32;
            assert!(dot2 > 0.0, "face {f} second triangle is wound backwards");
        }
    }

    /// A fully solid chunk with fully solid neighbours has no visible surface.
    #[test]
    fn enclosed_solid_chunk_meshes_to_zero_vertices() {
        let pos = ChunkPos::new(0, 2, 0);
        let mut n = empty_neighbors();
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    n[neighbor_index(dx, dy, dz)] = Some(solid_chunk(
                        ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz),
                        BlockId::STONE,
                    ));
                }
            }
        }
        let nb = Neighborhood::build(pos, &n, &sky_terrain(), 0);
        let (verts, indices) = mesh_chunk(&nb);
        assert_eq!(verts.len(), 0, "buried chunk emitted geometry");
        assert_eq!(indices.len(), 0);
    }

    /// One block alone in the sky shows all six of its faces and nothing else.
    #[test]
    fn isolated_block_meshes_to_exactly_six_quads() {
        // y=15 chunk is far above any terrain, so the skirt fallback is air.
        let pos = ChunkPos::new(0, 15, 0);
        let mut c = Chunk::new(pos);
        c.set(8, 8, 8, BlockId::STONE);
        c.generated = true;

        let nb = Neighborhood::isolated(Arc::new(c), &sky_terrain());
        let (verts, indices) = mesh_chunk(&nb);
        assert_eq!(verts.len(), 24, "expected 6 quads = 24 vertices");
        assert_eq!(indices.len(), 36, "expected 6 quads = 12 triangles");
    }

    /// And it lands in world space, not chunk space. This is the other half of
    /// the "everything stacked at the origin" bug.
    #[test]
    fn vertices_are_emitted_in_world_space() {
        let pos = ChunkPos::new(3, 15, -2);
        let mut c = Chunk::new(pos);
        c.set(0, 0, 0, BlockId::STONE);
        c.generated = true;

        let nb = Neighborhood::isolated(Arc::new(c), &sky_terrain());
        let (verts, _) = mesh_chunk(&nb);
        assert!(!verts.is_empty());
        let (ox, oy, oz) = pos.origin();
        for v in &verts {
            assert!(
                v.pos[0] >= ox as f32 && v.pos[0] <= ox as f32 + 1.0,
                "x {} outside the block's world column {}", v.pos[0], ox
            );
            assert!(v.pos[1] >= oy as f32 && v.pos[1] <= oy as f32 + 1.0);
            assert!(v.pos[2] >= oz as f32 && v.pos[2] <= oz as f32 + 1.0);
        }
    }

    #[test]
    fn seams_against_an_absent_neighbour_use_generation() {
        // A chunk buried inside solid terrain: its neighbours are not resident,
        // so the skirt is sampled from the generator. Underground stone is
        // enclosed, so no face of the outer shell should survive.
        let terrain = TerrainGen::new(1337);
        let pos = ChunkPos::new(0, 1, 0); // y 16..32, well under the surface
        let chunk = Arc::new(terrain.generate(pos));
        assert!(!chunk.is_empty(), "test picked an empty chunk");

        let nb = Neighborhood::isolated(chunk.clone(), &terrain);
        let (verts_absent, _) = mesh_chunk(&nb);

        // Now do it again with the real neighbours resident. Identical result.
        let mut n = empty_neighbors();
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let p = ChunkPos::new(pos.x + dx, pos.y + dy, pos.z + dz);
                    n[neighbor_index(dx, dy, dz)] = Some(if (dx, dy, dz) == (0, 0, 0) {
                        chunk.clone()
                    } else {
                        Arc::new(terrain.generate(p))
                    });
                }
            }
        }
        let nb2 = Neighborhood::build(pos, &n, &terrain, 0);
        let (verts_present, _) = mesh_chunk(&nb2);
        assert_eq!(
            verts_absent.len(),
            verts_present.len(),
            "the terrain fallback must agree with resident neighbours"
        );
    }

    #[test]
    fn a_carved_block_emits_sub_voxel_geometry() {
        let pos = ChunkPos::new(0, 15, 0);
        let mut c = Chunk::new(pos);
        c.set(8, 8, 8, BlockId::STONE);
        c.carve(8, 8, 8, 0, 0, 0);
        c.generated = true;

        let nb = Neighborhood::isolated(Arc::new(c), &sky_terrain());
        let (verts, _) = mesh_chunk(&nb);
        // 511 surviving sub-cubes; the exterior shell plus the three faces of
        // the missing corner. Far more than six quads, and strictly bounded.
        assert!(
            verts.len() > 24 && verts.len() < 511 * 24,
            "sub-voxel mesh had {} vertices",
            verts.len()
        );
    }

    #[test]
    fn air_chunk_meshes_to_nothing() {
        let pos = ChunkPos::new(0, 15, 0);
        let mut c = Chunk::new(pos);
        c.generated = true;
        let nb = Neighborhood::isolated(Arc::new(c), &sky_terrain());
        let (verts, indices) = mesh_chunk(&nb);
        assert!(verts.is_empty() && indices.is_empty());
    }
}
