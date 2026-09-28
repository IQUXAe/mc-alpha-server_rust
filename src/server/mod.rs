//! Native server: owns the `World`, the session set, config, and
//! persistence handles (mirrors `MinecraftServer` +
//! `ServerConfigurationManager` + the `NetworkListenThread` accept/tick
//! pump + the `NetServerHandler` chunk streamer).
//!
//! MAP OF THE SERVER MODULE TREE (`impl Server`, one responsibility each):
//! - `mod` (this file) — `Server` struct, protocol constants, chunk keys.
//! - `settings` — `Settings` from `server.properties`, list files.
//! - `sessions` — accept, login pump, join/leave, packet fan-out.
//! - `streaming` — per-player chunk queue pump.
//! - `tick` — the 20 TPS tick: drains, tracker pass, world tick.
//! - `saves` — players, world, shutdown.
//! - `console` — console commands + `parse_console`.
//! - `tests` — unit/integration tests (same module-tree access).
//!
//! Tick order mirrors `serverTick`: time broadcast, world tick, player
//! autosave, tracker pass, session pump (logins, packets, keep-alive,
//! chunk streaming), furnace-update drain, console lines. Delivery is an
//! immediate `Conn::send` into the per-socket write thread, like the C++
//! send queue.
//!
//! Deliberate divergences from vanilla/C++ (all safe direction):
//! - Duplicate login fully logs the old session out (save + tracker
//!   destroy, no "left" chat like C++): C++ kicks the socket but deletes
//!   the player row without saving and leaves a ghost entity on watchers.
//! - `sendAllToPlayer` after chunk sends is skipped: the per-tick tracker
//!   pass picks new visibility up the same tick.
//! - The C++ `save-modified-chunks-only` property is read by C++ but never
//!   used there; it is not parsed here either.
//! - RNG is the native deterministic stream (documented everywhere), so a
//!   fresh world's seed and spawn walk match in shape, not bit-for-bit.
//! - Anti-cheat clamps (strict by default): vanilla trusts the client —
//!   `handlePlayerInventory` assigns stacks verbatim and `readFromNBT`
//!   copies furnace timers verbatim — so hacked clients can grant 127
//!   stacks, over-damage tools, unknown ids, and infinite furnace burn.
//!   Counts clamp to the per-item max stack, damage to the per-item max
//!   durability, unknown ids drop, furnace `BurnTime`/`ItemBurnTime` to
//!   20000 (lava bucket) and `CookTime` to 200, with an inventory resync
//!   so the client sees the rollback.
//! - Accept gates vanilla lacks: a global `max-connections` TCP cap
//!   (vanilla gates only at join via `max-players`) and a per-IP login
//!   throttle (5 accepts / 10s). Gameplay packets are untouched.
//!
//! Faithful-to-vanilla notes (not divergences, do not "fix"):
//! - `teleport_wait` uses exact x/z equality like
//!   `NetServerHandler.java:48` (`==` on doubles, y² < 0.01).
//! - Chat allow-list excludes `§`: vanilla clients never send colors,
//!   so color packets are spoof attempts, not legit traffic.
//!
//! The `pub` re-exports below are the server tree's public surface (used
//! by `main`); everything else is module-tree-internal.

use std::collections::{BTreeSet, HashMap, HashSet};
use crate::entity::table::EntityId;
use crate::persist::ChunkStore;
use crate::world::World;

pub mod chunk_worker;
pub mod console;
pub mod saves;
pub mod sessions;
pub mod settings;
pub mod streaming;
#[cfg(test)]
mod tests;
pub mod tick;

pub use self::sessions::{Session, SessionState};
pub use self::settings::{Settings, load_settings};

pub const PROTOCOL_VERSION: i32 = 6;
/// Per-IP connection cap (mirrors `kMaxConnectionsPerIp`).
pub const MAX_CONNECTIONS_PER_IP: i32 = 5;
/// Idle ticks before a play session times out (mirrors the C++ 1200).
pub const READ_TIMEOUT_TICKS: u32 = 1200;
/// Inbound packets processed per connection per tick (mirrors the C++
/// `kMaxPacketsPerTick`; more is a rate-limit kick).
pub const MAX_PACKETS_PER_TICK: usize = 50;
/// Accepts per IP per login window (10s at 20 TPS). Vanilla has no login
/// throttle at all; without one, 8 slow logins (~5s HTTP each) wedge the
/// whole verifier pool (`MAX_CONCURRENT_VERIFICATIONS`).
pub const LOGIN_ATTEMPTS_MAX: u32 = 5;
/// Sliding window for the per-IP login throttle, in ticks.
pub const LOGIN_WINDOW_TICKS: u64 = 200;
/// Opaque connection handle.
pub type ConnId = u64;

/// Chunk key for the in-memory maps (low u32 = x, high u32 = z).
/// NOTE: intentionally different from the LevelDB `chunk_key_bytes`
/// layout in persist.rs; the two key spaces never mix.
pub fn chunk_key(x: i32, z: i32) -> i64 {
    (x as u32 as i64) | ((z as u32 as i64) << 32)
}

/// Inverse of [`chunk_key`] (mirrors the C++ decode in the unload pass).
pub fn chunk_of_key(key: i64) -> (i32, i32) {
    ((key & 0xFFFF_FFFF) as u32 as i32, ((key >> 32) & 0xFFFF_FFFF) as u32 as i32)
}

pub const LISTENER_TOKEN: mio::Token = mio::Token(usize::MAX - 1);
pub const WAKER_TOKEN: mio::Token = mio::Token(usize::MAX);

pub struct Server {
    pub settings: Settings,
    pub world: World,
    pub store: ChunkStore,
    pub level_dir: String,
    pub player_dir: String,
    pub ops: HashSet<String>,
    pub banned_players: BTreeSet<String>,
    pub banned_ips: BTreeSet<String>,
    pub ops_path: String,
    pub banned_players_path: String,
    pub banned_ips_path: String,
    pub sessions: HashMap<ConnId, Session>,
    pub players: HashMap<EntityId, ConnId>,
    pub players_by_chunk: HashMap<i64, HashSet<EntityId>>,
    pub next_conn: ConnId,
    pub ip_count: HashMap<String, i32>,
    /// Per-IP login throttle state: window-start tick + accepts in window.
    /// Pruned periodically in `tick`; see `LOGIN_ATTEMPTS_MAX`.
    pub login_attempts: HashMap<String, (u64, u32)>,
    pub tick_count: u64,
    pub console: Vec<String>,
    pub running: bool,
    pub poll: mio::Poll,
    pub events: mio::Events,
    pub waker: std::sync::Arc<mio::Waker>,
    pub listener: Option<mio::net::TcpListener>,
    pub chunk_worker: Option<chunk_worker::ChunkGenWorker>,
    pub pending_chunk_gens: HashSet<(i32, i32)>,
    pub tracker_scratch: TrackerScratch,
    pub cids_scratch: Vec<ConnId>,
    pub health_scratch: Vec<(ConnId, i8)>,
    pub sent_tiles_scratch: HashSet<(i32, i32, i32)>,
    pub store_missing: HashSet<(i32, i32)>,
    pub chunks_generated_this_tick: i32,
}

#[derive(Default)]
pub struct TrackerScratch {
    pub live_ids: Vec<EntityId>,
    pub gone_ids: Vec<EntityId>,
    pub observers: Vec<crate::tracker::Observer>,
    pub tracked: Vec<crate::tracker::TrackedEntity>,
    pub chunks: HashMap<EntityId, (i32, i32)>,
    pub out: Vec<crate::tracker::Outbox>,
}
