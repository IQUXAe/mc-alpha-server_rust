//! Line of sight, mob attacks, explosions, arrows and TNT on [`World`].
//! Split out of `world.rs`; behavior unchanged.

use crate::aabb::AxisAlignedBB;
use crate::block::table::{BlockType, block_properties_get};
use crate::entity::table::{Body, Entity, EntityId, MobKind};
use crate::material::Material;
use crate::math_helper::{floor_double, sqrt_float};
use crate::world::{World, has_collision_box, has_collision_id, material_of};
use crate::world::ai::CreatureSnap;

/// Creeper blast radius (mirrors the Alpha inline `explode`).
const CREEPER_BLAST_RADIUS: f32 = 3.0;
/// Fire block id placed by explosions.
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
            return Some([ccx, ccy, ccz]);
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
                    let bb = self.block_collision_box(x, y, z, bid);
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
            MobKind::Zombie => self.zombie_punch(id, target, dist),
            MobKind::Skeleton => self.skeleton_volley(id, target, dist, snap),
            MobKind::Spider => self.spider_attack(id, target, dist, snap),
            MobKind::Creeper => self.creeper_swell(id, target, dist, snap),
        }
    }

    /// Base melee (mirrors `EntityMob::attackTarget`): in-reach, vertical
    /// overlap, cooldown-gated strength-5 poke.
    fn zombie_punch(&mut self, id: EntityId, target: EntityId, dist: f32) -> Option<EntityId> {
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
            self.attack_living(target, 5, Some(id));
        }
        Some(target)
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
        let mut destroyed = std::collections::BTreeSet::new();
        for ix in 0..16 {
            for iy in 0..16 {
                for iz in 0..16 {
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
        self.explosion_events.push((px, py, pz, radius, cells));
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
            self.pending_tnt.push((bx, by, bz, fuse));
        }
    }

    /// Ignite TNT at a cell (mirrors `BlockTNT.onBlockDestroyedByPlayer` +
    /// `BlockFire.tryToCatchBlockOnFire` for id 46): the block vanishes at
    /// once (no drop) and the radius-4 blast lands when the fuse burns out.
    /// Hand-lit fuses run 80 ticks like `EntityTNTPrimed`; chained ones
    /// pass an explicit short fuse.
    pub fn ignite_tnt(&mut self, x: i32, y: i32, z: i32, fuse: i32) {
        if self.get_block_id(x, y, z) == 46 {
            self.apply_set_notify(x, y, z, 0);
        }
        self.pending_tnt.push((x, y, z, fuse));
    }

    /// Tick primed TNT fuses; expired ones detonate at radius 4.
    /// Call once per world tick before entity ticks.
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
    pub(crate) fn block_allows_attachment(&self, x: i32, y: i32, z: i32) -> bool {        let bid = self.get_block_id(x, y, z);
        if bid == 0 {
            return false;
        }
        let mat = material_of(block_properties_get(bid as u32).material);
        mat.is_solid() && mat.blocks_movement()
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
