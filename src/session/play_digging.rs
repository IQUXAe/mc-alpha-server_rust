//! Block dig + harvest on `PlaySession` (mirrors handleBlockDig).
//! Split out of `session.rs`; behavior unchanged.

use crate::entity::table::Entity;
use crate::item_data::{item_max_damage, item_tool_kind};
use crate::player::digging::{DigInput, dig_on_click, dig_on_tick};
use crate::player::mining::mining_can_harvest;
use crate::session::play::PlaySession;
use crate::session::{SessionCtx, SessionOutcome};
use crate::session_packets::tile_packet;
use crate::world::tiles::TileData;


impl PlaySession {
    // ---- digging (mirrors handleBlockDig + harvestBlock) ----

    pub(crate) fn dig(
        &mut self,
        ctx: &mut SessionCtx,
        status: i8,
        x: i32,
        y: i32,
        z: i32,
        _face: i8,
    ) -> Option<SessionOutcome> {
        self.sync_held(ctx.world);
        if !(0..crate::world::WORLD_HEIGHT).contains(&y) {
            return None;
        }
        let me = self.player;
        let (px, py, pz) = match ctx.world.entities.get(me) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return None,
        };
        let dy = (py - (y as f64 + 0.5)) + 1.5;
        let dist_sq = (px - (x as f64 + 0.5)).powi(2)
            + dy.powi(2)
            + (pz - (z as f64 + 0.5)).powi(2);
        if (status == 0 || status == 1) && dist_sq > 36.0 {
            return None;
        }
        let protected = {
            let sp = ctx.world.spawn;
            crate::session::is_spawn_protected(x, z, sp, ctx.spawn_protection)
        };
        if status == 0 {
            if protected && !self.is_op(ctx) {
                self.send_block_change(ctx.world, x, y, z);
                return None;
            }
            // Clear client-side chest prediction before digging starts.
            if ctx.world.get_block_id(x, y, z) == 54 {
                if let Some(TileData::Chest(_)) = ctx.world.tiles.get(&(x, y, z)) {
                    use crate::tile_entity::chest::chest_create;
                    self.outbox.push(tile_packet(x, y, z, &TileData::Chest(chest_create())));
                }
            }
            let bid = ctx.world.get_block_id(x, y, z);
            if bid == 0 {
                return None;
            }
            if bid == 64 {
                ctx.world.toggle_door(x, y, z);
            } else if bid == 69 {
                ctx.world.toggle_lever(x, y, z);
            } else if bid == 77 {
                ctx.world.press_button(x, y, z);
            } else if bid == 73 {
                ctx.world.apply_set_notify(x, y, z, 74);
            }
            let input = self.dig_input(ctx, bid as i32);
            if dig_on_click(input) {
                self.harvest(ctx, x, y, z);
            } else {
                let same_target = self.dig.has_target
                    && self.dig.target_x == x
                    && self.dig.target_y == y
                    && self.dig.target_z == z;
                if !same_target {
                    self.dig.cur_damage = 0.0;
                    self.dig.ground_damage = 0.0;
                    self.dig.initial_cooldown = 0;
                    self.dig.target_x = x;
                    self.dig.target_y = y;
                    self.dig.target_z = z;
                    self.dig.has_target = true;
                }
            }
        } else if status == 2 {
            crate::player::digging::dig_cancel(&mut self.dig);
        } else if status == 1 {
            if !protected || self.is_op(ctx) {
                let bid = ctx.world.get_block_id(x, y, z);
                if bid > 0 {
                    let same_target = self.dig.has_target
                        && self.dig.target_x == x
                        && self.dig.target_y == y
                        && self.dig.target_z == z;
                    if !same_target {
                        self.dig.cur_damage = 0.0;
                        self.dig.ground_damage = 0.0;
                        self.dig.initial_cooldown = 0;
                        self.dig.target_x = x;
                        self.dig.target_y = y;
                        self.dig.target_z = z;
                        self.dig.has_target = true;
                    }
                    let input = self.dig_input(ctx, bid as i32);
                    let done = dig_on_tick(&mut self.dig, x, y, z, input);
                    self.dig_ticked_this_tick = true;
                    if done {
                        self.harvest(ctx, x, y, z);
                    }
                }
            }
        } else if status == 3 && dist_sq < 256.0 {
            if !protected || self.is_op(ctx) {
                let same_target = self.dig.has_target
                    && self.dig.target_x == x
                    && self.dig.target_y == y
                    && self.dig.target_z == z;
                let bid = ctx.world.get_block_id(x, y, z);
                if same_target && bid > 0 {
                    let input = self.dig_input(ctx, bid as i32);
                    let hardness_tick = crate::player::mining::mining_check_hardness(
                        input.block_id,
                        input.held_item_id,
                        input.in_water,
                        input.on_ground,
                    );
                    let ground_hardness_tick = crate::player::mining::mining_check_hardness(
                        input.block_id,
                        input.held_item_id,
                        false,
                        true,
                    );
                    let ok = self.dig.cur_damage >= 0.70
                        || self.dig.ground_damage >= 0.70
                        || (hardness_tick > 0.0 && self.dig.cur_damage + hardness_tick >= 0.70)
                        || (ground_hardness_tick > 0.0 && self.dig.ground_damage + ground_hardness_tick >= 0.70);
                    if ok {
                        self.harvest(ctx, x, y, z);
                    }
                }
            }
            self.send_block_change(ctx.world, x, y, z);
        }
        None
    }

    fn dig_input(&self, ctx: &SessionCtx, block_id: i32) -> DigInput {
        let held = self.selected_stack(ctx.world).map(|s| s.item_id).unwrap_or(0);
        let (in_water, on_ground) = match ctx.world.entities.get(self.player) {
            Some(e) => (Self::is_inside_water(ctx.world, self.player), e.body().on_ground),
            None => (false, false),
        };
        DigInput { block_id, held_item_id: held, in_water, on_ground }
    }

    /// Break a block (mirrors `harvestBlock` + `removeBlock`): container
    /// scatter first, air set, tool wear, then the block drop when the
    /// held tool can harvest.
    fn harvest(&mut self, ctx: &mut SessionCtx, x: i32, y: i32, z: i32) {
        let bid = ctx.world.get_block_id(x, y, z);
        if bid == 0 {
            return;
        }
        // Pre-removal metadata rides into the drop (doors drop from the
        // lower half only, like BlockDoor.idDropped).
        let meta = ctx.world.get_block_meta(x, y, z);
        if matches!(bid, 54 | 61 | 62 | 63 | 68) {
            ctx.world.scatter_container_tile(x, y, z);
        } else {
            ctx.world.tiles.remove(&(x, y, z));
        }
        let removed = ctx.world.apply_set_notify(x, y, z, 0);
        // Capture harvest tool id before tool wear (a tool breaking on its
        // last use still harvests the block it just broke).
        let harvest_held_id = self.selected_stack(ctx.world).map(|s| s.item_id).unwrap_or(0);
        // Tool wear on the real held slot (pick/spade/axe/sword).
        let cur = match ctx.world.entities.get(self.player) {
            Some(Entity::Player(p)) => p.inventory.current,
            _ => -1,
        };
        if (0..36).contains(&cur) {
            let mut slot = match ctx.world.entities.get(self.player) {
                Some(Entity::Player(p)) => p.inventory.main[cur as usize],
                _ => None,
            };
            let mut broke_tool = false;
            if let Some(mut s) = slot {
                if s.item_id > 0 && s.item_id < 32000 && (self.held_id <= 0 || s.item_id == self.held_id) {
                    let kind = item_tool_kind(s.item_id);
                    // Java ItemTool.hitBlock 1, ItemSword.hitBlock 2.
                    let wear = if kind == crate::item_data::ItemToolKind::Pickaxe as i32
                        || kind == crate::item_data::ItemToolKind::Spade as i32
                        || kind == crate::item_data::ItemToolKind::Axe as i32
                    {
                        1
                    } else if kind == crate::item_data::ItemToolKind::Sword as i32 {
                        2
                    } else {
                        0
                    };
                    if wear > 0 {
                        let max = item_max_damage(s.item_id);
                        crate::inventory::item_stack_damage(&mut s, wear, max);
                        if s.count <= 0 || s.damage > max {
                            slot = None;
                            broke_tool = true;
                        } else {
                            slot = Some(s);
                        }
                    }
                }
            }
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(self.player) {
                p.inventory.main[cur as usize] = slot;
            }
            if broke_tool {
                self.held_id = 0;
                self.send_inventory(ctx.world);
            }
        }
        // Block drop when harvestable (uses the pre-removal id like C++).
        // TNT never drops: breaking it primes the fuse instead (Java
        // BlockTNT.onBlockDestroyedByPlayer).
        // Ice leaves water behind when the cell below is solid/liquid
        // (Java BlockIce.onBlockRemoval).
        if removed {
            crate::player::digging::dig_cancel(&mut self.dig);
            if bid == 46 {
                ctx.world.ignite_tnt(x, y, z, 80);
                return;
            }
            if bid == 79 {
                let below_solid = ctx.world.is_solid(x, y - 1, z);
                let below_liquid = {
                    let m = ctx.world.material_at(x, y - 1, z);
                    m.is_liquid()
                };
                if below_solid || below_liquid {
                    ctx.world.apply_set_notify(x, y, z, 8);
                    return;
                }
            }
            if mining_can_harvest(bid as i32, harvest_held_id) {
                ctx.world.drop_block_for(bid, meta, x, y, z);
            }
        }
    }

    /// Progress block digging damage on server tick when the client is actively
    /// digging a latched target block (Alpha client does not send status 1 every tick).
    pub(crate) fn dig_tick(&mut self, ctx: &mut SessionCtx) {
        if self.dig_ticked_this_tick {
            self.dig_ticked_this_tick = false;
            return;
        }
        if !self.dig.has_target {
            return;
        }
        let (x, y, z) = (self.dig.target_x, self.dig.target_y, self.dig.target_z);
        if !(0..crate::world::WORLD_HEIGHT).contains(&y) {
            crate::player::digging::dig_cancel(&mut self.dig);
            return;
        }
        let me = self.player;
        let (px, py, pz) = match ctx.world.entities.get(me) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => {
                crate::player::digging::dig_cancel(&mut self.dig);
                return;
            }
        };
        let dy = (py - (y as f64 + 0.5)) + 1.5;
        let dist_sq = (px - (x as f64 + 0.5)).powi(2)
            + dy.powi(2)
            + (pz - (z as f64 + 0.5)).powi(2);
        if dist_sq > 36.0 {
            crate::player::digging::dig_cancel(&mut self.dig);
            return;
        }
        let protected = {
            let sp = ctx.world.spawn;
            crate::session::is_spawn_protected(x, z, sp, ctx.spawn_protection)
        };
        if protected && !self.is_op(ctx) {
            crate::player::digging::dig_cancel(&mut self.dig);
            return;
        }
        let bid = ctx.world.get_block_id(x, y, z);
        if bid == 0 {
            crate::player::digging::dig_cancel(&mut self.dig);
            return;
        }
        self.sync_held(ctx.world);
        let input = self.dig_input(ctx, bid as i32);
        let hardness_tick = crate::player::mining::mining_check_hardness(
            input.block_id,
            input.held_item_id,
            input.in_water,
            input.on_ground,
        );
        let ground_hardness_tick = crate::player::mining::mining_check_hardness(
            input.block_id,
            input.held_item_id,
            false,
            true,
        );
        if hardness_tick > 0.0 {
            // Keep damage progressing during network jitter up to 0.80 without
            // ever harvesting in the background. Harvesting must be driven by
            // client packet synchronization (status 1 / status 3) so that
            // crack animations and break particles play naturally on the client.
            self.dig.cur_damage = (self.dig.cur_damage + hardness_tick).min(0.80);
            self.dig.ground_damage = (self.dig.ground_damage + ground_hardness_tick).min(0.80);
        }
    }
}
