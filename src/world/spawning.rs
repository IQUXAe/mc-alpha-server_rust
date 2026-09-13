//! Spawn fitness, hostile/passive spawn passes and the world tick.
//! Split out of `world.rs`; behavior unchanged. `World` implements the
//! `mob_spawning::SpawnerWorld` trait directly with explicit borrows.
//!

use crate::entity::table::{AnimalKind, Entity, EntityId, MobKind};
use crate::math_helper::floor_double;
use crate::world::tiles::TileData;
use crate::world::{is_air_material, World, GRASS_BLOCK_ID, WORLD_HEIGHT};

impl crate::mob_spawning::SpawnerWorld for World {
    fn spawn_next_int(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        self.rng.next_int_bound(bound)
    }

    fn spawn_next_float(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.rng.next_float()
    }

    fn spawn_chunk_exists(&mut self, x: i32, z: i32) -> bool {
        self.has_chunk(x, z)
    }

    fn spawn_is_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.is_solid(x, y, z)
    }

    fn spawn_is_air(&mut self, x: i32, y: i32, z: i32) -> bool {
        is_air_material(self.get_block_id(x, y, z))
    }

    fn spawn_is_liquid(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.material_at(x, y, z).is_liquid()
    }

    fn spawn_try_spawn(
        &mut self,
        hostile: bool,
        kind: u8,
        fx: f32,
        fy: f32,
        fz: f32,
        yaw: f32,
    ) -> Option<(i32, i32)> {
        let id = self.entities.alloc_id();
        if hostile {
            let mkind = match kind {
                0 => MobKind::Spider,
                1 => MobKind::Zombie,
                2 => MobKind::Skeleton,
                _ => MobKind::Creeper,
            };
            let mut m = crate::entity::table::MobEnt::new(id, mkind);
            m.living.body.set_position(fx as f64, fy as f64, fz as f64);
            m.living.body.yaw = yaw;
            self.entities.insert(Entity::Mob(m));
            if !self.spawner_mob_ok(id) {
                self.entities.remove(id);
                return None;
            }
        } else {
            let akind = match kind {
                0 => AnimalKind::Sheep,
                1 => AnimalKind::Pig,
                2 => AnimalKind::Chicken,
                _ => AnimalKind::Cow,
            };
            let mut a = crate::entity::table::AnimalEnt::new(id, akind);
            a.living.body.set_position(fx as f64, fy as f64, fz as f64);
            a.living.body.yaw = yaw;
            // The C++ chicken ctor rolls the egg clock at construction,
            // before the spawn check below (draw consumed even on reject).
            if akind == AnimalKind::Chicken {
                a.egg_timer = 6000 + self.rng.next_int_bound(6000);
            }
            self.entities.insert(Entity::Animal(a));
            if !self.spawner_animal_ok(id) {
                self.entities.remove(id);
                return None;
            }
        }
        Some((id, 4))
    }

    fn spawn_jockey(&mut self, fx: f32, fy: f32, fz: f32, yaw: f32, host_id: i32) -> bool {
        if self.entities.get(host_id).is_none() {
            return false;
        }
        let id = self.entities.alloc_id();
        let mut m = crate::entity::table::MobEnt::new(id, MobKind::Skeleton);
        m.living.body.set_position(fx as f64, fy as f64, fz as f64);
        m.living.body.yaw = yaw;
        self.entities.insert(Entity::Mob(m));
        self.entities.mount(id, Some(host_id));
        true
    }
}

