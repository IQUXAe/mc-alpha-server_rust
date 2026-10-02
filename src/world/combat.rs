//! Line of sight, mob attacks, explosions, arrows and TNT on [`World`].
//! Split out of `world.rs`; behavior unchanged.

use crate::aabb::AxisAlignedBB;
use crate::block::table::{BlockType, block_properties_get};
use crate::entity::table::{Body, Entity, EntityId, MobKind};
use crate::material::Material;
use crate::math_helper::{floor_double, sqrt_float};
use crate::world::{World, has_collision_box, has_collision_id};
use crate::world::ai::CreatureSnap;

/// Creeper blast radius (mirrors the Alpha inline `explode`).
pub(crate) const CREEPER_BLAST_RADIUS: f32 = 3.0;
/// Hard cap on destroyed cells per blast (hardening; vanilla is
/// uncapped). A radius-4 blast yields ~10^2 cells; chains of dozens
/// stay far below. Without a cap one player chaining a field of TNT
/// stalls the 50ms tick for everyone (block loop + per-cell packets).
pub const MAX_BLAST_CELLS: usize = 8192;
/// Fire block id placed by explosions.
/// Look direction from yaw/pitch degrees (mirrors the player-aim math
/// in `ItemBoat`/`EntityArrow`: x=-sin(yaw)cos(pitch), y=-sin(pitch),
/// z=cos(yaw)cos(pitch)).
fn look_dir(yaw: f32, pitch: f32) -> [f64; 3] {
    let yr = yaw as f64 * std::f64::consts::PI / 180.0;
    let pr = pitch as f64 * std::f64::consts::PI / 180.0;
    [
        -yr.sin() * pr.cos(),
        -pr.sin(),
        yr.cos() * pr.cos(),
    ]
}

impl World {
    /// Line of sight (mirrors `canEntitySee` → `rayTraceBlocks` with
    /// `includeLiquids = false`): the same DDA walk, blocked by any
    /// collidable block. Only fluids let sight through (`canCollideCheck`
    /// is false solely for `BlockFluid`; flowers, torches and crops block
    /// like stone). Missing chunks read air like every other native query
    /// (C++ loads/generates there).
    pub fn ray_trace_clear(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        if ![from[0], from[1], from[2], to[0], to[1], to[2]].iter().all(|v| v.is_finite()) {
            return true;
        }
        let (mut cx, mut cy, mut cz) = (from[0], from[1], from[2]);
        let (mut ccx, mut ccy, mut ccz) = (cx.floor() as i32, cy.floor() as i32, cz.floor() as i32);
        let (tx, ty, tz) = (to[0].floor() as i32, to[1].floor() as i32, to[2].floor() as i32);
        for _ in 0..=200 {
            if !cx.is_finite() || !cy.is_finite() || !cz.is_finite() {
                return true;
            }
            if ccx == tx && ccy == ty && ccz == tz {
                return true;
            }
            let (mut nbx, mut nby, mut nbz) = (999.0f64, 999.0f64, 999.0f64);
            if tx > ccx {
                nbx = ccx as f64 + 1.0;
            }
            if tx < ccx {
                nbx = ccx as f64;
            }
            if ty > ccy {
                nby = ccy as f64 + 1.0;
            }
            if ty < ccy {
                nby = ccy as f64;
            }
            if tz > ccz {
                nbz = ccz as f64 + 1.0;
            }
            if tz < ccz {
                nbz = ccz as f64;
            }
            let (dx, dy, dz) = (to[0] - cx, to[1] - cy, to[2] - cz);
            let (mut sx, mut sy, mut sz) = (999.0f64, 999.0f64, 999.0f64);
            if nbx != 999.0 && dx.abs() > 1.0e-7 {
                sx = (nbx - cx) / dx;
            }
            if nby != 999.0 && dy.abs() > 1.0e-7 {
                sy = (nby - cy) / dy;
            }
            if nbz != 999.0 && dz.abs() > 1.0e-7 {
                sz = (nbz - cz) / dz;
            }
            let side: i8;
            if sx < sy && sx < sz {
                side = if tx > ccx { 4 } else { 5 };
                cx = nbx;
                cy += dy * sx;
                cz += dz * sx;
            } else if sy < sz {
                side = if ty > ccy { 0 } else { 1 };
                cx += dx * sy;
                cy = nby;
                cz += dz * sy;
            } else {
                side = if tz > ccz { 2 } else { 3 };
                cx += dx * sz;
                cy += dy * sz;
                cz = nbz;
            }
            ccx = cx.floor() as i32;
            if side == 5 {
                ccx -= 1;
            }
            ccy = cy.floor() as i32;
            if side == 1 {
                ccy -= 1;
            }
            ccz = cz.floor() as i32;
            if side == 3 {
                ccz -= 1;
            }
            let bid = self.get_block_id(ccx, ccy, ccz);
            if bid == 0 {
                continue;
            }
            if block_properties_get(bid as u32).block_type == BlockType::Fluid as u8 {
                continue;
            }
            return false;
        }
        true
    }

    /// First-hit ray trace with liquids included (mirrors `rayTraceBlocks`
    /// with `includeLiquids = true` for boat aiming): same DDA walk,
    /// returning the first collidable cell. Still fluids block the ray
    /// (flowing water 1..7 lets it through like `canCollideCheck`).
    pub fn ray_trace_hit_liquids(&self, from: [f64; 3], to: [f64; 3]) -> Option<[i32; 3]> {
        self.ray_trace_face(from, to, true).map(|(pos, _)| pos)
    }

    /// First-hit ray trace with liquids included and face hit (mirrors Java `rayTraceBlocks`
    /// returning `MovingObjectPosition` with `sideHit`). Still fluids block the ray
    /// (flowing water 1..7 lets it through like `canCollideCheck`).
    pub fn ray_trace_hit_liquids_face(&self, from: [f64; 3], to: [f64; 3]) -> Option<([i32; 3], i8)> {
        self.ray_trace_face(from, to, true)
    }

    /// General first-hit ray trace with face hit and optional liquid collision
    /// (mirrors Java `World.rayTraceBlocks(Vec3D, Vec3D, boolean)` returning `MovingObjectPosition`).
    pub fn ray_trace_face(&self, from: [f64; 3], to: [f64; 3], include_liquids: bool) -> Option<([i32; 3], i8)> {
        if ![from[0], from[1], from[2], to[0], to[1], to[2]].iter().all(|v| v.is_finite()) {
            return None;
        }
        let (mut cx, mut cy, mut cz) = (from[0], from[1], from[2]);
        let (mut ccx, mut ccy, mut ccz) = (cx.floor() as i32, cy.floor() as i32, cz.floor() as i32);
        let (tx, ty, tz) = (to[0].floor() as i32, to[1].floor() as i32, to[2].floor() as i32);
        for _ in 0..=200 {
            if !cx.is_finite() || !cy.is_finite() || !cz.is_finite() {
                return None;
            }
            if ccx == tx && ccy == ty && ccz == tz {
                return None;
            }
            let (mut nbx, mut nby, mut nbz) = (999.0f64, 999.0f64, 999.0f64);
            if tx > ccx {
                nbx = ccx as f64 + 1.0;
            }
            if tx < ccx {
                nbx = ccx as f64;
            }
            if ty > ccy {
                nby = ccy as f64 + 1.0;
            }
            if ty < ccy {
                nby = ccy as f64;
            }
            if tz > ccz {
                nbz = ccz as f64 + 1.0;
            }
            if tz < ccz {
                nbz = ccz as f64;
            }
            let (dx, dy, dz) = (to[0] - cx, to[1] - cy, to[2] - cz);
            let (mut sx, mut sy, mut sz) = (999.0f64, 999.0f64, 999.0f64);
            if nbx != 999.0 && dx.abs() > 1.0e-7 {
                sx = (nbx - cx) / dx;
            }
            if nby != 999.0 && dy.abs() > 1.0e-7 {
                sy = (nby - cy) / dy;
            }
            if nbz != 999.0 && dz.abs() > 1.0e-7 {
                sz = (nbz - cz) / dz;
            }
            let side: i8;
            if sx < sy && sx < sz {
                side = if tx > ccx { 4 } else { 5 };
                cx = nbx;
                cy += dy * sx;
                cz += dz * sx;
            } else if sy < sz {
                side = if ty > ccy { 0 } else { 1 };
                cx += dx * sy;
                cy = nby;
                cz += dz * sy;
            } else {
                side = if tz > ccz { 2 } else { 3 };
                cx += dx * sz;
                cy += dy * sz;
                cz = nbz;
            }
            ccx = cx.floor() as i32;
            if side == 5 {
                ccx -= 1;
            }
            ccy = cy.floor() as i32;
            if side == 1 {
                ccy -= 1;
            }
            ccz = cz.floor() as i32;
            if side == 3 {
                ccz -= 1;
            }
            let bid = self.get_block_id(ccx, ccy, ccz);
            if bid == 0 {
                continue;
            }
            if block_properties_get(bid as u32).block_type == BlockType::Fluid as u8 {
                if !include_liquids {
                    continue;
                }
                // canCollideCheck(meta, true): falling (8+) reads as still,
                // still (0) blocks, flowing (1..7) lets the ray through.
                let mut meta = self.get_block_meta(ccx, ccy, ccz);
                if meta >= 8 {
                    meta = 0;
                }
                if meta != 0 {
                    continue;
                }
            }
            return Some(([ccx, ccy, ccz], side));
        }
        None
    }

