//! Simulation: the things that move and the clock they move under.
//!
//! The player's body, mobs and their pathfinding, the noise field mob AI
//! listens to, and the day/night cycle. `sound` here is the propagation
//! *model*; audible output lives in `audio`.

pub mod camera;
pub mod daylight;
pub mod mob;
pub mod pathfind;
pub mod session;
pub mod sound;
