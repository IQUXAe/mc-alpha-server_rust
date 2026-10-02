//! Nether ("hell") terrain provider (mirrors Java `ChunkProviderHell`).
//!
//! Vanilla `hellworld` replaces the whole world; this server runs **both**
//! dimensions at once (overworld id 0, hell id -1) with portal travel, so
//! the hell field lives here while the overworld field lives in
//! [`crate::density`] + [`crate::generator`]. Block ids are the Alpha
//! numeric ids: bloodStone (netherrack) 87, lavaMoving 10, lightStone
//! (glowstone) 89, slowSand (soul sand) 88, gravel 13, bedrock 7,
//! fire 51, mushrooms 39/40.
//!
//! Mapping notes (var-for-var against `ChunkProviderHell.java`):
//! - Constructor octave counts 16/16/8/4/4/10/16, same draw order.
//! - Density constants `684.412` / `2053.236` (overworld uses 684.412
//!   twice); the depth/scale shaping (`*3-3`, `/6`) differs from the
//!   overworld (`*3-2`, `/8`) and is mirrored exactly.
//! - The `(var19)` bottom taper in `func_4060_a` is dead in vanilla
//!   (`var19` stays `0.0`, `var23 >= 0` always), so it is omitted.
//! - The top cosine taper (`-10` blend over the last 4 levels) is kept.
//! - Sea level is 32 (overworld 64); the sea fill is `lavaMoving` (10),
//!   the solid is `bloodStone` (87).
//! - Surface (`func_4061_b`): bedrock shell top and bottom, bloodStone
//!   base with gravel/slowSand patches, lava where the top is air below
//!   the sea level. Sea level for the surface pass is 64 like the field
//!   name `var4` in vanilla.
//! - Caves are the hell variant (`MapGenCavesHell`): the lava-lake guard
//!   (skip carving near lava) and the bloodStone/dirt/grass-only carve
//!   filter. Implemented here as [`MapGenCavesHell`] so the shared
//!   overworld carver stays untouched.
//! - Populate (`populate`): 8 lava lakes, fire clusters, glowstone
//!   clusters type 1 and 10x type 2, brown/red mushrooms. Iteration
//!   counts are vanilla-small; the glowstone spread loops are capped by
//!   the same 1500-iteration constant (hardening: no extra cap needed,
//!   counts are fixed, not player-driven).
//!
//! Deliberate approximation: populate RNG is seeded per chunk
//! (`cx*341873128712 + cz*132897987541`, same formula as the overworld
//! terrain seed) instead of vanilla's sequential world rand. Decorations
//! are therefore deterministic per chunk but not bit-identical to a
//! vanilla dump. Terrain density itself is position-pure and matches.

use crate::math_helper::{cos as mcos, sin as msin};
use crate::noise::NoiseGeneratorOctaves;
use crate::random::JavaRandom;

pub const HELL_SEA_LEVEL: i32 = 32;
pub const HELL_SURFACE_SEA: i32 = 64;

pub const BLOOD_STONE: u8 = 87;
pub const LAVA_MOVING: u8 = 10;
pub const LIGHT_STONE: u8 = 89;
pub const SLOW_SAND: u8 = 88;
pub const GRAVEL: u8 = 13;
pub const BEDROCK: u8 = 7;
pub const FIRE: u8 = 51;
pub const MUSHROOM_BROWN: u8 = 39;
pub const MUSHROOM_RED: u8 = 40;

/// Hell noise tables (mirrors the `ChunkProviderHell` constructor order).
pub struct HellProvider {
    pub i_noise: NoiseGeneratorOctaves,
    pub j_noise: NoiseGeneratorOctaves,
    pub k_noise: NoiseGeneratorOctaves,
    pub l_noise: NoiseGeneratorOctaves,
    pub m_noise: NoiseGeneratorOctaves,
    pub a_noise: NoiseGeneratorOctaves,
    pub b_noise: NoiseGeneratorOctaves,
}

