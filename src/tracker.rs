//! Native entity tracker: the Rust-owned replacement for C++
/// `EntityTracker` / `TrackerEntry` (mirrors Java `EntityTrackerEntry`).
///
/// Takes per-tick snapshots (`TrackedEntity`, `Observer`) instead of
/// pointers, reuses `tracker_math` for every formula and `network` for
/// packet encoding, and emits owned per-player byte buffers (`Outbox`).
/// Chunk visibility comes from the caller (the native world); entity kinds
/// map exactly like the C++ `dynamic_cast` chain, including the pig-90
/// fallback for unknown entities.
///
/// Registration parameters mirror `EntityTracker::addEntity`:
/// player 512/1, item 64/20+velocity, arrow 64/5+velocity, boat 160/5+
/// velocity, mobs via their range/rate/velocity params. Falling sand and
/// anything else is not tracked (Alpha client behavior).
use std::collections::{HashMap, HashSet};

use crate::entity::table::{Entity, EntityId, MobKind, NO_ENTITY, animal_type_id, mob_type_id};
use crate::network::{Packet, encode_packet};
use crate::tracker_math::{
    tracker_encode_pos, tracker_encode_rot, tracker_encode_rot_spawn,
    tracker_in_range, tracker_move_kind, tracker_velocity_changed,
};

/// Tracked kind with the fields its spawn packet needs.
#[derive(Clone, Debug)]
pub enum TrackKind {
    Player { username: String, held: i16 },
    Item { item_id: i16, count: i8 },
    Arrow,
    Boat,
    Mob { mob_type: u8 },
}

#[derive(Clone, Debug)]
pub struct TrackedEntity {
    pub id: EntityId,
    pub kind: TrackKind,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub motion: [f64; 3],
    pub health: Option<i16>,
    pub sneaking: bool,
    pub fire_ticks: i32,
    pub riding: EntityId,
    pub range: i32,
    pub rate: i32,
    pub send_velocity: bool,
    pub is_player: bool,
}

impl TrackedEntity {
    /// Build from a native table row (`held` only matters for players).
    /// Returns `None` for untrackable kinds (mirrors `addEntity`).
    pub fn from_entity(e: &Entity, held: i16, range: i32, rate: i32, send_velocity: bool) -> Option<Self> {
        let b = e.body();
        let base = TrackedEntity {
            id: b.id,
            kind: TrackKind::Boat,
            pos: b.pos,
            yaw: b.yaw,
            pitch: b.pitch,
            motion: b.motion,
            health: None,
            sneaking: false,
            fire_ticks: b.fire,
            riding: b.riding,
            range,
            rate,
            send_velocity,
            is_player: false,
        };
        Some(match e {
            Entity::Player(p) => TrackedEntity {
                kind: TrackKind::Player { username: p.username.clone(), held },
                // Players are living rows too: hurt flashes and sneak
                // ride the tracker like the C++ TrackerEntry does.
                health: Some(p.living.health),
                sneaking: p.living.sneaking,
                is_player: true,
                ..base
            },
            Entity::Item(i) => TrackedEntity {
                kind: TrackKind::Item { item_id: i.item_id as i16, count: i.count as i8 },
                ..base
            },
            Entity::Arrow(_) => TrackedEntity { kind: TrackKind::Arrow, ..base },
            Entity::Boat(_) => TrackedEntity { kind: TrackKind::Boat, ..base },
            Entity::Mob(m) => TrackedEntity {
                kind: TrackKind::Mob { mob_type: mob_type_id(m.kind) },
                health: Some(m.living.health),
                ..base
            },
            Entity::Animal(a) => TrackedEntity {
                kind: TrackKind::Mob { mob_type: animal_type_id(a.kind) },
                health: Some(a.living.health),
                ..base
            },
            Entity::Falling(_) => return None,
        })
    }

    pub fn player(id: EntityId, username: &str, pos: [f64; 3], held: i16) -> Self {
        TrackedEntity {
            id,
            kind: TrackKind::Player { username: username.to_string(), held },
            pos,
            yaw: 0.0,
            pitch: 0.0,
            motion: [0.0; 3],
            health: None,
            sneaking: false,
            fire_ticks: 0,
            riding: NO_ENTITY,
            range: 512,
            rate: 2,
            send_velocity: false,
            is_player: true,
        }
    }

