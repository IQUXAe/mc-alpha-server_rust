//! Item verbs ported from C++ `Item.cpp` (`ItemHoe/Seeds/FlintAndSteel/Sign/
//! Block/Boat::onItemUse`, mirrors Java `Item*`).
//!
//! Each verb runs its checks and world mutations through the shared
//! `ItemUseWorld` context (a live `&mut World` plus the acting session).
//! Stack-count/damage bookkeeping stays in the caller (it owns `ItemStack`);
//! Rust reports outcomes the caller applies.
//!
//! Face-offset tables, soil rules, and growth constants are centralized
//! here so the six verbs share one tested source of truth.

use crate::block::pos::{BlockPos, DropSpec};
use crate::entity::table::Entity;
use crate::item_use::{item_furnace_facing, item_sign_yaw_meta};
use crate::math_helper::{cos, sin};
use crate::session::play::PlaySession;
use crate::world::World;

/// Live world access for item verbs: the world plus the session whose
/// outbox receives tile-entity packets. Built explicitly at each call site.
pub struct ItemUseWorld<'a> {
    pub world: &'a mut World,
    pub session: &'a mut PlaySession,
}

const PLACE_DX: [i32; 6] = [0, 0, 0, 0, -1, 1];
const PLACE_DY: [i32; 6] = [-1, 1, 0, 0, 0, 0];
const PLACE_DZ: [i32; 6] = [0, 0, -1, 1, 0, 0];

fn place_offset(side: i32) -> Option<(i32, i32, i32)> {
    if !(0..=5).contains(&side) {
        return None;
    }
    let s = side as usize;
    Some((PLACE_DX[s], PLACE_DY[s], PLACE_DZ[s]))
}

fn q_id(u: &mut ItemUseWorld, pos: BlockPos) -> u8 {
    u.world.id_at(pos)
}
fn rng_int(u: &mut ItemUseWorld, bound: i32) -> i32 {
    if bound <= 0 {
        return 0;
    }
    u.world.rng_next_int(bound)
}
fn rng_f64(u: &mut ItemUseWorld) -> f64 {
    u.world.rng_next_f64()
}

/// Collision-box check for cover/placement: fluids never collide, the rest
/// follow the block-type table.
fn is_collidable(w: &World, pos: BlockPos) -> bool {
    let bid = w.id_at(pos);
    let props = crate::block::table::block_properties_get(bid as u32);
    bid != 0
        && props.block_type != crate::block::table::BlockType::Fluid as u8
        && crate::world::has_collision_box(props.block_type)
}

/// canBlockStay drivers run straight on the live world.
fn can_stay(u: &mut ItemUseWorld, id: u8, pos: BlockPos) -> bool {
    let w = &mut *u.world;
    match id {
        37 | 38 => crate::block::ticks::block_flower_can_stay(w, pos),
        39 | 40 => crate::block::ticks::block_mushroom_can_stay(w, pos),
        50 => crate::block::ticks::block_torch_can_stay(w, pos),
        54 => crate::block::container::block_chest_can_place(w, 54, pos),
        81 => crate::block::ticks::block_cactus_can_stay(w, pos),
        83 => crate::block::ticks::block_reed_can_stay(w, pos),
        6 => crate::block::ticks::block_sapling_can_stay(w, pos),
        59 => crate::block::ticks::block_crops_can_stay(w, id, pos),
        _ => true,
    }
}