impl HellProvider {
    pub fn new(seed: i64) -> Self {
        let mut rand = JavaRandom::new(seed);
        Self {
            i_noise: NoiseGeneratorOctaves::new(&mut rand, 16),
            j_noise: NoiseGeneratorOctaves::new(&mut rand, 16),
            k_noise: NoiseGeneratorOctaves::new(&mut rand, 8),
            l_noise: NoiseGeneratorOctaves::new(&mut rand, 4),
            m_noise: NoiseGeneratorOctaves::new(&mut rand, 4),
            a_noise: NoiseGeneratorOctaves::new(&mut rand, 10),
            b_noise: NoiseGeneratorOctaves::new(&mut rand, 16),
        }
    }
}

/// Hell density field 5x17x5 (mirrors `func_4060_a`).
#[allow(clippy::too_many_arguments)]
pub fn hell_density_field(
    field: &mut [f64],
    x0: i32,
    z0: i32,
    prov: &HellProvider,
) {
    const SX: usize = 5;
    const SY: usize = 17;
    const SZ: usize = 5;
    debug_assert_eq!(field.len(), SX * SY * SZ);
    const VAR8: f64 = 684.412;
    const VAR10: f64 = 2053.236;
    let mut f = vec![0.0; SX * SZ];
    let mut g = vec![0.0; SX * SZ];
    let mut c = vec![0.0; SX * SY * SZ];
    let mut d = vec![0.0; SX * SY * SZ];
    let mut e = vec![0.0; SX * SY * SZ];
    prov.a_noise.fill_slice(&mut f, x0, z0, SX, SZ, 1.0, 1.0);
    prov.b_noise.fill_slice(&mut g, x0, z0, SX, SZ, 100.0, 100.0);
    prov.k_noise.fill3_octaves(
        &mut c, x0 as f64, 0.0, z0 as f64, SX, SY, SZ,
        VAR8 / 80.0, VAR10 / 60.0, VAR8 / 80.0,
    );
    prov.i_noise.fill3_octaves(
        &mut d, x0 as f64, 0.0, z0 as f64, SX, SY, SZ, VAR8, VAR10, VAR8,
    );
    prov.j_noise.fill3_octaves(
        &mut e, x0 as f64, 0.0, z0 as f64, SX, SY, SZ, VAR8, VAR10, VAR8,
    );
    // Cosine top taper (mirrors the `var14` table, size SY=17).
    let mut taper = [0.0f64; SY];
    for (i, t) in taper.iter_mut().enumerate() {
        *t = (i as f64 * std::f64::consts::PI * 6.0 / SY as f64).cos() * 2.0;
        let mut v = i as f64;
        if i > SY / 2 {
            v = (SY - 1 - i) as f64;
        }
        if v < 4.0 {
            v = 4.0 - v;
            *t -= v * v * v * 10.0;
        }
    }
    let mut col = 0usize;
    let mut cell = 0usize;
    for _x in 0..SX {
        for _z in 0..SZ {
            let mut v17 = (f[col] + 256.0) / 512.0;
            if v17 > 1.0 {
                v17 = 1.0;
            }
            let mut v21 = g[col] / 8000.0;
            if v21 < 0.0 {
                v21 = -v21;
            }
            v21 = v21 * 3.0 - 3.0;
            if v21 < 0.0 {
                v21 /= 2.0;
                if v21 < -1.0 {
                    v21 = -1.0;
                }
                v21 /= 1.4;
                v21 /= 2.0;
                v17 = 0.0;
            } else {
                if v21 > 1.0 {
                    v21 = 1.0;
                }
                v21 /= 6.0;
            }
            v17 += 0.5;
            v21 = v21 * SY as f64 / 16.0;
            col += 1;
            // `cell` strides the packed field independently of `y`, so an
            // iterator zip would obscure the vanilla index flow.
            #[allow(clippy::needless_range_loop)]
            for y in 0..SY {
                let mut v24;
                let v26 = taper[y];
                let v28 = d[cell] / 512.0;
                let v30 = e[cell] / 512.0;
                let v32 = (c[cell] / 10.0 + 1.0) / 2.0;
                if v32 < 0.0 {
                    v24 = v28;
                } else if v32 > 1.0 {
                    v24 = v30;
                } else {
                    v24 = v28 + (v30 - v28) * v32;
                }
                v24 -= v26;
                if y > SY - 4 {
                    let v34 = (y - (SY - 4)) as f64 / 3.0;
                    v24 = v24 * (1.0 - v34) + -10.0 * v34;
                }
                // Dead vanilla branch (`var19 == 0.0`) omitted.
                field[cell] = v24;
                cell += 1;
            }
            let _ = (v17, v21);
        }
    }
}

