//! Damage pipeline, death/drops and living/player ticks on [`World`].
//! Split out of `world.rs`; behavior unchanged.

use crate::entity::table::{AnimalKind, Entity, EntityId, LivingBody, MobKind};
use crate::material::Material;
use crate::math_helper::floor_double;
use crate::world::World;

impl World {
    pub(crate) fn living_eye_height(e: &Entity) -> f64 {
        match e {
            Entity::Player(_) => 1.62,
            _ => e.body().height as f64 * 0.85,
        }
    }

    /// Crate-visible eye height for sessions.
    pub(crate) fn living_eye_height_pub(e: &Entity) -> f64 {
        Self::living_eye_height(e)
    }

    /// Crate-visible chunk dirty flag for tile updates.
    pub(crate) fn mark_chunk_modified(&mut self, cx: i32, cz: i32) {
        if let Some(c) = self.chunks.get_mut(&(cx, cz)) {
            c.is_modified = true;
        }
    }

    /// Damage pipeline on a native living row (mirrors
    /// `EntityLiving::attackEntityFrom` + `onDeath` with mob/animal drops).
    /// `attacker` supplies knockback direction; `None` skips it. Players
    /// route through respawn immunity, difficulty scaling, and armor like
    /// `EntityPlayerMP::attackEntityFrom` (death message and the health
    /// packet are the network slice's).
    pub fn attack_living(&mut self, id: EntityId, amount: i32, attacker: Option<EntityId>) {
        use crate::entity::living::{AttackInput, living_attack_run};
        self.sheep_shear(id, attacker);
        let amount = match self.entities.get(id) {
            Some(Entity::Player(p)) if p.respawn_ticks > 0 => return,
            Some(Entity::Player(_)) => match self.player_armored_damage(id, amount, attacker) {
                Some(scaled) => scaled,
                None => return,
            },
            _ => amount,
        };
        let input = match self.entities.get(id) {
            Some(Entity::Mob(_)) | Some(Entity::Animal(_)) | Some(Entity::Player(_)) => {
                let (l, px, pz) = match self.entities.get(id) {
                    Some(Entity::Mob(m)) => (&m.living, m.living.body.pos[0], m.living.body.pos[2]),
                    Some(Entity::Animal(a)) => {
                        (&a.living, a.living.body.pos[0], a.living.body.pos[2])
                    }
                    Some(Entity::Player(p)) => {
                        (&p.living, p.living.body.pos[0], p.living.body.pos[2])
                    }
                    _ => unreachable!(),
                };
                let (ax, az, has) = match attacker.and_then(|a| self.entities.get(a)) {
                    Some(a) => (a.body().pos[0], a.body().pos[2], true),
                    None => (0.0, 0.0, false),
                };
                AttackInput {
                    health: l.health,
                    hurt_resist: l.hurt_resist,
                    max_hurt_resist: l.max_hurt_resist,
                    last_damage: l.last_damage,
                    hurt_time_in: l.hurt_time,
                    attack_time_in: l.attack_time,
                    dead: l.body.dead,
                    amount,
                    has_attacker: has,
                    self_x: px,
                    self_z: pz,
                    atk_x: ax,
                    atk_z: az,
                    motion_x: l.body.motion[0],
                    motion_y: l.body.motion[1],
                    motion_z: l.body.motion[2],
                }
            }
            _ => return,
        };
        let result = {
            let rng = &mut self.rng;
            living_attack_run(&input, &mut || rng.next_double())
        };
        let r = match result {
            Some(r) => r,
            None => return,
        };
        let died = r.died;
        match self.entities.get_mut(id) {
            Some(Entity::Mob(m)) => {
                m.living.health = r.health;
                m.living.last_damage = r.last_damage;
                m.living.hurt_resist = r.hurt_resist;
                m.living.hurt_time = r.hurt_time;
                m.living.attack_time = r.attack_time;
                if r.knocked {
                    m.living.body.motion = [r.kmx, r.kmy, r.kmz];
                }
            }
            Some(Entity::Animal(a)) => {
                a.living.health = r.health;
                a.living.last_damage = r.last_damage;
                a.living.hurt_resist = r.hurt_resist;
                a.living.hurt_time = r.hurt_time;
                a.living.attack_time = r.attack_time;
                if r.knocked {
                    a.living.body.motion = [r.kmx, r.kmy, r.kmz];
                }
            }
            Some(Entity::Player(p)) => {
                p.living.health = r.health;
                p.living.last_damage = r.last_damage;
                p.living.hurt_resist = r.hurt_resist;
                p.living.hurt_time = r.hurt_time;
                p.living.attack_time = r.attack_time;
                if r.knocked {
                    p.living.body.motion = [r.kmx, r.kmy, r.kmz];
                }
            }
            _ => return,
        }
        if r.send_status {
            self.status_events.push((id, 2));
        }
        if r.knocked {
            self.velocity_events.push((id, [r.kmx, r.kmy, r.kmz]));
        }
        if died {
            self.kill_living_by(id, attacker);
        }
    }

