//! Native entity table (mirrors Java `Entity` identity and mount semantics).
//!
//! Design: one `Body` with every base-`Entity` field, plus per-kind payloads
//! in the `Entity` enum. Downcasting becomes `match`; the mount graph
//! (`ridingEntityId` / `riddenByEntityId`) is manipulated through table
//! methods that tolerate missing rows.
//!
//! Physics, damage, and steering keep living in `entity_physics`,
//! `entity_living`, and `entity_ai`; this module owns identity, mounting,
//! per-tick previous-state sync, and rider placement. World-dependent
//! scans (environment, collisions) arrive with the native world.

use crate::aabb::AxisAlignedBB;

pub type EntityId = i32;
pub const NO_ENTITY: EntityId = -1;

fn next_id(counter: &mut EntityId) -> EntityId {
    let id = *counter;
    *counter = counter.wrapping_add(1);
    id
}

/// Every base-`Entity` field (positions, motion, box, flags, links).
#[derive(Clone, Debug)]
pub struct Body {
    pub id: EntityId,
    pub pos: [f64; 3],
    pub prev_pos: [f64; 3],
    pub track_pos: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub prev_yaw: f32,
    pub prev_pitch: f32,
    pub track_yaw: f32,
    pub track_pitch: f32,
    pub bounding_box: AxisAlignedBB,
    pub on_ground: bool,
    pub collided_horiz: bool,
    pub collided_vert: bool,
    pub dead: bool,
    pub width: f32,
    pub height: f32,
    pub y_offset: f32,
    pub step_height: f32,
    pub no_clip: bool,
    pub fall_distance: f32,
    pub fire: i32,
    pub air: i32,
    pub in_water: bool,
    pub dimension: i32,
    pub fire_resistance: i32,
    pub suppress_fall_state: bool,
    pub riding: EntityId,
    pub ridden_by: EntityId,
}