    pub fn mob(id: EntityId, kind: MobKind, pos: [f64; 3]) -> Self {
        TrackedEntity {
            id,
            kind: TrackKind::Mob { mob_type: mob_type_id(kind) },
            pos,
            yaw: 0.0,
            pitch: 0.0,
            motion: [0.0; 3],
            health: Some(20),
            sneaking: false,
            fire_ticks: 0,
            riding: NO_ENTITY,
            range: 160,
            rate: 3,
            send_velocity: false,
            is_player: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Observer {
    pub id: EntityId,
    pub pos: [f64; 3],
    pub alive: bool,
}

#[derive(Clone, Debug)]
pub struct Outbox {
    pub to: EntityId,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
struct Entry {
    range: i32,
    rate: i32,
    send_velocity: bool,
    last_fixed: [i32; 3],
    last_yaw: i8,
    last_pitch: i8,
    last_held: i16,
    last_sneaking: bool,
    last_burning: bool,
    last_mounted: EntityId,
    last_motion: [f64; 3],
    last_health: i16,
    is_player: bool,
    status2_sent: bool,
    tick: u64,
    tracking: HashSet<EntityId>,
}

impl Entry {
    fn new(e: &TrackedEntity) -> Self {
        let (fx, fy, fz) = (
            tracker_encode_pos(e.pos[0]),
            tracker_encode_pos(e.pos[1]),
            tracker_encode_pos(e.pos[2]),
        );
        Entry {
            range: e.range,
            rate: e.rate.max(1),
            send_velocity: e.send_velocity,
            last_fixed: [fx, fy, fz],
            last_yaw: tracker_encode_rot(e.yaw),
            last_pitch: tracker_encode_rot(e.pitch),
            last_held: 0,
            last_sneaking: false,
            last_burning: false,
            last_mounted: NO_ENTITY,
            last_motion: [0.0; 3],
            last_health: -1,
            is_player: e.is_player,
            status2_sent: false,
            tick: 0,
            tracking: HashSet::new(),
        }
    }
}

fn encode(pkt: Packet) -> Vec<u8> {
    let mut buf = Vec::new();
    encode_packet(&pkt, &mut buf);
    buf
}

fn encode_velocity(id: EntityId, m: [f64; 3]) -> Vec<u8> {
    // Vanilla velocity: clamp to ±3.9, scale by 8000.
    let enc = |v: f64| (v.clamp(-3.9, 3.9) * 8000.0) as i16;
    encode(Packet::EntityVelocity {
        entity_id: id,
        motion_x: enc(m[0]),
        motion_y: enc(m[1]),
        motion_z: enc(m[2]),
    })
}

fn encode_spawn(e: &TrackedEntity) -> Vec<u8> {
    let fx = tracker_encode_pos(e.pos[0]);
    let fy = tracker_encode_pos(e.pos[1]);
    let fz = tracker_encode_pos(e.pos[2]);
    // Spawn packets use truncating (int) cast (Packet20/24), not floor.
    let yaw = tracker_encode_rot_spawn(e.yaw);
    let pitch = tracker_encode_rot_spawn(e.pitch);
    match &e.kind {
        TrackKind::Player { username, held } => encode(Packet::NamedEntitySpawn {
            entity_id: e.id,
            username: username.clone(),
            x: fx,
            y: fy,
            z: fz,
            rotation: yaw,
            pitch,
            current_item: *held,
        }),
        TrackKind::Item { item_id, count } => encode(Packet::PickupSpawn {
            entity_id: e.id,
            item_id: *item_id,
            count: *count,
            x: fx,
            y: fy,
            z: fz,
            rotation: 0,
            pitch: 0,
            roll: 0,
        }),
        TrackKind::Arrow => encode(Packet::VehicleSpawn {
            entity_id: e.id,
            vehicle_type: 60,
            x: fx,
            y: fy,
            z: fz,
        }),
        TrackKind::Boat => encode(Packet::VehicleSpawn {
            entity_id: e.id,
            vehicle_type: 1,
            x: fx,
            y: fy,
            z: fz,
        }),
        TrackKind::Mob { mob_type } => encode(Packet::MobSpawn {
            entity_id: e.id,
            mob_type: *mob_type,
            x: fx,
            y: fy,
            z: fz,
            yaw,
            pitch,
        }),
    }
}

/// Native tracker: entries plus per-tick update. Emits owned byte buffers
/// addressed to player ids; delivery stays with the caller.
#[derive(Clone, Debug, Default)]
pub struct Tracker {
    entries: HashMap<EntityId, Entry>,
    fresh_scratch: Vec<EntityId>,
    gone_scratch: Vec<EntityId>,
}

impl Tracker {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            fresh_scratch: Vec::new(),
            gone_scratch: Vec::new(),
        }
    }

    pub fn add(&mut self, e: &TrackedEntity) {
        self.entries.entry(e.id).or_insert_with(|| Entry::new(e));
    }

    /// Remove an entry, emitting destroy packets to current watchers.
    pub fn remove(&mut self, id: EntityId, out: &mut Vec<Outbox>) {
        if let Some(entry) = self.entries.remove(&id) {
            let bytes = encode(Packet::DestroyEntity { entity_id: id });
            for watcher in entry.tracking {
                out.push(Outbox { to: watcher, bytes: bytes.clone() });
            }
        }
    }

    pub fn is_tracking(&self, id: EntityId, player: EntityId) -> bool {
        self.entries.get(&id).map(|e| e.tracking.contains(&player)).unwrap_or(false)
    }

    /// Currently tracked entity ids (the server retires entries whose
    /// rows died or vanished, emitting destroy packets via `remove`).
    pub fn tracked_ids(&self) -> Vec<EntityId> {
        let mut out = Vec::with_capacity(self.entries.len());
        self.collect_tracked_ids(&mut out);
        out
    }

    /// Iterator over tracked entity ids without any heap allocation.
    pub fn iter_tracked_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.entries.keys().copied()
    }

    /// Reuses caller-provided buffer to collect tracked ids without allocation.
    pub fn collect_tracked_ids(&self, out: &mut Vec<EntityId>) {
        out.clear();
        for &id in self.entries.keys() {
            out.push(id);
        }
    }

    /// Current watchers of an entity (mirrors `TrackerEntry.trackingPlayers`;
    /// the server uses it for swing fan-out, which skips the source player).
    pub fn watchers(&self, id: EntityId) -> Vec<EntityId> {
        self.entries.get(&id).map(|e| e.tracking.iter().copied().collect()).unwrap_or_default()
    }

    /// Send one entity's current state to a single player (login catch-up).
    fn send_spawn_to(&mut self, id: EntityId, e: &TrackedEntity, player: EntityId, out: &mut Vec<Outbox>) {
        out.push(Outbox { to: player, bytes: encode_spawn(e) });
        if e.riding >= 0 {
            out.push(Outbox {
                to: player,
                bytes: encode(Packet::AttachEntity { entity_id: id, vehicle_id: e.riding }),
            });
        }
        if e.send_velocity && (e.motion[0] != 0.0 || e.motion[1] != 0.0 || e.motion[2] != 0.0) {
            out.push(Outbox { to: player, bytes: encode_velocity(id, e.motion) });
        }
        if e.sneaking {
            out.push(Outbox {
                to: player,
                bytes: encode(Packet::ArmAnimation { entity_id: id, animate: 104 }),
            });
        }
        if e.fire_ticks > 0 {
            out.push(Outbox {
                to: player,
                bytes: encode(Packet::ArmAnimation { entity_id: id, animate: 102 }),
            });
        }
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.last_fixed = [
                tracker_encode_pos(e.pos[0]),
                tracker_encode_pos(e.pos[1]),
                tracker_encode_pos(e.pos[2]),
            ];
            entry.last_yaw = tracker_encode_rot(e.yaw);
            entry.last_pitch = tracker_encode_rot(e.pitch);
            if let Some(h) = e.health {
                entry.last_health = h;
            }
        }
    }