    /// Sheep shear (mirrors `EntitySheep::attackEntityFrom`): a living
    /// attacker scares 1..3 wool off first, independent of the damage
    /// pipeline that follows. Wool drops match the C++ shape exactly
    /// (spawn height, per-axis jitter, default pickup delay).
    fn sheep_shear(&mut self, id: EntityId, attacker: Option<EntityId>) {
        let (px, py, pz, h) = match self.entities.get(id) {
            Some(Entity::Animal(a)) if a.kind == AnimalKind::Sheep && !a.sheared => (
                a.living.body.pos[0],
                a.living.body.pos[1],
                a.living.body.pos[2],
                a.living.body.height,
            ),
            _ => return,
        };
        let living_attacker = attacker
            .and_then(|x| self.entities.get(x))
            .map(|e| matches!(e, Entity::Mob(_) | Entity::Animal(_) | Entity::Player(_)))
            .unwrap_or(false);
        if !living_attacker {
            return;
        }
        if let Some(Entity::Animal(a)) = self.entities.get_mut(id) {
            a.sheared = true;
        }
        let count = 1 + self.rng.next_int_bound(3);
        for _ in 0..count {
            let draws = [
                self.rng.next_double(),
                self.rng.next_double(),
                self.rng.next_double(),
                self.rng.next_double(),
                self.rng.next_double(),
            ];
            let wid = self.spawn_item_entity(35, 1, 0, px, py + h as f64 * 0.75, pz);
            if let Some(Entity::Item(e)) = self.entities.get_mut(wid) {
                e.body.motion[0] += (draws[1] - draws[2]) * 0.1;
                e.body.motion[1] += draws[0] * 0.05;
                e.body.motion[2] += (draws[3] - draws[4]) * 0.1;
            }
        }
    }

    /// Death: dismount both sides, spawn kind drops, mark dead (mirrors
    /// `EntityLiving::onDeath` + mob/animal `onDeath`). Players dismount
    /// and die with no drops yet (inventory scatter is the player slice).
    pub fn kill_living(&mut self, id: EntityId) {
        self.kill_living_by(id, None);
    }

    /// Kill with a known attacker (for creeper-record drops: killer must be
    /// a skeleton, EntityCreeper:77). `None` keeps the old behavior.
    pub fn kill_living_by(&mut self, id: EntityId, attacker: Option<EntityId>) {
        let (px, py, pz) = match self.entities.get(id) {
            Some(Entity::Mob(m)) => (m.living.body.pos[0], m.living.body.pos[1], m.living.body.pos[2]),
            Some(Entity::Animal(a)) => (a.living.body.pos[0], a.living.body.pos[1], a.living.body.pos[2]),
            Some(Entity::Player(p)) => (p.living.body.pos[0], p.living.body.pos[1], p.living.body.pos[2]),
            _ => return,
        };
        // Dismount rider and vehicle.
        let (riding, ridden_by) = match self.entities.get(id) {
            Some(e) => (e.body().riding, e.body().ridden_by),
            None => return,
        };
        if riding >= 0 {
            self.entities.mount(id, None);
        }
        if ridden_by >= 0 {
            self.entities.mount(ridden_by, None);
        }
        // Kind drops for mobs/animals (counts mirror the C++ getDropCount
        // formulas); players scatter their inventory instead.
        if matches!(self.entities.get(id), Some(Entity::Mob(_)) | Some(Entity::Animal(_))) {
            let killer_is_skeleton = attacker
                .and_then(|a| self.entities.get(a))
                .map(|e| matches!(e, Entity::Mob(m) if m.kind == MobKind::Skeleton))
                .unwrap_or(false);
            let (drop_id, drop_count) = self.living_drops(id);
            for _ in 0..drop_count {
                self.spawn_item_entity(drop_id, 1, 0, px, py, pz);
            }
            // Creeper killed by skeleton drops a record (13/14).
            if killer_is_skeleton {
                if let Some(Entity::Mob(m)) = self.entities.get(id) {
                    if m.kind == MobKind::Creeper {
                        let record = if self.rng.next_int_bound(2) == 0 { 2256 } else { 2257 };
                        self.spawn_item_entity(record, 1, 0, px, py, pz);
                    }
                }
            }
        }
        if matches!(self.entities.get(id), Some(Entity::Player(_))) {
            self.scatter_player_inventory(id, px, py, pz);
        }
        if let Some(e) = self.entities.get_mut(id) {
            e.body_mut().dead = true;
        }
        self.death_events.push(id);
    }

