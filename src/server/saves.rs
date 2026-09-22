//! Player/world persistence and shutdown.
//! Split out of `server.rs`; behavior unchanged.

use crate::entity::table::{Entity, EntityId};
use crate::server::sessions::{Session, SessionState};
use crate::server::{ConnId, Server};
use crate::server_log as log;
use crate::session::pkt_kick;

impl Server {
    /// Log a player out (mirrors `playerLoggedOut`): unmount both ways,
    /// tracker destroy, chunk-mapping cleanup, held snapshot, player
    /// save, death mark; optional leave chat; world flush when empty.
    pub(crate) fn logout(&mut self, eid: EntityId, held: i32, leave: bool) {
        let username = match self.world.entities.get(eid) {
            Some(Entity::Player(p)) => p.username.clone(),
            _ => {
                self.players.remove(&eid);
                return;
            }
        };
        let (riding, ridden_by) = match self.world.entities.get(eid) {
            Some(e) => (e.body().riding, e.body().ridden_by),
            None => (crate::entity::table::NO_ENTITY, crate::entity::table::NO_ENTITY),
        };
        if riding != crate::entity::table::NO_ENTITY {
            self.world.entities.mount(eid, None);
        }
        if ridden_by != crate::entity::table::NO_ENTITY {
            self.world.entities.mount(ridden_by, None);
        }
        let mut out = Vec::new();
        self.world.tracker.remove(eid, &mut out);
        self.route_outbox(out);
        for set in self.players_by_chunk.values_mut() {
            set.remove(&eid);
        }
        self.players_by_chunk.retain(|_, set| !set.is_empty());
        if let Some(Entity::Player(p)) = self.world.entities.get_mut(eid) {
            p.held_item_id = held;
            p.living.body.dead = true;
        }
        if !self.world.save_player_to(&self.player_dir, eid) {
            log::warning(&format!("Failed to save player data for {username}"));
        }
        // Drop the row explicitly: dead players are spared by the tick
        // purge (respawn needs them), so logout must clean up itself.
        self.world.entities.remove(eid);
        self.players.remove(&eid);
        if leave {
            self.broadcast_chat(format!("§e{username} left the game."));
        } else {
            log::info(&format!("{username} left the game."));
        }
        if self.players.is_empty() {
            log::info("Last player left; flushing world state to disk.");
            self.save_world();
        }
    }

    /// Save one player row (held snapshot folded in like `syncHeldItems`).
    pub(crate) fn save_player(&mut self, eid: EntityId, held: i32) {
        if let Some(Entity::Player(p)) = self.world.entities.get_mut(eid) {
            p.held_item_id = held;
        }
        self.world.save_player_to(&self.player_dir, eid);
    }

    /// Save all online players (mirrors `savePlayerStates`).
    pub(crate) fn save_players(&mut self) {
        let pairs: Vec<(EntityId, i32)> = self
            .players
            .iter()
            .map(|(eid, cid)| {
                let held = match self.sessions.get(cid) {
                    Some(Session { state: SessionState::Play(play, _), .. }) => play.held_id,
                    _ => 0,
                };
                (*eid, held)
            })
            .collect();
        for (eid, held) in pairs {
            self.save_player(eid, held);
        }
    }

    /// Flush staged unloaded chunks to LevelDB and free their memory.
    /// A failed encode/put keeps the chunk staged (vanilla retries via
    /// `isModified`) instead of silently dropping player builds.
    pub(crate) fn flush_unloaded_chunks(&mut self) {
        if self.world.unloaded.is_empty() {
            return;
        }
        let keys: Vec<(i32, i32)> = self.world.unloaded.keys().copied().collect();
        for (cx, cz) in keys {
            match crate::persist::encode_chunk_blob(&self.world, cx, cz, true) {
                None => {
                    log::warning(&format!("Failed to encode chunk {cx},{cz}; keeping staged"));
                }
                Some(blob) => match self.store.put_chunk(cx, cz, &blob) {
                    Ok(()) => {
                        self.world.unloaded.remove(&(cx, cz));
                    }
                    Err(e) => {
                        log::warning(&format!(
                            "Failed to save chunk {cx},{cz}: {e}; keeping staged"
                        ));
                    }
                },
            }
        }
    }

    /// Flush level.dat plus modified loaded chunks (mirrors vanilla
    /// `saveWorld`: only `isModified` chunks hit the disk, flag cleared on
    /// success; live boats pin their chunks like the C++ touch-up, staged
    /// unloads are flushed to LevelDB and freed from memory).
    pub(crate) fn save_world(&mut self) {
        if !self.world.save_level_to(&self.level_dir) {
            log::warning("Failed to save level.dat");
        }
        self.flush_unloaded_chunks();
        for eid in self.world.entities.alive_ids() {
            if let Some(Entity::Boat(b)) = self.world.entities.get(eid) {
                let (cx, cz) = (
                    (b.body.pos[0].floor() as i32).div_euclid(16),
                    (b.body.pos[2].floor() as i32).div_euclid(16),
                );
                self.world.mark_chunk_modified(cx, cz);
            }
        }
        let coords = self.world.loaded_chunk_coords();
        let mut saved = 0;
        let mut skipped = 0;
        for (cx, cz) in coords {
            let dirty = self
                .world
                .chunk_ref(cx, cz)
                .map(|c| c.is_modified)
                .unwrap_or(false);
            if !dirty {
                skipped += 1;
                continue;
            }
            if self.world.save_chunk_to(&mut self.store, cx, cz) {
                if let Some(c) = self.world.chunk_ref_mut(cx, cz) {
                    c.clear_modified();
                }
                saved += 1;
            } else {
                log::warning(&format!("Failed to save chunk {cx},{cz}; keeping dirty"));
            }
        }
        if let Err(e) = self.store.flush() {
            log::warning(&format!("Failed to flush chunk store to disk: {e}"));
        }
        log::info(&format!(
            "Saved level.dat and flushed {saved} modified chunks to disk ({skipped} clean skipped)."
        ));
    }

    /// Graceful shutdown (mirrors the `run` tail): kick everyone with
    /// the players still listed (like C++, no leave chat here), save
    /// players with held snapshots, then flush the world.
    pub fn shutdown(&mut self) {
        self.running = false;
        log::info("Stopping server");
        let cids: Vec<ConnId> = self.sessions.keys().copied().collect();
        for cid in cids {
            if let Some(mut sess) = self.sessions.remove(&cid) {
                let held = match &mut sess.state {
                    SessionState::Play(play, _) => {
                        play.outbox.push(pkt_kick("Server shutting down"));
                        play.held_id
                    }
                    SessionState::Login(_) => 0,
                };
                sess.flush();
                if let SessionState::Play(play, _) = &sess.state {
                    self.save_player(play.player, held);
                }
                self.remove_session(cid, sess);
            }
        }
        self.save_world();
        if let Err(e) = self.store.flush() {
            log::warning(&format!("Failed to flush chunk store on shutdown: {e}"));
        }
        log::info("Server stopped.");
    }
}