impl World {
    /// Mob spawn fitness (mirrors `EntityMob::getCanSpawnHere`): dark
    /// enough (two unconditional RNG draws like C++), collision-free, and
    /// dry.
    fn spawner_mob_ok(&mut self, id: EntityId) -> bool {
        let (px, min_y, pz, bbox) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().bounding_box.min_y, e.body().pos[2], e.body().bounding_box),
            None => return false,
        };
        let (x, y, z) = (floor_double(px), floor_double(min_y), floor_double(pz));
        if self.saved_light_value(0, x, y, z) as i32 > self.rng.next_int_bound(32) {
            return false;
        }
        if self.block_light_value(x, y, z) as i32 > self.rng.next_int_bound(8) {
            return false;
        }
        self.colliding_boxes(&bbox).is_empty() && !self.touching_liquid(id)
    }

    /// Animal spawn fitness (mirrors `EntityAnimals::getCanSpawnHere`):
    /// grass below, bright, collision-free, and dry. No RNG draws.
    fn spawner_animal_ok(&mut self, id: EntityId) -> bool {
        let (px, min_y, pz, bbox) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().bounding_box.min_y, e.body().pos[2], e.body().bounding_box),
            None => return false,
        };
        let (x, y, z) = (floor_double(px), floor_double(min_y), floor_double(pz));
        if self.get_block_id(x, y - 1, z) != GRASS_BLOCK_ID {
            return false;
        }
        if self.block_light_value(x, y, z) <= 8 {
            return false;
        }
        self.colliding_boxes(&bbox).is_empty() && !self.touching_liquid(id)
    }

    /// Player anchor positions for the spawn passes (mirrors
    /// `gatherPlayerPositions`: every joined player, dead or not).
    fn spawn_anchors(&self) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
        let mut rows: Vec<(EntityId, [f64; 3])> = Vec::new();
        for oid in self.entities.all_ids() {
            if let Some(Entity::Player(p)) = self.entities.get(oid) {
                rows.push((oid, p.living.body.pos));
            }
        }
        rows.sort_by_key(|(oid, _)| *oid);
        let mut xs = Vec::with_capacity(rows.len());
        let mut ys = Vec::with_capacity(rows.len());
        let mut zs = Vec::with_capacity(rows.len());
        for (_, pos) in rows {
            xs.push(pos[0]);
            ys.push(pos[1]);
            zs.push(pos[2]);
        }
        (xs, ys, zs)
    }

    /// Hostile spawn pass (mirrors `World::spawnHostileMobs`).
    pub fn spawn_hostile_mobs(&mut self) -> i32 {
        if !self.spawn_monsters {
            return 0;
        }
        let (px, py, pz) = self.spawn_anchors();
        let count = self.entities.count_mobs() as i32;
        crate::mob_spawning::spawn_hostile(self, &px, &py, &pz, count, self.spawn, WORLD_HEIGHT)
    }

    /// Passive spawn pass (mirrors `World::spawnPassiveMobs`).
    pub fn spawn_passive_mobs(&mut self) -> i32 {
        if !self.spawn_animals {
            return 0;
        }
        let (px, py, pz) = self.spawn_anchors();
        let count = self.entities.count_animals() as i32;
        crate::mob_spawning::spawn_passive(self, &px, &py, &pz, count, self.spawn, WORLD_HEIGHT)
    }

    /// Item pickup sweep (mirrors the in-loop pickup: ready items within
    /// the expanded player box merge via `player_add_item`; packets are
    /// the network slice's). Two-phase instead of interleaved, which is
    /// equivalent here: fresh drops always carry a pickup delay, and
    /// native players hold still between network ticks.
    pub(crate) fn pickup_items(&mut self) {
        let mut items: Vec<EntityId> = Vec::new();
        for oid in self.entities.alive_ids() {
            if let Some(Entity::Item(e)) = self.entities.get(oid) {
                if e.pickup_delay <= 0 {
                    items.push(oid);
                }
            }
        }
        items.sort_unstable();
        let mut players: Vec<EntityId> = Vec::new();
        for oid in self.entities.alive_ids() {
            if self.target_alive(oid) {
                players.push(oid);
            }
        }
        players.sort_unstable();
        for iid in items {
            if self.entities.get(iid).map(|e| e.body().dead).unwrap_or(true) {
                continue;
            }
            for pid in &players {
                if self.entities.get(iid).map(|e| e.body().dead).unwrap_or(true) {
                    break;
                }
                let hit = match (self.entities.get(iid), self.entities.get(*pid)) {
                    (Some(Entity::Item(it)), Some(Entity::Player(p))) => {
                        let ex = p.living.body.width as f64 / 2.0 + 1.0 + 0.125;
                        let min_y = p.living.body.pos[1] - 0.25;
                        let max_y = p.living.body.pos[1] + p.living.body.height as f64;
                        (p.living.body.pos[0] - it.body.pos[0]).abs() < ex
                            && it.body.pos[1] < max_y
                            && it.body.pos[1] + 0.25 > min_y
                            && (p.living.body.pos[2] - it.body.pos[2]).abs() < ex
                    }
                    _ => false,
                };
                if !hit {
                    continue;
                }
                let (item_id, count, damage) = match self.entities.get(iid) {
                    Some(Entity::Item(e)) => (e.item_id, e.count, e.damage),
                    _ => continue,
                };
                let rem = self.player_add_item(
                    *pid,
                    crate::inventory::ItemStack::new(item_id, count, damage),
                );
                if rem < count {
                    if rem <= 0 {
                        if let Some(e) = self.entities.get_mut(iid) {
                            e.body_mut().dead = true;
                        }
                        self.item_pickups.push((iid, *pid));
                    } else if let Some(Entity::Item(e)) = self.entities.get_mut(iid) {
                        e.count = rem;
                    }
                }
                if self.entities.get(iid).map(|e| e.body().dead).unwrap_or(true) {
                    break;
                }
            }
        }
    }

    /// Server tick (mirrors `World::tick` minus chunk I/O, lighting,
    /// chest/sign-tile behavior, and packets): clock, spawners, furnace
    /// tiles, scheduled and random block ticks, entity dispatch on a
    /// snapshot (mid-tick spawns wait a tick like C++), item pickup,
    /// dead-row purge, and periodic unload.
    ///
    /// Fills [`World::last_tick_stats`] with per-phase wall-clock times
    /// (10 `Instant` reads per tick, ~100ns total — noise against ms ticks).
    pub fn tick_world(&mut self) {
        use std::time::Instant;
        let total = Instant::now();
        self.time += 1;
        let t = Instant::now();
        if self.spawn_monsters {
            self.spawn_hostile_mobs();
        }
        if self.spawn_animals {
            self.spawn_passive_mobs();
        }
        let spawners = t.elapsed();
        let t = Instant::now();
        self.tick_furnaces();
        self.tick_primed_tnt();
        let furnaces = t.elapsed();
        let t = Instant::now();
        self.process_scheduled_ticks();
        let scheduled = t.elapsed();
        let t = Instant::now();
        self.random_block_ticks();
        let random = t.elapsed();
        let t = Instant::now();
        let mut ids = self.entities.alive_ids();
        ids.sort_unstable();
        for &id in &ids {
            if self.entities.get(id).map(|e| e.body().dead).unwrap_or(true) {
                continue;
            }
            match self.entities.get(id) {
                Some(Entity::Item(_)) => self.tick_item(id),
                Some(Entity::Falling(_)) => self.tick_falling(id),
                Some(Entity::Boat(_)) => self.tick_boat(id),
                Some(Entity::Arrow(_)) => self.tick_arrow(id),
                Some(Entity::Mob(_)) => self.tick_mob(id, &ids),
                Some(Entity::Animal(_)) => self.tick_animal(id, &ids),
                Some(Entity::Player(_)) => self.tick_player(id),
                None => {}
            }
        }
        let entities = t.elapsed();
        let t = Instant::now();
        self.pickup_items();
        self.entities.purge_dead();
        self.unload_chunks();
        let pickup = t.elapsed();
        let t = Instant::now();
        self.refresh_light();
        let light = t.elapsed();
        self.last_tick_stats = crate::world::TickStats {
            spawners_us: spawners.as_micros() as u64,
            furnaces_us: furnaces.as_micros() as u64,
            scheduled_us: scheduled.as_micros() as u64,
            random_us: random.as_micros() as u64,
            entities_us: entities.as_micros() as u64,
            pickup_us: pickup.as_micros() as u64,
            light_us: light.as_micros() as u64,
            total_us: total.elapsed().as_micros() as u64,
        };
    }

    /// Native furnace ticking (mirrors the `World::tick` tile-entity pass
    /// over `TileEntityFurnace::updateEntity`): every furnace tile looks
    /// up fuel from its own slot and runs the shared core, then a burn
    /// flip swaps the block 61 <-> 62 preserving metadata (mirrors
    /// `updateFurnaceBlockState`, whose no-notify set keeps the tile
    /// alive — here tiles live outside chunks, so any plain set is safe).
    /// The id/meta writes queue the cell in `block_updates` for the
    /// server tick to broadcast.
    pub fn tick_furnaces(&mut self) {
        let cells: Vec<(i32, i32, i32)> = self.tiles.keys().copied().collect();
        for (x, y, z) in cells {
            let ticked = match self.tiles.get_mut(&(x, y, z)) {
                Some(TileData::Furnace(state)) => {
                    crate::tile_entity::furnace::furnace_tick_native(state)
                }
                _ => continue,
            };
            if !ticked.needs_block_update {
                continue;
            }
            let burning = matches!(
                self.tiles.get(&(x, y, z)),
                Some(TileData::Furnace(state)) if state.burn_time > 0
            );
            let meta = self.get_block_meta(x, y, z);
            let new_id = if burning { 62 } else { 61 };
            if self.set_block_id(x, y, z, new_id) {
                self.set_block_meta(x, y, z, meta);
            }
        }
    }
}