/// Placement volume check (mirrors `isPlacementVolumeClear`): no live
/// boat/living intersecting the target box.
fn placement_clear(u: &mut ItemUseWorld, id: u8, pos: BlockPos) -> bool {
    let props = crate::block::table::block_properties_get(id as u32);
    if !crate::world::has_collision_box(props.block_type) {
        return true;
    }
    let mask = crate::aabb::AxisAlignedBB::get_bounding_box(
        pos.x as f64 + props.min_x as f64,
        pos.y as f64 + props.min_y as f64,
        pos.z as f64 + props.min_z as f64,
        pos.x as f64 + props.max_x as f64,
        pos.y as f64 + props.max_y as f64,
        pos.z as f64 + props.max_z as f64,
    );
    for oid in u.world.entities.alive_ids() {
        let blocks = matches!(
            u.world.entities.get(oid),
            Some(Entity::Boat(_))
                | Some(Entity::Mob(_))
                | Some(Entity::Animal(_))
                | Some(Entity::Player(_))
        );
        if blocks {
            if let Some(e) = u.world.entities.get(oid) {
                if mask.intersects_with(&e.body().bounding_box) {
                    return false;
                }
            }
        }
    }
    true
}

/// Torch facing like `onBlockPlaced` (the only `block_placed` override).
/// The attach metadata reads straight from the live world.
fn torch_placed(u: &mut ItemUseWorld, id: u8, pos: BlockPos, side: i32) {
    if id != 50 {
        return;
    }
    let w = &mut *u.world;
    let meta = crate::block::ticks::block_torch_attach_meta(w, side, pos);
    w.set_id_at(pos, id);
    w.set_meta_at(pos, meta);
}

/// Loose-item spawn with motion.
#[derive(Clone, Copy, Debug)]
struct ItemSpawn {
    drop: DropSpec,
    at: (f64, f64, f64),
    motion: (f64, f64, f64),
}

fn spawn_drop(u: &mut ItemUseWorld, s: ItemSpawn) {
    if s.drop.is_empty() {
        return;
    }
    let eid = u.world.spawn_item_entity(
        s.drop.item,
        s.drop.count,
        s.drop.damage,
        s.at.0,
        s.at.1,
        s.at.2,
    );
    if let Some(Entity::Item(e)) = u.world.entities.get_mut(eid) {
        e.body.motion = [s.motion.0, s.motion.1, s.motion.2];
    }
}

/// Tile-entity packet into the acting session's outbox.
fn send_te(u: &mut ItemUseWorld, pos: BlockPos) {
    if let Some(tile) = u.world.tiles.get(&(pos.x, pos.y, pos.z)) {
        u.session.outbox.push(crate::session_packets::tile_packet(
            pos.x, pos.y, pos.z, tile,
        ));
    }
}

/// Liquid-aware raycast over `World::ray_trace_hit_liquids`.
fn ray_hit(u: &mut ItemUseWorld, start: [f64; 3], end: [f64; 3]) -> Option<[i32; 3]> {
    u.world.ray_trace_hit_liquids(start, end)
}

/// Hoe tilling (mirrors `ItemHoe::onItemUse`): grass/dirt to soil, plus a
/// 1/8 seed drop on grass. Returns true when the caller should damage the stack
/// (and possibly destroy it when depleted).
pub fn item_hoe_use(w: &mut ItemUseWorld, seeds_id: i32, pos: BlockPos) -> bool {
    let block_id = q_id(w, pos);
    let cover = is_collidable(w.world, pos.above());
    if (cover || block_id != 2) && block_id != 3 {
        return false;
    }
    if !w.world.set_notify_at(pos, 60) {
        return false;
    }
    if block_id == 2 && seeds_id > 0 && rng_int(w, 8) == 0 {
        let (mx, mz) = (rng_f64(w) * 0.1 - 0.05, rng_f64(w) * 0.1 - 0.05);
        spawn_drop(
            w,
            ItemSpawn {
                drop: DropSpec::new(seeds_id, 1, 0),
                at: (pos.x as f64 + 0.5, pos.y as f64 + 1.1, pos.z as f64 + 0.5),
                motion: (mx, 0.12, mz),
            },
        );
    }
    true
}

/// Seed planting (mirrors `ItemSeeds::onItemUse`). True means the caller should
/// decrement the stack.
pub fn item_seeds_use(w: &mut ItemUseWorld, pos: BlockPos, side: i32) -> bool {
    if side != 1 {
        return false;
    }
    if q_id(w, pos) != 60 || q_id(w, pos.above()) != 0 {
        return false;
    }
    w.world.set_meta_notify_at(pos.above(), 59, 0)
}

