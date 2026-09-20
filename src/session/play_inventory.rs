//! Inventory apply, ghost stacks and sign text on `PlaySession`.
//! Split out of `session.rs`; behavior unchanged.

use crate::entity::table::Entity;
use crate::inventory::ItemStack;
use crate::session::play::PlaySession;
use crate::session::{SessionBroadcast, SessionCtx};
use crate::world::tiles::TileData;

impl PlaySession {
    /// Ghost fallback for a held item id the server cannot find in any
    /// real slot (desync/creative): a 1-count copy that shadows slot 35
    /// without touching real contents, like vanilla's `field_10_k`.
    pub(crate) fn ghost_stack(held_id: i32) -> ItemStack {
        ItemStack::new(held_id, 1, 0)
    }

    pub(crate) fn apply_inventory(
        &mut self,
        ctx: &mut SessionCtx,
        inv_type: i32,
        slots: &[crate::network::SlotData],
    ) {
        fn apply(bank: &mut [Option<ItemStack>], slots: &[crate::network::SlotData]) {
            let n = slots.len().min(bank.len());
            for i in 0..n {
                let id = slots[i].item_id as i32;
                if (0..32000).contains(&id) {
                    let dmg = if crate::item_data::item_max_damage(id) > 0 {
                        slots[i].damage as i32
                    } else {
                        0
                    };
                    let count = slots[i].count as i32;
                    bank[i] = if count > 0 {
                        Some(ItemStack::new(id, count, dmg))
                    } else {
                        None
                    };
                } else {
                    bank[i] = None;
                }
            }
        }
        let me = self.player;
        if inv_type == -1 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                apply(&mut p.inventory.main, slots);
            }
            if self.held_id > 0 {
                let mut found = false;
                if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                    for i in 0..36 {
                        if p.inventory.main[i].map(|s| s.item_id) == Some(self.held_id) {
                            p.inventory.current = i as i32;
                            found = true;
                            break;
                        }
                    }
                    if !found {
                        // Ghost fallback only; every real slot (35
                        // included) keeps its contents.
                        p.inventory.current = 35;
                    }
                }
                if found {
                    self.held_fallback = None;
                } else {
                    self.held_fallback = Some(Self::ghost_stack(self.held_id));
                }
            }
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
        let mut dec = flate2::read::GzDecoder::new(nbt_gz);
        let mut raw = Vec::new();
        if dec.read_to_end(&mut raw).is_err() || raw.len() > 524288 || raw.is_empty() {
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
        let (px, py, pz) = match ctx.world.entities.get(self.player) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return,
        };
        let dist_sq = (px - (x as f64 + 0.5)).powi(2)
            + (py - (y as f64 + 0.5)).powi(2)
            + (pz - (z as f64 + 0.5)).powi(2);
        if dist_sq > 64.0 {
            return;
        }
        if ctx.spawn_protection > 0 {
            let sp = ctx.world.spawn;
            let protected = (x - sp[0]).abs().max((z - sp[2]).abs()) <= ctx.spawn_protection;
            if protected && !self.is_op(ctx) {
                return;
            }
        }
        let tile = match ctx.world.tiles.get(&(x, y, z)) {
            Some(t) => *t,
            None => return,
        };
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
                if let Some(NbtTag::Short(v)) = nbt.map.get("BurnTime") {
                    s.burn_time = *v;
                }
                if let Some(NbtTag::Short(v)) = nbt.map.get("CookTime") {
                    s.cook_time = *v;
                }
                if let Some(NbtTag::Short(v)) = nbt.map.get("ItemBurnTime") {
                    s.current_item_burn_time = *v;
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
                                stack.count = stack.count.clamp(0, 64);
                                if stack.count > 0 && (0..32000).contains(&stack.item_id) {
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
                                stack.count = stack.count.clamp(0, 64);
                                if stack.count > 0 && (0..32000).contains(&stack.item_id) {
                                    s.slots[slot] = stack;
                                }
                            }
                        }
                    }
                }
                ctx.world.tiles.insert((x, y, z), TileData::Chest(s));
            }
        }
        ctx.broadcast.push(SessionBroadcast::TileChanged(x, y, z));
        ctx.world.mark_chunk_modified(x.div_euclid(16), z.div_euclid(16));
    }
}
