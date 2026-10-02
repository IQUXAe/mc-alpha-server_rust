//! Rail shape logic (mirrors Java `MinecartTrackLogic` + the
//! `BlockMinecartTrack` placement/neighbor hooks).
//!
//! Shape ids: 0 E-W flat, 1 N-S flat, 2 asc-E, 3 asc-W, 4 asc-N, 5
//! asc-S, 6-9 corners (S+E, S+W, N+W, N+E). Connection endpoints are
//! absolute cells `(x, y, z)`; slope ends sit one higher.
//!
//! Mapping notes (var-for-var against `MinecartTrackLogic.java`):
//! - `shape_conns` = `func_593_a` endpoint table.
//! - `resolve` = `func_595_a` (same cell, +1y, -1y priority).
//! - `prune` = `func_591_b`, `connects_to` = `func_590_b`,
//!   `can_connect` = `func_597_c` (including its always-true tail
//!   branch — mirrored, not "fixed").
//! - `refresh` = `func_596_a` (both the plain and the `force_join`
//!   corner tables), `join_shape` = `func_598_d` with the slope
//!   upgrades on both paths.
//! - Meta writes use silent `set_block_meta` (queues the client packet
//!   via `block_updates` but never re-notifies): the propagation loop
//!   is explicit like vanilla's, so no refresh storms are possible.
//!   The `powered` flag only picks corner variants on ties, like the
//!   vanilla `isBlockIndirectlyGettingPowered` argument.
//!
//! Hooked from `block_added` (place) and `neighbor_changed` (support
//! loss drops the rail as an item, like vanilla) in `world/blocks.rs`.

use crate::world::World;

/// Connection endpoints for a shape id (mirrors `func_593_a`).
fn shape_conns(x: i32, y: i32, z: i32, meta: u8) -> [(i32, i32, i32); 2] {
    match meta {
        // NOTE: vanilla's meta 0 connects along Z and meta 1 along X
        // (the names are swapped vs intuition); preserved exactly —
        // the cart physics reads the same table.
        0 => [(x, y, z - 1), (x, y, z + 1)],
        1 => [(x - 1, y, z), (x + 1, y, z)],
        2 => [(x - 1, y, z), (x + 1, y + 1, z)],
        3 => [(x - 1, y + 1, z), (x + 1, y, z)],
        4 => [(x, y + 1, z - 1), (x, y, z + 1)],
        5 => [(x, y, z - 1), (x, y + 1, z + 1)],
        6 => [(x + 1, y, z), (x, y, z + 1)],
        7 => [(x - 1, y, z), (x, y, z + 1)],
        8 => [(x - 1, y, z), (x, y, z - 1)],
        _ => [(x + 1, y, z), (x, y, z - 1)],
    }
}

fn is_rail(w: &World, x: i32, y: i32, z: i32) -> bool {
    w.get_block_id(x, y, z) == 66
}

/// Logic view of one rail cell: position + live endpoints.
struct RailLogic {
    x: i32,
    y: i32,
    z: i32,
    conns: Vec<(i32, i32, i32)>,
}

impl RailLogic {
    fn of(w: &World, x: i32, y: i32, z: i32) -> Self {
        let meta = w.get_block_meta(x, y, z);
        let conns = shape_conns(x, y, z, meta).to_vec();
        Self { x, y, z, conns }
    }

    /// Resolve a rail logic at a cell (mirrors `func_595_a`).
    fn resolve(w: &World, x: i32, y: i32, z: i32) -> Option<Self> {
        if is_rail(w, x, y, z) {
            Some(Self::of(w, x, y, z))
        } else if is_rail(w, x, y + 1, z) {
            Some(Self::of(w, x, y + 1, z))
        } else if is_rail(w, x, y - 1, z) {
            Some(Self::of(w, x, y - 1, z))
        } else {
            None
        }
    }

    /// Drop endpoints whose far side doesn't link back (mirrors `func_591_b`).
    fn prune(&mut self, w: &World) {
        let mut i = 0;
        while i < self.conns.len() {
            let (ex, ey, ez) = self.conns[i];
            match Self::resolve(w, ex, ey, ez) {
                Some(other) if other.connects_to(self.x, self.z) => {
                    self.conns[i] = (other.x, other.y, other.z);
                    i += 1;
                }
                _ => {
                    self.conns.remove(i);
                }
            }
        }
    }

