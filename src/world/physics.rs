//! Body movement, loose items, falling blocks and boats on [`World`].
//! Split out of `world.rs`; behavior unchanged.

use crate::entity::table::{Body, Entity, EntityId};
use crate::math_helper::floor_double;
use crate::world::{World, is_replaceable};

impl World {
    /// Spawn a loose item into the table (mirrors the common drop shape:
    /// default 10-tick pickup delay).
    pub fn spawn_item_entity(&mut self, item_id: i32, count: i32, damage: i32, x: f64, y: f64, z: f64) -> EntityId {
        use crate::entity::table::ItemEnt;
        let id = self.entities.alloc_id();
        let mut b = Body::new(id, 0.25, 0.25, 0.125);
        b.set_position(x, y, z);
        self.entities.insert(Entity::Item(ItemEnt {
            body: b,
            item_id,
            count,
            damage,
            age: 0,
            pickup_delay: 10,
        }));
        self.mark_chunk_modified((x.floor() as i32) >> 4, (z.floor() as i32) >> 4);
        id
    }

    /// Y-X-Z collision move for one body (mirrors `Entity::moveEntity`
    /// without the soil-walking sound, which arrives with the block phase).
    /// Returns the fall event distance when `onFall` must fire.
    pub fn move_body(&mut self, id: EntityId, dx: f64, dy: f64, dz: f64) -> Option<f32> {
        let (orig, no_clip, step, was_ground, suppress, fall, sneaking, old_chunk, persistent) =
            match self.entities.get(id) {
                Some(e) => {
                    let b = e.body();
                    let sneak = match e {
                        Entity::Player(p) => p.living.sneaking,
                        _ => false,
                    };
                    let pers = matches!(
                        e,
                        Entity::Item(_)
                            | Entity::Mob(_)
                            | Entity::Animal(_)
                            | Entity::Boat(_)
                            | Entity::Arrow(_)
                            | Entity::Falling(_)
                    );
                    (
                        b.bounding_box,
                        b.no_clip,
                        b.step_height,
                        b.on_ground,
                        b.suppress_fall_state,
                        b.fall_distance,
                        sneak,
                        ((b.pos[0].floor() as i32) >> 4, (b.pos[2].floor() as i32) >> 4),
                        pers,
                    )
                }
                None => return None,
            };
        let (old_x, old_y, old_z) = (dx, dy, dz);
        let (mut mx, mut my, mut mz) = (dx, dy, dz);
        // Sneak edge-stop (Java Entity.moveEntity:212-234): on ground while
        // sneaking, trim each horizontal axis in 0.05 steps so the destination
        // still has ground below.
        if was_ground && sneaking {
            let mut nx = mx;
            while nx != 0.0 && self.colliding_boxes(&orig.get_offset_bounding_box(nx, -1.0, 0.0)).is_empty() {
                if (-0.05..0.05).contains(&nx) {
                    nx = 0.0;
                } else if nx > 0.0 {
                    nx -= 0.05;
                } else {
                    nx += 0.05;
                }
            }
            mx = nx;
            let mut nz = mz;
            while nz != 0.0 && self.colliding_boxes(&orig.get_offset_bounding_box(0.0, -1.0, nz)).is_empty() {
                if (-0.05..0.05).contains(&nz) {
                    nz = 0.0;
                } else if nz > 0.0 {
                    nz -= 0.05;
                } else {
                    nz += 0.05;
                }
            }
            mz = nz;
        }
        let mut work = orig;
        if no_clip {
            if let Some(e) = self.entities.get_mut(id) {
                let b = e.body_mut();
                let (px, py, pz) = (b.pos[0] + mx, b.pos[1] + my, b.pos[2] + mz);
                b.set_position(px, py, pz);
            }
        } else {
            let boxes = self.colliding_boxes(&work.add_coord(mx, my, mz));
            for cb in &boxes {
                my = cb.calculate_y_offset(&work, my);
            }
            work.offset(0.0, my, 0.0);
            for cb in &boxes {
                mx = cb.calculate_x_offset(&work, mx);
            }
            work.offset(mx, 0.0, 0.0);
            for cb in &boxes {
                mz = cb.calculate_z_offset(&work, mz);
            }
            work.offset(0.0, 0.0, mz);

            if step > 0.0 && (was_ground || (old_y != my && old_y < 0.0)) && (old_x != mx || old_z != mz) {
                let boxes = self.colliding_boxes(&orig.add_coord(old_x, step as f64, old_z));
                let (mut sx, mut sy, mut sz) = (old_x, step as f64, old_z);
                let mut sbox = orig;
                for cb in &boxes {
                    sy = cb.calculate_y_offset(&sbox, sy);
                }
                sbox.offset(0.0, sy, 0.0);
                for cb in &boxes {
                    sx = cb.calculate_x_offset(&sbox, sx);
                }
                sbox.offset(sx, 0.0, 0.0);
                for cb in &boxes {
                    sz = cb.calculate_z_offset(&sbox, sz);
                }
                sbox.offset(0.0, 0.0, sz);
                if sx * sx + sz * sz > mx * mx + mz * mz {
                    work = sbox;
                    mx = sx;
                    my = sy;
                    mz = sz;
                }
            }

            let (falling, new_chunk) = {
                let e = self.entities.get_mut(id)?;
                let b = e.body_mut();
                b.bounding_box = work;
                b.pos[0] = (work.min_x + work.max_x) / 2.0;
                b.pos[1] = work.min_y + b.y_offset as f64;
                b.pos[2] = (work.min_z + work.max_z) / 2.0;
                b.collided_horiz = old_x != mx || old_z != mz;
                b.collided_vert = old_y != my;
                b.on_ground = old_y != my && old_y < 0.0;
                if old_x != mx {
                    b.motion[0] = 0.0;
                }
                if old_y != my {
                    b.motion[1] = 0.0;
                }
                if old_z != mz {
                    b.motion[2] = 0.0;
                }
                (
                    (b.on_ground, my),
                    ((b.pos[0].floor() as i32) >> 4, (b.pos[2].floor() as i32) >> 4),
                )
            };
            if persistent && new_chunk != old_chunk {
                self.mark_chunk_modified(old_chunk.0, old_chunk.1);
                self.mark_chunk_modified(new_chunk.0, new_chunk.1);
            }
            // Contact damage (mirrors the `onEntityCollidedWithBlock`
            // sweep at the tail of `Entity.moveEntity`): any cactus in
            // the post-move box deals 1 through the pipeline.
            self.cactus_contact(id);
            self.soul_sand_contact(id);
            self.pressure_plate_contact(id);
            self.entity_walking_contact(id);
            // Lava/fire touch (`World.func_523_c` via `Entity.moveEntity`,
            // Entity.java:395-411): fire/lava in the box deals 1 and
            // ignites for 300 ticks.
            self.lava_contact(id);
            if !suppress {
                let (nd, ev) =
                    crate::entity::physics::entity_fall_step(falling.0, falling.1, fall);
                if let Some(e) = self.entities.get_mut(id) {
                    e.body_mut().fall_distance = nd;
                }
                if let Some(ev) = ev {
                    return Some(ev);
                }
            }
        }
        None
    }

