//! Block behavior (mirrors Java `Block*`).
//!
//! Every per-block event (`onBlockAdded`, `onNeighborBlockChange`,
//! `updateTick`, `canBlockStay`, drops) lives here. World access is a
//! direct `&mut World` borrow, so draw sequences and check order are
//! preserved exactly.
//!
//! Parameter notes:
//! - positions travel as [`BlockPos`], drops as [`DropSpec`]: no more
//!   `(x, y, z)` / `(item, count, damage)` triples.
//! - block ids the caller already knows (`leaves_id`, `crop_id`,
//!   `is_lava`) are passed in instead of re-reading the registry.
//! - the leaves recursion guard is a plain `&mut i32` counter.
//! - sapling growth returns an action; tree generation itself runs through
//!   the world accessor path (see `world::blocks`).

use super::pos::{BlockPos, DropSpec};
use crate::entity::table::{Body, Entity};
use crate::material::Material;
use crate::world::{has_collision_box, has_collision_id, World};

pub const PLANT_GROWTH_STAGE_MAX: u8 = 15;
pub const LEAVES_DECAY_GUARD_MAX: i32 = 100;

/// Sapling tick outcome. `GrowTree` carries the `World::rand()` draw; the
/// caller runs tree generation for it.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaplingAction {
    None = 0,
    GrowTree = 1,
}

#[derive(Clone, Copy, Debug)]
pub struct TickAction {
    pub kind: u8,
    pub seed: u64,
}

// ---- internal query/update helpers (all position-based) ----

fn q_id(w: &World, pos: BlockPos) -> u8 {
    w.id_at(pos)
}
/// Neighbor scans that must not force chunk loads (mirrors NoChunkLoad calls).
fn q_id_nc(w: &World, pos: BlockPos) -> u8 {
    // Same map: the native world never force-loads chunks.
    w.id_at(pos)
}
fn q_meta(w: &World, pos: BlockPos) -> u8 {
    w.meta_at(pos)
}
fn q_light(w: &World, pos: BlockPos) -> i32 {
    w.light_at(pos) as i32
}
fn q_sky(w: &World, pos: BlockPos) -> bool {
    w.sky_at(pos)
}
fn q_attach_world(w: &World, pos: BlockPos) -> bool {
    w.attach_at(pos)
}
fn q_attach_torch(w: &World, pos: BlockPos) -> bool {
    // Solid material plus collidable (everything but fluids).
    if w.id_at(pos) == 0 {
        return false;
    }
    let m = w.material_at_pos(pos);
    m.is_solid() && !m.is_liquid()
}
fn q_solid(w: &World, pos: BlockPos) -> bool {
    // Material-solid like blockTickIsSolid (NOT the id list).
    w.id_at(pos) != 0 && w.material_at_pos(pos).is_solid()
}
fn q_solid_nc(w: &World, pos: BlockPos) -> bool {
    w.id_at(pos) != 0 && w.material_at_pos(pos).is_solid()
}
fn q_water_lava(w: &World, pos: BlockPos) -> bool {
    // Literal mirror (air and fire count as water-or-lava in C++!).
    let bid = w.id_at(pos);
    if bid == 0 || bid == 51 {
        return true;
    }
    w.material_at_pos(pos).is_liquid()
}
fn q_water(w: &World, pos: BlockPos) -> bool {
    w.material_at_pos(pos) == Material::WATER
}
#[allow(dead_code)]
fn q_collidable(w: &World, pos: BlockPos) -> bool {
    let bid = w.id_at(pos);
    if bid == 0 {
        return false;
    }
    let props = crate::block::table::block_properties_get(bid as u32);
    props.block_type != crate::block::table::BlockType::Fluid as u8
        && has_collision_box(props.block_type)
        && has_collision_id(bid)
}
fn u_set_meta(w: &mut World, pos: BlockPos, meta: u8) {
    w.set_meta_at(pos, meta);
}
fn u_set_notify(w: &mut World, pos: BlockPos, id: u8) {
    w.set_notify_at(pos, id);
}
fn u_set_meta_notify(w: &mut World, pos: BlockPos, id: u8, meta: u8) {
    w.set_meta_notify_at(pos, id, meta);
}
fn u_set_and_meta(w: &mut World, pos: BlockPos, id: u8, meta: u8) {
    w.set_id_at(pos, id);
    w.set_meta_at(pos, meta);
}
fn u_schedule(w: &mut World, pos: BlockPos, id: u8, delay: i32) {
    w.schedule_at(pos, id, delay);
}
fn u_notify(w: &mut World, pos: BlockPos, _id: u8) {
    w.notify_at(pos);
}
fn u_drop(w: &mut World, drop: DropSpec, fx: f64, fy: f64, fz: f64, spread: f64, up: f64) {
    if drop.is_empty() {
        return;
    }
    let eid = w.spawn_item_entity(drop.item, drop.count, drop.damage, fx, fy, fz);
    let (dx, dz) = (w.rng_next_f64(), w.rng_next_f64());
    if let Some(Entity::Item(e)) = w.entities.get_mut(eid) {
        e.body.motion[0] = -spread + 2.0 * spread * dx;
        e.body.motion[1] = up;
        e.body.motion[2] = -spread + 2.0 * spread * dz;
    }
}
/// Drop at a block center with the vanilla scatter.
fn u_drop_at(w: &mut World, drop: DropSpec, pos: BlockPos, spread: f64, up: f64) {
    let (fx, fy, fz) = pos.drop_center();
    u_drop(w, drop, fx, fy, fz, spread, up);
}
fn rng_int(w: &mut World, bound: i32) -> i32 {
    w.rng_next_int(bound)
}
fn rng_f01(w: &mut World) -> f32 {
    w.rng_next_f32()
}
fn rng_u64(w: &mut World) -> u64 {
    w.rng_next_u64()
}
fn chance_one_in(w: &mut World, one_in: i32) -> bool {
    if one_in <= 1 {
        return true;
    }
    rng_int(w, one_in) == 0
}

