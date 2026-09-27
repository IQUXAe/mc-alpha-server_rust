//! Block placement, neighbor updates, drops and tick scheduling on [`World`].
//! Split out of `world.rs`; behavior unchanged.

use crate::block::fire::{block_fire_added, block_fire_neighbor, block_fire_tick};
use crate::block::pos::{BlockPos, DropSpec};
use crate::block::table::block_properties_get;
use crate::block::ticks::{
    block_base_drop, block_cactus_neighbor, block_cactus_random_tick, block_crops_neighbor,
    block_crops_tick, block_flower_neighbor, block_flower_tick, block_fluid_added,
    block_fluid_neighbor, block_fluid_tick, block_leaves_neighbor, block_leaves_tick,
    block_mushroom_neighbor, block_reed_neighbor, block_reed_tick, block_sand_added,
    block_sand_neighbor, block_sand_tick, block_sapling_neighbor, block_sapling_tick,
    block_soil_neighbor, block_soil_tick, block_torch_added, block_torch_neighbor, CropIds,
};
use crate::entity::table::{Body, Entity, EntityId};
use crate::material::Material;
use crate::world::{
    animal_kind_of, animal_string_id, is_air_material, material_of, mob_kind_of, mob_string_id,
    pending_creature, TileData, World, WORLD_HEIGHT,
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
        (
            if p.id_dropped != 0 {
                p.id_dropped
            } else {
                bid as i32
            },
            p.quantity_dropped,
            0,
        )
    }

    pub(crate) fn native_drop_spec(bid: u8) -> DropSpec {
        let (d, q, g) = Self::native_drop_ids(bid);
        DropSpec::new(d, q, g)
    }

    fn crop_ids(bid: u8) -> CropIds {
        CropIds {
            block: bid,
            crop: bid,
            wheat: WHEAT_ITEM_ID,
            seeds: SEEDS_ITEM_ID,
        }
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
    /// chest/furnace contents, drop the tile row, and eject any jukebox record.
    fn block_removed(&mut self, x: i32, y: i32, z: i32, old: u8) {
        if matches!(old, 54 | 61 | 62 | 63 | 68) {
            self.scatter_container_tile(x, y, z);
            self.tiles.remove(&(x, y, z));
        } else if old == 84 {
            self.eject_jukebox_record(x, y, z);
            self.tiles.remove(&(x, y, z));
        } else {
            self.tiles.remove(&(x, y, z));
        }
    }

    /// Eject a record from a jukebox at `(x, y, z)` if `meta > 0`
    /// (`BlockJukeBox.ejectRecord`).
    pub fn eject_jukebox_record(&mut self, x: i32, y: i32, z: i32) -> bool {
        let meta = self.get_block_meta(x, y, z);
        if meta == 0 {
            return false;
        }
        self.set_block_meta(x, y, z, 0);
        let item_id = 2255 + meta as i32;
        block_base_drop(&mut *self, DropSpec::new(item_id, 1, 0), BlockPos::new(x, y, z), 1.0);
        true
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
            52 => {
                self.tiles.entry((x, y, z)).or_insert_with(|| {
                    TileData::MobSpawner(crate::world::tiles::MobSpawnerState::new("Pig"))
                });
            }
            54 => {
                self.tiles
                    .entry((x, y, z))
                    .or_insert_with(|| TileData::Chest(crate::tile_entity::chest::chest_create()));
            }
            61 | 62 => {
                self.tiles.entry((x, y, z)).or_insert_with(|| {
                    TileData::Furnace(crate::tile_entity::furnace::furnace_create())
                });
                if self.get_block_meta(x, y, z) == 0 {
                    let north = self.is_solid(x, y, z - 1);
                    let south = self.is_solid(x, y, z + 1);
                    let west = self.is_solid(x - 1, y, z);
                    let east = self.is_solid(x + 1, y, z);
                    let mut facing = 3u8;
                    if north && !south {
                        facing = 3;
                    }
                    if south && !north {
                        facing = 2;
                    }
                    if west && !east {
                        facing = 5;
                    }
                    if east && !west {
                        facing = 4;
                    }
                    self.set_block_meta(x, y, z, facing);
                }
            }
            63 | 68 => {
                self.tiles
                    .entry((x, y, z))
                    .or_insert_with(|| TileData::Sign(crate::tile_entity::sign::sign_create()));
            }
            _ => {}
        }
        match bid {
            12 | 13 => block_sand_added(&mut *self, bid, BlockPos::new(x, y, z)),
            8..=11 => {
                let rate = if self.material_at(x, y, z) == Material::LAVA {
                    30
                } else {
                    5
                };
                block_fluid_added(&mut *self, bid, rate, BlockPos::new(x, y, z));
                self.fluid_lava_contact(x, y, z, bid);
            }
            50 => block_torch_added(&mut *self, bid, BlockPos::new(x, y, z)),
            51 => block_fire_added(&mut *self, bid, 10, BlockPos::new(x, y, z)),
            46 if self.is_block_powered(x, y, z) => {
                self.ignite_at(BlockPos::new(x, y, z), 80);
                self.apply_set_notify(x, y, z, 0);
            }
            55 => {
                self.recalculate_redstone_around(x, y, z);
                self.notify_neighbors_of(x, y, z);
            }
            75 | 76 => {
                if self.get_block_meta(x, y, z) == 0 {
                    block_torch_added(&mut *self, bid, BlockPos::new(x, y, z));
                }
                self.redstone_torch_check(x, y, z, bid);
                self.recalculate_redstone_around(x, y, z);
                self.notify_neighbors_of(x, y, z);
            }
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
            12 | 13 => block_sand_neighbor(&mut *self, bid, BlockPos::new(x, y, z)),
            8..=11 => {
                let rate = if self.material_at(x, y, z) == Material::LAVA {
                    30
                } else {
                    5
                };
                block_fluid_neighbor(&mut *self, bid, rate, BlockPos::new(x, y, z));
                self.fluid_lava_contact(x, y, z, bid);
            }
            37 | 38 => {
                block_flower_neighbor(
                    &mut *self,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
            }
            39 | 40 => {
                block_mushroom_neighbor(
                    &mut *self,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
            }
            50 => {
                block_torch_neighbor(
                    &mut *self,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
            }
            75 | 76 => {
                block_torch_neighbor(
                    &mut *self,
                    DropSpec::new(76, 1, 0),
                    BlockPos::new(x, y, z),
                );
                if self.get_block_id(x, y, z) == bid {
                    self.redstone_torch_check(x, y, z, bid);
                }
            }
            55 => {
                if !self.attach_at(BlockPos::new(x, y - 1, z)) {
                    self.drop_block_for(55, 0, x, y, z);
                    self.apply_set_notify(x, y, z, 0);
                    self.recalculate_redstone_around(x, y, z);
                } else {
                    self.recalculate_redstone_around(x, y, z);
                }
            }
            70 | 72 if !self.attach_at(BlockPos::new(x, y - 1, z)) => {
                self.drop_block_for(bid, 0, x, y, z);
                self.apply_set_notify(x, y, z, 0);
                self.recalculate_redstone_around(x, y, z);
            }
            78 => self.snow_neighbor(x, y, z),
            81 => {
                block_cactus_neighbor(
                    &mut *self,
                    bid,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
            }
            83 => {
                block_reed_neighbor(
                    &mut *self,
                    bid,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
            }
            18 => {
                let mut guard = self.leaves_guard;
                block_leaves_neighbor(&mut *self, bid, bid, &mut guard, BlockPos::new(x, y, z));
                self.leaves_guard = guard;
            }
            6 => {
                block_sapling_neighbor(
                    &mut *self,
                    bid,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
            }
            59 => block_crops_neighbor(&mut *self, Self::crop_ids(bid), BlockPos::new(x, y, z)),
            60 => block_soil_neighbor(&mut *self, bid, BlockPos::new(x, y, z)),
            51 => block_fire_neighbor(&mut *self, BlockPos::new(x, y, z)),
            46 if self.is_block_powered(x, y, z) => {
                self.ignite_at(BlockPos::new(x, y, z), 80);
                self.apply_set_notify(x, y, z, 0);
            }
            64 | 71 => self.door_neighbor(x, y, z, bid),
            69 | 77 => self.lever_or_button_neighbor(x, y, z, bid),
            63 | 68 => {}
            _ => {}
        }
        if bid == 63 || bid == 68 {
            self.sign_neighbor(x, y, z, bid);
        }
    }

    /// Check whether a redstone wire at `(wx, wy, wz)` can connect diagonally
    /// to `(wx + dx, wy + dy, wz + dz)` without being cut by an opaque/attachable block.
    fn wire_can_connect_diagonal(&self, wx: i32, wy: i32, wz: i32, dx: i32, dy: i32, dz: i32) -> bool {
        if dy == 0 {
            true
        } else if dy == 1 {
            !self.attach_at(BlockPos::new(wx, wy + 1, wz))
        } else {
            !self.attach_at(BlockPos::new(wx + dx, wy, wz + dz))
        }
    }

    /// True if a powered redstone wire (`55` with `meta > 0`) at `(wx, wy, wz)`
    /// provides power to neighbor `(tx, ty, tz)` (`BlockRedstoneWire.isPoweringTo`).
    pub fn wire_powers_neighbor(&self, wx: i32, wy: i32, wz: i32, tx: i32, ty: i32, tz: i32) -> bool {
        if self.get_block_id(wx, wy, wz) != 55 || self.get_block_meta(wx, wy, wz) == 0 {
            return false;
        }
        if tx == wx && tz == wz && ty == wy - 1 {
            return true;
        }
        if ty != wy {
            return false;
        }
        let can_connect = |x: i32, y: i32, z: i32| {
            matches!(self.get_block_id(x, y, z), 55 | 69 | 70 | 72 | 75 | 76 | 77)
        };
        let conn_dir = |dx: i32, dz: i32| {
            can_connect(wx + dx, wy, wz + dz)
                || (!self.attach_at(BlockPos::new(wx + dx, wy, wz + dz))
                    && can_connect(wx + dx, wy - 1, wz + dz))
                || (!self.attach_at(BlockPos::new(wx, wy + 1, wz))
                    && self.attach_at(BlockPos::new(wx + dx, wy, wz + dz))
                    && can_connect(wx + dx, wy + 1, wz + dz))
        };
        let west = conn_dir(-1, 0);
        let east = conn_dir(1, 0);
        let north = conn_dir(0, -1);
        let south = conn_dir(0, 1);
        if !west && !east && !north && !south {
            return true;
        }
        if tx == wx && tz == wz + 1 {
            return north && !west && !east;
        }
        if tx == wx && tz == wz - 1 {
            return south && !west && !east;
        }
        if tx == wx + 1 && tz == wz {
            return west && !north && !south;
        }
        if tx == wx - 1 && tz == wz {
            return east && !north && !south;
        }
        false
    }

    /// True if `(x, y, z)` is strongly powered (`World.isBlockGettingPowered` /
    /// `Block.isIndirectlyPoweringTo`), excluding any source at `exclude`.
    fn is_block_strongly_powered(
        &self,
        x: i32,
        y: i32,
        z: i32,
        include_wires: bool,
        exclude: Option<(i32, i32, i32)>,
    ) -> bool {
        const OFF: [(i32, i32, i32); 6] = [
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ];
        for (dx, dy, dz) in OFF {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if exclude == Some((nx, ny, nz)) {
                continue;
            }
            let nid = self.get_block_id(nx, ny, nz);
            let nmeta = self.get_block_meta(nx, ny, nz);
            match nid {
                // Lit redstone torch below shines strong power up into (x, y, z).
                76 if dy == -1 => return true,
                // Lever or button attached to (x, y, z) strongly powers its support block.
                69 | 77 if (nmeta & 8) != 0 => {
                    let dir = nmeta & 7;
                    let attached_here = match dir {
                        1 => dx == 1 && dy == 0 && dz == 0,
                        2 => dx == -1 && dy == 0 && dz == 0,
                        3 => dx == 0 && dy == 0 && dz == 1,
                        4 => dx == 0 && dy == 0 && dz == -1,
                        _ => dx == 0 && dy == 1 && dz == 0,
                    };
                    if attached_here {
                        return true;
                    }
                }
                // Depressed pressure plate on top of (x, y, z) strongly powers (x, y, z).
                70 | 72 if nmeta > 0 && dy == 1 => return true,
                // Powered redstone wire strongly powers in the directions it powers.
                55 if include_wires && self.wire_powers_neighbor(nx, ny, nz, x, y, z) => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// True if `(x, y, z)` receives direct or indirect redstone power
    /// (`World.isBlockIndirectlyGettingPowered`).
    pub fn is_block_powered(&self, x: i32, y: i32, z: i32) -> bool {
        self.is_block_powered_ext(x, y, z, true)
    }

    fn is_block_powered_ext(&self, x: i32, y: i32, z: i32, include_wires: bool) -> bool {
        const OFF: [(i32, i32, i32); 6] = [
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ];
        for (dx, dy, dz) in OFF {
            let (nx, ny, nz) = (x + dx, y + dy, z + dz);
            if self.attach_at(BlockPos::new(nx, ny, nz)) {
                if self.is_block_strongly_powered(nx, ny, nz, include_wires, Some((x, y, z))) {
                    return true;
                }
                continue;
            }
            let nid = self.get_block_id(nx, ny, nz);
            let nmeta = self.get_block_meta(nx, ny, nz);
            match nid {
                76 => {
                    // Torch does not power the block it is attached to.
                    let attached_to_target = match nmeta {
                        1 => dx == 1 && dy == 0 && dz == 0,
                        2 => dx == -1 && dy == 0 && dz == 0,
                        3 => dx == 0 && dy == 0 && dz == 1,
                        4 => dx == 0 && dy == 0 && dz == -1,
                        _ => dx == 0 && dy == 1 && dz == 0,
                    };
                    if !attached_to_target {
                        return true;
                    }
                }
                69 | 77 if (nmeta & 8) != 0 => return true,
                70 | 72 if nmeta > 0 && dy >= 0 => return true,
                55 if include_wires && self.wire_powers_neighbor(nx, ny, nz, x, y, z) => {
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    /// Check whether the support block of a redstone torch at `(x, y, z)` is receiving power.
    pub fn is_redstone_torch_input_powered(&self, x: i32, y: i32, z: i32) -> bool {
        let meta = self.get_block_meta(x, y, z);
        let (sx, sy, sz) = match meta {
            1 => (x - 1, y, z),
            2 => (x + 1, y, z),
            3 => (x, y, z - 1),
            4 => (x, y, z + 1),
            _ => (x, y - 1, z),
        };
        self.is_block_strongly_powered(sx, sy, sz, true, Some((x, y, z)))
    }

    fn redstone_torch_check(&mut self, x: i32, y: i32, z: i32, bid: u8) {
        let powered = self.is_redstone_torch_input_powered(x, y, z);
        if (bid == 76 && powered) || (bid == 75 && !powered) {
            self.schedule_block_update(x, y, z, bid, 2);
        }
    }

    /// Recompute redstone wire (`55`) signal levels in the connected component around `(cx, cy, cz)`.
    pub fn recalculate_redstone_around(&mut self, cx: i32, cy: i32, cz: i32) {
        use std::collections::{BTreeMap, VecDeque};
        let mut wires: BTreeMap<(i32, i32, i32), u8> = BTreeMap::new();
        let mut queue: VecDeque<(i32, i32, i32)> = VecDeque::new();

        for dx in -2..=2 {
            for dy in -2..=2 {
                for dz in -2..=2 {
                    let (wx, wy, wz) = (cx + dx, cy + dy, cz + dz);
                    if self.get_block_id(wx, wy, wz) == 55 && !wires.contains_key(&(wx, wy, wz)) {
                        wires.insert((wx, wy, wz), 0);
                        queue.push_back((wx, wy, wz));
                    }
                }
            }
        }
        while let Some((wx, wy, wz)) = queue.pop_front() {
            if wires.len() >= 256 {
                break;
            }
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                for dy in -1..=1 {
                    if !self.wire_can_connect_diagonal(wx, wy, wz, dx, dy, dz) {
                        continue;
                    }
                    let p = (wx + dx, wy + dy, wz + dz);
                    if self.get_block_id(p.0, p.1, p.2) == 55 && !wires.contains_key(&p) {
                        wires.insert(p, 0);
                        queue.push_back(p);
                    }
                }
            }
        }
        if wires.is_empty() {
            return;
        }
        // Seed direct/indirect power (15) from active non-wire sources or strongly-powered
        // solid blocks adjacent to each wire cell (BlockRedstoneWire.java:39-41).
        let mut prop_q: VecDeque<(i32, i32, i32)> = VecDeque::new();
        let wire_coords: Vec<(i32, i32, i32)> = wires.keys().copied().collect();
        for &(wx, wy, wz) in &wire_coords {
            if self.is_block_powered_ext(wx, wy, wz, false) {
                wires.insert((wx, wy, wz), 15);
                prop_q.push_back((wx, wy, wz));
            }
        }
        while let Some((wx, wy, wz)) = prop_q.pop_front() {
            let cur = *wires.get(&(wx, wy, wz)).unwrap_or(&0);
            if cur <= 1 {
                continue;
            }
            let next_lvl = cur - 1;
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                for dy in -1..=1 {
                    if !self.wire_can_connect_diagonal(wx, wy, wz, dx, dy, dz) {
                        continue;
                    }
                    let p = (wx + dx, wy + dy, wz + dz);
                    if let Some(slot) = wires.get_mut(&p) {
                        if next_lvl > *slot {
                            *slot = next_lvl;
                            prop_q.push_back(p);
                        }
                    }
                }
            }
        }
        let mut changed: Vec<(i32, i32, i32)> = Vec::new();
        for ((wx, wy, wz), new_meta) in wires {
            if self.get_block_meta(wx, wy, wz) != new_meta {
                self.set_block_meta(wx, wy, wz, new_meta);
                changed.push((wx, wy, wz));
            }
        }
        for (wx, wy, wz) in changed {
            for (dx, dy, dz) in [(-1, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)] {
                let (nx, ny, nz) = (wx + dx, wy + dy, wz + dz);
                let nid = self.get_block_id(nx, ny, nz);
                if nid != 55 && nid != 0 {
                    self.neighbor_changed(nx, ny, nz);
                    for (tx, ty, tz) in [(-1, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)] {
                        let (rx, ry, rz) = (nx + tx, ny + ty, nz + tz);
                        let rid = self.get_block_id(rx, ry, rz);
                        if matches!(rid, 75 | 76 | 64 | 71 | 46) {
                            self.neighbor_changed(rx, ry, rz);
                        }
                    }
                }
            }
        }
    }

    /// Update stone (`70`) or wooden (`72`) pressure plate state based on colliding entities.
    pub fn update_pressure_plate(&mut self, x: i32, y: i32, z: i32, bid: u8) {
        if self.get_block_id(x, y, z) != bid {
            return;
        }
        let old_meta = self.get_block_meta(x, y, z);
        let box_min = [x as f64 + 0.125, y as f64, z as f64 + 0.125];
        let box_max = [x as f64 + 0.875, y as f64 + 0.25, z as f64 + 0.875];
        let mut pressed = false;
        for oid in self.entities.alive_ids() {
            if let Some(e) = self.entities.get(oid) {
                if e.body().dead {
                    continue;
                }
                if bid == 70
                    && !matches!(
                        e,
                        crate::entity::table::Entity::Mob(_)
                            | crate::entity::table::Entity::Animal(_)
                            | crate::entity::table::Entity::Player(_)
                    )
                {
                    continue;
                }
                let bb = e.body().bounding_box;
                if bb.max_x > box_min[0]
                    && bb.min_x < box_max[0]
                    && bb.max_y >= box_min[1]
                    && bb.min_y <= box_max[1]
                    && bb.max_z > box_min[2]
                    && bb.min_z < box_max[2]
                {
                    pressed = true;
                    break;
                }
            }
        }
        let new_meta = if pressed { 1 } else { 0 };
        if new_meta != old_meta {
            self.set_block_meta(x, y, z, new_meta);
            self.recalculate_redstone_around(x, y, z);
            self.notify_neighbors_of(x, y, z);
            self.notify_neighbors_of(x, y - 1, z);
        }
        if pressed {
            self.schedule_block_update(x, y, z, bid, 20);
        }
    }

    /// Toggle a wooden door (`64`) open/closed (`BlockDoor.blockActivated`).
    pub fn toggle_door(&mut self, x: i32, y: i32, z: i32) -> bool {
        let bid = self.get_block_id(x, y, z);
        if bid != 64 {
            return bid == 71;
        }
        let meta = self.get_block_meta(x, y, z);
        if (meta & 8) != 0 {
            if self.get_block_id(x, y - 1, z) == bid {
                return self.toggle_door(x, y - 1, z);
            }
            return true;
        }
        let toggled = meta ^ 4;
        self.set_block_meta(x, y, z, toggled);
        let has_upper = self.get_block_id(x, y + 1, z) == bid;
        if has_upper {
            self.set_block_meta(x, y + 1, z, toggled | 8);
            self.notify_neighbors_of(x, y + 1, z);
        }
        self.notify_neighbors_of(x, y, z);
        true
    }

    /// Set a wooden or iron door (`64 | 71`) open state (`BlockDoor.func_272_a`).
    pub fn set_door_open(&mut self, x: i32, y: i32, z: i32, open: bool) {
        let bid = self.get_block_id(x, y, z);
        if bid != 64 && bid != 71 {
            return;
        }
        let meta = self.get_block_meta(x, y, z);
        if (meta & 8) != 0 {
            if self.get_block_id(x, y - 1, z) == bid {
                self.set_door_open(x, y - 1, z, open);
            }
            return;
        }
        let is_open = (meta & 4) != 0;
        if is_open != open {
            let next = meta ^ 4;
            self.set_block_meta(x, y, z, next);
            let has_upper = self.get_block_id(x, y + 1, z) == bid;
            if has_upper {
                self.set_block_meta(x, y + 1, z, next | 8);
                self.notify_neighbors_of(x, y + 1, z);
            }
            self.notify_neighbors_of(x, y, z);
        }
    }

    fn door_neighbor(&mut self, x: i32, y: i32, z: i32, bid: u8) {
        let meta = self.get_block_meta(x, y, z);
        if (meta & 8) != 0 {
            if self.get_block_id(x, y - 1, z) != bid {
                self.apply_set_notify(x, y, z, 0);
            } else {
                self.door_neighbor(x, y - 1, z, bid);
            }
            return;
        }
        let mut broken = false;
        if self.get_block_id(x, y + 1, z) != bid {
            self.apply_set_notify(x, y, z, 0);
            broken = true;
        }
        if !self.attach_at(BlockPos::new(x, y - 1, z)) {
            self.apply_set_notify(x, y, z, 0);
            broken = true;
            if self.get_block_id(x, y + 1, z) == bid {
                self.apply_set_notify(x, y + 1, z, 0);
            }
        }
        if broken {
            self.drop_block_for(bid, meta, x, y, z);
        } else {
            let powered = self.is_block_powered(x, y, z) || self.is_block_powered(x, y + 1, z);
            self.set_door_open(x, y, z, powered);
        }
    }

    /// Toggle a lever (`69`) and notify its neighbors + support block (`BlockLever.blockActivated`).
    pub fn toggle_lever(&mut self, x: i32, y: i32, z: i32) -> bool {
        if self.get_block_id(x, y, z) != 69 {
            return false;
        }
        let meta = self.get_block_meta(x, y, z);
        let dir = meta & 7;
        let power = 8 - (meta & 8);
        self.set_block_meta(x, y, z, dir | power);
        self.recalculate_redstone_around(x, y, z);
        self.notify_neighbors_of(x, y, z);
        self.notify_lever_support(x, y, z, dir);
        true
    }

    /// Press a stone button (`77`) (`BlockButton.blockActivated`).
    pub fn press_button(&mut self, x: i32, y: i32, z: i32) -> bool {
        if self.get_block_id(x, y, z) != 77 {
            return false;
        }
        let meta = self.get_block_meta(x, y, z);
        if (meta & 8) != 0 {
            return true;
        }
        let dir = meta & 7;
        self.set_block_meta(x, y, z, dir | 8);
        self.recalculate_redstone_around(x, y, z);
        self.notify_neighbors_of(x, y, z);
        self.notify_lever_support(x, y, z, dir);
        self.schedule_block_update(x, y, z, 77, 20);
        true
    }

    fn notify_lever_support(&mut self, x: i32, y: i32, z: i32, dir: u8) {
        let (sx, sy, sz) = match dir {
            1 => (x - 1, y, z),
            2 => (x + 1, y, z),
            3 => (x, y, z - 1),
            4 => (x, y, z + 1),
            _ => (x, y - 1, z),
        };
        self.notify_neighbors_of(sx, sy, sz);
    }

    fn lever_or_button_neighbor(&mut self, x: i32, y: i32, z: i32, bid: u8) {
        let meta = self.get_block_meta(x, y, z);
        let dir = meta & 7;
        let supported = match dir {
            1 => self.attach_at(BlockPos::new(x - 1, y, z)),
            2 => self.attach_at(BlockPos::new(x + 1, y, z)),
            3 => self.attach_at(BlockPos::new(x, y, z - 1)),
            4 => self.attach_at(BlockPos::new(x, y, z + 1)),
            5 | 6 if bid == 69 => self.attach_at(BlockPos::new(x, y - 1, z)),
            _ => false,
        };
        if !supported {
            self.drop_block_for(bid, meta, x, y, z);
            self.apply_set_notify(x, y, z, 0);
            self.recalculate_redstone_around(x, y, z);
        }
    }

    /// Snow-layer support (mirrors `BlockSnow.func_275_g`): needs a solid
    /// attachable block below, else drops and vanishes.
    fn snow_neighbor(&mut self, x: i32, y: i32, z: i32) {
        let below = self.get_block_id(x, y - 1, z);
        let props = block_properties_get(below as u32);
        let ok = below != 0 && props.allows_attachment && material_of(props.material).is_solid();
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
            let pos = BlockPos::new(x, y, z);
            if meta >= 7 {
                block_base_drop(&mut *self, DropSpec::new(296, 1, 0), pos, 1.0);
            }
            for _ in 0..3 {
                // Draw from world RNG to keep the stream stable.
                let r = self.rng.next_int_bound(15);
                if r <= meta as i32 {
                    block_base_drop(&mut *self, DropSpec::new(295, 1, 0), pos, 1.0);
                }
            }
            return;
        }
        let (drop, qty) = self.rolled_drop_ids(bid, meta);
        if drop <= 0 {
            return;
        }
        // Immature crops drop nothing (rolled returns 0,0) — no seeds here.
        block_base_drop(
            &mut *self,
            DropSpec::new(drop, qty, 0),
            BlockPos::new(x, y, z),
            1.0,
        );
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
                            DropSpec::new(slot.item_id, slot.count, slot.damage),
                            BlockPos::new(x, y, z),
                        );
                    }
                }
            }
            TileData::Chest(s) => {
                for slot in s.slots {
                    if slot.count > 0 {
                        block_chest_scatter_stack(
                            &mut *self,
                            DropSpec::new(slot.item_id, slot.count, slot.damage),
                            BlockPos::new(x, y, z),
                        );
                    }
                }
            }
            TileData::Sign(_) | TileData::MobSpawner(_) => {}
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
            block_base_drop(
                &mut *self,
                DropSpec::new(SIGN_ITEM_ID, 1, 0),
                BlockPos::new(x, y, z),
                1.0,
            );
            self.set_block_id(x, y, z, 0);
        }
    }

    /// Scheduled/random-tick router (mirrors the `updateTick` overrides).
    pub(crate) fn update_block_tick(&mut self, x: i32, y: i32, z: i32) {
        let bid = self.get_block_id(x, y, z);
        if bid == 0 {
            return;
        }
        match bid {
            12 | 13 => block_sand_tick(&mut *self, bid, BlockPos::new(x, y, z)),
            8..=11 => {
                let lava = self.material_at(x, y, z) == Material::LAVA;
                block_fluid_tick(&mut *self, bid, lava, BlockPos::new(x, y, z));
            }
            37 | 38 => {
                block_flower_tick(&mut *self, Self::native_drop_spec(bid), BlockPos::new(x, y, z));
            }
            81 => {
                block_cactus_random_tick(
                    &mut *self,
                    bid,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
            }
            83 => {
                block_reed_tick(&mut *self, bid, Self::native_drop_spec(bid), BlockPos::new(x, y, z));
            }
            18 => {
                let mut guard = self.leaves_guard;
                // Decayed leaves drop a sapling 1/20 (BlockLeaves);
                // the tick itself always clears the cell.
                let (did, dqty) = if self.rng.next_int_bound(20) == 0 { (6, 1) } else { (0, 0) };
                block_leaves_tick(
                    &mut *self,
                    bid,
                    bid,
                    DropSpec::new(did, dqty, 0),
                    &mut guard,
                    BlockPos::new(x, y, z),
                );
                self.leaves_guard = guard;
            }
            6 => {
                let action = block_sapling_tick(
                    &mut *self,
                    bid,
                    Self::native_drop_spec(bid),
                    BlockPos::new(x, y, z),
                );
                if action.kind == 1 {
                    self.grow_sapling(x, y, z, bid, action.seed);
                }
            }
            59 => block_crops_tick(&mut *self, Self::crop_ids(bid), BlockPos::new(x, y, z)),
            60 => block_soil_tick(&mut *self, bid, BlockPos::new(x, y, z)),
            51 => block_fire_tick(&mut *self, bid, 10, BlockPos::new(x, y, z)),
            50
                // Torch re-seats meta 0 (Java BlockTorch.updateTick).
                if self.get_block_meta(x, y, z) == 0 => {
                    block_torch_added(&mut *self, bid, BlockPos::new(x, y, z));
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
            70 | 72 if self.get_block_meta(x, y, z) > 0 => {
                self.update_pressure_plate(x, y, z, bid);
            }
            74 => {
                // Glowing redstone cools back to idle (Java BlockRedstoneOre).
                self.apply_set_notify(x, y, z, 73);
            }
            75 | 76 => {
                let now = self.time;
                self.torch_burnouts.retain(|&(_, _, _, t)| now - t <= 100);
                let powered = self.is_redstone_torch_input_powered(x, y, z);
                let meta = self.get_block_meta(x, y, z);
                if bid == 76 && powered {
                    self.apply_set_meta_notify(x, y, z, 75, meta);
                    self.recalculate_redstone_around(x, y, z);
                    for (dx, dy, dz) in [(-1, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)] {
                        self.notify_neighbors_of(x + dx, y + dy, z + dz);
                    }
                    self.torch_burnouts.push((x, y, z, now));
                } else if bid == 75 && !powered {
                    let flips = self
                        .torch_burnouts
                        .iter()
                        .filter(|&&(bx, by, bz, _)| bx == x && by == y && bz == z)
                        .count();
                    if flips < 8 {
                        self.apply_set_meta_notify(x, y, z, 76, meta);
                        self.recalculate_redstone_around(x, y, z);
                        for (dx, dy, dz) in [(-1, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1)] {
                            self.notify_neighbors_of(x + dx, y + dy, z + dz);
                        }
                    }
                }
            }
            77 => {
                // Stone button pops back out after 20 ticks (Java BlockButton.updateTick).
                let meta = self.get_block_meta(x, y, z);
                if (meta & 8) != 0 {
                    let dir = meta & 7;
                    self.set_block_meta(x, y, z, dir);
                    self.recalculate_redstone_around(x, y, z);
                    self.notify_neighbors_of(x, y, z);
                    self.notify_lever_support(x, y, z, dir);
                }
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
        let ok = {
            let mut access = crate::decorators::WorldAccess {
                chunks: &mut self.chunks,
                populating: self.populating,
                queue: Some(&mut self.block_updates),
            };
            if big {
                crate::generate_big_tree(&mut access, seed as i64, x, y, z)
            } else {
                crate::generate_tree(&mut access, seed as i64, x, y, z)
            }
        };
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
    /// A 10 ms time budget also applies so a maturing wave (e.g. a big
    /// lava lake) can't blow the 50 ms tick; leftovers run next tick.
    pub(crate) fn process_scheduled_ticks(&mut self) {
        let start = std::time::Instant::now();
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
            if ran % 64 == 0 && start.elapsed().as_millis() > 10 {
                break;
            }
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
    /// spawn area (`|cx - scx|, |cz - scz| <= 3`). Live rows spill into the
    /// chunk lists for reload.
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
        let scx = self.spawn[0].div_euclid(16);
        let scz = self.spawn[2].div_euclid(16);
        let r = self.unload_radius;
        let drop: Vec<(i32, i32)> = self
            .chunks
            .keys()
            .filter(|&&(cx, cz)| {
                if (cx - scx).abs() <= 3 && (cz - scz).abs() <= 3 {
                    return false;
                }
                !anchors.iter().any(|&(px, pz)| (cx - px).abs() <= r && (cz - pz).abs() <= r)
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
    #[allow(dead_code)]
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
            chunk.is_modified = true;
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
