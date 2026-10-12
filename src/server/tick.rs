//! The 20 TPS tick: event drains, tracker pass, world tick.
//! Split out of `server.rs`; behavior unchanged.

use crate::entity::table::{Entity, EntityId};
use crate::server::sessions::{Session, SessionState};
use crate::server::{ChunkMapKey, ConnId, Server, TrackerScratch, chunk_map_key};
use crate::server_constants::TICKS_PER_SECOND;
use crate::session::{pkt_block_change, pkt_explosion, pkt_health, pkt_time};
use crate::tracker::{Observer, TrackedEntity};
use crate::world::World;
use std::collections::{HashMap, HashSet};

impl Server {
    /// Tracker pass (mirrors `EntityTracker::tick`): destroy packets for
    /// retired entries (dead or vanished rows, e.g. killed mobs that used
    /// to hang client-side), then per-entity updates for the live ones.
    /// Runs once per dimension; observers only ever see their own
    /// dimension (the `sent` gate carries the dimension key).
    fn tracker_tick(&mut self) {
        let dim = self.world.dimension;
        self.tracker_tick_dim(dim);
        if self.hell.is_some() {
            self.tracker_tick_dim(-1);
        }
    }

    fn tracker_tick_dim(&mut self, dim: i32) {
        let mut scratch = std::mem::take(&mut self.tracker_scratch);
        if dim == -1 && self.hell.is_some() {
            if let Some(hell) = self.hell.as_mut() {
                Self::tracker_tick_world(
                    dim,
                    hell,
                    &self.sessions,
                    &self.players,
                    self.settings.view_distance,
                    &mut scratch,
                );
            }
        } else if dim == self.world.dimension {
            Self::tracker_tick_world(
                dim,
                &mut self.world,
                &self.sessions,
                &self.players,
                self.settings.view_distance,
                &mut scratch,
            );
        }
        self.tracker_scratch = scratch;
    }

