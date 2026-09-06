//! The content registry: blocks, items, tool tiers and recipes, loaded from
//! RON data rather than spelled out in Rust.
//!
//! # Why this exists
//!
//! Adding a block used to mean editing five separate `match` statements and
//! recompiling. Now it means adding one entry to `assets/data/blocks.ron`. The
//! Rust side keeps the *API* -- `BlockId::STONE`, `id.hardness()`,
//! `crafting::resolve()` -- and this module is the source of truth underneath.
//!
//! # Where the data comes from
//!
//! The three RON files are `include_str!`d, so the binary always runs
//! standalone with no assets folder next to it. If a file of the same name
//! exists on disk it is loaded *instead* of the built-in copy, searched in this
//! order:
//!
//! 1. `<directory of the executable>/assets/data/<file>.ron` -- what a shipped
//!    game or a mod drops in.
//! 2. `./assets/data/<file>.ron` -- the repository itself, so `cargo run` from
//!    the project root picks up edits with no build step at all.
//!
//! Each file is resolved separately: overriding `blocks.ron` leaves the
//! built-in `recipes.ron` alone.
//!
//! # Speed
//!
//! `is_opaque` is asked once per block face by the mesher -- millions of times
//! a second -- so nothing on that path may hash, lock or allocate. Loading
//! flattens every hot property into `[BlockHot; 256]` indexed by the raw block
//! number, which the compiler proves in-bounds for a `u8` and so compiles to a
//! single indexed load. Reaching the table is one relaxed atomic load of an
//! already-initialised `OnceLock`. Names, drops and recipes live in cold
//! side-tables and may hash all they like.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::block::{BlockId, RenderKind};
use crate::crafting::{Pattern, Recipe, SmeltRecipe};
use crate::item::{HarvestRule, ItemId, ToolKind, ToolTier};

// ---------------------------------------------------------------------------
// The built-in data
// ---------------------------------------------------------------------------

const EMBEDDED_BLOCKS: &str = include_str!("../assets/data/blocks.ron");
const EMBEDDED_ITEMS: &str = include_str!("../assets/data/items.ron");
const EMBEDDED_RECIPES: &str = include_str!("../assets/data/recipes.ron");

const BLOCKS_FILE: &str = "blocks.ron";
const ITEMS_FILE: &str = "items.ron";
const RECIPES_FILE: &str = "recipes.ron";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A content file that could not be loaded, and why.
///
/// Every message names the file and the entry at fault, because the person
/// reading it is a designer with a typo, not a programmer with a debugger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError {
    /// The file the problem is in, as the path it was actually read from.
    pub file: String,
    /// What is wrong, in a sentence.
    pub detail: String,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.file, self.detail)
    }
}

impl std::error::Error for RegistryError {}

impl RegistryError {
    fn new(file: &str, detail: impl Into<String>) -> Self {
        Self {
            file: file.to_string(),
            detail: detail.into(),
        }
    }
}

/// Where one content file's text came from: a path on disk, or the copy built
/// into the binary. The label is what error messages quote.
struct Source {
    label: String,
    text: Cow<'static, str>,
}

impl Source {
    fn err(&self, detail: impl Into<String>) -> RegistryError {
        RegistryError::new(&self.label, detail)
    }

    fn parse<T: for<'de> Deserialize<'de>>(&self) -> Result<T, RegistryError> {
        ron::from_str::<T>(&self.text).map_err(|e| self.err(e.to_string()))
    }
}

/// Find the on-disk override for one content file, if there is one.
fn override_path(file: &str) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        roots.push(dir.to_path_buf());
    }
    roots.push(PathBuf::from("."));
    roots
        .into_iter()
        .map(|r| r.join("assets").join("data").join(file))
        .find(|p| p.is_file())
}

fn source_for(file: &str, embedded: &'static str) -> Result<Source, RegistryError> {
    match override_path(file) {
        Some(path) => {
            let label = path.display().to_string();
            let text = std::fs::read_to_string(&path)
                .map_err(|e| RegistryError::new(&label, format!("could not be read: {e}")))?;
            Ok(Source {
                label,
                text: Cow::Owned(text),
            })
        }
        None => Ok(Source {
            label: format!("<built-in {file}>"),
            text: Cow::Borrowed(embedded),
        }),
    }
}

// ---------------------------------------------------------------------------
// What a parsed file looks like
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlocksFile {
    blocks: Vec<BlockEntry>,
}

/// The tool kind a block responds to, with a fourth "no tool helps" state so
/// the common case can be left out of the data entirely.
#[derive(Deserialize, Copy, Clone, PartialEq, Eq, Debug, Default)]
enum ToolSlot {
    #[default]
    None,
    Pickaxe,
    Axe,
    Sword,
}

impl ToolSlot {
    fn kind(self) -> Option<ToolKind> {
        match self {
            ToolSlot::None => None,
            ToolSlot::Pickaxe => Some(ToolKind::Pickaxe),
            ToolSlot::Axe => Some(ToolKind::Axe),
            ToolSlot::Sword => Some(ToolKind::Sword),
        }
    }
}

/// What a block demands before it will drop anything.
#[derive(Deserialize, Clone, PartialEq, Eq, Debug, Default)]
enum Requires {
    /// Bare hands are enough.
    #[default]
    Hands,
    /// The block's `tool` kind, at this tier or better.
    Tier(String),
    /// Nothing ever harvests it: air, water, bedrock.
    Never,
}

/// What a block yields when mined.
#[derive(Deserialize, Clone, PartialEq, Eq, Debug, Default)]
enum Drops {
    /// The item that places this same block, if there is one.
    #[default]
    Itself,
    /// Mining it gives nothing.
    Nothing,
    /// A named item.
    Item(String),
}

const fn yes() -> bool {
    true
}