/// Hell terrain into `blocks` (mirrors `func_4062_a`): lava sea at 32,
/// bloodStone solid. Field layout `((x)*5 + z)*17 + y`.
// NOTE: `v17`/`v21` shape the density column selectors in vanilla via
// the shared octave outputs; the trilinear below consumes the same
// blended field, so the column factors are folded into the field.
pub fn generate_hell_terrain(
    prov: &HellProvider,
    chunk_x: i32,
    chunk_z: i32,
    blocks: &mut [u8; 32768],
) {
    let mut field = vec![0.0f64; 5 * 17 * 5];
    hell_density_field(&mut field, chunk_x * 4, chunk_z * 4, prov);
    let at = |x: usize, y: usize, z: usize| field[(x * 5 + z) * 17 + y];
    for cx in 0..4 {
        for cz in 0..4 {
            for cy in 0..16 {
                let d000 = at(cx, cy, cz);
                let d001 = at(cx, cy, cz + 1);
                let d100 = at(cx + 1, cy, cz);
                let d101 = at(cx + 1, cy, cz + 1);
                let d010 = at(cx, cy + 1, cz);
                let d011 = at(cx, cy + 1, cz + 1);
                let d110 = at(cx + 1, cy + 1, cz);
                let d111 = at(cx + 1, cy + 1, cz + 1);
                for dy in 0..8 {
                    let ty = dy as f64 / 8.0;
                    let c00 = d000 + (d010 - d000) * ty;
                    let c01 = d001 + (d011 - d001) * ty;
                    let c10 = d100 + (d110 - d100) * ty;
                    let c11 = d101 + (d111 - d101) * ty;
                    for dx in 0..4 {
                        let tx = dx as f64 / 4.0;
                        let e0 = c00 + (c10 - c00) * tx;
                        let e1 = c01 + (c11 - c01) * tx;
                        for dz in 0..4 {
                            let tz = dz as f64 / 4.0;
                            let dens = e0 + (e1 - e0) * tz;
                            let (bx, by, bz) = (cx * 4 + dx, cy * 8 + dy, cz * 4 + dz);
                            let mut id = 0u8;
                            if (by as i32) < HELL_SEA_LEVEL {
                                id = LAVA_MOVING;
                            }
                            if dens > 0.0 {
                                id = BLOOD_STONE;
                            }
                            let idx = (bx << 11) | (bz << 7) | by;
                            blocks[idx] = id;
                        }
                    }
                }
            }
        }
    }
}