    /// Melee line of sight (mirrors the attack `hasLineOfSight`): the
    /// attacker's eye against three target samples (feet + 0.1, middle,
    /// eye), blocked by swept collision boxes strictly inside the segment
    /// (`+ 1.0E-6 <` like C++).
    pub fn attack_los(&self, attacker_id: EntityId, target_id: EntityId) -> bool {
        use crate::vec3d::Vec3D;
        let (eye, samples) = match (self.entities.get(attacker_id), self.entities.get(target_id)) {
            (Some(a), Some(t)) => {
                let ab = a.body();
                let eye =
                    Vec3D::new(ab.pos[0], ab.pos[1] + Self::living_eye_height(a), ab.pos[2]);
                let tb = t.body();
                let teye = Self::living_eye_height(t);
                (
                    eye,
                    [
                        Vec3D::new(tb.pos[0], tb.bounding_box.min_y + 0.1, tb.pos[2]),
                        Vec3D::new(tb.pos[0], tb.pos[1] + tb.height as f64 * 0.5, tb.pos[2]),
                        Vec3D::new(tb.pos[0], tb.pos[1] + teye, tb.pos[2]),
                    ],
                )
            }
            _ => return false,
        };
        for to in &samples {
            if !self.segment_blocked(&eye, to) {
                return true;
            }
        }
        false
    }

    /// Swept-box segment test for [`World::attack_los`] (mirrors
    /// `rayHitsSolidBlock`): any collidable cell box clipped by the
    /// segment strictly inside counts.
    fn segment_blocked(&self, from: &crate::vec3d::Vec3D, to: &crate::vec3d::Vec3D) -> bool {
        let min_x = floor_double(from.x_coord.min(to.x_coord));
        let min_y = floor_double(from.y_coord.min(to.y_coord));
        let min_z = floor_double(from.z_coord.min(to.z_coord));
        let max_x = floor_double(from.x_coord.max(to.x_coord));
        let max_y = floor_double(from.y_coord.max(to.y_coord));
        let max_z = floor_double(from.z_coord.max(to.z_coord));
        let target_sq = (from.x_coord - to.x_coord).powi(2)
            + (from.y_coord - to.y_coord).powi(2)
            + (from.z_coord - to.z_coord).powi(2);
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                for z in min_z..=max_z {
                    let bid = self.get_block_id(x, y, z);
                    if bid == 0 {
                        continue;
                    }
                    let props = block_properties_get(bid as u32);
                    if !has_collision_box(props.block_type) || !has_collision_id(bid) {
                        continue;
                    }
                    let mut boxes = Vec::with_capacity(2);
                    self.block_collision_boxes(x, y, z, bid, &mut boxes);
                    for bb in boxes {
                        if let Some(hit) = bb.clip(from, to) {
                            let d = (from.x_coord - hit.hit_vec.x_coord).powi(2)
                                + (from.y_coord - hit.hit_vec.y_coord).powi(2)
                                + (from.z_coord - hit.hit_vec.z_coord).powi(2);
                            if d + 1.0e-6 < target_sq {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        false
    }

    /// Phase-1 attack dispatch (mirrors `attackEntityAt` → per-kind
    /// `attackTarget`). `dist` is the C++ phase-1 distance
    /// (`sqrt_float(float(full 3D pos distSq))`). Returns the effective
    /// target (spiders drop it in daylight).
    pub(crate) fn mob_attack(
        &mut self,
        id: EntityId,
        kind: MobKind,
        target: EntityId,
        dist: f32,
        snap: &mut CreatureSnap,
    ) -> Option<EntityId> {
        match kind {
            MobKind::Zombie | MobKind::Giant | MobKind::PigZombie => {
                self.zombie_punch(id, kind, target, dist)
            }
            MobKind::Skeleton => self.skeleton_volley(id, target, dist, snap),
            MobKind::Spider => self.spider_attack(id, target, dist, snap),
            MobKind::Creeper => self.creeper_swell(id, target, dist, snap),
            MobKind::Slime => self.slime_touch(id, target, dist),
            MobKind::Ghast => self.ghast_volley(id, target, dist),
        }
    }

    /// Base melee (mirrors `EntityMob::attackTarget`): in-reach, vertical
    /// overlap, cooldown-gated. Strength by kind (`mob_melee_damage`:
    /// zombie-line 5, giant 50).
    fn zombie_punch(
        &mut self,
        id: EntityId,
        kind: MobKind,
        target: EntityId,
        dist: f32,
    ) -> Option<EntityId> {
        let size = match self.entities.get(id) {
            Some(Entity::Mob(m)) => m.slime_size,
            _ => 1,
        };
        let damage = crate::entity::table::mob_melee_damage(kind, size);
        let (overlap, ready) = match (self.entities.get(id), self.entities.get(target)) {
            (Some(s), Some(t)) => (
                t.body().bounding_box.max_y > s.body().bounding_box.min_y
                    && t.body().bounding_box.min_y < s.body().bounding_box.max_y,
                matches!(s, Entity::Mob(m) if m.attack_cooldown == 0),
            ),
            _ => return Some(target),
        };
        if dist < 2.5 && overlap && ready {
            if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
                m.attack_cooldown = 20;
            }
            self.attack_living(target, damage, Some(id));
        }
        Some(target)
    }

    /// Slime touch damage (mirrors `EntitySlime` contact): reach-2.5
    /// strength-size poke with the same cooldown gate as the base melee.
    fn slime_touch(&mut self, id: EntityId, target: EntityId, dist: f32) -> Option<EntityId> {
        self.zombie_punch(id, MobKind::Slime, target, dist)
    }

    /// Ghast volley: loose a fireball at the victim's eye on a 100+rand40
    /// cooldown when inside 64 blocks with line of sight (vanilla
    /// `field_4103_aj` gate + `worldObj.playSoundAtEntity("mob.ghast.fireattack")`
    /// equivalent is client-side on the spawn packet).
    fn ghast_volley(&mut self, id: EntityId, target: EntityId, dist: f32) -> Option<EntityId> {
        if dist > 64.0 {
            return Some(target);
        }
        let ready = matches!(
            self.entities.get(id),
            Some(Entity::Mob(m)) if m.attack_cooldown == 0
        );
        if !ready {
            return Some(target);
        }
        if !self.attack_los(id, target) {
            return Some(target);
        }
        if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
            m.attack_cooldown = 100 + (self.rng.next_int_bound(40));
        }
        self.spawn_fireball(id, target);
        Some(target)
    }

    /// Shared thrown-projectile sweep (blocks + living): nearest
    /// collidable block cell, plus nearest living/player row intersecting
    /// the motion box (owner skipped while young). Bounded by the
    /// motion-box scan like the arrow sweep (no unbounded loops).
    fn projectile_sweep(
        &self,
        id: EntityId,
        owner: EntityId,
        grace_ticks: i32,
        ticks_in_air: i32,
    ) -> (Option<EntityId>, Option<(i32, i32, i32)>) {
        let (pos, motion, bbox) = match self.entities.get(id) {
            Some(e) => (e.body().pos, e.body().motion, e.body().bounding_box),
            None => return (None, None),
        };
        let sweep = bbox.add_coord(motion[0], motion[1], motion[2]).expand(1.0, 1.0, 1.0);
        let (min_x, min_y, min_z) = (
            floor_double(sweep.min_x), floor_double(sweep.min_y), floor_double(sweep.min_z),
        );
        let (max_x, max_y, max_z) = (
            floor_double(sweep.max_x), floor_double(sweep.max_y), floor_double(sweep.max_z),
        );
        let mut block_hit: Option<(i32, i32, i32)> = None;
        let mut best = f64::MAX;
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                for z in min_z..=max_z {
                    let bid = self.get_block_id(x, y, z);
                    if bid == 0 {
                        continue;
                    }
                    let props = block_properties_get(bid as u32);
                    if !has_collision_box(props.block_type) || !has_collision_id(bid) {
                        continue;
                    }
                    let d = (x as f64 + 0.5 - pos[0]).powi(2)
                        + (y as f64 + 0.5 - pos[1]).powi(2)
                        + (z as f64 + 0.5 - pos[2]).powi(2);
                    if d < best {
                        best = d;
                        block_hit = Some((x, y, z));
                    }
                }
            }
        }
        let mut entity_hit: Option<EntityId> = None;
        let mut cands: Vec<EntityId> = Vec::new();
        for oid in self.entities.alive_ids() {
            if oid == id || oid == owner && ticks_in_air < grace_ticks {
                continue;
            }
            // Projectiles don't hit items, other shots, or vehicles.
            if let Some(o) = self.entities.get(oid) {
                if !matches!(
                    o,
                    Entity::Mob(_) | Entity::Animal(_) | Entity::Player(_)
                ) {
                    continue;
                }
                if sweep.intersects_with(&o.body().bounding_box) {
                    cands.push(oid);
                }
            }
        }
        cands.sort_unstable();
        for oid in cands {
            let (ox, oy, oz) = match self.entities.get(oid) {
                Some(o) => (o.body().pos[0], o.body().pos[1], o.body().pos[2]),
                None => continue,
            };
            let d = (ox - pos[0]).powi(2) + (oy - pos[1]).powi(2) + (oz - pos[2]).powi(2);
            if d < best {
                best = d;
                entity_hit = Some(oid);
            }
        }
        (entity_hit, block_hit)
    }