/// Flint result: whether fire was placed, the new damage, and whether the stack broke
/// (caller zeroes it when `broke`).
#[derive(Clone, Copy, Debug)]
pub struct FlintOut {
    pub placed: bool,
    pub new_damage: i32,
    pub broke: bool,
}

/// Flint and steel (mirrors Java `ItemFlintAndSteel.onItemUse`): side offset
/// 0=y-1,1=y+1,2=z-1,3=z+1,4=x-1,5=x+1; if the target cell is air, ignite it.
/// Always damages the stack by 1 and always consumes the event, even when
/// nothing ignited. `broke` follows `ItemStack.damageItem`: strict
/// `new_damage > max_damage` (65 uses at max 64).
pub fn item_flint_use(
    w: &mut ItemUseWorld,
    damage_in: i32,
    max_damage: i32,
    pos: BlockPos,
    side: i32,
) -> FlintOut {
    let Some((dx, dy, dz)) = place_offset(side) else {
        // Invalid side: vanilla would still damage, but without a target cell
        // there is nothing to do — report no placement with damage applied.
        let new_damage = damage_in + 1;
        return FlintOut { placed: false, new_damage, broke: new_damage > max_damage };
    };
    let target = pos.offset(dx, dy, dz);
    let mut placed = false;
    if q_id(w, target) == 0 {
        placed = w.world.set_notify_at(target, 51);
    }
    let new_damage = damage_in + 1;
    FlintOut { placed, new_damage, broke: new_damage > max_damage }
}

/// Sign placement (mirrors `ItemSign::onItemUse`). True means the caller should
/// send the edit packet and decrement the stack.
pub fn item_sign_use(w: &mut ItemUseWorld, pos: BlockPos, side: i32, yaw: f32) -> bool {
    if side == 0 {
        return false;
    }
    let solid = w.world.material_at_pos(pos).is_solid();
    if !solid {
        return false;
    }
    let mut target = pos;
    match side {
        1 => target = target.above(),
        2 => target = target.offset(0, 0, -1),
        3 => target = target.offset(0, 0, 1),
        4 => target = target.offset(-1, 0, 0),
        5 => target = target.offset(1, 0, 0),
        _ => return false,
    }
    if !(0..128).contains(&target.y) || q_id(w, target) != 0 {
        return false;
    }
    if side == 1 {
        let meta = item_sign_yaw_meta(yaw);
        let ok = w.world.set_meta_notify_at(target, 63, meta);
        if !ok {
            return false;
        }
    } else if !w.world.set_meta_notify_at(target, 68, side as u8) {
        return false;
    }
    send_te(w, target);
    true
}

