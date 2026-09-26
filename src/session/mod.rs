//! Native sessions: TCP transport plus the login state machine (mirrors
//! the `NetworkListenThread`/`NetworkManager` framing and
//! `NetLoginHandler`), and the play packet dispatch (`NetServerHandler`).
//! The server tick owns the session set, broadcast, and config.
//!
//! Wire protocol bytes mirror the vanilla Alpha 1.2.6 layout field-for-field; inbound
//! packets reuse the owned `PacketData` enum from `network.rs`.
//!
//! MAP OF THE SESSION MODULE TREE:
//! - `mod` (this file) — shared context/outcome types, `pkt_*` re-export.
//! - `transport` — `Conn`: socket threads, packet framing, login verify.
//! - `login` — `LoginSession` handshake state machine.
//! - `play` — `PlaySession` core: dispatch (`pump`), movement echo, sends.
//! - `play_movement` — position/look validation.
//! - `play_digging` — block dig + harvest.
//! - `play_action` — entity use, chat, respawn, held-switch, arm swing.
//! - `play_inventory` — inventory apply, ghost stacks, sign text.
//! - `play_use` — block placement, block activation, air-use.
//! - `login_tests`, `play_tests` — tests (same module-tree access).
//!
//! The `pub` re-exports below are the session tree's public surface (used
//! by `server`/`server_admin`); everything else is module-tree-internal.

pub use crate::session_packets::{pkt_handshake, pkt_kick, pkt_login_response, pkt_chat, pkt_time, pkt_spawn_pos, pkt_health, pkt_teleport, pkt_block_change, pkt_inventory_section, pkt_tile_entity, pkt_respawn, pkt_keepalive, pkt_arm, pkt_pre_chunk, pkt_explosion, pkt_map_chunk, tile_packet};

use std::collections::HashSet;
use crate::entity::table::EntityId;
use crate::world::World;

pub mod login;
pub mod play;
pub mod play_action;
pub mod play_digging;
pub mod play_inventory;
pub mod play_movement;
pub mod play_use;
pub mod transport;

pub use self::login::{LoginEvent, LoginSession, VerifyFn, default_verify};
pub use self::play::PlaySession;
pub use self::transport::{Conn, ConnEvent, bind_listener};

/// Cross-session events for the server tick (fan-out, saves).
pub enum SessionBroadcast {
    Chat(String),
    /// Private message: deliver `text` to `target`, or echo the miss note
    /// back to the sender when offline (Java `/tell`).
    Tell { target: String, text: String },
    ArmSwing(EntityId),
    TileChanged(i32, i32, i32),
}

/// Server services borrowed by one pump call (tests fake this; the
/// server slice owns the real one).
pub struct SessionCtx<'a> {
    pub world: &'a mut World,
    pub ops: &'a HashSet<String>,
    pub spawn_protection: i32,
    pub pvp: bool,
    pub broadcast: &'a mut Vec<SessionBroadcast>,
}

/// Terminal session outcome.
pub enum SessionOutcome {
    Kick(String),
    Gone,
}

/// Spawn-protection check (mirrors vanilla `handleBlockDig`/`handlePlace`).
///
/// Vanilla computes `(int)abs((float)(x - spawnX))`; the float cast clamps
/// `i32::MIN` instead of panicking. We use `i64` for the same outcome on
/// normal coords without any panic on extremes.
pub fn is_spawn_protected(x: i32, z: i32, spawn: [i32; 3], radius: i32) -> bool {
    let dx = (x as i64 - spawn[0] as i64).abs();
    let dz = (z as i64 - spawn[2] as i64).abs();
    dx.max(dz) <= radius as i64
}