/// Base drop (`Block::dropBlockAsItemWithChance`): id/count/damage already
/// resolved by C++ virtuals. Returns whether anything was spawned.
fn base_drop(w: &mut World, drop: DropSpec, pos: BlockPos, chance: f32) -> bool {
    if drop.is_empty() {
        return false;
    }
    if rng_f01(w) > chance {
        return false;
    }
    for _ in 0..drop.count {
        let one = DropSpec::new(drop.item, 1, drop.damage);
        u_drop_at(w, one, pos, 0.1, 0.2);
    }
    true
}

// ---- sand ----

fn sand_schedule(w: &mut World, block_id: u8, pos: BlockPos) {
    u_schedule(w, pos, block_id, 3);
}

pub fn block_sand_added(w: &mut World, block_id: u8, pos: BlockPos) {
    sand_schedule(w, block_id, pos);
}

pub fn block_sand_neighbor(w: &mut World, block_id: u8, pos: BlockPos) {
    sand_schedule(w, block_id, pos);
}

fn sand_can_fall_below(w: &World, pos: BlockPos) -> bool {
    let id = q_id(w, pos);
    if id == 0 || id == 51 {
        return true;
    }
    // Mirrors initBlocks: every non-air material gets a Block instance.
    if !World::native_registered(id) {
        return true;
    }
    q_water_lava(w, pos)
}

pub fn block_sand_tick(w: &mut World, block_id: u8, pos: BlockPos) {
    if pos.y >= 0 && sand_can_fall_below(w, pos.below()) {
        w.set_id_at(pos, 0);
        let id = w.entities.alloc_id();
        let mut b = Body::new(id, 0.98, 0.98, 0.49);
        b.set_position(pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5);
        w.entities
            .insert(Entity::Falling(crate::entity::table::FallingEnt {
                body: b,
                block_id: block_id as i32,
                fall_time: 0,
            }));
    }
}

// ---- fluid ----

const FLOW_DIRS: [(i32, i32, i32); 6] = [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)];
const FLOW_PASSABLE: [u8; 15] = [
    6, 37, 38, 39, 40, 50, 51, 55, 59, 66, 69, 75, 76, 77, 78,
];

fn fluid_blocks_flow(w: &World, pos: BlockPos) -> bool {
    let id = q_id(w, pos);
    if id == 0 || id == 8 || id == 9 || id == 10 || id == 11 {
        return false;
    }
    !FLOW_PASSABLE.contains(&id)
}

fn fluid_can_flow_into(w: &World, pos: BlockPos) -> bool {
    let id = q_id(w, pos);
    if id == 0 {
        return true;
    }
    if id == 8 || id == 9 || id == 10 || id == 11 {
        return false;
    }
    FLOW_PASSABLE.contains(&id)
}

fn same_fluid(w: &World, pos: BlockPos, is_lava: bool) -> bool {
    let id = q_id(w, pos);
    if is_lava {
        id == 10 || id == 11
    } else {
        id == 8 || id == 9
    }
}

fn calculate_flow_cost(
    w: &World,
    is_lava: bool,
    pos: BlockPos,
    distance: i32,
    from_dir: usize,
) -> i32 {
    const H_DIRS: [(i32, i32, usize); 4] = [(-1, 0, 1), (1, 0, 0), (0, -1, 3), (0, 1, 2)];
    let mut best = 1000;
    for (dir, &(dx, dz, opp)) in H_DIRS.iter().enumerate() {
        if dir == opp && from_dir == dir {
            continue;
        }
        if (dir == 0 && from_dir == 1)
            || (dir == 1 && from_dir == 0)
            || (dir == 2 && from_dir == 3)
            || (dir == 3 && from_dir == 2)
        {
            continue;
        }
        let n = pos.offset(dx, 0, dz);
        if !fluid_blocks_flow(w, n) && (!same_fluid(w, n, is_lava) || q_meta(w, n) > 0) {
            if n.y > 0 && !fluid_blocks_flow(w, n.below()) {
                return distance;
            }
            if distance < 4 {
                let c = calculate_flow_cost(w, is_lava, n, distance + 1, dir);
                if c < best {
                    best = c;
                }
            }
        }
    }
    best
}

fn optimal_flow_dirs(w: &World, is_lava: bool, pos: BlockPos) -> [bool; 4] {
    const H_DIRS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    let mut costs = [1000i32; 4];
    for (dir, &(dx, dz)) in H_DIRS.iter().enumerate() {
        let n = pos.offset(dx, 0, dz);
        if !fluid_blocks_flow(w, n) && (!same_fluid(w, n, is_lava) || q_meta(w, n) > 0) {
            if n.y > 0 && !fluid_blocks_flow(w, n.below()) {
                costs[dir] = 0;
            } else {
                costs[dir] = calculate_flow_cost(w, is_lava, n, 1, dir);
            }
        }
    }
    let min_cost = *costs.iter().min().unwrap_or(&1000);
    [
        costs[0] == min_cost,
        costs[1] == min_cost,
        costs[2] == min_cost,
        costs[3] == min_cost,
    ]
}

pub fn block_fluid_added(w: &mut World, block_id: u8, tick_rate: i32, pos: BlockPos) {
    u_schedule(w, pos, block_id, tick_rate);
}

pub fn block_fluid_neighbor(w: &mut World, block_id: u8, tick_rate: i32, pos: BlockPos) {
    u_schedule(w, pos, block_id, tick_rate);
}

