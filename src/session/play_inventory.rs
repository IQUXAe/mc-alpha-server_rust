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
fn strict_tile_count(item_id: i32, count: i32, is_op: bool) -> i32 {
    let allowed = if is_op {
        crate::item_data::item_is_valid(item_id)
    } else {
        crate::item_data::item_is_obtainable(item_id)
    };
    if !allowed {
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
                let is_tool = crate::item_data::item_max_damage(item_id as i32) > 0;
                let mut best_idx = None;
                // Scan hotbar slots (0..9) first, preferring damaged tools
                for i in 0..9.min(p.inventory.main.len()) {
                    if let Some(s) = p.inventory.main[i] {
                        if s.item_id == item_id as i32
                            && s.count >= count as i32
                            && (best_idx.is_none() || (is_tool && s.damage > 0))
                        {
                            best_idx = Some(i);
                            if is_tool && s.damage > 0 {
                                break;
                            }
                        }
                    }
                }
                // If not in hotbar, scan upper inventory (9..36)
                if best_idx.is_none() {
                    for i in 9..p.inventory.main.len() {
                        if let Some(s) = p.inventory.main[i] {
                            if s.item_id == item_id as i32
                                && s.count >= count as i32
                                && (best_idx.is_none() || (is_tool && s.damage > 0))
                            {
                                best_idx = Some(i);
                                if is_tool && s.damage > 0 {
                                    break;
                                }
                            }
                        }
                    }
                }
                if let Some(idx) = best_idx {
                    if let Some(s) = &mut p.inventory.main[idx] {
                        damage = s.damage;
                        has_item = true;
                        s.count -= count as i32;
                        if s.count == 0 {
                            p.inventory.main[idx] = None;
                        }
                    }
                }
            }
            // GUI drops whose stack left the armor bank (helmet/chest/
            // legs/boots, plus the wearable pumpkin — `SlotArmor` allows
            // `Block.pumpkin`): the cursor stack is not part of any
            // `Packet5` bank, so until the client's 1-second sync fires
            // the server copy still holds it in armor.
            if !has_item {
                for slot in &mut p.inventory.armor {
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
            // Same for the 2x2 crafting grid (closing the inventory GUI
            // drops whatever was left in it).
            if !has_item {
                for slot in &mut p.inventory.crafting {
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

        // Check recent_removed_stacks (stack was dragged with mouse cursor in GUI and slot was already cleared)
        if !has_item {
            if let Some(pos) = self.recent_removed_stacks.iter().rposition(|s| s.item_id == item_id as i32) {
                let s = self.recent_removed_stacks.remove(pos);
                damage = s.damage;
                has_item = true;
            }
        }

        // Fallback: check last_tool_damage if item is a tool
        if damage == 0 && crate::item_data::item_max_damage(item_id as i32) > 0 {
            if let Some(&last_dmg) = self.last_tool_damage.get(&(item_id as i32)) {
                if last_dmg > 0 {
                    damage = last_dmg;
                }
            }
        }

        if !has_item {
            // Vanilla fallback (`NetServerHandler.handlePickupSpawn`
            // spawns the entity with no inventory check): the stack can
            // legitimately exist in no server bank — it sits on the
            // client's GUI cursor (`draggingItemStack`), which no
            // `Packet5` bank covers, and the client's periodic sync may
            // already have cleared its source slot server-side. Reject
            // the drop only when the id itself is unobtainable; a full
            // resync here punished the client for the gap and destroyed
            // the cursor stack (the evaporating-pumpkin bug).
            let is_op = self.is_op(ctx);
            let allowed = if is_op {
                crate::item_data::item_is_valid(item_id as i32)
            } else {
                crate::item_data::item_is_obtainable(item_id as i32)
            };
            if !allowed {
                return;
            }
        } else {
            self.sync_held(ctx.world);
            if self.selected_stack(ctx.world).is_none() {
                self.held_id = 0;
            }
        }

        if damage > 0 {
            self.last_tool_damage.insert(item_id as i32, damage);
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
        // code uses: unobtainable/unknown ids are dropped (ops may hold any
        // valid id), counts to the per-item max stack, damage to the
        // per-item max durability.
        let is_op = self.is_op(ctx);
        let mut removed = Vec::new();
        fn apply(
            bank: &mut [Option<ItemStack>],
            slots: &[crate::network::SlotData],
            is_op: bool,
            removed: &mut Vec<ItemStack>,
            last_tool_damage: &mut std::collections::HashMap<i32, i32>,
        ) {
            let n = slots.len().min(bank.len());
            for i in 0..n {
                let id = slots[i].item_id as i32;
                let allowed = if is_op {
                    crate::item_data::item_is_valid(id)
                } else {
                    crate::item_data::item_is_obtainable(id)
                };
                let max_stack = crate::player::inventory::inventory_max_stack_size(id).max(1);
                let count = if allowed { (slots[i].count as i32).clamp(0, max_stack) } else { 0 };
                let max_dmg = crate::item_data::item_max_damage(id);
                let dmg = if allowed && max_dmg > 0 {
                    (slots[i].damage as i32).clamp(0, max_dmg)
                } else {
                    0
                };
                if allowed && dmg > 0 {
                    last_tool_damage.insert(id, dmg);
                }

                if let Some(old_s) = bank[i] {
                    if !allowed || count == 0 || old_s.item_id != id {
                        removed.push(old_s);
                    } else if count < old_s.count {
                        removed.push(ItemStack::new(old_s.item_id, old_s.count - count, old_s.damage));
                    }
                }

                bank[i] = if allowed && count > 0 {
                    Some(ItemStack::new(id, count, dmg))
                } else {
                    None
                };
            }
        }
        let me = self.player;
        if inv_type == -1 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                apply(&mut p.inventory.main, slots, is_op, &mut removed, &mut self.last_tool_damage);
            }
            self.sync_held(ctx.world);
        } else if inv_type == -2 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                apply(&mut p.inventory.crafting, slots, is_op, &mut removed, &mut self.last_tool_damage);
            }
        } else if inv_type == -3 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                apply(&mut p.inventory.armor, slots, is_op, &mut removed, &mut self.last_tool_damage);
            }
        }
        for r in removed {
            self.recent_removed_stacks.push(r);
        }
        if self.recent_removed_stacks.len() > 16 {
            let excess = self.recent_removed_stacks.len() - 16;
            self.recent_removed_stacks.drain(0..excess);
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
        let is_op = self.is_op(ctx);
        if ctx.spawn_protection > 0 {
            let sp = ctx.world.spawn;
            let protected = crate::session::is_spawn_protected(x, z, sp, ctx.spawn_protection);
            if protected && !is_op {
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
                        // Sanitized storage (NUL/astral chars would crash
                        // every vanilla client reading the tile packet).
                        crate::tile_entity::sign::sign_set_line(&mut s, i as i32, text);
                    }
                }
                ctx.world.tiles.insert((x, y, z), TileData::Sign(s));
            }
            TileData::Furnace(mut s) => {
                // Timers are server-authoritative: burn/cook state is
                // driven by `furnace_tick_native` alone. Accepting client
                // `BurnTime`/`CookTime`/`ItemBurnTime` (vanilla
                // `readFromNBT` copies them verbatim — a known Alpha
                // exploit) let a forged packet set a cold furnace to
                // `CookTime=200` for instant smelts or 20000 free burn
                // ticks. The vanilla client never needs its timers
                // reflected: its GUI copy keeps animating from the
                // per-tick tile broadcasts.
                // Same replace-not-merge rule as chests (vanilla
                // readFromNBT starts from a fresh bank).
                //
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
                                stack.count = strict_tile_count(stack.item_id, stack.count, is_op);
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
                                stack.count = strict_tile_count(stack.item_id, stack.count, is_op);
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
