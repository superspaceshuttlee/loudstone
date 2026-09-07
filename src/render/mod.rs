//! Everything that reaches the GPU.
//!
//! The pipelines, the meshers that feed them, the procedural atlas they
//! sample, and the overlay drawn on top. Nothing in here decides what the
//! world contains -- only how it is drawn.

pub mod gfx;
pub mod hud;
pub mod mesh;
pub mod model;
pub mod screenshot;
pub mod texture;