/// Door placement (mirrors `ItemDoor.onItemUse` for wooden door 324 -> 64 and
/// iron door 330 -> 71). True means the caller should decrement the stack.
pub fn item_door_use(
    w: &mut ItemUseWorld,
    pos: BlockPos,
    side: i32,
    yaw: f32,
    door_block_id: u8,
) -> bool {
    if side != 1 {
        return false;
    }
    let lower = pos.above();
    let upper = lower.above();
    if lower.y <= 0 || lower.y >= 127 {
        return false;
    }
    if !w.world.attach_at(pos) || q_id(w, lower) != 0 || q_id(w, upper) != 0 {
        return false;
    }
    let mut dir = (crate::math_helper::floor_double(((yaw + 180.0) * 4.0 / 360.0) as f64 - 0.5)
        & 3) as u8;
    let (dx, dz) = match dir {
        0 => (0, 1),
        1 => (-1, 0),
        2 => (0, -1),
        _ => (1, 0),
    };
    let left_solid = i32::from(w.world.attach_at(lower.offset(-dx, 0, -dz)))
        + i32::from(w.world.attach_at(upper.offset(-dx, 0, -dz)));
    let right_solid = i32::from(w.world.attach_at(lower.offset(dx, 0, dz)))
        + i32::from(w.world.attach_at(upper.offset(dx, 0, dz)));
    let left_door = q_id(w, lower.offset(-dx, 0, -dz)) == door_block_id
        || q_id(w, upper.offset(-dx, 0, -dz)) == door_block_id;
    let right_door = q_id(w, lower.offset(dx, 0, dz)) == door_block_id
        || q_id(w, upper.offset(dx, 0, dz)) == door_block_id;
    if (left_door && !right_door) || right_solid > left_solid {
        dir = (dir.wrapping_sub(1) & 3) + 4;
    }
    w.world.set_block_id(lower.x, lower.y, lower.z, door_block_id);
    w.world.set_block_meta(lower.x, lower.y, lower.z, dir);
    w.world.set_block_id(upper.x, upper.y, upper.z, door_block_id);
    w.world.set_block_meta(upper.x, upper.y, upper.z, dir | 8);
    w.world.notify_neighbors_of(lower.x, lower.y, lower.z);
    w.world.notify_neighbors_of(upper.x, upper.y, upper.z);
    true
}

/// Record insertion into an empty jukebox (`84`, meta `0`) for records
/// `2256` ("13") and `2257` ("cat") (mirrors `ItemRecord.onItemUse`).
pub fn item_record_use(w: &mut ItemUseWorld, pos: BlockPos, item_id: i32) -> bool {
    if q_id(w, pos) == 84
        && w.world.meta_at(pos) == 0
        && (item_id == 2256 || item_id == 2257)
    {
        let meta = (item_id - 2255) as u8;
        w.world.set_meta_at(pos, meta);
        return true;
    }
    false
}

/// Block placement input bundled to keep the arity clippy-clean.
#[derive(Clone, Copy, Debug)]
pub struct BlockPlace {
    pub block_id: u8,
    pub stack_count: i32,
    pub side: i32,
    pub yaw: f32,
}

fn lever_or_button_meta(w: &World, pos: BlockPos, side: i32, is_lever: bool) -> Option<u8> {
    match side {
        1 if is_lever && w.attach_at(pos.below()) => Some(5),
        2 if w.attach_at(pos.offset(0, 0, 1)) => Some(4),
        3 if w.attach_at(pos.offset(0, 0, -1)) => Some(3),
        4 if w.attach_at(pos.offset(1, 0, 0)) => Some(2),
        5 if w.attach_at(pos.offset(-1, 0, 0)) => Some(1),
        _ => {
            if w.attach_at(pos.offset(-1, 0, 0)) {
                Some(1)
            } else if w.attach_at(pos.offset(1, 0, 0)) {
                Some(2)
            } else if w.attach_at(pos.offset(0, 0, -1)) {
                Some(3)
            } else if w.attach_at(pos.offset(0, 0, 1)) {
                Some(4)
            } else if is_lever && w.attach_at(pos.below()) {
                Some(5)
            } else {
                None
            }
        }
    }
}