fn has_burning_neighbor(w: &World, pos: BlockPos) -> bool {
    for (dx, dy, dz) in FLOW_DIRS {
        let n = pos.offset(dx, dy, dz);
        if w.material_at(n.x, n.y, n.z).get_burning() {
            return true;
        }
    }
    false
}

pub fn block_fluid_tick(w: &mut World, block_id: u8, is_lava: bool, pos: BlockPos) {
    // Lava creates fire on adjacent burnable blocks (Java BlockStationary).
    if is_lava && q_meta(w, pos) == 0 {
        let mut ignited = false;
        for (dx, dy, dz) in FLOW_DIRS {
            let n = pos.offset(dx, dy, dz);
            if q_id(w, n) == 0 && rng_int(w, 4) == 0 && has_burning_neighbor(w, n) {
                u_set_notify(w, n, 51);
                ignited = true;
            }
        }
        if !ignited {
            let mut has_nearby_flammable = false;
            'scan: for dy in 1..=3 {
                for dx in -2..=2 {
                    for dz in -2..=2 {
                        let p = pos.offset(dx, dy, dz);
                        if w.material_at(p.x, p.y, p.z).get_burning() {
                            has_nearby_flammable = true;
                            break 'scan;
                        }
                    }
                }
            }
            if has_nearby_flammable {
                let steps = rng_int(w, 3);
                let mut cur = pos;
                for _ in 0..steps {
                    cur = cur.offset(rng_int(w, 3) - 1, 1, rng_int(w, 3) - 1);
                    let id = q_id(w, cur);
                    if id == 0 {
                        if has_burning_neighbor(w, cur) {
                            u_set_notify(w, cur, 51);
                            break;
                        }
                    } else if w.material_at(cur.x, cur.y, cur.z).blocks_movement() {
                        break;
                    }
                }
            }
        }
    }

    let mut metadata = q_meta(w, pos) as i32;
    let step: i32 = if is_lava { 2 } else { 1 };
    if metadata > 0 {
        let mut adj_sources = 0;
        let mut min_decay: i32 = -100;
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let n = pos.offset(dx, 0, dz);
            if same_fluid(w, n, is_lava) {
                let mut nm = q_meta(w, n) as i32;
                if nm == 0 {
                    adj_sources += 1;
                }
                if nm >= 8 {
                    nm = 0;
                }
                if min_decay < 0 || nm < min_decay {
                    min_decay = nm;
                }
            }
        }
        let mut expected = min_decay + step;
        if expected >= 8 || min_decay < 0 {
            expected = -1;
        }
        if same_fluid(w, pos.above(), is_lava) {
            let above_meta = q_meta(w, pos.above()) as i32;
            expected = if above_meta >= 8 {
                above_meta
            } else {
                above_meta + 8
            };
        }
        if adj_sources >= 2
            && !is_lava
            && (q_attach_world(w, pos.below())
                || (same_fluid(w, pos.below(), false) && q_meta(w, pos.below()) == 0))
        {
            expected = 0;
        }
        if expected != metadata {
            metadata = expected;
            if expected < 0 {
                u_set_notify(w, pos, 0);
                return;
            }
            let new_id = if expected == 0 {
                if is_lava { 11 } else { 9 }
            } else {
                block_id
            };
            u_set_meta_notify(w, pos, new_id, expected as u8);
        }
    }

    if pos.y > 0 && fluid_can_flow_into(w, pos.below()) {
        if q_id(w, pos.below()) != 0 && !is_lava {
            w.drop_block_as_item(pos.x, pos.y - 1, pos.z);
        }
        let down_meta = if metadata >= 8 {
            metadata as u8
        } else {
            (metadata + 8) as u8
        };
        u_set_meta_notify(w, pos.below(), block_id, down_meta);
    } else if metadata == 0
        || (pos.y > 0
            && !fluid_can_flow_into(w, pos.below())
            && !same_fluid(w, pos.below(), is_lava))
    {
        let new_meta = if metadata >= 8 {
            step
        } else {
            metadata + step
        };
        if new_meta < 8 {
            let dirs = optimal_flow_dirs(w, is_lava, pos);
            for (dir, (dx, dz)) in [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter().enumerate() {
                if !dirs[dir] {
                    continue;
                }
                let n = pos.offset(dx, 0, dz);
                if fluid_can_flow_into(w, n) {
                    if q_id(w, n) != 0 && !is_lava {
                        w.drop_block_as_item(n.x, n.y, n.z);
                    }
                    u_set_meta_notify(w, n, block_id, new_meta as u8);
                }
            }
        }
    }
}

// ---- flower ----

fn flower_can_stay_here(w: &World, pos: BlockPos) -> bool {
    let below = q_id(w, pos.below());
    (q_light(w, pos) >= 8 || q_sky(w, pos)) && (below == 2 || below == 3 || below == 60)
}

pub fn block_flower_can_stay(w: &World, pos: BlockPos) -> bool {
    flower_can_stay_here(w, pos)
}

pub fn block_flower_neighbor(w: &mut World, drop: DropSpec, pos: BlockPos) {
    if !flower_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        w.set_id_at(pos, 0);
    }
}

pub fn block_flower_tick(w: &mut World, drop: DropSpec, pos: BlockPos) {
    if !flower_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
    }
}

// ---- tall grass (drop only; stay logic inherited from flower) ----

pub fn block_tallgrass_drop(w: &mut World, seeds_id: i32, chance: f32, pos: BlockPos) {
    if seeds_id <= 0 {
        return;
    }
    if rng_f01(w) > chance {
        return;
    }
    if !chance_one_in(w, 8) {
        return;
    }
    let (fx, fy, fz) = pos.center();
    u_drop(w, DropSpec::new(seeds_id, 1, 0), fx, fy, fz, 0.05, 0.15);
}

// ---- mushroom ----

