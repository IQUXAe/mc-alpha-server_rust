//! Mob/animal sensing, targeting, pathing and ticks on [`World`].
//! Split out of `world.rs`; behavior unchanged.

use crate::entity::ai::{
    ai_animal_path_weight, ai_mob_path_weight, chase_speed, face_run, steer_run, wander_pick,
    SteerIn,
};
use crate::entity::living::{HeadingIo, MoveFeedback, living_fall_damage, living_heading_run};
use crate::entity::physics::entity_push;
use crate::entity::table::{
    mob_attack_reach, mob_burns_in_daylight, AnimalKind, Entity, EntityId, MobKind,
};
use crate::material::Material;
use crate::math_helper::{floor_double, sqrt_float};
use crate::pathfinder::find_path_native;
use crate::world::{GRASS_BLOCK_ID, WORLD_HEIGHT, World};

/// Creature AI tuning constants (mirror the C++ `EntityCreature` /
/// `EntityMob` literals).
/// Target-acquire range (`getTargetRange`).
const CREATURE_TARGET_RANGE: f64 = 16.0;
/// Alpha grass block id (C++ `Block::grass->blockID`).
/// Alpha ladder block id (checked by the base `isOnLadder`).
const LADDER_BLOCK_ID: u8 = 65;
/// Alpha egg item id (`Item::egg` is `new Item(88)`, i.e. 256 + 88).
const EGG_ITEM_ID: i32 = 344;

/// Wander weight rule selecting the `getBlockPathWeight` override: mobs
/// score everything 0.0, animals prefer grass, else light minus a half.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WeightRule {
    Mob,
    Animal,
}

/// Owned AI snapshot of one mob/animal row so the tick can sequence shared
/// world queries and exclusive row updates without holding borrows.
/// `pub(crate)` because the combat slice steers creatures with it too.
#[derive(Clone, Debug)]
pub(crate) struct CreatureSnap {
    pub(crate) pos: [f64; 3],
    pub(crate) min_y: f64,
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) collided_horiz: bool,
    pub(crate) move_speed: f32,
    pub(crate) path: Vec<[i32; 3]>,
    pub(crate) path_index: usize,
}

impl World {
    /// Daytime check (mirrors `World::isDaytime`: the first half of the
    /// 24000-tick day).
    pub fn is_daytime(&self) -> bool {
        let t = self.time % 24000;
        (0..12000).contains(&t)
    }

    /// Sky visibility (mirrors `World::canBlockSeeSky`: at/above the height
    /// map is open, missing chunks and below-zero read closed).
    pub fn can_see_sky(&self, x: i32, y: i32, z: i32) -> bool {
        if y < 0 {
            return false;
        }
        if y >= WORLD_HEIGHT {
            return true;
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        self.chunks.get(&(cx, cz)).map(|c| y >= c.get_height_value(lx, lz)).unwrap_or(false)
    }

    /// Combined light as a 0.0..1.0 fraction (mirrors the
    /// `getBlockLightValue / 15.0f` brightness in `EntityMob`).
    pub fn brightness(&self, x: i32, y: i32, z: i32) -> f32 {
        self.block_light_value(x, y, z) as f32 / 15.0
    }

    /// Liquid touch (mirrors `EntityLiving::isTouchingLiquid`): liquid
    /// material at the feet or the block above. Missing chunks read air.
    pub(crate) fn touching_liquid(&self, id: EntityId) -> bool {
        let (px, min_y, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().bounding_box.min_y, e.body().pos[2]),
            None => return false,
        };
        let (x, y, z) = (floor_double(px), floor_double(min_y), floor_double(pz));
        self.material_at(x, y, z).is_liquid() || self.material_at(x, y + 1, z).is_liquid()
    }

