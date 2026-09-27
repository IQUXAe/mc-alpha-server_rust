use crate::random::JavaRandom;
use crate::block::table::block_properties_get;
use crate::world::material_of;
use super::BlockAccess;

/// True when any horizontal neighbor of a cactus cell is solid
/// (mirrors the `func_216_a` side checks in `BlockCactus.canBlockStay`).
fn cactus_blocked(accessor: &mut dyn BlockAccess, x: i32, y: i32, z: i32) -> bool {
    for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let id = accessor.get_block_id(x + dx, y, z + dz);
        if material_of(block_properties_get(id as u32).material).is_solid() {
            return true;
        }
    }
    false
}

// ============================================
// WorldGenLakes
// ============================================
pub struct WorldGenLakes {
    liquid_block_id: u8,
}

impl WorldGenLakes {
    pub fn new(block_id: u8) -> Self {
        Self { liquid_block_id: block_id }
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, mut x: i32, mut y: i32, mut z: i32) -> bool {
        x -= 8;
        z -= 8;
        while y > 0 && accessor.get_block_id(x, y, z) == 0 {
            y -= 1;
        }
        y -= 4;

        let mut arr = [false; 2048];
        let num_blobs = rand.next_int_bound(4) + 4;

        for _ in 0..num_blobs {
            let rx = rand.next_double() * 6.0 + 3.0;
            let ry = rand.next_double() * 4.0 + 2.0;
            let rz = rand.next_double() * 6.0 + 3.0;
            let cx = rand.next_double() * (16.0 - rx - 2.0) + 1.0 + rx / 2.0;
            let cy = rand.next_double() * (8.0 - ry - 4.0) + 2.0 + ry / 2.0;
            let cz = rand.next_double() * (16.0 - rz - 2.0) + 1.0 + rz / 2.0;

            for bx in 1..15 {
                for bz in 1..15 {
                    for by in 1..7 {
                        let dx = ((bx as f64) - cx) / (rx / 2.0);
                        let dy = ((by as f64) - cy) / (ry / 2.0);
                        let dz = ((bz as f64) - cz) / (rz / 2.0);
                        if dx * dx + dy * dy + dz * dz < 1.0 {
                            arr[(bx * 16 + bz) * 8 + by] = true;
                        }
                    }
                }
            }
        }

        // Check edges for liquids/solids
        for bx in 0..16 {
            let ibx = bx as i32;
            for bz in 0..16 {
                let ibz = bz as i32;
                for by in 0..8 {
                    let iby = by as i32;
                    let idx = (bx * 16 + bz) * 8 + by;
                    let is_edge = !arr[idx] &&
                        ((bx < 15 && arr[((bx + 1) * 16 + bz) * 8 + by]) ||
                         (bx > 0 && arr[((bx - 1) * 16 + bz) * 8 + by]) ||
                         (bz < 15 && arr[(bx * 16 + bz + 1) * 8 + by]) ||
                         (bz > 0 && arr[(bx * 16 + (bz - 1)) * 8 + by]) ||
                         (by < 7 && arr[(bx * 16 + bz) * 8 + by + 1]) ||
                         (by > 0 && arr[(bx * 16 + bz) * 8 + (by - 1)]));
                    if is_edge {
                        let bid = accessor.get_block_id(x + ibx, y + iby, z + ibz);
                        if by >= 4 && (bid == 8 || bid == 9 || bid == 10 || bid == 11) {
                            return false;
                        }
                        let solid = bid != 0 && bid != 8 && bid != 9 && bid != 10 && bid != 11;
                        if by < 4 && !solid && accessor.get_block_id(x + ibx, y + iby, z + ibz) != self.liquid_block_id {
                            return false;
                        }
                    }
                }
            }
        }

        // Place blocks
        for bx in 0..16 {
            let ibx = bx as i32;
            for bz in 0..16 {
                let ibz = bz as i32;
                for by in 0..8 {
                    let iby = by as i32;
                    if arr[(bx * 16 + bz) * 8 + by] {
                        accessor.set_block_id(x + ibx, y + iby, z + ibz, if by >= 4 { 0 } else { self.liquid_block_id });
                    }
                }
            }
        }

        // Grass conversion (only when exposed to sky light > 0, matching WorldGenLakes.java:79)
        for bx in 0..16 {
            let ibx = bx as i32;
            for bz in 0..16 {
                let ibz = bz as i32;
                for by in 4..8 {
                    let iby = by as i32;
                    if arr[(bx * 16 + bz) * 8 + by]
                        && accessor.get_block_id(x + ibx, y + iby - 1, z + ibz) == 3
                        && column_sky_light(&mut *accessor, x + ibx, y + iby, z + ibz) > 0
                    {
                        accessor.set_block_id(x + ibx, y + iby - 1, z + ibz, 2); // grass
                    }
                }
            }
        }

        true
    }
}

