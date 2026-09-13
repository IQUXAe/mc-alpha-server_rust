//! Block registry and per-block behavior.
//!
//! MAP OF THE BLOCK MODULE TREE:
//! - `mod` (this file) — module map only.
//! - `table` — block/material registry (`BlockType`, properties table).
//! - `container` — chest/furnace scatter and placement rules.
//! - `fire` — fire spread, aging, catching.
//! - `ticks` — per-block added/neighbor/tick drivers.

pub mod container;
pub mod fire;
pub mod pos;
pub mod table;
pub mod ticks;

pub use pos::{BlockPos, DropSpec};