    #[allow(clippy::too_many_arguments)]
    fn tracker_tick_world(
        dim: i32,
        world: &mut World,
        sessions: &HashMap<ConnId, Session>,
        players: &HashMap<EntityId, ConnId>,
        view_distance: i32,
        scratch: &mut TrackerScratch,
    ) {
        world.entities.collect_alive_ids(&mut scratch.live_ids);
        scratch.live_ids.sort_unstable();

        // Retire vanished entries (and dead players) with destroy packets
        // (mirrors `EntityTrackerEntry.func_604_a`). Dead mobs/animals stay in
        // `world.entities` for their 20-tick death animation (`death_time < 20`)
        // before `purge_dead` removes them, so `DestroyEntity` is sent after
        // the corpse animation rather than collapsing it on the death tick.
        scratch.gone_ids.clear();
        scratch.out.clear();
        for id in world.tracker.iter_tracked_ids() {
            match world.entities.get(id) {
                None => scratch.gone_ids.push(id),
                Some(Entity::Player(p)) if p.living.body.dead => scratch.gone_ids.push(id),
                Some(Entity::Mob(m))
                    if m.living.body.dead && (m.living.health > 0 || m.living.death_time >= 20) =>
                {
                    scratch.gone_ids.push(id)
                }
                Some(Entity::Animal(a))
                    if a.living.body.dead && (a.living.health > 0 || a.living.death_time >= 20) =>
                {
                    scratch.gone_ids.push(id)
                }
                Some(e) if e.body().dead && !matches!(e, Entity::Mob(_) | Entity::Animal(_)) => {
                    scratch.gone_ids.push(id)
                }
                Some(_) => {}
            }
        }
        for &id in &scratch.gone_ids {
            world.tracker.remove(id, &mut scratch.out);
        }

        scratch.observers.clear();
        for &id in &scratch.live_ids {
            if let Some(Entity::Player(p)) = world.entities.get(id) {
                if players.contains_key(&id) {
                    scratch.observers.push(Observer {
                        id,
                        pos: p.living.body.pos,
                        // The dead see nothing (mirrors the C++ isDead skip).
                        alive: !p.living.body.dead,
                    });
                }
            }
        }

        // Snapshot rows into owned tracked structs (params mirror C++).
        scratch.tracked.clear();
        scratch.chunks.clear();
        for &id in &scratch.live_ids {
            let held = match world.entities.get(id) {
                Some(Entity::Player(_)) => {
                    match players.get(&id).and_then(|cid| sessions.get(cid)) {
                        Some(Session { state: SessionState::Play(play, _), .. }) => {
                            play.held_id as i16
                        }
                        _ => 0,
                    }
                }
                _ => 0,
            };
            let te = match world.entities.get(id) {
                // Java EntityTracker: player 512/2, item 64/20, arrow 64/5,
                // boat 160/5, mobs+animals 160/3 — each clamped to the view
                // distance in blocks (EntityTracker.java:42-44).
                Some(e @ Entity::Player(_)) => TrackedEntity::from_entity(
                    e, held, 512.min(view_distance * 16), 2, false,
                ),
                Some(e @ Entity::Item(_)) => TrackedEntity::from_entity(
                    e, 0, 64.min(view_distance * 16), 20, true,
                ),
                Some(e @ Entity::Arrow(_)) => TrackedEntity::from_entity(
                    e, 0, 64.min(view_distance * 16), 5, true,
                ),
                // Thrown/cast shots ride the arrow channel (64/5+velocity,
                // like the vanilla tracker branches for snowball/fish).
                // Fireballs have no vanilla branch; visibility beats
                // invisibility (documented in entity/table.rs).
                Some(e @ Entity::Snowball(_))
                | Some(e @ Entity::FishHook(_))
                | Some(e @ Entity::Fireball(_)) => TrackedEntity::from_entity(
                    e, 0, 64.min(view_distance * 16), 5, true,
                ),
                Some(e @ Entity::Boat(_)) => TrackedEntity::from_entity(
                    e, 0, 160.min(view_distance * 16), 5, true,
                ),
                // Minecarts (160/5) and primed TNT (160/10) mirror the
                // vanilla `EntityTracker` registrations byte-for-byte.
                Some(e @ Entity::Minecart(_)) => TrackedEntity::from_entity(
                    e, 0, 160.min(view_distance * 16), 5, true,
                ),
                Some(e @ Entity::Tnt(_)) => TrackedEntity::from_entity(
                    e, 0, 160.min(view_distance * 16), 10, true,
                ),
                Some(e @ Entity::Mob(_)) | Some(e @ Entity::Animal(_)) => {
                    TrackedEntity::from_entity(
                        e, 0, 160.min(view_distance * 16), 3, false,
                    )
                }
                Some(Entity::Falling(_)) | None => None,
            };
            if let Some(te) = te {
                scratch.tracked.push(te);
            }
        }
        let mut observer_sent: Vec<(EntityId, &std::collections::HashSet<crate::server::ChunkMapKey>)> =
            Vec::with_capacity(scratch.observers.len());
        for o in &scratch.observers {
            if let Some(cid) = players.get(&o.id) {
                if let Some(Session { state: SessionState::Play(_, stream), .. }) = sessions.get(cid) {
                    if stream.dim == dim {
                        observer_sent.push((o.id, &stream.sent));
                    }
                }
            }
        }
        for te in &scratch.tracked {
            world.tracker.add(te);
            let ecx = (te.pos[0].floor() as i32).div_euclid(16);
            let ecz = (te.pos[2].floor() as i32).div_euclid(16);
            let chunk_key = chunk_map_key(dim, ecx, ecz);
            world.tracker.tick_entity(
                te,
                &scratch.observers,
                &|observer, _entity| {
                    for &(oid, sent) in &observer_sent {
                        if oid == observer {
                            return sent.contains(&chunk_key);
                        }
                    }
                    false
                },
                &mut scratch.out,
            );
        }
        for o in scratch.out.drain(..) {
            if let Some(cid) = players.get(&o.to).copied() {
                if let Some(sess) = sessions.get(&cid) {
                    sess.conn.queue(&o.bytes);
                }
            }
        }
    }
}

impl Server {
    /// Ship pickup feedback (mirrors `EntityPlayerMP.func_163_c`):
    /// `Packet22Collect` to the item's watchers and picker on full takes
    /// (the client plays `random.pop`, flies the item over and removes
    /// it), then `Packet17AddToInventory` so the client merges the stack
    /// itself — no full `Packet5` resync, which fought the client's
    /// 1-second bank sync and erased in-flight GUI edits. Drained before
    /// `tracker_tick` so Collect precedes the destroy for the dead item
    /// row. Runs per dimension.
    fn drain_pickup_events(&mut self) {
        let dim = self.world.dimension;
        self.drain_pickup_events_dim(dim);
        if self.hell.is_some() {
            self.drain_pickup_events_dim(-1);
        }
    }

