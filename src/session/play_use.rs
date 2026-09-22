//! Block placement, block activation and air-use on `PlaySession`
//! (mirrors handlePlace + activeBlockOrUseItem).
//! Split out of `session.rs`; behavior unchanged.

use crate::entity::table::Entity;
use crate::inventory::ItemStack;
use crate::item_data::{item_food_heal, item_max_damage};
use crate::item_use::item_food_bite;
use crate::item_verbs::{
    item_block_use, item_boat_aim, item_boat_throw, item_flint_use, item_hoe_use, item_seeds_use,
    item_sign_use, BlockPlace, BoatAimIn, ItemUseWorld,
};
use crate::session::play::PlaySession;
use crate::session::{SessionCtx, SessionOutcome};
use crate::session_packets::pkt_health;

impl PlaySession {
    /// Write a mutated stack into the real current slot (air-use path).
    fn write_back_current(&mut self, ctx: &mut SessionCtx, s: ItemStack) {
        let live = !s.is_empty();
        // Ghost fallback shadows the current slot while active.
        if self.held_fallback.is_some() {
            self.held_fallback = if live { Some(s) } else { None };
            return;
        }
        let cur = match ctx.world.entities.get(self.player) {
            Some(Entity::Player(p)) => p.inventory.current,
            _ => return,
        };
        if !(0..36).contains(&cur) {
            return;
        }
        if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(self.player) {
            p.inventory.main[cur as usize] =
                if !s.is_empty() { Some(s) } else { None };
        }
    }
}

impl PlaySession {
    // ---- placement (mirrors handlePlace + activeBlockOrUseItem) ----