    /// Player damage scaling (mirrors `EntityPlayerMP::attackEntityFrom`
    /// minus messaging and packets): difficulty scaling plus armor
    /// absorption with carry, damaging worn armor on the way. Returns
    /// `None` when the hit is fully absorbed.
    fn player_armored_damage(
        &mut self,
        id: EntityId,
        amount: i32,
        attacker: Option<EntityId>,
    ) -> Option<i32> {
        use crate::inventory::ItemStack;
        use crate::player::combat::combat_calculate_damage;
        use crate::player::inventory::{inventory_calc_armor, inventory_damage_armor};
        let attacker_is_player = attacker
            .and_then(|a| self.entities.get(a))
            .map(|e| matches!(e, Entity::Player(_)))
            .unwrap_or(false);
        // Vanilla scales only mob/arrow hits (EntityPlayer:197-209).
        // Environmental damage (fall/drown/fire/cactus, attacker=None) must
        // NOT scale — otherwise peaceful zeroes falls and easy nerfs them.
        let skip_difficulty_scale = attacker.is_none() || attacker_is_player;
        let mut tmp = [ItemStack::empty(); 4];
        let carry = match self.entities.get(id) {
            Some(Entity::Player(p)) => {
                for (i, slot) in p.inventory.armor.iter().enumerate() {
                    tmp[i] = slot.unwrap_or(tmp[i]);
                }
                p.armor_carry
            }
            _ => return Some(amount),
        };
        let res = combat_calculate_damage(
            amount,
            skip_difficulty_scale,
            self.difficulty,
            inventory_calc_armor(&tmp),
            carry,
        );
        if res.scaled_damage <= 0 {
            return None;
        }
        inventory_damage_armor(&mut tmp, res.scaled_damage);
        if let Some(Entity::Player(p)) = self.entities.get_mut(id) {
            p.armor_carry = res.new_armor_damage_carry;
            for (i, slot) in p.inventory.armor.iter_mut().enumerate() {
                *slot = if !tmp[i].is_empty() {
                    Some(tmp[i])
                } else {
                    None
                };
            }
        }
        Some(res.damage_after_armor)
    }

    /// Death scatter (mirrors `EntityPlayerMP::onDeath` drops): every
    /// non-empty main/armor/crafting stack becomes an item entity at the
    /// player's feet (offset by 0.5) with drop velocity (3 world-RNG draws
    /// per stack) and a 40-tick pickup delay; all banks clear.
    ///
    /// The inventory-resend packet is the network slice's.
    fn scatter_player_inventory(&mut self, id: EntityId, px: f64, py: f64, pz: f64) {
        use crate::entity::player::player_drop_velocity;
        let stacks: Vec<crate::inventory::ItemStack> = match self.entities.get(id) {
            Some(Entity::Player(p)) => p
                .inventory
                .main
                .iter()
                .chain(p.inventory.armor.iter())
                .chain(p.inventory.crafting.iter())
                .filter_map(|s| *s)
                .collect(),
            _ => return,
        };
        if let Some(Entity::Player(p)) = self.entities.get_mut(id) {
            p.inventory.main = [None; 36];
            p.inventory.armor = [None; 4];
            p.inventory.crafting = [None; 4];
        }
        for s in stacks {
            if s.is_empty() {
                continue;
            }
            let (ra, rb, rc) =
                (self.rng.next_double(), self.rng.next_double(), self.rng.next_double());
            let v = player_drop_velocity(ra, rb, rc);
            let eid = self.spawn_item_entity(s.item_id, s.count, s.damage, px, py + 0.5, pz);
            if let Some(Entity::Item(e)) = self.entities.get_mut(eid) {
                e.body.motion = [v.mx, v.my, v.mz];
                e.pickup_delay = 40;
            }
        }
    }