    /// Snowball tick (mirrors `EntitySnowball.onUpdate`): gravity 0.03,
    /// block impact dies, living impact deals 0 + knockback and dies.
    /// 1200-tick lifetime like arrows.
    pub fn tick_snowball(&mut self, id: EntityId) {
        self.entities.tick_base(id);
        let (owner, ticks) = match self.entities.get_mut(id) {
            Some(Entity::Snowball(s)) => {
                s.ticks_in_air += 1;
                if s.ticks_in_air > 1200 {
                    s.body.dead = true;
                    return;
                }
                (s.owner_id, s.ticks_in_air)
            }
            _ => return,
        };
        let (entity_hit, block_hit) = self.projectile_sweep(id, owner, 5, ticks);
        if let Some(v) = entity_hit {
            // Damage 0 by design (vanilla `attackEntity(attacker, 0)`):
            // our pipeline no-ops on non-positive amounts, so the
            // knockback is applied explicitly in the vanilla shape
            // (halve motion, shove 0.4 away + 0.4 up, capped).
            self.attack_living(v, 0, Some(owner));
            let (sx, sz) = match self.entities.get(id) {
                Some(e) => (e.body().pos[0], e.body().pos[2]),
                None => return,
            };
            if let Some(t) = self.entities.get_mut(v) {
                let b = t.body_mut();
                let (dx, dz) = (sx - b.pos[0], sz - b.pos[2]);
                let d = (dx * dx + dz * dz).sqrt();
                if d > 1e-6 {
                    b.motion[0] = b.motion[0] * 0.5 - dx / d * 0.4;
                    b.motion[1] = (b.motion[1] * 0.5 + 0.4).min(0.4);
                    b.motion[2] = b.motion[2] * 0.5 - dz / d * 0.4;
                }
            }
            if let Some(Entity::Snowball(s)) = self.entities.get_mut(id) {
                s.body.dead = true;
            }
            return;
        }
        if block_hit.is_some() {
            if let Some(Entity::Snowball(s)) = self.entities.get_mut(id) {
                s.body.dead = true;
            }
            return;
        }
        let (mx, my, mz) = match self.entities.get(id) {
            Some(e) => (e.body().motion[0], e.body().motion[1], e.body().motion[2]),
            None => return,
        };
        self.move_body(id, mx, my, mz);
        // Drag + gravity (vanilla tail: 0.99 air, 0.8 in water, -0.03).
        let in_water = match self.entities.get(id) {
            Some(e) => {
                let bb = e.body().bounding_box;
                crate::entity::misc::water_fraction_scan(
                    bb.min_x, bb.min_y, bb.min_z, bb.max_x, bb.max_y, bb.max_z,
                    |x, y, z| self.is_water(x, y, z),
                ) > 0.0
            }
            None => return,
        };
        if let Some(Entity::Snowball(s)) = self.entities.get_mut(id) {
            let drag = if in_water { 0.8 } else { 0.99 };
            s.body.motion[0] *= drag;
            s.body.motion[1] *= drag;
            s.body.motion[2] *= drag;
            s.body.motion[1] -= 0.03;
        }
    }

    /// Fireball tick (mirrors `EntityFireball.onUpdate` minus the vanilla
    /// fire trail): straight flight, radius-1 blast on any contact.
    /// 1200-tick lifetime cap (hardening; vanilla flies until it hits).
    pub fn tick_fireball(&mut self, id: EntityId) {
        self.entities.tick_base(id);
        let (owner, ticks) = match self.entities.get_mut(id) {
            Some(Entity::Fireball(f)) => {
                f.ticks_in_air += 1;
                if f.ticks_in_air > 1200 {
                    f.body.dead = true;
                    return;
                }
                (f.owner_id, f.ticks_in_air)
            }
            _ => return,
        };
        let (entity_hit, block_hit) = self.projectile_sweep(id, owner, 5, ticks);
        if entity_hit.is_some() || block_hit.is_some() {
            let (px, py, pz) = match self.entities.get(id) {
                Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
                None => return,
            };
            if let Some(e) = self.entities.get_mut(id) {
                e.body_mut().dead = true;
            }
            // Incendiary like the vanilla impact (lights the crater).
            self.blast_flaming(px, py, pz, 1.0, Some(owner), true);
            return;
        }
        let (mx, my, mz) = match self.entities.get(id) {
            Some(e) => (e.body().motion[0], e.body().motion[1], e.body().motion[2]),
            None => return,
        };
        self.move_body(id, mx, my, mz);
    }

