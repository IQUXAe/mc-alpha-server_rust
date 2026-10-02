//! Player/world persistence and shutdown.
//! Split out of `server.rs`; behavior unchanged.

use crate::entity::table::{Entity, EntityId};
use crate::server::sessions::{Session, SessionState};
use crate::server::{ConnId, Server};
use crate::server_log as log;
use crate::session::pkt_kick;

impl Server {
    /// Owning world for an entity id (portal travelers keep their id,
    /// only the world moves; unknown ids read as overworld).
    fn world_dim_of(&self, eid: EntityId) -> i32 {
        if self.hell.is_some() {
            if self.world.entities.get(eid).is_some() {
                0
            } else {
                -1
            }
        } else {
            self.world.dimension as i32
        }
    }

    /// Log a player out (mirrors `playerLoggedOut`): unmount both ways,
    /// tracker destroy, chunk-mapping cleanup, held snapshot, player
    /// save, death mark; optional leave chat; world flush when empty.
    /// Dim-aware: operates on the world that owns the row.
    pub(crate) fn logout(&mut self, eid: EntityId, held: i32, leave: bool) {
        let dim = self.world_dim_of(eid);
        // Phase 1: world-row reads/mutations (borrow ends at scope end).
        let username = {
            let world = if dim == -1 && self.hell.is_some() {
                match self.hell.as_mut() {
                    Some(h) => h,
                    None => {
                        self.players.remove(&eid);
                        return;
                    }
                }
            } else {
                &mut self.world
            };
            let username = match world.entities.get(eid) {
                Some(Entity::Player(p)) => p.username.clone(),
                _ => {
                    self.players.remove(&eid);
                    return;
                }
            };
            let (riding, ridden_by) = match world.entities.get(eid) {
                Some(e) => (e.body().riding, e.body().ridden_by),
                None => (crate::entity::table::NO_ENTITY, crate::entity::table::NO_ENTITY),
            };
            if riding != crate::entity::table::NO_ENTITY {
                world.entities.mount(eid, None);
            }
            if ridden_by != crate::entity::table::NO_ENTITY {
                world.entities.mount(ridden_by, None);
            }
            let mut out = Vec::new();
            world.tracker.remove(eid, &mut out);
            for set in self.players_by_chunk.values_mut() {
                set.remove(&eid);
            }
            self.players_by_chunk.retain(|_, set| !set.is_empty());
            if let Some(Entity::Player(p)) = world.entities.get_mut(eid) {
                p.held_item_id = held;
                p.living.body.dead = true;
            }
            if !world.save_player_to(&self.player_dir, eid) {
                log::warning(&format!("Failed to save player data for {username}"));
            }
            // Drop the row explicitly: dead players are spared by the tick
            // purge (respawn needs them), so logout must clean up itself.
            world.entities.remove(eid);
            (username, out)
        };
        let (username, mut out) = username;
        // Phase 2: fan-out without any world borrowed.
        self.route_outbox_drain(&mut out);
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
    /// Dim-aware (rows live in exactly one world).
    pub(crate) fn save_player(&mut self, eid: EntityId, held: i32) {
        if self.world_dim_of(eid) == -1 && self.hell.is_some() {
            if let Some(hell) = self.hell.as_mut() {
                if let Some(Entity::Player(p)) = hell.entities.get_mut(eid) {
                    p.held_item_id = held;
                }
                hell.save_player_to(&self.player_dir, eid);
            }
            return;
        }
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
        self.flush_unloaded_chunks_limit(32);
    }

    pub(crate) fn flush_unloaded_chunks_limit(&mut self, limit: usize) {
        // Split the budget across dimensions (hardening: one world's
        // eviction wave can't starve the other's).
        let half = (limit / 2).max(1);
        let world = &mut self.world;
        let store = &mut self.store;
        let missing = &mut self.store_missing;
        Self::flush_unloaded_into(world, store, missing, limit);
        if let Some(hell) = self.hell.as_mut() {
            if let Some(hstore) = self.hell_store.as_mut() {
                let hmissing = &mut self.store_missing_hell;
                Self::flush_unloaded_into(hell, hstore, hmissing, half);
            }
        }
    }

    /// Flush one world's staged unloads into its store (failed encodes
    /// keep the chunk staged like vanilla's `isModified` retry).
    fn flush_unloaded_into(
        world: &mut crate::world::World,
        store: &mut crate::persist::ChunkStore,
        missing: &mut std::collections::HashSet<(i32, i32)>,
        limit: usize,
    ) {
        if world.unloaded.is_empty() {
            return;
        }
        let keys: Vec<(i32, i32)> = world.unloaded.keys().copied().take(limit).collect();
        for (cx, cz) in keys {
            let dirty = world
                .unloaded
                .get(&(cx, cz))
                .map(|c| c.is_modified)
                .unwrap_or(false);
            if !dirty {
                world.unloaded.remove(&(cx, cz));
                world.tiles.remove_chunk(cx, cz);
                continue;
            }
            match crate::persist::encode_chunk_blob(world, cx, cz, true) {
                None => {
                    log::warning(&format!("Failed to encode chunk {cx},{cz}; keeping staged"));
                }
                Some(blob) => match store.put_chunk(cx, cz, &blob) {
                    Ok(()) => {
                        missing.remove(&(cx, cz));
                        world.unloaded.remove(&(cx, cz));
                        // Tiles ride in the blob just written; drop the
                        // memory copy like vanilla unloads its TileEntities
                        // (reload restores from disk, furnaces stop ticking
                        // while unloaded).
                        world.tiles.remove_chunk(cx, cz);
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
    /// success; live persistent entities pin their chunks, staged
    /// unloads are flushed to LevelDB and freed from memory).
    /// Hell chunks go to the DIM-1 store; `level.dat` is written once
    /// from the overworld clock.
    pub(crate) fn save_world(&mut self) {
        if !self.world.save_level_to(&self.level_dir) {
            log::warning("Failed to save level.dat");
        }
        self.flush_unloaded_chunks();
        Self::save_world_into(&mut self.world, &mut self.store, &mut self.store_missing);
        if let Some(hell) = self.hell.as_mut() {
            if let Some(hstore) = self.hell_store.as_mut() {
                let hmissing = &mut self.store_missing_hell;
                Self::save_world_into(hell, hstore, hmissing);
            }
        }
        if let Err(e) = self.store.flush() {
            log::warning(&format!("Failed to flush chunk store to disk: {e}"));
        }
        if let Some(hs) = self.hell_store.as_mut() {
            if let Err(e) = hs.flush() {
                log::warning(&format!("Failed to flush hell chunk store to disk: {e}"));
            }
        }
        log::info("Saved level.dat and flushed modified chunks to disk.");
    }

    /// Save one world's live-entity-pinned dirty chunks into its store.
    fn save_world_into(
        world: &mut crate::world::World,
        store: &mut crate::persist::ChunkStore,
        missing: &mut std::collections::HashSet<(i32, i32)>,
    ) {
        for eid in world.entities.alive_ids() {
            if let Some(
                Entity::Boat(_)
                | Entity::Minecart(_)
                | Entity::Item(_)
                | Entity::Animal(_)
                | Entity::Mob(_)
                | Entity::Arrow(_)
                | Entity::Falling(_),
            ) = world.entities.get(eid)
            {
                if let Some(e) = world.entities.get(eid) {
                    let (cx, cz) = (
                        (e.body().pos[0].floor() as i32).div_euclid(16),
                        (e.body().pos[2].floor() as i32).div_euclid(16),
                    );
                    world.mark_chunk_modified(cx, cz);
                }
            }
        }
        let coords = world.loaded_chunk_coords();
        let mut saved = 0;
        let mut skipped = 0;
        for (cx, cz) in coords {
            let dirty = world
                .chunk_ref(cx, cz)
                .map(|c| c.is_modified)
                .unwrap_or(false);
            if !dirty {
                skipped += 1;
                continue;
            }
            if world.save_chunk_to(store, cx, cz) {
                missing.remove(&(cx, cz));
                if let Some(c) = world.chunk_ref_mut(cx, cz) {
                    c.clear_modified();
                }
                saved += 1;
            } else {
                log::warning(&format!("Failed to save chunk {cx},{cz}; keeping dirty"));
            }
        }
        log::info(&format!(
            "Saved dim {} chunks to disk ({saved} saved, {skipped} clean skipped).",
            world.dimension,
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
        let mut attempts = 0;
        while !self.world.unloaded.is_empty() && attempts < 5 {
            let before = self.world.unloaded.len();
            self.flush_unloaded_chunks_limit(usize::MAX);
            if self.world.unloaded.len() >= before {
                break;
            }
            attempts += 1;
        }
        self.save_world();
        if let Err(e) = self.store.flush() {
            log::warning(&format!("Failed to flush chunk store on shutdown: {e}"));
        }
        log::info("Server stopped.");
    }
}