    /// Pick up a stack into main inventory (mirrors
    /// `addItemStackToInventory` acceptance: empty and out-of-range ids
    /// refuse). Returns the leftover count like the C++ remainder write.
    pub fn player_add_item(
        &mut self,
        id: EntityId,
        mut stack: crate::inventory::ItemStack,
    ) -> i32 {
        use crate::inventory::ItemStack;
        use crate::player::inventory::inventory_add_item;
        if stack.count <= 0 {
            return 0;
        }
        if stack.item_id <= 0 || stack.item_id >= 32000 {
            return stack.count;
        }
        match self.entities.get_mut(id) {
            Some(Entity::Player(p)) => {
                let mut tmp = [ItemStack::empty(); 36];
                for (i, slot) in p.inventory.main.iter().enumerate() {
                    tmp[i] = slot.unwrap_or(tmp[i]);
                }
                let rem = inventory_add_item(&mut tmp, &mut stack, 64);
                for (i, slot) in p.inventory.main.iter_mut().enumerate() {
                    *slot = if !tmp[i].is_empty() {
                        Some(tmp[i])
                    } else {
                        None
                    };
                }
                rem
            }
            _ => stack.count,
        }
    }

    /// Held stack (mirrors `getCurrentItem`).
    pub fn player_held(&self, id: EntityId) -> Option<crate::inventory::ItemStack> {
        match self.entities.get(id) {
            Some(Entity::Player(p)) => p.inventory.held(),
            _ => None,
        }
    }

    fn living_drops(&mut self, id: EntityId) -> (i32, i32) {
        // Java EntityLiving.onDeath:388: nextInt(3) = 0..2 of getDropItemId
        // for EVERY mob/animal. Sheep has no getDropItemId (wool comes only
        // from the attack-shear), so death drops nothing.
        match self.entities.get(id) {
            Some(Entity::Mob(m)) => match m.kind {
                crate::entity::table::MobKind::Spider => (287, self.rng.next_int_bound(3)),
                crate::entity::table::MobKind::Zombie => (288, self.rng.next_int_bound(3)),
                crate::entity::table::MobKind::Skeleton => (262, self.rng.next_int_bound(3)),
                crate::entity::table::MobKind::Creeper => (289, self.rng.next_int_bound(3)),
            },
            Some(Entity::Animal(a)) => match a.kind {
                crate::entity::table::AnimalKind::Sheep => (0, 0),
                crate::entity::table::AnimalKind::Pig => (319, self.rng.next_int_bound(3)),
                crate::entity::table::AnimalKind::Chicken => (288, self.rng.next_int_bound(3)),
                crate::entity::table::AnimalKind::Cow => (334, self.rng.next_int_bound(3)),
            },
            _ => (0, 0),
        }
    }

    /// Shared eye-cell sample for [`World::tick_living`].
    fn living_sample(
        &self,
        l: &LivingBody,
        eye: f64,
    ) -> (bool, bool, bool, i32, i32, i32, i32) {
        let ex = l.body.pos[0].floor() as i32;
        let ey = eye.floor() as i32;
        let ez = l.body.pos[2].floor() as i32;
        (
            !l.body.dead,
            self.is_solid(ex, ey, ez),
            self.material_at(ex, ey, ez) == Material::WATER,
            l.body.air,
            l.hurt_time,
            l.attack_time,
            l.hurt_resist,
        )
    }

