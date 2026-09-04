//! All tunable constants live here. Nothing gameplay-shaped should be a magic
//! number buried in a system -- if it is worth dialing in, it belongs in this file.

/// Blocks per chunk edge. Chunks are cubic.
pub const CHUNK_SIZE: usize = 16;
pub const CHUNK_SIZE_I: i32 = CHUNK_SIZE as i32;
/// Blocks per chunk.
pub const CHUNK_VOL: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// World height in blocks (16 chunks stacked).
pub const WORLD_HEIGHT: i32 = 256;
/// Vertical chunk count.
pub const CHUNK_COLUMN: i32 = WORLD_HEIGHT / CHUNK_SIZE_I;

/// Horizontal view radius, in chunks.
pub const RENDER_DISTANCE: i32 = 12;

/// Sub-voxels per block edge. 8 => 512 sub-voxels per block, stored sparsely.
pub const SUBVOX: usize = 8;

// --- camera / controls ---
pub const FOV_Y_DEG: f32 = 70.0;
pub const Z_NEAR: f32 = 0.1;
pub const Z_FAR: f32 = 1000.0;
pub const MOUSE_SENSITIVITY: f32 = 0.0022;
pub const FLY_SPEED: f32 = 18.0;
pub const FLY_SPEED_FAST: f32 = 60.0;

// --- look/feel ---
/// Ambient light floor; how dark a fully-occluded face gets.
pub const AO_STRENGTH: f32 = 0.55;
pub const SKY_COLOR: [f32; 3] = [0.48, 0.62, 0.82];