    /// Throw a snowball from a shooter's eye along its look (player
    /// item use; speed 1.5 like the bow draw).
    pub(crate) fn spawn_snowball(&mut self, owner: EntityId) {
        let (pos, yaw, pitch, eye) = match self.entities.get(owner) {
            Some(e) => (e.body().pos, e.body().yaw, e.body().pitch, Self::living_eye_height(e)),
            None => return,
        };
        let dir = look_dir(yaw, pitch);
        let nid = self.entities.alloc_id();
        let mut b = Body::new(nid, 0.25, 0.25, 0.0);
        b.set_position(pos[0], pos[1] + eye - 0.1, pos[2]);
        b.motion = [dir[0] * 1.5, dir[1] * 1.5, dir[2] * 1.5];
        self.entities.insert(Entity::Snowball(crate::entity::table::SnowballEnt {
            body: b,
            owner_id: owner,
            ticks_in_air: 0,
        }));
        self.stamp_dim(nid);
        self.mark_chunk_modified((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
    }

    /// Player bow shot (mirrors `ItemBow.onItemRightClick` +
    /// `EntityArrow` player ctor): eye start along the look at speed
    /// 2.0. The session layer consumes one arrow (262) and gates the
    /// 20-tick draw cooldown. Sound is client-side on the swing packet.
    pub(crate) fn spawn_player_arrow(&mut self, owner: EntityId) {
        let (pos, yaw, pitch, eye) = match self.entities.get(owner) {
            Some(e) => (e.body().pos, e.body().yaw, e.body().pitch, Self::living_eye_height(e)),
            None => return,
        };
        let dir = look_dir(yaw, pitch);
        let nid = self.entities.alloc_id();
        let mut b = Body::new(nid, 0.5, 0.5, 0.0);
        b.set_position(pos[0], pos[1] + eye - 0.1, pos[2]);
        b.yaw = yaw;
        b.pitch = pitch;
        b.prev_yaw = yaw;
        b.prev_pitch = pitch;
        b.motion = [dir[0] * 2.0, dir[1] * 2.0, dir[2] * 2.0];
        self.entities.insert(Entity::Arrow(crate::entity::table::ArrowEnt {
            body: b,
            in_ground: false,
            shake: 0,
            ticks_in_ground: 0,
            ticks_in_air: 0,
            shooter_id: owner,
            tile: [-1, -1, -1],
            in_tile: 0,
        }));
        self.mark_chunk_modified((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
    }

    /// Place a rideable minecart on rails (mirrors
    /// `ItemMinecart.onItemUse`: only on top of a rail block 66, only
    /// server-side rows here). Returns false when the target is not a
    /// rail (caller sends the rollback).
    pub(crate) fn place_minecart(&mut self, x: i32, y: i32, z: i32) -> bool {
        if self.get_block_id(x, y, z) != 66 {
            return false;
        }
        let nid = self.entities.alloc_id();
        let mut b = Body::new(nid, 0.98, 0.7, 0.0);
        b.set_position(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5);
        b.step_height = 1.0;
        self.entities.insert(Entity::Minecart(crate::entity::table::MinecartEnt {
            body: b,
            cart_type: 0,
            damage_taken: 0,
            time_since_hit: 0,
        }));
        self.stamp_dim(nid);
        self.mark_chunk_modified(x >> 4, z >> 4);
        true
    }

    /// Cast a fishing hook (mirrors `ItemFishingRod.onItemRightClick`
    /// cast half): eye start, look direction at moderate speed. At most
    /// one live hook per owner (re-cast reels the old one — session
    /// layer enforces this before calling).
    pub(crate) fn cast_fishing(&mut self, owner: EntityId) {
        let (pos, yaw, pitch, eye) = match self.entities.get(owner) {
            Some(e) => (e.body().pos, e.body().yaw, e.body().pitch, Self::living_eye_height(e)),
            None => return,
        };
        let dir = look_dir(yaw, pitch);
        let nid = self.entities.alloc_id();
        let mut b = Body::new(nid, 0.25, 0.25, 0.0);
        b.set_position(pos[0], pos[1] + eye - 0.1, pos[2]);
        // Launch speed 1.5 along the look (vanilla `func_6142_a`
        // normalizes then scales; the gaussian spread is omitted —
        // casts stay deterministic per aim).
        b.motion = [dir[0] * 1.5, dir[1] * 1.5, dir[2] * 1.5];
        self.entities.insert(Entity::FishHook(crate::entity::table::FishHookEnt {
            body: b,
            owner_id: owner,
            ticks_in_air: 0,
            nibble_ticks: 0,
            hooked_id: crate::entity::table::NO_ENTITY,
            stuck_tile: [-1, -1, -1],
            stuck_in: 0,
            stuck: false,
            shake: 0,
        }));
        self.stamp_dim(nid);
        self.mark_chunk_modified((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4);
    }

    /// Fishing hook tick (mirrors `EntityFish.onUpdate`): owner/rod/range
    /// guards, hooked-entity follow, stuck-in-block life, block/entity
    /// sweep (entity hits deal 0 and hook on), water buoyancy + drag,
    /// and the 1/500 nibble roll (`nibble_ticks = 10+rand(30)` with a
    /// motion dip the client renders as a bobber dunk). The fish itself
    /// is awarded on REEL inside the window, not on a timer.
    pub fn tick_fishhook(&mut self, id: EntityId) {
        self.entities.tick_base(id);
        let owner = match self.entities.get(id) {
            Some(Entity::FishHook(f)) => f.owner_id,
            _ => return,
        };
        // Owner guards (vanilla lines 89-95): dead, not holding a rod,
        // or farther than 32 blocks → line snaps.
        let owner_ok = match self.entities.get(owner) {
            Some(Entity::Player(p)) => {
                !p.living.body.dead
                    && matches!(
                        p.inventory.main.get(p.inventory.current as usize).copied().flatten(),
                        Some(s) if s.item_id == 346
                    )
                    && {
                        let (ox, oy, oz) = (p.living.body.pos[0], p.living.body.pos[1], p.living.body.pos[2]);
                        match self.entities.get(id) {
                            Some(e) => {
                                let (hx, hy, hz) = (e.body().pos[0], e.body().pos[1], e.body().pos[2]);
                                (ox - hx).powi(2) + (oy - hy).powi(2) + (oz - hz).powi(2) <= 1024.0
                            }
                            None => false,
                        }
                    }
            }
            _ => false,
        };
        if !owner_ok {
            if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                f.body.dead = true;
            }
            return;
        }
        // Hooked entity: ride it (vanilla lines 96-105).
        let hooked = match self.entities.get(id) {
            Some(Entity::FishHook(f)) => f.hooked_id,
            _ => return,
        };
        if hooked >= 0 {
            let alive = matches!(self.entities.get(hooked), Some(e) if !e.body().dead);
            if !alive {
                if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                    f.hooked_id = crate::entity::table::NO_ENTITY;
                }
            } else if let (Some(hk), Some(victim)) =
                (self.entities.get(id), self.entities.get(hooked))
            {
                let (hx, hy, hz) = (hk.body().pos[0], hk.body().pos[1], hk.body().pos[2]);
                let _ = (hx, hy, hz);
                let (vx, vy, vz, vh) = (
                    victim.body().pos[0],
                    victim.body().pos[1],
                    victim.body().pos[2],
                    victim.body().height as f64,
                );
                if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                    f.body.set_position(vx, vy + vh * 0.8, vz);
                    f.body.motion = [0.0; 3];
                }
                return;
            }
        }
        // Stuck-in-block life (vanilla lines 112-131): same block →
        // 1200-tick life; changed block → pop out damped.
        let stuck = matches!(self.entities.get(id), Some(Entity::FishHook(f)) if f.stuck);
        if stuck {
            let same = match self.entities.get(id) {
                Some(Entity::FishHook(f)) => {
                    self.get_block_id(f.stuck_tile[0], f.stuck_tile[1], f.stuck_tile[2]) as i32
                        == f.stuck_in
                }
                _ => false,
            };
            if same {
                let done = match self.entities.get_mut(id) {
                    Some(Entity::FishHook(f)) => {
                        f.ticks_in_air += 1;
                        f.ticks_in_air >= 1200
                    }
                    _ => true,
                };
                if done {
                    if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                        f.body.dead = true;
                    }
                }
                return;
            }
            if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                f.stuck = false;
                f.body.motion[0] *= self.rng.next_float() as f64 * 0.2;
                f.body.motion[1] *= self.rng.next_float() as f64 * 0.2;
                f.body.motion[2] *= self.rng.next_float() as f64 * 0.2;
                f.ticks_in_air = 0;
            }
        } else if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
            f.ticks_in_air += 1;
        }
        // Nibble countdown.
        if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
            if f.nibble_ticks > 0 {
                f.nibble_ticks -= 1;
            }
        }
        // Sweep: block raycast + entity box (owner grace 5 ticks).
        let ticks = match self.entities.get(id) {
            Some(Entity::FishHook(f)) => f.ticks_in_air,
            _ => return,
        };
        let (entity_hit, block_hit) = self.projectile_sweep(id, owner, 5, ticks);
        if let Some(v) = entity_hit {
            // Hook the victim (vanilla `field c`), damage 0 like snowballs.
            self.attack_living(v, 0, Some(owner));
            if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                f.hooked_id = v;
            }
            return;
        }
        if let Some((hx, hy, hz)) = block_hit {
            let in_id = self.get_block_id(hx, hy, hz) as i32;
            if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                f.stuck = true;
                f.stuck_tile = [hx, hy, hz];
                f.stuck_in = in_id;
                f.body.motion = [0.0; 3];
            }
            return;
        }
        // Free flight + water physics (vanilla lines 177-256).
        let (mx, my, mz) = match self.entities.get(id) {
            Some(e) => (e.body().motion[0], e.body().motion[1], e.body().motion[2]),
            None => return,
        };
        self.move_body(id, mx, my, mz);
        // Face of travel (smoothed like the snowball tail).
        if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
            let horizontal = (mx * mx + mz * mz).sqrt();
            let yaw = (mx.atan2(mz) * 180.0 / std::f64::consts::PI) as f32;
            let pitch = (my.atan2(horizontal) * 180.0 / std::f64::consts::PI) as f32;
            f.body.yaw = f.body.prev_yaw + (yaw - f.body.prev_yaw) * 0.2;
            f.body.pitch = f.body.prev_pitch + (pitch - f.body.prev_pitch) * 0.2;
        }
        // Water fraction over 5 vertical slices (mirrors the scan).
        let frac = match self.entities.get(id) {
            Some(e) => {
                let bb = e.body().bounding_box;
                crate::entity::misc::water_fraction_scan(
                    bb.min_x, bb.min_y, bb.min_z, bb.max_x, bb.max_y, bb.max_z,
                    |x, y, z| self.is_water(x, y, z),
                )
            }
            None => return,
        };
        let on_ground = matches!(self.entities.get(id), Some(e) if e.body().on_ground);
        let in_water_mat = self.touching_liquid(id);
        let mut drag = 0.92f64;
        if on_ground || in_water_mat {
            drag = 0.5;
        }
        if frac > 0.0 {
            // Nibble roll: 1/500 per wet tick → 10+rand(30) window + dip.
            if self.rng.next_int_bound(500) == 0 {
                if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
                    f.nibble_ticks = self.rng.next_int_bound(30) + 10;
                    f.body.motion[1] -= 0.2;
                }
            }
        }
        if let Some(Entity::FishHook(f)) = self.entities.get_mut(id) {
            if f.nibble_ticks > 0 {
                let r = self.rng.next_float() as f64;
                f.body.motion[1] -= r * r * r * 0.2;
            }
            let lift = frac * 2.0 - 1.0;
            f.body.motion[1] += 0.04 * lift;
            if frac > 0.0 {
                drag *= 0.9;
                f.body.motion[1] *= 0.8;
            }
            f.body.motion[0] *= drag;
            f.body.motion[1] *= drag;
            f.body.motion[2] *= drag;
        }
    }

    /// Reel-in outcome (vanilla `func_6143_c`): hooked entity → yank
    /// toward the owner + rod damage 3; nibble window → raw fish flown
    /// to the owner + damage 1; stuck in ground → damage 2; else 0.
    /// Always kills the hook and clears the owner's line.
    /// Returns the rod damage dealt.
    pub(crate) fn reel_fishing(&mut self, owner: EntityId, hook: EntityId) -> i32 {
        let (hx, hy, hz) = match self.entities.get(hook) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return 0,
        };
        let hooked = match self.entities.get(hook) {
            Some(Entity::FishHook(f)) => f.hooked_id,
            _ => crate::entity::table::NO_ENTITY,
        };
        let nibbling = matches!(
            self.entities.get(hook),
            Some(Entity::FishHook(f)) if f.nibble_ticks > 0
        );
        let stuck = matches!(
            self.entities.get(hook),
            Some(Entity::FishHook(f)) if f.stuck
        );
        let mut damage = 0;
        if hooked >= 0 && matches!(self.entities.get(hooked), Some(e) if !e.body().dead) {
            if let (Some(o), Some(_)) = (self.entities.get(owner), self.entities.get(hooked)) {
                let (dx, dy, dz) = (
                    o.body().pos[0] - hx,
                    o.body().pos[1] - hy,
                    o.body().pos[2] - hz,
                );
                let d = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
                if let Some(v) = self.entities.get_mut(hooked) {
                    let b = v.body_mut();
                    b.motion[0] += dx * 0.1;
                    b.motion[1] += dy * 0.1 + d.sqrt() * 0.08;
                    b.motion[2] += dz * 0.1;
                }
            }
            self.attack_living(hooked, 0, Some(owner));
            damage = 3;
        } else if nibbling {
            let fish = self.spawn_item_entity(349, 1, 0, hx, hy, hz);
            if let (Some(o), Some(_)) = (self.entities.get(owner), self.entities.get(fish)) {
                let (dx, dy, dz) = (
                    o.body().pos[0] - hx,
                    o.body().pos[1] - hy,
                    o.body().pos[2] - hz,
                );
                let d = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
                if let Some(Entity::Item(it)) = self.entities.get_mut(fish) {
                    it.body.motion[0] = dx * 0.1;
                    it.body.motion[1] = dy * 0.1 + d.sqrt() * 0.08;
                    it.body.motion[2] = dz * 0.1;
                }
            }
            damage = 1;
        } else if stuck {
            damage = 2;
        }
        if let Some(Entity::FishHook(f)) = self.entities.get_mut(hook) {
            f.body.dead = true;
        }
        if damage > 0 {
            self.damage_held_rod(owner, damage);
        }
        damage
    }

    /// Wear the owner's held fishing rod by `amount` (vanilla
    /// `damageItem` on reel/catch; rod breaks at 64 like tools).
    fn damage_held_rod(&mut self, owner: EntityId, amount: i32) {
        let cur = match self.entities.get(owner) {
            Some(Entity::Player(p)) => p.inventory.current,
            _ => return,
        };
        if !(0..36).contains(&cur) {
            return;
        }
        let mut slot = match self.entities.get(owner) {
            Some(Entity::Player(p)) => p.inventory.main[cur as usize],
            _ => None,
        };
        if let Some(mut s) = slot {
            if s.item_id == 346 {
                let max = crate::item_data::item_max_damage(346);
                crate::inventory::item_stack_damage(&mut s, amount, max);
                slot = if s.count <= 0 || s.damage > max { None } else { Some(s) };
            }
        }
        if let Some(Entity::Player(p)) = self.entities.get_mut(owner) {
            p.inventory.main[cur as usize] = slot;
        }
    }

    /// Ghast fireball (mirrors `EntityFireball` construction in
    /// `EntityGhast.func_4126_a`): eye-height start, normalized aim at
    /// the victim's eye with small spread, speed ~1.2, blast radius 1.
    /// Tracked as vehicle 65 so victims see it coming (vanilla has no
    /// tracker branch for it — documented in entity/table.rs).
    pub(crate) fn spawn_fireball(&mut self, id: EntityId, target: EntityId) {
        let (sp, eye) = match self.entities.get(id) {
            Some(e) => (e.body().pos, e.body().height as f64 * 0.5),
            None => return,
        };
        let tp = match self.entities.get(target) {
            Some(t) => {
                let te = Self::living_eye_height(t);
                [t.body().pos[0], t.body().pos[1] + te, t.body().pos[2]]
            }
            None => return,
        };
        let (mut dx, mut dy, mut dz) =
            (tp[0] - sp[0], tp[1] - (sp[1] + eye), tp[2] - sp[2]);
        let len = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
        // Vanilla spread: gaussian-ish jitter via rand draws.
        dx += self.rng.next_double() * 0.2 - 0.1;
        dy += self.rng.next_double() * 0.2 - 0.1;
        dz += self.rng.next_double() * 0.2 - 0.1;
        let nlen = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
        let motion = [dx / nlen * 1.2, dy / nlen * 1.2, dz / nlen * 1.2];
        let nid = self.entities.alloc_id();
        let mut b = Body::new(nid, 1.0, 1.0, 0.0);
        b.set_position(sp[0], sp[1] + eye, sp[2]);
        b.motion = motion;
        self.entities.insert(Entity::Fireball(crate::entity::table::FireballEnt {
            body: b,
            owner_id: id,
            ticks_in_air: 0,
        }));
        self.stamp_dim(nid);
        let _ = len;
    }

    /// Skeleton volley (mirrors `EntitySkeleton.java:30-53`): loose an arrow
    /// inside reach 10 on cooldown 30, face the target, and raise
    /// `has_attacked` (`field_387_ah = true`) so the skeleton strafes/stops
    /// instead of charging into melee range.
    fn skeleton_volley(
        &mut self,
        id: EntityId,
        target: EntityId,
        dist: f32,
        snap: &mut CreatureSnap,
    ) -> Option<EntityId> {
        if dist < 10.0 {
            let (dx, dz, ready) = match (self.entities.get(id), self.entities.get(target)) {
                (Some(s), Some(t)) => (
                    t.body().pos[0] - s.body().pos[0],
                    t.body().pos[2] - s.body().pos[2],
                    matches!(s, Entity::Mob(m) if m.attack_cooldown == 0),
                ),
                _ => return Some(target),
            };
            if ready {
                if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
                    m.attack_cooldown = 30;
                }
                self.spawn_skeleton_arrow(id, target);
            }
            snap.yaw = (dz.atan2(dx) * 180.0 / std::f64::consts::PI) as f32 - 90.0;
            snap.has_attacked = true;
        }
        Some(target)
    }

    /// Skeleton arrow (mirrors the ctor + volley sequence): eye-height
    /// start with the 0.16 yaw backoff (quantized tables), the Java-parity
    /// +1.4 lift, aim at the victim's eye minus 0.2 with the f32 range
    /// lift, and 0.6/12.0 launch. The ctor-equivalent spread run is kept
    /// (draws consumed like C++) with its result discarded unless the
    /// volley aim degenerates.
    pub(crate) fn spawn_skeleton_arrow(&mut self, id: EntityId, target: EntityId) {
        use crate::entity::misc::arrow_shoot_run;
        use crate::math_helper::{cos, sin};
        let (sp, yaw, pitch, eye) = match self.entities.get(id) {
            Some(e) => (e.body().pos, e.body().yaw, e.body().pitch, e.body().height as f64 * 0.85),
            None => return,
        };
        let tp = match self.entities.get(target) {
            Some(t) => {
                let te = Self::living_eye_height(t);
                [t.body().pos[0], t.body().pos[1] + te, t.body().pos[2]]
            }
            None => return,
        };
        let rad = yaw / 180.0 * std::f32::consts::PI;
        let (ax, ay, az) =
            (sp[0] - cos(rad) as f64 * 0.16, sp[1] + eye - 0.1, sp[2] - sin(rad) as f64 * 0.16);
        let (dx, dz) = (tp[0] - sp[0], tp[2] - sp[2]);
        let dy = tp[1] - 0.2 - ay;
        let lift = sqrt_float((dx * dx + dz * dz) as f32) * 0.2;
        // Ctor-equivalent spread (result discarded; draws consumed).
        let prad = pitch / 180.0 * std::f32::consts::PI;
        let (myaw, mpitch) = (rad, prad);
        let dir = [
            -sin(myaw) as f64 * cos(mpitch) as f64,
            -sin(mpitch) as f64,
            cos(myaw) as f64 * cos(mpitch) as f64,
        ];
        let ctor_motion = {
            let rng = &mut self.rng;
            arrow_shoot_run(dir[0], dir[1], dir[2], 1.5, 1.0, &mut || rng.next_double())
        };
        let motion = {
            let rng = &mut self.rng;
            arrow_shoot_run(dx, dy + lift as f64, dz, 0.6, 12.0, &mut || rng.next_double())
        };
        let motion = motion.or(ctor_motion).unwrap_or([0.0, 0.0, 0.0]);
        let (mut fy, mut fp) = (yaw, pitch);
        crate::entity::misc::arrow_face_velocity(motion[0], motion[1], motion[2], &mut fy, &mut fp);
        let nid = self.entities.alloc_id();
        let mut b = Body::new(nid, 0.5, 0.5, 0.0);
        b.set_position(ax, ay, az);
        b.yaw = fy;
        b.pitch = fp;
        b.prev_yaw = fy;
        b.prev_pitch = fp;
        b.motion = motion;
        self.entities.insert(Entity::Arrow(crate::entity::table::ArrowEnt {
            body: b,
            in_ground: false,
            shake: 0,
            ticks_in_ground: 0,
            ticks_in_air: 0,
            shooter_id: id,
            tile: [-1, -1, -1],
            in_tile: 0,
        }));
    }

    /// Spider attack (mirrors the override): drop the target in bright
    /// light sometimes, pounce from 2..6 blocks on the ground, else the
    /// reach-2.5 strength-2 bite with vertical overlap (Java EntityMobs:50).
    fn spider_attack(
        &mut self,
        id: EntityId,
        target: EntityId,
        dist: f32,
        snap: &mut CreatureSnap,
    ) -> Option<EntityId> {
        let (px, min_y, height, pz, on_ground, motion) = match self.entities.get(id) {
            Some(e) => (
                e.body().pos[0],
                e.body().bounding_box.min_y,
                e.body().height as f64,
                e.body().pos[2],
                e.body().on_ground,
                e.body().motion,
            ),
            None => return Some(target),
        };
        // EntitySpider.java:38-40: 1/100 daytime target drop
        if self.brightness(floor_double(px), floor_double(min_y + height * 0.66), floor_double(pz)) > 0.5
            && self.rng.next_int_bound(100) == 0
        {
            self.store_mob_target(id, None);
            snap.path.clear();
            snap.path_index = 0;
            return None;
        }
        if dist > 2.0 && dist < 6.0 && self.rng.next_int_bound(10) == 0 && on_ground {
            if let (Some(s), Some(t)) = (self.entities.get(id), self.entities.get(target)) {
                let (dx, dz) = (t.body().pos[0] - s.body().pos[0], t.body().pos[2] - s.body().pos[2]);
                let len = (dx * dx + dz * dz).sqrt().max(0.001);
                if let Some(e) = self.entities.get_mut(id) {
                    let b = e.body_mut();
                    b.motion[0] = dx / len * 0.4 + motion[0] * 0.2;
                    b.motion[2] = dz / len * 0.4 + motion[2] * 0.2;
                    b.motion[1] = 0.4;
                    let m = b.motion;
                    self.velocity_events.push((id, m));
                }
            }
            return Some(target);
        }
        let ready = matches!(self.entities.get(id), Some(Entity::Mob(m)) if m.attack_cooldown == 0);
        let overlap = match (self.entities.get(id), self.entities.get(target)) {
            (Some(s), Some(t)) => {
                t.body().bounding_box.max_y > s.body().bounding_box.min_y
                    && t.body().bounding_box.min_y < s.body().bounding_box.max_y
            }
            _ => false,
        };
        if dist < 2.5 && overlap && ready {
            if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
                m.attack_cooldown = 20;
            }
            self.attack_living(target, 2, Some(id));
        }
        Some(target)
    }

    /// Creeper fuse when target is visible (`EntityCreeper.java:85-100`):
    /// swell while close (3 blocks cold, 7 once lit), raise `has_attacked`
    /// (`field_387_ah = true`) so the creeper stops advancing while hissing,
    /// decay otherwise, and explode at 30.
    fn creeper_swell(
        &mut self,
        id: EntityId,
        target: EntityId,
        dist: f32,
        snap: &mut CreatureSnap,
    ) -> Option<EntityId> {
        let (time, prev_dir) = match self.entities.get(id) {
            Some(Entity::Mob(m)) => (m.swell_time, m.swell_dir),
            _ => return Some(target),
        };
        if (prev_dir <= 0 && dist < 3.0) || (prev_dir > 0 && dist < 7.0) {
            snap.has_attacked = true;
            let time = time + 1;
            if time >= 30 {
                if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
                    m.swell_time = time;
                    m.swell_dir = 1;
                }
                self.creeper_explode(id);
                return Some(target);
            }
            if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
                m.swell_time = time;
                m.swell_dir = 1;
            }
            if prev_dir <= 0 {
                self.status_events.push((id, 4));
            }
        } else {
            self.creeper_defuse_step(id);
        }
        Some(target)
    }

    /// Creeper fuse decay when out of range, out of LOS, or targetless
    /// (`EntityCreeper.java:55-83`): decrements `swell_time` if `> 0`,
    /// sets `swell_dir = -1`, and emits status 5 when transitioning from
    /// ignited (`> 0`) to defused (`-1`).
    pub(crate) fn creeper_defuse_step(&mut self, id: EntityId) {
        let (time, prev_dir) = match self.entities.get(id) {
            Some(Entity::Mob(m)) => (m.swell_time, m.swell_dir),
            _ => return,
        };
        let new_time = if time > 0 { time - 1 } else { time };
        if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
            m.swell_time = new_time;
            m.swell_dir = -1;
        }
        if prev_dir > 0 {
            self.status_events.push((id, 5));
        }
    }

    /// Shared blast (vanilla `Explosion`): entity damage with the Java
    /// formula `(v*v+v)/2*8*size+1` followed by blast velocity impulse
    /// (`Explosion.java:103-106`), LOS-gated by the eye raytrace; blocks in
    /// the sphere destroyed unless unbreakable, drops at 0.3 via the harvest
    /// table, TNT cells chain-ignite instead of dropping, and queues an
    /// explosion event for `Packet60` (`WorldServer.java:92-96`).
    pub(crate) fn blast(&mut self, px: f64, py: f64, pz: f64, radius: f32, attacker: Option<EntityId>) {
        self.blast_flaming(px, py, pz, radius, attacker, false);
    }

    /// Shared blast with optional fire ignition (`Explosion.field_12031_a`,
    /// `Explosion.java:110-122`).
    pub fn blast_flaming(
        &mut self,
        px: f64,
        py: f64,
        pz: f64,
        radius: f32,
        attacker: Option<EntityId>,
        is_flaming: bool,
    ) {
        // Phase 1: living victims in the blast radius.
        let mut victims: Vec<EntityId> = Vec::new();
        for oid in self.entities.alive_ids() {
            let is_living = match self.entities.get(oid) {
                Some(Entity::Mob(m)) if !m.living.body.dead => true,
                Some(Entity::Animal(a)) if !a.living.body.dead => true,
                Some(Entity::Player(p)) if !p.living.body.dead => true,
                _ => false,
            };
            if !is_living {
                continue;
            }
            if Some(oid) == attacker {
                continue;
            }
            let (qx, qy, qz) = match self.entities.get(oid) {
                Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
                None => continue,
            };
            let (dx, dy, dz) = (qx - px, qy - py, qz - pz);
            let d = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
            if d > radius {
                continue;
            }
            victims.push(oid);
        }
        victims.sort_unstable();
        for v in victims {
            let (qx, qy, qz, h) = match self.entities.get(v) {
                Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2], e.body().height as f64),
                None => continue,
            };
            let (dx, dy, dz) = (qx - px, qy - py, qz - pz);
            let d = ((dx * dx + dy * dy + dz * dz) as f32).sqrt();
            if d > radius {
                continue;
            }
            // LOS from the blast center to the victim's eye.
            if !self.ray_trace_clear([px, py, pz], [qx, qy + h * 0.85, qz]) {
                continue;
            }
            let vfrac = 1.0 - d / radius;
            let damage = ((vfrac as f64 * vfrac as f64 + vfrac as f64) / 2.0 * 8.0 * radius as f64 + 1.0) as i32;
            // Explosion.java:103-106: attackEntity runs FIRST, then the blast
            // velocity is added on top of post-hit motion.
            self.attack_living(v, damage.max(1), attacker);
            let len = (dx * dx + dy * dy + dz * dz).sqrt().max(0.001);
            if let Some(e) = self.entities.get_mut(v) {
                let b = e.body_mut();
                b.motion[0] += dx / len * vfrac as f64;
                b.motion[1] += dy / len * vfrac as f64;
                b.motion[2] += dz / len * vfrac as f64;
                let m = b.motion;
                self.velocity_events.push((v, m));
            }
        }
        // Phase 2: 16x16x16 border raycasting with per-step resistance attenuation (Explosion.java:46-75).
        // Hardening: stop starting new rays once the cell cap is hit
        // (single blasts never reach it; TNT fields can't lag the tick).
        let mut destroyed = std::collections::BTreeSet::new();
        'rays: for ix in 0..16 {
            for iy in 0..16 {
                for iz in 0..16 {
                    if destroyed.len() >= MAX_BLAST_CELLS {
                        break 'rays;
                    }
                    if ix != 0 && ix != 15 && iy != 0 && iy != 15 && iz != 0 && iz != 15 {
                        continue;
                    }
                    let mut vx = ix as f64 / 15.0 * 2.0 - 1.0;
                    let mut vy = iy as f64 / 15.0 * 2.0 - 1.0;
                    let mut vz = iz as f64 / 15.0 * 2.0 - 1.0;
                    let len = (vx * vx + vy * vy + vz * vz).sqrt();
                    vx /= len;
                    vy /= len;
                    vz /= len;

                    let mut power = radius * (0.7 + self.rng.next_float() * 0.6);
                    let mut rx = px;
                    let mut ry = py;
                    let mut rz = pz;
                    while power > 0.0 {
                        let bx = rx.floor() as i32;
                        let by = ry.floor() as i32;
                        let bz = rz.floor() as i32;
                        let bid = self.get_block_id(bx, by, bz);
                        if bid > 0 {
                            let props = block_properties_get(bid as u32);
                            if props.hardness < 0.0 {
                                break;
                            }
                            power -= (props.resistance / 5.0 + 0.3) * 0.3;
                        }
                        if power > 0.0 {
                            destroyed.insert((bx, by, bz));
                        }
                        rx += vx * 0.3;
                        ry += vy * 0.3;
                        rz += vz * 0.3;
                        power -= 0.3 * 0.75;
                    }
                }
            }
        }
        let cells: Vec<(i32, i32, i32)> = destroyed.iter().copied().collect();
        self.explosion_events.push((px, py, pz, radius, cells.clone()));
        let mut tnt_chain: Vec<(i32, i32, i32)> = Vec::new();
        let mut removals: Vec<(i32, i32, i32, u8, u8)> = Vec::new();
        for (bx, by, bz) in destroyed {
            let bid = self.get_block_id(bx, by, bz);
            if bid == 0 {
                continue;
            }
            if bid == 46 {
                tnt_chain.push((bx, by, bz));
                removals.push((bx, by, bz, bid, self.get_block_meta(bx, by, bz)));
                continue;
            }
            if self.rng.next_float() <= 0.3 {
                removals.push((bx, by, bz, bid, self.get_block_meta(bx, by, bz)));
            } else {
                removals.push((bx, by, bz, 0, 0));
            }
        }
        for (bx, by, bz, bid, meta) in removals {
            if bid == 0 {
                self.apply_set_notify(bx, by, bz, 0);
                continue;
            }
            if bid == 46 {
                // TNT never drops — it chains with a short fuse.
                self.apply_set_notify(bx, by, bz, 0);
                continue;
            }
            let (drop, qty) = self.rolled_drop_ids(bid, meta);
            self.apply_set_notify(bx, by, bz, 0);
            if drop > 0 && qty > 0 {
                self.spawn_item_entity(drop, qty, 0, bx as f64 + 0.5, by as f64 + 0.5, bz as f64 + 0.5);
            }
        }
        for (bx, by, bz) in tnt_chain {
            let fuse = 10 + self.rng.next_int_bound(21);
            self.ignite_tnt(bx, by, bz, fuse);
        }
        // Explosion.java:110-122: when isFlaming is set, 1/3 of air cells in
        // the blast volume sitting on top of an opaque/solid block ignite.
        if is_flaming {
            for &(bx, by, bz) in cells.iter().rev() {
                if self.get_block_id(bx, by, bz) == 0
                    && self.block_allows_attachment(bx, by - 1, bz)
                    && self.rng.next_int_bound(3) == 0
                {
                    self.apply_set_notify(bx, by, bz, 51);
                }
            }
        }
    }

    /// Ignite TNT at a cell (mirrors `BlockTNT.onBlockDestroyedByPlayer` +
    /// `BlockFire.tryToCatchBlockOnFire` for id 46): the block vanishes at
    /// once (no drop) and a visible primed entity (`EntityTNTPrimed`,
    /// tracked 160/10) carries the fuse. Hand-lit fuses run 80 ticks,
    /// chained ones pass an explicit short fuse.
    pub fn ignite_tnt(&mut self, x: i32, y: i32, z: i32, fuse: i32) {
        if self.get_block_id(x, y, z) == 46 {
            self.apply_set_notify(x, y, z, 0);
        }
        let nid = self.entities.alloc_id();
        let mut b = Body::new(nid, 0.98, 0.98, 0.0);
        b.set_position(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5);
        b.motion = [0.0, 0.2, 0.0];
        self.entities.insert(Entity::Tnt(crate::entity::table::TntEnt {
            body: b,
            fuse,
        }));
        self.stamp_dim(nid);
        self.mark_chunk_modified(x >> 4, z >> 4);
    }

    /// Tick one primed TNT entity (mirrors `EntityTNTPrimed.onUpdate`):
    /// gravity, ground friction, fuse countdown, radius-4 blast at zero.
    pub fn tick_tnt(&mut self, id: EntityId) {
        self.entities.tick_base(id);
        let (fuse_left, motion) = match self.entities.get_mut(id) {
            Some(Entity::Tnt(t)) => {
                t.fuse -= 1;
                t.body.motion[1] -= 0.04;
                t.body.motion[0] *= 0.98;
                t.body.motion[1] *= 0.98;
                t.body.motion[2] *= 0.98;
                (t.fuse, (t.body.motion[0], t.body.motion[1], t.body.motion[2]))
            }
            _ => return,
        };
        self.move_body(id, motion.0, motion.1, motion.2);
        if fuse_left <= 0 {
            let (px, py, pz) = match self.entities.get(id) {
                Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
                None => return,
            };
            if let Some(e) = self.entities.get_mut(id) {
                e.body_mut().dead = true;
            }
            self.blast(px, py, pz, 4.0, None);
        }
    }

    /// Drain legacy `pending_tnt` rows (pre-entity saves; the live path
    /// spawns `TntEnt` directly). Kept so old in-flight fuses still boom.
    pub fn tick_primed_tnt(&mut self) {
        if self.pending_tnt.is_empty() {
            return;
        }
        let mut due: Vec<(f64, f64, f64)> = Vec::new();
        for entry in self.pending_tnt.iter_mut() {
            entry.3 -= 1;
            if entry.3 <= 0 {
                due.push((entry.0 as f64 + 0.5, entry.1 as f64 + 0.5, entry.2 as f64 + 0.5));
            }
        }
        self.pending_tnt.retain(|e| e.3 > 0);
        for (px, py, pz) in due {
            self.blast(px, py, pz, 4.0, None);
        }
    }

    /// Creeper blast at radius 3 (mirrors `EntityCreeper` fuse end): shared
    /// ballistics, then the creeper dies without drops.
    pub(crate) fn creeper_explode(&mut self, id: EntityId) {
        let (px, py, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return,
        };
        self.blast(px, py, pz, CREEPER_BLAST_RADIUS, Some(id));
        if let Some(e) = self.entities.get_mut(id) {
            e.body_mut().dead = true;
        }
    }

    /// Lava-water contact (mirrors `BlockFluids.func_302_i`): when a lava
    /// cell touches water on any of the 4 sides or above, source lava
    /// (meta 0) becomes obsidian, flowing lava (meta <= 4) becomes
    /// cobblestone. Only lava cells trigger (water cells never do).
    pub(crate) fn fluid_lava_contact(&mut self, x: i32, y: i32, z: i32, bid: u8) {
        if !matches!(bid, 10 | 11) {
            return;
        }
        if self.get_block_id(x, y, z) != bid {
            return;
        }
        let wet = self.material_at(x + 1, y, z) == Material::WATER
            || self.material_at(x - 1, y, z) == Material::WATER
            || self.material_at(x, y, z + 1) == Material::WATER
            || self.material_at(x, y, z - 1) == Material::WATER
            || self.material_at(x, y + 1, z) == Material::WATER;
        if !wet {
            return;
        }
        let meta = self.get_block_meta(x, y, z);
        if meta == 0 {
            self.apply_set_notify(x, y, z, 49);
        } else if meta <= 4 {
            self.apply_set_notify(x, y, z, 4);
        }
    }

    /// Soil check for fire (mirrors `doesBlockAllowAttachment`: solid and
    /// movement-blocking material).
    pub(crate) fn block_allows_attachment(&self, x: i32, y: i32, z: i32) -> bool {
        let bid = self.get_block_id(x, y, z);
        if bid == 0 {
            return false;
        }
        block_properties_get(bid as u32).allows_attachment
    }

    /// Arrow tick (mirrors `EntityArrow::tick`): face init, shake decay,
    /// stuck-block tracking (1200-tick despawn, pop-out jitter), block
    /// sweep with AABB clips, entity sweep for 4 damage, then ballistic
    /// flight with yaw smoothing and water drag. Fire/cactus contacts
    /// arrive with the env-state slice.
    pub fn tick_arrow(&mut self, id: EntityId) {
        use crate::vec3d::Vec3D;
        self.entities.tick_base(id);
        if !matches!(self.entities.get(id), Some(Entity::Arrow(a)) if !a.body.dead) {
            return;
        }
        // Face from motion while both prev angles are still zero.
        let (mx, my, mz, yaw, pitch, pyaw, ppitch) = match self.entities.get(id) {
            Some(Entity::Arrow(a)) => (
                a.body.motion[0], a.body.motion[1], a.body.motion[2], a.body.yaw, a.body.pitch,
                a.body.prev_yaw, a.body.prev_pitch,
            ),
            _ => return,
        };
        if ppitch == 0.0 && pyaw == 0.0 {
            let (mut fy, mut fp) = (yaw, pitch);
            let faced =
                crate::entity::misc::arrow_face_velocity(mx, my, mz, &mut fy, &mut fp);
            if faced {
                if let Some(Entity::Arrow(a)) = self.entities.get_mut(id) {
                    a.body.prev_yaw = fy;
                    a.body.yaw = fy;
                    a.body.prev_pitch = fp;
                    a.body.pitch = fp;
                }
            }
        }
        if let Some(Entity::Arrow(a)) = self.entities.get_mut(id) {
            if a.shake > 0 {
                a.shake -= 1;
            }
        }
        // Stuck handling.
        let stuck_outcome = match self.entities.get(id) {
            Some(Entity::Arrow(a)) if a.in_ground
                && self.get_block_id(a.tile[0], a.tile[1], a.tile[2]) as i32 == a.in_tile => {
                    let old = a.ticks_in_ground;
                    if let Some(Entity::Arrow(x)) = self.entities.get_mut(id) {
                        x.ticks_in_ground = old + 1;
                    }
                    if old + 1 >= 1200 {
                        if let Some(Entity::Arrow(x)) = self.entities.get_mut(id) {
                            x.body.dead = true;
                        }
                    }
                    true // stay (dead or still stuck)
                }
            _ => false,
        };
        if stuck_outcome {
            return;
        }
        let popped = matches!(self.entities.get(id), Some(Entity::Arrow(a)) if a.in_ground);
        if popped {
            let (jx, jy, jz) =
                (self.rng.next_double(), self.rng.next_double(), self.rng.next_double());
            if let Some(Entity::Arrow(a)) = self.entities.get_mut(id) {
                a.body.motion[0] *= jx * 0.2;
                a.body.motion[1] *= jy * 0.2;
                a.body.motion[2] *= jz * 0.2;
                a.in_ground = false;
                a.ticks_in_ground = 0;
                a.ticks_in_air = 0;
            }
        } else if let Some(Entity::Arrow(a)) = self.entities.get_mut(id) {
            a.ticks_in_air += 1;
        }
        let (pos, motion, bbox) = match self.entities.get(id) {
            Some(Entity::Arrow(a)) => (a.body.pos, a.body.motion, a.body.bounding_box),
            _ => return,
        };
        let start = Vec3D::new(pos[0], pos[1], pos[2]);
        let end = Vec3D::new(pos[0] + motion[0], pos[1] + motion[1], pos[2] + motion[2]);
        // Block sweep over the padded flight box (nearest clip wins).
        let sweep = bbox.add_coord(motion[0], motion[1], motion[2]).expand(1.0, 1.0, 1.0);
        let (min_x, min_y, min_z) = (
            floor_double(sweep.min_x), floor_double(sweep.min_y), floor_double(sweep.min_z),
        );
        let (max_x, max_y, max_z) = (
            floor_double(sweep.max_x), floor_double(sweep.max_y), floor_double(sweep.max_z),
        );
        let mut block_hit: Option<(i32, i32, i32, i32, Vec3D, f64)> = None;
        for x in min_x..=max_x {
            for y in min_y..=max_y {
                for z in min_z..=max_z {
                    let bid = self.get_block_id(x, y, z);
                    if bid == 0 {
                        continue;
                    }
                    let props = block_properties_get(bid as u32);
                    if !has_collision_box(props.block_type) || !has_collision_id(bid) {
                        continue;
                    }
                    let bb = AxisAlignedBB::get_bounding_box(
                        x as f64 + props.min_x as f64,
                        y as f64 + props.min_y as f64,
                        z as f64 + props.min_z as f64,
                        x as f64 + props.max_x as f64,
                        y as f64 + props.max_y as f64,
                        z as f64 + props.max_z as f64,
                    );
                    if let Some(hit) = bb.clip(&start, &end) {
                        let d = start.square_distance_to(&hit.hit_vec);
                        if block_hit.map(|(_, _, _, _, _, bd)| d < bd).unwrap_or(true) {
                            block_hit = Some((x, y, z, bid as i32, hit.hit_vec, d));
                        }
                    }
                }
            }
        }
        // Entity sweep (anything collidable: mobs, animals, players, boats,
        // items — arrows themselves are not; the shooter is immune while
        // the arrow is young).
        let (shooter, ticks_in_air) = match self.entities.get(id) {
            Some(Entity::Arrow(a)) => (a.shooter_id, a.ticks_in_air),
            _ => return,
        };
        let mut best = block_hit.map(|(_, _, _, _, _, d)| d).unwrap_or(f64::MAX);
        let mut entity_hit: Option<EntityId> = None;
        let mut cands: Vec<EntityId> = Vec::new();
        for oid in self.entities.alive_ids() {
            if oid == id {
                continue;
            }
            if matches!(self.entities.get(oid), Some(Entity::Arrow(_))) {
                continue;
            }
            if oid == shooter && ticks_in_air < 5 {
                continue;
            }
            if let Some(o) = self.entities.get(oid) {
                if sweep.intersects_with(&o.body().bounding_box) {
                    cands.push(oid);
                }
            }
        }
        cands.sort_unstable();
        for oid in cands {
            let expanded = match self.entities.get(oid) {
                Some(o) => o.body().bounding_box.expand(0.3, 0.3, 0.3),
                None => continue,
            };
            if let Some(hit) = expanded.clip(&start, &end) {
                let d = start.square_distance_to(&hit.hit_vec);
                if d < best {
                    best = d;
                    entity_hit = Some(oid);
                }
            }
        }
        if let Some(v) = entity_hit {
            let shooter_living = match self.entities.get(shooter) {
                Some(Entity::Mob(_)) | Some(Entity::Animal(_)) | Some(Entity::Player(_)) => {
                    Some(shooter)
                }
                _ => None,
            };
            match self.entities.get(v) {
                Some(Entity::Boat(_)) => {
                    self.damage_boat(v, 4);
                }
                Some(Entity::Mob(_)) | Some(Entity::Animal(_)) | Some(Entity::Player(_)) => {
                    self.attack_living(v, 4, shooter_living);
                }
                _ => {}
            }
            if let Some(Entity::Arrow(a)) = self.entities.get_mut(id) {
                a.body.dead = true;
            }
            return;
        }
        if let Some((hx, hy, hz, bid, hit_vec, _)) = block_hit {
            if let Some(Entity::Arrow(a)) = self.entities.get_mut(id) {
                a.tile = [hx, hy, hz];
                a.in_tile = bid;
                a.body.set_position(hit_vec.x_coord, hit_vec.y_coord, hit_vec.z_coord);
                a.in_ground = true;
                a.shake = 7;
            }
            return;
        }
        // Free flight with yaw smoothing and drag.
        let (mx, my, mz) = match self.entities.get(id) {
            Some(Entity::Arrow(a)) => (a.body.motion[0], a.body.motion[1], a.body.motion[2]),
            _ => return,
        };
        let horizontal = sqrt_float((mx * mx + mz * mz) as f32);
        let mut yaw = (mx.atan2(mz) * 180.0 / std::f64::consts::PI) as f32;
        let mut pitch = (my.atan2(horizontal as f64) * 180.0 / std::f64::consts::PI) as f32;
        let (mut prev_yaw, mut prev_pitch) = match self.entities.get(id) {
            Some(Entity::Arrow(a)) => (a.body.prev_yaw, a.body.prev_pitch),
            _ => return,
        };
        while pitch - prev_pitch < -180.0 {
            prev_pitch -= 360.0;
        }
        while pitch - prev_pitch >= 180.0 {
            prev_pitch -= 360.0;
        }
        while yaw - prev_yaw < -180.0 {
            prev_yaw -= 360.0;
        }
        while yaw - prev_yaw >= 180.0 {
            prev_yaw -= 360.0;
        }
        pitch = prev_pitch + (pitch - prev_pitch) * 0.2;
        yaw = prev_yaw + (yaw - prev_yaw) * 0.2;
        // Water drag (local probe; the env slice will own `in_water`).
        let probe = match self.entities.get(id) {
            Some(Entity::Arrow(a)) => a.body.bounding_box.expand(0.0, -0.4, 0.0),
            _ => return,
        };
        let mut in_water = false;
        for x in floor_double(probe.min_x)..=floor_double(probe.max_x) {
            for y in floor_double(probe.min_y)..=floor_double(probe.max_y) {
                for z in floor_double(probe.min_z)..=floor_double(probe.max_z) {
                    if self.material_at(x, y, z) == Material::WATER {
                        in_water = true;
                        break;
                    }
                }
                if in_water {
                    break;
                }
            }
            if in_water {
                break;
            }
        }
        let drag = if in_water { 0.8f32 } else { 0.99f32 };
        if let Some(Entity::Arrow(a)) = self.entities.get_mut(id) {
            a.body.prev_yaw = prev_yaw;
            a.body.prev_pitch = prev_pitch;
            a.body.yaw = yaw;
            a.body.pitch = pitch;
            a.body.motion[0] = mx * drag as f64;
            a.body.motion[1] = my * drag as f64 - 0.03;
            a.body.motion[2] = mz * drag as f64;
            let (px, py, pz) = (a.body.pos[0] + mx, a.body.pos[1] + my, a.body.pos[2] + mz);
            // NOTE: C++ integrates the pre-drag motion, then damps.
            a.body.set_position(px, py, pz);
        }
    }
}