/// Block placement (mirrors `ItemBlock::onItemUse`). True means the caller should
/// decrement the stack.
pub fn item_block_use(w: &mut ItemUseWorld, place: BlockPlace, pos: BlockPos) -> bool {
    if place.stack_count == 0 {
        return false;
    }
    // Single slab placed on top of a single slab merges into a double slab (BlockStep).
    if place.block_id == 44 && place.side == 1 && q_id(w, pos) == 44 {
        if placement_clear(w, 43, pos) && w.world.set_notify_at(pos, 43) {
            return true;
        }
        return false;
    }
    let mut target = pos;
    // Snow layers are replaced instead of offset.
    if q_id(w, pos) != 78 {
        let Some((dx, dy, dz)) = place_offset(place.side) else {
            return false;
        };
        target = target.offset(dx, dy, dz);
    }
    if !(0..128).contains(&target.y) {
        return false;
    }
    let occupying = q_id(w, target);
    if occupying != 0 && !matches!(occupying, 8 | 9 | 10 | 11 | 51 | 78) {
        return false;
    }
    if !World::native_registered(place.block_id) {
        return false;
    }
    if !can_stay(w, place.block_id, target) {
        return false;
    }
    let attach_meta = if place.block_id == 69 || place.block_id == 77 {
        let Some(m) = lever_or_button_meta(w.world, target, place.side, place.block_id == 69) else {
            return false;
        };
        Some(m)
    } else {
        None
    };
    if !placement_clear(w, place.block_id, target) {
        return false;
    }
    if !w.world.set_notify_at(target, place.block_id) {
        return false;
    }
    if place.block_id == 61 || place.block_id == 62 {
        let meta = item_furnace_facing(place.yaw);
        w.world.set_meta_at(target, meta);
        w.world.tile_updates.push([target.x, target.y, target.z]);
    } else if place.block_id == 54 {
        w.world.tile_updates.push([target.x, target.y, target.z]);
    }
    if let Some(m) = attach_meta {
        w.world.set_meta_at(target, m);
    }
    torch_placed(w, place.block_id, target, place.side);
    true
}

/// Boat look vector + eye start (mirrors `ItemBoat::onItemRightClick`
/// interpolation with `partialTick = 1.0`, then the 5-block ray).
#[derive(Clone, Copy, Debug)]
pub struct BoatThrow {
    pub lx: f64,
    pub ly: f64,
    pub lz: f64,
    pub sx: f64,
    pub sy: f64,
    pub sz: f64,
    pub ex: f64,
    pub ey: f64,
    pub ez: f64,
}

/// Boat aim input: interpolated yaw/pitch plus prev/cur eye positions.
#[derive(Clone, Copy, Debug)]
pub struct BoatAimIn {
    pub prev_yaw: f32,
    pub yaw: f32,
    pub prev_pitch: f32,
    pub pitch: f32,
    pub prev: [f64; 3],
    pub cur: [f64; 3],
    pub y_offset: f64,
}

pub fn item_boat_aim(v: BoatAimIn) -> BoatThrow {
    // partialTick is constant 1.0: prev + (cur - prev) * 1.0, in f32/f64
    // exactly like C++.
    let iyaw = v.prev_yaw + (v.yaw - v.prev_yaw) * 1.0f32;
    let ipitch = v.prev_pitch + (v.pitch - v.prev_pitch) * 1.0f32;
    let sx = v.prev[0] + (v.cur[0] - v.prev[0]) * 1.0;
    let mut sy = v.prev[1] + (v.cur[1] - v.prev[1]) * 1.0;
    let sz = v.prev[2] + (v.cur[2] - v.prev[2]) * 1.0;
    sy += 1.62 - v.y_offset;
    let half_pi = std::f32::consts::PI / 180.0f32;
    let cos_yaw = cos(-iyaw * half_pi - std::f32::consts::PI);
    let sin_yaw = sin(-iyaw * half_pi - std::f32::consts::PI);
    let look_h = -cos(-ipitch * half_pi);
    let look_y = sin(-ipitch * half_pi);
    let (lx, lz) = (sin_yaw * look_h, cos_yaw * look_h);
    let (lx, ly, lz) = (lx as f64, look_y as f64, lz as f64);
    BoatThrow {
        lx,
        ly,
        lz,
        sx,
        sy,
        sz,
        ex: sx + lx * 5.0,
        ey: sy + ly * 5.0,
        ez: sz + lz * 5.0,
    }
}