    fn send_to_watchers(&self, id: EntityId, bytes: Vec<u8>, out: &mut Vec<Outbox>) {
        if let Some(entry) = self.entries.get(&id) {
            for watcher in &entry.tracking {
                out.push(Outbox { to: *watcher, bytes: bytes.clone() });
            }
        }
    }

    fn send_to_watchers_and_self(&self, e: &TrackedEntity, bytes: Vec<u8>, out: &mut Vec<Outbox>) {
        self.send_to_watchers(e.id, bytes.clone(), out);
        if e.is_player {
            out.push(Outbox { to: e.id, bytes });
        }
    }

    /// Pickup fan-out (mirrors the collect packet in
    /// `EntityPlayerMP.onUpdate`): the item's watchers plus the picker
    /// itself get `Packet22Collect`, so the client plays `random.pop`,
    /// flies the item to the player and removes it.
    pub fn collect_fx(&self, item_id: EntityId, picker_id: EntityId, out: &mut Vec<Outbox>) {
        let bytes = encode(Packet::Collect { collected_id: item_id, collector_id: picker_id });
        self.send_to_watchers(item_id, bytes.clone(), out);
        if !self.is_tracking(item_id, picker_id) {
            out.push(Outbox { to: picker_id, bytes });
        }
    }

    /// Death animation (mirrors the `Packet38` status-3 broadcast in
    /// `EntityLiving.onDeath` via `WorldServer.func_9206_a`).
    pub fn death_fx(&self, id: EntityId, out: &mut Vec<Outbox>) {
        let bytes = encode(Packet::EntityStatus { entity_id: id, status: 3 });
        self.send_to_watchers(id, bytes, out);
    }

