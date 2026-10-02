//! Alpha 1.2.6 server in native Rust.
//!
//! Layout: `world/` (storage in `mod`, one responsibility per submodule:
//! physics, living, AI, combat, spawning, blocks, gen) +
//! `session` (transport+play) + `server` (tick fan-out); packet builders
//! live in `session_packets`, tile rows in `world::tiles`.
pub mod block;
pub mod entity;
pub mod byte_buffer;
pub mod inventory;
pub mod tile_entity;
pub mod random;
pub mod noise;
pub mod biome;
pub mod density;
pub mod caves;
pub mod hell_gen;
pub mod decorators;
pub mod nbt;
pub mod chunk;
pub mod network;
pub mod generator;
pub mod pathfinder;
pub mod persist;
pub mod commands;
pub mod player;
pub mod tracker_math;
pub mod mob_spawning;
pub mod server;
pub mod server_admin;
pub mod tracker;
pub mod session;
pub mod session_packets;
pub mod world;
pub mod material;
pub mod nibble;
pub mod server_config;
pub mod server_log;
pub mod aabb;
pub mod math_helper;
pub mod server_constants;
pub mod vec3d;
pub mod item_data;
pub mod item_use;
pub mod item_verbs;

use crate::random::JavaRandom;

/// Grow a small tree through a block accessor (sapling path).
pub fn generate_tree(
    accessor: &mut dyn crate::decorators::BlockAccess,
    seed: i64,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    let mut rand = JavaRandom::new(seed);
    crate::decorators::trees::WorldGenTrees::new().generate(accessor, &mut rand, x, y, z)
}

/// Grow a big tree through a block accessor (1/10 sapling roll).
pub fn generate_big_tree(
    accessor: &mut dyn crate::decorators::BlockAccess,
    seed: i64,
    x: i32,
    y: i32,
    z: i32,
) -> bool {
    let mut rand = JavaRandom::new(seed);
    let mut big_tree = crate::decorators::trees::WorldGenBigTree::new();
    big_tree.configure(1.0, 1.0, 1.0);
    big_tree.generate(accessor, &mut rand, x, y, z)
}
