//! The 20 TPS tick: event drains, tracker pass, world tick.
//! Split out of `server.rs`; behavior unchanged.

use std::collections::{HashMap, HashSet};
use crate::entity::table::{Entity, EntityId};
use crate::server::sessions::{Session, SessionState};
use crate::server::{ConnId, Server, chunk_key};
use crate::server_constants::TICKS_PER_SECOND;
use crate::session::{pkt_block_change, pkt_health, pkt_time};
use crate::tracker::{Observer, TrackedEntity};

impl Server {
    /// Tracker pass (mirrors `EntityTracker::tick`): destroy packets for
    /// retired entries (dead or vanished rows, e.g. killed mobs that used
    /// to hang client-side), then per-entity updates for the live ones.
    fn tracker_tick(&mut self) {
        let mut ids = self.world.entities.alive_ids();
        ids.sort_unstable();
        let live: HashSet<EntityId> = ids
            .iter()
            .copied()
            .filter(|id| match self.world.entities.get(*id) {
                Some(e) => !e.body().dead,
                None => false,
            })
            .collect();
        // Retire dead/vanished entries with destroy packets (mirrors
        // `EntityTrackerEntry.func_604_a`), so killed mobs stop hanging
        // client-side. Runs before the tick loop on a shared outbox.
        let mut out = Vec::new();
        let gone: Vec<EntityId> =
            self.world.tracker.tracked_ids().into_iter().filter(|id| !live.contains(id)).collect();
        for id in gone {
            self.world.tracker.remove(id, &mut out);
        }
        let observers: Vec<Observer> = live
            .iter()
            .filter_map(|id| match self.world.entities.get(*id) {
                Some(Entity::Player(p)) if self.players.contains_key(id) => Some(Observer {
                    id: *id,
                    pos: p.living.body.pos,
                    // The dead see nothing (mirrors the C++ isDead skip).
                    alive: !p.living.body.dead,
                }),
                _ => None,
            })
            .collect();
        // Snapshot rows into owned tracked structs (params mirror C++).
        let mut tracked = Vec::new();
        let mut chunks = HashMap::new();
        for id in &live {
            let held = match self.world.entities.get(*id) {
                Some(Entity::Player(_)) => {
                    match self.players.get(id).and_then(|cid| self.sessions.get(cid)) {
                        Some(Session { state: SessionState::Play(play, _), .. }) => {
                            play.held_id as i16
                        }
                        _ => 0,
                    }
                }
                _ => 0,
            };
            let te = match self.world.entities.get(*id) {
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
                chunks.insert(
                    te.id,
                    (
                        (te.pos[0].floor() as i32).div_euclid(16),
                        (te.pos[2].floor() as i32).div_euclid(16),
                    ),
                );
                tracked.push(te);
            }
        }
        let sessions = &self.sessions;
        let players = &self.players;
        let tracker = &mut self.world.tracker;
        for te in &tracked {
            tracker.add(te);
            tracker.tick_entity(
                te,
                &observers,
                &|observer, entity| {
                    let (ecx, ecz) = chunks.get(&entity).copied().unwrap_or((0, 0));
                    match players.get(&observer).and_then(|cid| sessions.get(cid)) {
                        Some(Session { state: SessionState::Play(_, stream), .. }) => {
                            stream.sent.contains(&chunk_key(ecx, ecz))
                        }
                        _ => false,
                    }
                },
                &mut out,
            );
        }
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

    /// Ship death animations (mirrors the status-3 broadcast in
    /// `EntityLiving.onDeath` via `WorldServer.func_9206_a`): drained
    /// before `tracker_tick` so the animation precedes the destroy packet
    /// for the same tick's kills.
    fn drain_death_events(&mut self) {
        let deaths = std::mem::take(&mut self.world.death_events);
        let statuses = std::mem::take(&mut self.world.status_events);
        if deaths.is_empty() && statuses.is_empty() {
            return;
        }
        let mut out = Vec::new();
        for id in deaths {
            self.world.tracker.death_fx(id, &mut out);
        }
        for (id, status) in statuses {
            self.world.tracker.status_fx(id, status, &mut out);
        }
        self.route_outbox(out);
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
    fn drain_block_updates(&mut self) {
        for [x, y, z] in self.world.take_block_updates() {
            let bytes = pkt_block_change(x, y, z, self.world.get_block_id(x, y, z), self.world.get_block_meta(x, y, z));
            let cids = self.conns_with_chunk(chunk_key(x.div_euclid(16), z.div_euclid(16)));
            for cid in cids {
                if let Some(sess) = self.sessions.get(&cid) {
                    sess.conn.send(bytes.clone());
                }
            }
        }
    }

    /// One server tick (mirrors `serverTick`).
    pub fn tick(&mut self) {
        if !self.running {
            return;
        }
        self.tick_count += 1;
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
        let cids: Vec<ConnId> = self.sessions.keys().copied().collect();
        for cid in cids {
            self.pump_one(cid);
        }
        self.drain_block_updates();
        let lines = std::mem::take(&mut self.console);
        for line in lines {
            self.handle_console(&line);
        }
    }
}