    /// Generic status broadcast (creeper 4/5, hurt 2, etc.).
    pub fn status_fx(&mut self, id: EntityId, status: i8, out: &mut Vec<Outbox>) {
        let bytes = encode(Packet::EntityStatus { entity_id: id, status });
        self.send_to_watchers(id, bytes, out);
        if status == 2 {
            if let Some(entry) = self.entries.get_mut(&id) {
                entry.status2_sent = true;
            }
        }
    }

    /// Immediate velocity impulse broadcast (mirrors Packet28 on knockback/damage).
    pub fn velocity_fx(&self, id: EntityId, motion: [f64; 3], out: &mut Vec<Outbox>) {
        let bytes = encode_velocity(id, motion);
        self.send_to_watchers(id, bytes.clone(), out);
        if self.entries.get(&id).map(|e| e.is_player).unwrap_or(false) {
            out.push(Outbox { to: id, bytes });
        }
    }

    /// Per-entity per-tick update (mirrors `updateTracking` + `sendUpdates`).
    /// `chunk_visible(observer, entity)` answers the chunk-loaded check.
    pub fn tick_entity(
        &mut self,
        e: &TrackedEntity,
        observers: &[Observer],
        chunk_visible: &dyn Fn(EntityId, EntityId) -> bool,
        out: &mut Vec<Outbox>,
    ) {
        if !self.entries.contains_key(&e.id) {
            return;
        }
        // Membership pass.
        self.fresh_scratch.clear();
        self.gone_scratch.clear();
        {
            let entry = &self.entries[&e.id];
            for o in observers {
                if !o.alive {
                    continue;
                }
                let in_range = tracker_in_range(
                    o.pos[0],
                    o.pos[2],
                    entry.last_fixed[0],
                    entry.last_fixed[2],
                    entry.range,
                );
                let seen = chunk_visible(o.id, e.id);
                let already = entry.tracking.contains(&o.id);
                if in_range && seen && !already && o.id != e.id {
                    self.fresh_scratch.push(o.id);
                } else if (!in_range || !seen) && already {
                    self.gone_scratch.push(o.id);
                }
            }
        }
        for i in 0..self.fresh_scratch.len() {
            let pid = self.fresh_scratch[i];
            if let Some(entry) = self.entries.get_mut(&e.id) {
                entry.tracking.insert(pid);
            }
            self.send_spawn_to(e.id, e, pid, out);
        }
        for i in 0..self.gone_scratch.len() {
            let pid = self.gone_scratch[i];
            if let Some(entry) = self.entries.get_mut(&e.id) {
                entry.tracking.remove(&pid);
            }
            out.push(Outbox {
                to: pid,
                bytes: encode(Packet::DestroyEntity { entity_id: e.id }),
            });
        }

        // Movement pass (gated by `entry.rate`; state/mount/held/health
        // checks below run every tick so Packet39AttachEntity, hurt flash,
        // sneak, fire, and held-item switches are never delayed by `rate`).
        let do_move = match self.entries.get_mut(&e.id) {
            Some(entry) => {
                entry.tick += 1;
                (entry.tick - 1).is_multiple_of(entry.rate as u64)
            }
            None => return,
        };
        if do_move {
            let fx = tracker_encode_pos(e.pos[0]);
            let fy = tracker_encode_pos(e.pos[1]);
            let fz = tracker_encode_pos(e.pos[2]);
            let yaw = tracker_encode_rot(e.yaw);
            let pitch = tracker_encode_rot(e.pitch);
            let (dx, dy, dz, moved, turned, vel_changed) = {
                let entry = &self.entries[&e.id];
                let dx = fx - entry.last_fixed[0];
                let dy = fy - entry.last_fixed[1];
                let dz = fz - entry.last_fixed[2];
                let moved = dx != 0 || dy != 0 || dz != 0;
                let turned = yaw != entry.last_yaw || pitch != entry.last_pitch;
                let vel_changed = tracker_velocity_changed(
                    e.motion[0],
                    e.motion[1],
                    e.motion[2],
                    entry.last_motion[0],
                    entry.last_motion[1],
                    entry.last_motion[2],
                    entry.send_velocity,
                );
                (dx, dy, dz, moved, turned, vel_changed)
            };

            if vel_changed {
                if let Some(entry) = self.entries.get_mut(&e.id) {
                    entry.last_motion = e.motion;
                }
                let bytes = encode_velocity(e.id, e.motion);
                self.send_to_watchers(e.id, bytes, out);
            }

            let kind = tracker_move_kind(dx, dy, dz, moved, turned);
            let move_bytes = match kind {
                3 => encode(Packet::RelEntityMoveLook {
                    entity_id: e.id,
                    dx: dx as i8,
                    dy: dy as i8,
                    dz: dz as i8,
                    yaw,
                    pitch,
                }),
                1 => encode(Packet::RelEntityMove {
                    entity_id: e.id,
                    dx: dx as i8,
                    dy: dy as i8,
                    dz: dz as i8,
                }),
                2 => encode(Packet::EntityLook { entity_id: e.id, yaw, pitch }),
                // Tracker teleport (EntityTrackerEntry:86 → Packet34) reuses the
                // floor-encoded yaw/pitch, NOT the spawn trunc form. Using trunc
                // here skewed negative angles by 1 unit (0.35°).
                4 => encode(Packet::EntityTeleport {
                    entity_id: e.id,
                    x: fx,
                    y: fy,
                    z: fz,
                    yaw,
                    pitch,
                }),
                _ => encode(Packet::Entity { entity_id: e.id }),
            };
            self.send_to_watchers(e.id, move_bytes, out);

            if moved || turned {
                if let Some(entry) = self.entries.get_mut(&e.id) {
                    entry.last_fixed = [fx, fy, fz];
                    entry.last_yaw = yaw;
                    entry.last_pitch = pitch;
                }
            }
        }

        // Held item (players).
        if let TrackKind::Player { held, .. } = &e.kind {
            let changed = self.entries.get(&e.id).map(|en| *held != en.last_held).unwrap_or(false);
            if changed {
                if let Some(entry) = self.entries.get_mut(&e.id) {
                    entry.last_held = *held;
                }
                self.send_to_watchers(
                    e.id,
                    encode(Packet::BlockItemSwitch { entity_id: e.id, item_id: *held }),
                    out,
                );
            }
        }

        // Health / sneak / fire (living).
        if let Some(health) = e.health {
            let last = self.entries.get(&e.id).map(|en| en.last_health).unwrap_or(-1);
            let already_sent = self.entries.get(&e.id).map(|en| en.status2_sent).unwrap_or(false);
            if last >= 0 && health != last {
                // Java EntityLiving:313 sends hurt (2) BEFORE the death check,
                // so the killing blow flashes too. Suppressed if status_fx already emitted it.
                if health < last && !already_sent {
                    self.send_to_watchers(
                        e.id,
                        encode(Packet::EntityStatus { entity_id: e.id, status: 2 }),
                        out,
                    );
                }
                if let Some(entry) = self.entries.get_mut(&e.id) {
                    entry.last_health = health;
                    entry.status2_sent = false;
                }
            } else if already_sent {
                if let Some(entry) = self.entries.get_mut(&e.id) {
                    entry.last_health = health;
                    entry.status2_sent = false;
                }
            }
            let was_sneaking = self.entries.get(&e.id).map(|en| en.last_sneaking).unwrap_or(false);
            if e.sneaking != was_sneaking {
                if let Some(entry) = self.entries.get_mut(&e.id) {
                    entry.last_sneaking = e.sneaking;
                }
                self.send_to_watchers_and_self(
                    e,
                    encode(Packet::ArmAnimation {
                        entity_id: e.id,
                        animate: if e.sneaking { 104 } else { 105 },
                    }),
                    out,
                );
            }
            let burning = e.fire_ticks > 0;
            let was_burning = self.entries.get(&e.id).map(|en| en.last_burning).unwrap_or(false);
            if burning != was_burning {
                if let Some(entry) = self.entries.get_mut(&e.id) {
                    entry.last_burning = burning;
                }
                self.send_to_watchers_and_self(
                    e,
                    encode(Packet::ArmAnimation {
                        entity_id: e.id,
                        animate: if burning { 102 } else { 103 },
                    }),
                    out,
                );
            }
        }

        // Mount link.
        let was_mounted = self.entries.get(&e.id).map(|en| en.last_mounted).unwrap_or(NO_ENTITY);
        if e.riding != was_mounted {
            if let Some(entry) = self.entries.get_mut(&e.id) {
                entry.last_mounted = e.riding;
            }
            self.send_to_watchers_and_self(
                e,
                encode(Packet::AttachEntity { entity_id: e.id, vehicle_id: e.riding }),
                out,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observer(id: EntityId, x: f64, z: f64) -> Observer {
        Observer { id, pos: [x, 64.0, z], alive: true }
    }

    fn always(_o: EntityId, _e: EntityId) -> bool {
        true
    }

    #[test]
    fn test_spawn_and_despawn_cycle() {
        let mut t = Tracker::new();
        let mob = TrackedEntity::mob(7, MobKind::Zombie, [10.5, 64.0, 10.5]);
        t.add(&mob);
        let obs = vec![observer(1, 12.0, 12.0)];
        let mut out = Vec::new();
        t.tick_entity(&mob, &obs, &always, &mut out);
        assert!(t.is_tracking(7, 1));
        assert_eq!(t.watchers(7), vec![1]);
        assert!(t.watchers(999).is_empty());
        // First packet of a fresh watcher is the mob spawn (id 24).
        assert!(!out.is_empty());
        assert_eq!(out[0].to, 1);
        assert_eq!(out[0].bytes[0], 24);
        // Walk away: destroy packet (id 29) and untracked. The range
        // check runs against the last sent position, so one tick suffices
        // (observer jumps far outside the 160-block range).
        let far = vec![observer(1, 900.0, 900.0)];
        t.tick_entity(&mob, &far, &always, &mut out);
        t.tick_entity(&mob, &far, &always, &mut out);
        assert!(!t.is_tracking(7, 1));
        assert!(out.iter().any(|o| o.to == 1 && o.bytes[0] == 29));
    }

    #[test]
    fn test_movement_packets_follow_kind() {
        let mut t = Tracker::new();
        // Rate 1: every tick sends.
        let mut mob = TrackedEntity::mob(7, MobKind::Zombie, [10.5, 64.0, 10.5]);
        mob.rate = 1;
        t.add(&mob);
        let obs = vec![observer(1, 12.0, 12.0)];
        let mut out = Vec::new();
        t.tick_entity(&mob, &obs, &always, &mut out);
        out.clear();
        // Nudge +x by one fixed unit (1/32 block): relative move (31).
        mob.pos[0] += 1.0 / 32.0;
        t.tick_entity(&mob, &obs, &always, &mut out);
        assert_eq!(out[0].bytes[0], 31);
        // Teleport-scale jump: packet 34.
        out.clear();
        mob.pos[0] += 100.0;
        t.tick_entity(&mob, &obs, &always, &mut out);
        // Membership re-check may despawn first (out of 160 range? no:
        // observer at 12, entity now ~110 -> dist ~98 < 160 still tracking).
        assert!(out.iter().any(|o| o.bytes[0] == 34));
    }

    #[test]
    fn test_player_spawn_carries_name_and_held() {
        let mut t = Tracker::new();
        let p = TrackedEntity::player(3, "steve", [0.5, 64.0, 0.5], 278);
        t.add(&p);
        let obs = vec![observer(9, 5.0, 5.0)];
        let mut out = Vec::new();
        t.tick_entity(&p, &obs, &always, &mut out);
        assert_eq!(out[0].bytes[0], 20);
        // Name bytes follow the entity id.
        let name = b"steve";
        assert!(out[0].bytes.windows(name.len()).any(|w| w == name));
    }

    #[test]
    fn test_health_sneak_and_mount_events() {
        let mut t = Tracker::new();
        let mut mob = TrackedEntity::mob(7, MobKind::Spider, [10.5, 64.0, 10.5]);
        mob.rate = 1;
        t.add(&mob);
        let obs = vec![observer(1, 12.0, 12.0)];
        let mut out = Vec::new();
        t.tick_entity(&mob, &obs, &always, &mut out);
        out.clear();
        // Damage without death: status 2 packet (38).
        mob.health = Some(15);
        t.tick_entity(&mob, &obs, &always, &mut out);
        assert!(out.iter().any(|o| o.bytes[0] == 38));
        // Sneak toggle: arm animation to watchers.
        out.clear();
        mob.sneaking = true;
        t.tick_entity(&mob, &obs, &always, &mut out);
        assert!(out.iter().any(|o| o.bytes[0] == 18));
        // Mount link change: attach packet (39).
        out.clear();
        mob.riding = 42;
        t.tick_entity(&mob, &obs, &always, &mut out);
        assert!(out.iter().any(|o| o.bytes[0] == 39));
    }

    #[test]
    fn test_remove_emits_destroy() {
        let mut t = Tracker::new();
        let mob = TrackedEntity::mob(7, MobKind::Zombie, [10.5, 64.0, 10.5]);
        t.add(&mob);
        let obs = vec![observer(1, 12.0, 12.0)];
        let mut out = Vec::new();
        t.tick_entity(&mob, &obs, &always, &mut out);
        assert!(t.is_tracking(7, 1));
        let mut out = Vec::new();
        t.remove(7, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].to, out[0].bytes[0]), (1, 29));
    }

    #[test]
    fn test_collect_fx_to_watchers_and_picker() {
        // Pickup fan-out (Packet22Collect): the item's watchers plus the
        // picker animate it; a watching picker gets exactly one copy.
        let mut t = Tracker::new();
        let mob = TrackedEntity::mob(7, MobKind::Zombie, [10.5, 64.0, 10.5]);
        t.add(&mob);
        let obs = vec![observer(1, 12.0, 12.0)];
        let mut out = Vec::new();
        t.tick_entity(&mob, &obs, &always, &mut out);
        assert!(t.is_tracking(7, 1));
        let mut out = Vec::new();
        t.collect_fx(7, 2, &mut out);
        let mut tos: Vec<EntityId> = out.iter().map(|o| o.to).collect();
        tos.sort_unstable();
        assert_eq!(tos, vec![1, 2]);
        for o in &out {
            assert_eq!(o.bytes[0], 22);
            assert_eq!(&o.bytes[1..], &[0, 0, 0, 7, 0, 0, 0, 2]);
        }
        let mut out = Vec::new();
        t.collect_fx(7, 1, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].to, out[0].bytes[0]), (1, 22));
    }

    #[test]
    fn test_from_entity_kinds() {
        let mut table = crate::entity::table::EntityTable::new();
        let id = table.alloc_id();
        let mut l = crate::entity::table::LivingBody::new(id, 0.6, 1.9, 0.0);
        l.body.set_position(0.0, 64.0, 0.0);
        table.insert(crate::entity::table::Entity::Mob(crate::entity::table::MobEnt {
            living: l,
            kind: MobKind::Creeper,
            target: None,
            attack_cooldown: 0,
            target_timer: 0,
            burn_ticks: 0,
            age: 0,
            path: Vec::new(),
            path_index: 0,
            swell_time: 0,
            swell_dir: -1,
        }));
        let row = table.get(id).unwrap();
        let tracked = TrackedEntity::from_entity(row, 0, 160, 3, false).unwrap();
        assert!(matches!(tracked.kind, TrackKind::Mob { mob_type: 50 }));
        assert_eq!(tracked.health, Some(20));
        // Animals map to their pig-90-family ids.
        let id2 = table.alloc_id();
        let mut l2 = crate::entity::table::LivingBody::new(id2, 0.6, 1.3, 0.0);
        l2.body.set_position(0.0, 64.0, 0.0);
        table.insert(crate::entity::table::Entity::Animal(crate::entity::table::AnimalEnt {
            living: l2,
            kind: crate::entity::table::AnimalKind::Cow,
            sheared: false,
            saddled: false,
            egg_timer: 6000,
            age: 0,
            path: Vec::new(),
            path_index: 0,
        }));
        let row2 = table.get(id2).unwrap();
        let tracked2 = TrackedEntity::from_entity(row2, 0, 160, 3, false).unwrap();
        assert!(matches!(tracked2.kind, TrackKind::Mob { mob_type: 92 }));
    }
}
