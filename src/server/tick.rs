//! The 20 TPS tick: event drains, tracker pass, world tick.
//! Split out of `server.rs`; behavior unchanged.

use crate::entity::table::Entity;
use crate::server::sessions::{Session, SessionState};
use crate::server::{ConnId, Server, chunk_key};
use crate::server_constants::TICKS_PER_SECOND;
use crate::session::{pkt_block_change, pkt_explosion, pkt_health, pkt_time};
use crate::tracker::{Observer, TrackedEntity};

impl Server {
    /// Tracker pass (mirrors `EntityTracker::tick`): destroy packets for
    /// retired entries (dead or vanished rows, e.g. killed mobs that used
    /// to hang client-side), then per-entity updates for the live ones.
    fn tracker_tick(&mut self) {
        let mut scratch = std::mem::take(&mut self.tracker_scratch);
        self.world.entities.collect_alive_ids(&mut scratch.live_ids);
        scratch.live_ids.sort_unstable();

        // Retire vanished entries (and dead players) with destroy packets
        // (mirrors `EntityTrackerEntry.func_604_a`). Dead mobs/animals stay in
        // `world.entities` for their 20-tick death animation (`death_time < 20`)
        // before `purge_dead` removes them, so `DestroyEntity` is sent after
        // the corpse animation rather than collapsing it on the death tick.
        scratch.gone_ids.clear();
        scratch.out.clear();
        for id in self.world.tracker.tracked_ids() {
            match self.world.entities.get(id) {
                None => scratch.gone_ids.push(id),
                Some(Entity::Player(p)) if p.living.body.dead => scratch.gone_ids.push(id),
                Some(_) => {}
            }
        }
        for &id in &scratch.gone_ids {
            self.world.tracker.remove(id, &mut scratch.out);
        }

        scratch.observers.clear();
        for &id in &scratch.live_ids {
            if let Some(Entity::Player(p)) = self.world.entities.get(id) {
                if self.players.contains_key(&id) {
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
            let held = match self.world.entities.get(id) {
                Some(Entity::Player(_)) => {
                    match self.players.get(&id).and_then(|cid| self.sessions.get(cid)) {
                        Some(Session { state: SessionState::Play(play, _), .. }) => {
                            play.held_id as i16
                        }
                        _ => 0,
                    }
                }
                _ => 0,
            };
            let te = match self.world.entities.get(id) {
                // Java EntityTracker: player 512/2, item 64/20, arrow 64/5,
                // boat 160/5, mobs+animals 160/3 — each clamped to the view
                // distance in blocks (EntityTracker.java:42-44).
                Some(e @ Entity::Player(_)) => TrackedEntity::from_entity(
                    e, held, 512.min(self.settings.view_distance * 16), 2, false,
                ),
                Some(e @ Entity::Item(_)) => TrackedEntity::from_entity(
                    e, 0, 64.min(self.settings.view_distance * 16), 20, true,
                ),
                Some(e @ Entity::Arrow(_)) => TrackedEntity::from_entity(
                    e, 0, 64.min(self.settings.view_distance * 16), 5, true,
                ),
                Some(e @ Entity::Boat(_)) => TrackedEntity::from_entity(
                    e, 0, 160.min(self.settings.view_distance * 16), 5, true,
                ),
                Some(e @ Entity::Mob(_)) | Some(e @ Entity::Animal(_)) => {
                    TrackedEntity::from_entity(
                        e, 0, 160.min(self.settings.view_distance * 16), 3, false,
                    )
                }
                Some(Entity::Falling(_)) | None => None,
            };
            if let Some(te) = te {
                scratch.chunks.insert(
                    te.id,
                    (
                        (te.pos[0].floor() as i32).div_euclid(16),
                        (te.pos[2].floor() as i32).div_euclid(16),
                    ),
                );
                scratch.tracked.push(te);
            }
        }
        let sessions = &self.sessions;
        let players = &self.players;
        let tracker = &mut self.world.tracker;
        for te in &scratch.tracked {
            tracker.add(te);
            tracker.tick_entity(
                te,
                &scratch.observers,
                &|observer, entity| {
                    let (ecx, ecz) = scratch.chunks.get(&entity).copied().unwrap_or((0, 0));
                    match players.get(&observer).and_then(|cid| sessions.get(cid)) {
                        Some(Session { state: SessionState::Play(_, stream), .. }) => {
                            stream.sent.contains(&chunk_key(ecx, ecz))
                        }
                        _ => false,
                    }
                },
                &mut scratch.out,
            );
        }
        let out = std::mem::take(&mut scratch.out);
        self.tracker_scratch = scratch;
        self.route_outbox(out);
    }
}

impl Server {
    /// Ship pickup feedback (mirrors the collect packet in
    /// `EntityPlayerMP.onUpdate` plus the inventory sync): `Packet22Collect`
    /// to the item's watchers and picker (the client plays `random.pop`,
    /// flies the item over and removes it), then a full inventory sync for
    /// the picker. Drained before `tracker_tick` so Collect precedes the
    /// destroy for the dead item row.
    fn drain_pickup_events(&mut self) {
        let pickups = std::mem::take(&mut self.world.item_pickups);
        if pickups.is_empty() {
            return;
        }
        let mut out = Vec::new();
        for (item_id, player_id) in &pickups {
            self.world.tracker.collect_fx(*item_id, *player_id, &mut out);
        }
        self.route_outbox(out);
        for (_, player_id) in &pickups {
            if let Some(cid) = self.players.get(player_id).copied() {
                if let Some(Session { state: SessionState::Play(play, _), .. }) =
                    self.sessions.get_mut(&cid)
                {
                    play.send_inventory(&self.world);
                }
            }
        }
    }

    /// Ship death animations, status changes, velocity impulses, and
    /// explosions (`Packet60`, `WorldServer.java:92-96` within 64 blocks):
    /// drained before `tracker_tick` so animations precede destroy packets
    /// for the same tick's kills.
    fn drain_death_events(&mut self) {
        let deaths = std::mem::take(&mut self.world.death_events);
        let statuses = std::mem::take(&mut self.world.status_events);
        let velocities = std::mem::take(&mut self.world.velocity_events);
        let explosions = std::mem::take(&mut self.world.explosion_events);
        if !deaths.is_empty() || !statuses.is_empty() || !velocities.is_empty() {
            let mut out = Vec::new();
            for id in deaths {
                self.world.tracker.death_fx(id, &mut out);
            }
            for (id, status) in statuses {
                self.world.tracker.status_fx(id, status, &mut out);
            }
            for (id, motion) in velocities {
                self.world.tracker.velocity_fx(id, motion, &mut out);
            }
            self.route_outbox(out);
        }
        for (ex, ey, ez, radius, cells) in explosions {
            let pkt = pkt_explosion(ex, ey, ez, radius, &cells);
            for (&pid, &cid) in &self.players {
                let in_range = match self.world.entities.get(pid) {
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
                        sess.conn.send(pkt.clone());
                    }
                }
            }
        }
    }

    /// Health watch (mirrors the `Packet8` send in
    /// `EntityPlayerMP.func_175_i`): push 0x08 whenever a player's health
    /// changed since the last send, so fall, mob, burn and drown damage
    /// all reach the HUD — previously only login/respawn/eat synced it.
    fn push_health_changes(&mut self) {
        let mut changed: Vec<(ConnId, i8)> = Vec::new();
        for (eid, cid) in &self.players {
            let health = match self.world.entities.get(*eid) {
                Some(Entity::Player(p)) => p.living.health as i8,
                _ => continue,
            };
            let last = match self.sessions.get(cid) {
                Some(Session { state: SessionState::Play(play, _), .. }) => play.last_health,
                _ => continue,
            };
            if health != last {
                changed.push((*cid, health));
            }
        }
        for (cid, health) in changed {
            if let Some(sess) = self.sessions.get_mut(&cid) {
                sess.conn.send(pkt_health(health));
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
    fn drain_block_updates(&mut self) -> std::collections::HashSet<(i32, i32, i32)> {
        self.world.refresh_light();
        let mut sent_tiles = std::collections::HashSet::new();
        for [x, y, z] in self.world.take_block_updates() {
            let id = self.world.get_block_id(x, y, z);
            let meta = self.world.get_block_meta(x, y, z);
            if matches!(id, 61 | 62) {
                for ((lcx, lcz), sc) in
                    crate::session_packets::pkt_subchunk_furnace_light(&self.world, x, y, z)
                {
                    for cid in self.conns_with_chunk(chunk_key(lcx, lcz)) {
                        if let Some(sess) = self.sessions.get(&cid) {
                            sess.conn.send(sc.clone());
                        }
                    }
                }
            }
            let subchunk = if matches!(id, 61 | 62) {
                crate::session_packets::pkt_subchunk_block(&self.world, x, y, z)
            } else {
                None
            };
            let furnace_tile = if matches!(id, 61 | 62) {
                self.world
                    .tiles
                    .get(&(x, y, z))
                    .map(|t| crate::session_packets::tile_packet(x, y, z, t))
            } else {
                None
            };
            if furnace_tile.is_some() {
                sent_tiles.insert((x, y, z));
            }
            let bytes = pkt_block_change(x, y, z, id, meta);
            let cids = self.conns_with_chunk(chunk_key(x.div_euclid(16), z.div_euclid(16)));
            for cid in cids {
                if let Some(sess) = self.sessions.get(&cid) {
                    sess.conn.send(bytes.clone());
                    if let Some(ref tp) = furnace_tile {
                        sess.conn.send(tp.clone());
                        if let Some(ref sc) = subchunk {
                            sess.conn.send(sc.clone());
                        }
                    }
                }
            }
        }
        sent_tiles
    }

    /// Ship queued tile-entity updates (`Packet59ComplexEntity`) to chunk-loaded players.
    fn drain_tile_updates(&mut self, mut already_sent: std::collections::HashSet<(i32, i32, i32)>) {
        for [x, y, z] in self.world.take_tile_updates() {
            if !already_sent.insert((x, y, z)) {
                continue;
            }
            if let Some(tile) = self.world.tiles.get(&(x, y, z)).copied() {
                let bytes = crate::session_packets::tile_packet(x, y, z, &tile);
                let subchunk = if matches!(tile, crate::world::TileData::Furnace(_)) {
                    crate::session_packets::pkt_subchunk_block(&self.world, x, y, z)
                } else {
                    None
                };
                let cids = self.conns_with_chunk(chunk_key(x.div_euclid(16), z.div_euclid(16)));
                for cid in cids {
                    if let Some(sess) = self.sessions.get(&cid) {
                        sess.conn.send(bytes.clone());
                        if let Some(ref sc) = subchunk {
                            sess.conn.send(sc.clone());
                        }
                    }
                }
            }
        }
    }

    /// One server tick (mirrors `serverTick`).
    pub fn tick(&mut self) {
        if !self.running {
            return;
        }
        self.poll_network(std::time::Duration::ZERO);
        self.tick_count += 1;
        if self.tick_count.is_multiple_of(crate::server::LOGIN_WINDOW_TICKS) {
            self.prune_login_attempts();
        }

        // Process incoming client packets and session events first (mirrors MinecraftServer func_715_a).
        let cids: Vec<ConnId> = self.sessions.keys().copied().collect();
        for cid in cids {
            self.pump_one(cid);
        }

        if self.tick_count.is_multiple_of(TICKS_PER_SECOND as u64) {
            let bytes = pkt_time(self.world.time);
            let cids: Vec<ConnId> = self.sessions.keys().copied().collect();
            for cid in cids {
                if self.is_play(cid) {
                    if let Some(sess) = self.sessions.get(&cid) {
                        sess.conn.send(bytes.clone());
                    }
                }
            }
        }
        self.world.tick_world();
        self.flush_unloaded_chunks();
        self.drain_pickup_events();
        self.drain_death_events();
        self.push_health_changes();
        if self.settings.auto_save_interval > 0
            && self.tick_count.is_multiple_of(self.settings.auto_save_interval as u64)
        {
            self.save_players();
            self.save_world();
        }
        self.tracker_tick();
        let sent_tiles = self.drain_block_updates();
        self.drain_tile_updates(sent_tiles);
        self.poll_network(std::time::Duration::ZERO);
        let lines = std::mem::take(&mut self.console);
        for line in lines {
            self.handle_console(&line);
        }
    }
}
