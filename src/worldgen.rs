//! Deterministic terrain generation.
//!
//! Everything here is a pure function of the seed and the block coordinate, so a
//! chunk generated on any rayon worker at any time produces identical bytes.
//! There is no global mutable state and no order-dependent randomness: variation
//! comes from noise fields and from hashing the seed together with a coordinate.
//!
//! # The three views of the same world
//!
//! Three call paths must agree *exactly*, or chunk seams tear:
//!
//! * [`TerrainGen::generate`] fills a whole chunk, hoisting per-column work out
//!   of the inner loop and stamping veins and trees as volumes.
//! * [`TerrainGen::block_at_surface`] answers one coordinate. `mesh.rs` uses it
//!   for the meshing skirt whenever a neighbouring chunk is not resident.
//! * [`TerrainGen::block_at`] is the same thing without the surface hint, used
//!   by `World::block_at` for unloaded space.
//!
//! They agree because they share the same helpers. `generation_matches_the_point_query`
//! asserts it block for block, which is the test to keep green above all others.
//!
//! # Trees across chunk boundaries
//!
//! A tree is never "written into a neighbour". The world is tiled by
//! [`tuning::TREE_CELL`]-sized cells; each cell deterministically hashes to at
//! most one tree, and a chunk simply *reads* every cell that could reach into
//! it. A trunk in one chunk and its leaves in the next therefore agree without
//! the two chunks ever communicating, in either generation order.

use crate::block::BlockId;
use crate::chunk::{Chunk, ChunkPos, local_index};
use crate::config::{
    CHUNK_SIZE, CHUNK_SIZE_I, CHUNK_VOL, SURFACE_PROBE_MARGIN, SURFACE_PROBE_STRIDE, WORLD_HEIGHT,
};
use noise::{NoiseFn, Perlin};
use std::cell::RefCell;

// ===========================================================================
//  TUNING BLOCK -- every number the world's shape depends on lives here.
//  Nothing below this module invents a magic constant of its own.
// ===========================================================================
pub mod tuning {
    use crate::block::BlockId;

    // --- sea, base height, ceiling -----------------------------------------

    /// Everything at or below this y that is not solid ground becomes water.
    pub const WATER_LEVEL: i32 = 63;
    /// Height the continent spline is measured from.
    pub const BASE_HEIGHT: f64 = 64.0;
    /// Terrain is clamped here so tall trees still fit under the world ceiling.
    pub const MAX_TERRAIN_Y: i32 = 208;

    // --- continents ---------------------------------------------------------

    /// Broad land/sea shape. Lower is bigger continents.
    pub const CONTINENT_SCALE: f64 = 0.00125;
    pub const CONTINENT_OCTAVES: u32 = 5;
    /// Multiplier applied before the spline. Perlin fbm rarely leaves +-0.4 on
    /// its own, so without a gain the spline's extremes would never be reached
    /// and the world would be all shoreline.
    pub const CONTINENT_GAIN: f64 = 2.7;
    /// Pushes the whole world up the spline. Positive means less ocean.
    pub const CONTINENT_BIAS: f64 = 0.13;
    /// Piecewise-linear map from continent noise (-1..1) to height relative to
    /// [`BASE_HEIGHT`]. This is the single most useful dial in the file: the
    /// flat middle section is lowland, the right-hand climb is mountains.
    pub const CONTINENT_SPLINE: &[(f64, f64)] = &[
        (-1.00, -40.0), // abyss
        (-0.55, -22.0), // deep ocean
        (-0.22, -7.0),  // shelf
        (-0.05, 0.0),   // shoreline
        (0.10, 5.0),    // coastal plain
        (0.32, 10.0),   // lowland
        (0.52, 24.0),   // upland
        (0.72, 52.0),   // foothills
        (0.88, 84.0),   // mountains
        (1.00, 118.0),  // peaks
    ];

    // --- local relief -------------------------------------------------------

    /// Rolling hills riding on the continents.
    pub const HILL_SCALE: f64 = 0.0085;
    pub const HILL_OCTAVES: u32 = 4;
    pub const HILL_AMP: f64 = 14.0;
    /// Erosion field: high erosion flattens, low erosion leaves terrain rough.
    /// This is what stops every ridge in the world looking like every other.
    pub const EROSION_SCALE: f64 = 0.0032;
    pub const EROSION_OCTAVES: u32 = 3;
    /// How much full erosion flattens the hills. 1.0 would make plains dead flat.
    pub const EROSION_FLATTEN: f64 = 0.78;
    /// Fine surface detail, a couple of blocks either way.
    ///
    /// A heightmap world quantises a smooth surface to whole blocks, and on any
    /// gently sloping ground that turns into wide flat terraces following the
    /// contour lines -- the stair-stepped, contour-map look. Adding a little
    /// high-frequency noise before the rounding means the contour a step follows
    /// is ragged instead of smooth, so the eye reads rock rather than a graph.
    pub const DETAIL_SCALE: f64 = 0.026;
    pub const DETAIL_OCTAVES: u32 = 2;
    pub const DETAIL_AMP: f64 = 2.3;
    /// Detail is strongest on steep ground and fades out on the flat, so plains
    /// stay walkable and buildable instead of becoming permanently bumpy.
    pub const DETAIL_SLOPE_GAIN: f64 = 2.2;
    /// Mid-scale relief on bare mountain rock: shoulders, ledges and benches.
    ///
    /// A uniformly steep heightmap slope quantises to a perfectly regular
    /// staircase, and from a distance that regularity reads as a grid of little
    /// pyramids rather than as a mountain. Fine noise does not fix it -- it just
    /// makes the staircase fuzzy. What breaks it is relief at the scale of the
    /// landform itself, which is what puts a ledge here and a bulge there and
    /// gives the slope somewhere for the eye to rest.
    pub const LEDGE_SCALE: f64 = 0.0125;
    pub const LEDGE_OCTAVES: u32 = 2;
    pub const LEDGE_AMP: f64 = 5.5;

    /// Block-scale roughness on rock. Short wavelength, about a block of
    /// amplitude, and the single most important term for how a mountain reads.
    ///
    /// A heightmap slope of gradient one quantises to a *perfectly regular*
    /// staircase: every column steps down exactly one block from its neighbour,
    /// and the result is diagonal corduroy across the whole face. No amount of
    /// large-scale noise fixes it, because the problem is not the shape of the
    /// slope, it is that the rounding is uniform along it.
    ///
    /// Minecraft stopped having this problem in 1.18 by giving up heightmaps
    /// entirely: solidity there is a 3D density function of (x, y, z), so a
    /// cliff face is irregular and can overhang, and there is no staircase to
    /// quantise. That is the better answer and it is also a rewrite -- surface
    /// height is assumed by the lighting seed, decoration, tree placement and
    /// the meshing skirt here. Roughening the height by about a block at a
    /// wavelength of a few blocks breaks the regularity in the same place the
    /// eye sees it, and stays inside the architecture.
    pub const SCREE_SCALE: f64 = 0.29;
    pub const SCREE_AMP: f64 = 1.5;
    /// Roughness applied to all land; bare rock gets [`SCREE_AMP`] on top.
    pub const SCREE_BASE: f64 = 2.2;
    /// Size of the patches of andesite, gravel and granite on an exposed face.
    pub const ROCK_PATCH_SCALE: f64 = 0.032;

    /// Ridged noise for mountain spines.
    pub const RIDGE_SCALE: f64 = 0.0055;
    pub const RIDGE_OCTAVES: u32 = 3;
    pub const RIDGE_AMP: f64 = 46.0;
    /// Domain-warp distance for the ridge field, as a fraction of one lattice
    /// cell. See [`TerrainGen::ridged2`] -- without this the mountains wear a
    /// regular grid of pyramids.
    pub const RIDGE_WARP: f64 = 0.42;
    /// Continent height at which ridging starts and reaches full strength.
    pub const MOUNTAIN_LO: f64 = 10.0;
    pub const MOUNTAIN_HI: f64 = 46.0;

    // --- rivers -------------------------------------------------------------

    pub const RIVER_SCALE: f64 = 0.0013;
    pub const RIVER_OCTAVES: u32 = 2;
    pub const RIVER_GAIN: f64 = 2.2;
    /// Half-width of a river in noise units. Bigger is wider.
    pub const RIVER_WIDTH: f64 = 0.055;
    /// Bed height, relative to [`WATER_LEVEL`]. Must be negative or rivers dry up.
    pub const RIVER_BED_DROP: f64 = 4.0;
    /// Rivers fade out on mountains rather than sawing them in half.
    pub const RIVER_MOUNTAIN_FADE: f64 = 0.85;

    // --- guaranteed dry spawn ----------------------------------------------

    /// Within this radius of the world origin the ground is nudged above the
    /// waterline, so a new game never begins on the sea floor. On land it is a
    /// no-op: the nudge is a `max`, not an offset.
    pub const SPAWN_BIAS_RADIUS: f64 = 96.0;
    /// Minimum ground height at the very origin.
    pub const SPAWN_MIN_HEIGHT: f64 = 66.0;
    /// No tree or plant is rooted within this many blocks of the origin column,
    /// so the player never spawns inside a trunk.
    pub const SPAWN_CLEAR_RADIUS: i32 = 6;

    // --- climate ------------------------------------------------------------

    pub const CLIMATE_SCALE: f64 = 0.0016;
    pub const CLIMATE_OCTAVES: u32 = 2;
    pub const CLIMATE_GAIN: f64 = 1.9;
    /// Mid-frequency warp applied to temperature and humidity, which makes
    /// biome borders ragged instead of smooth blobs.
    pub const CLIMATE_WARP_SCALE: f64 = 0.011;
    pub const CLIMATE_WARP_AMOUNT: f32 = 0.11;
    /// Per-column dither on the biome *choice* only (never on terrain height),
    /// so a border between two grass tints stipples instead of drawing a line.
    pub const BIOME_DITHER: f32 = 0.30;
    /// Width of a climate biome's influence. Wider blends further.
    pub const CLIMATE_SIGMA: f32 = 0.17;
    /// Temperature lost per block of altitude above the waterline. This is what
    /// puts snow on peaks without a "snow biome" existing.
    pub const TEMP_LAPSE: f32 = 0.0056;
    /// Below this temperature the ground carries snow.
    pub const SNOW_TEMP: f32 = 0.20;
    /// Below this temperature open water freezes over.
    pub const ICE_TEMP: f32 = 0.16;

    // --- alpine rock --------------------------------------------------------

    /// Surface heights between these two get progressively more bare stone
    /// instead of soil. Dithered per column, so the treeline is speckled.
    pub const ROCK_LO: f32 = 88.0;
    pub const ROCK_HI: f32 = 132.0;
    /// Column counts as "mountain biome" above this rock fraction.
    pub const MOUNTAIN_BIOME_ROCK: f32 = 0.35;

    // --- soil ---------------------------------------------------------------

    pub const DIRT_DEPTH: i32 = 4;
    pub const SAND_DEPTH: i32 = 5;
    /// Ground within this many blocks of the waterline turns to beach sand.
    pub const BEACH_ABOVE: i32 = 2;
    pub const BEACH_BELOW: i32 = 4;
    /// Patchiness of sea-floor materials.
    pub const SEABED_SCALE: f64 = 0.045;

    // --- caves --------------------------------------------------------------

    /// Tunnel network. Two noise fields near zero at once carve a tube, which
    /// gives long connected worms rather than a sponge.
    pub const CAVE_SCALE: f64 = 0.0165;
    pub const CAVE_OCTAVES: u32 = 2;
    /// Tube radius in noise units. Bigger is wider tunnels.
    pub const CAVE_RADIUS: f64 = 0.145;
    /// Vertical squash of the tunnel domain: >1 makes flatter, wider galleries.
    pub const CAVE_Y_SQUASH: f64 = 1.7;
    /// Blobby caverns layered on top of the tunnels.
    pub const CHEESE_SCALE: f64 = 0.036;
    pub const CHEESE_THRESHOLD: f64 = 0.635;
    pub const CHEESE_MAX_Y: i32 = 56;
    /// Caves stop this far below the surface, so the world is not a sponge.
    pub const CAVE_SURFACE_FADE: i32 = 6;
    /// ...but on bare mountain rock they cut much closer, which is where the
    /// cheap overhangs and cliff mouths come from.
    pub const CAVE_CLIFF_FADE: i32 = 2;
    /// Under an ocean or lake floor, keep a thicker seal so water bodies do not
    /// drain into the cave system.
    pub const CAVE_WATER_FADE: i32 = 10;
    /// No carving at or below this height, so the bedrock shell stays intact.
    pub const CAVE_MIN_Y: i32 = 4;

    // --- ravines ------------------------------------------------------------

    pub const RAVINE_SCALE: f64 = 0.0042;
    pub const RAVINE_OCTAVES: u32 = 2;
    /// Half-width in BLOCKS. Measured as a real distance rather than a noise
    /// value, so a ravine is the same width wherever it runs.
    pub const RAVINE_HALF_WIDTH: f64 = 4.5;
    pub const RAVINE_MIN_Y: i32 = 10;
    /// Strength x profile must clear this for a block to be cut away.
    pub const RAVINE_CUT: f32 = 0.34;
    /// Fraction of the ravine's height over which it tapers to a point.
    pub const RAVINE_TAPER: f32 = 0.30;

    // --- bedrock ------------------------------------------------------------

    /// Highest y that can be bedrock. y=0 always is; above that it frays out.
    pub const BEDROCK_MAX: i32 = 3;

