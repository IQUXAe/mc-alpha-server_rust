//! Fire behavior ported from C++ `BlockFire` (mirrors Java `BlockFire`).
//!
//! Burn-rate tables live here as constants (from the old constructor's
//! `setBurnRate` calls); C++ keeps only the `Block` shell. Spread, aging,
//! and catching run on a direct `&mut World` borrow; detonating TNT calls
//! straight into the world.

use super::pos::BlockPos;
use crate::world::World;

/// (block id, encourage chance, catch ability) from `BlockFire` ctor.
const BURN_RATES: [(u8, i32, i32); 6] = [
    (5, 5, 20),   // planks
    (17, 5, 5),   // wood (log)
    (18, 30, 60), // leaves
    (47, 30, 20), // bookshelf
    (46, 15, 100), // TNT
    (35, 30, 60), // cloth (wool)
];

fn encourage(id: u8) -> i32 {
    BURN_RATES.iter().find(|(b, _, _)| *b == id).map(|(_, c, _)| *c).unwrap_or(0)
}

fn ability(id: u8) -> i32 {
    BURN_RATES.iter().find(|(b, _, _)| *b == id).map(|(_, _, a)| *a).unwrap_or(0)
}

fn q_id(w: &World, pos: BlockPos) -> u8 {
    w.id_at(pos)
}
fn q_meta(w: &World, pos: BlockPos) -> u8 {
    w.meta_at(pos)
}
fn q_attach(w: &World, pos: BlockPos) -> bool {
    w.attach_at(pos)
}
fn rng_int(w: &mut World, bound: i32) -> i32 {
    w.rng_next_int(bound)
}
fn u_set_meta(w: &mut World, pos: BlockPos, meta: u8) {
    w.set_meta_at(pos, meta);
}
fn u_set_notify(w: &mut World, pos: BlockPos, id: u8) {
    w.set_notify_at(pos, id);
}
fn u_schedule(w: &mut World, pos: BlockPos, id: u8, delay: i32) {
    w.schedule_at(pos, id, delay);
}

fn encourage_at(w: &World, pos: BlockPos, current: i32) -> i32 {
    let v = encourage(q_id(w, pos));
    if v > current {
        v
    } else {
        current
    }
}

fn has_burnable_neighbor(w: &World, pos: BlockPos) -> bool {
    encourage_at(w, pos.offset(1, 0, 0), 0) > 0
        || encourage_at(w, pos.offset(-1, 0, 0), 0) > 0
        || encourage_at(w, pos.offset(0, -1, 0), 0) > 0
        || encourage_at(w, pos.offset(0, 1, 0), 0) > 0
        || encourage_at(w, pos.offset(0, 0, -1), 0) > 0
        || encourage_at(w, pos.offset(0, 0, 1), 0) > 0
}

fn neighbors_encourage(w: &World, pos: BlockPos) -> i32 {
    if q_id(w, pos) != 0 {
        return 0;
    }
    let mut result = 0;
    result = encourage_at(w, pos.offset(1, 0, 0), result);
    result = encourage_at(w, pos.offset(-1, 0, 0), result);
    result = encourage_at(w, pos.offset(0, -1, 0), result);
    result = encourage_at(w, pos.offset(0, 1, 0), result);
    result = encourage_at(w, pos.offset(0, 0, -1), result);
    result = encourage_at(w, pos.offset(0, 0, 1), result);
    result
}

fn try_catch_fire(w: &mut World, fire_id: u8, pos: BlockPos, chance: i32) {
    let id = q_id(w, pos);
    if ability(id) > 0 && rng_int(w, chance) < ability(id) {
        let is_tnt = id == 46;
        if rng_int(w, 2) == 0 {
            u_set_notify(w, pos, fire_id);
        } else {
            u_set_notify(w, pos, 0);
        }
        if is_tnt {
            w.ignite_at(pos, 80);
        }
    }
}