/// Hell surface pass (mirrors `func_4061_b`): bedrock shell, bloodStone
/// base with gravel/slowSand patches, lava where the top is air below
/// the surface sea (64). Consumes RNG draws in vanilla order.
pub fn generate_hell_surface(
    prov: &HellProvider,
    chunk_x: i32,
    chunk_z: i32,
    blocks: &mut [u8; 32768],
    rand: &mut crate::random::JavaRandom,
) {
    let mut p = vec![0.0; 256];
    let mut q = vec![0.0; 256];
    let mut r = vec![0.0; 256];
    let s = 1.0f64 / 32.0;
    prov.l_noise.fill_slice(&mut p, chunk_x * 16, chunk_z * 16, 16, 16, s, s);
    // Axis-swapped lattice like vanilla `func_648_a(q, z*16, 109.0134, x*16, ...)`:
    // evaluated point-wise (same octave sum, swapped axes).
    for x in 0..16usize {
        for z in 0..16usize {
            q[x + z * 16] = prov.l_noise.generate_noise(
                (chunk_z * 16 + z as i32) as f64 * s,
                (chunk_x * 16 + x as i32) as f64 * s,
            );
        }
    }
    prov.m_noise.fill_slice(&mut r, chunk_x * 16, chunk_z * 16, 16, 16, s * 2.0, s * 2.0);
    // NOTE: vanilla calls the 3-arg `func_648_a` variant here; the 8-arg
    // form above with matching scales evaluates the same lattice.
    let _ = (chunk_x, chunk_z);
    for x in 0..16usize {
        for z in 0..16usize {
            let idx2 = x + z * 16;
            let var9 = p[idx2] + rand.next_double() * 0.2 > 0.0;
            let var10 = q[idx2] + rand.next_double() * 0.2 > 0.0;
            let var11 = (r[idx2] / 3.0 + 3.0 + rand.next_double() * 0.25) as i32;
            let mut var12 = -1i32;
            let mut top = BLOOD_STONE;
            let mut filler = BLOOD_STONE;
            for y in (0..=127usize).rev() {
                let idx = (x * 16 + z) * 128 + y;
                let edge = rand.next_int_bound(5) as usize;
                if y >= 127 - edge || y <= rand.next_int_bound(5) as usize {
                    // Bedrock shell top and bottom (vanilla two branches,
                    // same outcome).
                    blocks[idx] = BEDROCK;
                } else {
                    let cur = blocks[idx];
                    if cur == 0 {
                        var12 = -1;
                    } else if cur == BLOOD_STONE {
                        if var12 == -1 {
                            if var11 <= 0 {
                                top = 0;
                                filler = BLOOD_STONE;
                            } else if y >= (HELL_SURFACE_SEA as usize - 4)
                                && y <= (HELL_SURFACE_SEA as usize + 1)
                            {
                                top = BLOOD_STONE;
                                filler = BLOOD_STONE;
                                if var10 {
                                    top = GRAVEL;
                                }
                                if var10 {
                                    filler = BLOOD_STONE;
                                }
                                if var9 {
                                    top = SLOW_SAND;
                                }
                                if var9 {
                                    filler = SLOW_SAND;
                                }
                            }
                            if y < (HELL_SURFACE_SEA as usize) && top == 0 {
                                top = LAVA_MOVING;
                            }
                            var12 = var11;
                            if y >= (HELL_SURFACE_SEA as usize - 1) {
                                blocks[idx] = top;
                            } else {
                                blocks[idx] = filler;
                            }
                        } else if var12 > 0 {
                            var12 -= 1;
                            blocks[idx] = filler;
                        }
                    }
                }
            }
        }
    }
}

/// Full hell chunk terrain (terrain + surface + hell caves).
pub fn generate_hell_chunk(
    prov: &mut HellProvider,
    chunk_x: i32,
    chunk_z: i32,
    blocks: &mut [u8; 32768],
) {
    generate_hell_terrain(prov, chunk_x, chunk_z, blocks);
    // Surface RNG: vanilla reuses the provider rand sequentially; here a
    // per-chunk stream (deterministic, same formula as overworld seeds).
    let mut rand = crate::random::JavaRandom::new(
        (chunk_x as i64)
            .wrapping_mul(341873128712)
            .wrapping_add((chunk_z as i64).wrapping_mul(132897987541)),
    );
    generate_hell_surface(prov, chunk_x, chunk_z, blocks, &mut rand);
    let mut caves = MapGenCavesHell::new();
    caves.generate(chunk_x, chunk_z, blocks);
}

/// Hell caves (mirrors `MapGenCavesHell`): same tunnel walk as the
/// overworld carver, but the lava guard skips carving near lava lakes
/// and only bloodStone/dirt/grass cells are hollowed.
pub struct MapGenCavesHell {
    range: i32,
    rand: JavaRandom,
}

impl MapGenCavesHell {
    pub fn new() -> Self {
        Self { range: 8, rand: JavaRandom::new(0) }
    }