    fn drain_pickup_events_dim(&mut self, dim: i32) {
        let world: &mut World = if dim == -1 && self.hell.is_some() {
            let Some(h) = self.hell.as_mut() else { return };
            h
        } else if dim == self.world.dimension {
            &mut self.world
        } else {
            return;
        };
        if world.item_pickups.is_empty() {
            return;
        }
        self.tracker_scratch.out.clear();
        for i in 0..world.item_pickups.len() {
            let ev = world.item_pickups[i];
            // Collect animation only when the row was fully consumed; a
            // partial merge leaves the item in the world (vanilla sends
            // no packet for partials at all, the delta arrives below).
            if ev.full {
                world.tracker.collect_fx(ev.item, ev.player, &mut self.tracker_scratch.out);
            }
        }
        for o in self.tracker_scratch.out.drain(..) {
            if let Some(cid) = self.players.get(&o.to).copied() {
                if let Some(sess) = self.sessions.get(&cid) {
                    sess.conn.queue(&o.bytes);
                }
            }
        }
        // `Packet17AddToInventory` (vanilla `EntityPlayerMP.func_163_c`):
        // the client merges the stack into the first partial/empty slot
        // itself, without a full `Packet5` bank overwrite. A resync here
        // fought the client's 1-second bank sync and erased in-flight GUI
        // edits (the "resync sandwich" item loss).
        for i in 0..world.item_pickups.len() {
            let ev = world.item_pickups[i];
            if let Some(cid) = self.players.get(&ev.player).copied() {
                if let Some(Session { state: SessionState::Play(play, _), .. }) =
                    self.sessions.get_mut(&cid)
                {
                    play.outbox.push(crate::session_packets::pkt_add_to_inventory(
                        ev.item_id as i16,
                        ev.count.clamp(1, 127) as i8,
                        ev.damage.clamp(0, 32767) as i16,
                    ));
                }
            }
        }
        world.item_pickups.clear();
    }

    /// Ship death animations, status changes, velocity impulses, and
    /// explosions (`Packet60`, `WorldServer.java:92-96` within 64 blocks):
    /// drained before `tracker_tick` so animations precede destroy packets
    /// for the same tick's kills. Runs per dimension (explosions only
    /// reach players standing in the same dimension).
    fn drain_death_events(&mut self) {
        let dim = self.world.dimension;
        self.drain_death_events_dim(dim);
        if self.hell.is_some() {
            self.drain_death_events_dim(-1);
        }
    }

    fn drain_death_events_dim(&mut self, dim: i32) {
        let world: &mut World = if dim == -1 && self.hell.is_some() {
            let Some(h) = self.hell.as_mut() else { return };
            h
        } else if dim == self.world.dimension {
            &mut self.world
        } else {
            return;
        };
        if !world.death_events.is_empty()
            || !world.status_events.is_empty()
            || !world.velocity_events.is_empty()
        {
            self.tracker_scratch.out.clear();
            for &id in &world.death_events {
                world.tracker.death_fx(id, &mut self.tracker_scratch.out);
            }
            for &(id, status) in &world.status_events {
                world.tracker.status_fx(id, status, &mut self.tracker_scratch.out);
            }
            for &(id, motion) in &world.velocity_events {
                world.tracker.velocity_fx(id, motion, &mut self.tracker_scratch.out);
            }
            for o in self.tracker_scratch.out.drain(..) {
                if let Some(cid) = self.players.get(&o.to).copied() {
                    if let Some(sess) = self.sessions.get(&cid) {
                        sess.conn.queue(&o.bytes);
                    }
                }
            }
            world.death_events.clear();
            world.status_events.clear();
            world.velocity_events.clear();
        }
        for (ex, ey, ez, radius, cells) in world.explosion_events.drain(..) {
            let pkt = pkt_explosion(ex, ey, ez, radius, &cells);
            for (&pid, &cid) in &self.players {
                let in_range = match world.entities.get(pid) {
                    Some(Entity::Player(p)) => {
                        let dx = ex - p.living.body.pos[0];
                        let dy = ey - p.living.body.pos[1];
                        let dz = ez - p.living.body.pos[2];
                        dx * dx + dy * dy + dz * dz < 4096.0
                    }
                    _ => false,
                };
                if in_range {
                    if let Some(sess) = self.sessions.get(&cid) {
                        sess.conn.queue(&pkt);
                    }
                }
            }
        }
    }

    /// Health watch (mirrors the `Packet8` send in
    /// `EntityPlayerMP.func_175_i`): push 0x08 whenever a player's health
    /// changed since the last send, so fall, mob, burn and drown damage
    /// all reach the HUD — previously only login/respawn/eat synced it.
    /// Runs per dimension (rows live in exactly one world).
    fn push_health_changes(&mut self) {
        let dim = self.world.dimension;
        self.push_health_changes_dim(dim);
        if self.hell.is_some() {
            self.push_health_changes_dim(-1);
        }
    }