pub fn block_fire_tick(w: &mut World, fire_id: u8, tick_rate: i32, pos: BlockPos) {
    let on_netherrack = q_id(w, pos.below()) == 87;
    let meta = q_meta(w, pos);

    if meta < 15 {
        u_set_meta(w, pos, meta.saturating_add(1));
        u_schedule(w, pos, fire_id, tick_rate);
    }

    if !on_netherrack && !has_burnable_neighbor(w, pos) {
        if !q_attach(w, pos.below()) || meta > 3 {
            u_set_notify(w, pos, 0);
        }
    } else if !on_netherrack
        && encourage(q_id(w, pos.below())) == 0
        && meta == 15
        && rng_int(w, 4) == 0
    {
        u_set_notify(w, pos, 0);
    } else if meta.is_multiple_of(2) && meta > 2 {
        try_catch_fire(w, fire_id, pos.offset(1, 0, 0), 300);
        try_catch_fire(w, fire_id, pos.offset(-1, 0, 0), 300);
        try_catch_fire(w, fire_id, pos.offset(0, -1, 0), 250);
        try_catch_fire(w, fire_id, pos.offset(0, 1, 0), 250);
        try_catch_fire(w, fire_id, pos.offset(0, 0, -1), 300);
        try_catch_fire(w, fire_id, pos.offset(0, 0, 1), 300);

        for nx in pos.x - 1..=pos.x + 1 {
            for nz in pos.z - 1..=pos.z + 1 {
                for ny in pos.y - 1..=pos.y + 4 {
                    if nx == pos.x && ny == pos.y && nz == pos.z {
                        continue;
                    }
                    let mut chance = 100;
                    if ny > pos.y + 1 {
                        chance += (ny - (pos.y + 1)) * 100;
                    }
                    let neighbor = neighbors_encourage(w, BlockPos::new(nx, ny, nz));
                    // Java BlockFire: nextInt(chance) <= encourage (inclusive).
                    if neighbor > 0 && rng_int(w, chance) <= neighbor {
                        u_set_notify(w, BlockPos::new(nx, ny, nz), fire_id);
                    }
                }
            }
        }
    }
}

pub fn block_fire_can_place(w: &World, pos: BlockPos) -> bool {
    q_attach(w, pos.below()) || has_burnable_neighbor(w, pos)
}

pub fn block_fire_neighbor(w: &mut World, pos: BlockPos) {
    if !q_attach(w, pos.below()) && !has_burnable_neighbor(w, pos) {
        u_set_notify(w, pos, 0);
    }
}

/// Attempt to ignite a 4x5 Nether portal frame (`BlockPortal.tryToCreatePortal`).
pub fn try_create_portal(w: &mut World, pos: BlockPos) -> bool {
    let mut dx = 0;
    let mut dz = 0;
    if q_id(w, pos.offset(-1, 0, 0)) == 49 || q_id(w, pos.offset(1, 0, 0)) == 49 {
        dx = 1;
    }
    if q_id(w, pos.offset(0, 0, -1)) == 49 || q_id(w, pos.offset(0, 0, 1)) == 49 {
        dz = 1;
    }
    if dx == dz {
        return false;
    }
    let (mut x, y, mut z) = (pos.x, pos.y, pos.z);
    let left_id = q_id(w, BlockPos::new(x - dx, y, z - dz));
    if left_id == 0 || left_id == 51 {
        x -= dx;
        z -= dz;
    }
    for ih in -1..=2 {
        for iv in -1..=3 {
            let is_frame = ih == -1 || ih == 2 || iv == -1 || iv == 3;
            if (ih != -1 && ih != 2) || (iv != -1 && iv != 3) {
                let bid = q_id(w, BlockPos::new(x + dx * ih, y + iv, z + dz * ih));
                if is_frame {
                    if bid != 49 {
                        return false;
                    }
                } else if bid != 0 && bid != 51 {
                    return false;
                }
            }
        }
    }
    for ih in 0..2 {
        for iv in 0..3 {
            w.set_block_id(x + dx * ih, y + iv, z + dz * ih, 90);
        }
    }
    for ih in 0..2 {
        for iv in 0..3 {
            w.notify_neighbors_of(x + dx * ih, y + iv, z + dz * ih);
        }
    }
    true
}