    /// X/Z membership test (mirrors `func_590_b` / `func_599_b`).
    fn connects_to(&self, x: i32, z: i32) -> bool {
        self.conns.iter().any(|(cx, _, cz)| *cx == x && *cz == z)
    }

    /// Whether `other` may join this rail (mirrors `func_597_c`,
    /// dead always-true tail included).
    fn can_connect(&self, ox: i32, oy: i32, oz: i32) -> bool {
        if self.connects_to(ox, oz) {
            return true;
        }
        if self.conns.len() == 2 {
            return false;
        }
        if self.conns.is_empty() {
            return true;
        }
        let (fx, fy, _fz) = self.conns[0];
        let _ = (fx, fy);
        // Vanilla: `return var1.y == this.y && first.y == this.y ? true : true`
        let _ = (oy, self.y);
        true
    }
}

/// Neighbor-connection probe (mirrors `func_592_c`).
fn neighbor_ok(w: &World, from_x: i32, from_y: i32, from_z: i32, nx: i32, ny: i32, nz: i32) -> bool {
    match RailLogic::resolve(w, nx, ny, nz) {
        None => false,
        Some(mut logic) => {
            logic.prune(w);
            logic.can_connect(from_x, from_y, from_z)
        }
    }
}

/// Pick a shape from four neighbor flags (mirrors the `func_596_a`
/// tables; `force_join` selects the corner preference on ties).
fn pick_shape(n: bool, s: bool, w: bool, e: bool, force_join: bool) -> i8 {
    let mut shape: i8 = -1;
    if (n || s) && !w && !e {
        shape = 0;
    }
    if (w || e) && !n && !s {
        shape = 1;
    }
    if s && e && !n && !w {
        shape = 6;
    }
    if s && w && !n && !e {
        shape = 7;
    }
    if n && w && !s && !e {
        shape = 8;
    }
    if n && e && !s && !w {
        shape = 9;
    }
    if shape == -1 {
        if n || s {
            shape = 0;
        }
        if w || e {
            shape = 1;
        }
        if force_join {
            if s && e {
                shape = 6;
            }
            if w && s {
                shape = 7;
            }
            if e && n {
                shape = 9;
            }
            if n && w {
                shape = 8;
            }
        } else {
            if n && w {
                shape = 8;
            }
            if e && n {
                shape = 9;
            }
            if w && s {
                shape = 7;
            }
            if s && e {
                shape = 6;
            }
        }
    }
    shape
}

/// Slope upgrade for flat picks (mirrors both `func_596_a` tails).
fn slope_upgrade(w: &World, x: i32, y: i32, z: i32, shape: i8) -> i8 {
    let mut shape = shape;
    if shape == 0 {
        if is_rail(w, x, y + 1, z - 1) {
            shape = 4;
        }
        if is_rail(w, x, y + 1, z + 1) {
            shape = 5;
        }
    }
    if shape == 1 {
        if is_rail(w, x + 1, y + 1, z) {
            shape = 2;
        }
        if is_rail(w, x - 1, y + 1, z) {
            shape = 3;
        }
    }
    if shape < 0 {
        shape = 0;
    }
    shape
}

/// Recompute one rail's shape and propagate to its neighbors
/// (mirrors `func_596_a`). Bounded: touches the cell + ≤2 neighbors.
pub fn track_refresh(w: &mut World, x: i32, y: i32, z: i32, force_join: bool) {
    if w.get_block_id(x, y, z) != 66 {
        return;
    }
    let n = neighbor_ok(w, x, y, z, x, y, z - 1);
    let s = neighbor_ok(w, x, y, z, x, y, z + 1);
    let ww = neighbor_ok(w, x, y, z, x - 1, y, z);
    let e = neighbor_ok(w, x, y, z, x + 1, y, z);
    let mut shape = pick_shape(n, s, ww, e, force_join);
    shape = slope_upgrade(w, x, y, z, shape);
    if w.get_block_meta(x, y, z) != shape as u8 {
        w.set_block_meta(x, y, z, shape as u8);
    }
    // Propagate: neighbors that accept us recompute via `join_shape`
    // (mirrors the `func_598_d` fan-out; silent writes, no cascade).
    let ends = shape_conns(x, y, z, shape as u8);
    for (ex, ey, ez) in ends {
        let mut other = match RailLogic::resolve(w, ex, ey, ez) {
            Some(o) => o,
            None => continue,
        };
        other.prune(w);
        if !other.connects_to(x, z) && !other.can_connect(x, y, z) {
            continue;
        }
        join_shape(w, &mut other, x, y, z);
    }
}