    fn push_health_changes_dim(&mut self, dim: i32) {
        let world: &World = if dim == -1 && self.hell.is_some() {
            let Some(h) = self.hell.as_ref() else { return };
            h
        } else if dim == self.world.dimension {
            &self.world
        } else {
            return;
        };
        self.health_scratch.clear();
        for (eid, cid) in &self.players {
            let health = match world.entities.get(*eid) {
                Some(Entity::Player(p)) => p.living.health as i8,
                _ => continue,
            };
            let last = match self.sessions.get(cid) {
                Some(Session { state: SessionState::Play(play, _), .. }) => play.last_health,
                _ => continue,
            };
            if health != last {
                self.health_scratch.push((*cid, health));
            }
        }
        for i in 0..self.health_scratch.len() {
            let (cid, health) = self.health_scratch[i];
            if let Some(sess) = self.sessions.get_mut(&cid) {
                sess.conn.queue(&pkt_health(health));
                if let SessionState::Play(play, _) = &mut sess.state {
                    play.last_health = health;
                }
            }
        }
    }

    /// Ship queued block changes to chunk-loaded players (one packet
    /// per changed cell per tick).
    /// For furnaces (`61`/`62`), precede `Packet53BlockChange` with per-chunk
    /// subchunk packets covering the 13-block light radius so `Chunk.func_1004_a`
    /// on the client updates the block ID, facing metadata, and surrounding
    /// blocklight/skylight maps without calling `BlockFurnace.onBlockAdded`
    /// (which would replace `GuiFurnace.field_978_j` and clobber facing metadata).
    /// Runs per dimension.
    fn drain_block_updates(&mut self) {
        let dim = self.world.dimension;
        self.drain_block_updates_dim(dim);
        if self.hell.is_some() {
            self.drain_block_updates_dim(-1);
        }
    }

    fn drain_block_updates_dim(&mut self, dim: i32) {
        let world: &mut World = if dim == -1 && self.hell.is_some() {
            let Some(h) = self.hell.as_mut() else { return };
            h
        } else if dim == self.world.dimension {
            &mut self.world
        } else {
            return;
        };
        world.refresh_light();
        let players_by_chunk = &self.players_by_chunk;
        const MAX_BLOCK_UPDATES_PER_TICK: usize = 1024;
        for [x, y, z] in world.take_block_updates_budgeted(MAX_BLOCK_UPDATES_PER_TICK) {
            let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
            if !Self::has_conn_in_chunk(players_by_chunk, dim, cx, cz) {
                continue;
            }
            let id = world.get_block_id(x, y, z);
            let meta = world.get_block_meta(x, y, z);
            if matches!(id, 61 | 62) {
                for ((lcx, lcz), sc) in
                    crate::session_packets::pkt_subchunk_furnace_light_filtered(
                        world,
                        x,
                        y,
                        z,
                        |cx, cz| Self::has_conn_in_chunk(players_by_chunk, dim, cx, cz),
                    )
                {
                    Self::for_each_conn_in_chunk(
                        &self.players_by_chunk,
                        &self.players,
                        chunk_map_key(dim, lcx, lcz),
                        |cid| {
                            if let Some(sess) = self.sessions.get(&cid) {
                                sess.conn.queue(&sc);
                            }
                        },
                    );
                }
            }
            let subchunk = if matches!(id, 61 | 62) {
                crate::session_packets::pkt_subchunk_block(world, x, y, z)
            } else {
                None
            };
            let furnace_tile = if matches!(id, 61 | 62) {
                world
                    .tiles
                    .get(&(x, y, z))
                    .map(|t| crate::session_packets::tile_packet(x, y, z, t))
            } else {
                None
            };
            if furnace_tile.is_some() {
                self.sent_tiles_scratch.insert((dim, x, y, z));
            }
            let bytes = pkt_block_change(x, y, z, id, meta);
            Self::for_each_conn_in_chunk(
                &self.players_by_chunk,
                &self.players,
                chunk_map_key(dim, cx, cz),
                |cid| {
                    if let Some(sess) = self.sessions.get(&cid) {
                        sess.conn.queue(&bytes);
                        if let Some(ref tp) = furnace_tile {
                            sess.conn.queue(tp);
                        }
                        if let Some(ref sc) = subchunk {
                            sess.conn.queue(sc);
                        }
                    }
                },
            );
        }
    }