    /// Lava touch (mirrors the lava branch of `EntityLiving.func_148_c`):
    /// lava at the feet or the block above damps 0.5 instead of 0.8.
    fn touching_lava(&self, id: EntityId) -> bool {
        let (px, min_y, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().bounding_box.min_y, e.body().pos[2]),
            None => return false,
        };
        let (x, y, z) = (floor_double(px), floor_double(min_y), floor_double(pz));
        self.material_at(x, y, z) == Material::LAVA || self.material_at(x, y + 1, z) == Material::LAVA
    }

    /// Ground slipperiness under the feet (mirrors `Block.slipperiness`
    /// read in `EntityLiving.func_148_c`): 0.98 on ice, 0.6 default.
    fn ground_slipperiness(&self, id: EntityId) -> f32 {
        let (px, min_y, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().bounding_box.min_y, e.body().pos[2]),
            None => return 0.6,
        };
        let (x, y, z) = (floor_double(px), floor_double(min_y) - 1, floor_double(pz));
        if self.get_block_id(x, y, z) == 79 {
            0.98
        } else {
            0.6
        }
    }

    /// Ladder grip (mirrors `isOnLadder`): ladders for everyone, any
    /// adjacent solid block for spiders (the Java wall-climb).
    fn ladder_for(&self, id: EntityId) -> bool {
        let row = self.entities.get(id);
        let (px, min_y, pz) = match row {
            Some(e) => (e.body().pos[0], e.body().bounding_box.min_y, e.body().pos[2]),
            None => return false,
        };
        let (x, y, z) = (floor_double(px), floor_double(min_y), floor_double(pz));
        let spider = matches!(row, Some(Entity::Mob(m)) if m.kind == MobKind::Spider);
        if spider {
            self.is_solid(x - 1, y, z)
                || self.is_solid(x + 1, y, z)
                || self.is_solid(x, y, z - 1)
                || self.is_solid(x, y, z + 1)
        } else {
            self.get_block_id(x, y, z) == LADDER_BLOCK_ID
                || self.get_block_id(x, y + 1, z) == LADDER_BLOCK_ID
        }
    }

    /// A target row counts when it is a live player with health left
    /// (mirrors `hasValidTarget`).
    pub(crate) fn target_alive(&self, id: EntityId) -> bool {
        matches!(
            self.entities.get(id),
            Some(Entity::Player(p)) if !p.living.body.dead && p.living.health > 0
        )
    }

    /// Aggro gate (mirrors `shouldAggroPlayer`): spiders only hunt when
    /// their own brightness is below 0.5, everyone else always aggroes.
    fn mob_aggro_ok(&self, kind: MobKind, id: EntityId) -> bool {
        if kind != MobKind::Spider {
            return true;
        }
        let (px, min_y, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().bounding_box.min_y, e.body().pos[2]),
            None => return false,
        };
        self.brightness(floor_double(px), floor_double(min_y), floor_double(pz)) < 0.5
    }

    /// Fresh-target scan (mirrors `acquireTarget`): closest live player in
    /// range that passes the aggro gate.
    fn acquire_target(&self, id: EntityId, kind: MobKind) -> Option<EntityId> {
        let (px, py, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return None,
        };
        let near = self.closest_player(px, py, pz, CREATURE_TARGET_RANGE)?;
        if !self.target_alive(near) || !self.mob_aggro_ok(kind, id) {
            return None;
        }
        // Java EntityMobs.func_158_i checks LOS, but EntitySpider overrides
        // to brightness + distance only (no raytrace) for the initial
        // acquire. Requiring LOS here made spiders blind around corners.
        if kind != MobKind::Spider {
            // Java EntityMobs.func_158_i: only players the mob can see
            // (eye-to-eye raytrace clear, EntityLiving.func_145_g).
            if !self.can_entity_see(id, near) {
                return None;
            }
        }
        Some(near)
    }

    /// Eye-to-eye visibility (mirrors `EntityLiving.func_145_g`): raytrace
    /// between eye heights (pos + height*0.85) must be clear.
    fn can_entity_see(&self, id: EntityId, target: EntityId) -> bool {
        let (sp, sh) = match self.entities.get(id) {
            Some(e) => (e.body().pos, e.body().height as f64),
            None => return false,
        };
        let (tp, th) = match self.entities.get(target) {
            Some(e) => (e.body().pos, e.body().height as f64),
            None => return false,
        };
        let from = [sp[0], sp[1] + sh * 0.85, sp[2]];
        let to = [tp[0], tp[1] + th * 0.85, tp[2]];
        self.ray_trace_clear(from, to)
    }

    /// Mob target refresh (mirrors the head of `EntityMob::updateAI`):
    /// re-acquire every 5 ticks, drop out-of-`1.5x`-range targets, and
    /// re-check aggro every tick. One deliberate hardening: a missing/dead
    /// target row clears to `None` (C++ keeps the stale pointer but nulls
    /// the effective target, same observable behavior).
    fn refresh_mob_target(&mut self, id: EntityId, kind: MobKind) -> Option<EntityId> {
        let (mut timer, mut target) = match self.entities.get(id) {
            Some(Entity::Mob(m)) => (m.target_timer, m.target),
            _ => return None,
        };
        if target.map(|t| !self.target_alive(t)).unwrap_or(false) {
            target = None;
        }
        timer -= 1;
        if timer <= 0 {
            timer = 5;
            match self.acquire_target(id, kind) {
                Some(fresh) => target = Some(fresh),
                None => {
                    if let Some(cur) = target {
                        let max = CREATURE_TARGET_RANGE * 1.5;
                        let (sp, tp) = match (self.entities.get(id), self.entities.get(cur)) {
                            (Some(s), Some(t)) => (s.body().pos, t.body().pos),
                            _ => return None,
                        };
                        let (dx, dy, dz) = (tp[0] - sp[0], tp[1] - sp[1], tp[2] - sp[2]);
                        if dx * dx + dy * dy + dz * dz > max * max {
                            target = None;
                        }
                    }
                }
            }
        }
        if let Some(cur) = target {
            if !self.target_alive(cur) || !self.mob_aggro_ok(kind, id) {
                target = None;
            }
        }
        if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
            m.target_timer = timer;
            m.target = target;
        }
        target
    }

    /// Wander destination (mirrors `pickWanderDestination`) through the
    /// shared [`wander_pick`] kernel. The chunk map and the RNG are
    /// disjoint field borrows, so the closures share the exact selection
    /// flow.
    fn wander_destination(&mut self, rule: WeightRule, base: [i32; 3]) -> Option<[i32; 3]> {
        let rng = &mut self.rng;
        let chunks = &self.chunks;
        let mut next = |bound: i32| rng.next_int_bound(bound);
        let mut weight = |x: i32, y: i32, z: i32| match rule {
            WeightRule::Mob => {
                ai_mob_path_weight(World::block_light_in(chunks, x, y, z) as f32 / 15.0)
            }
            WeightRule::Animal => ai_animal_path_weight(
                World::block_id_in(chunks, x, y - 1, z) == GRASS_BLOCK_ID,
                World::block_light_in(chunks, x, y, z) as f32 / 15.0,
            ),
        };
        wander_pick(base, &mut next, &mut weight)
    }

    /// A* over the native chunk map through the shared [`find_path_native`]
    /// core.
    fn path_points(
        &self,
        start: (f64, f64, f64),
        target: (f64, f64, f64),
        width: f32,
        height: f32,
        max_dist: f32,
    ) -> Vec<[i32; 3]> {
        let chunks = &self.chunks;
        let is_liquid = |x: i32, y: i32, z: i32| World::material_in(chunks, x, y, z).is_liquid();
        let blocks = |x: i32, y: i32, z: i32| World::material_in(chunks, x, y, z).blocks_movement();
        let q = crate::pathfinder::BlockQueries {
            is_liquid: &is_liquid,
            blocks_movement: &blocks,
        };
        find_path_native(
            &q,
            [start.0, start.1, start.2],
            [target.0, target.1, target.2],
            [width, height],
            max_dist,
        )
        .into_iter()
        .map(|(x, y, z)| [x, y, z])
        .collect()
    }

    /// Path to a block (mirrors `getPathToBlock`: integer target shifted by
    /// half a block like the C++ `createEntityPathTo` int overload).
    pub(crate) fn path_block_points(&self, id: EntityId, dst: [i32; 3], max_dist: f32) -> Vec<[i32; 3]> {
        let (bb, width, height) = match self.entities.get(id) {
            Some(e) => (e.body().bounding_box, e.body().width, e.body().height),
            None => return Vec::new(),
        };
        self.path_points(
            (bb.min_x, bb.min_y, bb.min_z),
            (dst[0] as f64 + 0.5, dst[1] as f64 + 0.5, dst[2] as f64 + 0.5),
            width,
            height,
            max_dist,
        )
    }

    /// Path to an entity (mirrors the pointer `getPathToEntity` overload:
    /// target feet plus eye height, not the bounding-box floor).
    fn path_target_points(&self, id: EntityId, target: EntityId, max_dist: f32) -> Vec<[i32; 3]> {
        let (bb, width, height) = match self.entities.get(id) {
            Some(e) => (e.body().bounding_box, e.body().width, e.body().height),
            None => return Vec::new(),
        };
        let tp = match self.entities.get(target) {
            Some(t) => {
                let eye = Self::living_eye_height(t);
                [t.body().pos[0], t.body().pos[1] + eye, t.body().pos[2]]
            }
            None => return Vec::new(),
        };
        self.path_points((bb.min_x, bb.min_y, bb.min_z), (tp[0], tp[1], tp[2]), width, height, max_dist)
    }

    /// Owned AI snapshot of one mob/animal row.
    pub(crate) fn creature_snapshot(&self, id: EntityId) -> Option<CreatureSnap> {
        let (body, move_speed, path, path_index) = match self.entities.get(id) {
            Some(Entity::Mob(m)) => (
                &m.living.body,
                m.living.move_speed,
                m.path.clone(),
                m.path_index,
            ),
            Some(Entity::Animal(a)) => (
                &a.living.body,
                a.living.move_speed,
                a.path.clone(),
                a.path_index,
            ),
            _ => return None,
        };
        Some(CreatureSnap {
            pos: body.pos,
            min_y: body.bounding_box.min_y,
            width: body.width,
            height: body.height,
            yaw: body.yaw,
            pitch: body.pitch,
            collided_horiz: body.collided_horiz,
            move_speed,
            path,
            path_index,
        })
    }

    /// Write back navigation state (yaw/pitch/path/jump flag) to a
    /// mob/animal row.
    fn store_creature_nav(&mut self, id: EntityId, snap: &CreatureSnap, jumping: bool) {
        match self.entities.get_mut(id) {
            Some(Entity::Mob(m)) => {
                m.living.body.yaw = snap.yaw;
                m.living.body.pitch = snap.pitch;
                m.living.jumping = jumping;
                m.path = snap.path.clone();
                m.path_index = snap.path_index;
            }
            Some(Entity::Animal(a)) => {
                a.living.body.yaw = snap.yaw;
                a.living.body.pitch = snap.pitch;
                a.living.jumping = jumping;
                a.path = snap.path.clone();
                a.path_index = snap.path_index;
            }
            _ => {}
        }
    }

    /// Clear a mob target (animals never hold one; no-op for them).
    pub(crate) fn store_mob_target(&mut self, id: EntityId, target: Option<EntityId>) {
        if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
            m.target = target;
        }
    }

    /// Shared creature phases (mirrors `EntityCreature::updateAI` minus the
    /// attack hook): target validation, wander/re-path selection, and path
    /// following. The `canSee + attackEntityAt` call arrives with the attack
    /// slice; `isAttacking_` is a literal false because C++ sets it false
    /// and never raises it. Returns the effective target.
    #[allow(clippy::too_many_arguments)]
    fn creature_phases(
        &mut self,
        id: EntityId,
        mut target: Option<EntityId>,
        rule: WeightRule,
        mob_kind: Option<MobKind>,
        snap: &mut CreatureSnap,
        in_liquid: bool,
        strafe: &mut f32,
        forward: &mut f32,
        jumping: &mut bool,
    ) -> Option<EntityId> {
        // Phase 1: drop dead/missing targets like the base clear branch,
        // then let visible targets take the per-kind attack.
        if let Some(t) = target {
            if !self.target_alive(t) {
                target = None;
                self.store_mob_target(id, None);
            }
        }
        if let (Some(kind), Some(t)) = (mob_kind, target) {
            let (sp, tp, teye) = match (self.entities.get(id), self.entities.get(t)) {
                (Some(s), Some(te)) => (s.body().pos, te.body().pos, Self::living_eye_height(te)),
                _ => return target,
            };
            let (dx, dy, dz) = (tp[0] - sp[0], tp[1] - sp[1], tp[2] - sp[2]);
            let dist = sqrt_float((dx * dx + dy * dy + dz * dz) as f32);
            let self_eye = snap.height as f64 * 0.85;
            let from = [sp[0], sp[1] + self_eye, sp[2]];
            let to = [tp[0], tp[1] + teye, tp[2]];
            if self.ray_trace_clear(from, to) {
                target = self.mob_attack(id, kind, t, dist, snap);
            }
        }
        // Phase 2: wander when targetless (or when the re-path gate skips),
        // else refresh the chase path. Draw order and short-circuits mirror
        // C++ exactly.
        let had_path = !snap.path.is_empty();
        if target.is_none() || (had_path && self.rng.next_int_bound(20) != 0) {
            if (!had_path && self.rng.next_int_bound(80) == 0) || self.rng.next_int_bound(80) == 0
            {
                let base =
                    [floor_double(snap.pos[0]), floor_double(snap.min_y), floor_double(snap.pos[2])];
                if let Some(dst) = self.wander_destination(rule, base) {
                    snap.path = self.path_block_points(id, dst, 10.0);
                    snap.path_index = 0;
                }
            }
        } else if let Some(t) = target {
            snap.path = self.path_target_points(id, t, CREATURE_TARGET_RANGE as f32);
            snap.path_index = 0;
        }
        // Phase 3: follow the path (mirrors `followPath`).
        *strafe = 0.0;
        *forward = 0.0;
        *jumping = false;
        if !snap.path.is_empty() && self.rng.next_int_bound(100) != 0 {
            // Path-point screen position: integer point plus the truncated
            // `(int)(width + 1) * 0.5` offset, like `PathEntity`.
            let width_offset = ((snap.width + 1.0) as i32) as f64 * 0.5;
            let wide = (snap.width * 2.0f32) as f64;
            let threshold = wide * wide;
            let mut point: Option<[f64; 3]> = None;
            loop {
                if snap.path_index >= snap.path.len() {
                    snap.path.clear();
                    snap.path_index = 0;
                    break;
                }
                let pt = snap.path[snap.path_index];
                let px = pt[0] as f64 + width_offset;
                let py = pt[1] as f64;
                let pz = pt[2] as f64 + width_offset;
                let (dxh, dzh) = (px - snap.pos[0], pz - snap.pos[2]);
                if dxh * dxh + dzh * dzh >= threshold {
                    point = Some([px, py, pz]);
                    break;
                }
                snap.path_index += 1;
                if snap.path_index >= snap.path.len() {
                    snap.path.clear();
                    snap.path_index = 0;
                    break;
                }
            }
            if let Some([px, py, pz]) = point {
                let dx = px - snap.pos[0];
                let dz = pz - snap.pos[2];
                let dy = py - floor_double(snap.min_y) as f64;
                // `moveForward_` was just zeroed, so `forward_in` is 0.0
                // literally like C++.
                let (tdx, tdz) = match target.and_then(|t| self.entities.get(t)) {
                    Some(t) => (t.body().pos[0] - snap.pos[0], t.body().pos[2] - snap.pos[2]),
                    None => (0.0, 0.0),
                };
                let steer = steer_run(SteerIn {
                    delta: [dx, dy, dz],
                    cur_yaw: snap.yaw,
                    attacking: false,
                    has_target: target.is_some(),
                    target: [tdx, tdz],
                    forward_in: 0.0,
                });
                snap.yaw = steer.new_yaw;
                *strafe = steer.strafe;
                *forward = steer.forward;
                if steer.jump {
                    *jumping = true;
                }
            }
            // Face a chase target while closing in (30-degree turn).
            if let Some(t) = target {
                if let Some(te) = self.entities.get(t) {
                    let (dx, dz, dy) = {
                        let tb = te.body();
                        let self_eye = snap.height as f64 * 0.85;
                        let dy = match te {
                            Entity::Player(_)
                            | Entity::Mob(_)
                            | Entity::Animal(_) => {
                                let eye = Self::living_eye_height(te);
                                tb.pos[1] + eye - (snap.pos[1] + self_eye)
                            }
                            _ => {
                                (tb.bounding_box.min_y + tb.bounding_box.max_y) / 2.0
                                    - (snap.pos[1] + self_eye)
                            }
                        };
                        (tb.pos[0] - snap.pos[0], tb.pos[2] - snap.pos[2], dy)
                    };
                    let (ny, np) = face_run(dx, dz, dy, snap.yaw, snap.pitch, 30.0);
                    snap.yaw = ny;
                    snap.pitch = np;
                }
            }
        }
        // Jump over obstacles, paddle in liquid, then walk the path.
        if snap.collided_horiz {
            *jumping = true;
        }
        if self.rng.next_float() < 0.8 && in_liquid {
            *jumping = true;
        }
        if !snap.path.is_empty() {
            *forward = snap.move_speed;
        }
        target
    }

    /// Heading integration for one creature row through the shared
    /// [`living_heading_run`] core, with explicit borrows throughout.
    /// The ladder grip is read up front (the core consults it exactly
    /// once, before moving), and the pre-move fall distance mirrors the
    /// core's ladder branch below (`ladder -> fall = 0.0`): the mover
    /// syncs that exact value into the row ahead of `move_body`
    /// (mirrors C++ where the zero lands on the entity field ahead of
    /// `moveEntity` -> `updateFallState`).
    /// Returns the fall event distance when `onFall` must fire.
    fn move_creature_heading(&mut self, id: EntityId, strafe: f32, forward: f32) -> Option<f32> {
        let (jumping, on_ground, yaw, mut io) = match self.entities.get(id) {
            Some(Entity::Mob(m)) => (
                m.living.jumping,
                m.living.body.on_ground,
                m.living.body.yaw,
                HeadingIo {
                    motion_x: m.living.body.motion[0],
                    motion_y: m.living.body.motion[1],
                    motion_z: m.living.body.motion[2],
                    fall_distance: m.living.body.fall_distance,
                },
            ),
            Some(Entity::Animal(a)) => (
                a.living.jumping,
                a.living.body.on_ground,
                a.living.body.yaw,
                HeadingIo {
                    motion_x: a.living.body.motion[0],
                    motion_y: a.living.body.motion[1],
                    motion_z: a.living.body.motion[2],
                    fall_distance: a.living.body.fall_distance,
                },
            ),
            _ => return None,
        };
        let liquid = self.touching_liquid(id);
        let lava = self.touching_lava(id);
        let friction = self.ground_slipperiness(id);
        // Read once: `living_heading_run` consults the grip exactly once,
        // before any movement, so an up-front read sees identical state.
        let ladder_grip = self.ladder_for(id);
        // Mirror of the core's ladder branch: gripped fall resets to zero.
        let pre_move_fall = if ladder_grip { 0.0 } else { io.fall_distance };
        let mut fall_ev: Option<f32> = None;
        let mut mover = |dx: f64, dy: f64, dz: f64, fb: &mut MoveFeedback| {
            let w = &mut *self;
            if let Some(e) = w.entities.get_mut(id) {
                e.body_mut().fall_distance = pre_move_fall;
            }
            fall_ev = w.move_body(id, dx, dy, dz);
            if let Some(e) = w.entities.get(id) {
                fb.on_ground = e.body().on_ground;
                fb.collided_vert = e.body().collided_vert;
                fb.collided_horiz = e.body().collided_horiz;
                fb.pos_y = e.body().pos[1];
            }
            true
        };
        living_heading_run(strafe, forward, jumping, on_ground, yaw, &mut io, liquid, lava, friction, &mut || ladder_grip, &mut mover);
        // Only motion round-trips through `io` now: fall state already
        // lives in the row (synced pre-move, accumulated by `move_body`).
        match self.entities.get_mut(id) {
            Some(Entity::Mob(m)) => {
                m.living.body.motion = [io.motion_x, io.motion_y, io.motion_z];
            }
            Some(Entity::Animal(a)) => {
                a.living.body.motion = [io.motion_x, io.motion_y, io.motion_z];
            }
            _ => {}
        }
        fall_ev
    }

    /// Shove live neighbors apart (mirrors the tail of the mob/animal
    /// ticks: `getEntitiesWithinAABBExcludingEntity` skips the dead, then
    /// `applyEntityCollision` pushes both sides). Id-sorted for
    /// determinism.
    fn push_neighbors(&mut self, id: EntityId) {
        let (mask, self_pos) = match self.entities.get(id) {
            Some(e) => (e.body().bounding_box.expand(0.2, 0.0, 0.2), e.body().pos),
            None => return,
        };
        let self_pushable = !self.entities.get(id).map(|e| e.body().dead).unwrap_or(true);
        let mut others: Vec<(EntityId, f64, f64)> = Vec::new();
        for oid in self.entities.alive_ids() {
            if oid == id {
                continue;
            }
            if let Some(o) = self.entities.get(oid) {
                if mask.intersects_with(&o.body().bounding_box) {
                    others.push((oid, o.body().pos[0], o.body().pos[2]));
                }
            }
        }
        others.sort_by_key(|(oid, _, _)| *oid);
        for (oid, ox, oz) in others {
            let (sx, sz) = match self.entities.get(id) {
                Some(e) => (e.body().pos[0], e.body().pos[2]),
                None => (self_pos[0], self_pos[2]),
            };
            let Some(push) = entity_push(ox, oz, sx, sz, true, self_pushable) else {
                continue;
            };
            if let Some(o) = self.entities.get_mut(oid) {
                o.body_mut().motion[0] += push.dvx1;
                o.body_mut().motion[2] += push.dvz1;
            }
            if let Some(e) = self.entities.get_mut(id) {
                e.body_mut().motion[0] += push.dvx2;
                e.body_mut().motion[2] += push.dvz2;
            }
        }
    }

    /// Mob daylight ignition (mirrors `checkDaylightBurn`): burning kinds
    /// in daytime with bright sky access catch a 300-tick burn. Note the
    /// two heights: sky access reads `floor(posY)`, brightness reads
    /// `floor(minY)` like C++.
    fn check_daylight_burn(&mut self, id: EntityId, kind: MobKind) {
        if !mob_burns_in_daylight(kind) || !self.is_daytime() {
            return;
        }
        let (px, py, pz, min_y) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2], e.body().bounding_box.min_y),
            None => return,
        };
        let (bx, by, bz) = (floor_double(px), floor_double(py), floor_double(pz));
        let brightness = self.brightness(bx, floor_double(min_y), bz);
        if brightness > 0.5 && self.can_see_sky(bx, by, bz) && self.rng.next_float() * 30.0 < (brightness - 0.4) * 2.0
        {
            if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
                m.burn_ticks = 300;
                m.living.body.fire = m.living.body.fire.max(300);
            }
        }
    }

    /// Mob AI update (mirrors `EntityMob::updateAI`): target refresh, the
    /// shared creature phases, the chase-speed bonus, and the liquid paddle
    /// gate. Returns the (strafe, forward) pair for the heading move.
    fn update_mob_ai(&mut self, id: EntityId, kind: MobKind, in_liquid: bool) -> (f32, f32) {
        let mut strafe = 0.0f32;
        let mut forward = 0.0f32;
        let mut jumping = false;
        let target = self.refresh_mob_target(id, kind);
        let mut snap = match self.creature_snapshot(id) {
            Some(s) => s,
            None => return (0.0, 0.0),
        };
        let target =
            self.creature_phases(id, target, WeightRule::Mob, Some(kind), &mut snap, in_liquid, &mut strafe, &mut forward, &mut jumping);
        // Chase bonus: full speed plus 20% past attack reach + 1.
        if let Some(t) = target {
            if let (Some(s), Some(te)) = (self.entities.get(id), self.entities.get(t)) {
                let (sp, tp) = (s.body().pos, te.body().pos);
                let (dx, dy, dz) = (tp[0] - sp[0], tp[1] - sp[1], tp[2] - sp[2]);
                let dist = sqrt_float((dx * dx + dy * dy + dz * dz) as f32);
                forward = chase_speed(snap.move_speed, dist, mob_attack_reach(kind));
            }
        }
        if in_liquid && self.rng.next_int_bound(5) != 0 {
            jumping = true;
        }
        self.store_creature_nav(id, &snap, jumping);
        (strafe, forward)
    }

    /// Animal AI update (mirrors `EntityCreature::updateAI` for animals:
    /// targetless shared phases, no chase bonus, no paddle gate).
    fn update_animal_ai(&mut self, id: EntityId, in_liquid: bool) -> (f32, f32) {
        let mut strafe = 0.0f32;
        let mut forward = 0.0f32;
        let mut jumping = false;
        let mut snap = match self.creature_snapshot(id) {
            Some(s) => s,
            None => return (0.0, 0.0),
        };
        self.creature_phases(id, None, WeightRule::Animal, None, &mut snap, in_liquid, &mut strafe, &mut forward, &mut jumping);
        self.store_creature_nav(id, &snap, jumping);
        (strafe, forward)
    }

    /// Despawn (mirrors `EntityLiving.func_152_d`: ++age; dead past 128
    /// blocks from the nearest player, or past age 600 + 1/800 roll past
    /// 32 blocks). Nearest-player search covers all players (the -1.0D
    /// radius means "any"). Returns true when the row died here.
    fn despawn_check(&mut self, id: EntityId) -> bool {
        let (px, py, pz) = match self.entities.get(id) {
            Some(e) if !e.body().dead => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            _ => return true,
        };
        // Rebuild the player-position cache once per tick (players are few;
        // scanning them per mob is cheap, allocating per mob is not).
        if self.player_pos_cache.0 != self.time {
            let mut pp = Vec::new();
            for oid in self.entities.alive_ids() {
                if let Some(Entity::Player(p)) = self.entities.get(oid) {
                    if !p.living.body.dead {
                        pp.push(p.living.body.pos);
                    }
                }
            }
            self.player_pos_cache = (self.time, pp);
        }
        let mut best: Option<f64> = None;
        for q in self.player_pos_cache.1.clone() {
            let (dx, dy, dz) = (q[0] - px, q[1] - py, q[2] - pz);
            let d2 = dx * dx + dy * dy + dz * dz;
            best = Some(best.map_or(d2, |b: f64| b.min(d2)));
        }
        let Some(d2) = best else {
            // No live players: still age the row, never despawn.
            match self.entities.get_mut(id) {
                Some(Entity::Mob(m)) => m.age += 1,
                Some(Entity::Animal(a)) => a.age += 1,
                _ => {}
            }
            return false;
        };
        if d2 > 16384.0 {
            if let Some(e) = self.entities.get_mut(id) {
                e.body_mut().dead = true;
            }
            return true;
        }
        // Java EntityMobs: brightness > 0.5 ages +2 extra (mobs in light
        // despawn faster). Compute before the &mut borrow below.
        let bright_extra = {
            match self.entities.get(id) {
                Some(Entity::Mob(m)) => {
                    let b = &m.living.body;
                    let bx = floor_double(b.pos[0]);
                    let by = floor_double(b.bounding_box.min_y);
                    let bz = floor_double(b.pos[2]);
                    if self.brightness(bx, by, bz) > 0.5 { 2 } else { 0 }
                }
                _ => 0,
            }
        };
        let age = match self.entities.get_mut(id) {
            Some(Entity::Mob(m)) => {
                m.age += 1 + bright_extra;
                m.age
            }
            Some(Entity::Animal(a)) => {
                a.age += 1;
                a.age
            }
            _ => return true,
        };
        if age > 600 && self.rng.next_int_bound(800) == 0 {
            if d2 < 1024.0 {
                match self.entities.get_mut(id) {
                    Some(Entity::Mob(m)) => m.age = 0,
                    Some(Entity::Animal(a)) => a.age = 0,
                    _ => {}
                }
            } else {
                if let Some(e) = self.entities.get_mut(id) {
                    e.body_mut().dead = true;
                }
                return true;
            }
        }
        false
    }

    /// Mob tick (mirrors `EntityMob::tick`): living maintenance, cooldown
    /// and burn schedule, daylight ignition, AI, heading move with fall
    /// damage, and neighbor shoves. Like C++, the AI and move still run
    /// when burn damage kills mid-tick.
    pub fn tick_mob(&mut self, id: EntityId) {
        // Peaceful (difficulty 0): mobs die instead of ticking
        // (Java EntityMobs.onUpdate: monstersEnabled == 0 -> dead).
        if self.difficulty == 0 {
            if let Some(e) = self.entities.get_mut(id) {
                e.body_mut().dead = true;
            }
            return;
        }
        if self.despawn_check(id) {
            return;
        }
        self.tick_living(id);
        let kind = match self.entities.get(id) {
            Some(Entity::Mob(m)) if !m.living.body.dead => m.kind,
            _ => return,
        };
        let mut burn_hit = false;
        if let Some(Entity::Mob(m)) = self.entities.get_mut(id) {
            if m.attack_cooldown > 0 {
                m.attack_cooldown -= 1;
            }
            if m.burn_ticks <= 0 && m.living.body.fire <= 20 {
                m.living.body.fire = 0;
            }
            if m.burn_ticks > 0 {
                m.living.body.fire = m.living.body.fire.max(20);
                if m.burn_ticks % 20 == 0 {
                    burn_hit = true;
                }
                m.burn_ticks -= 1;
            }
        }
        if burn_hit {
            self.attack_living(id, 1, None);
        }
        self.check_daylight_burn(id, kind);
        let in_liquid = self.touching_liquid(id);
        let (strafe, forward) = self.update_mob_ai(id, kind, in_liquid);
        if let Some(dist) = self.move_creature_heading(id, strafe, forward) {
            let damage = living_fall_damage(dist);
            if damage > 0 {
                self.attack_living(id, damage, None);
            }
        }
        self.push_neighbors(id);
    }

    /// Animal tick (mirrors `EntityAnimals::tick`): living maintenance, the
    /// shared creature AI, heading move with fall damage (chickens override
    /// `onFall` to a no-op), chicken extras, and neighbor shoves.
    pub fn tick_animal(&mut self, id: EntityId) {
        if self.despawn_check(id) {
            return;
        }
        self.tick_living(id);
        let kind = match self.entities.get(id) {
            Some(Entity::Animal(a)) if !a.living.body.dead => a.kind,
            _ => return,
        };
        let in_liquid = self.touching_liquid(id);
        let (strafe, forward) = self.update_animal_ai(id, in_liquid);
        if let Some(dist) = self.move_creature_heading(id, strafe, forward) {
            if kind != AnimalKind::Chicken {
                let damage = living_fall_damage(dist);
                if damage > 0 {
                    self.attack_living(id, damage, None);
                }
            }
        }
        if kind == AnimalKind::Chicken {
            self.chicken_extra(id);
        }
        self.push_neighbors(id);
    }

    /// Chicken extras (mirrors `tickExtra`): slow sinking plus the egg
    /// clock. Runs after the move like C++, so the damping shapes the next
    /// tick's motion.
    fn chicken_extra(&mut self, id: EntityId) {
        if let Some(Entity::Animal(a)) = self.entities.get_mut(id) {
            if a.living.body.motion[1] < 0.0 && !a.living.body.on_ground {
                a.living.body.motion[1] *= 0.6;
            }
            a.egg_timer -= 1;
        }
        let lay = matches!(self.entities.get(id), Some(Entity::Animal(a)) if a.egg_timer <= 0);
        if !lay {
            return;
        }
        let (px, py, pz) = match self.entities.get(id) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            None => return,
        };
        self.spawn_item_entity(EGG_ITEM_ID, 1, 0, px, py, pz);
        let roll = self.rng.next_int_bound(6000);
        if let Some(Entity::Animal(a)) = self.entities.get_mut(id) {
            a.egg_timer = 6000 + roll;
        }
    }
}