fn mushroom_can_stay_here(w: &World, pos: BlockPos) -> bool {
    // Mirrors `BlockMushroom.canBlockStay`: dim light plus an attachable
    // (opaque) block below (`field_540_p`).
    q_light(w, pos) <= 13 && q_attach_world(w, pos.below())
}

pub fn block_mushroom_can_stay(w: &World, pos: BlockPos) -> bool {
    mushroom_can_stay_here(w, pos)
}

pub fn block_mushroom_neighbor(w: &mut World, drop: DropSpec, pos: BlockPos) {
    if !mushroom_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        w.set_id_at(pos, 0);
    }
}

// ---- torch ----

fn torch_attached(w: &World, pos: BlockPos) -> bool {
    q_attach_torch(w, pos)
}

/// Java BlockTorch.onBlockPlaced metadata from the clicked face.
/// Takes the torch position; side: 1=floor(default 5),2,3,4,5 wall faces.
pub fn block_torch_attach_meta(w: &World, side: i32, pos: BlockPos) -> u8 {
    if side == 2 && torch_attached(w, pos.offset(0, 0, 1)) {
        4
    } else if side == 3 && torch_attached(w, pos.offset(0, 0, -1)) {
        3
    } else if side == 4 && torch_attached(w, pos.offset(1, 0, 0)) {
        2
    } else if side == 5 && torch_attached(w, pos.offset(-1, 0, 0)) {
        1
    } else {
        5
    }
}

pub fn block_torch_added(w: &mut World, block_id: u8, pos: BlockPos) {
    if q_meta(w, pos) != 0 {
        return;
    }
    let meta = if torch_attached(w, pos.offset(-1, 0, 0)) {
        1
    } else if torch_attached(w, pos.offset(1, 0, 0)) {
        2
    } else if torch_attached(w, pos.offset(0, 0, -1)) {
        3
    } else if torch_attached(w, pos.offset(0, 0, 1)) {
        4
    } else if torch_attached(w, pos.below()) {
        5
    } else {
        0
    };
    if meta != 0 {
        u_set_and_meta(w, pos, block_id, meta);
    }
}

fn torch_can_stay_here(w: &World, pos: BlockPos) -> bool {
    torch_attached(w, pos.offset(-1, 0, 0))
        || torch_attached(w, pos.offset(1, 0, 0))
        || torch_attached(w, pos.offset(0, 0, -1))
        || torch_attached(w, pos.offset(0, 0, 1))
        || torch_attached(w, pos.below())
}

pub fn block_torch_can_stay(w: &World, pos: BlockPos) -> bool {
    torch_can_stay_here(w, pos)
}

pub fn block_torch_neighbor(w: &mut World, drop: DropSpec, pos: BlockPos) {
    let meta = q_meta(w, pos);
    let detach = (meta == 1 && !torch_attached(w, pos.offset(-1, 0, 0)))
        || (meta == 2 && !torch_attached(w, pos.offset(1, 0, 0)))
        || (meta == 3 && !torch_attached(w, pos.offset(0, 0, -1)))
        || (meta == 4 && !torch_attached(w, pos.offset(0, 0, 1)))
        || (meta == 5 && !torch_attached(w, pos.below()));
    if detach {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
    }
}

// ---- cactus / reed (shared growth shape, different stay rules) ----

fn cactus_can_stay_here(w: &World, pos: BlockPos) -> bool {
    if q_solid(w, pos.offset(-1, 0, 0))
        || q_solid(w, pos.offset(1, 0, 0))
        || q_solid(w, pos.offset(0, 0, -1))
        || q_solid(w, pos.offset(0, 0, 1))
    {
        return false;
    }
    let below = q_id(w, pos.below());
    below == 12 || below == 81
}

fn reed_can_stay_here(w: &World, pos: BlockPos) -> bool {
    let below = q_id(w, pos.below());
    if below == 83 {
        return true;
    }
    // Mirrors `BlockReed.canPlaceBlockAt`: grass or dirt only (sand
    // never hosts reed), with water adjacent at soil level.
    if below != 2 && below != 3 {
        return false;
    }
    for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let id = q_id(w, pos.offset(dx, -1, dz));
        if id == 8 || id == 9 {
            return true;
        }
    }
    false
}

/// Shared cactus/reed tick: stay check, headroom, grow to height 3 max.
fn stalk_tick_inner(w: &mut World, block_id: u8, can_stay: bool, pos: BlockPos, reschedule: bool) {
    if !can_stay {
        return;
    }
    if q_id(w, pos.above()) != 0 {
        if reschedule {
            u_schedule(w, pos, block_id, 20);
        }
        return;
    }
    let mut height = 1;
    while q_id(w, pos.offset(0, -height, 0)) == block_id {
        height += 1;
    }
    if height < 3 {
        let age = q_meta(w, pos);
        if age >= PLANT_GROWTH_STAGE_MAX {
            u_set_notify(w, pos.above(), block_id);
            u_set_meta(w, pos, 0);
        } else {
            u_set_meta(w, pos, age.saturating_add(1));
        }
    }
    if reschedule {
        u_schedule(w, pos, block_id, 20);
    }
}

pub fn block_cactus_can_stay(w: &World, pos: BlockPos) -> bool {
    cactus_can_stay_here(w, pos)
}

pub fn block_reed_can_stay(w: &World, pos: BlockPos) -> bool {
    reed_can_stay_here(w, pos)
}

pub fn block_cactus_added(w: &mut World, block_id: u8, pos: BlockPos) {
    u_schedule(w, pos, block_id, 20);
}

pub fn block_reed_added(w: &mut World, block_id: u8, pos: BlockPos) {
    u_schedule(w, pos, block_id, 20);
}