impl Body {
    pub fn new(id: EntityId, width: f32, height: f32, y_offset: f32) -> Self {
        Self {
            id,
            pos: [0.0; 3],
            prev_pos: [0.0; 3],
            track_pos: [0.0; 3],
            motion: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            prev_yaw: 0.0,
            prev_pitch: 0.0,
            track_yaw: 0.0,
            track_pitch: 0.0,
            bounding_box: AxisAlignedBB::get_bounding_box(0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            on_ground: false,
            collided_horiz: false,
            collided_vert: false,
            dead: false,
            width,
            height,
            y_offset,
            step_height: 0.0,
            no_clip: false,
            fall_distance: 0.0,
            fire: 0,
            air: 300,
            in_water: false,
            dimension: 0,
            fire_resistance: 1,
            suppress_fall_state: false,
            riding: NO_ENTITY,
            ridden_by: NO_ENTITY,
        }
    }

    /// Mirrors `Entity::setPosition` (same formula as the C++ inline).
    pub fn set_position(&mut self, x: f64, y: f64, z: f64) {
        self.pos = [x, y, z];
        let hw = self.width as f64 / 2.0;
        let yo = self.y_offset as f64;
        self.bounding_box = AxisAlignedBB::get_bounding_box(
            x - hw,
            y - yo,
            z - hw,
            x + hw,
            y - yo + self.height as f64,
            z + hw,
        );
    }

    pub fn distance_sq(&self, x: f64, y: f64, z: f64) -> f64 {
        let dx = self.pos[0] - x;
        let dy = self.pos[1] - y;
        let dz = self.pos[2] - z;
        dx * dx + dy * dy + dz * dz
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MobKind {
    Spider,
    Zombie,
    Skeleton,
    Creeper,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimalKind {
    Sheep,
    Pig,
    Chicken,
    Cow,
}

/// Mob body size (width, height): Java defaults 0.6x1.8 (Entity.java:36-37);
/// only spider/zombie override. Creeper keeps the default 1.8 height.
pub fn mob_dims(kind: MobKind) -> (f32, f32) {
    match kind {
        MobKind::Zombie => (0.6, 1.8),
        MobKind::Skeleton => (0.6, 1.8),
        MobKind::Spider => (1.4, 0.9),
        MobKind::Creeper => (0.6, 1.8),
    }
}

/// Base move speed: living default 0.7 (EntityLiving.java:57); zombie 0.5,
/// spider 0.8; skeleton/creeper keep 0.7.
pub fn mob_base_speed(kind: MobKind) -> f32 {
    match kind {
        MobKind::Zombie => 0.5,
        MobKind::Skeleton => 0.7,
        MobKind::Spider => 0.8,
        MobKind::Creeper => 0.7,
    }
}

/// Attack reach mirroring `getAttackReach` (skeleton shoots at 10, the
/// rest melee at 2.5). Also drives the chase-speed switch in `updateAI`.
pub fn mob_attack_reach(kind: MobKind) -> f32 {
    match kind {
        MobKind::Skeleton => 10.0,
        _ => 2.5,
    }
}

/// Daylight combustion mirroring `burnsInDaylight` (zombies and skeletons
/// only; spiders gate aggro on brightness instead, creepers ignore it).
pub fn mob_burns_in_daylight(kind: MobKind) -> bool {
    matches!(kind, MobKind::Zombie | MobKind::Skeleton)
}

/// Animal body size mirroring the C++ constructors (pig 0.9x0.9, sheep and
/// cow 0.9x1.3, chicken 0.3x0.4).
pub fn animal_dims(kind: AnimalKind) -> (f32, f32) {
    match kind {
        AnimalKind::Pig => (0.9, 0.9),
        AnimalKind::Sheep => (0.9, 1.3),
        AnimalKind::Cow => (0.9, 1.3),
        AnimalKind::Chicken => (0.3, 0.4),
    }
}
/// Mob network type ids (mirrors `getMobTypeId`).
pub fn mob_type_id(kind: MobKind) -> u8 {
    match kind {
        MobKind::Spider => 52,
        MobKind::Zombie => 54,
        MobKind::Skeleton => 51,
        MobKind::Creeper => 50,
    }
}

/// Animal network type ids (mirrors the pig-90/sheep-91/cow-92/chicken-93 ids).
pub fn animal_type_id(kind: AnimalKind) -> u8 {
    match kind {
        AnimalKind::Pig => 90,
        AnimalKind::Sheep => 91,
        AnimalKind::Cow => 92,
        AnimalKind::Chicken => 93,
    }
}

#[derive(Clone, Debug)]
pub struct LivingBody {
    pub body: Body,
    pub health: i16,
    pub max_health: i16,
    pub hurt_time: i32,
    pub death_time: i32,
    pub attack_time: i32,
    pub hurt_resist: i32,
    pub max_hurt_resist: i32,
    pub last_damage: i32,
    pub move_speed: f32,
    pub sneaking: bool,
    pub jumping: bool,
}

impl LivingBody {
    pub fn new(id: EntityId, width: f32, height: f32, y_offset: f32) -> Self {
        Self {
            body: Body::new(id, width, height, y_offset),
            health: 20,
            max_health: 20,
            hurt_time: 0,
            death_time: 0,
            attack_time: 0,
            hurt_resist: 0,
            max_hurt_resist: 20,
            last_damage: 0,
            move_speed: 0.7,
            sneaking: false,
            jumping: false,
        }
    }

    pub fn alive(&self) -> bool {
        !self.body.dead
    }
}

#[derive(Clone, Debug)]
pub struct ItemEnt {
    pub body: Body,
    pub item_id: i32,
    pub count: i32,
    pub damage: i32,
    pub age: i32,
    pub pickup_delay: i32,
}

#[derive(Clone, Debug)]
pub struct ArrowEnt {
    pub body: Body,
    pub in_ground: bool,
    pub shake: i32,
    pub ticks_in_ground: i32,
    pub ticks_in_air: i32,
    pub shooter_id: EntityId,
    /// Stuck-block cell (`tileX/Y/Z`) and the block id it lodged in
    /// (`inTile`); the arrow pops out when the cell changes.
    pub tile: [i32; 3],
    pub in_tile: i32,
}

#[derive(Clone, Debug)]
pub struct BoatEnt {
    pub body: Body,
    pub time_since_hit: i32,
    pub damage_taken: i32,
    pub forward_dir: i32,
}

#[derive(Clone, Debug)]
pub struct FallingEnt {
    pub body: Body,
    pub block_id: i32,
    pub fall_time: i32,
}

#[derive(Clone, Debug)]
pub struct MobEnt {
    pub living: LivingBody,
    pub kind: MobKind,
    pub target: Option<EntityId>,
    pub attack_cooldown: i32,
    pub target_timer: i32,
    pub burn_ticks: i32,
    /// Despawn age (mirrors `field_9132_bn`): ++ per tick, dead past
    /// 128 blocks or (600 + 1/800 roll) past 32 blocks.
    pub age: i32,
    pub path: Vec<[i32; 3]>,
    pub path_index: usize,
    /// Creeper fuse state (`swellTime_` / `swellDirection_`): counts up
    /// while the target stays close, explodes at 30.
    pub swell_time: i32,
    pub swell_dir: i32,
}

impl MobEnt {
    /// Fresh mob row with C++ constructor dims/speed and zeroed AI state
    /// (`targetRefreshTime_` starts at 0 so the first tick acquires).
    pub fn new(id: EntityId, kind: MobKind) -> Self {
        let (w, h) = mob_dims(kind);
        let mut living = LivingBody::new(id, w, h, 0.0);
        living.move_speed = mob_base_speed(kind);
        living.body.step_height = 0.5; // EntityLiving ctor (players stay 0.0 like EntityPlayerMP)
        living.max_hurt_resist = 20; // EntityLiving.java:6 (mobs never change it)
        Self {
            living,
            kind,
            target: None,
            attack_cooldown: 0,
            target_timer: 0,
            burn_ticks: 0,
            age: 0,
            path: Vec::new(),
            path_index: 0,
            swell_time: 0,
            swell_dir: -1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnimalEnt {
    pub living: LivingBody,
    pub kind: AnimalKind,
    pub sheared: bool,
    /// Pig saddle (mirrors `EntityPig::saddled`, NBT `Saddle`).
    pub saddled: bool,
    pub egg_timer: i32,
    /// Despawn age, same rules as mobs (vanilla counts animals too).
    pub age: i32,
    pub path: Vec<[i32; 3]>,
    pub path_index: usize,
}

impl AnimalEnt {
    /// Fresh animal row with C++ constructor dims. The chicken egg timer
    /// defaults to the 6000 floor; the spawner (or tests) adds the
    /// `rngNextInt(6000)` roll, mirroring the C++ constructor draw.
    pub fn new(id: EntityId, kind: AnimalKind) -> Self {
        let (w, h) = animal_dims(kind);
        let mut living = LivingBody::new(id, w, h, 0.0);
        living.body.step_height = 0.5; // EntityLiving ctor (players stay 0.0 like EntityPlayerMP)
        living.max_hurt_resist = 20; // EntityLiving.java:6 (animals never change it)
        // Java EntityLiving:28 — default 10 HP; EntityMobs raises to 20,
        // EntityAnimals leaves it, EntityChicken sets 4.
        // The old code gave every animal 20 (2x hits to kill).
        if kind == AnimalKind::Chicken {
            living.health = 4;
            living.max_health = 4;
        } else {
            living.health = 10;
            living.max_health = 10;
        }
        Self {
            living,
            kind,
            sheared: false,
            saddled: false,
            egg_timer: 6000,
            age: 0,
            path: Vec::new(),
            path_index: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct PlayerEnt {
    pub living: LivingBody,
    pub username: String,
    pub score: i32,
    pub inventory: PlayerInventory,
    /// Ticks of post-respawn immunity (mirrors
    /// `respawnInvulnerabilityTicks`, fresh spawns start at 60).
    pub respawn_ticks: i32,
    /// Fractional armor carry (mirrors `armorDamageCarry`).
    pub armor_carry: i32,
    /// Selected held-item id (mirrors `savedHeldItemId`): written by the
    /// server on logout/save from the session, read back on login to
    /// restore the selection. Never touched by simulation ticks.
    pub held_item_id: i32,
}

/// Native player inventory (mirrors `InventoryPlayer`: 36 main, 4 armor,
/// 4 crafting slots plus the held-slot index). Empty is `None`, matching
/// the C++ null `unique_ptr` slots.
#[derive(Clone, Debug)]
pub struct PlayerInventory {
    pub main: [Option<crate::inventory::ItemStack>; 36],
    pub armor: [Option<crate::inventory::ItemStack>; 4],
    pub crafting: [Option<crate::inventory::ItemStack>; 4],
    pub current: i32,
}

impl Default for PlayerInventory {
    fn default() -> Self {
        Self {
            main: [None; 36],
            armor: [None; 4],
            crafting: [None; 4],
            current: 0,
        }
    }
}

impl PlayerInventory {
    /// Held stack (`getCurrentItem`): `None` out of range or empty.
    pub fn held(&self) -> Option<crate::inventory::ItemStack> {
        if self.current >= 0 && (self.current as usize) < self.main.len() {
            self.main[self.current as usize]
        } else {
            None
        }
    }
}

impl PlayerEnt {
    /// Fresh player row (mirrors the `EntityPlayer` ctor dims with an
    /// empty inventory and full respawn immunity).
    pub fn new(id: EntityId, username: &str) -> Self {
        Self {
            living: LivingBody::new(id, 0.6, 1.8, 0.0),
            username: username.to_string(),
            score: 0,
            inventory: PlayerInventory::default(),
            respawn_ticks: 60,
            armor_carry: 0,
            held_item_id: 0,
        }
    }
}

#[derive(Clone, Debug)]
// Player carries the full inventory; entity counts are small and the table
// is an arena, so inline storage beats pointer chasing. Revisit with data.
#[allow(clippy::large_enum_variant)]
pub enum Entity {
    Item(ItemEnt),
    Arrow(ArrowEnt),
    Boat(BoatEnt),
    Falling(FallingEnt),
    Mob(MobEnt),
    Animal(AnimalEnt),
    Player(PlayerEnt),
}

impl Entity {
    pub fn body(&self) -> &Body {
        match self {
            Entity::Item(e) => &e.body,
            Entity::Arrow(e) => &e.body,
            Entity::Boat(e) => &e.body,
            Entity::Falling(e) => &e.body,
            Entity::Mob(e) => &e.living.body,
            Entity::Animal(e) => &e.living.body,
            Entity::Player(e) => &e.living.body,
        }
    }

    pub fn body_mut(&mut self) -> &mut Body {
        match self {
            Entity::Item(e) => &mut e.body,
            Entity::Arrow(e) => &mut e.body,
            Entity::Boat(e) => &mut e.body,
            Entity::Falling(e) => &mut e.body,
            Entity::Mob(e) => &mut e.living.body,
            Entity::Animal(e) => &mut e.living.body,
            Entity::Player(e) => &mut e.living.body,
        }
    }

    pub fn id(&self) -> EntityId {
        self.body().id
    }

    pub fn is_living(&self) -> bool {
        matches!(self, Entity::Mob(_) | Entity::Animal(_) | Entity::Player(_))
    }

    /// Mounted-vehicle eye height for riders (mirrors `getMountedYOffset`;
    /// boats override to -0.3, spiders to `height * 0.75 - 0.5`).
    pub fn mounted_y_offset(&self) -> f64 {
        match self {
            Entity::Boat(_) => -0.3,
            Entity::Mob(m) if m.kind == MobKind::Spider => self.body().height as f64 * 0.75 - 0.5,
            _ => self.body().height as f64 * 0.75,
        }
    }
}

/// Entity table keyed by id (mirrors the C++ `entities_` list +
/// `getEntityById`, without pointer invalidation).
#[derive(Clone, Debug, Default)]
pub struct EntityTable {
    rows: std::collections::HashMap<EntityId, Entity>,
    next_id: EntityId,
}

impl EntityTable {
    pub fn new() -> Self {
        Self { rows: std::collections::HashMap::new(), next_id: 1 }
    }

    pub fn alloc_id(&mut self) -> EntityId {
        next_id(&mut self.next_id)
    }

    pub fn insert(&mut self, entity: Entity) -> EntityId {
        let id = entity.id();
        self.rows.insert(id, entity);
        id
    }

    pub fn get(&self, id: EntityId) -> Option<&Entity> {
        self.rows.get(&id)
    }

    pub fn get_mut(&mut self, id: EntityId) -> Option<&mut Entity> {
        self.rows.get_mut(&id)
    }

    pub fn remove(&mut self, id: EntityId) -> Option<Entity> {
        self.rows.remove(&id)
    }

    /// Drop dead rows (mirrors the end-of-tick erase); returns the purged
    /// ids sorted for determinism. Dead players are spared: like vanilla,
    /// the player row survives death for the respawn packet and is only
    /// dropped explicitly on logout.
    pub fn purge_dead(&mut self) -> Vec<EntityId> {
        // Vanilla EntityLiving: death_time ticks to 20 before setEntityDead
        // ONLY when health <= 0 (damage death playing status-3 animation).
        // When setEntityDead is called directly with health > 0 (creeper
        // explosion, despawn, peaceful clear), remove on the same tick.
        for e in self.rows.values_mut() {
            let dead = e.body().dead;
            if !dead {
                continue;
            }
            match e {
                Entity::Mob(m)
                    if m.living.health <= 0 && m.living.death_time < 20 => {
                        m.living.death_time += 1;
                    }
                Entity::Animal(a)
                    if a.living.health <= 0 && a.living.death_time < 20 => {
                        a.living.death_time += 1;
                    }
                _ => {}
            }
        }
        let mut dead: Vec<EntityId> = self
            .rows
            .iter()
            .filter(|(_, e)| {
                if !e.body().dead || matches!(e, Entity::Player(_)) {
                    return false;
                }
                match e {
                    Entity::Mob(m) => m.living.health > 0 || m.living.death_time >= 20,
                    Entity::Animal(a) => a.living.health > 0 || a.living.death_time >= 20,
                    // Items/arrows/boats/falling have no death_time: purge now.
                    _ => true,
                }
            })
            .map(|(id, _)| *id)
            .collect();
        dead.sort_unstable();
        for id in &dead {
            self.rows.remove(id);
        }
        dead
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, EntityId, Entity> {
        self.rows.iter()
    }

    pub fn alive_ids(&self) -> Vec<EntityId> {
        let mut out = Vec::with_capacity(self.rows.len());
        self.collect_alive_ids(&mut out);
        out
    }

    /// Reuses a caller-provided buffer to collect alive entity ids without heap allocation.
    pub fn collect_alive_ids(&self, out: &mut Vec<EntityId>) {
        out.clear();
        for (&id, e) in &self.rows {
            if !e.body().dead {
                out.push(id);
            }
        }
    }

    /// Every row id, dead or not (spawn anchors count dead players like
    /// the C++ gather).
    pub fn all_ids(&self) -> Vec<EntityId> {
        let mut out = Vec::with_capacity(self.rows.len());
        self.collect_all_ids(&mut out);
        out
    }

    /// Reuses a caller-provided buffer to collect all entity ids without heap allocation.
    pub fn collect_all_ids(&self, out: &mut Vec<EntityId>) {
        out.clear();
        for &id in self.rows.keys() {
            out.push(id);
        }
    }

    pub fn count_mobs(&self) -> usize {
        self.rows.values().filter(|e| matches!(e, Entity::Mob(m) if !m.living.body.dead)).count()
    }

    pub fn count_animals(&self) -> usize {
        self.rows.values().filter(|e| matches!(e, Entity::Animal(a) if !a.living.body.dead)).count()
    }

    /// Mount `rider` onto `vehicle` (`None` dismounts), mirroring
    /// `Entity::mountEntity` exactly: re-mounting the same vehicle
    /// dismounts, stale reverse links are cleared, and a vehicle with a
    /// different live rider evicts it first. Dangling ids count as null
    /// (like the C++ failed lookups). One deliberate hardening: mounting a
    /// missing vehicle sets the rider link but skips the vehicle side
    /// instead of crashing on a null dereference.
    pub fn mount(&mut self, rider_id: EntityId, vehicle: Option<EntityId>) {
        // Resolve current (dangling counts as none, like C++ nullptr).
        let raw = self.get(rider_id).map(|e| e.body().riding);
        let current = raw.filter(|id| *id == NO_ENTITY || self.rows.contains_key(id));
        if current == vehicle {
            if let Some(vid) = current {
                if self.get(vid).map(|e| e.body().ridden_by) == Some(rider_id) {
                    if let Some(v) = self.get_mut(vid) {
                        v.body_mut().ridden_by = NO_ENTITY;
                    }
                }
            }
            if let Some(r) = self.get_mut(rider_id) {
                r.body_mut().riding = NO_ENTITY;
            }
            return;
        }
        if let Some(cur) = current {
            if self.get(cur).map(|e| e.body().ridden_by) == Some(rider_id) {
                if let Some(v) = self.get_mut(cur) {
                    v.body_mut().ridden_by = NO_ENTITY;
                }
            }
        }
        if let Some(vid) = vehicle {
            let occupant = self.get(vid).map(|e| e.body().ridden_by).unwrap_or(NO_ENTITY);
            if occupant >= 0 && occupant != rider_id {
                if let Some(o) = self.get_mut(occupant) {
                    o.body_mut().riding = NO_ENTITY;
                }
            }
            if let Some(r) = self.get_mut(rider_id) {
                r.body_mut().riding = vid;
            }
            if let Some(v) = self.get_mut(vid) {
                v.body_mut().ridden_by = rider_id;
            }
        } else if let Some(r) = self.get_mut(rider_id) {
            r.body_mut().riding = NO_ENTITY;
        }
    }

    /// Base tick for one row (mirrors `Entity::tick`): drop links to dead
    /// partners and sync previous/tracker state. Environmental scans and
    /// kind ticks arrive with the native world.
    pub fn tick_base(&mut self, id: EntityId) {
        let (riding_dead, ridden_dead) = match self.get(id) {
            Some(e) => {
                let b = e.body();
                let rd = b.riding >= 0
                    && self.get(b.riding).map(|v| v.body().dead).unwrap_or(true);
                let rb = b.ridden_by >= 0
                    && self.get(b.ridden_by).map(|v| v.body().dead).unwrap_or(true);
                (rd, rb)
            }
            None => return,
        };
        if let Some(e) = self.get_mut(id) {
            let is_living = e.is_living();
            let b = e.body_mut();
            if riding_dead {
                b.riding = NO_ENTITY;
            }
            if ridden_dead {
                b.ridden_by = NO_ENTITY;
            }
            b.prev_pos = b.pos;
            b.prev_yaw = b.yaw;
            b.prev_pitch = b.pitch;
            b.track_pos = b.pos;
            b.track_yaw = b.yaw;
            b.track_pitch = b.pitch;
            if !is_living && b.pos[1] < -64.0 {
                b.dead = true;
            }
        }
    }

    /// Rider placement (mirrors `Entity::updateRiderPosition`).
    pub fn update_rider_position(&mut self, id: EntityId) {
        let rider = match self.get(id) {
            Some(e) => e.body().ridden_by,
            None => return,
        };
        if rider < 0 {
            return;
        }
        let (px, py, pz, off) = match self.get(id) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2], e.mounted_y_offset()),
            None => return,
        };
        if let Some(r) = self.get_mut(rider) {
            r.body_mut().set_position(px, py + off, pz);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boat(table: &mut EntityTable) -> EntityId {
        let id = table.alloc_id();
        let mut b = Body::new(id, 1.5, 0.6, 0.3);
        b.set_position(10.0, 64.0, 10.0);
        table.insert(Entity::Boat(BoatEnt { body: b, time_since_hit: 0, damage_taken: 0, forward_dir: 1 }));
        id
    }

    fn player(table: &mut EntityTable, name: &str) -> EntityId {
        let id = table.alloc_id();
        let mut p = PlayerEnt::new(id, name);
        p.living.body.set_position(11.0, 64.0, 10.0);
        table.insert(Entity::Player(p));
        id
    }

    #[test]
    fn test_ids_unique_and_lookup() {
        let mut t = EntityTable::new();
        let a = boat(&mut t);
        let b = player(&mut t, "steve");
        assert_ne!(a, b);
        assert_eq!(t.len(), 2);
        assert!(t.get(9999).is_none());
        assert_eq!(t.get(b).unwrap().id(), b);
    }

    #[test]
    fn test_mount_and_dismount() {
        let mut t = EntityTable::new();
        let b = boat(&mut t);
        let p = player(&mut t, "steve");
        t.mount(p, Some(b));
        assert_eq!(t.get(p).unwrap().body().riding, b);
        assert_eq!(t.get(b).unwrap().body().ridden_by, p);
        // Same vehicle again dismounts both sides.
        t.mount(p, Some(b));
        assert_eq!(t.get(p).unwrap().body().riding, NO_ENTITY);
        assert_eq!(t.get(b).unwrap().body().ridden_by, NO_ENTITY);
        // Explicit dismount of a free rider is a no-op.
        t.mount(p, None);
        assert_eq!(t.get(p).unwrap().body().riding, NO_ENTITY);
    }

    #[test]
    fn test_mount_evicts_previous_rider() {
        let mut t = EntityTable::new();
        let b = boat(&mut t);
        let p1 = player(&mut t, "a");
        let p2 = player(&mut t, "b");
        t.mount(p1, Some(b));
        t.mount(p2, Some(b));
        assert_eq!(t.get(b).unwrap().body().ridden_by, p2);
        assert_eq!(t.get(p1).unwrap().body().riding, NO_ENTITY);
        assert_eq!(t.get(p2).unwrap().body().riding, b);
    }

    #[test]
    fn test_mount_missing_rows_safe() {
        let mut t = EntityTable::new();
        let p = player(&mut t, "steve");
        t.mount(p, Some(4242)); // no such vehicle: rider link set, nothing else
        assert_eq!(t.get(p).unwrap().body().riding, 4242);
        t.mount(4243, Some(p)); // no such rider: no crash
        t.mount(4243, None);
    }

    #[test]
    fn test_tick_base_clears_dead_links_and_syncs() {
        let mut t = EntityTable::new();
        let b = boat(&mut t);
        let p = player(&mut t, "steve");
        t.mount(p, Some(b));
        // Kill the boat: the rider link must drop on tick.
        t.get_mut(b).unwrap().body_mut().dead = true;
        t.tick_base(p);
        assert_eq!(t.get(p).unwrap().body().riding, NO_ENTITY);
        let b = t.get(p).unwrap().body().clone();
        assert_eq!((b.prev_pos, b.track_pos), (b.pos, b.pos));
    }

    #[test]
    fn test_rider_position_boat_offset() {
        let mut t = EntityTable::new();
        let b = boat(&mut t);
        let p = player(&mut t, "steve");
        t.mount(p, Some(b));
        t.update_rider_position(b);
        let rp = t.get(p).unwrap().body().pos;
        assert!((rp[0] - 10.0).abs() < 1e-9);
        assert!((rp[1] - (64.0 - 0.3)).abs() < 1e-9);
    }

    #[test]
    fn test_type_ids_match_java() {
        assert_eq!(mob_type_id(MobKind::Spider), 52);
        assert_eq!(mob_type_id(MobKind::Zombie), 54);
        assert_eq!(mob_type_id(MobKind::Skeleton), 51);
        assert_eq!(mob_type_id(MobKind::Creeper), 50);
        assert_eq!(animal_type_id(AnimalKind::Pig), 90);
        assert_eq!(animal_type_id(AnimalKind::Sheep), 91);
        assert_eq!(animal_type_id(AnimalKind::Cow), 92);
        assert_eq!(animal_type_id(AnimalKind::Chicken), 93);
    }
}
