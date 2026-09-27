//! Inventory apply and sign text on `PlaySession`.
//! Split out of `session.rs`; behavior unchanged.

use crate::entity::table::Entity;
use crate::inventory::ItemStack;
use crate::session::play::PlaySession;
use crate::session::{SessionBroadcast, SessionCtx};
use crate::world::tiles::TileData;

/// Strict count for a tile stack from a client NBT packet (unknown ids and
/// non-positive counts become 0, i.e. the slot stays empty; the rest clamp
/// to the per-item max stack like `apply_inventory`).
fn strict_tile_count(item_id: i32, count: i32) -> i32 {
    if !crate::item_data::item_is_obtainable(item_id) {
        return 0;
    }
    count.clamp(0, crate::player::inventory::inventory_max_stack_size(item_id).max(1))
}

/// Strict damage for a tile stack from a client NBT packet.
fn strict_tile_damage(item_id: i32, damage: i32) -> i32 {
    let max = crate::item_data::item_max_damage(item_id);
    if max > 0 {
        damage.clamp(0, max)
    } else {
        0
    }
}

impl PlaySession {

    /// Handle client dropping items into the world (mirrors NetServerHandler.handlePickupSpawn).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pickup_spawn(
        &mut self,
        ctx: &mut SessionCtx,
        item_id: i16,
        count: i8,
        x: i32,
        y: i32,
        z: i32,
        rotation: i8,
        pitch: i8,
        roll: i8,
    ) {
        if count <= 0 || item_id <= 0 || count > 64 {
            return;
        }
        let (px, py, pz) = match ctx.world.entities.get(self.player) {
            Some(e) if !e.body().dead => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            _ => return,
        };
        let rx = x as f64 / 32.0;
        let ry = y as f64 / 32.0;
        let rz = z as f64 / 32.0;
        let dist_sq = (px - rx).powi(2) + (py - ry).powi(2) + (pz - rz).powi(2);
        if dist_sq > 64.0 {
            return;
        }

        let mut damage = 0;
        let mut has_item = false;
        if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(self.player) {
            let cur = p.inventory.current;
            if cur >= 0 && (cur as usize) < p.inventory.main.len() {
                if let Some(s) = &mut p.inventory.main[cur as usize] {
                    if s.item_id == item_id as i32 && s.count >= count as i32 {
                        damage = s.damage;
                        has_item = true;
                        s.count -= count as i32;
                        if s.count == 0 {
                            p.inventory.main[cur as usize] = None;
                        }
                    }
                }
            }
            if !has_item {
                for slot in &mut p.inventory.main {
                    if let Some(s) = slot {
                        if s.item_id == item_id as i32 && s.count >= count as i32 {
                            damage = s.damage;
                            has_item = true;
                            s.count -= count as i32;
                            if s.count == 0 {
                                *slot = None;
                            }
                            break;
                        }
                    }
                }
            }
        }

        if !has_item {
            self.send_inventory(ctx.world);
            return;
        }
        self.sync_held(ctx.world);
        if self.selected_stack(ctx.world).is_none() {
            self.held_id = 0;
        }

        let eid = ctx.world.spawn_item_entity(item_id as i32, count as i32, damage, rx, ry, rz);
        if let Some(Entity::Item(it)) = ctx.world.entities.get_mut(eid) {
            it.body.motion[0] = rotation as f64 / 128.0;
            it.body.motion[1] = pitch as f64 / 128.0;
            it.body.motion[2] = roll as f64 / 128.0;
            it.pickup_delay = 10;
        }
    }

    pub(crate) fn apply_inventory(
        &mut self,
        ctx: &mut SessionCtx,
        inv_type: i32,
        slots: &[crate::network::SlotData],
    ) {
        // Server-authoritative clamp (deliberate vanilla divergence, see
        // `server::mod` docs): vanilla `handlePlayerInventory` assigns the
        // client stacks verbatim (`NetServerHandler.java:413`), so a hacked
        // client can grant itself 127-count stacks, out-of-range damage, or
        // unknown/unobtainable ids. Clamp to the same tables the survival
        // code uses: unobtainable/unknown ids are dropped, counts to the
        // per-item max stack, damage to the per-item max durability.
        fn apply(bank: &mut [Option<ItemStack>], slots: &[crate::network::SlotData]) {
            let n = slots.len().min(bank.len());
            for i in 0..n {
                let id = slots[i].item_id as i32;
                if !crate::item_data::item_is_obtainable(id) {
                    bank[i] = None;
                    continue;
                }
                let max_stack = crate::player::inventory::inventory_max_stack_size(id).max(1);
                let count = (slots[i].count as i32).clamp(0, max_stack);
                let max_dmg = crate::item_data::item_max_damage(id);
                let dmg = if max_dmg > 0 {
                    (slots[i].damage as i32).clamp(0, max_dmg)
                } else {
                    0
                };
                bank[i] = if count > 0 {
                    Some(ItemStack::new(id, count, dmg))
                } else {
                    None
                };
            }
        }
        let me = self.player;
        if inv_type == -1 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                apply(&mut p.inventory.main, slots);
            }
            self.sync_held(ctx.world);
        } else if inv_type == -2 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                apply(&mut p.inventory.crafting, slots);
            }
        } else if inv_type == -3 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                apply(&mut p.inventory.armor, slots);
            }
        }
    }

    pub(crate) fn complex_entity(
        &mut self,
        ctx: &mut SessionCtx,
        x: i32,
        y: i32,
        z: i32,
        nbt_gz: &[u8],
    ) {
        use crate::nbt::{NbtTag, read_root};
        use std::io::Read as _;
        if nbt_gz.is_empty() || nbt_gz.len() > 65536 {
            return;
        }
        let (px, py, pz) = match ctx.world.entities.get(self.player) {
            Some(e) if !e.body().dead => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            _ => return,
        };
        let dist_sq = (px - (x as f64 + 0.5)).powi(2)
            + (py - (y as f64 + 0.5)).powi(2)
            + (pz - (z as f64 + 0.5)).powi(2);
        if dist_sq > 64.0 {
            return;
        }
        if ctx.spawn_protection > 0 {
            let sp = ctx.world.spawn;
            let protected = crate::session::is_spawn_protected(x, z, sp, ctx.spawn_protection);
            if protected && !self.is_op(ctx) {
                return;
            }
        }
        let tile = match ctx.world.tiles.get(&(x, y, z)) {
            Some(TileData::MobSpawner(_)) | None => return,
            Some(t) => *t,
        };
        let dec = flate2::read::GzDecoder::new(nbt_gz);
        let mut raw = Vec::new();
        if dec.take(32769).read_to_end(&mut raw).is_err() || raw.len() > 32768 || raw.is_empty() {
            return;
        }
        let mut cursor = std::io::Cursor::new(raw);
        let nbt = match read_root(&mut cursor) {
            Ok((_, root)) => root,
            Err(_) => return,
        };
        let (nx, ny, nz) = (
            match nbt.map.get("x") {
                Some(NbtTag::Int(v)) => *v,
                _ => return,
            },
            match nbt.map.get("y") {
                Some(NbtTag::Int(v)) => *v,
                _ => return,
            },
            match nbt.map.get("z") {
                Some(NbtTag::Int(v)) => *v,
                _ => return,
            },
        );
        if nx != x || ny != y || nz != z {
            return;
        }
        match tile {
            TileData::Sign(mut s) => {
                for i in 0..4 {
                    if let Some(NbtTag::String(text)) = nbt.map.get(&format!("Text{}", i + 1)) {
                        let bytes = text.as_bytes();
                        let len = bytes.len().min(15);
                        s.lines[i] = [0u8; 16];
                        s.lines[i][..len].copy_from_slice(&bytes[..len]);
                    }
                }
                ctx.world.tiles.insert((x, y, z), TileData::Sign(s));
            }
            TileData::Furnace(mut s) => {
                // Strict (vanilla `readFromNBT` copies the shorts
                // verbatim): clamp to the survival ranges — cook progress
                // never exceeds the 200-tick recipe, burn timers never
                // exceed the hottest fuel (lava bucket, 20000 ticks).
                // Otherwise a hacked sign-update packet grants free smelts.
                // When the furnace is already burning/cooking on the server
                // and the client sends an inventory update (`writeToNBT`,
                // which omits `"ItemBurnTime"` and echoes scaled/stale
                // timers), keep the server's authoritative timers intact.
                let server_active =
                    s.burn_time > 0 || s.cook_time > 0 || s.current_item_burn_time > 0;
                if !server_active || nbt.map.contains_key("ItemBurnTime") {
                    if let Some(NbtTag::Short(v)) = nbt.map.get("BurnTime") {
                        s.burn_time = (*v).clamp(0, 20000);
                    }
                    if let Some(NbtTag::Short(v)) = nbt.map.get("CookTime") {
                        s.cook_time = (*v).clamp(0, 200);
                    }
                    if let Some(NbtTag::Short(v)) = nbt.map.get("ItemBurnTime") {
                        s.current_item_burn_time = (*v).clamp(0, 20000);
                    }
                }
                // Same replace-not-merge rule as chests (vanilla
                // readFromNBT starts from a fresh bank).
                s.slots = crate::tile_entity::furnace::furnace_create().slots;
                if let Some(NbtTag::List(l)) = nbt.map.get("Items") {
                    for elem in &l.elements {
                        if let NbtTag::Compound(im) = elem {
                            let slot = match im.map.get("Slot") {
                                Some(NbtTag::Byte(b)) => *b as usize,
                                _ => continue,
                            };
                            if slot < s.slots.len() {
                                let mut stack = crate::persist::read_stack(&im.map);
                                stack.count = strict_tile_count(stack.item_id, stack.count);
                                stack.damage = strict_tile_damage(stack.item_id, stack.damage);
                                if stack.count > 0 {
                                    s.slots[slot] = stack;
                                }
                            }
                        }
                    }
                }
                ctx.world.tiles.insert((x, y, z), TileData::Furnace(s));
            }
            TileData::Chest(mut s) => {
                // Vanilla readFromNBT replaces the whole bank: clear first
                // so client-removed stacks do not linger server-side.
                s.slots = crate::tile_entity::chest::chest_create().slots;
                if let Some(NbtTag::List(l)) = nbt.map.get("Items") {
                    for elem in &l.elements {
                        if let NbtTag::Compound(im) = elem {
                            let slot = match im.map.get("Slot") {
                                Some(NbtTag::Byte(b)) => *b as usize,
                                _ => continue,
                            };
                            if slot < s.slots.len() {
                                let mut stack = crate::persist::read_stack(&im.map);
                                stack.count = strict_tile_count(stack.item_id, stack.count);
                                stack.damage = strict_tile_damage(stack.item_id, stack.damage);
                                if stack.count > 0 {
                                    s.slots[slot] = stack;
                                }
                            }
                        }
                    }
                }
                ctx.world.tiles.insert((x, y, z), TileData::Chest(s));
            }
            TileData::MobSpawner(_) => {
                // Clients cannot rewrite mob spawners via Packet59ComplexEntity.
                return;
            }
        }
        ctx.broadcast.push(SessionBroadcast::TileChanged(x, y, z));
        ctx.world.mark_chunk_modified(x.div_euclid(16), z.div_euclid(16));
    }
}