    pub(crate) fn place(
        &mut self,
        ctx: &mut SessionCtx,
        item_id: i16,
        x: i32,
        y: i32,
        z: i32,
        direction: i8,
    ) -> Option<SessionOutcome> {
        let me = self.player;
        if direction == -1 {
            // Right-click in air: use the held item.
            let held = self.selected_stack(ctx.world);
            if let Some(s) = held {
                self.use_item_air(ctx, s);
            }
            return None;
        }
        if !(0..crate::world::WORLD_HEIGHT).contains(&y) {
            return None;
        }
        let (px, py, pz) = match ctx.world.entities.get(me) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return None,
        };
        let dist_sq = (px - (x as f64 + 0.5)).powi(2)
            + (py - (y as f64 + 0.5)).powi(2)
            + (pz - (z as f64 + 0.5)).powi(2);
        if dist_sq > 64.0 {
            self.send_block_change(ctx.world, x, y, z);
            return None;
        }
        let dir = (direction as u8) as i32;
        let prot = {
            let sp = ctx.world.spawn;
            crate::session::is_spawn_protected(x, z, sp, ctx.spawn_protection)
        };
        if prot && !self.is_op(ctx) {
            self.send_block_change(ctx.world, x, y, z);
            return None;
        }
        // Chest/furnace contents first (client opens the GUI early).
        let clicked = ctx.world.get_block_id(x, y, z);
        if matches!(clicked, 54 | 61 | 62) {
            self.send_tile(ctx.world, x, y, z);
        }
        // Find the stack: selected if it matches, else first main match.
        let mut stack: Option<ItemStack> = None;
        let mut stack_slot: Option<usize> = None;
        if item_id >= 0 {
            if let Some(s) = self.selected_stack(ctx.world) {
                if s.item_id == item_id as i32 && s.count > 0 {
                    stack = Some(s);
                    stack_slot = match ctx.world.entities.get(me) {
                        Some(Entity::Player(p)) => {
                            let cur = p.inventory.current;
                            (0..36).contains(&cur).then_some(cur as usize)
                        }
                        _ => None,
                    };
                }
            }
            if stack.is_none() {
                if let Some(Entity::Player(p)) = ctx.world.entities.get(me) {
                    for (i, slot) in p.inventory.main.iter().enumerate() {
                        if let Some(s) = slot {
                            if s.item_id == item_id as i32 && s.count > 0 {
                                stack = Some(*s);
                                stack_slot = Some(i);
                                break;
                            }
                        }
                    }
                }
            }
        }
        if let Some(mut s) = stack {
            let water = ctx.world.material_at(x, y, z) == crate::material::Material::WATER;
            if s.item_id == 333 && water {
                // Boat on water: spawn + consume, like C++.
                let bid = ctx.world.entities.alloc_id();
                let mut b = crate::entity::table::Body::new(bid, 1.5, 0.6, 0.3);
                b.set_position(x as f64 + 0.5, y as f64 + 1.5, z as f64 + 0.5);
                ctx.world.entities.insert(Entity::Boat(crate::entity::table::BoatEnt {
                    body: b,
                    time_since_hit: 0,
                    damage_taken: 0,
                    forward_dir: 1,
                }));
                if s.count > 0 {
                    s.count -= 1;
                }
                self.write_stack_slot(ctx, stack_slot, s);
            } else if matches!(clicked, 54 | 58 | 61 | 62) {
                // GUI blocks swallow the click (chest/furnace already synced).
                let _ = self.activated_block(ctx, clicked, x, y, z);
            } else {
                let used_first = self.active_block_or_use(ctx, &mut s, x, y, z, dir);
                if !used_first && s.item_id == 333 {
                    // Boat fallback: right-click throw on blocks.
                    self.use_item_air(ctx, s);
                } else {
                    self.write_stack_slot(ctx, stack_slot, s);
                }
            }
        }
        // Drop emptied stacks like C++.
        if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
            for slot in p.inventory.main.iter_mut() {
                if let Some(s) = slot {
                    if s.count <= 0 {
                        *slot = None;
                    }
                }
            }
        }
        self.send_inventory(ctx.world);
        // Rollback views at both cells.
        self.send_block_change(ctx.world, x, y, z);
        let (nx, ny, nz) = match dir {
            0 => (x, y - 1, z),
            1 => (x, y + 1, z),
            2 => (x, y, z - 1),
            3 => (x, y, z + 1),
            4 => (x - 1, y, z),
            5 => (x + 1, y, z),
            _ => (x, y, z),
        };
        self.send_block_change(ctx.world, nx, ny, nz);
        None
    }

    /// Write a mutated held stack back to its slot (or the fallback copy
    /// while a ghost is active, so real slot contents are never shadowed).
    fn write_stack_slot(&mut self, ctx: &mut SessionCtx, slot: Option<usize>, s: ItemStack) {
        let live = !s.is_empty();
        if self.held_fallback.is_some() {
            self.held_fallback = if live { Some(s) } else { None };
            return;
        }
        if let Some(i) = slot {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(self.player) {
                if i < 36 {
                    p.inventory.main[i] = if live { Some(s) } else { None };
                }
            }
        }
    }

    /// GUI block activation (mirrors blockActivated for chest/furnace/
    /// workbench; the chest tile-removal line in C++ is a data-eating bug
    /// and is deliberately NOT mirrored).
    fn activated_block(&mut self, ctx: &mut SessionCtx, clicked: u8, x: i32, y: i32, z: i32) -> bool {
        match clicked {
            54 => {
                if ctx.world.is_solid(x, y + 1, z) {
                    return true;
                }
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    if ctx.world.get_block_id(x + dx, y, z + dz) == 54
                        && ctx.world.is_solid(x + dx, y + 1, z + dz)
                    {
                        return true;
                    }
                }
                self.send_tile(ctx.world, x, y, z);
                true
            }
            61 | 62 => {
                self.send_tile(ctx.world, x, y, z);
                true
            }
            58 => true,
            _ => false,
        }
    }

    /// Item-on-block routing (mirrors `activeBlockOrUseItem` + the
    /// per-item `onItemUse` stack rules).
    fn active_block_or_use(
        &mut self,
        ctx: &mut SessionCtx,
        s: &mut ItemStack,
        x: i32,
        y: i32,
        z: i32,
        side: i32,
    ) -> bool {
        let clicked = ctx.world.get_block_id(x, y, z);
        if clicked > 0 && matches!(clicked, 54 | 58 | 61 | 62) {
            return self.activated_block(ctx, clicked, x, y, z);
        }
        if s.count <= 0 {
            return false;
        }
        let me = self.player;
        let yaw = ctx.world.entities.get(me).map(|e| e.body().yaw).unwrap_or(0.0);
        // Explicit borrow split: the world and the session reborrowed into
        // the verb context.
        let mut u = ItemUseWorld {
            world: &mut *ctx.world,
            session: &mut *self,
        };
        let pos = crate::block::pos::BlockPos::new(x, y, z);
        match s.item_id {
            290..=294 => {
                if !item_hoe_use(&mut u, 295, pos) {
                    false
                } else {
                    let max = item_max_damage(s.item_id);
                    crate::inventory::item_stack_damage(&mut *s, 1, max);
                    true
                }
            }
            295 => {
                if side != 1 || !item_seeds_use(&mut u, pos, side) {
                    false
                } else {
                    if s.count > 0 {
                        s.count -= 1;
                    }
                    true
                }
            }
            259 => {
                let max = item_max_damage(s.item_id);
                let out = item_flint_use(&mut u, s.damage, max, pos, side);
                s.damage = out.new_damage;
                if out.broke {
                    s.count = 0;
                }
                true
            }
            323 => {
                if !item_sign_use(&mut u, pos, side, yaw) {
                    false
                } else {
                    if s.count > 0 {
                        s.count -= 1;
                    }
                    true
                }
            }
            333 => false,
            1..=255 => {
                let place = BlockPlace {
                    block_id: s.item_id as u8,
                    stack_count: s.count,
                    side,
                    yaw,
                };
                if !item_block_use(&mut u, place, pos) {
                    false
                } else {
                    if s.count > 0 {
                        s.count -= 1;
                    }
                    true
                }
            }
            _ => false,
        }
    }

    /// Right-click in air (mirrors `useItem`: food bites with heal,
    /// soup to bowl, boats via aim+throw).
    pub(crate) fn use_item_air(&mut self, ctx: &mut SessionCtx, mut s: ItemStack) -> bool {
        let me = self.player;
        let heal = item_food_heal(s.item_id);
        if heal > 0 {
            let bite = item_food_bite(s.count, heal);
            if s.item_id == 282 {
                s.item_id = 281;
                s.count = 1;
                s.damage = 0;
            } else {
                s.count = bite.new_count;
            }
            if bite.heal > 0 {
                if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                    // Java EntityLiving.heal:283 also resets hurtResist = max/2.
                    if bite.heal > 0 && !p.living.body.dead && p.living.health > 0 {
                        p.living.hurt_resist = p.living.max_hurt_resist / 2;
                    }
                    p.living.health = crate::entity::living::living_heal(
                        p.living.health,
                        p.living.max_health,
                        bite.heal,
                        p.living.body.dead,
                    );
                }
                self.outbox.push(pkt_health(match ctx.world.entities.get(me) {
                    Some(Entity::Player(p)) => p.living.health as i8,
                    _ => 0,
                }));
            }
            self.write_back_current(ctx, s);
            self.send_inventory(ctx.world);
            return true;
        }
        if s.item_id == 333 {
            let (pyaw, ppitch, ppos, pyoff, prev_yaw, prev_pitch, prev_pos) =
                match ctx.world.entities.get(me) {
                    Some(e) => {
                        let b = e.body();
                        (b.yaw, b.pitch, b.pos, b.y_offset as f64, b.prev_yaw, b.prev_pitch, b.prev_pos)
                    }
                    None => return false,
                };
            let aim = item_boat_aim(BoatAimIn {
                prev_yaw,
                yaw: pyaw,
                prev_pitch,
                pitch: ppitch,
                prev: prev_pos,
                cur: ppos,
                y_offset: pyoff,
            });
            let mut u = ItemUseWorld {
                world: &mut *ctx.world,
                session: &mut *self,
            };
            let Some([hx, hy, hz]) =
                item_boat_throw(&mut u, [aim.sx, aim.sy, aim.sz], [aim.ex, aim.ey, aim.ez])
            else {
                return false;
            };
            let bid = ctx.world.entities.alloc_id();
            let mut b = crate::entity::table::Body::new(bid, 1.5, 0.6, 0.3);
            b.set_position(hx as f64 + 0.5, hy as f64 + 1.5, hz as f64 + 0.5);
            ctx.world.entities.insert(Entity::Boat(crate::entity::table::BoatEnt {
                body: b,
                time_since_hit: 0,
                damage_taken: 0,
                forward_dir: 1,
            }));
            if s.count > 0 {
                s.count -= 1;
            }
            self.write_back_current(ctx, s);
            self.send_inventory(ctx.world);
            return true;
        }
        false
    }
}