/// Boat raycast resolution (mirrors the tail of `ItemBoat::onItemRightClick`).
/// Returns the hit cell when the boat should spawn and the stack decrement.
pub fn item_boat_throw(w: &mut ItemUseWorld, start: [f64; 3], end: [f64; 3]) -> Option<[i32; 3]> {
    ray_hit(w, start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::entity::table::Entity;
    use crate::session::play::PlaySession;
    use crate::world::World;

    /// Seed whose world RNG stream opens with a 1/8 roll of 0 (the hoe
    /// seed drop below draws first: staging and soil routing draw nothing).
    const HOE_SEED: i64 = 4102;

    fn harness(seed: i64) -> (World, PlaySession) {
        (World::new(seed), PlaySession::new(1))
    }

    /// One chunk plus scripted cells, written straight through the chunk
    /// (no neighbor routing: the verbs under test drive the real routing
    /// themselves when they place).
    fn stage(w: &mut World, cells: &[(i32, i32, i32, u8)]) {
        w.insert_chunk(Chunk::new(0, 0));
        let c = w.chunk_ref_mut(0, 0).unwrap();
        for &(x, y, z, id) in cells {
            c.set_block_id(x, y, z, id);
        }
        c.generate_height_map();
    }

    fn use_ctx<'a>(w: &'a mut World, s: &'a mut PlaySession) -> ItemUseWorld<'a> {
        ItemUseWorld { world: w, session: s }
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

    #[test]
    fn test_verb_scenarios() {
        fn bp(x: i32, y: i32, z: i32) -> BlockPos {
            BlockPos::new(x, y, z)
        }
        // 1. Hoe tills grass to soil, drops a seed on a 1/8 roll.
        let (mut w, mut s) = harness(HOE_SEED);
        stage(&mut w, &[(0, 64, 0, 2)]);
        assert!(item_hoe_use(
            &mut use_ctx(&mut w, &mut s),
            295,
            bp(0, 64, 0)
        ));
        assert_eq!(w.get_block_id(0, 64, 0), 60);
        let drops = items_at(&w, 295);
        assert_eq!(drops.len(), 1, "{drops:?}");
        assert_eq!(drops[0].1, 1);
        assert!((drops[0].0[0] - 0.5).abs() < 1e-9);
        assert!((drops[0].0[1] - 65.1).abs() < 1e-9);
        assert!((drops[0].0[2] - 0.5).abs() < 1e-9);

        // 2. Hoe refuses covered grass and stone.
        let (mut w, mut s) = harness(HOE_SEED);
        stage(&mut w, &[(0, 64, 0, 2), (0, 65, 0, 1)]);
        assert!(!item_hoe_use(
            &mut use_ctx(&mut w, &mut s),
            295,
            bp(0, 64, 0)
        ));
        assert_eq!(w.get_block_id(0, 64, 0), 2);
        assert!(items_at(&w, 295).is_empty());
        let (mut w, mut s) = harness(HOE_SEED);
        stage(&mut w, &[(0, 64, 0, 1)]);
        assert!(!item_hoe_use(
            &mut use_ctx(&mut w, &mut s),
            295,
            bp(0, 64, 0)
        ));

        // 3. Seeds plant on soil with air above, only from the top face.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 60)]);
        assert!(item_seeds_use(
            &mut use_ctx(&mut w, &mut s),
            bp(0, 64, 0),
            1
        ));
        assert_eq!(w.get_block_id(0, 65, 0), 59);
        assert!(!item_seeds_use(
            &mut use_ctx(&mut w, &mut s),
            bp(0, 64, 0),
            2
        ));

        // 4. Flint: vanilla side map (3 => +z), always damages, always consumes.
        // The target needs solid ground: floating fire with no fuel is
        // extinguished on placement, like vanilla.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 1), (0, 63, 1, 1)]);
        let out = item_flint_use(&mut use_ctx(&mut w, &mut s), 3, 64, bp(0, 64, 0), 3);
        assert!(out.placed && out.new_damage == 4 && !out.broke);
        assert_eq!(w.get_block_id(0, 64, 1), 51);
        // Breaks strictly above max (65 uses at max 64).
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 1), (0, 63, 1, 1)]);
        let out = item_flint_use(&mut use_ctx(&mut w, &mut s), 64, 64, bp(0, 64, 0), 3);
        assert!(out.broke && out.new_damage == 65);
        // Occupied target: no placement but still damages + consumes.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 1), (0, 64, 1, 1)]);
        let out = item_flint_use(&mut use_ctx(&mut w, &mut s), 0, 64, bp(0, 64, 0), 3);
        assert!(!out.placed && out.new_damage == 1);
        assert_eq!(w.get_block_id(0, 64, 1), 1);

        // 5. Sign post on solid ground takes yaw metadata; wall sign takes side.
        // Each placement pushes one tile packet (id 59) to the session outbox.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(1, 64, 0, 1)]);
        assert!(item_sign_use(
            &mut use_ctx(&mut w, &mut s),
            bp(1, 64, 0),
            1,
            0.0
        ));
        assert_eq!(w.get_block_id(1, 65, 0), 63);
        assert_eq!(w.get_block_meta(1, 65, 0), 8);
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox[0][0], 59);
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(1, 64, 0, 1)]);
        assert!(item_sign_use(
            &mut use_ctx(&mut w, &mut s),
            bp(1, 64, 0),
            4,
            0.0
        ));
        assert_eq!(w.get_block_id(0, 64, 0), 68);
        assert_eq!(w.get_block_meta(0, 64, 0), 4);
        assert!(!item_sign_use(
            &mut use_ctx(&mut w, &mut s),
            bp(1, 64, 0),
            0,
            0.0
        ));

        // 6. Block placement offsets by face, replaces snow, sets furnace facing.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 1)]);
        assert!(item_block_use(
            &mut use_ctx(&mut w, &mut s),
            BlockPlace {
                block_id: 5,
                stack_count: 1,
                side: 1,
                yaw: 0.0
            },
            bp(0, 64, 0),
        ));
        assert_eq!(w.get_block_id(0, 65, 0), 5);
        // Occupied by stone: refused.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 1), (0, 65, 0, 1)]);
        assert!(!item_block_use(
            &mut use_ctx(&mut w, &mut s),
            BlockPlace {
                block_id: 5,
                stack_count: 1,
                side: 1,
                yaw: 0.0
            },
            bp(0, 64, 0),
        ));
        assert_eq!(w.get_block_id(0, 65, 0), 1);
        // Furnace gets yaw facing metadata.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 1)]);
        assert!(item_block_use(
            &mut use_ctx(&mut w, &mut s),
            BlockPlace {
                block_id: 61,
                stack_count: 1,
                side: 1,
                yaw: 90.0
            },
            bp(0, 64, 0),
        ));
        assert_eq!(w.get_block_id(0, 65, 0), 61);
        assert_eq!(w.get_block_meta(0, 65, 0), 5);

        // 7. Boat throw: a straight-down ray over water hits the liquid cell;
        // a ray across empty sky misses.
        let (mut w, mut s) = harness(7);
        stage(&mut w, &[(0, 64, 0, 9)]);
        assert_eq!(
            item_boat_throw(
                &mut use_ctx(&mut w, &mut s),
                [0.5, 66.0, 0.5],
                [0.5, 60.0, 0.5]
            ),
            Some([0, 64, 0])
        );
        assert_eq!(
            item_boat_throw(
                &mut use_ctx(&mut w, &mut s),
                [0.5, 100.0, 0.5],
                [200.5, 100.0, 0.5],
            ),
            None
        );
    }

    #[test]
    fn test_boat_aim_interpolation() {
        // partialTick is constant 1.0.
        let aim = item_boat_aim(BoatAimIn {
            prev_yaw: 0.0,
            yaw: 0.0,
            prev_pitch: 0.0,
            pitch: 0.0,
            prev: [0.5, 64.0, 0.5],
            cur: [0.5, 64.0, 0.5],
            y_offset: 0.0,
        });
        assert!((aim.sy - 65.62).abs() < 1e-9);
    }
}