    /// Per-tick living maintenance (mirrors `EntityLiving::tick`).
    pub fn tick_living(&mut self, id: EntityId) {
        self.entities.tick_base(id);
        // Fire decay (Entity.onUpdate: fire-- each tick, 1 damage per 20
        // ticks while burning, extinguished in water). Water only, like
        // vanilla `handleWaterMovement` (Entity.java:446): lava must NOT
        // extinguish — it is the igniter. (`is_liquid` covers both; the
        // old code cleared `fire` inside lava, so lava dealt contact
        // damage but never burned.)
        let in_water_for_fire = {
            match self.entities.get(id) {
                Some(Entity::Mob(m)) => {
                    let b = &m.living.body;
                    let (fx, fy, fz) = (
                        floor_double(b.pos[0]),
                        floor_double(b.bounding_box.min_y),
                        floor_double(b.pos[2]),
                    );
                    self.material_at(fx, fy, fz) == Material::WATER
                        || self.material_at(fx, fy + 1, fz) == Material::WATER
                }
                Some(Entity::Animal(a)) => {
                    let b = &a.living.body;
                    let (fx, fy, fz) = (
                        floor_double(b.pos[0]),
                        floor_double(b.bounding_box.min_y),
                        floor_double(b.pos[2]),
                    );
                    self.material_at(fx, fy, fz) == Material::WATER
                        || self.material_at(fx, fy + 1, fz) == Material::WATER
                }
                Some(Entity::Player(p)) => {
                    let b = &p.living.body;
                    let (fx, fy, fz) = (
                        floor_double(b.pos[0]),
                        floor_double(b.bounding_box.min_y),
                        floor_double(b.pos[2]),
                    );
                    self.material_at(fx, fy, fz) == Material::WATER
                        || self.material_at(fx, fy + 1, fz) == Material::WATER
                }
                _ => false,
            }
        };
        let fire_damage = match self.entities.get_mut(id) {
            Some(Entity::Mob(m)) => {
                if in_water_for_fire {
                    m.living.body.fire = 0;
                    m.burn_ticks = 0;
                    false
                } else if m.living.body.fire > 0 {
                    m.living.body.fire -= 1;
                    // Damage every 20 ticks of burn (300 → ~15 hits), unless
                    // `burn_ticks` in `tick_mob` is already managing the burn schedule.
                    m.burn_ticks <= 0 && m.living.body.fire % 20 == 0 && m.living.body.fire > 0
                } else {
                    false
                }
            }
            Some(Entity::Animal(a)) => {
                if in_water_for_fire {
                    a.living.body.fire = 0;
                    false
                } else if a.living.body.fire > 0 {
                    a.living.body.fire -= 1;
                    a.living.body.fire % 20 == 0 && a.living.body.fire > 0
                } else {
                    false
                }
            }
            Some(Entity::Player(p)) => {
                if in_water_for_fire {
                    p.living.body.fire = 0;
                    false
                } else if p.living.body.fire > 0 {
                    p.living.body.fire -= 1;
                    p.living.body.fire % 20 == 0 && p.living.body.fire > 0
                } else {
                    false
                }
            }
            _ => false,
        };
        if fire_damage {
            self.attack_living(id, 1, None);
        }
        let (alive, opaque, water, air, hurt, attack, resist) = match self.entities.get(id) {
            Some(Entity::Mob(m)) => {
                let l = &m.living;
                let eye = l.body.pos[1] + l.body.height as f64 * 0.85;
                Self::living_sample(self, l, eye)
            }
            Some(Entity::Animal(a)) => {
                let l = &a.living;
                let eye = l.body.pos[1] + l.body.height as f64 * 0.85;
                Self::living_sample(self, l, eye)
            }
            Some(Entity::Player(p)) => {
                let l = &p.living;
                Self::living_sample(self, l, l.body.pos[1] + 1.62)
            }
            _ => return,
        };
        let t = crate::entity::living::living_tick(alive, opaque, water, air, hurt, attack, resist);
        match self.entities.get_mut(id) {
            Some(Entity::Mob(m)) => {
                m.living.body.air = t.air;
                m.living.hurt_time = t.hurt_time;
                m.living.attack_time = t.attack_time;
                m.living.hurt_resist = t.hurt_resist;
            }
            Some(Entity::Animal(a)) => {
                a.living.body.air = t.air;
                a.living.hurt_time = t.hurt_time;
                a.living.attack_time = t.attack_time;
                a.living.hurt_resist = t.hurt_resist;
            }
            Some(Entity::Player(p)) => {
                p.living.body.air = t.air;
                p.living.hurt_time = t.hurt_time;
                p.living.attack_time = t.attack_time;
                p.living.hurt_resist = t.hurt_resist;
            }
            _ => return,
        }
        if t.suffocate {
            self.attack_living(id, 1, None);
        }
        if t.drown {
            self.attack_living(id, 2, None);
        }
    }

    /// Player tick (mirrors `EntityPlayerMP::tick` minus digging and arm
    /// swing: living maintenance plus respawn-immunity decay).
    pub fn tick_player(&mut self, id: EntityId) {
        self.tick_living(id);
        if let Some(Entity::Player(p)) = self.entities.get_mut(id) {
            if p.respawn_ticks > 0 {
                p.respawn_ticks -= 1;
            }
        }
    }
}