pub fn block_fire_added(w: &mut World, fire_id: u8, tick_rate: i32, pos: BlockPos) {
    if q_id(w, pos.below()) == 49 && try_create_portal(w, pos) {
        return;
    }
    if !q_attach(w, pos.below()) && !has_burnable_neighbor(w, pos) {
        u_set_notify(w, pos, 0);
    } else {
        u_schedule(w, pos, fire_id, tick_rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::world::World;

    fn harness(seed: i64) -> World {
        World::new(seed)
    }

    /// One chunk plus scripted cells (id + meta), with height and skylight
    /// maps. No neighbor routing: the drivers under test drive the real
    /// routing themselves.
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

    #[test]
    fn test_fire_scenarios() {
        // 1. Young fire on attached stone ages and reschedules.
        let mut w = harness(7);
        stage(&mut w, &[(0, 5, 0, 51, 2), (0, 4, 0, 1, 0)]);
        block_fire_tick(&mut w, 51, 10, p(0, 5, 0));
        assert_eq!(w.get_block_meta(0, 5, 0), 3);
        assert!(w.scheduled.contains_key(&(10, 0, 5, 0, 51)));

        // 2. Old fire without fuel dies.
        let mut w = harness(7);
        stage(&mut w, &[(0, 5, 0, 51, 5), (0, 4, 0, 1, 0)]);
        block_fire_tick(&mut w, 51, 10, p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 0);

        // 3. Fire on netherrack never starves.
        let mut w = harness(7);
        stage(&mut w, &[(0, 5, 0, 51, 15), (0, 4, 0, 87, 0)]);
        block_fire_tick(&mut w, 51, 10, p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 51);

        // 4. Adjacent planks catch fire on a lucky roll (seed opens with
        // next_int(300) < 20, then next_int(2) == 0). Stone below keeps
        // the new fire attached (unsupported fire burns out on placement,
        // like vanilla).
        let mut w = harness(81);
        stage(
            &mut w,
            &[
                (0, 5, 0, 51, 4),
                (0, 4, 0, 87, 0),
                (1, 5, 0, 5, 0),
                (1, 4, 0, 1, 0),
            ],
        );
        block_fire_tick(&mut w, 51, 10, p(0, 5, 0));
        assert_eq!(w.get_block_id(1, 5, 0), 51);

        // 5. TNT catches, clears, and primes a visible entity (seed opens with
        // next_int(300) < 100, then next_int(2) == 1).
        let mut w = harness(0);
        stage(
            &mut w,
            &[(0, 5, 0, 51, 4), (0, 4, 0, 87, 0), (1, 5, 0, 46, 0)],
        );
        block_fire_tick(&mut w, 51, 10, p(0, 5, 0));
        assert_eq!(w.get_block_id(1, 5, 0), 0);
        let primed: Vec<_> = w
            .entities
            .alive_ids()
            .into_iter()
            .filter_map(|id| match w.entities.get(id) {
                Some(crate::entity::table::Entity::Tnt(t)) => Some((t.body.pos, t.fuse)),
                _ => None,
            })
            .collect();
        assert_eq!(primed.len(), 1);
        assert_eq!(primed[0].1, 80);
        assert!((primed[0].0[0] - 1.5).abs() < 1e-9);

        // 6. Encouraged air next to the fire ignites through the spread loop
        // (air at +1 with planks at +2; seed opens with next_int(100) <= 5).
        let mut w = harness(300018);
        stage(
            &mut w,
            &[(0, 5, 0, 51, 4), (0, 4, 0, 87, 0), (2, 5, 0, 5, 0)],
        );
        block_fire_tick(&mut w, 51, 10, p(0, 5, 0));
        assert_eq!(w.get_block_id(1, 5, 0), 51);

        // 7. Placement needs support or fuel; dead fire clears on touch.
        let mut w = harness(7);
        assert!(!block_fire_can_place(&w, p(0, 5, 0)));
        stage(&mut w, &[(0, 5, 0, 51, 0)]);
        block_fire_neighbor(&mut w, p(0, 5, 0));
        assert_eq!(w.get_block_id(0, 5, 0), 0);
        let mut w = harness(7);
        stage(&mut w, &[(0, 4, 0, 1, 0)]);
        block_fire_added(&mut w, 51, 10, p(0, 5, 0));
        assert!(w.scheduled.contains_key(&(10, 0, 5, 0, 51)));
    }
}
