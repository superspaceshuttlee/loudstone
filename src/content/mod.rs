//! What the game is made of, and the rules for combining it.
//!
//! Blocks, items, inventories and recipes, all validated against the RON
//! tables in `assets/data` at startup. This layer knows nothing about
//! rendering or about the world it will be placed into.

pub mod block;
pub mod crafting;
pub mod inventory;
pub mod item;
pub mod registry;
