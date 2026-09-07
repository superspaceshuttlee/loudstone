//! Every number the terrain is made of, and nothing else.
//!
//! Kept apart from the generator on purpose. Tuning terrain is a long sequence
//! of small numeric changes, and having them in one file means a change to how
//! the world looks is never mixed into a diff with a change to how it is built.
//! Each constant carries what it does and what going wrong looks like, because
//! the failure modes here are visual and hard to reason back from.

use crate::content::block::BlockId;

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
/// Piecewise-linear map from continent noise (-1..1) to height relative to
/// [`BASE_HEIGHT`].
///
/// The shape of this curve is what decides how much of the world is a place
/// you would walk around in and how much is a wall. The climb used to begin
/// at 0.52, which put everything past the middle of the noise range into
/// foothills or higher -- so mountains were not landmarks, they were the
/// default, and the world read as one continuous overwhelming massif with
/// occasional flat bits. The climb now starts later and rises harder, so the
/// same peaks exist and are rarer, with real lowland between them.
pub const CONTINENT_SPLINE: &[(f64, f64)] = &[
    (-1.00, -42.0), // abyss
    (-0.55, -24.0), // deep ocean
    (-0.22, -8.0),  // shelf
    (-0.05, 0.0),   // shoreline
    (0.20, 4.0),    // coastal plain
    (0.56, 10.0),   // lowland -- the widest band, deliberately
    (0.76, 22.0),   // upland
    (0.88, 46.0),   // foothills
    (0.96, 74.0),   // mountains
    (1.00, 100.0),  // peaks
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
pub const SCREE_SCALE: f64 = 0.115;
pub const SCREE_AMP: f64 = 1.0;
/// Roughness applied to all land; bare rock gets [`SCREE_AMP`] on top.
pub const SCREE_BASE: f64 = 1.1;
/// Size of the patches of andesite, gravel and granite on an exposed face.
pub const ROCK_PATCH_SCALE: f64 = 0.032;

/// Ridged noise for mountain spines.
pub const RIDGE_SCALE: f64 = 0.0055;
pub const RIDGE_OCTAVES: u32 = 3;
pub const RIDGE_AMP: f64 = 30.0;
/// Domain-warp distance for the ridge field, as a fraction of one lattice
/// cell. See [`TerrainGen::ridged2`] -- without this the mountains wear a
/// regular grid of pyramids.
pub const RIDGE_WARP: f64 = 0.42;
/// Continent height at which ridging starts and reaches full strength.
/// Continent height at which ridging starts and reaches full strength.
///
/// Raised so that ridged spines belong to genuinely high ground. Starting at
/// 10 meant almost half of all land got some ridging on top of whatever the
/// spline already gave it, which is a large part of how the height field
/// came to overshoot the world ceiling.
pub const MOUNTAIN_LO: f64 = 26.0;
pub const MOUNTAIN_HI: f64 = 64.0;

// --- rivers -------------------------------------------------------------

pub const RIVER_SCALE: f64 = 0.0013;
/// Three octaves, plus the warp below. With two octaves at this scale the
/// field's wavelength is about 770 blocks, so across any view you can
/// actually see, its zero contour -- which is where the river runs -- is
/// barely a third of one wave and comes out as a straight line ruled across
/// the landscape. Rivers meander because their course is set by fine detail
/// as well as by the broad slope.
pub const RIVER_OCTAVES: u32 = 3;
/// How far the river's course is dragged sideways by the warp field, in
/// blocks. This is what turns a smooth contour into a winding one.
pub const RIVER_WARP: f64 = 220.0;
/// A river stops carving this far above the waterline.
///
/// Rivers run downhill to the sea; they do not cross summits. The previous
/// rule faded them by mountain-ness to 15% strength, and 15% of a carve
/// down to the riverbed is still an eighteen-block slot sawn over the top of
/// a peak -- which is exactly what it did, while the comment above it
/// claimed peaks were safe.
pub const RIVER_MAX_RISE: f64 = 30.0;
pub const RIVER_GAIN: f64 = 2.2;
/// Half-width of a river in noise units. Bigger is wider.
pub const RIVER_WIDTH: f64 = 0.055;
/// Bed height, relative to [`WATER_LEVEL`]. Must be negative or rivers dry up.
pub const RIVER_BED_DROP: f64 = 4.0;

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
/// Temperature lost per block of altitude above the waterline.
///
/// This one number decides how much of the world is snow, and at 0.0056 the
/// answer was "most of it". An average column starts around temperature 0.5
/// and turns to snow below [`SNOW_TEMP`], so the snowline sat only
/// `(0.5 - 0.2) / 0.0056` = 54 blocks above the sea -- around y=117, which
/// is *below* the foothills the continent spline produces. Every landmass
/// past its own coastal fringe came out white, and a world where the default
/// ground colour is snow reads as one endless overwhelming mountain no
/// matter how good the terrain under it is.
///
/// At 0.0022 the same column keeps its grass to about 135 blocks above the
/// sea, so snow is a peak rather than the default ground colour. Cold biomes
/// still ice over far lower, and hot ones never freeze at all.
pub const TEMP_LAPSE: f32 = 0.0026;
/// Below this temperature the ground carries snow.
///
/// This is the other half of how much of the world is white: it decides how
/// much land is snowy at sea level regardless of height. Together with
/// [`TEMP_LAPSE`] it is the whole snow budget, and the two were tuned as if
/// independent -- which is how the world ended up three-quarters snow with
/// neither number looking obviously wrong on its own.
pub const SNOW_TEMP: f32 = 0.16;
/// Below this temperature open water freezes over.
pub const ICE_TEMP: f32 = 0.125;

// --- alpine rock --------------------------------------------------------

/// Surface heights between these two get progressively more bare stone
/// instead of soil. Dithered per column, so the treeline is speckled.
/// Where soil gives way to bare alpine rock. Raised alongside
/// [`TEMP_LAPSE`]: with the treeline at y=88 and the snowline just above it,
/// the two together left no band of high *green* ground anywhere.
pub const ROCK_LO: f32 = 124.0;
pub const ROCK_HI: f32 = 172.0;
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
/// Sideways drag on a ravine's course, in blocks. See [`RIVER_WARP`].
pub const RAVINE_WARP: f64 = 90.0;
/// Half-width in BLOCKS. Measured as a real distance rather than a noise
/// value, so a ravine is the same width wherever it runs.
pub const RAVINE_HALF_WIDTH: f64 = 4.5;
pub const RAVINE_MIN_Y: i32 = 10;
/// Strength x profile must clear this for a block to be cut away.
pub const RAVINE_CUT: f32 = 0.34;
/// Fraction of the ravine's height over which it tapers to a point.
pub const RAVINE_TAPER: f32 = 0.30;
/// How far above the waterline a ravine may still open at the surface.
///
/// A ravine used to run from [`RAVINE_MIN_Y`] up to whatever the surface
/// happened to be, so under a two-hundred-block mountain it became a
/// two-hundred-block slot that split the peak from summit to base. Real
/// ravines are a feature of low ground. Above this, the ravine stops before
/// it reaches daylight; well above it, there is no ravine at all.
pub const RAVINE_MAX_RISE: i32 = 34;
/// Fraction of a ravine's height over which it closes up again at the top,
/// so it narrows into the ground instead of ending in a full-width gash.
pub const RAVINE_ROOF_TAPER: f32 = 0.22;
/// How hard 3D noise pushes the walls in and out, as a fraction of the cut
/// threshold.
///
/// Without this a ravine's walls are *perfectly flat vertical planes*: the
/// carve reads a single per-column number, so every column is either cut for
/// its whole height or not cut at all, and the result looks sawn rather than
/// eroded.
pub const RAVINE_WALL: f32 = 0.30;
pub const RAVINE_WALL_SCALE: f64 = 0.055;
/// Walls vary more slowly up the slot than across it, which is what makes
/// them read as walls rather than as a cloud of blobs.
pub const RAVINE_WALL_SQUASH: f64 = 0.45;

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