/// Recompute a neighbor's shape with `(x, y, z)` joined in
/// (mirrors `func_598_d`).
fn join_shape(w: &mut World, other: &mut RailLogic, x: i32, y: i32, z: i32) {
    other.conns.push((x, y, z));
    let has = |ox: i32, oz: i32| other.connects_to(ox, oz);
    let n = has(other.x, other.z - 1);
    let s = has(other.x, other.z + 1);
    let ww = has(other.x - 1, other.z);
    let e = has(other.x + 1, other.z);
    let mut shape: i8 = -1;
    if n || s {
        shape = 0;
    }
    if ww || e {
        shape = 1;
    }
    if s && e && !n && !ww {
        shape = 6;
    }
    if s && ww && !n && !e {
        shape = 7;
    }
    if n && ww && !s && !e {
        shape = 8;
    }
    if n && e && !s && !ww {
        shape = 9;
    }
    if shape == 0 {
        if is_rail(w, other.x, other.y + 1, other.z - 1) {
            shape = 4;
        }
        if is_rail(w, other.x, other.y + 1, other.z + 1) {
            shape = 5;
        }
    }
    if shape == 1 {
        if is_rail(w, other.x + 1, other.y + 1, other.z) {
            shape = 2;
        }
        if is_rail(w, other.x - 1, other.y + 1, other.z) {
            shape = 3;
        }
    }
    if shape < 0 {
        shape = 0;
    }
    if w.get_block_meta(other.x, other.y, other.z) != shape as u8 {
        w.set_block_meta(other.x, other.y, other.z, shape as u8);
    }
}

/// Rail support rule (mirrors `BlockMinecartTrack.onNeighborBlockChange`
/// and `canPlaceBlockAt`): solid floor, plus the uphill side block for
/// ascending shapes 2-5.
pub fn track_supported(w: &World, x: i32, y: i32, z: i32) -> bool {
    if !w.block_allows_attachment(x, y - 1, z) {
        return false;
    }
    match w.get_block_meta(x, y, z) {
        2 => w.block_allows_attachment(x + 1, y, z),
        3 => w.block_allows_attachment(x - 1, y, z),
        4 => w.block_allows_attachment(x, y, z - 1),
        5 => w.block_allows_attachment(x, y, z + 1),
        _ => true,
    }
}

/// Neighbor hook for rails: drop-as-item when unsupported, else
/// refresh connections (placement and breaks nearby reshape the line).
pub fn track_neighbor(w: &mut World, x: i32, y: i32, z: i32) {
    if w.get_block_id(x, y, z) != 66 {
        return;
    }
    if !track_supported(w, x, y, z) {
        w.drop_block_as_item(x, y, z);
        w.apply_set_notify(x, y, z, 0);
        return;
    }
    let powered = w.is_block_powered(x, y, z);
    track_refresh(w, x, y, z, powered);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::world::World;

    fn rail_world() -> World {
        let mut w = World::new(5);
        let mut c = Chunk::new(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                c.set_block_id(x, 63, z, 1);
            }
        }
        c.generate_height_map();
        w.insert_chunk(c);
        w
    }

    fn lay(w: &mut World, x: i32, z: i32) {
        w.apply_set_notify(x, 64, z, 66);
    }

    #[test]
    fn straight_line_connects() {
        let mut w = rail_world();
        lay(&mut w, 4, 4);
        lay(&mut w, 5, 4);
        lay(&mut w, 6, 4);
        // East-west line along X must settle on shape 1 (vanilla ids).
        assert_eq!(w.get_block_meta(5, 64, 4), 1);
    }

    #[test]
    fn corner_forms_on_l() {
        let mut w = rail_world();
        lay(&mut w, 4, 4);
        lay(&mut w, 5, 4);
        lay(&mut w, 5, 5);
        // South + west open at (5,64,4): shape 7 per the vanilla table
        // (s && w, 6 is south + east).
        assert_eq!(w.get_block_meta(5, 64, 4), 7);
    }

    #[test]
    fn unsupported_rail_drops() {
        let mut w = rail_world();
        // Knock out the floor first: rails need support below.
        w.apply_set_notify(7, 63, 7, 0);
        assert_eq!(w.get_block_id(7, 63, 7), 0);
        w.apply_set_notify(7, 64, 7, 66);
        // Neighbor pass drops it (no support).
        track_neighbor(&mut w, 7, 64, 7);
        assert_eq!(w.get_block_id(7, 64, 7), 0);
    }
}