    fn has_conn_in_chunk(
        players_by_chunk: &HashMap<ChunkMapKey, HashSet<EntityId>>,
        dim: i32,
        cx: i32,
        cz: i32,
    ) -> bool {
        players_by_chunk
            .get(&chunk_map_key(dim, cx, cz))
            .is_some_and(|s| !s.is_empty())
    }

    /// Ship queued tile-entity updates (`Packet59ComplexEntity`) to chunk-loaded players.
    /// Runs per dimension.
    fn drain_tile_updates(&mut self) {
        let dim = self.world.dimension;
        self.drain_tile_updates_dim(dim);
        if self.hell.is_some() {
            self.drain_tile_updates_dim(-1);
        }
    }

    fn drain_tile_updates_dim(&mut self, dim: i32) {
        let world: &mut World = if dim == -1 && self.hell.is_some() {
            let Some(h) = self.hell.as_mut() else { return };
            h
        } else if dim == self.world.dimension {
            &mut self.world
        } else {
            return;
        };
        for [x, y, z] in world.take_tile_updates() {
            if !self.sent_tiles_scratch.insert((dim, x, y, z)) {
                continue;
            }
            if let Some(tile) = world.tiles.get(&(x, y, z)).copied() {
                let bytes = crate::session_packets::tile_packet(x, y, z, &tile);
                // Identical payload => watchers already have this exact
                // state; skip the re-broadcast entirely. Cooking furnaces
                // keep changing every tick (CookTime), so they stay
                // dynamic — idle repeats (re-marked chests, furnace flips
                // that altered nothing) cost nothing now.
                let key = (dim, x, y, z);
                if let Some(prev) = self.last_tile_packets.get(&key) {
                    if *prev == bytes {
                        continue;
                    }
                }
                self.last_tile_packets.insert(key, bytes.clone());
                let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
                Self::for_each_conn_in_chunk(
                    &self.players_by_chunk,
                    &self.players,
                    chunk_map_key(dim, cx, cz),
                    |cid| {
                        if let Some(sess) = self.sessions.get(&cid) {
                            sess.conn.queue(&bytes);
                        }
                    },
                );
            }
        }
    }

    /// One server tick (mirrors `serverTick`).
    pub fn tick(&mut self) {
        if !self.running {
            return;
        }
        self.chunks_generated_this_tick = 0;
        self.chunks_loaded_this_tick = 0;
        self.poll_network(std::time::Duration::ZERO);
        self.tick_count += 1;
        if self.tick_count.is_multiple_of(crate::server::LOGIN_WINDOW_TICKS) {
            self.prune_login_attempts();
        }

        // Process incoming client packets and session events first (mirrors MinecraftServer func_715_a).
        self.cids_scratch.clear();
        for &cid in self.sessions.keys() {
            self.cids_scratch.push(cid);
        }
        for i in 0..self.cids_scratch.len() {
            let cid = self.cids_scratch[i];
            self.pump_one(cid);
        }

        if self.tick_count.is_multiple_of(TICKS_PER_SECOND as u64) {
            let bytes = pkt_time(self.world.time);
            for &cid in &self.cids_scratch {
                if self.is_play(cid) {
                    if let Some(sess) = self.sessions.get(&cid) {
                        sess.conn.queue(&bytes);
                    }
                }
            }
        }
        self.world.tick_world();
        // Hell ticks on the same clock (single time source like vanilla's
        // one World; skylight inside is fixed, see calculate fn).
        if let Some(hell) = self.hell.as_mut() {
            hell.time = self.world.time;
            hell.tick_world();
        }

        self.flush_unloaded_chunks();
        self.drain_pickup_events();
        self.drain_death_events();
        self.push_health_changes();
        if self.settings.auto_save_interval > 0
            && self.tick_count.is_multiple_of(self.settings.auto_save_interval as u64)
            && !self.auto_save_active
        {
            self.save_players();
            let _ = self.world.save_level_to(&self.level_dir);
            self.begin_autosave();
        }
        if self.auto_save_active {
            self.step_autosave(8);
        }
        self.tracker_tick();
        self.sent_tiles_scratch.clear();
        self.drain_block_updates();
        self.drain_tile_updates();
        self.flush_all_sessions();
        self.pump_closing_sessions();
        self.poll_network(std::time::Duration::ZERO);
        let lines = std::mem::take(&mut self.console);
        for line in lines {
            self.handle_console(&line);
        }
    }
}