pub fn block_cactus_neighbor(w: &mut World, _block_id: u8, drop: DropSpec, pos: BlockPos) {
    if !cactus_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
    }
}

pub fn block_reed_neighbor(w: &mut World, _block_id: u8, drop: DropSpec, pos: BlockPos) {
    if !reed_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
    }
}

pub fn block_cactus_tick(w: &mut World, block_id: u8, drop: DropSpec, pos: BlockPos) {
    if !cactus_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
        return;
    }
    stalk_tick_inner(w, block_id, true, pos, true);
}

pub fn block_cactus_random_tick(w: &mut World, block_id: u8, drop: DropSpec, pos: BlockPos) {
    if !cactus_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
        return;
    }
    stalk_tick_inner(w, block_id, true, pos, false);
}

pub fn block_reed_tick(w: &mut World, block_id: u8, drop: DropSpec, pos: BlockPos) {
    if !reed_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
        return;
    }
    stalk_tick_inner(w, block_id, true, pos, false);
}

// ---- leaves (recursion guard passed by mutable borrow) ----

fn leaf_propagate(w: &World, leaves_id: u8, pos: BlockPos, current: i32) -> i32 {
    let id = q_id_nc(w, pos);
    if id == 17 {
        return 16;
    }
    if id == leaves_id {
        let meta = q_meta(w, pos) as i32;
        if meta != 0 && meta > current {
            return meta;
        }
    }
    current
}

fn update_leaf_distance(
    w: &mut World,
    block_id: u8,
    leaves_id: u8,
    pos: BlockPos,
    guard: &mut i32,
) {
    *guard += 1;
    if *guard > LEAVES_DECAY_GUARD_MAX {
        return;
    }

    // Server fix: leaves on solid ground don't decay.
    let mut candidate = 0;
    if q_solid_nc(w, pos.below()) {
        candidate = 16;
    }

    let metadata = q_meta(w, pos) as i32;
    if metadata == 0 {
        u_set_meta(w, pos, 1);
    }

    candidate = leaf_propagate(w, leaves_id, pos.offset(-1, 0, 0), candidate);
    candidate = leaf_propagate(w, leaves_id, pos.offset(1, 0, 0), candidate);
    candidate = leaf_propagate(w, leaves_id, pos.offset(0, -1, 0), candidate);
    candidate = leaf_propagate(w, leaves_id, pos.offset(0, 1, 0), candidate);
    candidate = leaf_propagate(w, leaves_id, pos.offset(0, 0, -1), candidate);
    candidate = leaf_propagate(w, leaves_id, pos.offset(0, 0, 1), candidate);

    let mut new_meta = candidate - 1;
    if new_meta < 10 {
        new_meta = 1;
    }

    if new_meta != metadata {
        u_set_meta(w, pos, new_meta as u8);
        // Server fix: force neighbors to recalculate, otherwise the
        // flood-fill chain reaction stops here.
        u_notify(w, pos, block_id);
        update_neighbor_leaf(
            w,
            block_id,
            leaves_id,
            pos.offset(-1, 0, 0),
            metadata,
            guard,
        );
        update_neighbor_leaf(w, block_id, leaves_id, pos.offset(1, 0, 0), metadata, guard);
        update_neighbor_leaf(
            w,
            block_id,
            leaves_id,
            pos.offset(0, -1, 0),
            metadata,
            guard,
        );
        update_neighbor_leaf(w, block_id, leaves_id, pos.offset(0, 1, 0), metadata, guard);
        update_neighbor_leaf(
            w,
            block_id,
            leaves_id,
            pos.offset(0, 0, -1),
            metadata,
            guard,
        );
        update_neighbor_leaf(w, block_id, leaves_id, pos.offset(0, 0, 1), metadata, guard);
    }
}

fn update_neighbor_leaf(
    w: &mut World,
    block_id: u8,
    leaves_id: u8,
    pos: BlockPos,
    previous: i32,
    guard: &mut i32,
) {
    if q_id_nc(w, pos) != leaves_id {
        return;
    }
    let meta = q_meta(w, pos) as i32;
    if meta != 0 && meta == previous - 1 {
        update_leaf_distance(w, block_id, leaves_id, pos, guard);
    }
}

pub fn block_leaves_added(w: &mut World, block_id: u8, pos: BlockPos) {
    u_schedule(w, pos, block_id, 40);
}

pub fn block_leaves_neighbor(
    w: &mut World,
    block_id: u8,
    leaves_id: u8,
    guard: &mut i32,
    pos: BlockPos,
) {
    *guard = 0;
    update_leaf_distance(w, block_id, leaves_id, pos, guard);
}

pub fn block_leaves_tick(
    w: &mut World,
    block_id: u8,
    leaves_id: u8,
    drop: DropSpec,
    guard: &mut i32,
    pos: BlockPos,
) {
    let metadata = q_meta(w, pos) as i32;
    if metadata == 0 {
        *guard = 0;
        update_leaf_distance(w, block_id, leaves_id, pos, guard);
    } else if metadata == 1 {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
    } else if chance_one_in(w, 10) {
        update_leaf_distance(w, block_id, leaves_id, pos, guard);
    }
}

pub fn block_leaves_drop(w: &mut World, sapling_id: i32, chance: f32, pos: BlockPos) {
    if sapling_id <= 0 {
        return;
    }
    if rng_f01(w) > chance {
        return;
    }
    if !chance_one_in(w, 20) {
        return;
    }
    let (fx, fy, fz) = pos.center();
    u_drop(w, DropSpec::new(sapling_id, 1, 0), fx, fy, fz, 0.05, 0.15);
}

// ---- sapling ----

fn sapling_can_stay_here(w: &World, pos: BlockPos) -> bool {
    let below = q_id(w, pos.below());
    (q_light(w, pos) >= 8 || q_sky(w, pos)) && (below == 2 || below == 3 || below == 60)
}

