//! Block placement, neighbor updates, drops and tick scheduling on [`World`].
//! Split out of `world.rs`; behavior unchanged.

use crate::block::table::block_properties_get;
use crate::block::fire::{block_fire_added, block_fire_neighbor, block_fire_tick};
use crate::block::ticks::{
     block_base_drop, block_cactus_added, block_cactus_neighbor, block_cactus_tick,
     block_crops_added, block_crops_neighbor, block_crops_tick, block_flower_neighbor,
     block_flower_tick, block_fluid_added, block_fluid_neighbor, block_fluid_tick,
     block_leaves_added, block_leaves_neighbor, block_leaves_tick, block_mushroom_neighbor,
     block_reed_added, block_reed_neighbor, block_reed_tick, block_sand_added, block_sand_neighbor,
     block_sand_tick, block_sapling_added, block_sapling_neighbor, block_sapling_tick,
     block_soil_added, block_soil_neighbor, block_soil_tick, block_torch_added, block_torch_neighbor,
};
use crate::entity::table::{Body, Entity, EntityId};
use crate::material::Material;
use crate::world::{
    TileData, WORLD_HEIGHT, World, animal_kind_of, animal_string_id, is_air_material, material_of,
    mob_kind_of, mob_string_id, pending_creature,
};

/// Alpha wheat/seeds item ids for the crops drivers.
const WHEAT_ITEM_ID: i32 = 296;
const SEEDS_ITEM_ID: i32 = 295;
/// Sign drop item id (BlockSign::idDropped).
const SIGN_ITEM_ID: i32 = 323;

impl World {
    /// (drop_id, drop_count, drop_damage) mirroring the Java idDropped /
    /// quantityDropped call sites. No damageDropped overrides exist, so
    /// damage is always 0.
    pub(crate) fn native_drop_ids(bid: u8) -> (i32, i32, i32) {
        let p = block_properties_get(bid as u32);
        (if p.id_dropped != 0 { p.id_dropped } else { bid as i32 }, p.quantity_dropped, 0)
    }

    /// Placement write (mirrors `setBlockWithNotify`): run the removal hook
    /// for the old id (container scatter + tile clear, Java
    /// `onBlockRemoval`), then set, run the added-router, notify neighbors.
    pub(crate) fn apply_set_notify(&mut self, x: i32, y: i32, z: i32, id: u8) -> bool {
        let old = self.get_block_id(x, y, z);
        if old != 0 && old != id {
            self.block_removed(x, y, z, old);
        }
        if !self.set_block_id(x, y, z, id) {
            return false;
        }
        self.block_added(x, y, z, id);
        self.notify_neighbors_of(x, y, z);
        true
    }

