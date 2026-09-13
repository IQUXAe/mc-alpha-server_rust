//! Container-block helpers ported from C++ `BlockChest` / `BlockFurnace`
//! (mirrors Java `BlockChest` / `BlockFurnace`).
//!
//! Only the world-query half moves: chest placement rules and the
//! inventory-scatter loops. TileEntity lookup, GUI packets
//! (`blockActivated`), and TileEntity lifetime stay in C++ until the
//! TileEntity table moves to Rust.
//!
//! RNG notes: the old chest scatter drew from a per-call
//! `mt19937(random_device)` (offsets, chunk sizes, gaussian motion) and the
//! furnace from `World::rand`. Both now draw from the world's own stream.
//! Scatter motion is cosmetic; exact historical streams
//! are intentionally not replicated (the chest stream was re-seeded per
//! call anyway).

use super::pos::{BlockPos, DropSpec};
use crate::world::World;

fn rng_int(w: &mut World, bound: i32) -> i32 {
    w.rng_next_int(bound)
}
fn rng_f32(w: &mut World) -> f32 {
    w.rng_next_f32()
}
fn rng_f64(w: &mut World) -> f64 {
    w.rng_next_f64()
}

/// Standard normal via Box-Muller (two uniforms per sample, no caching).
fn gaussian(w: &mut World) -> f64 {
    let (mut u1, mut u2) = (rng_f64(w), rng_f64(w));
    if u1 <= 0.0 {
        u1 = f64::MIN_POSITIVE;
    }
    if u2 <= 0.0 {
        u2 = f64::MIN_POSITIVE;
    }
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

/// Item spawn: what + where + initial motion.
#[derive(Clone, Copy, Debug)]
struct Spawn {
    drop: DropSpec,
    at: (f64, f64, f64),
    motion: (f64, f64, f64),
}

fn emit(w: &mut World, s: Spawn) {
    if s.drop.is_empty() {
        return;
    }
    let eid = w.spawn_item_entity(
        s.drop.item,
        s.drop.count,
        s.drop.damage,
        s.at.0,
        s.at.1,
        s.at.2,
    );
    if let Some(crate::entity::table::Entity::Item(e)) = w.entities.get_mut(eid) {
        e.body.motion = [s.motion.0, s.motion.1, s.motion.2];
    }
}

/// One chest slot scattered in 10..30-sized chunks (mirrors
/// `BlockChest::onBlockRemoval`). Returns the leftover (always 0).
pub fn block_chest_scatter_stack(w: &mut World, drop: DropSpec, pos: BlockPos) -> i32 {
    if drop.is_empty() {
        return drop.count.max(0);
    }
    // Offsets stay in f32 like the C++ float distribution, then promote.
    // Positions add in f32 too (C++ int+float), widening only at the call.
    // Draws hoisted: one reborrow at a time (same left-to-right order).
    let (r1, r2, r3) = (rng_f32(w), rng_f32(w), rng_f32(w));
    let (ox, oy, oz) = (r1 * 0.8 + 0.1, r2 * 0.8 + 0.1, r3 * 0.8 + 0.1);
    let mut left = drop.count;
    while left > 0 {
        let n = left.min(rng_int(w, 21) + 10);
        left -= n;
        // Draws hoisted: one reborrow at a time (same left-to-right order).
        let (g1, g2, g3) = (gaussian(w), gaussian(w), gaussian(w));
        emit(
            w,
            Spawn {
                drop: DropSpec::new(drop.item, n, drop.damage),
                at: (
                    (pos.x as f32 + ox) as f64,
                    (pos.y as f32 + oy) as f64,
                    (pos.z as f32 + oz) as f64,
                ),
                motion: (g1 * 0.05, g2 * 0.05 + 0.2, g3 * 0.05),
            },
        );
    }
    left
}

/// One furnace slot scattered whole (mirrors `BlockFurnace::onBlockRemoval`).
pub fn block_furnace_scatter_stack(w: &mut World, drop: DropSpec, pos: BlockPos) {
    if drop.is_empty() {
        return;
    }
    let (mx, mz) = (rng_f64(w) * 0.2 - 0.1, rng_f64(w) * 0.2 - 0.1);
    emit(
        w,
        Spawn {
            drop,
            at: (pos.x as f64 + 0.5, pos.y as f64 + 0.7, pos.z as f64 + 0.5),
            motion: (mx, 0.2, mz),
        },
    );
}

/// Chest placement (mirrors `BlockChest::canPlaceBlockAt`): no more than
/// one adjacent chest, and no adjacent double-chest.
pub fn block_chest_can_place(w: &World, chest_id: u8, pos: BlockPos) -> bool {
    let at = |d: (i32, i32)| w.get_block_id(pos.x + d.0, pos.y, pos.z + d.1) == chest_id;
    let mut adjacent = 0;
    for d in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        if at(d) {
            adjacent += 1;
        }
    }
    if adjacent > 1 {
        return false;
    }
    // No neighbor that already has its own neighbor (would make a triple).
    for d in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        if !at(d) {
            continue;
        }
        let (nx, nz) = (pos.x + d.0, pos.z + d.1);
        for e in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            if w.get_block_id(nx + e.0, pos.y, nz + e.1) == chest_id {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::entity::table::Entity;
    use crate::world::World;

    fn harness(seed: i64) -> World {
        World::new(seed)
    }

    /// One chunk plus scripted cells, so placement reads see real blocks.
    fn stage(w: &mut World, cells: &[(i32, i32, i32, u8)]) {
        w.insert_chunk(Chunk::new(0, 0));
        let c = w.chunk_ref_mut(0, 0).unwrap();
        for &(x, y, z, id) in cells {
            c.set_block_id(x, y, z, id);
        }
        c.generate_height_map();
        c.generate_skylight_map();
    }

    /// (item id, count, pos) of every loose item.
    fn spawns(w: &World) -> Vec<(i32, i32, [f64; 3])> {
        let mut out = Vec::new();
        for oid in w.entities.all_ids() {
            if let Some(Entity::Item(e)) = w.entities.get(oid) {
                out.push((e.item_id, e.count, e.body.pos));
            }
        }
        out
    }

    fn p(x: i32, y: i32, z: i32) -> BlockPos {
        BlockPos::new(x, y, z)
    }

    #[test]
    fn test_container_scenarios() {
        // 1. Chest stack of 25 scatters whole: chunk sizes vary with RNG,
        // but the total is invariant and every drop lands inside the cell.
        // The first chunk is always >= 10 (10 + roll); spawn order in the
        // table is unordered, so pin existence, not position.
        let mut w = harness(11);
        let left = block_chest_scatter_stack(&mut w, DropSpec::new(35, 25, 0), p(10, 64, 10));
        assert_eq!(left, 0);
        let l = spawns(&w);
        assert!(!l.is_empty(), "{l:?}");
        let total: i32 = l.iter().map(|(_, c, _)| c).sum();
        assert_eq!(total, 25);
        assert!(l.iter().any(|(_, c, _)| *c >= 10), "{l:?}");
        for (id, c, pos) in &l {
            assert_eq!(*id, 35);
            assert!(*c >= 1);
            assert!((10.0..=11.0).contains(&pos[0]), "{pos:?}");
            assert!((64.0..=65.0).contains(&pos[1]), "{pos:?}");
            assert!((10.0..=11.0).contains(&pos[2]), "{pos:?}");
        }

        // 2. Empty/degenerate inputs scatter nothing.
        let mut w = harness(11);
        assert_eq!(
            block_chest_scatter_stack(&mut w, DropSpec::new(35, 0, 0), p(0, 0, 0)),
            0
        );
        assert_eq!(
            block_chest_scatter_stack(&mut w, DropSpec::new(0, 5, 0), p(0, 0, 0)),
            5
        );
        assert!(spawns(&w).is_empty());
        block_furnace_scatter_stack(&mut w, DropSpec::new(0, 5, 0), p(0, 0, 0));
        assert!(spawns(&w).is_empty());

        // 3. Furnace slot drops whole at block center-top (motion is
        // RNG-driven, only finiteness is pinned).
        let mut w = harness(11);
        block_furnace_scatter_stack(&mut w, DropSpec::new(265, 3, 0), p(1, 2, 3));
        let l = spawns(&w);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].0, l[0].1), (265, 3));
        assert!((l[0].2[0] - 1.5).abs() < 1e-9);
        assert!((l[0].2[1] - 2.7).abs() < 1e-9);
        assert!((l[0].2[2] - 3.5).abs() < 1e-9);

        // 4. Chest placement: solo ok, double ok, triple rejected.
        let mut w = harness(11);
        stage(&mut w, &[]);
        assert!(block_chest_can_place(&w, 54, p(0, 64, 0)));
        stage(&mut w, &[(1, 64, 0, 54)]);
        assert!(block_chest_can_place(&w, 54, p(0, 64, 0)));
        stage(&mut w, &[(1, 64, 0, 54), (2, 64, 0, 54)]);
        assert!(!block_chest_can_place(&w, 54, p(0, 64, 0)));
        // Two adjacent directly: also rejected.
        let mut w = harness(11);
        stage(&mut w, &[(1, 64, 0, 54), (0, 64, 1, 54)]);
        assert!(!block_chest_can_place(&w, 54, p(0, 64, 0)));
    }
}