const fn one() -> u8 {
    1
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlockEntry {
    id: u8,
    name: String,
    color: Vec<f32>,
    hardness: f32,
    #[serde(default = "yes")]
    opaque: bool,
    #[serde(default = "yes")]
    solid: bool,
    #[serde(default)]
    replaceable: bool,
    #[serde(default)]
    render: RenderKind,
    #[serde(default)]
    light: u8,
    #[serde(default)]
    light_opacity: Option<u8>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    tool: ToolSlot,
    #[serde(default)]
    requires: Requires,
    #[serde(default)]
    drops: Drops,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemsFile {
    tiers: Vec<TierEntry>,
    items: Vec<ItemEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TierEntry {
    name: String,
    rank: u8,
    speed: f32,
    durability: u16,
    material: String,
    color: Vec<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemEntry {
    id: u16,
    name: String,
    display: String,
    #[serde(default)]
    places: Option<String>,
    #[serde(default)]
    tool: Option<(ToolKind, String)>,
    #[serde(default)]
    max_stack: Option<u8>,
    #[serde(default)]
    durability: Option<u16>,
    #[serde(default = "one_f32")]
    attack: f32,
    #[serde(default)]
    fuel: Option<f32>,
    #[serde(default)]
    color: Option<Vec<f32>>,
}

fn one_f32() -> f32 {
    1.0
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipesFile {
    #[serde(default)]
    shapeless: Vec<ShapelessEntry>,
    #[serde(default)]
    shaped: Vec<ShapedEntry>,
    #[serde(default)]
    smelting: Vec<SmeltEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapelessEntry {
    output: String,
    #[serde(default = "one")]
    count: u8,
    ingredients: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapedEntry {
    output: String,
    #[serde(default = "one")]
    count: u8,
    pattern: Vec<String>,
    key: HashMap<char, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmeltEntry {
    input: String,
    output: String,
    seconds: f32,
}

// ---------------------------------------------------------------------------
// The resolved tables
// ---------------------------------------------------------------------------

/// Bit positions in [`BlockHot::flags`].
pub const F_OPAQUE: u8 = 1 << 0;
pub const F_SOLID: u8 = 1 << 1;
pub const F_REPLACEABLE: u8 = 1 << 2;
pub const F_GRASSY: u8 = 1 << 3;
pub const F_LOG: u8 = 1 << 4;
pub const F_LEAVES: u8 = 1 << 5;

/// Everything a hot loop asks about a block, packed flat and indexed by the raw
/// block number. One cache line holds two of these.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BlockHot {
    pub color: [f32; 3],
    pub hardness: f32,
    pub flags: u8,
    pub render: RenderKind,
    /// Block light emitted, 0..=15.
    pub light: u8,
    /// Light eaten passing through, 0..=15.
    pub light_opacity: u8,
}

impl BlockHot {
    /// What an id nobody defined looks like: magenta, unremarkable, solid.
    const MISSING: BlockHot = BlockHot {
        color: [1.0, 0.0, 1.0],
        hardness: 1.0,
        flags: F_OPAQUE | F_SOLID,
        render: RenderKind::Solid,
        light: 0,
        light_opacity: 15,
    };

    #[inline]
    pub fn has(self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// The cold half of a block: everything that is not asked for per face.
#[derive(Clone, Debug)]
pub struct BlockDef {
    pub id: BlockId,
    pub name: String,
    pub harvest: HarvestRule,
    /// What mining it yields, once [`HarvestRule`] has been satisfied.
    pub drop: Option<ItemId>,
}

/// One tool material, with the numbers that make it worth upgrading to.
#[derive(Clone, Debug)]
pub struct TierDef {
    pub name: String,
    pub rank: u8,
    pub speed: f32,
    pub durability: u16,
    pub material: ItemId,
    pub color: [f32; 3],
}

/// One item, fully resolved.
#[derive(Clone, Debug)]
pub struct ItemDef {
    pub id: ItemId,
    pub name: String,
    pub display: String,
    pub places: Option<BlockId>,
    pub tool: Option<(ToolKind, ToolTier)>,
    pub max_stack: u8,
    pub durability: u16,
    pub attack: f32,
    pub fuel: Option<f32>,
    pub color: [f32; 3],
}

/// Every piece of content the game knows about.
pub struct Registry {
    hot: [BlockHot; 256],
    blocks: Vec<Option<BlockDef>>,
    block_by_name: HashMap<String, BlockId>,
    items: Vec<Option<ItemDef>>,
    item_by_name: HashMap<String, ItemId>,
    item_ids: Vec<ItemId>,
    /// The item for each `[tool kind][tier]`, so `ItemId::tool_item` is a
    /// lookup rather than a nine-arm match.
    tool_items: [[ItemId; 3]; 3],
    tiers: [TierDef; 3],
    recipes: Vec<Recipe>,
    smelting: Vec<SmeltRecipe>,
}

/// A one-line summary. Printing 256 block rows in a failed assertion helps
/// nobody.
impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Registry {{ {} blocks, {} items, {} recipes, {} smelting }}",
            self.blocks.iter().flatten().count(),
            self.item_ids.len(),
            self.recipes.len(),
            self.smelting.len()
        )
    }
}

impl Registry {
    /// Hot per-block properties. Never fails: an unknown id reads as
    /// [`BlockHot::MISSING`], so a save from a newer build renders magenta
    /// instead of panicking.
    #[inline]
    pub fn hot(&self, id: BlockId) -> &BlockHot {
        // `u8 as usize` is provably < 256, so this indexes without a check.
        &self.hot[id.0 as usize]
    }

    pub fn block(&self, id: BlockId) -> Option<&BlockDef> {
        self.blocks.get(id.0 as usize).and_then(|b| b.as_ref())
    }

    /// The block with this data-file name.
    pub fn block_id(&self, name: &str) -> Option<BlockId> {
        self.block_by_name.get(name).copied()
    }

    pub fn item(&self, id: ItemId) -> Option<&ItemDef> {
        self.items.get(id.0 as usize).and_then(|i| i.as_ref())
    }

    /// The item with this data-file name.
    pub fn item_id(&self, name: &str) -> Option<ItemId> {
        self.item_by_name.get(name).copied()
    }

    /// Every item id the registry knows, in file order. A superset of
    /// [`ItemId::ALL`] once a mod adds items.
    pub fn item_ids(&self) -> &[ItemId] {
        &self.item_ids
    }

    pub fn tier(&self, tier: ToolTier) -> &TierDef {
        &self.tiers[tier_index(tier)]
    }

    /// The one item that is this kind of tool at this tier. Loading fails
    /// unless every combination is filled exactly once, so this cannot miss.
    pub fn tool_item(&self, kind: ToolKind, tier: ToolTier) -> ItemId {
        self.tool_items[kind_index(kind)][tier_index(tier)]
    }

    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }

    pub fn smelting(&self) -> &[SmeltRecipe] {
        &self.smelting
    }

    /// The highest block id defined. Blocks above it read as missing.
    pub fn max_block_id(&self) -> u8 {
        self.blocks
            .iter()
            .rposition(|b| b.is_some())
            .unwrap_or(0)
            .min(255) as u8
    }
}

fn tier_index(tier: ToolTier) -> usize {
    match tier {
        ToolTier::Wood => 0,
        ToolTier::Stone => 1,
        ToolTier::Iron => 2,
    }
}

fn kind_index(kind: ToolKind) -> usize {
    match kind {
        ToolKind::Pickaxe => 0,
        ToolKind::Axe => 1,
        ToolKind::Sword => 2,
    }
}

/// The tier names this build understands. `ToolTier` is an enum because
/// `texture.rs` matches on it to pick an icon, so the *set* of tiers is code
/// and their numbers are data.
const TIER_NAMES: [(&str, ToolTier); 3] = [
    ("wood", ToolTier::Wood),
    ("stone", ToolTier::Stone),
    ("iron", ToolTier::Iron),
];

/// Tags a block entry may carry.
const KNOWN_TAGS: [&str; 3] = ["grassy", "log", "leaves"];

// ---------------------------------------------------------------------------
// Loading
// ---------------------------------------------------------------------------

impl Registry {
    /// Load from disk overrides where they exist, and the built-in copy where
    /// they do not.
    pub fn load() -> Result<Registry, RegistryError> {
        Registry::from_sources(
            &source_for(BLOCKS_FILE, EMBEDDED_BLOCKS)?,
            &source_for(ITEMS_FILE, EMBEDDED_ITEMS)?,
            &source_for(RECIPES_FILE, EMBEDDED_RECIPES)?,
        )
    }

    /// Load only the copy compiled into the binary, ignoring anything on disk.
    /// Tests assert against this so a stray `assets/` folder cannot change what
    /// they measure.
    pub fn from_embedded() -> Result<Registry, RegistryError> {
        Registry::from_str_data(EMBEDDED_BLOCKS, EMBEDDED_ITEMS, EMBEDDED_RECIPES)
    }

    /// Load from three strings, labelled as the built-in files. This is the
    /// entry point a test uses to feed deliberately broken data.
    pub fn from_str_data(
        blocks: &str,
        items: &str,
        recipes: &str,
    ) -> Result<Registry, RegistryError> {
        let mk = |file: &str, text: &str| Source {
            label: format!("<built-in {file}>"),
            text: Cow::Owned(text.to_string()),
        };
        Registry::from_sources(
            &mk(BLOCKS_FILE, blocks),
            &mk(ITEMS_FILE, items),
            &mk(RECIPES_FILE, recipes),
        )
    }

    fn from_sources(
        blocks_src: &Source,
        items_src: &Source,
        recipes_src: &Source,
    ) -> Result<Registry, RegistryError> {
        let blocks_file: BlocksFile = blocks_src.parse()?;
        let items_file: ItemsFile = items_src.parse()?;
        let recipes_file: RecipesFile = recipes_src.parse()?;

        // --- pass 1: names and ids, so the three files can refer to each other
        let mut block_by_name: HashMap<String, BlockId> = HashMap::new();
        let mut block_name_of: HashMap<u8, &str> = HashMap::new();
        for b in &blocks_file.blocks {
            if let Some(prev) = block_name_of.insert(b.id, &b.name) {
                return Err(blocks_src.err(format!(
                    "id {} is used twice, by \"{prev}\" and by \"{}\" -- \
                     block ids are frozen and must be unique",
                    b.id, b.name
                )));
            }
            if b.name.is_empty() {
                return Err(blocks_src.err(format!("the block with id {} has no name", b.id)));
            }
            if block_by_name.insert(b.name.clone(), BlockId(b.id)).is_some() {
                return Err(blocks_src.err(format!(
                    "two blocks are named \"{}\" -- names must be unique",
                    b.name
                )));
            }
        }

        let mut item_by_name: HashMap<String, ItemId> = HashMap::new();
        let mut item_name_of: HashMap<u16, &str> = HashMap::new();
        for i in &items_file.items {
            if let Some(prev) = item_name_of.insert(i.id, &i.name) {
                return Err(items_src.err(format!(
                    "id {} is used twice, by \"{prev}\" and by \"{}\" -- \
                     item ids are frozen and must be unique",
                    i.id, i.name
                )));
            }
            if i.name.is_empty() {
                return Err(items_src.err(format!("the item with id {} has no name", i.id)));
            }
            if item_by_name.insert(i.name.clone(), ItemId(i.id)).is_some() {
                return Err(items_src.err(format!(
                    "two items are named \"{}\" -- names must be unique",
                    i.name
                )));
            }
        }

        // --- tiers, which the block gates and the tool items both refer to ---
        let tiers = build_tiers(&items_file, items_src, &item_by_name)?;

        // --- items ------------------------------------------------------------
        let max_item = items_file.items.iter().map(|i| i.id).max().unwrap_or(0);
        let mut items: Vec<Option<ItemDef>> = vec![None; max_item as usize + 1];
        let mut item_ids: Vec<ItemId> = Vec::with_capacity(items_file.items.len());
        for e in &items_file.items {
            let where_ = format!("item \"{}\" (id {})", e.name, e.id);

            let places = match &e.places {
                None => None,
                Some(name) => {
                    let id = block_by_name.get(name.as_str()).copied().ok_or_else(|| {
                        items_src.err(format!(
                            "{where_} places block \"{name}\", which no entry in {BLOCKS_FILE} defines"
                        ))
                    })?;
                    // The one rule the save format cannot bend.
                    if id.0 as u16 != e.id {
                        return Err(items_src.err(format!(
                            "{where_} places block \"{name}\", which has id {} -- \
                             a block item must carry the same number as its block",
                            id.0
                        )));
                    }
                    Some(id)
                }
            };

            let tool = match &e.tool {
                None => None,
                Some((kind, tier_name)) => Some((*kind, lookup_tier(tier_name, items_src, &where_)?)),
            };

            let max_stack = e.max_stack.unwrap_or(if tool.is_some() { 1 } else { 64 });
            if max_stack == 0 || max_stack > 64 {
                return Err(items_src.err(format!(
                    "{where_} has max_stack {max_stack}; it must be between 1 and 64"
                )));
            }
            let durability = match (e.durability, tool) {
                (Some(d), _) => d,
                (None, Some((_, tier))) => tiers[tier_index(tier)].durability,
                (None, None) => 0,
            };
            if !e.attack.is_finite() || e.attack < 0.0 {
                return Err(items_src.err(format!(
                    "{where_} has attack {}; it must be a number of at least 0",
                    e.attack
                )));
            }
            if let Some(f) = e.fuel
                && !(f.is_finite() && f > 0.0)
            {
                return Err(items_src.err(format!(
                    "{where_} burns for {f} seconds; fuel must be a positive number"
                )));
            }
            let color = match (&e.color, places, tool) {
                (Some(c), _, _) => to_color(c, items_src, &where_)?,
                // Filled in below, once blocks are resolved.
                (None, Some(_), _) => [1.0, 0.0, 1.0],
                (None, None, Some((_, tier))) => tiers[tier_index(tier)].color,
                (None, None, None) => [1.0, 0.0, 1.0],
            };

            items[e.id as usize] = Some(ItemDef {
                id: ItemId(e.id),
                name: e.name.clone(),
                display: e.display.clone(),
                places,
                tool,
                max_stack,
                durability,
                attack: e.attack,
                fuel: e.fuel,
                color,
            });
            item_ids.push(ItemId(e.id));
        }

        // --- blocks -----------------------------------------------------------
        let max_block = blocks_file.blocks.iter().map(|b| b.id).max().unwrap_or(0);
        let mut hot = [BlockHot::MISSING; 256];
        let mut blocks: Vec<Option<BlockDef>> = vec![None; max_block as usize + 1];
        for e in &blocks_file.blocks {
            let where_ = format!("block \"{}\" (id {})", e.name, e.id);
            let color = to_color(&e.color, blocks_src, &where_)?;
            if e.hardness.is_nan() || e.hardness <= 0.0 {
                return Err(blocks_src.err(format!(
                    "{where_} has hardness {}; it must be greater than 0 (use `inf` for unbreakable)",
                    e.hardness
                )));
            }
            if e.light > 15 {
                return Err(blocks_src.err(format!(
                    "{where_} emits light {}; the range is 0..=15",
                    e.light
                )));
            }
            if let Some(o) = e.light_opacity
                && o > 15
            {
                return Err(blocks_src.err(format!(
                    "{where_} has light_opacity {o}; the range is 0..=15"
                )));
            }

            let mut flags = 0u8;
            if e.opaque {
                flags |= F_OPAQUE;
            }
            if e.solid {
                flags |= F_SOLID;
            }
            if e.replaceable {
                flags |= F_REPLACEABLE;
            }
            for tag in &e.tags {
                match tag.as_str() {
                    "grassy" => flags |= F_GRASSY,
                    "log" => flags |= F_LOG,
                    "leaves" => flags |= F_LEAVES,
                    other => {
                        return Err(blocks_src.err(format!(
                            "{where_} carries the tag \"{other}\", which means nothing here; \
                             the tags this build knows are {}",
                            KNOWN_TAGS.join(", ")
                        )));
                    }
                }
            }

            // Light stops dead at anything opaque and is merely dimmed by a
            // canopy, unless the entry says otherwise.
            let light_opacity = e.light_opacity.unwrap_or({
                if e.opaque {
                    15
                } else if flags & F_LEAVES != 0 {
                    1
                } else {
                    0
                }
            });

            hot[e.id as usize] = BlockHot {
                color,
                hardness: e.hardness,
                flags,
                render: e.render,
                light: e.light,
                light_opacity,
            };

            // --- harvest gating ---
            let effective = e.tool.kind();
            let (required, harvestable) = match &e.requires {
                Requires::Hands => (None, true),
                Requires::Never => (None, false),
                Requires::Tier(name) => {
                    let tier = lookup_tier(name, blocks_src, &where_)?;
                    let Some(kind) = effective else {
                        return Err(blocks_src.err(format!(
                            "{where_} requires a {name} tool but names no `tool` kind -- \
                             add `tool: Pickaxe` (or Axe, or Sword)"
                        )));
                    };
                    (Some((kind, tier)), true)
                }
            };

            let drop = match &e.drops {
                Drops::Nothing => None,
                Drops::Itself => {
                    // The item that places this block, if the data defines one.
                    let self_item = ItemId(e.id as u16);
                    match items.get(self_item.0 as usize).and_then(|i| i.as_ref()) {
                        Some(def) if def.places == Some(BlockId(e.id)) => Some(self_item),
                        _ => None,
                    }
                }
                Drops::Item(name) => Some(item_by_name.get(name.as_str()).copied().ok_or_else(
                    || {
                        blocks_src.err(format!(
                            "{where_} drops \"{name}\", which no entry in {ITEMS_FILE} defines"
                        ))
                    },
                )?),
            };

            blocks[e.id as usize] = Some(BlockDef {
                id: BlockId(e.id),
                name: e.name.clone(),
                harvest: HarvestRule {
                    effective,
                    required,
                    harvestable,
                },
                drop,
            });
        }

        // --- the tool grid ----------------------------------------------------
        // `ItemId::tool_item(kind, tier)` has to return something for all nine
        // combinations, so all nine must exist and none may be claimed twice.
        let mut tool_items = [[None::<ItemId>; 3]; 3];
        for def in items.iter().flatten() {
            let Some((kind, tier)) = def.tool else { continue };
            let slot = &mut tool_items[kind_index(kind)][tier_index(tier)];
            if let Some(prev) = *slot {
                let prev_name = items[prev.0 as usize]
                    .as_ref()
                    .map_or("?", |d| d.name.as_str());
                return Err(items_src.err(format!(
                    "\"{}\" and \"{prev_name}\" are both the {} {kind:?} -- \
                     each tool kind and tier may be claimed by exactly one item",
                    def.name,
                    tiers[tier_index(tier)].name,
                )));
            }
            *slot = Some(def.id);
        }
        let mut tool_grid = [[ItemId(0); 3]; 3];
        for kind in ToolKind::ALL {
            for tier in ToolTier::ALL {
                match tool_items[kind_index(kind)][tier_index(tier)] {
                    Some(id) => tool_grid[kind_index(kind)][tier_index(tier)] = id,
                    None => {
                        return Err(items_src.err(format!(
                            "no item is the {} {kind:?}; the game needs one of every \
                             tool kind at every tier, even if no recipe makes it",
                            tiers[tier_index(tier)].name
                        )));
                    }
                }
            }
        }

        // Block items borrow their block's colour, which is only known now.
        for def in items.iter_mut().flatten() {
            if let Some(b) = def.places {
                let entry = items_file.items.iter().find(|e| e.id == def.id.0);
                if entry.is_some_and(|e| e.color.is_none()) {
                    def.color = hot[b.0 as usize].color;
                }
            }
        }

        // --- recipes ----------------------------------------------------------
        let mut recipes: Vec<Recipe> = Vec::new();
        for e in &recipes_file.shapeless {
            let where_ = format!("shapeless recipe for \"{}\"", e.output);
            let output = lookup_item(&e.output, &item_by_name, recipes_src, &where_)?;
            check_count(e.count, recipes_src, &where_)?;
            if e.ingredients.is_empty() || e.ingredients.len() > 9 {
                return Err(recipes_src.err(format!(
                    "{where_} lists {} ingredients; a grid holds between 1 and 9",
                    e.ingredients.len()
                )));
            }
            let mut ingredients = Vec::with_capacity(e.ingredients.len());
            for name in &e.ingredients {
                ingredients.push(lookup_item(name, &item_by_name, recipes_src, &where_)?);
            }
            recipes.push(Recipe {
                pattern: Pattern::Shapeless(ingredients),
                output,
                count: e.count,
            });
        }
        for e in &recipes_file.shaped {
            let where_ = format!("shaped recipe for \"{}\"", e.output);
            let output = lookup_item(&e.output, &item_by_name, recipes_src, &where_)?;
            check_count(e.count, recipes_src, &where_)?;
            let (width, height, cells) =
                build_pattern(e, &item_by_name, recipes_src, &where_)?;
            recipes.push(Recipe {
                pattern: Pattern::Shaped {
                    width,
                    height,
                    cells,
                },
                output,
                count: e.count,
            });
        }

        let mut smelting: Vec<SmeltRecipe> = Vec::new();
        for e in &recipes_file.smelting {
            let where_ = format!("smelting recipe for \"{}\"", e.output);
            let input = lookup_item(&e.input, &item_by_name, recipes_src, &where_)?;
            let output = lookup_item(&e.output, &item_by_name, recipes_src, &where_)?;
            if !(e.seconds.is_finite() && e.seconds > 0.0) {
                return Err(recipes_src.err(format!(
                    "{where_} takes {} seconds; it must be a positive number",
                    e.seconds
                )));
            }
            if smelting.iter().any(|s: &SmeltRecipe| s.input == input) {
                return Err(recipes_src.err(format!(
                    "{where_} is the second recipe that smelts \"{}\"; only the first would ever run",
                    e.input
                )));
            }
            smelting.push(SmeltRecipe {
                input,
                output,
                seconds: e.seconds,
            });
        }

        Ok(Registry {
            hot,
            blocks,
            block_by_name,
            items,
            item_by_name,
            item_ids,
            tool_items: tool_grid,
            tiers,
            recipes,
            smelting,
        })
    }
}

/// Read a `[r, g, b]` list, saying plainly what is wrong with it if anything is.
fn to_color(c: &[f32], src: &Source, where_: &str) -> Result<[f32; 3], RegistryError> {
    if c.len() != 3 {
        return Err(src.err(format!(
            "{where_} has a colour of {} numbers; it must be [red, green, blue]",
            c.len()
        )));
    }
    for v in c {
        if !v.is_finite() || *v < 0.0 || *v > 1.0 {
            return Err(src.err(format!(
                "{where_} has colour {c:?}; every channel must be between 0.0 and 1.0"
            )));
        }
    }
    Ok([c[0], c[1], c[2]])
}

fn check_count(count: u8, src: &Source, where_: &str) -> Result<(), RegistryError> {
    if count == 0 || count > 64 {
        return Err(src.err(format!(
            "{where_} yields {count} items; the range is 1 to 64"
        )));
    }
    Ok(())
}

fn lookup_tier(name: &str, src: &Source, where_: &str) -> Result<ToolTier, RegistryError> {
    TIER_NAMES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
        .ok_or_else(|| {
            src.err(format!(
                "{where_} names the tool tier \"{name}\", which is not defined; \
                 this build knows {}",
                TIER_NAMES
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

fn lookup_item(
    name: &str,
    by_name: &HashMap<String, ItemId>,
    src: &Source,
    where_: &str,
) -> Result<ItemId, RegistryError> {
    by_name.get(name).copied().ok_or_else(|| {
        src.err(format!(
            "{where_} uses \"{name}\", which no entry in {ITEMS_FILE} defines"
        ))
    })
}

fn build_tiers(
    file: &ItemsFile,
    src: &Source,
    item_by_name: &HashMap<String, ItemId>,
) -> Result<[TierDef; 3], RegistryError> {
    let mut slots: [Option<TierDef>; 3] = [None, None, None];
    for e in &file.tiers {
        let where_ = format!("tool tier \"{}\"", e.name);
        let tier = lookup_tier(&e.name, src, &where_)?;
        let idx = tier_index(tier);
        if slots[idx].is_some() {
            return Err(src.err(format!("{where_} is defined twice")));
        }
        if !(e.speed.is_finite() && e.speed >= 1.0) {
            return Err(src.err(format!(
                "{where_} mines at speed {}; a tier must be at least 1.0, which is bare hands",
                e.speed
            )));
        }
        if e.durability == 0 {
            return Err(src.err(format!("{where_} has 0 durability, so it breaks unused")));
        }
        let color = to_color(&e.color, src, &where_)?;
        let material = lookup_item(&e.material, item_by_name, src, &where_)?;
        slots[idx] = Some(TierDef {
            name: e.name.clone(),
            rank: e.rank,
            speed: e.speed,
            durability: e.durability,
            material,
            color,
        });
    }

    let mut out: Vec<TierDef> = Vec::with_capacity(3);
    for (name, tier) in TIER_NAMES {
        match slots[tier_index(tier)].take() {
            Some(t) => out.push(t),
            None => {
                return Err(src.err(format!(
                    "no tool tier named \"{name}\" is defined, and this build needs all of {}",
                    TIER_NAMES
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
        }
    }
    // `ToolTier` compares by declaration order, and `can_harvest` uses that
    // ordering, so a rank that disagrees would gate the wrong way round.
    for pair in out.windows(2) {
        if pair[1].rank <= pair[0].rank {
            return Err(src.err(format!(
                "tier \"{}\" has rank {} but comes after \"{}\" at rank {}; \
                 ranks must increase in the order {}",
                pair[1].name,
                pair[1].rank,
                pair[0].name,
                pair[0].rank,
                TIER_NAMES
                    .iter()
                    .map(|(n, _)| *n)
                    .collect::<Vec<_>>()
                    .join(" < ")
            )));
        }
    }

    let [a, b, c] = <[TierDef; 3]>::try_from(out).ok().expect("three tiers");
    Ok([a, b, c])
}

/// Turn rows of text into the flat cell grid the matcher wants.
fn build_pattern(
    e: &ShapedEntry,
    item_by_name: &HashMap<String, ItemId>,
    src: &Source,
    where_: &str,
) -> Result<(usize, usize, Vec<Option<ItemId>>), RegistryError> {
    let height = e.pattern.len();
    if height == 0 || height > 3 {
        return Err(src.err(format!(
            "{where_} is {height} rows tall; a crafting grid is at most 3"
        )));
    }
    let rows: Vec<Vec<char>> = e.pattern.iter().map(|r| r.chars().collect()).collect();
    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if width == 0 || width > 3 {
        return Err(src.err(format!(
            "{where_} is {width} cells wide; a crafting grid is at most 3"
        )));
    }
    if let Some(bad) = e.key.keys().find(|c| c.is_whitespace()) {
        return Err(src.err(format!(
            "{where_} maps {bad:?} in its key, but a space is always an empty cell"
        )));
    }

    let mut cells = vec![None; width * height];
    for (y, row) in rows.iter().enumerate() {
        for (x, ch) in row.iter().enumerate() {
            if *ch == ' ' {
                continue;
            }
            let name = e.key.get(ch).ok_or_else(|| {
                src.err(format!(
                    "{where_} draws {ch:?} in its pattern but its key does not say what that is"
                ))
            })?;
            cells[y * width + x] = Some(lookup_item(name, item_by_name, src, where_)?);
        }
    }
    if cells.iter().all(|c| c.is_none()) {
        return Err(src.err(format!("{where_} has an entirely empty pattern")));
    }
    // A key entry nobody draws is a typo in one place or the other.
    for ch in e.key.keys() {
        if !rows.iter().any(|r| r.contains(ch)) {
            return Err(src.err(format!(
                "{where_} maps {ch:?} in its key but never draws it in the pattern"
            )));
        }
    }
    Ok((width, height, cells))
}

// ---------------------------------------------------------------------------
// The global registry
// ---------------------------------------------------------------------------

static REGISTRY: OnceLock<Registry> = OnceLock::new();

/// Load the content data, reporting anything wrong with it.
///
/// Call this once at startup, before the world is generated, and print the
/// error if there is one. The registry is left holding the built-in data even
/// when this fails, so the game still starts and the player sees a working
/// world rather than a crash. Calling it twice, or after something has already
/// read the registry, is harmless and does nothing.
pub fn init() -> Result<(), RegistryError> {
    match Registry::load() {
        Ok(r) => {
            let _ = REGISTRY.set(r);
            Ok(())
        }
        Err(e) => {
            let _ = REGISTRY.set(
                Registry::from_embedded().expect("the built-in content data must always load"),
            );
            Err(e)
        }
    }
}

/// The registry, loading it on first use if [`init`] was never called.
#[inline(always)]
pub fn get() -> &'static Registry {
    match REGISTRY.get() {
        Some(r) => r,
        None => load_lazily(),
    }
}

#[cold]
#[inline(never)]
fn load_lazily() -> &'static Registry {
    REGISTRY.get_or_init(|| match Registry::load() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[loudstone] {e}");
            eprintln!("[loudstone] falling back to the built-in content data");
            Registry::from_embedded().expect("the built-in content data must always load")
        }
    })
}

/// Hot per-block properties. This is the call in the mesher's inner loop: one
/// atomic load and one indexed read, no hashing and no locking.
#[inline(always)]
pub fn block(id: BlockId) -> &'static BlockHot {
    get().hot(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded() -> Registry {
        Registry::from_embedded().expect("the built-in data must parse")
    }

    #[test]
    fn the_embedded_data_parses_and_validates() {
        let r = embedded();
        assert_eq!(r.max_block_id(), BlockId::MAX);
        assert!(r.recipes().len() >= 12);
        assert_eq!(r.smelting().len(), 1);
        assert!(r.item_ids().len() >= 28);
    }

    #[test]
    fn frozen_block_ids_resolve_to_their_names() {
        // If any of these move, every save file on disk silently becomes a
        // different world and the atlas paints the wrong tiles.
        let frozen: [(u8, &str); 18] = [
            (0, "air"),
            (1, "stone"),
            (2, "dirt"),
            (3, "grass"),
            (4, "sand"),
            (5, "wood"),
            (6, "leaves"),
            (7, "planks"),
            (8, "cobblestone"),
            (9, "coal_ore"),
            (10, "iron_ore"),
            (11, "gold_ore"),
            (12, "diamond_ore"),
            (13, "bedrock"),
            (14, "water"),
            (15, "torch"),
            (16, "crafting_table"),
            (17, "furnace"),
        ];
        let r = embedded();
        for (id, name) in frozen {
            let def = r
                .block(BlockId(id))
                .unwrap_or_else(|| panic!("block id {id} is missing from blocks.ron"));
            assert_eq!(def.name, name, "block id {id} changed identity");
            assert_eq!(r.block_id(name), Some(BlockId(id)));
        }
    }

    #[test]
    fn appended_block_ids_are_also_pinned() {
        let appended: [(u8, &str); 21] = [
            (18, "sandstone"),
            (19, "gravel"),
            (20, "clay"),
            (21, "snow"),
            (22, "ice"),
            (23, "granite"),
            (24, "diorite"),
            (25, "andesite"),
            (26, "birch_log"),
            (27, "birch_leaves"),
            (28, "spruce_log"),
            (29, "spruce_leaves"),
            (30, "cactus"),
            (31, "tall_grass"),
            (32, "flower_red"),
            (33, "flower_yellow"),
            (34, "dead_bush"),
            (35, "grass_cold"),
            (36, "grass_dry"),
            (37, "grass_swamp"),
            (38, "podzol"),
        ];
        let r = embedded();
        for (id, name) in appended {
            assert_eq!(r.block(BlockId(id)).map(|d| d.name.as_str()), Some(name));
        }
    }

    #[test]
    fn frozen_item_ids_resolve_to_their_names() {
        let frozen: [(u16, &str); 28] = [
            (1, "stone"),
            (2, "dirt"),
            (3, "grass"),
            (4, "sand"),
            (5, "wood"),
            (6, "leaves"),
            (7, "planks"),
            (8, "cobblestone"),
            (9, "coal_ore"),
            (10, "iron_ore"),
            (11, "gold_ore"),
            (12, "diamond_ore"),
            (15, "torch"),
            (16, "crafting_table"),
            (17, "furnace"),
            (100, "stick"),
            (101, "coal"),
            (102, "raw_iron"),
            (103, "iron_ingot"),
            (110, "wooden_pickaxe"),
            (111, "stone_pickaxe"),
            (112, "iron_pickaxe"),
            (120, "wooden_axe"),
            (121, "stone_axe"),
            (122, "iron_axe"),
            (130, "wooden_sword"),
            (131, "stone_sword"),
            (132, "iron_sword"),
        ];
        let r = embedded();
        for (id, name) in frozen {
            assert_eq!(
                r.item(ItemId(id)).map(|d| d.name.as_str()),
                Some(name),
                "item id {id} changed identity"
            );
        }
    }

    #[test]
    fn every_block_a_texture_needs_exists() {
        // `texture.rs` walks 1..=BlockId::MAX and demands a tile for each, so
        // every one of those ids must be a real block.
        let r = embedded();
        for n in 1..=BlockId::MAX {
            assert!(
                r.block(BlockId(n)).is_some(),
                "texture.rs asks for a tile for block {n}, which blocks.ron does not define"
            );
        }
    }

    #[test]
    fn every_recipe_names_real_items() {
        let r = embedded();
        let known = |i: ItemId| r.item(i).is_some();
        for recipe in r.recipes() {
            assert!(known(recipe.output), "recipe output {:?} is unknown", recipe.output);
            assert!(recipe.count > 0);
            match &recipe.pattern {
                Pattern::Shapeless(items) => {
                    for i in items {
                        assert!(known(*i), "shapeless ingredient {i:?} is unknown");
                    }
                }
                Pattern::Shaped {
                    width,
                    height,
                    cells,
                } => {
                    assert_eq!(cells.len(), width * height);
                    assert!(*width <= 3 && *height <= 3);
                    for i in cells.iter().flatten() {
                        assert!(known(*i), "shaped ingredient {i:?} is unknown");
                    }
                }
            }
        }
        for s in r.smelting() {
            assert!(known(s.input) && known(s.output));
            assert!(s.seconds > 0.0);
        }
    }

    #[test]
    fn every_block_drop_and_gate_resolves() {
        let r = embedded();
        for n in 0..=BlockId::MAX {
            let def = r.block(BlockId(n)).expect("defined");
            if let Some(d) = def.drop {
                assert!(r.item(d).is_some(), "block {n} drops an item that does not exist");
            }
            if let Some((kind, tier)) = def.harvest.required {
                assert_eq!(def.harvest.effective, Some(kind));
                // The tier must be one the registry actually built.
                assert!(!r.tier(tier).name.is_empty());
            }
        }
    }

    // --- the error paths -----------------------------------------------------

    /// Swap one field in the built-in blocks file, so a test can break exactly
    /// one thing and leave everything else valid.
    fn blocks_with(replace: &str, with: &str) -> String {
        assert!(
            EMBEDDED_BLOCKS.contains(replace),
            "the fixture text {replace:?} is no longer in blocks.ron"
        );
        EMBEDDED_BLOCKS.replacen(replace, with, 1)
    }

    fn load_broken_blocks(text: &str) -> RegistryError {
        Registry::from_str_data(text, EMBEDDED_ITEMS, EMBEDDED_RECIPES)
            .expect_err("this data should have been rejected")
    }

    #[test]
    fn malformed_ron_reports_the_file_and_the_place() {
        let e = load_broken_blocks("( blocks: [ (id: 0, name: \"air\" ");
        assert!(e.file.contains("blocks.ron"), "{e}");
        // The message must point at a position, not just say "error".
        assert!(
            e.to_string().contains("1:") || e.to_string().to_lowercase().contains("line"),
            "no position in {e}"
        );
    }

    #[test]
    fn an_unknown_field_is_named() {
        let e = load_broken_blocks(&blocks_with("hardness: 1.5,\n            tool: Pickaxe", "hardnes: 1.5,\n            tool: Pickaxe"));
        assert!(e.to_string().contains("hardnes"), "{e}");
    }

    #[test]
    fn a_duplicate_id_is_rejected_by_name() {
        let e = load_broken_blocks(&blocks_with("(id: 19, name: \"gravel\"", "(id: 18, name: \"gravel\""));
        assert!(e.detail.contains("gravel") && e.detail.contains("18"), "{e}");
    }

    #[test]
    fn an_unknown_drop_item_is_rejected_by_name() {
        let e = load_broken_blocks(&blocks_with("drops: Item(\"raw_iron\")", "drops: Item(\"raw_irn\")"));
        assert!(e.detail.contains("iron_ore") && e.detail.contains("raw_irn"), "{e}");
        assert!(e.detail.contains(ITEMS_FILE), "{e}");
    }

    #[test]
    fn an_undefined_tool_tier_is_rejected_by_name() {
        let e = load_broken_blocks(&blocks_with("requires: Tier(\"stone\")", "requires: Tier(\"bronze\")"));
        assert!(e.detail.contains("bronze") && e.detail.contains("iron_ore"), "{e}");
        assert!(e.detail.contains("wood"), "the message should list the real tiers: {e}");
    }

    #[test]
    fn a_tier_gate_without_a_tool_kind_is_rejected() {
        let e = load_broken_blocks(&blocks_with(
            "(id: 18, name: \"sandstone\",      color: [0.76, 0.70, 0.49], hardness: 0.9)",
            "(id: 18, name: \"sandstone\",      color: [0.76, 0.70, 0.49], hardness: 0.9, requires: Tier(\"iron\"))",
        ));
        assert!(e.detail.contains("sandstone") && e.detail.contains("tool"), "{e}");
    }

    #[test]
    fn an_unknown_tag_is_rejected_and_the_real_ones_listed() {
        let e = load_broken_blocks(&blocks_with("tags: [\"grassy\"], drops: Item(\"dirt\")", "tags: [\"grasy\"], drops: Item(\"dirt\")"));
        assert!(e.detail.contains("grasy") && e.detail.contains("grassy"), "{e}");
    }

    #[test]
    fn an_impossible_hardness_is_rejected() {
        let e = load_broken_blocks(&blocks_with("hardness: 0.9)", "hardness: 0.0)"));
        assert!(e.detail.contains("sandstone") && e.detail.contains("hardness"), "{e}");
    }

    #[test]
    fn a_colour_outside_the_range_is_rejected() {
        let e = load_broken_blocks(&blocks_with("color: [0.76, 0.70, 0.49]", "color: [76.0, 0.70, 0.49]"));
        assert!(e.detail.contains("sandstone") && e.detail.contains("colour"), "{e}");
    }

    #[test]
    fn a_recipe_for_a_nonexistent_item_is_rejected() {
        let recipes = EMBEDDED_RECIPES.replacen("output: \"stick\"", "output: \"stik\"", 1);
        let e = Registry::from_str_data(EMBEDDED_BLOCKS, EMBEDDED_ITEMS, &recipes)
            .expect_err("an unknown recipe output must be rejected");
        assert!(e.detail.contains("stik") && e.detail.contains(ITEMS_FILE), "{e}");
    }

    #[test]
    fn a_pattern_character_with_no_key_is_rejected() {
        let recipes = EMBEDDED_RECIPES.replacen("\"CCC\",\n                \"C C\"", "\"CQC\",\n                \"C C\"", 1);
        let e = Registry::from_str_data(EMBEDDED_BLOCKS, EMBEDDED_ITEMS, &recipes)
            .expect_err("an undrawable pattern character must be rejected");
        assert!(e.detail.contains("furnace") && e.detail.contains('Q'), "{e}");
    }

    #[test]
    fn a_pattern_wider_than_the_grid_is_rejected() {
        let recipes = EMBEDDED_RECIPES.replacen("\"CCC\",\n                \"C C\"", "\"CCCC\",\n                \"C C\"", 1);
        let e = Registry::from_str_data(EMBEDDED_BLOCKS, EMBEDDED_ITEMS, &recipes)
            .expect_err("an oversized pattern must be rejected");
        assert!(e.detail.contains("furnace") && e.detail.contains('4'), "{e}");
    }

    #[test]
    fn a_block_item_that_renumbers_its_block_is_rejected() {
        let items = EMBEDDED_ITEMS.replacen(
            "(id:  2, name: \"dirt\",           display: \"Dirt\",           places: Some(\"dirt\"))",
            "(id:  2, name: \"dirt\",           display: \"Dirt\",           places: Some(\"sand\"))",
            1,
        );
        let e = Registry::from_str_data(EMBEDDED_BLOCKS, &items, EMBEDDED_RECIPES)
            .expect_err("a block item must carry its block's number");
        assert!(e.detail.contains("dirt") && e.detail.contains("same number"), "{e}");
    }

    #[test]
    fn a_missing_tier_is_rejected() {
        let items = EMBEDDED_ITEMS.replacen("(name: \"stone\", rank: 2", "(name: \"bronze\", rank: 2", 1);
        let e = Registry::from_str_data(EMBEDDED_BLOCKS, &items, EMBEDDED_RECIPES)
            .expect_err("an unknown tier name must be rejected");
        assert!(e.detail.contains("bronze"), "{e}");
    }

    #[test]
    fn out_of_order_tier_ranks_are_rejected() {
        let items = EMBEDDED_ITEMS.replacen("(name: \"iron\",  rank: 3", "(name: \"iron\",  rank: 1", 1);
        let e = Registry::from_str_data(EMBEDDED_BLOCKS, &items, EMBEDDED_RECIPES)
            .expect_err("ranks must increase with the tier order");
        assert!(e.detail.contains("rank"), "{e}");
    }

    /// A registry read is one atomic load and one array index. This does not
    /// prove a number, it proves nothing pathological -- a lock or a hash --
    /// has crept onto the mesher's hot path.
    #[test]
    fn hot_lookups_stay_about_as_cheap_as_an_array_index() {
        use std::hint::black_box;
        use std::time::Instant;

        const N: usize = 2_000_000;

        // Control: exactly the flat table, reached without the registry.
        let table: [u8; 256] = std::array::from_fn(|i| (i as u8) | 1);
        let control = |reps: usize| {
            let t = Instant::now();
            let mut acc = 0u64;
            for _ in 0..reps {
                for n in 0..=255u8 {
                    acc += black_box(table[black_box(n) as usize]) as u64;
                }
            }
            black_box(acc);
            t.elapsed().as_secs_f64()
        };
        let measured = |reps: usize| {
            let t = Instant::now();
            let mut acc = 0u64;
            for _ in 0..reps {
                for n in 0..=255u8 {
                    let id = BlockId(black_box(n));
                    acc += id.is_opaque() as u64 + (id.hardness() as u64 & 1);
                }
            }
            black_box(acc);
            t.elapsed().as_secs_f64()
        };

        let reps = N / 256;
        // Warm both paths, then take the best of two to shake off scheduling.
        control(reps);
        measured(reps);
        let base = control(reps).min(control(reps)).max(1e-9);
        let ours = measured(reps).min(measured(reps));

        let per_call_ns = ours / N as f64 * 1e9;
        println!(
            "is_opaque + hardness: {per_call_ns:.2} ns per block ({:.2}x a bare array index)",
            ours / base
        );
        assert!(
            ours < base * 8.0,
            "registry lookups cost {:.1}x a plain array index ({per_call_ns:.1} ns each) -- \
             something with a lock or a hash is on the hot path",
            ours / base
        );
    }
}