    // --- trees --------------------------------------------------------------

    /// World is tiled by cells this wide; each holds at most one tree.
    pub const TREE_CELL: i32 = 4;
    /// Widest a canopy ever reaches from its trunk. Drives how many cells a
    /// point query and a chunk have to consider.
    pub const CANOPY_MAX: i32 = 3;
    /// Tallest anything above the ground a tree occupies. Used to bound chunk
    /// streaming, so a canopy is never left in an unloaded chunk.
    pub const TREE_TOP_CLEARANCE: i32 = 14;
    /// Cheap rejection before a cell's biome is even looked up. Must be >= the
    /// largest `tree_chance` in [`CLIMATE`], or forests would thin out.
    pub const TREE_MAX_CHANCE: f32 = 0.62;
    /// Share of forest trees that come out as birch rather than oak.
    pub const BIRCH_MIX: f32 = 0.28;
    /// Trunk height ranges, inclusive.
    pub const OAK_HEIGHT: (i32, i32) = (4, 6);
    pub const BIRCH_HEIGHT: (i32, i32) = (5, 7);
    pub const SPRUCE_HEIGHT: (i32, i32) = (6, 10);
    pub const CACTUS_HEIGHT: (i32, i32) = (1, 3);
    /// Chance a canopy corner block is trimmed away, for a less boxy silhouette.
    pub const LEAF_TRIM: f32 = 0.55;

    // --- climate biome table ------------------------------------------------

    /// Which tree a biome grows.
    #[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
    pub enum TreeKind {
        #[default]
        None,
        Oak,
        Birch,
        Spruce,
        Cactus,
    }

    /// One entry in the temperature/humidity map. Weights fall off with
    /// distance from `(temp, humid)`, so every parameter below is *blended*
    /// across a border rather than switched.
    pub struct ClimateBiome {
        pub biome: super::Biome,
        pub temp: f32,
        pub humid: f32,
        /// Multiplier on hill amplitude. Swamps are flat, taiga is rugged.
        pub amp: f32,
        pub tree: TreeKind,
        /// Probability a tree cell in this biome actually grows its tree.
        pub tree_chance: f32,
        /// Probability a surface block carries tall grass (or a dead bush).
        pub plant_chance: f32,
        pub flower_chance: f32,
        /// Surface block, and what sits directly under it.
        pub top: BlockId,
        pub filler: BlockId,
    }

    /// THE biome table. Read it as a map: temperature left to right, humidity
    /// bottom to top.
    pub const CLIMATE: &[ClimateBiome] = &[
        ClimateBiome {
            biome: super::Biome::SnowyTundra,
            temp: 0.06,
            humid: 0.30,
            amp: 0.55,
            tree: TreeKind::Spruce,
            tree_chance: 0.03,
            plant_chance: 0.01,
            flower_chance: 0.0,
            top: BlockId::SNOW,
            filler: BlockId::DIRT,
        },
        ClimateBiome {
            biome: super::Biome::Taiga,
            temp: 0.24,
            humid: 0.74,
            amp: 1.00,
            tree: TreeKind::Spruce,
            tree_chance: 0.44,
            plant_chance: 0.05,
            flower_chance: 0.01,
            top: BlockId::GRASS_COLD,
            filler: BlockId::DIRT,
        },
        ClimateBiome {
            biome: super::Biome::Plains,
            temp: 0.50,
            humid: 0.28,
            amp: 0.42,
            tree: TreeKind::Oak,
            tree_chance: 0.04,
            plant_chance: 0.12,
            flower_chance: 0.055,
            top: BlockId::GRASS,
            filler: BlockId::DIRT,
        },
        ClimateBiome {
            biome: super::Biome::Forest,
            temp: 0.52,
            humid: 0.74,
            amp: 0.82,
            tree: TreeKind::Oak,
            tree_chance: 0.58,
            plant_chance: 0.08,
            flower_chance: 0.035,
            top: BlockId::GRASS,
            filler: BlockId::DIRT,
        },
        ClimateBiome {
            biome: super::Biome::Swamp,
            temp: 0.68,
            humid: 0.97,
            amp: 0.16,
            tree: TreeKind::Oak,
            tree_chance: 0.14,
            plant_chance: 0.10,
            flower_chance: 0.005,
            top: BlockId::GRASS_SWAMP,
            filler: BlockId::DIRT,
        },
        ClimateBiome {
            biome: super::Biome::Savanna,
            temp: 0.80,
            humid: 0.30,
            amp: 0.52,
            tree: TreeKind::Oak,
            tree_chance: 0.05,
            plant_chance: 0.13,
            flower_chance: 0.01,
            top: BlockId::GRASS_DRY,
            filler: BlockId::DIRT,
        },
        ClimateBiome {
            biome: super::Biome::Desert,
            temp: 0.96,
            humid: 0.04,
            amp: 0.60,
            tree: TreeKind::Cactus,
            tree_chance: 0.06,
            plant_chance: 0.025,
            flower_chance: 0.0,
            top: BlockId::SAND,
            filler: BlockId::SANDSTONE,
        },
    ];

    // --- ore and stone veins ------------------------------------------------

    /// One vein family. Space is tiled by `cell`-sized cubes; each cube hashes
    /// `per_cell` candidate blobs, and a blob whose centre falls outside
    /// `y_min..=y_max` simply does not exist. Earlier entries win where two
    /// veins overlap, so ores must come before the stone variants.
    pub struct Vein {
        pub block: BlockId,
        pub cell: i32,
        pub per_cell: u32,
        pub radius: f32,
        pub y_min: i32,
        pub y_max: i32,
    }

    /// THE ore table. `ore_counts_are_playable` measures what this produces --
    /// change a number here and read the test output rather than guessing.
    pub const VEINS: &[Vein] = &[
        Vein {
            block: BlockId::DIAMOND_ORE,
            cell: 26,
            per_cell: 3,
            radius: 1.6,
            y_min: 2,
            y_max: 15,
        },
        Vein {
            block: BlockId::GOLD_ORE,
            cell: 26,
            per_cell: 2,
            radius: 1.7,
            y_min: 2,
            y_max: 33,
        },
        Vein {
            block: BlockId::IRON_ORE,
            cell: 20,
            per_cell: 3,
            radius: 1.9,
            y_min: 3,
            y_max: 66,
        },
        Vein {
            block: BlockId::COAL_ORE,
            cell: 20,
            per_cell: 3,
            radius: 2.05,
            y_min: 5,
            y_max: 130,
        },
        Vein {
            block: BlockId::GRAVEL,
            cell: 26,
            per_cell: 2,
            radius: 3.0,
            y_min: 4,
            y_max: 110,
        },
        Vein {
            block: BlockId::DIRT,
            cell: 26,
            per_cell: 2,
            radius: 3.0,
            y_min: 4,
            y_max: 130,
        },
        Vein {
            block: BlockId::GRANITE,
            cell: 30,
            per_cell: 3,
            radius: 4.2,
            y_min: 1,
            // Reaches mountain height. Capping these at 96 left every peak in
            // the world a single uniform grey, which is half of why a slope read
            // as repeating corduroy: identical blocks make the staircase legible.
            y_max: 200,
        },
        Vein {
            block: BlockId::DIORITE,
            cell: 30,
            per_cell: 3,
            radius: 4.2,
            y_min: 1,
            y_max: 200,
        },
        Vein {
            block: BlockId::ANDESITE,
            cell: 30,
            per_cell: 3,
            radius: 4.2,
            y_min: 1,
            y_max: 200,
        },
    ];

    /// Largest factor the anisotropy below can stretch a semi-axis by. Bounds
    /// how far outside its own cell a blob is able to reach.
    pub const VEIN_MAX_STRETCH: f32 = 1.60;

    impl Vein {
        /// How many blocks outside a cell this family's blobs can reach.
        #[inline]
        pub fn reach(&self) -> i32 {
            (self.radius * VEIN_MAX_STRETCH).ceil() as i32
        }
    }

    // --- hash salts ---------------------------------------------------------
    // Distinct salts keep independent decisions from correlating with each other.

    pub const SALT_BIOME_DITHER: u32 = 0x0100;
    pub const SALT_ROCK: u32 = 0x0200;
    pub const SALT_SNOW: u32 = 0x0300;
    pub const SALT_SEABED: u32 = 0x0400;
    pub const SALT_BEDROCK: u32 = 0x0500;
    pub const SALT_TREE_ROLL: u32 = 0x0600;
    pub const SALT_TREE_POS: u32 = 0x0700;
    pub const SALT_TREE_SHAPE: u32 = 0x0800;
    pub const SALT_LEAF: u32 = 0x0900;
    pub const SALT_PLANT: u32 = 0x0A00;
    pub const SALT_FLOWER: u32 = 0x0B00;
    pub const SALT_VEIN: u32 = 0x0C00;
}

use tuning::{ClimateBiome, TreeKind};

// ---------------------------------------------------------------------------
// Hashing. Deterministic pseudo-randomness keyed on the seed and a coordinate:
// never an RNG whose state depends on the order chunks happen to be generated.
// ---------------------------------------------------------------------------

#[inline]
fn mix(mut h: u32) -> u32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    h = h.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 16;
    h
}

#[inline]
fn hash2(seed: u32, x: i32, z: i32, salt: u32) -> u32 {
    let mut h = seed ^ 0x9E37_79B9;
    h ^= (x as u32).wrapping_mul(0x85EB_CA6B);
    h = h.rotate_left(13);
    h ^= (z as u32).wrapping_mul(0x27D4_EB2F);
    h = h.rotate_left(7);
    h ^= salt.wrapping_mul(0x1656_67B1);
    mix(h)
}

#[inline]
fn hash3(seed: u32, x: i32, y: i32, z: i32, salt: u32) -> u32 {
    let mut h = seed ^ 0x9E37_79B9;
    h ^= (x as u32).wrapping_mul(0x85EB_CA6B);
    h = h.rotate_left(13);
    h ^= (y as u32).wrapping_mul(0xC2B2_AE35);
    h = h.rotate_left(11);
    h ^= (z as u32).wrapping_mul(0x27D4_EB2F);
    h = h.rotate_left(7);
    h ^= salt.wrapping_mul(0x1656_67B1);
    mix(h)
}

/// A hash word as a float in `0.0..1.0`.
#[inline]
fn unit(h: u32) -> f32 {
    (h >> 8) as f32 * (1.0 / 16_777_216.0)
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn smoothstep64(edge0: f64, edge1: f64, x: f64) -> f64 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Piecewise-linear lookup. Linear rather than smooth on purpose: smoothing the
/// joins flattens the derivative at every knot, which shows up in the world as
/// terraces at fixed heights.
fn spline(points: &[(f64, f64)], t: f64) -> f64 {
    if t <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        let (x0, y0) = w[0];
        let (x1, y1) = w[1];
        if t <= x1 {
            let f = (t - x0) / (x1 - x0);
            return y0 + (y1 - y0) * f;
        }
    }
    points[points.len() - 1].1
}

// ---------------------------------------------------------------------------
// Biomes
// ---------------------------------------------------------------------------

/// What a column reads as. The first three are derived from height and water
/// rather than from climate; the rest come from the [`tuning::CLIMATE`] table,
/// and `Mountains`/`SnowyPeaks` are an altitude overlay on top of any of them.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Biome {
    Ocean,
    Beach,
    River,
    #[default]
    Plains,
    Forest,
    Desert,
    Savanna,
    Taiga,
    SnowyTundra,
    Swamp,
    Mountains,
    SnowyPeaks,
}

impl Biome {
    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "ocean",
            Biome::Beach => "beach",
            Biome::River => "river",
            Biome::Plains => "plains",
            Biome::Forest => "forest",
            Biome::Desert => "desert",
            Biome::Savanna => "savanna",
            Biome::Taiga => "taiga",
            Biome::SnowyTundra => "snowy tundra",
            Biome::Swamp => "swamp",
            Biome::Mountains => "mountains",
            Biome::SnowyPeaks => "snowy peaks",
        }
    }
}

/// Everything about one world column that does not depend on `y`.
///
/// Hoisting this out of the inner loop is worth roughly 16x on chunk
/// generation: without it, every one of a chunk's 4,096 blocks would re-run
/// two dozen octaves of 2D noise for values that are constant down the column.
#[derive(Copy, Clone, Debug, Default)]
pub struct Column {
    /// y of the topmost solid ground block.
    pub surface: i32,
    /// Temperature at the surface, 0..1, already reduced by altitude.
    pub temp: f32,
    pub humid: f32,
    /// How alpine this column is, 0..1. Drives bare rock and cliff-side caves.
    pub rock: f32,
    pub biome: Biome,
    /// Block placed at `surface`.
    pub top: BlockId,
    /// Block placed for the next `filler_depth` blocks below `surface`.
    pub filler: BlockId,
    pub filler_depth: i32,
    /// The tree this column would grow, if a tree cell lands here.
    pub tree: TreeKind,
    pub tree_chance: f32,
    pub plant_chance: f32,
    pub flower_chance: f32,
    /// Ravine strength, 0 when this column is nowhere near one.
    pub ravine: f32,
}

/// The vertical extent of solid ground in one chunk-column, bounded cheaply.
///
/// `lo`/`hi` are conservative: everything generation places in the column is
/// guaranteed to sit inside them, water surface and tree canopies included.
/// The streamer uses this to avoid queueing sky chunks at all, so an
/// *under*-estimate punches holes in the world while an over-estimate only
/// costs an empty chunk.
#[derive(Copy, Clone, Debug)]
pub struct ColumnBounds {
    pub lo: i32,
    pub hi: i32,
}

