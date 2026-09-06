//! All tunable constants live here. Nothing gameplay-shaped should be a magic
//! number buried in a system -- if it is worth dialing in, it belongs in this file.

// ---------------------------------------------------------------------------
// World geometry
// ---------------------------------------------------------------------------

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
pub const SUBVOX_I: i32 = SUBVOX as i32;
pub const SUBVOX_F: f32 = SUBVOX as f32;

// ---------------------------------------------------------------------------
// Chunk streaming
//
// The naive policy -- every one of the 16 vertical chunks across the whole
// horizontal disc -- is ~7,200 chunks for a 12-chunk radius, and the great
// majority of those are either pure sky or rock the player can never see. The
// policy below cuts that to roughly 2,000 by exploiting three facts:
//
//   1. Nothing above the terrain surface is ever solid, so chunks whose bottom
//      sits above the column's highest ground are never loaded at all. The
//      meshing skirt and `World::block_at` both fall back to generation, which
//      answers AIR for them correctly and for free.
//   2. From more than a few chunks away, the surface skin occludes everything
//      beneath it. Distant columns therefore load only the chunks that straddle
//      the ground, not the 60+ blocks of rock underneath.
//   3. Near the player, caves and overhangs matter, so those columns load a
//      full vertical window that follows the player down a mineshaft.
//
// A column upgrades from "far" to "near" as the player approaches, so nothing
// pops in wrong -- it just streams the deeper chunks in when they can matter.
// ---------------------------------------------------------------------------

/// Columns within this chunk radius load a full vertical window around the player.
pub const FULL_COLUMN_RADIUS: i32 = 6;
/// How far above and below the player's own chunk a near column reaches.
pub const VERTICAL_LOAD_RADIUS: i32 = 3;
/// Extra chunks loaded below a far column's lowest ground, so shallow caves and
/// cliff faces at the horizon still have geometry behind them.
pub const SURFACE_SKIN_DEPTH: i32 = 1;
/// Stride, in blocks, of the coarse height probe used to bound a column's
/// surface. 4 keeps it to 25 noise samples per chunk-column instead of 256.
pub const SURFACE_PROBE_STRIDE: i32 = 4;
/// Safety margin, in blocks, added to the coarse probe's min/max. Terrain cannot
/// move more than a few blocks between probe points at these noise frequencies,
/// so 8 is generous. Over-estimating only costs an empty chunk; under-estimating
/// would punch a hole in the world.
pub const SURFACE_PROBE_MARGIN: i32 = 8;
/// Chunks further than `RENDER_DISTANCE + this` are dropped.
pub const UNLOAD_MARGIN: i32 = 2;

/// Cap on generation jobs queued on the rayon pool at once. Bounds the memory
/// held by results waiting in the channel.
pub const GEN_JOBS_IN_FLIGHT: usize = 96;
/// Cap on meshing jobs queued on the rayon pool at once.
pub const MESH_JOBS_IN_FLIGHT: usize = 96;
/// Wall-clock slice per frame the main thread may spend creating GPU buffers.
pub const UPLOAD_BUDGET_MS: f32 = 4.0;
/// Hard cap on mesh uploads per frame, whatever the clock says.
pub const UPLOAD_BUDGET_COUNT: usize = 96;
/// Hard cap on generated chunks absorbed into the map per frame.
pub const GEN_INTAKE_PER_FRAME: usize = 192;

// ---------------------------------------------------------------------------
// Camera / controls
// ---------------------------------------------------------------------------

pub const FOV_Y_DEG: f32 = 70.0;
pub const Z_NEAR: f32 = 0.1;
pub const Z_FAR: f32 = 1000.0;
pub const MOUSE_SENSITIVITY: f32 = 0.0022;
/// Noclip fly speeds (F toggles noclip).
pub const FLY_SPEED: f32 = 18.0;
pub const FLY_SPEED_FAST: f32 = 60.0;

// ---------------------------------------------------------------------------
// Player physics
// ---------------------------------------------------------------------------

pub const PLAYER_WIDTH: f32 = 0.6;
pub const PLAYER_HEIGHT: f32 = 1.8;
/// Eye offset above the AABB's base.
pub const PLAYER_EYE_HEIGHT: f32 = 1.62;
pub const GRAVITY: f32 = -30.0;
pub const JUMP_SPEED: f32 = 9.0;
pub const WALK_SPEED: f32 = 4.6;
pub const SPRINT_SPEED: f32 = 7.4;
/// Horizontal acceleration blend, per second. Higher is snappier.
pub const GROUND_ACCEL: f32 = 22.0;
pub const AIR_ACCEL: f32 = 5.0;
/// Tallest lip the player walks up without jumping.
pub const STEP_HEIGHT: f32 = 1.05;
pub const TERMINAL_VELOCITY: f32 = -80.0;
/// Gap left between the player's AABB and a surface it lands against.
pub const COLLIDE_EPSILON: f32 = 1.0e-4;

// ---------------------------------------------------------------------------
// Interaction / mining
// ---------------------------------------------------------------------------

/// Maximum block-picking distance, in blocks.
pub const REACH: f32 = 6.0;
/// Radius of a chip pulse, in sub-voxels.
pub const CHIP_RADIUS: f32 = 2.7;
/// Seconds between chip pulses for a hardness-1.0 block. Scales with hardness.
pub const CHIP_INTERVAL: f32 = 0.055;
/// Seconds to charge a full-block smash on a hardness-1.0 block.
pub const SMASH_CHARGE: f32 = 0.30;
/// Seconds between block placements while the right button is held.
pub const PLACE_INTERVAL: f32 = 0.20;
/// Loudness of one chip pulse. Quiet: this is the safe, slow way to mine.
pub const NOISE_CHIP: f32 = 0.25;
/// Loudness of a full-block smash. Loud: fast, and every mob hears it.
pub const NOISE_SMASH: f32 = 1.0;
/// How far the targeting wireframe is pushed off the block surface.
pub const HIGHLIGHT_INFLATE: f32 = 0.004;