pub fn block_sapling_added(w: &mut World, block_id: u8, pos: BlockPos) {
    u_schedule(w, pos, block_id, 100);
}

pub fn block_sapling_can_stay(w: &World, pos: BlockPos) -> bool {
    sapling_can_stay_here(w, pos)
}

pub fn block_sapling_neighbor(w: &mut World, _block_id: u8, drop: DropSpec, pos: BlockPos) {
    if !sapling_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
    }
}

/// Sapling tick. Returns GrowTree (with the rand draw) when the caller
/// should run tree generation; it restores the sapling if generation fails.
pub fn block_sapling_tick(
    w: &mut World,
    _block_id: u8,
    drop: DropSpec,
    pos: BlockPos,
) -> TickAction {
    let none = TickAction {
        kind: SaplingAction::None as u8,
        seed: 0,
    };
    if !sapling_can_stay_here(w, pos) {
        base_drop(w, drop, pos, 1.0);
        u_set_notify(w, pos, 0);
        return none;
    }
    if q_light(w, pos.offset(0, 1, 0)) < 9 || !chance_one_in(w, 5) {
        return none;
    }
    let metadata = q_meta(w, pos);
    if metadata < 15 {
        u_set_meta(w, pos, metadata.saturating_add(1));
        return none;
    }
    let seed = rng_u64(w);
    u_set_notify(w, pos, 0);
    TickAction {
        kind: SaplingAction::GrowTree as u8,
        seed,
    }
}

// ---- crops ----

fn crops_can_stay_here(w: &World, _crop_id: u8, pos: BlockPos) -> bool {
    q_id(w, pos.below()) == 60 && (q_light(w, pos) >= 8 || q_sky(w, pos))
}

fn crops_growth_rate(w: &World, crop_id: u8, pos: BlockPos) -> f32 {
    let mut rate = 1.0f32;
    for dx in -1..=1 {
        for dz in -1..=1 {
            if q_id(w, pos.offset(dx, -1, dz)) == 60 {
                // Java: 1.0 dry, 3.0 hydrated (soil meta > 0).
                let mut bonus = if q_meta(w, pos.offset(dx, -1, dz)) > 0 {
                    3.0
                } else {
                    1.0
                };
                if dx != 0 || dz != 0 {
                    bonus /= 4.0;
                }
                rate += bonus;
            }
        }
    }
    let row = q_id(w, pos.offset(-1, 0, 0)) == crop_id || q_id(w, pos.offset(1, 0, 0)) == crop_id;
    let col = q_id(w, pos.offset(0, 0, -1)) == crop_id || q_id(w, pos.offset(0, 0, 1)) == crop_id;
    let diag = q_id(w, pos.offset(-1, 0, -1)) == crop_id
        || q_id(w, pos.offset(1, 0, -1)) == crop_id
        || q_id(w, pos.offset(1, 0, 1)) == crop_id
        || q_id(w, pos.offset(-1, 0, 1)) == crop_id;
    if diag || (row && col) {
        rate /= 2.0;
    }
    rate
}

pub fn block_crops_added(w: &mut World, block_id: u8, pos: BlockPos) {
    u_schedule(w, pos, block_id, 20);
}

pub fn block_crops_can_stay(w: &World, crop_id: u8, pos: BlockPos) -> bool {
    crops_can_stay_here(w, crop_id, pos)
}

/// Crop neighbor/tick ids bundled to keep the arity clippy-clean.
#[derive(Clone, Copy, Debug)]
pub struct CropIds {
    pub block: u8,
    pub crop: u8,
    pub wheat: i32,
    pub seeds: i32,
}

pub fn block_crops_neighbor(w: &mut World, ids: CropIds, pos: BlockPos) {
    if !crops_can_stay_here(w, ids.crop, pos) {
        let meta = q_meta(w, pos);
        block_crops_drop(w, ids.wheat, pos, meta);
        u_set_notify(w, pos, 0);
    }
}

fn block_crops_drop(w: &mut World, wheat_id: i32, pos: BlockPos, metadata: u8) {
    // Natural break (neighbor/tick → dropBlockAsItem): wheat only when
    // mature. Seeds come only from player harvest
    // (onBlockDestroyedByPlayer), handled in World::rolled_drop_ids.
    // The old code dropped seeds here too, doubling them on tramples.
    if metadata >= 7 {
        let (fx, fy, fz) = pos.center();
        u_drop(w, DropSpec::new(wheat_id, 1, 0), fx, fy, fz, 0.05, 0.15);
    }
}

pub fn block_crops_drop_harvest(
    w: &mut World,
    wheat_id: i32,
    seeds_id: i32,
    pos: BlockPos,
    metadata: u8,
    chance: f32,
) {
    if rng_f01(w) > chance {
        return;
    }
    // Player-harvest path (onBlockDestroyedByPlayer): wheat + 3 seed rolls.
    let (fx, fy, fz) = pos.center();
    if metadata >= 7 {
        u_drop(w, DropSpec::new(wheat_id, 1, 0), fx, fy, fz, 0.05, 0.15);
    }
    for _ in 0..3 {
        if rng_int(w, 15) <= metadata as i32 {
            u_drop(w, DropSpec::new(seeds_id, 1, 0), fx, fy, fz, 0.05, 0.15);
        }
    }
}

pub fn block_crops_tick(w: &mut World, ids: CropIds, pos: BlockPos) {
    if !crops_can_stay_here(w, ids.crop, pos) {
        let meta = q_meta(w, pos);
        block_crops_drop(w, ids.wheat, pos, meta);
        u_set_notify(w, pos, 0);
        return;
    }
    if q_light(w, pos.above()) >= 9 {
        let metadata = q_meta(w, pos);
        if metadata < 7 {
            let rate = crops_growth_rate(w, ids.crop, pos);
            let chance = 2i32.max((100.0f32 / rate) as i32);
            if rng_int(w, chance) == 0 {
                u_set_meta(w, pos, metadata.saturating_add(1));
            }
        }
    }
}