// ---------------------------------------------------------------------------
// Trees
// ---------------------------------------------------------------------------

/// One resolved tree. Produced identically by every caller from the cell hash,
/// which is what lets a trunk and its canopy straddle a chunk boundary without
/// the two chunks talking to each other.
#[derive(Copy, Clone, Debug)]
pub struct Tree {
    pub x: i32,
    pub z: i32,
    /// y of the lowest trunk block (one above the ground).
    pub base: i32,
    pub kind: TreeKind,
    pub height: i32,
    salt: u32,
}

impl Tree {
    #[inline]
    fn log(&self) -> BlockId {
        match self.kind {
            TreeKind::Birch => BlockId::BIRCH_LOG,
            TreeKind::Spruce => BlockId::SPRUCE_LOG,
            _ => BlockId::WOOD,
        }
    }

    #[inline]
    fn leaf(&self) -> BlockId {
        match self.kind {
            TreeKind::Birch => BlockId::BIRCH_LEAVES,
            TreeKind::Spruce => BlockId::SPRUCE_LEAVES,
            _ => BlockId::LEAVES,
        }
    }

    /// Lowest and highest world y this tree can occupy.
    #[inline]
    pub fn y_range(&self) -> (i32, i32) {
        (self.base, self.base + self.height)
    }