// ---------------------------------------------------------------------------
// Look and feel
//
// Colours in `block.rs` are authored in sRGB (what you would type into a colour
// picker). The shader converts them to linear before lighting, so these shade
// and AO numbers behave like real light multipliers instead of being squashed
// into the top of the sRGB curve. That is why the scene reads with contrast
// rather than as white mush.
// ---------------------------------------------------------------------------

/// Ambient light floor: what a fully-occluded corner keeps. Lower = deeper AO.
pub const AO_STRENGTH: f32 = 0.30;
/// Flat per-face light, in the mesher's face order:
/// `[+Y, -Y, +Z, -Z, +X, -X]`. A fake sun from +X/+Z keeps opposite walls apart.
/// Baked per-axis face shade: top, bottom, +Z, -Z, +X, -X.
///
/// These used to differ between the two faces of an axis (0.74 against 0.56),
/// which is a sun direction painted permanently into the geometry -- and once
/// the shader grew a real sun that moves with the time of day, the two fought
/// each other: a face could be baked dark while the actual sun was full on it.
/// They are symmetric per axis now, the way Minecraft's are. What they provide
/// is the constant readability of a voxel edge; the direction comes from the sun.
pub const FACE_SHADE: [f32; 6] = [1.00, 0.50, 0.80, 0.80, 0.62, 0.62];
/// Extra darkening applied to freshly carved sub-voxel surfaces, so craters read
/// as recessed rather than as bright new geometry.
pub const CARVE_SHADE: f32 = 0.78;
/// Sky, authored in sRGB.
pub const SKY_COLOR: [f32; 3] = [0.46, 0.62, 0.85];
/// Fog band, as a fraction of the render distance in blocks.
// Fog used to begin at 62% of the view distance, which put a wall of haze
// across the middle of every landscape and hid the terrain shape at exactly the
// range where you judge it. Starting it late leaves the land readable and keeps
// haze for what it is good at: the far horizon.
pub const FOG_START_FRAC: f32 = 0.84;
pub const FOG_END_FRAC: f32 = 1.0;
/// Colour of the targeting wireframe, authored in sRGB.
pub const HIGHLIGHT_COLOR: [f32; 3] = [0.03, 0.03, 0.05];

/// Render distance expressed in blocks, for fog and far-plane maths.
pub const fn render_distance_blocks() -> f32 {
    (RENDER_DISTANCE * CHUNK_SIZE_I) as f32
}

/// One sRGB channel to linear. The exact IEC 61966-2-1 curve, so CPU-side
/// conversions (the clear colour) match what the GPU does to the swapchain.
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// An sRGB triple to linear.
pub fn srgb_to_linear3(c: [f32; 3]) -> [f32; 3] {
    [
        srgb_to_linear(c[0]),
        srgb_to_linear(c[1]),
        srgb_to_linear(c[2]),
    ]
}

// ---------------------------------------------------------------------------
// Lighting
//
// Two channels, both 0..=15, packed into one byte per block. Block light comes
// from torches and does not care what time it is; sky light comes from open sky
// and is dimmed by the day cycle. They are combined with `max`, never summed,
// which is what makes a torch read as a torch rather than as a general brightener.
// ---------------------------------------------------------------------------

/// Maximum value of either light channel. Four bits, so this cannot exceed 15.
pub const MAX_LIGHT: u8 = 15;
/// Block light a torch emits. One below the maximum, so a torch is visibly a
/// local source rather than a small sun.
pub const TORCH_LIGHT: u8 = 14;
/// Shape of the level-to-brightness curve. Above 1.0 the light falls off faster
/// than the level does, which is what makes a torch read as a pool with an edge
/// rather than as a soft wash. Dialed in against screenshots: at 1.4 an unlit
/// cave is genuinely dark, a torch is bright for about four blocks, and open
/// ground at night sits around a fifth of full daylight.
pub const LIGHT_GAMMA: f32 = 1.4;
/// Brightness a completely unlit surface keeps. Not zero: a pitch-black cave
/// wall is indistinguishable from the void behind it, which reads as a bug.
pub const LIGHT_AMBIENT: f32 = 0.07;
/// How many levels midnight takes off the sky channel. 11 leaves open ground at
/// an effective level of 4 at night: navigable, but worth carrying a torch.
pub const NIGHT_SKY_SUBTRACT: u8 = 11;
/// Chunks re-meshed per frame when the day cycle crosses a sky-subtract step.
/// Light is baked into vertices, so a step change means the resident world has
/// to be rebuilt -- but slowly, in the background, off the frame's critical path.
pub const RELIGHT_CHUNKS_PER_FRAME: usize = 16;

// --- combat ---
/// Seconds between swings.
pub const ATTACK_INTERVAL: f32 = 0.40;
/// Horizontal impulse applied to a mob that is hit.
pub const KNOCKBACK: f32 = 7.0;
pub const KNOCKBACK_LIFT: f32 = 3.0;
/// Swinging is nearly as loud as smashing a block.
pub const NOISE_ATTACK: f32 = 0.8;