// ---- soil ----

fn soil_has_water(w: &World, pos: BlockPos) -> bool {
    for cx in pos.x - 4..=pos.x + 4 {
        for cy in pos.y..=pos.y + 1 {
            for cz in pos.z - 4..=pos.z + 4 {
                if q_water(w, BlockPos::new(cx, cy, cz)) {
                    return true;
                }
            }
        }
    }
    false
}

fn soil_set_moisture(w: &mut World, pos: BlockPos, moisture: u8) {
    if q_meta(w, pos) == moisture {
        return;
    }
    u_set_meta(w, pos, moisture);
}

pub fn block_soil_added(w: &mut World, block_id: u8, pos: BlockPos) {
    u_schedule(w, pos, block_id, 20);
}

pub fn block_soil_tick(w: &mut World, _block_id: u8, pos: BlockPos) {
    if chance_one_in(w, 5) {
        if soil_has_water(w, pos) {
            soil_set_moisture(w, pos, 7);
        } else {
            let moisture = q_meta(w, pos);
            if moisture > 0 {
                soil_set_moisture(w, pos, moisture - 1);
            } else if q_id(w, pos.above()) != 59 {
                u_set_notify(w, pos, 3);
            }
        }
    }
}

pub fn block_soil_walking(w: &mut World, pos: BlockPos) {
    if chance_one_in(w, 4) {
        u_set_notify(w, pos, 3);
    }
}

pub fn block_soil_neighbor(w: &mut World, _block_id: u8, pos: BlockPos) {
    if q_solid(w, pos.above()) {
        u_set_notify(w, pos, 3);
    }
}

// ---- base drop entry point (Block::dropBlockAsItemWithChance) ----