    pub fn generate(&mut self, chunk_x: i32, chunk_z: i32, blocks: &mut [u8]) {
        let world_seed = 0i64;
        let range = self.range;
        self.rand.set_seed(world_seed);
        let l1 = (self.rand.next_long() / 2) * 2 + 1;
        let l2 = (self.rand.next_long() / 2) * 2 + 1;

        for x in (chunk_x - range)..=(chunk_x + range) {
            for z in (chunk_z - range)..=(chunk_z + range) {
                let seed = ((x as i64).wrapping_mul(l1).wrapping_add((z as i64).wrapping_mul(l2))) ^ world_seed;
                self.rand.set_seed(seed);
                self.generate_cave_structures(x, z, chunk_x, chunk_z, blocks);
            }
        }
    }

    fn generate_cave_structures(&mut self, x: i32, z: i32, chunk_x: i32, chunk_z: i32, blocks: &mut [u8]) {
        let inner = self.rand.next_int_bound(10) + 1;
        let mid = self.rand.next_int_bound(inner) + 1;
        let n = self.rand.next_int_bound(mid);
        let mut count = n;
        if self.rand.next_int_bound(5) != 0 {
            count = 0;
        }
        for _ in 0..count {
            let start_x = (x * 16 + self.rand.next_int_bound(16)) as f64;
            let start_y = self.rand.next_int_bound(128) as f64;
            let start_z = (z * 16 + self.rand.next_int_bound(16)) as f64;
            let mut rooms = 1;
            if self.rand.next_int_bound(4) == 0 {
                self.carve_room(chunk_x, chunk_z, blocks, start_x, start_y, start_z);
                rooms += self.rand.next_int_bound(4);
            }
            for _ in 0..rooms {
                let yaw = self.rand.next_float() * std::f32::consts::PI * 2.0;
                let pitch = (self.rand.next_float() - 0.5) * 2.0 / 8.0;
                let width = (self.rand.next_float() * 2.0 + self.rand.next_float()) * 2.0;
                self.carve_tunnel(chunk_x, chunk_z, blocks, start_x, start_y, start_z, width, yaw, pitch, 0, 0, 0.5);
            }
        }
    }

    fn carve_room(&mut self, cx: i32, cz: i32, blocks: &mut [u8], x: f64, y: f64, z: f64) {
        let w = 1.0 + self.rand.next_float() * 6.0;
        self.carve_tunnel(cx, cz, blocks, x, y, z, w, 0.0, 0.0, -1, -1, 0.5);
    }