    /// Soul sand (`88`) horizontal speed damping (`BlockSlowSand.onEntityCollidedWithBlock`).
    fn soul_sand_contact(&mut self, id: EntityId) {
        let bbox = match self.entities.get(id) {
            Some(e) => e.body().bounding_box,
            None => return,
        };
        let (x0, y0, z0) = (
            floor_double(bbox.min_x),
            floor_double(bbox.min_y),
            floor_double(bbox.min_z),
        );
        let (x1, y1, z1) = (
            floor_double(bbox.max_x),
            floor_double(bbox.max_y),
            floor_double(bbox.max_z),
        );
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    if self.get_block_id(x, y, z) == 88 {
                        if let Some(e) = self.entities.get_mut(id) {
                            let b = e.body_mut();
                            b.motion[0] *= 0.4;
                            b.motion[2] *= 0.4;
                        }
                    }
                }
            }
        }
    }

    /// Stone (`70`) and wooden (`72`) pressure plate contact (`BlockPressurePlate.onEntityCollidedWithBlock`).
    fn pressure_plate_contact(&mut self, id: EntityId) {
        let bbox = match self.entities.get(id) {
            Some(e) => e.body().bounding_box,
            None => return,
        };
        let (x0, y0, z0) = (
            floor_double(bbox.min_x),
            floor_double(bbox.min_y),
            floor_double(bbox.min_z),
        );
        let (x1, y1, z1) = (
            floor_double(bbox.max_x),
            floor_double(bbox.max_y),
            floor_double(bbox.max_z),
        );
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    let bid = self.get_block_id(x, y, z);
                    if (bid == 70 || bid == 72) && self.get_block_meta(x, y, z) == 0 {
                        self.update_pressure_plate(x, y, z, bid);
                    }
                }
            }
        }
    }

    /// Lava/fire touch for one living body (mirrors `World.func_523_c` as
    /// fired from the tail of `Entity.moveEntity`, Entity.java:395-411:
    /// any fire (51), still lava (11) or flowing lava (10) cell
    /// intersecting the box deals 1 damage and ignites for 300 ticks.
    /// Water must NOT trigger this — it only extinguishes (see
    /// `tick_living`). Only mobs, animals and players qualify.
    fn lava_contact(&mut self, id: EntityId) {
        let bbox = match self.entities.get(id) {
            Some(Entity::Mob(m)) => m.living.body.bounding_box,
            Some(Entity::Animal(a)) => a.living.body.bounding_box,
            Some(Entity::Player(p)) => p.living.body.bounding_box,
            _ => return,
        };
        let (x0, y0, z0) = (
            floor_double(bbox.min_x),
            floor_double(bbox.min_y),
            floor_double(bbox.min_z),
        );
        let (x1, y1, z1) = (
            floor_double(bbox.max_x),
            floor_double(bbox.max_y),
            floor_double(bbox.max_z),
        );
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    // Vanilla `func_523_c`: fire, still lava, flowing lava.
                    // (`is_lava` is lava-only now that `Material` carries
                    // singleton identity; fire is a block id, not lava.)
                    if self.is_lava(x, y, z) || self.get_block_id(x, y, z) == 51 {
                        self.attack_living(id, 1, None);
                        match self.entities.get_mut(id) {
                            Some(Entity::Mob(m)) => {
                                m.living.body.fire = m.living.body.fire.max(300)
                            }
                            Some(Entity::Animal(a)) => {
                                a.living.body.fire = a.living.body.fire.max(300)
                            }
                            Some(Entity::Player(p)) => {
                                p.living.body.fire = p.living.body.fire.max(300)
                            }
                            _ => {}
                        }
                        return;
                    }
                }
            }
        }
    }

    fn cactus_contact(&mut self, id: EntityId) {
        let bbox = match self.entities.get(id) {
            Some(Entity::Mob(m)) => m.living.body.bounding_box,
            Some(Entity::Animal(a)) => a.living.body.bounding_box,
            Some(Entity::Player(p)) => p.living.body.bounding_box,
            _ => return,
        };
        let (x0, y0, z0) = (
            floor_double(bbox.min_x),
            floor_double(bbox.min_y),
            floor_double(bbox.min_z),
        );
        let (x1, y1, z1) = (
            floor_double(bbox.max_x),
            floor_double(bbox.max_y),
            floor_double(bbox.max_z),
        );
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    if self.get_block_id(x, y, z) == 81 {
                        self.attack_living(id, 1, None);
                        return;
                    }
                }
            }
        }
    }

    /// OnEntityWalking hook for walking across blocks (BlockFarmland trampling and BlockRedstoneOre activation).
    fn entity_walking_contact(&mut self, id: EntityId) {
        let (pos, on_ground, y_offset) = match self.entities.get(id) {
            Some(Entity::Player(p)) => (p.living.body.pos, p.living.body.on_ground, p.living.body.y_offset),
            Some(Entity::Mob(m)) => (m.living.body.pos, m.living.body.on_ground, m.living.body.y_offset),
            Some(Entity::Animal(a)) => (a.living.body.pos, a.living.body.on_ground, a.living.body.y_offset),
            _ => return,
        };
        if !on_ground {
            return;
        }
        let wx = floor_double(pos[0]);
        let wy = floor_double(pos[1] - 0.20000000298023224 - y_offset as f64);
        let wz = floor_double(pos[2]);
        let bid = self.get_block_id(wx, wy, wz);
        if bid == 60 {
            if self.rng.next_int_bound(4) == 0 {
                self.apply_set_notify(wx, wy, wz, 3);
            }
        } else if bid == 73 {
            self.apply_set_notify(wx, wy, wz, 74);
        }
    }

    /// Item push-out from solid rock (mirrors `pushOutOfBlocks`).
    pub fn item_push_out(&mut self, id: EntityId) {
        let (ix, iy, iz, lx, ly, lz) = match self.entities.get(id) {
            Some(Entity::Item(e)) => {
                let (x, y, z) = (e.body.pos[0], e.body.pos[1], e.body.pos[2]);
                (
                    x.floor() as i32,
                    y.floor() as i32,
                    z.floor() as i32,
                    x - (x.floor()),
                    y - (y.floor()),
                    z - (z.floor()),
                )
            }
            _ => return,
        };
        if !self.is_solid(ix, iy, iz) {
            return;
        }
        let side = crate::entity::misc::item_push_side(
            [
                !self.is_solid(ix - 1, iy, iz),
                !self.is_solid(ix + 1, iy, iz),
                !self.is_solid(ix, iy - 1, iz),
                !self.is_solid(ix, iy + 1, iz),
                !self.is_solid(ix, iy, iz - 1),
                !self.is_solid(ix, iy, iz + 1),
            ],
            [lx, ly, lz],
        );
        if side < 0 {
            return;
        }
        let impulse = self.rng.next_double() * 0.2 + 0.1;
        if let Some(Entity::Item(e)) = self.entities.get_mut(id) {
            match side {
                0 => e.body.motion[0] = -impulse,
                1 => e.body.motion[0] = impulse,
                2 => e.body.motion[1] = -impulse,
                3 => e.body.motion[1] = impulse,
                4 => e.body.motion[2] = -impulse,
                _ => e.body.motion[2] = impulse,
            }
        }
    }

    /// Loose-item tick (mirrors `EntityItem::tick`).
    pub fn tick_item(&mut self, id: EntityId) {
        self.entities.tick_base(id);
        let alive = match self.entities.get_mut(id) {
            Some(Entity::Item(e)) => {
                if e.pickup_delay > 0 {
                    e.pickup_delay -= 1;
                }
                e.age += 1;
                if e.age >= 6000 {
                    let (cx, cz) = (
                        (e.body.pos[0].floor() as i32) >> 4,
                        (e.body.pos[2].floor() as i32) >> 4,
                    );
                    e.body.dead = true;
                    self.mark_chunk_modified(cx, cz);
                    return;
                }
                e.body.motion[1] -= 0.04;
                (e.body.motion[0], e.body.motion[1], e.body.motion[2])
            }
            _ => return,
        };
        self.item_push_out(id);
        self.move_body(id, alive.0, alive.1, alive.2);
        if let Some(Entity::Item(e)) = self.entities.get_mut(id) {
            let mut m = crate::entity::misc::ItemMotion { mx: e.body.motion[0], my: e.body.motion[1], mz: e.body.motion[2] };
            crate::entity::misc::item_damp(e.body.on_ground, &mut m);
            e.body.motion = [m.mx, m.my, m.mz];
        }
    }

        /// Falling-sand tick (mirrors `EntityFallingSand::tick`).
    pub fn tick_falling(&mut self, id: EntityId) {        self.entities.tick_base(id);
        let (block_id, motion) = match self.entities.get_mut(id) {
            Some(Entity::Falling(e)) => {
                if e.block_id == 0 {
                    e.body.dead = true;
                    return;
                }
                e.fall_time += 1;
                e.body.motion[1] -= 0.04;
                (e.block_id, (e.body.motion[0], e.body.motion[1], e.body.motion[2]))
            }
            _ => return,
        };
        self.move_body(id, motion.0, motion.1, motion.2);
        if let Some(Entity::Falling(e)) = self.entities.get_mut(id) {
            e.body.motion[0] *= 0.98;
            e.body.motion[1] *= 0.98;
            e.body.motion[2] *= 0.98;
        }
        let (on_ground, by, px, py, pz) = match self.entities.get(id) {
            Some(Entity::Falling(e)) => (
                e.body.on_ground,
                e.body.pos[1].floor() as i32,
                e.body.pos[0],
                e.body.pos[1],
                e.body.pos[2],
            ),
            _ => return,
        };
        let bx = px.floor() as i32;
        let bz = pz.floor() as i32;
        let land_id = self.get_block_id(bx, by, bz) as i32;
        let fall_time = match self.entities.get(id) {
            Some(Entity::Falling(e)) => e.fall_time,
            _ => return,
        };
        let action = crate::entity::misc::falling_land(
            block_id, on_ground, by, land_id,
            is_replaceable(land_id as u8),
            (1..256).contains(&block_id),
            fall_time,
        );
        match action {
            1 => {
                if let Some(Entity::Falling(e)) = self.entities.get_mut(id) {
                    e.body.dead = true;
                }
                self.set_block_id(bx, by, bz, block_id as u8);
            }
            2 => {
                let drop_y = if on_ground { py + 0.5 } else { py };
                if let Some(Entity::Falling(e)) = self.entities.get_mut(id) {
                    e.body.dead = true;
                }
                self.spawn_item_entity(block_id, 1, 0, px, drop_y, pz);
            }
            _ => {}
        }
    }

    /// Boat damage (mirrors `EntityBoat::attackEntityFrom`): rock the boat,
    /// break past 40 damage with plank/stick drops. Returns true when the
    /// boat broke.
    pub fn damage_boat(&mut self, id: EntityId, amount: i32) -> bool {
        if amount <= 0 {
            return false;
        }
        let (broke, chunk_pos) = match self.entities.get_mut(id) {
            Some(Entity::Boat(b)) => {
                if b.body.dead {
                    return false;
                }
                b.forward_dir = -b.forward_dir;
                b.time_since_hit = 10;
                b.damage_taken += amount * 10;
                (
                    b.damage_taken > 40,
                    ((b.body.pos[0].floor() as i32) >> 4, (b.body.pos[2].floor() as i32) >> 4),
                )
            }
            _ => return false,
        };
        self.mark_chunk_modified(chunk_pos.0, chunk_pos.1);
        if !broke {
            return false;
        }
        // Eject the rider like C++ before dropping materials.
        let rider = self.entities.get(id).map(|e| e.body().ridden_by).unwrap_or(-1);
        if rider >= 0 {
            self.entities.mount(rider, None);
        }
        let (px, py, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return true,
        };
        for _ in 0..3 {
            self.spawn_item_entity(5, 1, 0, px, py, pz);
        }
        for _ in 0..2 {
            self.spawn_item_entity(280, 1, 0, px, py, pz);
        }
        if let Some(Entity::Boat(b)) = self.entities.get_mut(id) {
            b.body.dead = true;
        }
        self.mark_chunk_modified((px.floor() as i32) >> 4, (pz.floor() as i32) >> 4);
        true
    }

    /// Boat tick (mirrors `EntityBoat::tick`).
    pub fn tick_boat(&mut self, id: EntityId) {
        self.entities.tick_base(id);
        let alive = match self.entities.get_mut(id) {
            Some(Entity::Boat(b)) => {
                if b.body.dead {
                    return;
                }
                if b.time_since_hit > 0 {
                    b.time_since_hit -= 1;
                }
                if b.damage_taken > 0 {
                    b.damage_taken -= 1;
                }
                // Eject non-player riders (custom-logic edge case).
                let rider = b.body.ridden_by;
                (rider, b.body.motion[0], b.body.motion[1], b.body.motion[2])
            }
            _ => return,
        };
        if alive.0 >= 0 {
            let is_player = matches!(self.entities.get(alive.0), Some(Entity::Player(_)));
            if !is_player {
                self.entities.mount(alive.0, None);
            }
        }
        // Buoyancy from the water fraction under the hull.
        let (min_x, min_y, min_z, max_x, max_y, max_z) = match self.entities.get(id) {
            Some(e) => {
                let b = e.body();
                (b.bounding_box.min_x, b.bounding_box.min_y, b.bounding_box.min_z,
                 b.bounding_box.max_x, b.bounding_box.max_y, b.bounding_box.max_z)
            }
            None => return,
        };
        let fraction = crate::entity::misc::water_fraction_scan(
            min_x,
            min_y,
            min_z,
            max_x,
            max_y,
            max_z,
            |x, y, z| self.is_water(x, y, z),
        );
        // Rider drive.
        let rider_motion = match self.entities.get(id) {
            Some(e) => {
                let r = e.body().ridden_by;
                if r >= 0 {
                    self.entities.get(r).map(|re| (re.body().motion[0], re.body().motion[2]))
                } else {
                    None
                }
            }
            None => return,
        };
        if let Some(Entity::Boat(b)) = self.entities.get_mut(id) {
            b.body.motion[1] += 0.04 * (fraction * 2.0 - 1.0);
            if let Some((rx, rz)) = rider_motion {
                b.body.motion[0] += rx * 0.2;
                b.body.motion[2] += rz * 0.2;
            }
            b.body.motion[0] = b.body.motion[0].clamp(-0.4, 0.4);
            b.body.motion[2] = b.body.motion[2].clamp(-0.4, 0.4);
            if b.body.on_ground {
                b.body.motion[0] *= 0.5;
                b.body.motion[1] *= 0.5;
                b.body.motion[2] *= 0.5;
            }
        }
        let motion = match self.entities.get(id) {
            Some(e) => (e.body().motion[0], e.body().motion[1], e.body().motion[2]),
            None => return,
        };
        self.move_body(id, motion.0, motion.1, motion.2);
        // Crash: eject, drop, die.
        let crash = match self.entities.get(id) {
            Some(Entity::Boat(b)) => {
                let speed =
                    (b.body.motion[0] * b.body.motion[0] + b.body.motion[2] * b.body.motion[2]).sqrt();
                b.body.collided_horiz && speed > 0.15
            }
            _ => return,
        };
        if crash {
            let rider = self.entities.get(id).map(|e| e.body().ridden_by).unwrap_or(-1);
            if rider >= 0 {
                self.entities.mount(rider, None);
            }
            let (px, py, pz) = match self.entities.get(id) {
                Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
                None => return,
            };
            for _ in 0..3 {
                self.spawn_item_entity(5, 1, 0, px, py, pz);
            }
            for _ in 0..2 {
                self.spawn_item_entity(280, 1, 0, px, py, pz);
            }
            if let Some(Entity::Boat(b)) = self.entities.get_mut(id) {
                b.body.dead = true;
            }
            return;
        }
        if let Some(Entity::Boat(b)) = self.entities.get_mut(id) {
            b.body.motion[0] *= 0.99;
            b.body.motion[1] *= 0.95;
            b.body.motion[2] *= 0.99;
        }
        // Yaw follows travel direction.
        let (dx, dz, yaw) = match self.entities.get(id) {
            Some(e) => {
                let b = e.body();
                (b.pos[0] - b.prev_pos[0], b.pos[2] - b.prev_pos[2], b.yaw)
            }
            None => return,
        };
        // NOTE: C++ reads prevPos AFTER move (already synced by tick_base
        // at the START of next tick); here prev holds the pre-move value
        // from this tick's tick_base, which matches because C++ compares
        // post-move pos against the same pre-move snapshot.
        let mut new_yaw = yaw;
        crate::entity::misc::boat_steer(dx, dz, yaw, &mut new_yaw);
        if let Some(Entity::Boat(b)) = self.entities.get_mut(id) {
            b.body.yaw = new_yaw;
            b.body.pitch = 0.0;
        }
        // Boat-on-boat shoves.
        let boats: Vec<EntityId> = self
            .entities
            .alive_ids()
            .into_iter()
            .filter(|oid| *oid != id && matches!(self.entities.get(*oid), Some(Entity::Boat(_))))
            .collect();
        for oid in boats {
            let (ax, az, bx, bz) = match (self.entities.get(id), self.entities.get(oid)) {
                (Some(a), Some(b)) => (a.body().pos[0], a.body().pos[2], b.body().pos[0], b.body().pos[2]),
                _ => continue,
            };
            let Some(push) =
                crate::entity::physics::entity_push(ax, az, bx, bz, true, true)
            else {
                continue;
            };
            if let Some(Entity::Boat(b)) = self.entities.get_mut(id) {
                b.body.motion[0] += push.dvx1;
                b.body.motion[2] += push.dvz1;
            }
            if let Some(Entity::Boat(o)) = self.entities.get_mut(oid) {
                o.body.motion[0] += push.dvx2;
                o.body.motion[2] += push.dvz2;
            }
        }
        self.entities.update_rider_position(id);
    }
}