// ============================================
// WorldGenFlowers
// ============================================
pub struct WorldGenFlowers {
    plant_block_id: u8,
}

impl WorldGenFlowers {
    pub fn new(block_id: u8) -> Self {
        Self { plant_block_id: block_id }
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        for _ in 0..64 {
            let fx = x + rand.next_int_bound(8) - rand.next_int_bound(8);
            let fy = y + rand.next_int_bound(4) - rand.next_int_bound(4);
            let fz = z + rand.next_int_bound(8) - rand.next_int_bound(8);
            // Java resolves out-of-range cells to air (no placement); the
            // canvas has no such guard, so skip explicitly (fy < 1 also
            // keeps the soil read below in range).
            if !(1..=127).contains(&fy) {
                continue;
            }
            if accessor.get_block_id(fx, fy, fz) == 0 && flower_soil_ok(&mut *accessor, self.plant_block_id, fx, fy, fz) {
                accessor.set_block_id(fx, fy, fz, self.plant_block_id);
            }
        }
        true
    }
}

/// Soil rule (mirrors `BlockFlower.canBlockStay` minus the light half:
/// skylight is not computed yet during populate, so a light check here
/// would veto every flower; the block tick pops wrongly lit plants right
/// after, like vanilla self-correction). Flowers (37/38) need
/// grass/dirt/tilled soil; mushrooms (39/40) need an attachable block
/// below (`field_540_p`, read as the table flag off the canvas) AND shade:
/// vanilla populate sees real skylight, so sunlit cells read ~15 (>13)
/// and reject. The canvas has no light yet, so shade is approximated by
/// the heightmap: a cell at/above the top is sunlit (reject), below it is
/// shaded (accept, like the 15-opacity-minus-layers vanilla outcome).
fn column_sky_light(accessor: &mut dyn BlockAccess, x: i32, y: i32, z: i32) -> i32 {
    let mut sky = 15;
    for cy in (y..128).rev() {
        let id = accessor.get_block_id(x, cy, z);
        let op = block_properties_get(id as u32).light_opacity;
        if op >= 15 {
            return 0;
        }
        sky = (sky - op).max(0);
    }
    sky
}

fn flower_soil_ok(accessor: &mut dyn BlockAccess, plant_id: u8, x: i32, y: i32, z: i32) -> bool {
    let below = accessor.get_block_id(x, y - 1, z);
    match plant_id {
        37 | 38 => {
            (below == 2 || below == 3 || below == 60)
                && (y >= accessor.get_height_value(x, z) || column_sky_light(accessor, x, y, z) >= 8)
        }
        39 | 40 => {
            if below == 0 || !block_properties_get(below as u32).allows_attachment {
                return false;
            }
            y < accessor.get_height_value(x, z)
        }
        _ => false,
    }
}

// ============================================
// WorldGenReed
// ============================================
pub struct WorldGenReed;