pub fn block_base_drop(w: &mut World, drop: DropSpec, pos: BlockPos, chance: f32) -> bool {
    base_drop(w, drop, pos, chance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::entity::table::Entity;
    use crate::world::World;

    /// Fresh world. Seeds below were picked so the scenario's FIRST RNG
    /// draw hits (staging and routing draw nothing on these paths).
    fn harness(seed: i64) -> World {
        World::new(seed)
    }

    /// One chunk plus scripted cells (id + meta), with height and skylight
    /// maps, so light queries read real values. No neighbor routing: the
    /// drivers under test drive the real routing themselves.
    fn stage(w: &mut World, cells: &[(i32, i32, i32, u8, u8)]) {
        // 3x3 chunk neighborhood: scheduling requires loaded surroundings
        // (radius 8), so the center chunk never ticks alone.
        for cx in -1..=1 {
            for cz in -1..=1 {
                w.insert_chunk(Chunk::new(cx, cz));
            }
        }
        let c = w.chunk_ref_mut(0, 0).unwrap();
        for &(x, y, z, id, meta) in cells {
            c.set_block_id(x, y, z, id);
            c.set_block_metadata(x, y, z, meta);
        }
        c.generate_height_map();
        c.generate_skylight_map();
    }

    fn p(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(x, y, z)
    }

    fn d(item: i32, count: i32, damage: i32) -> DropSpec {
        DropSpec::new(item, count, damage)
    }

    /// (pos, count) of every loose item with this id.
    fn items_at(w: &World, id: i32) -> Vec<([f64; 3], i32)> {
        let mut out = Vec::new();
        for oid in w.entities.all_ids() {
            if let Some(Entity::Item(e)) = w.entities.get(oid) {
                if e.item_id == id {
                    out.push((e.body.pos, e.count));
                }
            }
        }
        out
    }

    /// (block id, pos) of every falling block.
    fn fallings(w: &World) -> Vec<(i32, [f64; 3])> {
        let mut out = Vec::new();
        for oid in w.entities.all_ids() {
            if let Some(Entity::Falling(e)) = w.entities.get(oid) {
                out.push((e.block_id, e.body.pos));
            }
        }
        out
    }

    #[test]
    fn test_block_scenarios() {
        // 1. Sand falls into air: clears itself and spawns the entity.
        let mut w = harness(7);
        stage(&mut w, &[(0, 5, 0, 12, 0)]);
        block_sand_tick(&mut w, 12, p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 0);
        let f = fallings(&w);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].0, 12);
        assert!((f[0].1[0] - 0.5).abs() < 1e-9 && (f[0].1[2] - 0.5).abs() < 1e-9);

        // 2. Sand on solid ground: nothing happens.
        let mut w = harness(7);
        stage(&mut w, &[(0, 5, 0, 12, 0), (0, 4, 0, 1, 0)]);
        block_sand_tick(&mut w, 12, p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 12);
        assert!(w.entities.all_ids().is_empty());

        // 3. Cactus grows at age 15 with headroom: new block above, age reset.
        let mut w = harness(7);
        stage(&mut w, &[(0, 1, 0, 81, 15), (0, 0, 0, 12, 0)]);
        block_cactus_tick(&mut w, 81, d(81, 1, 0), p(0, 1, 0));
        assert_eq!(w.get_block_id(0, 2, 0), 81);
        assert_eq!(w.get_block_meta(0, 1, 0), 0);

        // 4. Cactus at max height 3: no growth, only reschedule.
        let mut w = harness(7);
        stage(
            &mut w,
            &[
                (0, 3, 0, 81, 15),
                (0, 2, 0, 81, 0),
                (0, 1, 0, 81, 0),
                (0, 0, 0, 12, 0),
            ],
        );
        block_cactus_tick(&mut w, 81, d(81, 1, 0), p(0, 3, 0));
        assert_eq!(w.get_block_id(0, 4, 0), 0);
        assert!(w.scheduled.contains_key(&(20, 0, 3, 0, 81)));

        // 5. Flower under a roof without light: uprooted on neighbor change.
        let mut w = harness(7);
        stage(
            &mut w,
            &[(0, 5, 0, 37, 0), (0, 4, 0, 1, 0), (0, 6, 0, 1, 0)],
        );
        block_flower_neighbor(&mut w, d(37, 1, 0), p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 0);
        let drops = items_at(&w, 37);
        assert_eq!(drops.len(), 1, "{drops:?}");
        assert_eq!(drops[0].1, 1);

        // 6. Torch meta 1 with no western support: detaches.
        let mut w = harness(7);
        stage(&mut w, &[(0, 5, 0, 50, 1)]);
        block_torch_neighbor(&mut w, d(50, 1, 0), p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 0);
        let drops = items_at(&w, 50);
        assert_eq!(drops.len(), 1, "{drops:?}");

        // 7. Torch placement picks wall metadata (side 4 = west face => meta 2).
        let mut w = harness(7);
        stage(&mut w, &[(1, 5, 0, 1, 0)]);
        assert_eq!(block_torch_attach_meta(&w, 4, p(0, 5, 0)), 2);

        // 8. Crops grow one stage when the roll succeeds (seed opens with
        // next_int(50) == 0 on dry soil: growth rate 2.0, chance 50).
        let mut w = harness(18);
        stage(&mut w, &[(0, 5, 0, 59, 3), (0, 4, 0, 60, 0)]);
        block_crops_tick(
            &mut w,
            CropIds {
                block: 59,
                crop: 59,
                wheat: 296,
                seeds: 295,
            },
            p(0, 5, 0),
        );
        assert_eq!(w.get_block_meta(0, 5, 0), 4);

        // 9. Mature crops drop one wheat plus seed rolls (Java: 3x
        // rand(15) <= 7 seeds on destroy; the wheat drop's motion draws
        // sit between the rolls, so the seed accounts for them).
        let mut w = harness(1);
        block_crops_drop_harvest(&mut w, 296, 295, p(0, 5, 0), 7, 1.0);
        let wheat = items_at(&w, 296);
        assert_eq!(wheat.len(), 1, "{wheat:?}");
        assert_eq!(wheat[0].1, 1);
        let seeds: i32 = items_at(&w, 295).iter().map(|(_, c)| c).sum();
        assert_eq!(seeds, 3);

        // 10. Dry soil without crops reverts to dirt (seed opens with
        // next_int(5) == 0).
        let mut w = harness(0);
        stage(&mut w, &[(0, 4, 0, 60, 0)]);
        block_soil_tick(&mut w, 60, p(0, 4, 0));
        assert_eq!(w.get_block_id(0, 4, 0), 3);

        // 11. Trampled soil reverts on a 1/4 roll (seed opens with
        // next_int(4) == 0).
        let mut w = harness(4096);
        stage(&mut w, &[(0, 4, 0, 60, 0)]);
        block_soil_walking(&mut w, p(0, 4, 0));
        assert_eq!(w.get_block_id(0, 4, 0), 3);

        // 12. Lava source ignites air neighbors adjacent to burnable blocks
        // on a 1/4 roll (seed opens with next_int(4) == 0), while bare stone
        // without burnable neighbors does not ignite.
        let mut w = harness(102400);
        stage(&mut w, &[(0, 5, 0, 11, 0), (1, 4, 0, 5, 0)]);
        block_fluid_tick(&mut w, 11, true, p(0, 5, 0));
        assert_eq!(w.get_block_id(1, 5, 0), 51);

        let mut w_stone = harness(102400);
        stage(&mut w_stone, &[(0, 5, 0, 11, 0), (1, 4, 0, 1, 0)]);
        block_fluid_tick(&mut w_stone, 11, true, p(0, 5, 0));
        assert_eq!(w_stone.get_block_id(1, 5, 0), 0);

        // 13. Sapling at growth stage with light grows: action + seed,
        // cleared (seed opens with next_int(5) == 0, then u64 draws).
        let mut w = harness(0);
        stage(&mut w, &[(0, 5, 0, 6, 15), (0, 4, 0, 3, 0)]);
        let act = block_sapling_tick(&mut w, 6, d(6, 1, 0), p(0, 5, 0));
        assert_eq!(act.kind, SaplingAction::GrowTree as u8);
        assert_eq!(act.seed, 15337379307980049274);
        assert_eq!(w.get_block_id(0, 5, 0), 0);

        // 14. Decayed leaves (meta 1) drop and clear on tick.
        let mut w = harness(7);
        stage(&mut w, &[(0, 5, 0, 18, 1)]);
        let mut guard = 0;
        block_leaves_tick(&mut w, 18, 18, d(6, 1, 0), &mut guard, p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 0);
        let drops = items_at(&w, 6);
        assert_eq!(drops.len(), 1, "{drops:?}");

        // 15. Reed needs grass/dirt near water (sand never hosts reed,
        // like Java `BlockReed.canPlaceBlockAt`); mushroom over void
        // does not stay.
        let mut w = harness(7);
        stage(
            &mut w,
            &[
                (0, 5, 0, 83, 0),
                (0, 4, 0, 12, 0),
                (1, 4, 0, 8, 0),
                (0, 6, 0, 39, 0),
            ],
        );
        assert!(!block_reed_can_stay(&w, p(0, 5, 0)));
        w.set_block_id(0, 4, 0, 3);
        assert!(block_reed_can_stay(&w, p(0, 5, 0)));
        assert!(!block_mushroom_can_stay(&w, p(0, 6, 0)));
    }
}