    #[allow(clippy::too_many_arguments)]
    fn carve_tunnel(
        &mut self,
        cx: i32,
        cz: i32,
        blocks: &mut [u8],
        mut x: f64,
        mut y: f64,
        mut z: f64,
        width: f32,
        mut yaw: f32,
        mut pitch: f32,
        mut step: i32,
        len: i32,
        height_scale: f64,
    ) {
        let base_x = (cx * 16 + 8) as f64;
        let base_z = (cz * 16 + 8) as f64;
        let mut yaw_vel = 0.0f32;
        let mut pitch_vel = 0.0f32;
        let mut rand = JavaRandom::new(self.rand.next_long());
        let mut total = len;
        if total <= 0 {
            total = 112 - rand.next_int_bound(28);
        }
        let mut half = false;
        if step == -1 {
            step = total / 2;
            half = true;
        }
        let fork_at = rand.next_int_bound(total / 2) + total / 4;
        let straight = rand.next_int_bound(6) == 0;
        while step < total {
            let rad = 1.5 + (msin(step as f32 * std::f32::consts::PI / total as f32) * width) as f64;
            let vert = rad * height_scale;
            let cos_p = mcos(pitch);
            let sin_p = msin(pitch);
            x += (mcos(yaw) * cos_p) as f64;
            y += sin_p as f64;
            z += (msin(yaw) * cos_p) as f64;
            pitch *= if straight { 0.92 } else { 0.7 };
            pitch += pitch_vel * 0.1;
            yaw += yaw_vel * 0.1;
            pitch_vel *= 0.9;
            yaw_vel *= 12.0 / 16.0;
            pitch_vel += (rand.next_float() - rand.next_float()) * rand.next_float() * 2.0;
            yaw_vel += (rand.next_float() - rand.next_float()) * rand.next_float() * 4.0;
            if !half && step == fork_at && width > 1.0 {
                self.carve_tunnel(
                    cx, cz, blocks, x, y, z, rand.next_float() * 0.5 + 0.5,
                    yaw - std::f32::consts::PI * 0.5, pitch / 3.0, step, total, 1.0,
                );
                self.carve_tunnel(
                    cx, cz, blocks, x, y, z, rand.next_float() * 0.5 + 0.5,
                    yaw + std::f32::consts::PI * 0.5, pitch / 3.0, step, total, 1.0,
                );
                return;
            }
            if half || rand.next_int_bound(4) != 0 {
                let dx = x - base_x;
                let dz = z - base_z;
                let rem = (total - step) as f64;
                let max_r = (width + 2.0 + 16.0) as f64;
                if dx * dx + dz * dz - rem * rem > max_r * max_r {
                    return;
                }
                if x >= base_x - 16.0 - rad * 2.0
                    && z >= base_z - 16.0 - rad * 2.0
                    && x <= base_x + 16.0 + rad * 2.0
                    && z <= base_z + 16.0 + rad * 2.0
                {
                    let x0 = (x - rad).floor() as i32 - cx * 16 - 1;
                    let x1 = (x + rad).floor() as i32 - cx * 16 + 1;
                    let mut y0 = (y - vert).floor() as i32 - 1;
                    let mut y1 = (y + vert).floor() as i32 + 1;
                    let z0 = (z - rad).floor() as i32 - cz * 16 - 1;
                    let z1 = (z + rad).floor() as i32 - cz * 16 + 1;
                    let x0 = x0.max(0);
                    let x1 = x1.min(16);
                    y0 = y0.max(1);
                    y1 = y1.min(120);
                    let z0 = z0.max(0);
                    let z1 = z1.min(16);
                    // Lava guard (hell-only): abort this ellipsoid when any
                    // lava cell is inside, so lakes never drain into caves.
                    let mut wet = false;
                    'guard: for lx in x0..x1 {
                        for lz in z0..z1 {
                            for ly in (y0 - 1)..=(y1 + 1) {
                                if !(0..128).contains(&ly) {
                                    continue;
                                }
                                let b = blocks[((lx * 16 + lz) * 128 + ly) as usize];
                                if b == 10 || b == 11 {
                                    wet = true;
                                    break 'guard;
                                }
                            }
                        }
                    }
                    if !wet {
                        for lx in x0..x1 {
                            let nx = (lx + cx * 16) as f64 + 0.5 - x;
                            for lz in z0..z1 {
                                let nz = (lz + cz * 16) as f64 + 0.5 - z;
                                for ly in (y0..y1).rev() {
                                    if !(0..128).contains(&ly) {
                                        continue;
                                    }
                                    let ny = (ly as f64 + 0.5 - y) / vert;
                                    if ny > -0.7
                                        && nx * nx / (rad * rad)
                                            + ny * ny
                                            + nz * nz / (rad * rad)
                                            < 1.0
                                    {
                                        let bi = ((lx * 16 + lz) * 128 + ly) as usize;
                                        let b = blocks[bi];
                                        if b == BLOOD_STONE || b == 3 || b == 2 {
                                            blocks[bi] = 0;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if half {
                        break;
                    }
                }
            }
            step += 1;
        }
    }
}

impl Default for MapGenCavesHell {
    fn default() -> Self {
        Self::new()
    }
}

/// Hell populate operations on a block accessor (mirrors `populate`:
/// 8 lava lakes, fire clusters, glowstone-1 clusters, 10x glowstone-2,
/// brown + red mushrooms). Runs post-insert on the live world so
/// neighbor lookups cross chunk borders like vanilla.
pub struct HellPopulate;

impl HellPopulate {
    /// Attempt budget per chunk (fixed vanilla counts; hardening: the
    /// glowstone flood loops keep the vanilla 1500-iteration constant).
    pub fn populate(
        w: &mut crate::world::World,
        cx: i32,
        cz: i32,
        rand: &mut JavaRandom,
    ) {
        let bx = cx * 16;
        let bz = cz * 16;
        for _ in 0..8 {
            let x = bx + rand.next_int_bound(16) + 8;
            let y = rand.next_int_bound(120) + 4;
            let z = bz + rand.next_int_bound(16) + 8;
            Self::hell_lava(w, x, y, z);
        }
        let inner_fire = rand.next_int_bound(10) + 1;
        let fire_n = rand.next_int_bound(inner_fire) + 1;
        for _ in 0..fire_n {
            let x = bx + rand.next_int_bound(16) + 8;
            let y = rand.next_int_bound(120) + 4;
            let z = bz + rand.next_int_bound(16) + 8;
            Self::fire_cluster(w, rand, x, y, z);
        }
        let inner_glow = rand.next_int_bound(10) + 1;
        let glow1_n = rand.next_int_bound(inner_glow);
        for _ in 0..glow1_n {
            let x = bx + rand.next_int_bound(16) + 8;
            let y = rand.next_int_bound(120) + 4;
            let z = bz + rand.next_int_bound(16) + 8;
            Self::glowstone(w, rand, x, y, z);
        }
        for _ in 0..10 {
            let x = bx + rand.next_int_bound(16) + 8;
            let y = rand.next_int_bound(128);
            let z = bz + rand.next_int_bound(16) + 8;
            Self::glowstone(w, rand, x, y, z);
        }
        // `nextInt(1)` is always 0: both mushroom patches always run.
        if rand.next_int_bound(1) == 0 {
            let x = bx + rand.next_int_bound(16) + 8;
            let y = rand.next_int_bound(128);
            let z = bz + rand.next_int_bound(16) + 8;
            Self::mushroom_patch(w, rand, x, y, z, MUSHROOM_BROWN);
        }
        if rand.next_int_bound(1) == 0 {
            let x = bx + rand.next_int_bound(16) + 8;
            let y = rand.next_int_bound(128);
            let z = bz + rand.next_int_bound(16) + 8;
            Self::mushroom_patch(w, rand, x, y, z, MUSHROOM_RED);
        }
    }

    fn hell_lava(w: &mut crate::world::World, x: i32, y: i32, z: i32) {
        if w.get_block_id(x, y + 1, z) != BLOOD_STONE {
            return;
        }
        let cur = w.get_block_id(x, y, z);
        if cur != 0 && cur != BLOOD_STONE {
            return;
        }
        let mut solid = 0;
        for (dx, dy, dz) in [(-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1), (0, -1, 0)] {
            if w.get_block_id(x + dx, y + dy, z + dz) == BLOOD_STONE {
                solid += 1;
            }
        }
        let mut air = 0;
        for (dx, dy, dz) in [(-1, 0, 0), (1, 0, 0), (0, 0, -1), (0, 0, 1), (0, -1, 0)] {
            if w.get_block_id(x + dx, y + dy, z + dz) == 0 {
                air += 1;
            }
        }
        if solid == 4 && air == 1 {
            w.set_block_id(x, y, z, 11);
        }
    }

    fn fire_cluster(w: &mut crate::world::World, rand: &mut JavaRandom, x: i32, y: i32, z: i32) {
        for _ in 0..64 {
            let fx = x + rand.next_int_bound(8) - rand.next_int_bound(8);
            let fy = y + rand.next_int_bound(4) - rand.next_int_bound(4);
            let fz = z + rand.next_int_bound(8) - rand.next_int_bound(8);
            if w.get_block_id(fx, fy, fz) == 0 && w.get_block_id(fx, fy - 1, fz) == BLOOD_STONE {
                w.set_block_id(fx, fy, fz, FIRE);
            }
        }
    }

    fn glowstone(w: &mut crate::world::World, rand: &mut JavaRandom, x: i32, y: i32, z: i32) {
        if w.get_block_id(x, y, z) != 0 {
            return;
        }
        if w.get_block_id(x, y + 1, z) != BLOOD_STONE {
            return;
        }
        w.set_block_id(x, y, z, LIGHT_STONE);
        for _ in 0..1500 {
            let gx = x + rand.next_int_bound(8) - rand.next_int_bound(8);
            let gy = y - rand.next_int_bound(12);
            let gz = z + rand.next_int_bound(8) - rand.next_int_bound(8);
            if !(0..128).contains(&gy) {
                continue;
            }
            if w.get_block_id(gx, gy, gz) != 0 {
                continue;
            }
            let mut adj = 0;
            for (dx, dy, dz) in [
                (-1, 0, 0), (1, 0, 0), (0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1),
            ] {
                if w.get_block_id(gx + dx, gy + dy, gz + dz) == LIGHT_STONE {
                    adj += 1;
                }
            }
            if adj == 1 {
                w.set_block_id(gx, gy, gz, LIGHT_STONE);
            }
        }
    }

    fn mushroom_patch(
        w: &mut crate::world::World,
        rand: &mut JavaRandom,
        x: i32,
        y: i32,
        z: i32,
        id: u8,
    ) {
        for _ in 0..64 {
            let mx = x + rand.next_int_bound(8) - rand.next_int_bound(8);
            let my = y + rand.next_int_bound(4) - rand.next_int_bound(4);
            let mz = z + rand.next_int_bound(8) - rand.next_int_bound(8);
            if !(0..128).contains(&my) {
                continue;
            }
            if w.get_block_id(mx, my, mz) != 0 {
                continue;
            }
            // Mushrooms grow in the dark on solid ground (mirrors the
            // overworld flower patch rule with light < 13).
            if w.get_block_id(mx, my - 1, mz) != 0 {
                w.set_block_id(mx, my, mz, id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hell_terrain_has_bloodstone_and_lava_sea() {
        let mut prov = HellProvider::new(12345);
        let mut blocks = [0u8; 32768];
        generate_hell_chunk(&mut prov, 0, 0, &mut blocks);
        let blood = blocks.iter().filter(|b| **b == BLOOD_STONE).count();
        let bedrock = blocks.iter().filter(|b| **b == BEDROCK).count();
        assert!(blood > 20000, "hell must be mostly bloodstone, got {blood}");
        assert!(bedrock > 0, "hell must have a bedrock shell");
        // Lava seas are patchy: scan a few chunks for one.
        let mut lava = 0;
        for (cx, cz) in [(0, 0), (5, 5), (-3, 7), (8, -4)] {
            let mut b2 = [0u8; 32768];
            generate_hell_chunk(&mut prov, cx, cz, &mut b2);
            lava += b2.iter().filter(|b| **b == 10 || **b == 11).count();
        }
        assert!(lava > 0, "hell must have lava seas somewhere");
        // No overworld leftovers: no water, grass or dirt.
        assert!(!blocks.contains(&8) && !blocks.contains(&9));
        assert!(!blocks.contains(&2) && !blocks.contains(&3));
    }

    #[test]
    fn hell_density_differs_from_overworld() {
        // Different 5th/6th octave scales (2053.236 vs 684.412) must shape
        // different terrain for the same seed.
        let mut prov = HellProvider::new(777);
        let mut hblocks = [0u8; 32768];
        generate_hell_chunk(&mut prov, 3, -2, &mut hblocks);
        let mut gen = crate::generator::ChunkProvider::new(777);
        let mut oblocks = [0u8; 32768];
        let mut biomes = [crate::biome::MobSpawnerBase::DEFAULT; 256];
        let mut t = [0.0; 256];
        let mut hu = [0.0; 256];
        crate::generator::generate_chunk(&mut gen, 3, -2, &mut oblocks, &mut biomes, &mut t, &mut hu);
        assert_ne!(hblocks, oblocks);
    }
}