    /// Id+meta placement write (mirrors `setBlockAndMetadataWithNotify`
    /// the same way).
    pub(crate) fn apply_set_meta_notify(&mut self, x: i32, y: i32, z: i32, id: u8, meta: u8) -> bool {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return false;
        }
        let old = self.get_block_id(x, y, z);
        if old != 0 && old != id {
            self.block_removed(x, y, z, old);
        }
        self.set_block_id(x, y, z, id);
        self.set_block_meta(x, y, z, meta);
        self.block_added(x, y, z, id);
        self.notify_neighbors_of(x, y, z);
        true
    }

    /// Removal hook (mirrors `Block.onBlockRemoval` for containers): scatter
    /// chest/furnace contents, drop the tile row. Safe to call when no tile
    /// exists (scatter is a no-op then).
    fn block_removed(&mut self, x: i32, y: i32, z: i32, old: u8) {
        if matches!(old, 54 | 61 | 62 | 63 | 68) {
            self.scatter_container_tile(x, y, z);
            self.tiles.remove(&(x, y, z));
        } else {
            self.tiles.remove(&(x, y, z));
        }
    }

    /// Neighbor fan-out (mirrors `notifyBlocksOfNeighborChange`).
    pub(crate) fn notify_neighbors_of(&mut self, x: i32, y: i32, z: i32) {
        const OFF: [[i32; 3]; 6] =
            [[-1, 0, 0], [1, 0, 0], [0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1]];
        for o in OFF {
            self.neighbor_changed(x + o[0], y + o[1], z + o[2]);
        }
    }

    /// Placement router (mirrors the `onBlockAdded` overrides, including
    /// container tile creation).
    fn block_added(&mut self, x: i32, y: i32, z: i32, bid: u8) {
        match bid {
            54 => {
                self.tiles.entry((x, y, z)).or_insert_with(|| {
                    TileData::Chest(crate::tile_entity::chest::chest_create())
                });
            }
            61 | 62 => {
                self.tiles.entry((x, y, z)).or_insert_with(|| {
                    TileData::Furnace(crate::tile_entity::furnace::furnace_create())
                });
            }
            63 | 68 => {
                self.tiles.entry((x, y, z)).or_insert_with(|| {
                    TileData::Sign(crate::tile_entity::sign::sign_create())
                });
            }
            _ => {}
        }
        match bid {
            12 | 13 => block_sand_added(&mut *self, bid, x, y, z),
            8..=11 => {
                let rate = if self.material_at(x, y, z) == Material::LAVA {
                    30
                } else {
                    5
                };
                block_fluid_added(&mut *self, bid, rate, x, y, z);
                self.fluid_lava_contact(x, y, z, bid);
            }
            81 => block_cactus_added(&mut *self, bid, x, y, z),
            83 => block_reed_added(&mut *self, bid, x, y, z),
            50 => block_torch_added(&mut *self, bid, x, y, z),
            18 => block_leaves_added(&mut *self, bid, x, y, z),
            6 => block_sapling_added(&mut *self, bid, x, y, z),
            59 => block_crops_added(&mut *self, bid, x, y, z),
            60 => block_soil_added(&mut *self, bid, x, y, z),
            51 => block_fire_added(&mut *self, bid, 10, x, y, z),
            _ => {}
        }
    }

    /// Neighbor router (mirrors the `onNeighborBlockChange` overrides,
    /// plus the inline sign-break check).
    pub(crate) fn neighbor_changed(&mut self, x: i32, y: i32, z: i32) {
        let bid = self.get_block_id(x, y, z);
        if bid == 0 || !Self::native_registered(bid) {
            return;
        }
        let _meta = self.get_block_meta(x, y, z);
        match bid {
            12 | 13 => block_sand_neighbor(&mut *self, bid, x, y, z),
            8..=11 => {
                let rate = if self.material_at(x, y, z) == Material::LAVA {
                    30
                } else {
                    5
                };
                block_fluid_neighbor(&mut *self, bid, rate, x, y, z);
                self.fluid_lava_contact(x, y, z, bid);
            }
            37 | 38 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_flower_neighbor(&mut *self, d, q, g, x, y, z);
            }
            39 | 40 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_mushroom_neighbor(&mut *self, d, q, g, x, y, z);
            }
            50 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_torch_neighbor(&mut *self, d, q, g, x, y, z);
            }
            78 => self.snow_neighbor(x, y, z),
            81 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_cactus_neighbor(&mut *self, bid, d, q, g, x, y, z);
            }
            83 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_reed_neighbor(&mut *self, bid, d, q, g, x, y, z);
            }
            18 => {
                let mut guard = self.leaves_guard;
                block_leaves_neighbor(&mut *self, bid, bid, &mut guard, x, y, z);
                self.leaves_guard = guard;
            }
            6 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_sapling_neighbor(&mut *self, bid, d, q, g, x, y, z);
            }
            59 => block_crops_neighbor(&mut *self, bid, bid, WHEAT_ITEM_ID, SEEDS_ITEM_ID, x, y, z),
            60 => block_soil_neighbor(&mut *self, bid, x, y, z),
            51 => block_fire_neighbor(&mut *self, x, y, z),
            63 | 68 => {}
            _ => {}
        }
        if bid == 63 || bid == 68 {
            self.sign_neighbor(x, y, z, bid);
        }
    }

    /// Snow-layer support (mirrors `BlockSnow.func_275_g`): needs a solid
    /// attachable block below, else drops and vanishes.
    fn snow_neighbor(&mut self, x: i32, y: i32, z: i32) {
        let below = self.get_block_id(x, y - 1, z);
        let ok = below != 0
            && block_properties_get(below as u32).allows_attachment
            && material_of(block_properties_get(below as u32).material).is_solid();
        if !ok {
            self.drop_block_for(78, 0, x, y, z);
            self.apply_set_notify(x, y, z, 0);
        }
    }

    /// Registered-block check (mirrors `blocksList[id] != nullptr` via the
    /// initBlocks rule: every non-air material gets an instance).
    pub(crate) fn native_registered(bid: u8) -> bool {
        bid != 0 && !is_air_material(bid)
    }

    /// Block-as-item drop at chance 1.0 (mirrors `dropBlockAsItem` for an
    /// already-captured block id — the cell may be air by now, so the
    /// pre-removal metadata rides along for meta-sensitive drops).
    pub(crate) fn drop_block_for(&mut self, bid: u8, meta: u8, x: i32, y: i32, z: i32) {
        if bid == 0 {
            return;
        }
        // Player harvest of crops (BlockCrops.onBlockDestroyedByPlayer):
        // wheat when mature + 3 seed rolls. Multi-drop, so spawn here.
        if bid == 59 {
            if meta >= 7 {
                block_base_drop(&mut *self, 296, 1, 0, x, y, z, 1.0);
            }
            for _ in 0..3 {
                // Draw from world RNG to keep the stream stable.
                let r = self.rng.next_int_bound(15);
                if r <= meta as i32 {
                    block_base_drop(&mut *self, 295, 1, 0, x, y, z, 1.0);
                }
            }
            return;
        }
        let (drop, qty) = self.rolled_drop_ids(bid, meta);
        if drop <= 0 {
            return;
        }
        // Immature crops drop nothing (rolled returns 0,0) — no seeds here.
        block_base_drop(&mut *self, drop, qty, 0, x, y, z, 1.0);
    }

    /// idDropped/quantityDropped rolls that need world RNG or metadata
    /// (mirrors the Java overrides, which draw from the block-break
    /// Random): doors drop the item only from the lower half (wood 324,
    /// iron 330), gravel flints 1/10, redstone dust comes 4-5, leaves
    /// drop a sapling 1/20, crops drop wheat + seed rolls on player
    /// harvest. Everything else is the props table.
    ///
    /// Crops are special: they drop up to 4 stacks (1 wheat + up to 3
    /// seeds), spawned directly in `drop_block_for`. Here we only report
    /// the wheat part for previews; immature crops report nothing.
    pub(crate) fn rolled_drop_ids(&mut self, bid: u8, meta: u8) -> (i32, i32) {
        match bid {
            59 => {
                if meta >= 7 { (296, 1) } else { (0, 0) }
            }
            64 | 71 => {
                if meta & 8 != 0 {
                    (0, 0)
                } else if bid == 71 {
                    (330, 1)
                } else {
                    (324, 1)
                }
            }
            13 => {
                if self.rng.next_int_bound(10) == 0 {
                    (318, 1)
                } else {
                    (13, 1)
                }
            }
            73 | 74 => (331, 4 + self.rng.next_int_bound(2)),
            18 => {
                if self.rng.next_int_bound(20) == 0 {
                    (6, 1)
                } else {
                    (0, 0)
                }
            }
            _ => {
                let (d, q, _) = Self::native_drop_ids(bid);
                (d, q)
            }
        }
    }

    /// Block-as-item drop for the live occupant (fluid wash path).
    pub(crate) fn drop_block_as_item(&mut self, x: i32, y: i32, z: i32) {
        let bid = self.get_block_id(x, y, z);
        let meta = self.get_block_meta(x, y, z);
        self.drop_block_for(bid, meta, x, y, z);
    }

    /// Container tile scatter on break (mirrors the furnace/chest
    /// `onBlockRemoval` halves, including the tile-row removal).
    /// Tile-less cells are a no-op.
    pub(crate) fn scatter_container_tile(&mut self, x: i32, y: i32, z: i32) {
        use crate::block::container::{block_chest_scatter_stack, block_furnace_scatter_stack};
        let tile = match self.tiles.get(&(x, y, z)) {
            Some(t) => *t,
            None => return,
        };
        match tile {
            TileData::Furnace(s) => {
                for slot in s.slots {
                    if slot.count > 0 {
                        block_furnace_scatter_stack(
                            &mut *self,
                            slot.item_id,
                            slot.count,
                            slot.damage,
                            x,
                            y,
                            z,
                        );
                    }
                }
            }
            TileData::Chest(s) => {
                for slot in s.slots {
                    if slot.count > 0 {
                        block_chest_scatter_stack(
                            &mut *self,
                            slot.item_id,
                            slot.count,
                            slot.damage,
                            x,
                            y,
                            z,
                        );
                    }
                }
            }
            TileData::Sign(_) => {}
        }
        self.tiles.remove(&(x, y, z));
    }

    /// Sign support check (mirrors `BlockSign::onNeighborBlockChange`):
    /// wall signs need material-solid behind per facing, posts need solid
    /// below; otherwise drop 323 through the standard base-drop path and
    /// clear.
    fn sign_neighbor(&mut self, x: i32, y: i32, z: i32, bid: u8) {
        let supported = if bid == 68 {
            match self.get_block_meta(x, y, z) {
                2 => self.material_at(x, y, z + 1).is_solid(),
                3 => self.material_at(x, y, z - 1).is_solid(),
                4 => self.material_at(x + 1, y, z).is_solid(),
                5 => self.material_at(x - 1, y, z).is_solid(),
                _ => false,
            }
        } else {
            self.material_at(x, y - 1, z).is_solid()
        };
        if !supported {
            block_base_drop(&mut *self, SIGN_ITEM_ID, 1, 0, x, y, z, 1.0);
            self.set_block_id(x, y, z, 0);
        }
    }

    /// Scheduled/random-tick router (mirrors the `updateTick` overrides).
    fn update_block_tick(&mut self, x: i32, y: i32, z: i32) {
        let bid = self.get_block_id(x, y, z);
        if bid == 0 {
            return;
        }
        match bid {
            12 | 13 => block_sand_tick(&mut *self, bid, x, y, z),
            8..=11 => {
                let lava = self.material_at(x, y, z) == Material::LAVA;
                block_fluid_tick(&mut *self, bid, lava, x, y, z);
            }
            37 | 38 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_flower_tick(&mut *self, d, q, g, x, y, z);
            }
            81 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_cactus_tick(&mut *self, bid, d, q, g, x, y, z);
            }
            83 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                block_reed_tick(&mut *self, bid, d, q, g, x, y, z);
            }
            18 => {
                let mut guard = self.leaves_guard;
                // Decayed leaves drop a sapling 1/20 (BlockLeaves);
                // the tick itself always clears the cell.
                let (did, dqty) = if self.rng.next_int_bound(20) == 0 { (6, 1) } else { (0, 0) };
                block_leaves_tick(&mut *self, bid, bid, did, dqty, 0, &mut guard, x, y, z);
                self.leaves_guard = guard;
            }
            6 => {
                let (d, q, g) = Self::native_drop_ids(bid);
                let action = block_sapling_tick(&mut *self, bid, d, q, g, x, y, z);
                if action.kind == 1 {
                    self.grow_sapling(x, y, z, bid, action.seed);
                }
            }
            59 => block_crops_tick(&mut *self, bid, bid, WHEAT_ITEM_ID, SEEDS_ITEM_ID, x, y, z),
            60 => block_soil_tick(&mut *self, bid, x, y, z),
            51 => block_fire_tick(&mut *self, bid, 10, x, y, z),
            50
                // Torch re-seats meta 0 (Java BlockTorch.updateTick).
                if self.get_block_meta(x, y, z) == 0 => {
                    block_torch_added(&mut *self, bid, x, y, z);
                }
            2 => self.grass_tick(x, y, z),
            78
                // Snow melts under strong block light (Java BlockSnow).
                if self.saved_light_value(1, x, y, z) > 11 => {
                    self.drop_block_for(78, 0, x, y, z);
                    self.apply_set_notify(x, y, z, 0);
                }
            79
                // Ice melts to flowing water (Java BlockIce: light > 11-3).
                if self.saved_light_value(1, x, y, z) > 8 => {
                    self.apply_set_notify(x, y, z, 9);
                }
            80
                if self.saved_light_value(1, x, y, z) > 11 => {
                    self.drop_block_for(80, 0, x, y, z);
                    self.apply_set_notify(x, y, z, 0);
                }
            74 => {
                // Glowing redstone cools back to idle (Java BlockRedstoneOre).
                self.apply_set_notify(x, y, z, 73);
            }
            _ => {}
        }
    }

    /// Grass spread/decay (mirrors `BlockGrass.updateTick`): dark + opaque
    /// cover turns to dirt (1/4 roll), bright spreads to nearby dirt.
    fn grass_tick(&mut self, x: i32, y: i32, z: i32) {
        let above_light = self.block_light_value(x, y + 1, z);
        let above_mat = self.material_at(x, y + 1, z);
        if above_light < 4 && above_mat.can_block_grass() {
            if self.rng.next_int_bound(4) != 0 {
                return;
            }
            self.apply_set_notify(x, y, z, 3);
        } else if above_light >= 9 {
            let (nx, ny, nz) = (
                x + self.rng.next_int_bound(3) - 1,
                y + self.rng.next_int_bound(5) - 3,
                z + self.rng.next_int_bound(3) - 1,
            );
            if self.get_block_id(nx, ny, nz) == 3
                && self.block_light_value(nx, ny + 1, nz) >= 4
                && !self.material_at(nx, ny + 1, nz).can_block_grass()
            {
                self.apply_set_notify(nx, ny, nz, 2);
            }
        }
    }

    /// Sapling growth: clear, roll the
    /// 1/10 big tree, generate through the tree accessor, restore the
    /// sapling on failure.
    pub(crate) fn grow_sapling(&mut self, x: i32, y: i32, z: i32, bid: u8, seed: u64) {
        self.apply_set_notify(x, y, z, 0);
        let big = self.rng.next_int_bound(10) == 0;
        let mut access = crate::decorators::WorldAccess {
            chunks: &mut self.chunks,
            populating: self.populating,
            queue: Some(&mut self.block_updates),
        };
        let ok = if big {
            crate::generate_big_tree(&mut access, seed as i64, x, y, z)
        } else {
            crate::generate_tree(&mut access, seed as i64, x, y, z)
        };
        drop(access);
        if !ok {
            self.apply_set_notify(x, y, z, bid);
        }
    }

    /// Queue a block update (mirrors `scheduleBlockUpdate`): entries dedup
    /// by (x, y, z, id) — a second schedule for the same cell+id is dropped
    /// (Java `scheduledTickSet`), and cells without loaded surroundings
    /// (radius 8) are skipped. Time-Keyed map keeps fire order.
    pub fn schedule_block_update(&mut self, x: i32, y: i32, z: i32, block_id: u8, delay: i32) {
        if block_id == 0 {
            return;
        }
        if !self.chunks_exist_radius(x, z, 8) {
            return;
        }
        if !self.scheduled_set.insert((x, y, z, block_id)) {
            return;
        }
        self.scheduled.entry((self.time + delay as i64, x, y, z, block_id)).or_insert(());
    }

    /// Loaded-area check for a block radius (mirrors `checkChunksExist`).
    fn chunks_exist_radius(&self, x: i32, z: i32, r: i32) -> bool {
        let (x0, z0) = ((x - r).div_euclid(16), (z - r).div_euclid(16));
        let (x1, z1) = ((x + r).div_euclid(16), (z + r).div_euclid(16));
        for cx in x0..=x1 {
            for cz in z0..=z1 {
                if !self.has_chunk(cx, cz) {
                    return false;
                }
            }
        }
        true
    }

    /// Due scheduled updates, oldest first, capped at 1000 per tick like
    /// C++ (stale entries die on the id check; missing chunks skip).
    pub(crate) fn process_scheduled_ticks(&mut self) {
        let mut ran = 0;
        while ran < 1000 {
            let next = self.scheduled.iter().next().map(|(k, _)| *k);
            let (t, x, y, z, bid) = match next {
                Some(n) => n,
                None => break,
            };
            if t > self.time {
                break;
            }
            self.scheduled.remove(&(t, x, y, z, bid));
            self.scheduled_set.remove(&(x, y, z, bid));
            ran += 1;
            // Java processes only ticks whose radius-8 surroundings are
            // loaded (World.scheduleBlockUpdate/process path).
            if !self.chunks_exist_radius(x, z, 8) {
                continue;
            }
            if bid == 0 || self.get_block_id(x, y, z) != bid {
                continue;
            }
            self.update_block_tick(x, y, z);
        }
    }

    /// Random block ticks (mirrors the 80-cells-per-chunk pass over the
    /// radius-9 loaded chunks around every joined player (Java World:1345);
    /// chunk order is sorted for determinism where C++ iterates an
    /// unordered set).
    pub(crate) fn random_block_ticks(&mut self) {
        let mut players: Vec<(f64, f64, f64)> = Vec::new();
        for oid in self.entities.all_ids() {
            if let Some(Entity::Player(p)) = self.entities.get(oid) {
                let b = &p.living.body;
                players.push((b.pos[0], b.pos[1], b.pos[2]));
            }
        }
        if players.is_empty() {
            return;
        }
        let mut keys: Vec<(i32, i32)> = Vec::new();
        for (px, _, pz) in &players {
            let (cx, cz) = ((px / 16.0).floor() as i32, (pz / 16.0).floor() as i32);
            for dx in -9..=9 {
                for dz in -9..=9 {
                    keys.push((cx + dx, cz + dz));
                }
            }
        }
        keys.sort_unstable();
        keys.dedup();
        for (cx, cz) in keys {
            if !self.has_chunk(cx, cz) {
                continue;
            }
            for _ in 0..80 {
                let bx = (cx << 4) + self.rng.next_int_bound(16);
                let by = self.rng.next_int_bound(128);
                let bz = (cz << 4) + self.rng.next_int_bound(16);
                let id = self.get_block_id(bx, by, bz);
                if id == 0 {
                    continue;
                }
                if block_properties_get(id as u32).tick_on_load {
                    self.update_block_tick(bx, by, bz);
                }
            }
        }
    }

    /// Periodic unload (mirrors the `worldTime % 100` pass): chunks outside
    /// the unload radius of every joined player go, except the protected
    /// spawn area (|cx|,|cz| <= 3). Live rows spill into the chunk lists
    /// for reload; with no players everything stays (no-server case).
    pub(crate) fn unload_chunks(&mut self) {
        if self.time % 100 != 0 {
            return;
        }
        let mut anchors: Vec<(i32, i32)> = Vec::new();
        for oid in self.entities.all_ids() {
            if let Some(Entity::Player(p)) = self.entities.get(oid) {
                anchors.push((
                    (p.living.body.pos[0].floor() as i32) >> 4,
                    (p.living.body.pos[2].floor() as i32) >> 4,
                ));
            }
        }
        if anchors.is_empty() {
            return;
        }
        let r = self.unload_radius;
        let drop: Vec<(i32, i32)> = self
            .chunks
            .keys()
            .filter(|(cx, cz)| {
                if cx.abs() <= 3 && cz.abs() <= 3 {
                    return false;
                }
                !anchors.iter().any(|(px, pz)| (cx - px).abs() <= r && (cz - pz).abs() <= r)
            })
            .copied()
            .collect();
        for (cx, cz) in drop {
            self.spill_chunk(cx, cz);
            if let Some(chunk) = self.chunks.remove(&(cx, cz)) {
                self.unloaded.insert((cx, cz), chunk);
            }
        }
    }

    /// Recall every staged chunk (the server calls this before a world
    /// save: native unloads stage to memory without hitting the disk,
    /// while C++ unloads save through, so staged edits must be pulled
    /// back before the flush or they die in memory).
    pub(crate) fn recall_all_staged(&mut self) {
        let keys: Vec<(i32, i32)> = self.unloaded.keys().copied().collect();
        for (cx, cz) in keys {
            self.recall_chunk(cx, cz);
        }
    }

    /// Recall an unloaded chunk and thaw its spill back into rows
    /// (mirrors the load path; disk fills `unloaded` in the persistence
    /// slice). Returns false when nothing was staged.
    pub fn recall_chunk(&mut self, cx: i32, cz: i32) -> bool {
        if self.chunks.contains_key(&(cx, cz)) {
            return true;
        }
        if let Some(chunk) = self.unloaded.remove(&(cx, cz)) {
            self.chunks.insert((cx, cz), chunk);
            self.restore_chunk_entities(cx, cz);
            return true;
        }
        false
    }

    /// Freeze a chunk's live rows into its pending lists (mirrors the
    /// unload spill; boats included).
    fn spill_chunk(&mut self, cx: i32, cz: i32) {
        let mut gone: Vec<EntityId> = Vec::new();
        for oid in self.entities.alive_ids() {
            let e = match self.entities.get(oid) {
                Some(e) => e,
                None => continue,
            };
            if (e.body().pos[0].floor() as i32) >> 4 != cx
                || (e.body().pos[2].floor() as i32) >> 4 != cz
            {
                continue;
            }
            let chunk = match self.chunks.get_mut(&(cx, cz)) {
                Some(c) => c,
                None => continue,
            };
            match e {
                Entity::Item(it) => chunk.pending_items.push(crate::chunk::PendingItem {
                    item_id: it.item_id,
                    count: it.count,
                    damage: it.damage,
                    age: it.age,
                    pickup_delay: it.pickup_delay,
                    pos: it.body.pos,
                }),
                Entity::Animal(a) => chunk.pending_animals.push(pending_creature(
                    animal_string_id(a.kind),
                    &a.living,
                    a.saddled,
                    a.sheared,
                    a.egg_timer,
                )),
                Entity::Mob(m) => chunk.pending_monsters.push(pending_creature(
                    mob_string_id(m.kind),
                    &m.living,
                    false,
                    false,
                    0,
                )),
                Entity::Boat(b) => chunk.pending_boats.push(crate::chunk::PendingBoat {
                    pos: b.body.pos,
                    motion: b.body.motion,
                    yaw: b.body.yaw,
                    pitch: b.body.pitch,
                    time_since_hit: b.time_since_hit,
                    damage_taken: b.damage_taken,
                    forward_dir: b.forward_dir,
                }),
                _ => continue,
            }
            gone.push(oid);
        }
        gone.sort_unstable();
        for oid in gone {
            self.entities.remove(oid);
        }
    }

    /// Thaw a chunk's pending lists back into rows (used on load and by
    /// tests; unknown string ids are skipped like the C++ restore).
    pub fn restore_chunk_entities(&mut self, cx: i32, cz: i32) {
        let chunk = match self.chunks.get_mut(&(cx, cz)) {
            Some(c) => c,
            None => return,
        };
        let items = std::mem::take(&mut chunk.pending_items);
        let animals = std::mem::take(&mut chunk.pending_animals);
        let monsters = std::mem::take(&mut chunk.pending_monsters);
        let boats = std::mem::take(&mut chunk.pending_boats);
        for it in items {
            let id = self.entities.alloc_id();
            let mut b = Body::new(id, 0.25, 0.25, 0.125);
            b.set_position(it.pos[0], it.pos[1], it.pos[2]);
            self.entities.insert(Entity::Item(crate::entity::table::ItemEnt {
                body: b,
                item_id: it.item_id,
                count: it.count,
                damage: it.damage,
                age: it.age,
                pickup_delay: it.pickup_delay,
            }));
        }
        for an in animals {
            if let Some(kind) = animal_kind_of(&an.string_id) {
                let id = self.entities.alloc_id();
                let mut a = crate::entity::table::AnimalEnt::new(id, kind);
                a.living.body.set_position(an.pos[0], an.pos[1], an.pos[2]);
                a.living.body.motion = an.motion;
                a.living.body.yaw = an.yaw;
                a.living.body.pitch = an.pitch;
                a.living.health = an.health;
                a.living.max_health = an.max_health;
                a.saddled = an.saddled;
                a.sheared = an.sheared;
                a.egg_timer = an.egg_timer;
                self.entities.insert(Entity::Animal(a));
            }
        }
        for mo in monsters {
            if let Some(kind) = mob_kind_of(&mo.string_id) {
                let id = self.entities.alloc_id();
                let mut m = crate::entity::table::MobEnt::new(id, kind);
                m.living.body.set_position(mo.pos[0], mo.pos[1], mo.pos[2]);
                m.living.body.motion = mo.motion;
                m.living.body.yaw = mo.yaw;
                m.living.body.pitch = mo.pitch;
                m.living.health = mo.health;
                m.living.max_health = mo.max_health;
                self.entities.insert(Entity::Mob(m));
            }
        }
        for bt in boats {
            let id = self.entities.alloc_id();
            let mut b = Body::new(id, 1.5, 0.6, 0.3);
            b.set_position(bt.pos[0], bt.pos[1], bt.pos[2]);
            b.motion = bt.motion;
            b.yaw = bt.yaw;
            b.pitch = bt.pitch;
            self.entities.insert(Entity::Boat(crate::entity::table::BoatEnt {
                body: b,
                time_since_hit: bt.time_since_hit,
                damage_taken: bt.damage_taken,
                forward_dir: bt.forward_dir,
            }));
        }
    }
}
