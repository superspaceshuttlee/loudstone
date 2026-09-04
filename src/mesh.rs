//! Chunk meshing: face culling, per-vertex ambient occlusion, and sub-voxel geometry
//! for blocks the player has chipped.
//!
//! Meshing runs on worker threads, so it must not touch the world map. Instead the
//! main thread snapshots a `Neighborhood` -- the chunk plus a one-block skirt of its
//! neighbours -- and the worker meshes purely from that.

use crate::block::BlockId;
use crate::chunk::{Chunk, SubMask};
use crate::config::{AO_STRENGTH, CHUNK_SIZE, SUBVOX};
use std::collections::HashMap;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub color: [f32; 3],
    /// Combined face shade and ambient occlusion, already multiplied.
    pub light: f32,
}

impl Vertex {
    pub const ATTRS: [wgpu::VertexAttribute; 3] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32];

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

/// A meshing-ready snapshot of a chunk and its immediate surroundings.
pub struct Neighborhood {
    /// Padded block ids, indexed by [(y*PAD + z)*PAD + x] with a +1 offset.
    pub blocks: Vec<BlockId>,
    /// Sub-voxel masks for damaged blocks in the *centre* chunk only.
    pub damage: HashMap<u16, SubMask>,
    /// Damaged blocks do not fully occlude their neighbours' faces.
    pub damaged_pad: Vec<bool>,
    pub origin: (i32, i32, i32),
}

#[inline]
fn pad_index(x: usize, y: usize, z: usize) -> usize {
    (y * PAD + z) * PAD + x
}

impl Neighborhood {
    /// Build from the centre chunk plus a sampler that answers world-space block queries
    /// for the skirt. The sampler is called only for the 1-block border.
    pub fn build(chunk: &Chunk, mut sample: impl FnMut(i32, i32, i32) -> BlockId) -> Self {
        let origin = chunk.pos.origin();
        let mut blocks = vec![BlockId::AIR; PAD * PAD * PAD];
        let mut damaged_pad = vec![false; PAD * PAD * PAD];

        for y in 0..PAD {
            for z in 0..PAD {
                for x in 0..PAD {
                    let inside = (1..=CHUNK_SIZE).contains(&x)
                        && (1..=CHUNK_SIZE).contains(&y)
                        && (1..=CHUNK_SIZE).contains(&z);
                    let id = if inside {
                        chunk.get(x - 1, y - 1, z - 1)
                    } else {
                        sample(
                            origin.0 + x as i32 - 1,
                            origin.1 + y as i32 - 1,
                            origin.2 + z as i32 - 1,
                        )
                    };
                    blocks[pad_index(x, y, z)] = id;
                }
            }
        }

        for &i in chunk.damage_map().keys() {
            let li = i as usize;
            let x = li % CHUNK_SIZE;
            let z = (li / CHUNK_SIZE) % CHUNK_SIZE;
            let y = li / (CHUNK_SIZE * CHUNK_SIZE);
            damaged_pad[pad_index(x + 1, y + 1, z + 1)] = true;
        }

        Self {
            blocks,
            damage: chunk.damage_map().clone(),
            damaged_pad,
            origin,
        }
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
}

/// The six cube faces, as (normal, shade). Shade alone gives the scene enough form
/// that no texture or normal map is needed.
const FACES: [([i32; 3], f32); 6] = [
    ([0, 1, 0], 1.00),  // +Y top
    ([0, -1, 0], 0.45), // -Y bottom
    ([0, 0, 1], 0.80),  // +Z
    ([0, 0, -1], 0.80), // -Z
    ([1, 0, 0], 0.65),  // +X
    ([-1, 0, 0], 0.65), // -X
];

/// Corner offsets for each face, counter-clockwise seen from outside.
const FACE_CORNERS: [[[f32; 3]; 4]; 6] = [
    // +Y
    [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
    // -Y
    [[0., 0., 1.], [0., 0., 0.], [1., 0., 0.], [1., 0., 1.]],
    // +Z
    [[1., 0., 1.], [0., 0., 1.], [0., 1., 1.], [1., 1., 1.]],
    // -Z
    [[0., 0., 0.], [1., 0., 0.], [1., 1., 0.], [0., 1., 0.]],
    // +X
    [[1., 0., 0.], [1., 0., 1.], [1., 1., 1.], [1., 1., 0.]],
    // -X
    [[0., 0., 1.], [0., 0., 0.], [0., 1., 0.], [0., 1., 1.]],
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
                let li = crate::chunk::local_index(x as usize, y as usize, z as usize);
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
) {
    let base = verts.len() as u32;
    for (i, c) in corners.iter().enumerate() {
        verts.push(Vertex {
            pos: *c,
            color,
            light: light[i],
        });
    }
    // Flip the triangle split along the darker diagonal, otherwise strong AO
    // gradients show a visible crease across the quad.
    if light[0] + light[2] > light[1] + light[3] {
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    } else {
        indices.extend_from_slice(&[base + 1, base + 2, base + 3, base + 1, base + 3, base]);
    }
}

fn emit_full_block(
    nb: &Neighborhood,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    x: i32,
    y: i32,
    z: i32,
    id: BlockId,
) {
    let color = id.color();
    for (f, (n, shade)) in FACES.iter().enumerate() {
        if nb.occludes(x + n[0], y + n[1], z + n[2]) {
            continue;
        }
        let (t, b) = tangents(*n);
        let mut light = [0.0f32; 4];
        let mut corners = [[0.0f32; 3]; 4];
        let np = [x + n[0], y + n[1], z + n[2]];
        for (i, c) in FACE_CORNERS[f].iter().enumerate() {
            corners[i] = [x as f32 + c[0], y as f32 + c[1], z as f32 + c[2]];
            // Which side of each tangent axis this corner sits on.
            let du = if dot_sign(*c, t) { 1 } else { -1 };
            let dv = if dot_sign(*c, b) { 1 } else { -1 };
            let s1 = nb.occludes(np[0] + t[0] * du, np[1] + t[1] * du, np[2] + t[2] * du);
            let s2 = nb.occludes(np[0] + b[0] * dv, np[1] + b[1] * dv, np[2] + b[2] * dv);
            let cn = nb.occludes(
                np[0] + t[0] * du + b[0] * dv,
                np[1] + t[1] * du + b[1] * dv,
                np[2] + t[2] * du + b[2] * dv,
            );
            light[i] = shade * corner_ao(s1, s2, cn);
        }
        push_quad(verts, indices, corners, color, light);
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
    let color = id.color();
    let s = 1.0 / SUBVOX as f32;
    let n = SUBVOX as i32;

    for sy in 0..n {
        for sz in 0..n {
            for sx in 0..n {
                if !mask.get(sx as usize, sy as usize, sz as usize) {
                    continue;
                }
                for (f, (nrm, shade)) in FACES.iter().enumerate() {
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
                    let mut corners = [[0.0f32; 3]; 4];
                    for (i, c) in FACE_CORNERS[f].iter().enumerate() {
                        corners[i] = [
                            x as f32 + (sx as f32 + c[0]) * s,
                            y as f32 + (sy as f32 + c[1]) * s,
                            z as f32 + (sz as f32 + c[2]) * s,
                        ];
                    }
                    // Carved surfaces take a flat shade; per-sub-voxel AO is not
                    // worth the cost at this scale.
                    let l = shade * 0.9;
                    push_quad(verts, indices, corners, color, [l, l, l, l]);
                }
            }
        }
    }
}