impl Default for WorldGenReed {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldGenReed {
    pub fn new() -> Self {
        Self
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        for _ in 0..20 {
            let rx = x + rand.next_int_bound(4) - rand.next_int_bound(4);
            let ry = y;
            let rz = z + rand.next_int_bound(4) - rand.next_int_bound(4);
            if accessor.get_block_id(rx, ry, rz) == 0 {
                let has_water = accessor.get_block_id(rx - 1, ry - 1, rz) == 8 || accessor.get_block_id(rx - 1, ry - 1, rz) == 9 ||
                                accessor.get_block_id(rx + 1, ry - 1, rz) == 8 || accessor.get_block_id(rx + 1, ry - 1, rz) == 9 ||
                                accessor.get_block_id(rx, ry - 1, rz - 1) == 8 || accessor.get_block_id(rx, ry - 1, rz - 1) == 9 ||
                                accessor.get_block_id(rx, ry - 1, rz + 1) == 8 || accessor.get_block_id(rx, ry - 1, rz + 1) == 9;
                if has_water {
                    let step1 = rand.next_int_bound(3) + 1;
                    let height = 2 + rand.next_int_bound(step1);
                    for h in 0..height {
                        // Java ignores out-of-range sets; skip explicitly.
                        if !(1..=127).contains(&(ry + h)) {
                            break;
                        }
                        let below = accessor.get_block_id(rx, ry + h - 1, rz);
                        if h == 0 {
                            // `BlockReed.canPlaceBlockAt`: grass or dirt
                            // only (sand never hosts reed).
                            if below != 2 && below != 3 {
                                break;
                            }
                        } else {
                            if below != 83 { // reed
                                break;
                            }
                        }
                        if accessor.get_block_id(rx, ry + h, rz) == 0 {
                            accessor.set_block_id(rx, ry + h, rz, 83); // reed
                        }
                    }
                }
            }
        }
        true
    }
}

// ============================================
// WorldGenCactus
// ============================================
pub struct WorldGenCactus;

impl Default for WorldGenCactus {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldGenCactus {
    pub fn new() -> Self {
        Self
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        for _ in 0..10 {
            let cx = x + rand.next_int_bound(8) - rand.next_int_bound(8);
            let cy = y + rand.next_int_bound(4) - rand.next_int_bound(4);
            let cz = z + rand.next_int_bound(8) - rand.next_int_bound(8);
            if accessor.get_block_id(cx, cy, cz) == 0 {
                let step1 = rand.next_int_bound(3) + 1;
                let height = 1 + rand.next_int_bound(step1);
                for h in 0..height {
                    // Java ignores out-of-range sets; skip explicitly.
                    if !(1..=127).contains(&(cy + h)) {
                        break;
                    }
                    let below = accessor.get_block_id(cx, cy + h - 1, cz);
                    if h == 0 {
                        if below != 12 { // sand
                            break;
                        }
                    } else {
                        if below != 81 { // cactus
                            break;
                        }
                    }
                    // `BlockCactus.canBlockStay`: no *solid* (`func_216_a`)
                    // neighbor — water and air are both fine.
                    if cactus_blocked(&mut *accessor, cx, cy + h, cz) {
                        break;
                    }
                    if accessor.get_block_id(cx, cy + h, cz) == 0 {
                        accessor.set_block_id(cx, cy + h, cz, 81); // cactus
                    }
                }
            }
        }
        true
    }
}

// ============================================
// WorldGenPumpkin
// ============================================
pub struct WorldGenPumpkin;

impl Default for WorldGenPumpkin {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldGenPumpkin {
    pub fn new() -> Self {
        Self
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        for _ in 0..64 {
            let px = x + rand.next_int_bound(8) - rand.next_int_bound(8);
            let py = y + rand.next_int_bound(4) - rand.next_int_bound(4);
            let pz = z + rand.next_int_bound(8) - rand.next_int_bound(8);
            if accessor.get_block_id(px, py, pz) == 0 && accessor.get_block_id(px, py - 1, pz) == 2 {
                accessor.set_block_id(px, py, pz, 86); // pumpkin
                accessor.set_block_meta(px, py, pz, rand.next_int_bound(4) as u8);
            }
        }
        true
    }
}

// ============================================
// WorldGenLiquids
// ============================================
pub struct WorldGenLiquids {
    liquid_block_id: u8,
}

impl WorldGenLiquids {
    pub fn new(block_id: u8) -> Self {
        Self { liquid_block_id: block_id }
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, _rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        if accessor.get_block_id(x, y + 1, z) != 1 { return false; } // stone above
        if accessor.get_block_id(x, y - 1, z) != 1 { return false; } // stone below
        let current = accessor.get_block_id(x, y, z);
        if current != 0 && current != 1 { return false; }

        let mut stone_count = 0;
        if accessor.get_block_id(x - 1, y, z) == 1 { stone_count += 1; }
        if accessor.get_block_id(x + 1, y, z) == 1 { stone_count += 1; }
        if accessor.get_block_id(x, y, z - 1) == 1 { stone_count += 1; }
        if accessor.get_block_id(x, y, z + 1) == 1 { stone_count += 1; }

        let mut air_count = 0;
        if accessor.get_block_id(x - 1, y, z) == 0 { air_count += 1; }
        if accessor.get_block_id(x + 1, y, z) == 0 { air_count += 1; }
        if accessor.get_block_id(x, y, z - 1) == 0 { air_count += 1; }
        if accessor.get_block_id(x, y, z + 1) == 0 { air_count += 1; }

        if stone_count == 3 && air_count == 1 {
            accessor.set_block_id(x, y, z, self.liquid_block_id);
            let mut budget = 256usize;
            flow_liquid_instant(accessor, self.liquid_block_id, x, y, z, &mut budget);
        }
        true
    }
}

fn liquid_is_same(id: u8, liquid_block_id: u8) -> bool {
    id == liquid_block_id || id == liquid_block_id + 1
}

fn liquid_blocks_flow(id: u8) -> bool {
    if matches!(id, 64 | 71 | 63 | 65 | 83) {
        return true;
    }
    if id == 0 {
        return false;
    }
    crate::world::material_of(block_properties_get(id as u32).material).blocks_movement()
}

fn liquid_can_displace(accessor: &mut dyn BlockAccess, liquid_block_id: u8, x: i32, y: i32, z: i32) -> bool {
    if !(0..128).contains(&y) {
        return false;
    }
    let id = accessor.get_block_id(x, y, z);
    if liquid_is_same(id, liquid_block_id) || id == 10 || id == 11 {
        return false;
    }
    !liquid_blocks_flow(id)
}

fn liquid_flow_cost(
    accessor: &mut dyn BlockAccess,
    liquid_block_id: u8,
    x: i32,
    y: i32,
    z: i32,
    dist: i32,
    from_dir: usize,
) -> i32 {
    const DIRS: [(i32, i32, usize); 4] = [(-1, 0, 1), (1, 0, 0), (0, -1, 3), (0, 1, 2)];
    let mut best = 1000;
    for (dir, &(dx, dz, opp)) in DIRS.iter().enumerate() {
        if dir == from_dir {
            continue;
        }
        let (nx, nz) = (x + dx, z + dz);
        let nid = accessor.get_block_id(nx, y, nz);
        if !liquid_blocks_flow(nid)
            && (!liquid_is_same(nid, liquid_block_id) || accessor.get_block_meta(nx, y, nz) != 0)
        {
            if y > 0 && !liquid_blocks_flow(accessor.get_block_id(nx, y - 1, nz)) {
                return dist;
            }
            if dist < 4 {
                let c = liquid_flow_cost(accessor, liquid_block_id, nx, y, nz, dist + 1, opp);
                if c < best {
                    best = c;
                }
            }
        }
    }
    best
}

fn liquid_optimal_dirs(
    accessor: &mut dyn BlockAccess,
    liquid_block_id: u8,
    x: i32,
    y: i32,
    z: i32,
) -> [bool; 4] {
    const DIRS: [(i32, i32, usize); 4] = [(-1, 0, 1), (1, 0, 0), (0, -1, 3), (0, 1, 2)];
    let mut costs = [1000i32; 4];
    for (dir, &(dx, dz, opp)) in DIRS.iter().enumerate() {
        let (nx, nz) = (x + dx, z + dz);
        let nid = accessor.get_block_id(nx, y, nz);
        if !liquid_blocks_flow(nid)
            && (!liquid_is_same(nid, liquid_block_id) || accessor.get_block_meta(nx, y, nz) != 0)
        {
            if y > 0 && !liquid_blocks_flow(accessor.get_block_id(nx, y - 1, nz)) {
                costs[dir] = 0;
            } else {
                costs[dir] = liquid_flow_cost(accessor, liquid_block_id, nx, y, nz, 1, opp);
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

fn flow_liquid_instant(
    accessor: &mut dyn BlockAccess,
    liquid_block_id: u8,
    x: i32,
    y: i32,
    z: i32,
    budget: &mut usize,
) {
    if *budget == 0 || !(0..128).contains(&y) {
        return;
    }
    *budget -= 1;
    let meta = accessor.get_block_meta(x, y, z) as i32;
    // Stabilize current cell into stationary id (9 or 11) with current meta.
    accessor.set_block_id(x, y, z, liquid_block_id + 1);
    accessor.set_block_meta(x, y, z, meta as u8);

    if y > 0 && liquid_can_displace(accessor, liquid_block_id, x, y - 1, z) {
        let down_meta = if meta >= 8 { meta } else { meta + 8 };
        accessor.set_block_id(x, y - 1, z, liquid_block_id);
        accessor.set_block_meta(x, y - 1, z, down_meta as u8);
        flow_liquid_instant(accessor, liquid_block_id, x, y - 1, z, budget);
    } else if meta >= 0 && (meta == 0 || (y > 0 && liquid_blocks_flow(accessor.get_block_id(x, y - 1, z)))) {
        let step = if liquid_block_id == 10 { 2 } else { 1 };
        let next_meta = if meta >= 8 { 1 } else { meta + step };
        if next_meta >= 8 {
            return;
        }
        let dirs = liquid_optimal_dirs(accessor, liquid_block_id, x, y, z);
        const OFFSETS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
        for (i, &(dx, dz)) in OFFSETS.iter().enumerate() {
            if dirs[i] && liquid_can_displace(accessor, liquid_block_id, x + dx, y, z + dz) {
                accessor.set_block_id(x + dx, y, z + dz, liquid_block_id);
                accessor.set_block_meta(x + dx, y, z + dz, next_meta as u8);
                flow_liquid_instant(accessor, liquid_block_id, x + dx, y, z + dz, budget);
            }
        }
    }
}

// ============================================
// WorldGenDungeons
// ============================================
pub struct WorldGenDungeons;

impl Default for WorldGenDungeons {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldGenDungeons {
    pub fn new() -> Self {
        Self
    }

    pub fn generate(&self, accessor: &mut dyn BlockAccess, rand: &mut JavaRandom, x: i32, y: i32, z: i32) -> bool {
        let half_x = rand.next_int_bound(2) + 2;
        let half_z = rand.next_int_bound(2) + 2;
        let height = 3;
        let mut solid_count = 0;

        // Check walls
        for dx in (x - half_x - 1)..=(x + half_x + 1) {
            for dy in (y - 1)..=(y + height + 1) {
                for dz in (z - half_z - 1)..=(z + half_z + 1) {
                    let bid = accessor.get_block_id(dx, dy, dz);
                    let solid = bid != 0 && bid != 8 && bid != 9 && bid != 10 && bid != 11;
                    if dy == y - 1 && !solid { return false; }
                    if dy == y + height + 1 && !solid { return false; }
                    if (dx == x - half_x - 1 || dx == x + half_x + 1 || dz == z - half_z - 1 || dz == z + half_z + 1) &&
                        dy == y && accessor.get_block_id(dx, dy, dz) == 0 && accessor.get_block_id(dx, dy + 1, dz) == 0
                    {
                        solid_count += 1;
                    }
                }
            }
        }

        if (1..=5).contains(&solid_count) {
            // Hollow out and place walls
            for dx in (x - half_x - 1)..=(x + half_x + 1) {
                for dy in (y - 1..=y + height).rev() {
                    for dz in (z - half_z - 1)..=(z + half_z + 1) {
                        if dx != x - half_x - 1 && dy != y - 1 && dz != z - half_z - 1 &&
                            dx != x + half_x + 1 && dy != y + height + 1 && dz != z + half_z + 1
                        {
                            accessor.set_block_id(dx, dy, dz, 0); // air
                        } else {
                            let below_id = if dy > 0 {
                                accessor.get_block_id(dx, dy - 1, dz)
                            } else {
                                0
                            };
                            let below_solid = below_id != 0
                                && below_id != 8
                                && below_id != 9
                                && below_id != 10
                                && below_id != 11;
                            let bid = accessor.get_block_id(dx, dy, dz);
                            let solid = bid != 0 && bid != 8 && bid != 9 && bid != 10 && bid != 11;
                            if dy >= 0 && !below_solid {
                                accessor.set_block_id(dx, dy, dz, 0);
                            } else if solid {
                                if dy == y - 1 && rand.next_int_bound(4) != 0 {
                                    accessor.set_block_id(dx, dy, dz, 48); // mossy cobblestone
                                } else {
                                    accessor.set_block_id(dx, dy, dz, 4); // cobblestone
                                }
                            }
                        }
                    }
                }
            }
            // Place chests (up to 2 attempts)
            for _ in 0..2 {
                for _ in 0..3 {
                    let cx = x + rand.next_int_bound(half_x * 2 + 1) - half_x;
                    let cz = z + rand.next_int_bound(half_z * 2 + 1) - half_z;
                    if accessor.get_block_id(cx, y, cz) == 0 {
                        let mut solid_count = 0;
                        if accessor.is_block_solid(cx - 1, y, cz) { solid_count += 1; }
                        if accessor.is_block_solid(cx + 1, y, cz) { solid_count += 1; }
                        if accessor.is_block_solid(cx, y, cz - 1) { solid_count += 1; }
                        if accessor.is_block_solid(cx, y, cz + 1) { solid_count += 1; }

                        if solid_count == 1 {
                            accessor.set_block_id(cx, y, cz, 54); // chest block

                            // Chest loot (Java WorldGenDungeons: 8 rolls of
                            // func_434_a into random slots).
                            for _ in 0..8 {
                                if let Some((item, count)) = dungeon_loot(rand) {
                                    let slot = rand.next_int_bound(27);
                                    accessor.push_dungeon_loot(cx, y, cz, slot, item, count);
                                }
                            }
                            break;
                        }
                    }
                }
            }

            // Place spawner
            accessor.set_block_id(x, y, z, 52); // mob spawner
            let kind = dungeon_spawner_kind(rand);
            accessor.push_dungeon_loot(x, y, z, -1, kind, 0);
            true
        } else {
            false
        }
    }
}

/// Dungeon loot (Java WorldGenDungeons.func_434_a): (shiftedIndex, count).
/// Draw order matches Java exactly (quantity rolls only on taken branches).
fn dungeon_loot(rand: &mut JavaRandom) -> Option<(i32, i32)> {
    match rand.next_int_bound(11) {
        0 => Some((329, 1)),                          // saddle
        1 => Some((265, rand.next_int_bound(4) + 1)), // iron ingots
        2 => Some((297, 1)),                          // bread
        3 => Some((296, rand.next_int_bound(4) + 1)), // wheat
        4 => Some((289, rand.next_int_bound(4) + 1)), // gunpowder
        5 => Some((287, rand.next_int_bound(4) + 1)), // string
        6 => Some((325, 1)),                          // bucket
        7 => {
            if rand.next_int_bound(100) == 0 {
                Some((322, 1)) // golden apple
            } else {
                None
            }
        }
        8 => {
            if rand.next_int_bound(2) == 0 {
                Some((331, rand.next_int_bound(4) + 1)) // redstone
            } else {
                None
            }
        }
        9 => {
            if rand.next_int_bound(10) == 0 {
                Some((2256 + rand.next_int_bound(2), 1)) // record 13/cat
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Spawner kind roll (Java func_433_b: 0 skeleton, 1-2 zombie, 3 spider).
/// Returns 51/54/52 type ids for future spawner tiles; today only the RNG
/// burn matters.
fn dungeon_spawner_kind(rand: &mut JavaRandom) -> i32 {
    match rand.next_int_bound(4) {
        0 => 51,
        3 => 52,
        _ => 54,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeAccess {
        cells: HashMap<(i32, i32, i32), u8>,
        metas: HashMap<(i32, i32, i32), u8>,
    }

    impl FakeAccess {
        fn new() -> Self {
            Self { cells: HashMap::new(), metas: HashMap::new() }
        }
        fn flat_grass(&mut self) {
            self.cells.clear();
            self.metas.clear();
            for x in -16..16 {
                for z in -16..16 {
                    self.set(x, 63, z, 2); // grass
                }
            }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, id: u8) {
            self.cells.insert((x, y, z), id);
        }
        fn planted(&self, id: u8) -> usize {
            self.cells.values().filter(|v| **v == id).count()
        }
    }

    impl BlockAccess for FakeAccess {
        fn get_block_id(&mut self, x: i32, y: i32, z: i32) -> u8 {
            self.cells.get(&(x, y, z)).copied().unwrap_or(0)
        }
        fn set_block_id(&mut self, x: i32, y: i32, z: i32, id: u8) {
            self.cells.insert((x, y, z), id);
        }
        fn get_block_meta(&mut self, x: i32, y: i32, z: i32) -> u8 {
            self.metas.get(&(x, y, z)).copied().unwrap_or(0)
        }
        fn set_block_meta(&mut self, x: i32, y: i32, z: i32, m: u8) {
            self.metas.insert((x, y, z), m & 0xF);
        }
        fn allows_attachment(&mut self, _x: i32, _y: i32, _z: i32) -> bool {
            false
        }
        fn is_block_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
            self.get_block_id(x, y, z) != 0
        }
        fn get_height_value(&mut self, x: i32, z: i32) -> i32 {
            for y in (0..128).rev() {
                if self.get_block_id(x, y, z) != 0 {
                    return y + 1;
                }
            }
            0
        }
    }

    #[test]
    fn test_mushroom_rejects_sunlit_ground() {
        // Open grass flat: vanilla reads skylight ~15 (>13) and plants no
        // mushrooms; shaded cells (canopy above) accept.
        let mut acc = FakeAccess::new();
        acc.flat_grass();
        let mut rand = JavaRandom::new(1234);
        WorldGenFlowers::new(39).generate(&mut acc, &mut rand, 0, 64, 0);
        WorldGenFlowers::new(40).generate(&mut acc, &mut rand, 0, 64, 0);
        assert_eq!(acc.planted(39), 0, "brown mushrooms must not plant on sunlit ground");
        assert_eq!(acc.planted(40), 0, "red mushrooms must not plant on sunlit ground");
        // Same flat with a leaf canopy overhead: shade accepts.
        for x in -16..16 {
            for z in -16..16 {
                acc.set(x, 70, z, 18);
            }
        }
        let mut rand = JavaRandom::new(1234);
        WorldGenFlowers::new(39).generate(&mut acc, &mut rand, 0, 64, 0);
        assert!(acc.planted(39) > 0, "shaded mushrooms must plant");
    }

    #[test]
    fn test_flowers_plant_on_open_grass() {
        let mut acc = FakeAccess::new();
        acc.flat_grass();
        let mut rand = JavaRandom::new(42);
        WorldGenFlowers::new(37).generate(&mut acc, &mut rand, 0, 64, 0);
        WorldGenFlowers::new(38).generate(&mut acc, &mut rand, 0, 64, 0);
        assert!(acc.planted(37) > 0, "yellow flowers must plant on open grass");
        assert!(acc.planted(38) > 0, "red flowers must plant on open grass");

        // Underground dark cave (opaque stone ceiling at y=70): flowers must NOT plant
        let mut cave = FakeAccess::new();
        cave.flat_grass();
        for x in -16..16 {
            for z in -16..16 {
                cave.set(x, 70, z, 1); // stone ceiling
            }
        }
        let mut rand = JavaRandom::new(42);
        WorldGenFlowers::new(37).generate(&mut cave, &mut rand, 0, 64, 0);
        WorldGenFlowers::new(38).generate(&mut cave, &mut rand, 0, 64, 0);
        assert_eq!(cave.planted(37), 0, "yellow flowers must not plant in dark cave");
        assert_eq!(cave.planted(38), 0, "red flowers must not plant in dark cave");
    }

    #[test]
    fn test_world_gen_liquids_flows_immediately() {
        let mut acc = FakeAccess::new();
        // Stone floor and surround for spring at (0, 20, 0), open air at (1, 20, 0) with stone floor at (1, 19, 0)
        acc.set(0, 21, 0, 1);
        acc.set(0, 19, 0, 1);
        acc.set(-1, 20, 0, 1);
        acc.set(0, 20, -1, 1);
        acc.set(0, 20, 1, 1);
        acc.set(1, 19, 0, 1);
        let mut rand = JavaRandom::new(7);
        assert!(WorldGenLiquids::new(8).generate(&mut acc, &mut rand, 0, 20, 0));
        // Source stabilized to stationary water (9) at (0, 20, 0) and flowed into (1, 20, 0)
        assert_eq!(acc.get_block_id(0, 20, 0), 9);
        assert!(matches!(acc.get_block_id(1, 20, 0), 8 | 9), "spring must flow into adjacent air");
    }
}
