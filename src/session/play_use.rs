//! Block placement, block activation and air-use on `PlaySession`
//! (mirrors handlePlace + activeBlockOrUseItem).
//! Split out of `session.rs`; behavior unchanged.

use crate::entity::table::Entity;
use crate::inventory::ItemStack;
use crate::item_data::{item_food_heal, item_max_damage};
use crate::item_use::item_food_bite;
use crate::item_verbs::{
    item_block_use, item_boat_aim, item_boat_throw, item_door_use, item_flint_use, item_hoe_use,
    item_record_use, item_seeds_use, item_sign_use, BlockPlace, BoatAimIn, ItemUseWorld,
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
        let clicked = ctx.world.get_block_id(x, y, z);
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
            } else if matches!(clicked, 54 | 58 | 61 | 62 | 64 | 69 | 77)
                || (clicked == 84 && ctx.world.get_block_meta(x, y, z) > 0)
            {
                let _ = self.activated_block(ctx, clicked, x, y, z);
            } else {
                let used_first = self.active_block_or_use(ctx, &mut s, x, y, z, dir);
                if !used_first && matches!(s.item_id, 325 | 326 | 327 | 333) {
                    self.use_item_air(ctx, s);
                } else {
                    self.write_stack_slot(ctx, stack_slot, s);
                }
            }
        } else if clicked > 0 {
            let _ = self.activated_block(ctx, clicked, x, y, z);
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
        if matches!(ctx.world.get_block_id(nx, ny, nz), 54 | 61 | 62) {
            self.send_tile(ctx.world, nx, ny, nz);
        }
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

    /// Block activation (mirrors `blockActivated` for chest/furnace/
    /// workbench/doors/levers/buttons/jukebox).
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
                for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    if ctx.world.get_block_id(x + dx, y, z + dz) == 54 {
                        self.send_tile(ctx.world, x + dx, y, z + dz);
                    }
                }
                true
            }
            61 | 62 => {
                self.send_tile(ctx.world, x, y, z);
                true
            }
            58 => true,
            64 => ctx.world.toggle_door(x, y, z),
            71 => true,
            69 => ctx.world.toggle_lever(x, y, z),
            77 => ctx.world.press_button(x, y, z),
            84 => ctx.world.eject_jukebox_record(x, y, z),
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
        if clicked > 0
            && (matches!(clicked, 54 | 58 | 61 | 62 | 64 | 69 | 77)
                || (clicked == 84 && ctx.world.get_block_meta(x, y, z) > 0))
        {
            return self.activated_block(ctx, clicked, x, y, z);
        }
        if s.count <= 0 {
            return false;
        }
        if matches!(s.item_id, 325..=327) {
            return self.use_bucket_on_block(ctx, s, x, y, z, side);
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
            324 | 330 => {
                let door_id = if s.item_id == 324 { 64 } else { 71 };
                if !item_door_use(&mut u, pos, side, yaw, door_id) {
                    false
                } else {
                    if s.count > 0 {
                        s.count -= 1;
                    }
                    true
                }
            }
            2256 | 2257 => {
                if !item_record_use(&mut u, pos, s.item_id) {
                    false
                } else {
                    if s.count > 0 {
                        s.count -= 1;
                    }
                    true
                }
            }
            331 | 338 => {
                let block_id = if s.item_id == 331 { 55 } else { 83 };
                let place = BlockPlace {
                    block_id,
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

    fn use_bucket_on_block(
        &mut self,
        ctx: &mut SessionCtx,
        s: &mut ItemStack,
        x: i32,
        y: i32,
        z: i32,
        side: i32,
    ) -> bool {
        let (nx, ny, nz) = match side {
            0 => (x, y - 1, z),
            1 => (x, y + 1, z),
            2 => (x, y, z - 1),
            3 => (x, y, z + 1),
            4 => (x - 1, y, z),
            5 => (x + 1, y, z),
            _ => (x, y, z),
        };
        if s.item_id == 325 {
            for (tx, ty, tz) in [(x, y, z), (nx, ny, nz)] {
                let bid = ctx.world.get_block_id(tx, ty, tz);
                let meta = ctx.world.get_block_meta(tx, ty, tz);
                if (bid == 8 || bid == 9) && meta == 0 {
                    ctx.world.apply_set_notify(tx, ty, tz, 0);
                    *s = ItemStack::new(326, 1, 0);
                    return true;
                }
                if (bid == 10 || bid == 11) && meta == 0 {
                    ctx.world.apply_set_notify(tx, ty, tz, 0);
                    *s = ItemStack::new(327, 1, 0);
                    return true;
                }
            }
            return false;
        }
        if !(0..crate::world::WORLD_HEIGHT).contains(&ny) {
            return false;
        }
        let target_id = ctx.world.get_block_id(nx, ny, nz);
        if target_id == 0 || !ctx.world.material_at(nx, ny, nz).is_solid() {
            let fluid_id = if s.item_id == 326 { 8 } else { 10 };
            if target_id != 0 && !matches!(target_id, 8..=11) {
                ctx.world.drop_block_as_item(nx, ny, nz);
            }
            ctx.world.apply_set_meta_notify(nx, ny, nz, fluid_id, 0);
            *s = ItemStack::new(325, 1, 0);
            return true;
        }
        false
    }

    /// Right-click in air (mirrors `useItem`: food bites with heal,
    /// soup to bowl, buckets, and boats via aim+throw).
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
        if matches!(s.item_id, 325..=327) {
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
            let Some([hx, hy, hz]) =
                ctx.world.ray_trace_hit_liquids([aim.sx, aim.sy, aim.sz], [aim.ex, aim.ey, aim.ez])
            else {
                return false;
            };
            if s.item_id == 325 {
                let bid = ctx.world.get_block_id(hx, hy, hz);
                let meta = ctx.world.get_block_meta(hx, hy, hz);
                if (bid == 8 || bid == 9) && meta == 0 {
                    ctx.world.apply_set_notify(hx, hy, hz, 0);
                    s = ItemStack::new(326, 1, 0);
                    self.write_back_current(ctx, s);
                    self.send_inventory(ctx.world);
                    return true;
                }
                if (bid == 10 || bid == 11) && meta == 0 {
                    ctx.world.apply_set_notify(hx, hy, hz, 0);
                    s = ItemStack::new(327, 1, 0);
                    self.write_back_current(ctx, s);
                    self.send_inventory(ctx.world);
                    return true;
                }
                return false;
            }
            // Place fluid at the last air/non-solid cell before the hit cell along the ray
            let fluid_id = if s.item_id == 326 { 8 } else { 10 };
            let (mut tx, mut ty, mut tz) = (hx, hy, hz);
            if ctx.world.material_at(hx, hy, hz).is_solid() {
                let dx = aim.sx - (hx as f64 + 0.5);
                let dy = aim.sy - (hy as f64 + 0.5);
                let dz = aim.sz - (hz as f64 + 0.5);
                if dy.abs() >= dx.abs() && dy.abs() >= dz.abs() {
                    ty += if dy >= 0.0 { 1 } else { -1 };
                } else if dx.abs() >= dz.abs() {
                    tx += if dx >= 0.0 { 1 } else { -1 };
                } else {
                    tz += if dz >= 0.0 { 1 } else { -1 };
                }
            }
            if (0..crate::world::WORLD_HEIGHT).contains(&ty)
                && !ctx.world.material_at(tx, ty, tz).is_solid()
            {
                ctx.world.apply_set_meta_notify(tx, ty, tz, fluid_id, 0);
                s = ItemStack::new(325, 1, 0);
                self.write_back_current(ctx, s);
                self.send_inventory(ctx.world);
                return true;
            }
            return false;
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