    /// The block this tree puts at a world coordinate, if any.
    #[inline]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        let dy = y - self.base;
        if dy < 0 || dy > self.height {
            return None;
        }
        let dx = x - self.x;
        let dz = z - self.z;
        if self.kind == TreeKind::Cactus {
            return (dx == 0 && dz == 0 && dy < self.height).then_some(BlockId::CACTUS);
        }
        if dx == 0 && dz == 0 && dy < self.height {
            return Some(self.log());
        }
        let r = self.canopy_radius(dy);
        if r == 0 {
            // The very tip still gets a leaf directly over the trunk.
            return (dx == 0 && dz == 0).then(|| self.leaf());
        }
        if dx.abs() > r || dz.abs() > r {
            return None;
        }
        // Round the corners off, then trim the remaining outer ring at random
        // so no two canopies have the same silhouette.
        let d2 = dx * dx + dz * dz;
        if d2 > r * r + 1 {
            return None;
        }
        if dx.abs() == r
            && dz.abs() == r
            && unit(hash3(self.salt, dx, dy, dz, tuning::SALT_LEAF)) < tuning::LEAF_TRIM
        {
            return None;
        }
        Some(self.leaf())
    }

    /// Canopy half-width at a height above the trunk base.
    #[inline]
    fn canopy_radius(&self, dy: i32) -> i32 {
        match self.kind {
            TreeKind::Spruce => {
                // Layered cone: a wide ring every third layer going down.
                let k = self.height - dy;
                if k < 0 || dy < 2 {
                    return 0;
                }
                match k {
                    0 => 0,
                    1 => 1,
                    _ if k % 3 == 2 => 2,
                    _ => 1,
                }
            }
            TreeKind::Cactus | TreeKind::None => 0,
            // Oak and birch: two fat layers around the top of the trunk and a
            // narrow cap over them.
            _ => match self.height - dy {
                0 => 1,
                1 => 2,
                2 | 3 => 2,
                _ => 0,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Per-thread column memo
//
// The same column is asked for many times: once per chunk in a 16-high stack,
// once per tree cell in range, and 18x18 times by the meshing skirt. Computing
// a column is ~25 octaves of 2D noise, so a memo turns the repeat visits into a
// load. It is a pure cache -- keyed on (seed, x, z) with the full key compared
// on hit -- so it cannot change what is generated, only how fast.
// ---------------------------------------------------------------------------

const CACHE_BITS: u32 = 11;
const CACHE_SLOTS: usize = 1 << CACHE_BITS;

#[derive(Copy, Clone)]
struct Slot {
    seed: u32,
    x: i32,
    z: i32,
    valid: bool,
    col: Column,
}

impl Default for Slot {
    fn default() -> Self {
        Slot {
            seed: 0,
            x: 0,
            z: 0,
            valid: false,
            col: Column::default(),
        }
    }
}

thread_local! {
    static COLUMN_MEMO: RefCell<Vec<Slot>> =
        RefCell::new(vec![Slot::default(); CACHE_SLOTS]);
}

#[inline]
fn memo_index(seed: u32, x: i32, z: i32) -> usize {
    (hash2(seed, x, z, 0) >> (32 - CACHE_BITS)) as usize
}

// ---------------------------------------------------------------------------
// The generator
// ---------------------------------------------------------------------------

/// Per-octave rotation, roughly 32 degrees. Any angle that is not a multiple of
/// 45 will do; what matters is that repeated application never returns the grid
/// to where it started, which is what an axis- or diagonal-aligned angle would.
const OCT_COS: f64 = 0.848_048;
const OCT_SIN: f64 = 0.529_919;
/// Frequency step between octaves. Deliberately not 2.0 -- see [`TerrainGen::fbm2`].
const OCT_LACUNARITY: f64 = 2.037;

pub struct TerrainGen {
    continent: Perlin,
    hills: Perlin,
    erosion: Perlin,
    ridge: Perlin,
    river: Perlin,
    temp: Perlin,
    humid: Perlin,
    warp: Perlin,
    ravine: Perlin,
    detail: Perlin,
    seabed: Perlin,
    cave_a: Perlin,
    cave_b: Perlin,
    cheese: Perlin,
    pub seed: u32,
}

impl TerrainGen {
    pub fn new(seed: u32) -> Self {
        Self {
            continent: Perlin::new(seed),
            hills: Perlin::new(seed.wrapping_add(1)),
            erosion: Perlin::new(seed.wrapping_add(2)),
            ridge: Perlin::new(seed.wrapping_add(3)),
            river: Perlin::new(seed.wrapping_add(4)),
            temp: Perlin::new(seed.wrapping_add(5)),
            humid: Perlin::new(seed.wrapping_add(6)),
            warp: Perlin::new(seed.wrapping_add(7)),
            ravine: Perlin::new(seed.wrapping_add(8)),
            detail: Perlin::new(seed.wrapping_add(13)),
            seabed: Perlin::new(seed.wrapping_add(9)),
            cave_a: Perlin::new(seed.wrapping_add(10)),
            cave_b: Perlin::new(seed.wrapping_add(11)),
            cheese: Perlin::new(seed.wrapping_add(12)),
            seed,
        }
    }

    // --- noise helpers ------------------------------------------------------


    /// Fractal sum of a 2D noise source, normalised to roughly -1..1.
    ///
    /// Each octave is **rotated and offset** before it is sampled, and the
    /// frequency step is deliberately not exactly 2. This is not a flourish; it
    /// is the difference between terrain and corduroy.
    ///
    /// Perlin noise is built on an axis-aligned integer lattice, and its
    /// features line up with that lattice and its diagonals. Stack octaves at
    /// exactly double frequency from a common origin and every octave's lattice
    /// lands on top of every other one, so instead of cancelling out, those
    /// alignments reinforce into visible diagonal ribbing across whole
    /// mountainsides. Rotating each octave by an irrational-ish angle and
    /// stepping by 2.037 rather than 2.0 means no two octaves ever share a grid
    /// again, and the ribbing has nothing to build on.
    fn fbm2(noise: &Perlin, x: f64, z: f64, scale: f64, octaves: u32) -> f64 {
        let mut sum = 0.0;
        let mut amp = 1.0;
        let mut norm = 0.0;
        let (mut px, mut pz) = (x * scale, z * scale);
        for _ in 0..octaves {
            sum += noise.get([px, pz]) * amp;
            norm += amp;
            amp *= 0.5;
            let (rx, rz) = (px * OCT_COS - pz * OCT_SIN, px * OCT_SIN + pz * OCT_COS);
            px = rx * OCT_LACUNARITY + 71.31;
            pz = rz * OCT_LACUNARITY - 43.77;
        }
        sum / norm
    }

    fn fbm3(noise: &Perlin, p: [f64; 3], octaves: u32) -> f64 {
        let mut sum = 0.0;
        let mut amp = 1.0;
        let mut freq = 1.0;
        let mut norm = 0.0;
        for _ in 0..octaves {
            sum += noise.get([p[0] * freq, p[1] * freq, p[2] * freq]) * amp;
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }

    /// Ridged fractal noise, 0..1, with sharp crests where the source crosses
    /// zero. This is what makes mountain spines look like spines.
    ///
    /// The input is **domain warped** first, and that is not optional here.
    /// Perlin noise is exactly zero at every point of its integer lattice, and a
    /// ridged function peaks wherever its source is zero -- so plain ridged
    /// Perlin puts a crest on every single lattice point and produces a perfect
    /// regular grid of little pyramids across every mountainside. Rotating the
    /// octaves does not help: each octave's own lattice is still a grid.
    /// Warping the coordinates with a lower-frequency noise bends that grid into
    /// something with no repeating structure left, and as a side effect it is
    /// what makes a ridge wind like a real spine instead of running in rows.
    fn ridged2(noise: &Perlin, warp: &Perlin, x: f64, z: f64, scale: f64, octaves: u32) -> f64 {
        // Warp distance scales with the feature size, so the distortion is
        // always a meaningful fraction of a lattice cell.
        let amp = tuning::RIDGE_WARP / scale;
        let wx = Self::fbm2(warp, x, z, scale * 0.6, 2);
        let wz = Self::fbm2(warp, x + 3110.0, z - 9770.0, scale * 0.6, 2);
        let (x, z) = (x + wx * amp, z + wz * amp);
        let mut sum = 0.0;
        let mut amp = 1.0;
        let mut freq = scale;
        let mut norm = 0.0;
        // See `fbm2`: ridged noise shows lattice alignment even more plainly,
        // because a crest lands exactly where the source crosses zero and
        // aligned octaves put those crossings in rows.
        let (mut px, mut pz) = (x * freq, z * freq);
        for _ in 0..octaves {
            let v = 1.0 - (noise.get([px, pz]).abs() * 1.42).min(1.0);
            sum += v * v * amp;
            norm += amp;
            amp *= 0.5;
            let (rx, rz) = (px * OCT_COS - pz * OCT_SIN, px * OCT_SIN + pz * OCT_COS);
            px = rx * OCT_LACUNARITY - 19.44;
            pz = rz * OCT_LACUNARITY + 57.02;
        }
        sum / norm
    }

    // --- columns ------------------------------------------------------------

    /// Column data at a world position, memoised per thread.
    pub fn column(&self, x: i32, z: i32) -> Column {
        let i = memo_index(self.seed, x, z);
        let hit = COLUMN_MEMO.with(|m| {
            let m = m.borrow();
            let s = m[i];
            (s.valid && s.seed == self.seed && s.x == x && s.z == z).then_some(s.col)
        });
        if let Some(col) = hit {
            return col;
        }
        let col = self.compute_column(x, z);
        COLUMN_MEMO.with(|m| {
            m.borrow_mut()[i] = Slot {
                seed: self.seed,
                x,
                z,
                valid: true,
                col,
            };
        });
        col
    }

    /// The real work behind [`TerrainGen::column`]. Pure function of seed and
    /// position; the memo above only avoids repeating it.
    fn compute_column(&self, x: i32, z: i32) -> Column {
        use tuning as t;
        let (xf, zf) = (x as f64, z as f64);

        // --- height ---------------------------------------------------------
        let cont_raw = Self::fbm2(
            &self.continent,
            xf,
            zf,
            t::CONTINENT_SCALE,
            t::CONTINENT_OCTAVES,
        );
        let cont = (cont_raw * t::CONTINENT_GAIN + t::CONTINENT_BIAS).clamp(-1.0, 1.0);
        let continental = spline(t::CONTINENT_SPLINE, cont);

        // Erosion flattens; low erosion leaves the ground rough.
        let erosion01 =
            (Self::fbm2(&self.erosion, xf, zf, t::EROSION_SCALE, t::EROSION_OCTAVES) * 0.5 + 0.5)
                .clamp(0.0, 1.0);
        let roughness = 1.0 - t::EROSION_FLATTEN * erosion01;

        // Climate, before the altitude correction (which needs the height).
        let warp = Self::fbm2(&self.warp, xf, zf, t::CLIMATE_WARP_SCALE, 2) as f32
            * t::CLIMATE_WARP_AMOUNT;
        let temp0 = ((Self::fbm2(&self.temp, xf, zf, t::CLIMATE_SCALE, t::CLIMATE_OCTAVES)
            * t::CLIMATE_GAIN) as f32
            * 0.5
            + 0.5
            + warp)
            .clamp(0.0, 1.0);
        let humid0 = ((Self::fbm2(&self.humid, xf, zf, t::CLIMATE_SCALE, t::CLIMATE_OCTAVES)
            * t::CLIMATE_GAIN) as f32
            * 0.5
            + 0.5
            - warp)
            .clamp(0.0, 1.0);

        let weights = climate_weights(temp0, humid0);
        let mut amp_mul = 0.0f32;
        let mut tree_chance = 0.0f32;
        let mut plant_chance = 0.0f32;
        let mut flower_chance = 0.0f32;
        for (w, c) in weights.iter().zip(t::CLIMATE) {
            amp_mul += w * c.amp;
            tree_chance += w * c.tree_chance;
            plant_chance += w * c.plant_chance;
            flower_chance += w * c.flower_chance;
        }

        let hills = Self::fbm2(&self.hills, xf, zf, t::HILL_SCALE, t::HILL_OCTAVES);
        let mountainness = smoothstep64(t::MOUNTAIN_LO, t::MOUNTAIN_HI, continental);
        let ridged = Self::ridged2(
            &self.ridge,
            &self.warp,
            xf,
            zf,
            t::RIDGE_SCALE,
            t::RIDGE_OCTAVES,
        );

        let mut h = t::BASE_HEIGHT
            + continental
            + hills * t::HILL_AMP * roughness * amp_mul as f64
            + ridged * t::RIDGE_AMP * mountainness * (0.35 + 0.65 * roughness);

        // Break the block-scale staircase. See `SCREE_AMP`.
        //
        // Applied to all land, not just to mountains. Terracing is a function of
        // *slope*, not of altitude: the widest, ugliest shelves appear on the
        // gentle flanks low down, where a shallow gradient rounds into shelves
        // many blocks deep. Gating this on height left exactly those slopes bare
        // and fixed only the peaks, which were the part that looked least wrong.
        //
        // It fades out at the waterline so beaches and the sea floor stay clean;
        // a lumpy shoreline reads as broken rather than as natural ground.
        let scree = Self::fbm2(&self.detail, xf + 111.0, zf - 777.0, t::SCREE_SCALE, 1);
        let above_water = smoothstep64(0.0, 10.0, h - t::WATER_LEVEL as f64);
        h += scree * (t::SCREE_BASE + t::SCREE_AMP * mountainness) * above_water;

        // Ledges and shoulders on mountain rock, so a steep face is a series of
        // benches rather than one unbroken ramp.
        if mountainness > 0.0 {
            let ledge = Self::fbm2(&self.detail, xf + 5000.0, zf - 9000.0, t::LEDGE_SCALE, t::LEDGE_OCTAVES);
            h += ledge * t::LEDGE_AMP * mountainness;
        }

        // Break up the contour terracing. Weighted by how steep the ground
        // already is: a flat plain should stay a flat plain, but a mountainside
        // that would otherwise render as a stack of smooth shelves gets the
        // roughness that makes it read as rock.
        let steep = (hills.abs() * roughness + mountainness).min(1.0);
        let detail = Self::fbm2(&self.detail, xf, zf, t::DETAIL_SCALE, t::DETAIL_OCTAVES);
        h += detail * t::DETAIL_AMP * (0.25 + t::DETAIL_SLOPE_GAIN * steep).min(1.0);

        // Rivers: carve a valley down to the bed wherever the river field
        // crosses zero, fading out on mountains so peaks are not sawn in half.
        let rn = (Self::fbm2(&self.river, xf, zf, t::RIVER_SCALE, t::RIVER_OCTAVES)
            * t::RIVER_GAIN)
            .clamp(-1.0, 1.0);
        let river = 1.0 - smoothstep64(0.0, t::RIVER_WIDTH, rn.abs());
        let river_strength = river * (1.0 - t::RIVER_MOUNTAIN_FADE * mountainness);
        let bed = t::WATER_LEVEL as f64 - t::RIVER_BED_DROP;
        if river_strength > 0.0 && h > bed {
            h += (bed - h) * river_strength;
        }

        // A guaranteed dry landing spot at the origin.
        let d = (xf * xf + zf * zf).sqrt();
        let near_spawn = 1.0 - smoothstep64(0.0, t::SPAWN_BIAS_RADIUS, d);
        if near_spawn > 0.0 {
            h += (h.max(t::SPAWN_MIN_HEIGHT) - h) * near_spawn;
        }

        let surface = (h as i32).clamp(1, t::MAX_TERRAIN_Y);

        // --- climate, corrected for altitude --------------------------------
        let temp =
            (temp0 - (surface - t::WATER_LEVEL).max(0) as f32 * t::TEMP_LAPSE).clamp(0.0, 1.0);
        let humid = humid0;

        // --- alpine rock ----------------------------------------------------
        let rock = smoothstep(t::ROCK_LO, t::ROCK_HI, surface as f32);

        // --- ravines --------------------------------------------------------
        let rav_n = Self::fbm2(&self.ravine, xf, zf, t::RAVINE_SCALE, t::RAVINE_OCTAVES);
        let ravine = if surface > t::WATER_LEVEL + 2 {
            // Taper to nothing over the spawn clearing: a ravine straight
            // through the origin would drop the player into it on frame one.
            let keep = smoothstep64(
                t::SPAWN_CLEAR_RADIUS as f64,
                t::SPAWN_CLEAR_RADIUS as f64 * 3.0,
                d,
            );
            // Distance to the ravine's centre line, not the raw noise value.
            //
            // Thresholding |noise| alone looks like it carves a winding slot and
            // does so almost everywhere -- but wherever the field flattens out
            // near zero, the band satisfying |n| < width balloons into an
            // enormous round pit. That is where the giant craters came from.
            // Dividing by the local gradient converts the noise into an
            // approximate distance in blocks, which is constant-width by
            // construction and has no plateaus to blow up.
            let e = 2.0;
            let gx = Self::fbm2(&self.ravine, xf + e, zf, t::RAVINE_SCALE, t::RAVINE_OCTAVES)
                - Self::fbm2(&self.ravine, xf - e, zf, t::RAVINE_SCALE, t::RAVINE_OCTAVES);
            let gz = Self::fbm2(&self.ravine, xf, zf + e, t::RAVINE_SCALE, t::RAVINE_OCTAVES)
                - Self::fbm2(&self.ravine, xf, zf - e, t::RAVINE_SCALE, t::RAVINE_OCTAVES);
            let grad = (((gx * gx + gz * gz).sqrt()) / (2.0 * e)).max(1.0e-9);
            let dist_blocks = rav_n.abs() / grad;
            ((1.0 - smoothstep64(0.0, t::RAVINE_HALF_WIDTH, dist_blocks)) * keep) as f32
        } else {
            0.0
        };

        // --- surface materials ---------------------------------------------
        let mut col = Column {
            surface,
            temp,
            humid,
            rock,
            biome: Biome::Plains,
            top: BlockId::GRASS,
            filler: BlockId::DIRT,
            filler_depth: t::DIRT_DEPTH,
            tree: TreeKind::None,
            tree_chance,
            plant_chance,
            flower_chance,
            ravine,
        };
        self.dress_surface(x, z, &weights, river, &mut col);
        col
    }

    /// Choose the surface blocks, biome label and plant budget for a column.
    fn dress_surface(
        &self,
        x: i32,
        z: i32,
        weights: &[f32; CLIMATE_COUNT],
        river: f64,
        col: &mut Column,
    ) {
        use tuning as t;

        // Dithered argmax over the climate weights. The dither is applied to
        // the *choice* only, never to the height, so borders stipple without
        // the ground turning to noise.
        let mut best = 0usize;
        let mut best_w = f32::MIN;
        for (i, w) in weights.iter().enumerate() {
            let d = w
                * (1.0
                    + t::BIOME_DITHER
                        * (unit(hash2(self.seed, x, z, t::SALT_BIOME_DITHER + i as u32)) - 0.5));
            if d > best_w {
                best_w = d;
                best = i;
            }
        }
        let climate: &ClimateBiome = &t::CLIMATE[best];

        col.biome = climate.biome;
        col.top = climate.top;
        col.filler = climate.filler;
        col.filler_depth = if climate.top == BlockId::SAND {
            t::SAND_DEPTH
        } else {
            t::DIRT_DEPTH
        };
        col.tree = climate.tree;

        let s = col.surface;

        // Under water: sea and river floors get their own materials.
        if s < t::WATER_LEVEL - t::BEACH_ABOVE {
            let jitter = (unit(hash2(self.seed, x, z, t::SALT_SEABED)) - 0.5) as f64 * 0.12;
            let n = self
                .seabed
                .get([x as f64 * t::SEABED_SCALE, z as f64 * t::SEABED_SCALE])
                + jitter;
            col.biome = if river > 0.5 {
                Biome::River
            } else {
                Biome::Ocean
            };
            col.top = if s > t::WATER_LEVEL - 8 && n > 0.25 {
                BlockId::CLAY
            } else if n < -0.2 {
                BlockId::GRAVEL
            } else {
                BlockId::SAND
            };
            col.filler = if col.top == BlockId::GRAVEL {
                BlockId::GRAVEL
            } else {
                BlockId::SAND
            };
            col.filler_depth = 3;
            col.tree = TreeKind::None;
            col.plant_chance = 0.0;
            col.flower_chance = 0.0;
            col.ravine = 0.0;
            return;
        }

        // Shoreline: sand wherever land meets water.
        if s <= t::WATER_LEVEL + t::BEACH_ABOVE && s >= t::WATER_LEVEL - t::BEACH_BELOW {
            col.biome = Biome::Beach;
            col.top = BlockId::SAND;
            col.filler = BlockId::SAND;
            col.filler_depth = t::SAND_DEPTH;
            col.tree = TreeKind::None;
            col.plant_chance = 0.0;
            col.flower_chance = 0.0;
            return;
        }

        // Alpine rock, dithered so the treeline is speckled rather than drawn.
        if unit(hash2(self.seed, x, z, t::SALT_ROCK)) < col.rock {
            // Which rock. Chosen from a mid-scale noise rather than per column,
            // so the face breaks into patches of andesite and gravel a few
            // blocks across instead of salt-and-pepper. A mountainside of one
            // repeated block is what lets the eye lock on to the staircase; give
            // it patches and it reads the shape instead of the grid.
            let v = Self::fbm2(
                &self.detail,
                x as f64 - 2200.0,
                z as f64 + 1700.0,
                t::ROCK_PATCH_SCALE,
                2,
            );
            let j = unit(hash2(self.seed, x, z, t::SALT_ROCK + 7));
            // Stone stays the majority; the rest are accents. Granite is left
            // underground -- exposed, its pink reads as damage rather than rock.
            let face = match v + (j as f64 - 0.5) * 0.10 {
                n if n < -0.40 => BlockId::ANDESITE,
                n if n < -0.22 => BlockId::GRAVEL,
                n if n > 0.42 => BlockId::DIORITE,
                _ => BlockId::STONE,
            };
            col.top = face;
            col.filler = BlockId::STONE;
            col.filler_depth = 0;
            col.tree = TreeKind::None;
            col.plant_chance = 0.0;
            col.flower_chance = 0.0;
        }
        if col.rock > t::MOUNTAIN_BIOME_ROCK {
            col.biome = Biome::Mountains;
        }

        // Snow cover. Also dithered, so a snowline is a scatter, not a contour.
        let snow_edge = t::SNOW_TEMP + 0.05 * (unit(hash2(self.seed, x, z, t::SALT_SNOW)) - 0.5);
        if col.temp < snow_edge {
            if col.top == BlockId::STONE {
                col.filler = BlockId::STONE;
                col.filler_depth = 0;
            } else if col.top.is_grassy() {
                col.filler = BlockId::DIRT;
                col.filler_depth = t::DIRT_DEPTH;
            }
            col.top = BlockId::SNOW;
            col.plant_chance = 0.0;
            col.flower_chance = 0.0;
            col.biome = if col.rock > t::MOUNTAIN_BIOME_ROCK {
                Biome::SnowyPeaks
            } else if col.biome != Biome::Mountains {
                col.biome
            } else {
                Biome::SnowyPeaks
            };
        }

        // Taiga forest floor is patchy podzol.
        let podzol = 0.08 + 0.34 * col.humid;
        if col.top == BlockId::GRASS_COLD
            && unit(hash2(self.seed, x, z, t::SALT_PLANT + 1)) < podzol
        {
            col.top = BlockId::PODZOL;
        }

        // Cacti only grow out of desert sand.
        if col.tree == TreeKind::Cactus && col.top != BlockId::SAND {
            col.tree = TreeKind::None;
        }
        if !col.top.is_grassy() && col.top != BlockId::SAND && col.top != BlockId::SNOW {
            col.tree = TreeKind::None;
        }
    }

    /// Ground surface height at a world column.
    #[inline]
    pub fn height_at(&self, x: i32, z: i32) -> i32 {
        self.column(x, z).surface
    }

    /// The biome a column reads as. Stable for a given seed and position.
    #[inline]
    pub fn biome_at(&self, x: i32, z: i32) -> Biome {
        self.column(x, z).biome
    }

    /// Highest y this column can place anything at: its canopy, its ground
    /// cover, or the water surface, whichever is highest.
    #[inline]
    fn column_top(&self, col: &Column) -> i32 {
        let t = if col.tree == TreeKind::None {
            col.surface + 1
        } else {
            col.surface + tuning::TREE_TOP_CLEARANCE
        };
        t.max(tuning::WATER_LEVEL)
    }

    // --- underground --------------------------------------------------------

    #[inline]
    fn is_bedrock(&self, x: i32, y: i32, z: i32) -> bool {
        if y <= 0 {
            return true;
        }
        if y > tuning::BEDROCK_MAX {
            return false;
        }
        let keep = 1.0 - y as f32 / (tuning::BEDROCK_MAX + 1) as f32;
        unit(hash3(self.seed, x, y, z, tuning::SALT_BEDROCK)) < keep
    }

    /// Whether a cave, cavern or ravine removes the ground at this position.
    fn is_carved(&self, x: i32, y: i32, z: i32, col: &Column) -> bool {
        use tuning as t;
        if y < t::CAVE_MIN_Y {
            return false;
        }

        // Ravines: a vertical slot that reaches all the way to daylight.
        if col.ravine > 0.0 && y <= col.surface && y >= t::RAVINE_MIN_Y {
            let span = (col.surface - t::RAVINE_MIN_Y).max(1) as f32;
            let f = (y - t::RAVINE_MIN_Y) as f32 / span;
            let profile = smoothstep(0.0, t::RAVINE_TAPER, f);
            if col.ravine * profile > t::RAVINE_CUT {
                return true;
            }
        }

        // Caves stay below the soil, except on bare mountain rock where they
        // cut close enough to open as cliff mouths and overhangs.
        let fade = if col.surface <= t::WATER_LEVEL + 1 {
            t::CAVE_WATER_FADE
        } else {
            let a = t::CAVE_SURFACE_FADE as f32;
            let b = t::CAVE_CLIFF_FADE as f32;
            (a + (b - a) * col.rock).round() as i32
        };
        if col.surface - y < fade {
            return false;
        }

        let s = t::CAVE_SCALE;
        let p = [x as f64 * s, y as f64 * s * t::CAVE_Y_SQUASH, z as f64 * s];
        let a = Self::fbm3(&self.cave_a, p, t::CAVE_OCTAVES);
        if a.abs() < t::CAVE_RADIUS {
            let b = Self::fbm3(&self.cave_b, p, t::CAVE_OCTAVES);
            if a * a + b * b < t::CAVE_RADIUS * t::CAVE_RADIUS {
                return true;
            }
        }

        // Blobby caverns, deep only.
        if y < t::CHEESE_MAX_Y {
            let cs = t::CHEESE_SCALE;
            let v = self
                .cheese
                .get([x as f64 * cs, y as f64 * cs * 1.4, z as f64 * cs]);
            if v > t::CHEESE_THRESHOLD {
                return true;
            }
        }
        false
    }

    /// Which vein, if any, replaces stone at this position.
    ///
    /// Space is tiled by per-family cells; each cell hashes a fixed number of
    /// candidate blobs. Nothing is written anywhere, so two threads asking the
    /// same question always get the same answer.
    pub fn vein_at(&self, x: i32, y: i32, z: i32) -> Option<BlockId> {
        for (vi, v) in tuning::VEINS.iter().enumerate() {
            let reach = v.reach();
            let (cx0, cx1) = (
                (x - reach).div_euclid(v.cell),
                (x + reach).div_euclid(v.cell),
            );
            let (cy0, cy1) = (
                (y - reach).div_euclid(v.cell),
                (y + reach).div_euclid(v.cell),
            );
            let (cz0, cz1) = (
                (z - reach).div_euclid(v.cell),
                (z + reach).div_euclid(v.cell),
            );
            for cy in cy0..=cy1 {
                // Whole cells outside the family's depth band cannot hold one.
                if (cy + 1) * v.cell <= v.y_min || cy * v.cell > v.y_max {
                    continue;
                }
                for cz in cz0..=cz1 {
                    for cx in cx0..=cx1 {
                        for k in 0..v.per_cell {
                            if let Some(b) = vein_blob(self.seed, v, vi as u32, cx, cy, cz, k)
                                && b.contains(x, y, z)
                            {
                                return Some(v.block);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// Ground material at a position at or below the surface, *before* ore and
    /// stone-variant veins are applied.
    ///
    /// Everything this answers `STONE` for is exactly the set of blocks a vein
    /// may replace, which is what lets [`TerrainGen::stamp_veins`] run as a
    /// volume pass and still agree with the per-block query.
    #[inline]
    fn ground_base(&self, x: i32, y: i32, z: i32, col: &Column) -> BlockId {
        if self.is_bedrock(x, y, z) {
            return BlockId::BEDROCK;
        }
        if self.is_carved(x, y, z, col) {
            return BlockId::AIR;
        }
        let depth = col.surface - y;
        if depth == 0 {
            return col.top;
        }
        if depth <= col.filler_depth {
            return col.filler;
        }
        BlockId::STONE
    }

    /// Ground material at a position known to be at or below the surface.
    #[inline]
    fn ground_block(&self, x: i32, y: i32, z: i32, col: &Column) -> BlockId {
        let base = self.ground_base(x, y, z, col);
        if base != BlockId::STONE {
            return base;
        }
        self.vein_at(x, y, z).unwrap_or(BlockId::STONE)
    }

    /// Paint every vein blob that overlaps a chunk into its block array.
    ///
    /// Asking `vein_at` per block costs around a hundred hashes for each of the
    /// 4,096 blocks of a buried chunk. Walking the handful of blobs that
    /// actually overlap it costs a few hundred ellipsoid tests instead.
    /// Priority falls out of the table order for free: a family only writes
    /// into blocks that are still `STONE`, so the first family to claim a block
    /// keeps it and every later family skips over it -- exactly what the
    /// first-match-wins point query does.
    fn stamp_veins(
        &self,
        ox: i32,
        oy: i32,
        oz: i32,
        cols: &[Column; CHUNK_SIZE * CHUNK_SIZE],
        blocks: &mut [BlockId; CHUNK_VOL],
    ) {
        let cs = CHUNK_SIZE_I;
        for (vi, v) in tuning::VEINS.iter().enumerate() {
            let reach = v.reach();
            let cell = v.cell;
            let (cx0, cx1) = (
                (ox - reach).div_euclid(cell),
                (ox + cs - 1 + reach).div_euclid(cell),
            );
            let (cy0, cy1) = (
                (oy - reach).div_euclid(cell),
                (oy + cs - 1 + reach).div_euclid(cell),
            );
            let (cz0, cz1) = (
                (oz - reach).div_euclid(cell),
                (oz + cs - 1 + reach).div_euclid(cell),
            );
            for cy in cy0..=cy1 {
                if (cy + 1) * cell <= v.y_min || cy * cell > v.y_max {
                    continue;
                }
                for cz in cz0..=cz1 {
                    for cx in cx0..=cx1 {
                        for k in 0..v.per_cell {
                            let Some(blob) = vein_blob(self.seed, v, vi as u32, cx, cy, cz, k)
                            else {
                                continue;
                            };
                            let (lo, hi) = blob.bounds();
                            let x0 = lo.0.max(ox);
                            let x1 = hi.0.min(ox + cs - 1);
                            let y0 = lo.1.max(oy);
                            let y1 = hi.1.min(oy + cs - 1);
                            let z0 = lo.2.max(oz);
                            let z1 = hi.2.min(oz + cs - 1);
                            for wy in y0..=y1 {
                                for wz in z0..=z1 {
                                    for wx in x0..=x1 {
                                        let (lx, ly, lz) = (wx - ox, wy - oy, wz - oz);
                                        let col = &cols[(lz * cs + lx) as usize];
                                        // Only the region `ground_base` calls
                                        // STONE is eligible, so soil and bare
                                        // mountain rock stay where they are.
                                        if col.surface - wy <= col.filler_depth {
                                            continue;
                                        }
                                        let i = local_index(lx as usize, ly as usize, lz as usize)
                                            as usize;
                                        if blocks[i] != BlockId::STONE {
                                            continue;
                                        }
                                        if blob.contains(wx, wy, wz) {
                                            blocks[i] = v.block;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[inline]
    fn water_block(&self, y: i32, col: &Column) -> BlockId {
        if y == tuning::WATER_LEVEL && col.temp < tuning::ICE_TEMP {
            BlockId::ICE
        } else {
            BlockId::WATER
        }
    }

    // --- trees and ground cover ---------------------------------------------

    /// True inside the small keep-clear disc around the world origin.
    #[inline]
    fn in_spawn_clearing(x: i32, z: i32) -> bool {
        let r = tuning::SPAWN_CLEAR_RADIUS;
        x * x + z * z <= r * r
    }

    /// The single tree a cell holds, if it holds one.
    ///
    /// One hash decides "maybe" before any column is looked at, which is what
    /// keeps the point query cheap: most cells are rejected in a few
    /// nanoseconds without touching noise at all.
    pub fn tree_in_cell(&self, cx: i32, cz: i32) -> Option<Tree> {
        use tuning as t;
        let roll = unit(hash2(self.seed, cx, cz, t::SALT_TREE_ROLL));
        if roll >= t::TREE_MAX_CHANCE {
            return None;
        }
        let hp = hash2(self.seed, cx, cz, t::SALT_TREE_POS);
        let x = cx * t::TREE_CELL + (hp % t::TREE_CELL as u32) as i32;
        let z = cz * t::TREE_CELL + ((hp >> 8) % t::TREE_CELL as u32) as i32;
        if Self::in_spawn_clearing(x, z) {
            return None;
        }
        let col = self.column(x, z);
        if col.tree == TreeKind::None || roll >= col.tree_chance {
            return None;
        }
        let hs = hash2(self.seed, cx, cz, t::SALT_TREE_SHAPE);
        let mut kind = col.tree;
        if kind == TreeKind::Oak && unit(hs) < t::BIRCH_MIX {
            kind = TreeKind::Birch;
        }
        let (lo, hi) = match kind {
            TreeKind::Birch => t::BIRCH_HEIGHT,
            TreeKind::Spruce => t::SPRUCE_HEIGHT,
            TreeKind::Cactus => t::CACTUS_HEIGHT,
            _ => t::OAK_HEIGHT,
        };
        let height = lo + ((hs >> 12) % (hi - lo + 1) as u32) as i32;
        Some(Tree {
            x,
            z,
            base: col.surface + 1,
            kind,
            height,
            salt: mix(hs ^ self.seed),
        })
    }

    /// The tree block at a position, considering every cell whose canopy could
    /// reach it. `surface` is the *local* column's ground: a tree never
    /// replaces terrain, it only fills air above it.
    fn tree_at(&self, x: i32, y: i32, z: i32, surface: i32) -> Option<BlockId> {
        use tuning as t;
        // Trunks start above the waterline, so nothing below it can be a tree.
        if y <= surface || y <= t::WATER_LEVEL + 1 {
            return None;
        }
        let (cx0, cx1) = (
            (x - t::CANOPY_MAX).div_euclid(t::TREE_CELL),
            (x + t::CANOPY_MAX).div_euclid(t::TREE_CELL),
        );
        let (cz0, cz1) = (
            (z - t::CANOPY_MAX).div_euclid(t::TREE_CELL),
            (z + t::CANOPY_MAX).div_euclid(t::TREE_CELL),
        );
        // z outer, x inner: `generate` collects trees in the same order, so
        // "first match wins" resolves overlaps identically in both paths.
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                if let Some(tree) = self.tree_in_cell(cx, cz)
                    && let Some(id) = tree.block_at(x, y, z)
                {
                    return Some(id);
                }
            }
        }
        None
    }

    /// Ground cover sitting directly on the surface block.
    fn decoration_at(&self, x: i32, z: i32, col: &Column) -> Option<BlockId> {
        use tuning as t;
        if col.surface < t::WATER_LEVEL || Self::in_spawn_clearing(x, z) {
            return None;
        }
        // Nothing grows over a hole. Decoration is chosen from the column's
        // surface height, but a cave mouth or a ravine can take that surface
        // block away underneath it, which leaves a flower hanging in the air
        // over the opening. The column knows its own height; only the carver
        // knows whether the block is still there, so it has to be asked.
        if self.is_carved(x, col.surface, z, col) {
            return None;
        }
        if col.top == BlockId::SAND {
            return (unit(hash2(self.seed, x, z, t::SALT_PLANT)) < col.plant_chance)
                .then_some(BlockId::DEAD_BUSH);
        }
        if !col.top.is_grassy() {
            return None;
        }
        let f = unit(hash2(self.seed, x, z, t::SALT_FLOWER));
        if f < col.flower_chance {
            return Some(if f * 2.0 < col.flower_chance {
                BlockId::FLOWER_RED
            } else {
                BlockId::FLOWER_YELLOW
            });
        }
        (unit(hash2(self.seed, x, z, t::SALT_PLANT)) < col.plant_chance)
            .then_some(BlockId::TALL_GRASS)
    }

    // --- the three views ----------------------------------------------------

    /// The block generation places at a world coordinate.
    ///
    /// `surface` is accepted for source compatibility with the meshing skirt,
    /// which already knows the column height. It must equal `height_at(x, z)`;
    /// the column memo makes looking it up again essentially free.
    #[inline]
    pub fn block_at_surface(&self, x: i32, y: i32, z: i32, surface: i32) -> BlockId {
        if y <= 0 {
            return BlockId::BEDROCK;
        }
        if y >= WORLD_HEIGHT {
            return BlockId::AIR;
        }
        let col = self.column(x, z);
        debug_assert_eq!(
            surface, col.surface,
            "block_at_surface was handed a stale surface height"
        );
        if y > col.surface {
            if let Some(id) = self.tree_at(x, y, z, col.surface) {
                return id;
            }
            if y <= tuning::WATER_LEVEL {
                return self.water_block(y, &col);
            }
            if y == col.surface + 1
                && let Some(id) = self.decoration_at(x, z, &col)
            {
                return id;
            }
            return BlockId::AIR;
        }
        self.ground_block(x, y, z, &col)
    }

    /// The block that generation places at a world coordinate.
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> BlockId {
        if y <= 0 {
            return BlockId::BEDROCK;
        }
        if y >= WORLD_HEIGHT {
            return BlockId::AIR;
        }
        self.block_at_surface(x, y, z, self.column(x, z).surface)
    }

    /// The 16x16 surface heights of one chunk-column, in `local_index` x/z order
    /// (`z * CHUNK_SIZE + x`).
    pub fn column_heights(&self, cx: i32, cz: i32) -> [i32; CHUNK_SIZE * CHUNK_SIZE] {
        let ox = cx * CHUNK_SIZE_I;
        let oz = cz * CHUNK_SIZE_I;
        let mut out = [0i32; CHUNK_SIZE * CHUNK_SIZE];
        for z in 0..CHUNK_SIZE {
            for x in 0..CHUNK_SIZE {
                out[z * CHUNK_SIZE + x] = self.height_at(ox + x as i32, oz + z as i32);
            }
        }
        out
    }

    /// Conservative vertical bounds of everything in a chunk-column, from a
    /// coarse probe rather than all 256 columns.
    ///
    /// The probe reaches [`tuning::CANOPY_MAX`] blocks outside the chunk,
    /// because a tree rooted just over the edge still drops leaves inside it,
    /// and `hi` counts the canopy and the water surface as well as the ground.
    pub fn column_bounds(&self, cx: i32, cz: i32) -> ColumnBounds {
        let ox = cx * CHUNK_SIZE_I;
        let oz = cz * CHUNK_SIZE_I;
        let m = tuning::CANOPY_MAX;
        let mut lo = i32::MAX;
        let mut hi = i32::MIN;
        let mut z = -m;
        while z <= CHUNK_SIZE_I + m {
            let mut x = -m;
            while x <= CHUNK_SIZE_I + m {
                let col = self.column(ox + x.min(CHUNK_SIZE_I + m), oz + z.min(CHUNK_SIZE_I + m));
                lo = lo.min(col.surface);
                hi = hi.max(self.column_top(&col));
                x += SURFACE_PROBE_STRIDE;
            }
            z += SURFACE_PROBE_STRIDE;
        }
        ColumnBounds {
            lo: (lo - SURFACE_PROBE_MARGIN).max(0),
            hi: (hi + SURFACE_PROBE_MARGIN).min(WORLD_HEIGHT - 1),
        }
    }

    /// Every tree whose canopy can reach into a chunk, in the same order the
    /// point query walks its cells.
    fn trees_near_chunk(&self, ox: i32, oz: i32) -> Vec<Tree> {
        use tuning as t;
        let (cx0, cx1) = (
            (ox - t::CANOPY_MAX).div_euclid(t::TREE_CELL),
            (ox + CHUNK_SIZE_I - 1 + t::CANOPY_MAX).div_euclid(t::TREE_CELL),
        );
        let (cz0, cz1) = (
            (oz - t::CANOPY_MAX).div_euclid(t::TREE_CELL),
            (oz + CHUNK_SIZE_I - 1 + t::CANOPY_MAX).div_euclid(t::TREE_CELL),
        );
        let mut out = Vec::new();
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                if let Some(tree) = self.tree_in_cell(cx, cz) {
                    out.push(tree);
                }
            }
        }
        out
    }

    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let (ox, oy, oz) = pos.origin();
        let cs = CHUNK_SIZE_I;

        // Cheap conservative reject. Anything above the column's bound is sky,
        // and the caller can see that from `Chunk::is_empty` without touching a
        // single block.
        if oy > self.column_bounds(pos.x, pos.z).hi {
            let mut c = Chunk::new(pos);
            c.generated = true;
            return c;
        }

        let mut cols = [Column::default(); CHUNK_SIZE * CHUNK_SIZE];
        let mut min_surface = i32::MAX;
        for lz in 0..cs {
            for lx in 0..cs {
                let c = self.column(ox + lx, oz + lz);
                min_surface = min_surface.min(c.surface);
                cols[(lz * cs + lx) as usize] = c;
            }
        }

        let mut blocks = Box::new([BlockId::AIR; CHUNK_VOL]);

        // --- ground, water and caves ---------------------------------------
        for lz in 0..cs {
            for lx in 0..cs {
                let col = &cols[(lz * cs + lx) as usize];
                let wx = ox + lx;
                let wz = oz + lz;
                for ly in 0..cs {
                    let wy = oy + ly;
                    let id = if wy <= col.surface {
                        self.ground_base(wx, wy, wz, col)
                    } else if wy <= tuning::WATER_LEVEL {
                        self.water_block(wy, col)
                    } else {
                        continue; // already air; trees and plants come later
                    };
                    if !id.is_air() {
                        blocks[local_index(lx as usize, ly as usize, lz as usize) as usize] = id;
                    }
                }
            }
        }

        self.stamp_veins(ox, oy, oz, &cols, &mut blocks);

        // Nothing above ground reaches this chunk: skip trees and ground cover
        // entirely. This is the fast path for the deep underground.
        if oy + cs - 1 >= min_surface {
            // --- trees ------------------------------------------------------
            for tree in self.trees_near_chunk(ox, oz) {
                let (ty0, ty1) = tree.y_range();
                let y0 = ty0.max(oy);
                let y1 = ty1.min(oy + cs - 1);
                if y0 > y1 {
                    continue;
                }
                let r = tuning::CANOPY_MAX;
                let x0 = (tree.x - r).max(ox);
                let x1 = (tree.x + r).min(ox + cs - 1);
                let z0 = (tree.z - r).max(oz);
                let z1 = (tree.z + r).min(oz + cs - 1);
                for wy in y0..=y1 {
                    for wz in z0..=z1 {
                        for wx in x0..=x1 {
                            let (lx, lz) = (wx - ox, wz - oz);
                            // Terrain always wins: a tree only fills air above
                            // the local ground, never carves into it.
                            if wy <= cols[(lz * cs + lx) as usize].surface {
                                continue;
                            }
                            let i =
                                local_index(lx as usize, (wy - oy) as usize, lz as usize) as usize;
                            if !blocks[i].is_air() {
                                continue; // an earlier tree already claimed it
                            }
                            if let Some(id) = tree.block_at(wx, wy, wz) {
                                blocks[i] = id;
                            }
                        }
                    }
                }
            }

            // --- ground cover -----------------------------------------------
            for lz in 0..cs {
                for lx in 0..cs {
                    let col = &cols[(lz * cs + lx) as usize];
                    let wy = col.surface + 1;
                    if wy < oy || wy >= oy + cs {
                        continue;
                    }
                    let i = local_index(lx as usize, (wy - oy) as usize, lz as usize) as usize;
                    if !blocks[i].is_air() {
                        continue;
                    }
                    if let Some(id) = self.decoration_at(ox + lx, oz + lz, col) {
                        blocks[i] = id;
                    }
                }
            }
        }

        Chunk::from_blocks(pos, blocks)
    }
}

// ---------------------------------------------------------------------------
// Climate weights
// ---------------------------------------------------------------------------

/// Number of entries in [`tuning::CLIMATE`]. A const so the weights live on the
/// stack rather than in a heap allocation per column.
pub const CLIMATE_COUNT: usize = 7;

/// Smooth membership of a (temperature, humidity) point in every climate biome,
/// normalised to sum to 1. This is the mechanism that blends amplitude, tree
/// density and plant cover across a border instead of switching on it.
pub fn climate_weights(temp: f32, humid: f32) -> [f32; CLIMATE_COUNT] {
    debug_assert_eq!(tuning::CLIMATE.len(), CLIMATE_COUNT);
    let inv = 1.0 / (2.0 * tuning::CLIMATE_SIGMA * tuning::CLIMATE_SIGMA);
    let mut w = [0.0f32; CLIMATE_COUNT];
    let mut sum = 0.0;
    for (i, c) in tuning::CLIMATE.iter().enumerate() {
        let dt = temp - c.temp;
        let dh = humid - c.humid;
        let v = (-(dt * dt + dh * dh) * inv).exp();
        w[i] = v;
        sum += v;
    }
    let inv_sum = if sum > 0.0 { 1.0 / sum } else { 0.0 };
    for v in &mut w {
        *v *= inv_sum;
    }
    w
}

// ---------------------------------------------------------------------------
// Vein blobs
// ---------------------------------------------------------------------------

/// One resolved ore blob: an axis-aligned ellipsoid.
#[derive(Copy, Clone, Debug)]
struct Blob {
    cx: f32,
    cy: f32,
    cz: f32,
    /// Reciprocal semi-axes, pre-divided so `contains` is three mul-adds.
    ix: f32,
    iy: f32,
    iz: f32,
}

impl Blob {
    /// Inclusive integer bounding box of the ellipsoid.
    #[inline]
    fn bounds(&self) -> ((i32, i32, i32), (i32, i32, i32)) {
        let (rx, ry, rz) = (1.0 / self.ix, 1.0 / self.iy, 1.0 / self.iz);
        (
            (
                (self.cx - rx).ceil() as i32,
                (self.cy - ry).ceil() as i32,
                (self.cz - rz).ceil() as i32,
            ),
            (
                (self.cx + rx).floor() as i32,
                (self.cy + ry).floor() as i32,
                (self.cz + rz).floor() as i32,
            ),
        )
    }

    #[inline]
    fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        let dx = (x as f32 - self.cx) * self.ix;
        let dy = (y as f32 - self.cy) * self.iy;
        let dz = (z as f32 - self.cz) * self.iz;
        dx * dx + dy * dy + dz * dz <= 1.0
    }
}

/// The `k`th candidate blob of vein family `vi` in a cell, or `None` when that
/// candidate falls outside the family's depth band.
#[inline]
fn vein_blob(
    seed: u32,
    v: &tuning::Vein,
    vi: u32,
    cx: i32,
    cy: i32,
    cz: i32,
    k: u32,
) -> Option<Blob> {
    let salt = tuning::SALT_VEIN + vi * 16 + k;
    let h = hash3(seed, cx, cy, cz, salt);
    let cell = v.cell as f32;
    let px = cx as f32 * cell + (h & 0xFF) as f32 * (cell / 256.0);
    let py = cy as f32 * cell + ((h >> 8) & 0xFF) as f32 * (cell / 256.0);
    let pz = cz as f32 * cell + ((h >> 16) & 0xFF) as f32 * (cell / 256.0);
    if py < v.y_min as f32 || py > v.y_max as f32 {
        return None;
    }
    // Anisotropy, flattened in y, so veins read as seams rather than marbles.
    let g = mix(h);
    let sx = v.radius * (0.75 + 0.85 * unit(g));
    let sy = v.radius * (0.60 + 0.60 * unit(g.rotate_left(11)));
    let sz = v.radius * (0.75 + 0.85 * unit(g.rotate_left(21)));
    Some(Blob {
        cx: px,
        cy: py,
        cz: pz,
        ix: 1.0 / sx,
        iy: 1.0 / sy,
        iz: 1.0 / sz,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Instant;
    use tuning::WATER_LEVEL;

    fn chunks_around(r: i32) -> Vec<ChunkPos> {
        let mut out = Vec::new();
        for cz in -r..=r {
            for cx in -r..=r {
                for cy in 0..8 {
                    out.push(ChunkPos::new(cx, cy, cz));
                }
            }
        }
        out
    }

    // --- determinism --------------------------------------------------------

    /// Deterministic worldgen is the contract every other system leans on.
    #[test]
    fn generation_is_byte_identical_for_the_same_seed_and_position() {
        let a = TerrainGen::new(1337);
        let b = TerrainGen::new(1337);
        for pos in [
            ChunkPos::new(0, 4, 0),
            ChunkPos::new(-7, 3, 12),
            ChunkPos::new(51, 0, -33),
            ChunkPos::new(2, 15, 2),
            ChunkPos::new(-140, 4, 88),
        ] {
            let ca = a.generate(pos);
            let cb = b.generate(pos);
            assert_eq!(
                ca.blocks().as_slice(),
                cb.blocks().as_slice(),
                "chunk {pos:?} differs between two generators with the same seed"
            );
            let cc = a.generate(pos);
            assert_eq!(ca.blocks().as_slice(), cc.blocks().as_slice());
        }
    }

    /// Chunks are generated on rayon workers, so two threads racing on the same
    /// position must not be able to disagree -- in particular the per-thread
    /// column memo must not leak between them.
    #[test]
    fn generation_is_identical_across_threads() {
        let g = std::sync::Arc::new(TerrainGen::new(24680));
        let pos = ChunkPos::new(3, 4, -5);
        let reference = g.generate(pos);
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let g = g.clone();
                std::thread::spawn(move || {
                    // Warm the thread's memo with unrelated columns first, so a
                    // stale entry would show up if the key were wrong.
                    for i in 0..500 {
                        let _ = g.height_at(i * 37, i * 91);
                    }
                    g.generate(pos).blocks().to_vec()
                })
            })
            .collect();
        for h in handles {
            assert_eq!(h.join().unwrap().as_slice(), reference.blocks().as_slice());
        }
    }

    #[test]
    fn a_different_seed_produces_a_different_world() {
        let a = TerrainGen::new(1337);
        let b = TerrainGen::new(9001);
        let pos = ChunkPos::new(3, 4, 3);
        assert_ne!(
            a.generate(pos).blocks().as_slice(),
            b.generate(pos).blocks().as_slice()
        );
    }

    /// The single most important invariant in the file: whole-chunk generation
    /// and the one-coordinate query must produce the same world, or the meshing
    /// skirt tears every seam where a neighbour has not streamed in yet.
    #[test]
    fn generation_matches_the_point_query() {
        let g = TerrainGen::new(1337);
        for pos in [
            ChunkPos::new(0, 3, 0),
            ChunkPos::new(0, 4, 0),
            ChunkPos::new(-3, 4, 5),
            ChunkPos::new(11, 1, -7),
            ChunkPos::new(-40, 5, 40),
            ChunkPos::new(7, 0, 7),
        ] {
            let c = g.generate(pos);
            let (ox, oy, oz) = pos.origin();
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        let (wx, wy, wz) = (ox + x as i32, oy + y as i32, oz + z as i32);
                        assert_eq!(
                            c.get(x, y, z),
                            g.block_at(wx, wy, wz),
                            "generate and block_at disagree at {wx},{wy},{wz}"
                        );
                    }
                }
            }
        }
    }

    /// `mesh.rs` calls `block_at_surface` with a height it looked up itself.
    #[test]
    fn hoisted_surface_matches_the_general_path() {
        let g = TerrainGen::new(4242);
        for x in [-33, -1, 0, 7, 250] {
            for z in [-9, 0, 3, 128] {
                let s = g.height_at(x, z);
                for y in [0, 1, 12, 60, s - 1, s, s + 1, s + 6, 200, 255] {
                    assert_eq!(
                        g.block_at(x, y, z),
                        g.block_at_surface(x, y, z, s),
                        "mismatch at {x},{y},{z}"
                    );
                }
            }
        }
    }

    // --- water --------------------------------------------------------------

    #[test]
    fn water_never_generates_above_sea_level() {
        for seed in [1337u32, 7, 99_991] {
            let g = TerrainGen::new(seed);
            for pos in chunks_around(2) {
                let c = g.generate(pos);
                if c.is_empty() {
                    continue;
                }
                let (_, oy, _) = pos.origin();
                for y in 0..CHUNK_SIZE {
                    for z in 0..CHUNK_SIZE {
                        for x in 0..CHUNK_SIZE {
                            let b = c.get(x, y, z);
                            if b == BlockId::WATER || b == BlockId::ICE {
                                assert!(
                                    oy + y as i32 <= WATER_LEVEL,
                                    "{b:?} at y={} is above sea level",
                                    oy + y as i32
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    /// A sea needs a floor and a shore, not just a hole full of blue.
    #[test]
    fn oceans_have_water_and_beaches_somewhere() {
        let g = TerrainGen::new(1337);
        let mut water = 0usize;
        let mut sand = 0usize;
        for x in (-900..900).step_by(7) {
            for z in (-900..900).step_by(7) {
                let col = g.column(x, z);
                if col.surface < WATER_LEVEL {
                    water += 1;
                }
                if col.biome == Biome::Beach {
                    sand += 1;
                }
            }
        }
        assert!(water > 200, "world has almost no water ({water} columns)");
        assert!(sand > 50, "world has almost no beaches ({sand} columns)");
    }

    /// Every water column must be capped: no open shaft of air between the sea
    /// floor and the surface.
    #[test]
    fn water_columns_are_filled_to_the_surface() {
        let g = TerrainGen::new(555);
        let mut checked = 0;
        for x in (-400..400).step_by(13) {
            for z in (-400..400).step_by(13) {
                let s = g.height_at(x, z);
                if s >= WATER_LEVEL {
                    continue;
                }
                checked += 1;
                for y in (s + 1)..=WATER_LEVEL {
                    let b = g.block_at(x, y, z);
                    assert!(
                        b == BlockId::WATER || b == BlockId::ICE,
                        "gap at {x},{y},{z}: {b:?}"
                    );
                }
            }
        }
        assert!(checked > 50, "no ocean columns in the sample");
    }

    // --- biomes -------------------------------------------------------------

    #[test]
    fn biome_assignment_is_stable() {
        let a = TerrainGen::new(2024);
        let b = TerrainGen::new(2024);
        for x in (-2000..2000).step_by(97) {
            for z in (-2000..2000).step_by(89) {
                assert_eq!(a.biome_at(x, z), b.biome_at(x, z));
                // And repeat calls on the same generator, through the memo.
                assert_eq!(a.biome_at(x, z), a.biome_at(x, z));
            }
        }
    }

    #[test]
    fn climate_weights_blend_rather_than_switch() {
        // Weights are a partition of unity everywhere...
        for t in 0..=10 {
            for h in 0..=10 {
                let w = climate_weights(t as f32 / 10.0, h as f32 / 10.0);
                let sum: f32 = w.iter().sum();
                assert!((sum - 1.0).abs() < 1e-4, "weights sum to {sum}");
            }
        }
        // ...and they move smoothly, so no parameter can jump at a border.
        let mut prev = climate_weights(0.0, 0.5);
        for i in 1..=200 {
            let w = climate_weights(i as f32 / 200.0, 0.5);
            let delta: f32 = w.iter().zip(prev).map(|(a, b)| (a - b).abs()).sum();
            assert!(delta < 0.06, "weights jump by {delta} at step {i}");
            prev = w;
        }
    }

    /// The world must actually contain the biomes the table describes.
    #[test]
    fn the_world_contains_a_spread_of_biomes() {
        let g = TerrainGen::new(1337);
        let mut seen: HashMap<&'static str, usize> = HashMap::new();
        for x in (-2400..2400).step_by(31) {
            for z in (-2400..2400).step_by(31) {
                *seen.entry(g.biome_at(x, z).name()).or_default() += 1;
            }
        }
        for want in [
            "ocean",
            "beach",
            "plains",
            "forest",
            "desert",
            "taiga",
            "swamp",
            "mountains",
        ] {
            assert!(
                seen.get(want).copied().unwrap_or(0) > 5,
                "biome {want} is missing or vanishingly rare: {seen:?}"
            );
        }
    }

    #[test]
    fn every_climate_chance_is_under_the_prefilter() {
        for c in tuning::CLIMATE {
            assert!(
                c.tree_chance <= tuning::TREE_MAX_CHANCE,
                "{:?} would be thinned by the tree prefilter",
                c.biome
            );
        }
    }

    // --- trees --------------------------------------------------------------

    /// The classic voxel worldgen bug: a trunk in one chunk whose leaves fall in
    /// the next, generated separately and never agreeing. Generate a block of
    /// adjacent chunks independently, stitch them, and check every tree is whole.
    #[test]
    fn trees_are_continuous_across_chunk_boundaries() {
        let g = TerrainGen::new(1337);

        // Stitch a 3x3 column of chunks centred on a chunk origin.
        let stitch = |cx0: i32, cz0: i32| {
            let mut world: HashMap<(i32, i32, i32), BlockId> = HashMap::new();
            for cx in cx0 - 1..=cx0 + 1 {
                for cz in cz0 - 1..=cz0 + 1 {
                    for cy in 3..7 {
                        let pos = ChunkPos::new(cx, cy, cz);
                        let c = g.generate(pos);
                        let (ox, oy, oz) = pos.origin();
                        for y in 0..CHUNK_SIZE {
                            for z in 0..CHUNK_SIZE {
                                for x in 0..CHUNK_SIZE {
                                    let b = c.get(x, y, z);
                                    if !b.is_air() {
                                        world.insert(
                                            (ox + x as i32, oy + y as i32, oz + z as i32),
                                            b,
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
            world
        };

        // Hunt for somewhere forested rather than assuming the origin is.
        //
        // This test is about a property -- a canopy must not be cut in half by a
        // chunk boundary -- and that property has nothing to do with where the
        // world happens to put a forest. Pinning it to the origin made it a
        // tripwire for any change to the terrain shape, which is exactly the
        // kind of false failure that gets a real test deleted.
        let mut world = HashMap::new();
        let mut base = (0i32, 0i32);
        for &(cx0, cz0) in &[(0, 0), (6, 0), (0, 6), (-6, 4), (12, -8), (-14, -14), (20, 20)] {
            let candidate = stitch(cx0, cz0);
            let logs = candidate.values().filter(|b| b.is_log()).count();
            if logs > 20 {
                world = candidate;
                base = (cx0 * CHUNK_SIZE as i32, cz0 * CHUNK_SIZE as i32);
                break;
            }
        }
        assert!(!world.is_empty(), "no forested patch found anywhere to test");

        // Only judge trunks well inside the stitched region, so "missing" never
        // means "outside the generated box".
        let inside = |x: i32, y: i32, z: i32| {
            (base.0 - 12..base.0 + 12).contains(&x)
                && (base.1 - 12..base.1 + 12).contains(&z)
                && (52..108).contains(&y)
        };

        let mut trunks = 0;
        let mut crossings = 0;
        for (&(x, y, z), &b) in &world {
            if !b.is_log() || !inside(x, y, z) {
                continue;
            }
            // The block under a trunk is either more trunk or the ground it
            // grew out of -- never air.
            let below = world.get(&(x, y - 1, z)).copied().unwrap_or(BlockId::AIR);
            assert!(!below.is_air(), "trunk at {x},{y},{z} is floating over air");
            trunks += 1;

            // A trunk's canopy must exist. Search the block above the trunk top.
            if world
                .get(&(x, y + 1, z))
                .copied()
                .unwrap_or(BlockId::AIR)
                .is_log()
            {
                continue; // not the top of this trunk
            }
            let mut leaves = 0;
            for dy in -2..=2i32 {
                for dz in -3..=3i32 {
                    for dx in -3..=3i32 {
                        if world
                            .get(&(x + dx, y + dy, z + dz))
                            .copied()
                            .unwrap_or(BlockId::AIR)
                            .is_leaves()
                        {
                            leaves += 1;
                        }
                    }
                }
            }
            assert!(
                leaves >= 4,
                "trunk topping out at {x},{y},{z} has only {leaves} leaves around it"
            );
            // Count the ones whose canopy genuinely spans a chunk edge.
            if (x.rem_euclid(16) <= 2)
                || (x.rem_euclid(16) >= 13)
                || (z.rem_euclid(16) <= 2)
                || (z.rem_euclid(16) >= 13)
            {
                crossings += 1;
            }
        }
        assert!(
            trunks > 10,
            "no trees in the sample ({trunks} trunk blocks)"
        );
        assert!(
            crossings > 0,
            "no tree in the sample straddles a chunk boundary, so this test proved nothing"
        );
    }

    /// A tree read one block at a time must match the tree a chunk stamps.
    #[test]
    fn tree_blocks_agree_between_the_two_paths() {
        let g = TerrainGen::new(31415);
        let mut found = 0;
        for pos in chunks_around(2) {
            let c = g.generate(pos);
            if c.is_empty() {
                continue;
            }
            let (ox, oy, oz) = pos.origin();
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        let b = c.get(x, y, z);
                        if b.is_log() || b.is_leaves() {
                            found += 1;
                            assert_eq!(
                                b,
                                g.block_at(ox + x as i32, oy + y as i32, oz + z as i32),
                                "tree block disagrees at {},{},{}",
                                ox + x as i32,
                                oy + y as i32,
                                oz + z as i32
                            );
                        }
                    }
                }
            }
        }
        assert!(found > 0, "no trees anywhere in the sampled chunks");
    }

    #[test]
    fn trees_never_stand_in_water() {
        let g = TerrainGen::new(4321);
        for pos in chunks_around(2) {
            let c = g.generate(pos);
            if c.is_empty() {
                continue;
            }
            let (_, oy, _) = pos.origin();
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        if c.get(x, y, z).is_log() {
                            assert!(
                                oy + y as i32 > WATER_LEVEL,
                                "trunk at y={} is below the waterline",
                                oy + y as i32
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn deserts_grow_cacti_and_forests_grow_trees() {
        let g = TerrainGen::new(1337);
        let mut cactus = 0;
        let mut oak = 0;
        let mut birch = 0;
        let mut spruce = 0;
        for cx in -60..60 {
            for cz in -60..60 {
                if let Some(t) = g.tree_in_cell(cx * 3, cz * 3) {
                    match t.kind {
                        TreeKind::Cactus => cactus += 1,
                        TreeKind::Oak => oak += 1,
                        TreeKind::Birch => birch += 1,
                        TreeKind::Spruce => spruce += 1,
                        TreeKind::None => {}
                    }
                }
            }
        }
        assert!(oak > 0 && birch > 0, "no oak/birch ({oak}/{birch})");
        assert!(spruce > 0, "no spruce anywhere");
        assert!(cactus > 0, "no cactus anywhere");
    }

    // --- surface and caves --------------------------------------------------

    /// Grass belongs on top of the ground, never buried in it.
    #[test]
    fn surface_blocks_only_sit_on_the_surface() {
        let g = TerrainGen::new(1337);
        for cx in -2..3 {
            for cz in -2..3 {
                let heights = g.column_heights(cx, cz);
                for cy in 0..8 {
                    let c = g.generate(ChunkPos::new(cx, cy, cz));
                    if c.is_empty() {
                        continue;
                    }
                    for y in 0..CHUNK_SIZE {
                        for z in 0..CHUNK_SIZE {
                            for x in 0..CHUNK_SIZE {
                                let b = c.get(x, y, z);
                                if b.is_grassy() {
                                    let wy = cy * CHUNK_SIZE_I + y as i32;
                                    assert_eq!(
                                        wy,
                                        heights[z * CHUNK_SIZE + x],
                                        "{b:?} at world y={wy} is not on the surface"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// A steep slope must not be a perfectly regular staircase.
    ///
    /// This is the artifact that made mountains look like corduroy: a smooth
    /// heightmap on ground of gradient one rounds to exactly one block of drop
    /// per block of travel, every time, so the whole face is a repeating
    /// diagonal ripple of identical blocks. It is invisible in any test that
    /// looks at heights one at a time and obvious the moment you look at the
    /// sequence of differences.
    #[test]
    fn steep_ground_is_not_a_perfectly_regular_staircase() {
        let g = TerrainGen::new(4242);
        let mut worst: Option<(i32, i32, f32, usize)> = None;
        // Sample long transects and keep the most regular one found.
        for k in 0..64 {
            let z = -2000 + k * 61;
            let x0 = -2000 + k * 37;
            let hs: Vec<i32> = (0..96).map(|i| g.height_at(x0 + i, z)).collect();
            let steps: Vec<i32> = hs.windows(2).map(|w| w[1] - w[0]).collect();
            // Only judge genuinely steep runs; flat ground is allowed to be flat.
            let drop = (hs[hs.len() - 1] - hs[0]).abs();
            if drop < 48 {
                continue;
            }
            let distinct = steps.iter().collect::<std::collections::HashSet<_>>().len();
            let modal = steps
                .iter()
                .map(|s| steps.iter().filter(|o| *o == s).count())
                .max()
                .unwrap_or(0);
            let uniformity = modal as f32 / steps.len() as f32;
            if worst.map(|w| uniformity > w.2).unwrap_or(true) {
                worst = Some((x0, z, uniformity, distinct));
            }
        }
        let Some((x, z, uniformity, distinct)) = worst else {
            panic!("no steep transect found to judge");
        };
        println!("steepest transect at {x},{z}: {uniformity:.2} uniform, {distinct} distinct steps");
        assert!(
            uniformity < 0.75 && distinct >= 3,
            "slope at {x},{z} is a regular staircase: {:.0}% of steps identical,              only {distinct} distinct step sizes",
            uniformity * 100.0
        );
    }

    #[test]
    fn plants_stand_directly_on_ground_they_can_root_in() {
        let g = TerrainGen::new(8888);
        for pos in chunks_around(2) {
            let c = g.generate(pos);
            if c.is_empty() {
                continue;
            }
            let (ox, oy, oz) = pos.origin();
            for y in 0..CHUNK_SIZE {
                for z in 0..CHUNK_SIZE {
                    for x in 0..CHUNK_SIZE {
                        let b = c.get(x, y, z);
                        if !matches!(
                            b,
                            BlockId::TALL_GRASS
                                | BlockId::FLOWER_RED
                                | BlockId::FLOWER_YELLOW
                                | BlockId::DEAD_BUSH
                        ) {
                            continue;
                        }
                        let (wx, wy, wz) = (ox + x as i32, oy + y as i32, oz + z as i32);
                        let under = g.block_at(wx, wy - 1, wz);
                        assert!(
                            under.is_grassy() || under == BlockId::SAND,
                            "{b:?} at {wx},{wy},{wz} is rooted in {under:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn caves_exist_and_do_not_eat_the_whole_world() {
        let g = TerrainGen::new(1337);
        let mut air = 0usize;
        let mut solid = 0usize;
        for x in (-160..160).step_by(3) {
            for z in (-160..160).step_by(3) {
                for y in (10..50).step_by(2) {
                    if g.block_at(x, y, z).is_air() {
                        air += 1;
                    } else {
                        solid += 1;
                    }
                }
            }
        }
        let frac = air as f64 / (air + solid) as f64;
        assert!(
            (0.03..0.45).contains(&frac),
            "underground air fraction is {frac:.3}, which is either solid rock or swiss cheese"
        );
    }

    #[test]
    fn bedrock_floors_the_world_and_sky_is_air() {
        let g = TerrainGen::new(1337);
        assert_eq!(g.block_at(5, 0, 5), BlockId::BEDROCK);
        assert_eq!(g.block_at(5, -1, 5), BlockId::BEDROCK);
        assert_eq!(g.block_at(5, WORLD_HEIGHT, 5), BlockId::AIR);
        assert_eq!(g.block_at(5, WORLD_HEIGHT - 1, 5), BlockId::AIR);
    }

    /// The player must not spawn underwater or inside a trunk: `main.rs` puts
    /// them at `surface_y(0, 0) + 1`.
    #[test]
    fn the_spawn_column_is_dry_land_and_clear() {
        for seed in [1337u32, 1, 2, 3, 77, 4242, 99_991, 31415] {
            let g = TerrainGen::new(seed);
            let s = g.height_at(0, 0);
            assert!(
                s > WATER_LEVEL,
                "seed {seed} spawns on the sea floor (y={s})"
            );
            assert!(!g.block_at(0, s, 0).is_air(), "seed {seed} has no ground");
            assert_eq!(
                g.block_at(0, s + 1, 0),
                BlockId::AIR,
                "seed {seed} spawns the player inside something"
            );
            assert_eq!(g.block_at(0, s + 2, 0), BlockId::AIR);
        }
    }

    // --- bounds and streaming ----------------------------------------------

    /// If `column_bounds` under-reports, the streamer never queues the chunk and
    /// the world grows a hole. Check it against what generation actually makes.
    #[test]
    fn column_bounds_contain_everything_generation_places() {
        let g = TerrainGen::new(1337);
        for cx in -2..3 {
            for cz in -2..3 {
                let b = g.column_bounds(cx, cz);
                for cy in 0..16 {
                    let c = g.generate(ChunkPos::new(cx, cy, cz));
                    if c.is_empty() {
                        continue;
                    }
                    for y in 0..CHUNK_SIZE {
                        for z in 0..CHUNK_SIZE {
                            for x in 0..CHUNK_SIZE {
                                if c.get(x, y, z).is_air() {
                                    continue;
                                }
                                let wy = cy * CHUNK_SIZE_I + y as i32;
                                assert!(
                                    wy <= b.hi,
                                    "block at y={wy} escapes bounds {b:?} at column {cx},{cz}"
                                );
                            }
                        }
                    }
                }
                for h in g.column_heights(cx, cz) {
                    assert!(h >= b.lo && h <= b.hi, "height {h} escapes {b:?}");
                }
            }
        }
    }

    /// Chunks the bound says are sky must really be empty, or the early-out is
    /// silently deleting terrain.
    #[test]
    fn the_empty_chunk_early_out_never_discards_anything() {
        let g = TerrainGen::new(606);
        for cx in -2..3 {
            for cz in -2..3 {
                let b = g.column_bounds(cx, cz);
                let first_sky = b.hi.div_euclid(CHUNK_SIZE_I) + 1;
                for cy in first_sky..16 {
                    let (ox, oy, oz) = ChunkPos::new(cx, cy, cz).origin();
                    for z in (0..CHUNK_SIZE_I).step_by(4) {
                        for x in (0..CHUNK_SIZE_I).step_by(4) {
                            for y in (0..CHUNK_SIZE_I).step_by(4) {
                                assert!(
                                    g.block_at(ox + x, oy + y, oz + z).is_air(),
                                    "chunk {cx},{cy},{cz} was skipped but is not empty"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // --- ores ---------------------------------------------------------------

    /// Counted, not eyeballed. Prints the measured rates so the tuning table
    /// above can be dialled from real numbers.
    #[test]
    fn ore_counts_are_playable() {
        let g = TerrainGen::new(1337);
        let mut counts: HashMap<u8, usize> = HashMap::new();
        let mut stone_like = 0usize;
        // A 64x64 footprint of the top 128 blocks: about half a million blocks.
        for x in 0..64 {
            for z in 0..64 {
                let col = g.column(x, z);
                for y in 1..=col.surface.min(127) {
                    let b = g.block_at(x, y, z);
                    if b.is_air() || b == BlockId::BEDROCK {
                        continue;
                    }
                    stone_like += 1;
                    *counts.entry(b.0).or_default() += 1;
                }
            }
        }
        let pct = |id: BlockId| {
            100.0 * counts.get(&id.0).copied().unwrap_or(0) as f64 / stone_like as f64
        };
        println!(
            "ore sample: {stone_like} solid blocks -- coal {:.3}%  iron {:.3}%  gold {:.3}%  \
             diamond {:.3}%  granite {:.2}%  diorite {:.2}%  andesite {:.2}%  gravel {:.2}%",
            pct(BlockId::COAL_ORE),
            pct(BlockId::IRON_ORE),
            pct(BlockId::GOLD_ORE),
            pct(BlockId::DIAMOND_ORE),
            pct(BlockId::GRANITE),
            pct(BlockId::DIORITE),
            pct(BlockId::ANDESITE),
            pct(BlockId::GRAVEL),
        );
        // Rates a player can feel: coal common, iron findable, gold and diamond
        // worth the trip down. Numbers are percentages of solid blocks.
        assert!(
            (0.4..3.0).contains(&pct(BlockId::COAL_ORE)),
            "coal at {:.3}%",
            pct(BlockId::COAL_ORE)
        );
        assert!(
            (0.15..1.6).contains(&pct(BlockId::IRON_ORE)),
            "iron at {:.3}%",
            pct(BlockId::IRON_ORE)
        );
        assert!(
            (0.01..0.4).contains(&pct(BlockId::GOLD_ORE)),
            "gold at {:.3}%",
            pct(BlockId::GOLD_ORE)
        );
        assert!(
            (0.003..0.2).contains(&pct(BlockId::DIAMOND_ORE)),
            "diamond at {:.3}%",
            pct(BlockId::DIAMOND_ORE)
        );
        for v in [BlockId::GRANITE, BlockId::DIORITE, BlockId::ANDESITE] {
            assert!(pct(v) > 0.5, "{v:?} at {:.2}% is invisible", pct(v));
        }
    }

    /// Depth weighting: diamond must not be lying about on the surface.
    #[test]
    fn ores_respect_their_depth_bands() {
        let g = TerrainGen::new(1337);
        for x in 0..48 {
            for z in 0..48 {
                let col = g.column(x, z);
                for y in 1..=col.surface.min(200) {
                    match g.block_at(x, y, z) {
                        BlockId::DIAMOND_ORE => assert!(y <= 20, "diamond at y={y}"),
                        BlockId::GOLD_ORE => assert!(y <= 40, "gold at y={y}"),
                        BlockId::IRON_ORE => assert!(y <= 74, "iron at y={y}"),
                        _ => {}
                    }
                }
            }
        }
    }

    // --- performance --------------------------------------------------------

    /// The streamer fills a 12-chunk radius at startup. If generation gets slow
    /// the frame budget collapses, so hold the line on it. Measured
    /// single-threaded here; the real streamer spreads this over rayon.
    #[test]
    fn generation_stays_within_the_streaming_budget() {
        let g = TerrainGen::new(1337);
        // Warm up: the first call pays for the memo allocation.
        let _ = g.generate(ChunkPos::new(0, 4, 0));

        let mut positions = Vec::new();
        for cz in -6..6 {
            for cx in -6..6 {
                let b = g.column_bounds(cx, cz);
                let top = b.hi.div_euclid(CHUNK_SIZE_I).min(15);
                let lo = (b.lo.div_euclid(CHUNK_SIZE_I) - 1).max(0);
                for cy in lo..=top {
                    positions.push(ChunkPos::new(cx, cy, cz));
                }
            }
        }
        let t0 = Instant::now();
        let mut solid = 0usize;
        for p in &positions {
            let c = g.generate(*p);
            if !c.is_empty() {
                solid += 1;
            }
        }
        let dt = t0.elapsed();
        let per = dt.as_secs_f64() / positions.len() as f64;
        println!(
            "generated {} chunks ({solid} non-empty) in {:.3}s -- {:.0} us/chunk",
            positions.len(),
            dt.as_secs_f64(),
            per * 1e6
        );
        // The shipping streamer runs this across the rayon pool with at least
        // four workers, so a 1.5 s wall-clock target means roughly 6 s of CPU
        // for this many chunks. Leave headroom; this is a regression guard, not
        // a benchmark.
        assert!(
            per < 3.0e-3,
            "generation costs {:.0} us/chunk, which will not stream in time",
            per * 1e6
        );
    }
}

#[cfg(test)]
mod ravine_shape {
    use super::*;

    /// A ravine must be a narrow slot, not a crater.
    ///
    /// The original test was `|noise| < width`, which carves a winding slot
    /// almost everywhere -- but wherever the noise field flattens out near
    /// zero, the qualifying band balloons into an enormous round pit. Measuring
    /// the area they occupy catches exactly that: a slot covers a sliver of the
    /// map, a crater field covers a great deal of it.
    #[test]
    fn ravines_are_slots_rather_than_craters() {
        let g = TerrainGen::new(4242);
        let (mut active, mut total) = (0usize, 0usize);
        let mut worst_run = 0usize;

        let mut z = -600;
        while z < 600 {
            let mut run = 0usize;
            for x in -600..600 {
                let col = g.column(x, z);
                total += 1;
                if col.ravine > tuning::RAVINE_CUT {
                    active += 1;
                    run += 1;
                    worst_run = worst_run.max(run);
                } else {
                    run = 0;
                }
            }
            z += 7;
        }

        let fraction = active as f64 / total as f64;
        println!("ravine coverage {:.3}%, widest crossing {worst_run} blocks", fraction * 100.0);
        assert!(
            fraction < 0.05,
            "ravines cover {:.1}% of the world, which is a crater field, not slots",
            fraction * 100.0
        );
        // A slot crossed at a glancing angle is legitimately wide, but nothing
        // should be a hundred blocks across.
        assert!(
            worst_run < 90,
            "widest ravine crossing is {worst_run} blocks, which is a pit"
        );
    }
}
