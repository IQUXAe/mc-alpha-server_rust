//! Pure mob-spawning math (mirrors Java `SpawnerAnimals::func_4111_a`).
//!
//! Two layers:
//! - `spawn_*` helpers: the cap formula, pack-spread step, and
//!   world-spawn exclusion check. Pure (no allocation), the single source of truth.
//! - `spawn_hostile` / `spawn_passive`: batch drivers
//!   that run the whole per-tick spawn pass against a `SpawnerWorld`.
//!   The live `World` implements the trait directly (explicit borrows, no
//!   tests implement it on a scripted fake, so draw
//!   sequences and check order stay exactly covered.
//!
//! NOTE on draw order: the original loop drew the four pack-spread values as
//! `x += spread6(rand) - spread6(rand)`, whose evaluation order was
//! unspecified. The driver below fixes the order (x-pair, then z-pair);
//! the draw *count* per attempt is unchanged.
//!
//! World access is explicit through the `SpawnerWorld` trait (block
//! storage, entity list, `getCanSpawnHere`), and RNG draws go through it
//! into `World::rand`, so check order and draw sequences are preserved.

/// Spawn cap: `budget_per_256 * num_eligible_chunks / 256`.
///
/// Mirrors `World::spawnHostileMobs` (`100 * n / 256`) and
/// `World::spawnPassiveMobs` (`20 * n / 256`), which mirror Java
/// `var29.field_4220_d * field_4311_a.size() / 256` per creature type.
/// Multiplication first, then truncating division, exactly like C++ `int`.
pub fn spawn_max_count(num_eligible_chunks: i32, budget_per_256: i32) -> i32 {
    budget_per_256.wrapping_mul(num_eligible_chunks).wrapping_div(256)
}

/// One pack-spread step: `first - second`.
///
/// Mirrors `x += spread6(rand) - spread6(rand)` (Java
/// `var15 += rand.nextInt(6) - rand.nextInt(6)`). Each draw is uniform in
/// `[0, 6)`, so the step is triangular in `[-5, 5]`. The two draws are
/// passed in (C++ owns the RNG stream); this function only owns the shape.
pub fn spawn_pack_offset(first: i32, second: i32) -> i32 {
    first.wrapping_sub(second)
}

/// World-spawn exclusion: is the candidate inside the 24-block no-spawn
/// cube around the world spawn (squared distance `< 576.0`)?
///
/// Mirrors the `dsx*dsx + dsy*dsy + dsz*dsz < 576.0f` check in both spawn
/// functions (Java `var26 >= 576.0F` gate). `f32` arithmetic matches the
/// C++ `float` computation bit-for-bit for the same inputs.
pub fn spawn_too_close_to_spawn(
    fx: f32,
    fy: f32,
    fz: f32,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
) -> bool {
    let dsx = fx - spawn_x as f32;
    let dsy = fy - spawn_y as f32;
    let dsz = fz - spawn_z as f32;
    dsx * dsx + dsy * dsy + dsz * dsz < 576.0
}

/// World access for the spawn drivers as an explicit trait. The live world
/// implements it with direct borrows; tests implement it on a scripted fake.
///
/// RNG draws go through `spawn_next_int` / `spawn_next_float` into
/// `World::rand`, preserving the exact historical draw sequence.
/// `spawn_try_spawn` must construct `kind`, position it, run
/// `getCanSpawnHere`, join the world on success and return the entity id
/// with `getMaxSpawnedInChunk` (`None` on failure).
/// Kinds are 0=spider,1=zombie,2=skeleton,3=creeper for the hostile driver
/// and 0=sheep,1=pig,2=chicken,3=cow for the passive driver.
pub trait SpawnerWorld {
    fn spawn_next_int(&mut self, bound: i32) -> i32;
    fn spawn_next_float(&mut self, lo: f32, hi: f32) -> f32;
    fn spawn_chunk_exists(&mut self, x: i32, z: i32) -> bool;
    fn spawn_is_solid(&mut self, x: i32, y: i32, z: i32) -> bool;
    fn spawn_is_air(&mut self, x: i32, y: i32, z: i32) -> bool;
    fn spawn_is_liquid(&mut self, x: i32, y: i32, z: i32) -> bool;
    fn spawn_try_spawn(
        &mut self,
        hostile: bool,
        kind: u8,
        fx: f32,
        fy: f32,
        fz: f32,
        yaw: f32,
    ) -> Option<(i32, i32)>;
    fn spawn_jockey(&mut self, fx: f32, fy: f32, fz: f32, yaw: f32, host_id: i32) -> bool;
}

const CHUNK_RADIUS: i32 = 8;
const GROUP_ATTEMPTS: i32 = 3;
const PACK_ATTEMPTS: i32 = 4;
const PACK_SPREAD: i32 = 6;
const PLAYER_RANGE: f64 = 24.0;
const JOCKEY_ROLL: i32 = 100;
const HOSTILE_BUDGET: i32 = 100;
const PASSIVE_BUDGET: i32 = 20;

fn chunk_key(chunk_x: i32, chunk_z: i32) -> u64 {
    ((chunk_x as u32 as u64) << 32) | (chunk_z as u32 as u64)
}

/// One full spawn pass. Returns the number of primary spawns (jockey
/// skeletons excluded). Mirrors `SpawnerAnimals.performSpawning` and
/// `MobSpawnerBase.biomeMonsters` (`[Spider, Zombie, Zombie, Skeleton, Creeper]`).
#[allow(clippy::too_many_arguments)]
fn spawn_pass(
    world: &mut impl SpawnerWorld,
    players_x: &[f64],
    players_y: &[f64],
    players_z: &[f64],
    hostile: bool,
    current_count: i32,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
    world_height: i32,
) -> i32 {
    // Eligible chunks: 8-chunk square around every player (Java HashSet,
    // C++ vector+sort+unique — same set either way).
    let mut eligible: Vec<u64> = Vec::new();
    for i in 0..players_x.len() {
        let cx = (players_x[i] / 16.0).floor() as i32;
        let cz = (players_z[i] / 16.0).floor() as i32;
        for dx in -CHUNK_RADIUS..=CHUNK_RADIUS {
            for dz in -CHUNK_RADIUS..=CHUNK_RADIUS {
                eligible.push(chunk_key(cx + dx, cz + dz));
            }
        }
    }
    if eligible.is_empty() {
        return 0;
    }
    eligible.sort_unstable();
    eligible.dedup();

    let budget = if hostile { HOSTILE_BUDGET } else { PASSIVE_BUDGET };
    if current_count > spawn_max_count(eligible.len() as i32, budget) {
        return 0;
    }

    let mut spawned = 0;
    for key in &eligible {
        let chunk_x = (key >> 32) as i32;
        let chunk_z = (key & 0xFFFF_FFFF) as i32;
        if !world.spawn_chunk_exists(chunk_x, chunk_z) {
            continue;
        }

        let base_x = chunk_x.wrapping_mul(16);
        let base_z = chunk_z.wrapping_mul(16);
        // Java MobSpawnerBase: biomeMonsters has 5 entries
        // [Spider, Zombie, Zombie, Skeleton, Creeper] (Zombie 2x weight),
        // biomeCreatures has 4 entries [Sheep, Pig, Chicken, Cow].
        let kind = if hostile {
            match world.spawn_next_int(5) {
                0 => 0,
                1 | 2 => 1,
                3 => 2,
                _ => 3,
            }
        } else {
            world.spawn_next_int(4) as u8
        };
        let origin_x = base_x + world.spawn_next_int(16);
        let origin_y = world.spawn_next_int(world_height);
        let origin_z = base_z + world.spawn_next_int(16);

        if world.spawn_is_solid(origin_x, origin_y, origin_z)
            || !world.spawn_is_air(origin_x, origin_y, origin_z)
        {
            continue;
        }

        let mut move_to_next_chunk = false;
        for _ in 0..GROUP_ATTEMPTS {
            if move_to_next_chunk {
                break;
            }
            let mut group_count = 0;
            let (mut x, mut y, mut z) = (origin_x, origin_y, origin_z);
            for _ in 0..PACK_ATTEMPTS {
                // Java order per attempt: x-pair, y-pair, z-pair. The y draws
                // are nextInt(1) (always 0) but still consume RNG state.
                x += spawn_pack_offset(
                    world.spawn_next_int(PACK_SPREAD),
                    world.spawn_next_int(PACK_SPREAD),
                );
                y += spawn_pack_offset(world.spawn_next_int(1), world.spawn_next_int(1));
                z += spawn_pack_offset(
                    world.spawn_next_int(PACK_SPREAD),
                    world.spawn_next_int(PACK_SPREAD),
                );

                if !world.spawn_is_solid(x, y - 1, z)
                    || world.spawn_is_solid(x, y, z)
                    || world.spawn_is_liquid(x, y, z)
                    || world.spawn_is_solid(x, y + 1, z)
                {
                    continue;
                }

                let fx = x as f32 + 0.5;
                let fy = y as f32;
                let fz = z as f32 + 0.5;
                // 24-block nearest-player check (mirrors getClosestPlayer 3D scan).
                let mut too_close = false;
                for i in 0..players_x.len() {
                    let dx = players_x[i] - fx as f64;
                    let dy = players_y[i] - fy as f64;
                    let dz = players_z[i] - fz as f64;
                    if dx * dx + dy * dy + dz * dz < PLAYER_RANGE * PLAYER_RANGE {
                        too_close = true;
                        break;
                    }
                }
                if too_close {
                    continue;
                }
                if spawn_too_close_to_spawn(fx, fy, fz, spawn_x, spawn_y, spawn_z) {
                    continue;
                }

                let yaw = world.spawn_next_float(0.0, 360.0);
                let Some((id, max_in_chunk)) =
                    world.spawn_try_spawn(hostile, kind, fx, fy, fz, yaw)
                else {
                    continue;
                };
                spawned += 1;

                // Alpha spider jockey chance (hostile only).
                if hostile && kind == 0 && world.spawn_next_int(JOCKEY_ROLL) == 0 {
                    world.spawn_jockey(fx, fy, fz, yaw, id);
                }

                group_count += 1;
                if group_count >= max_in_chunk {
                    move_to_next_chunk = true; // Java: continue label110
                    break;
                }
            }
        }
    }
    spawned
}

/// Batch driver for `World::spawnHostileMobs`: parallel player-position
/// slices. Returns the primary spawn count.
pub fn spawn_hostile(
    world: &mut impl SpawnerWorld,
    players_x: &[f64],
    players_y: &[f64],
    players_z: &[f64],
    current_count: i32,
    spawn: [i32; 3],
    world_height: i32,
) -> i32 {
    spawn_pass(
        world,
        players_x,
        players_y,
        players_z,
        true,
        current_count,
        spawn[0],
        spawn[1],
        spawn[2],
        world_height,
    )
}

/// Batch driver for `World::spawnPassiveMobs`. Same contract as hostile;
/// spider-jockey logic is skipped.
pub fn spawn_passive(
    world: &mut impl SpawnerWorld,
    players_x: &[f64],
    players_y: &[f64],
    players_z: &[f64],
    current_count: i32,
    spawn: [i32; 3],
    world_height: i32,
) -> i32 {
    spawn_pass(
        world,
        players_x,
        players_y,
        players_z,
        false,
        current_count,
        spawn[0],
        spawn[1],
        spawn[2],
        world_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_count_hostile_budget() {
        // 100 per 256 chunks (World::spawnHostileMobs).
        assert_eq!(spawn_max_count(256, 100), 100);
        assert_eq!(spawn_max_count(128, 100), 50);
        assert_eq!(spawn_max_count(0, 100), 0);
        // Truncation like C++ int division: 100*1/256 == 0.
        assert_eq!(spawn_max_count(1, 100), 0);
        assert_eq!(spawn_max_count(3, 100), 1);
    }

    #[test]
    fn test_max_count_passive_budget() {
        // 20 per 256 chunks (World::spawnPassiveMobs).
        assert_eq!(spawn_max_count(256, 20), 20);
        assert_eq!(spawn_max_count(128, 20), 10);
        assert_eq!(spawn_max_count(0, 20), 0);
    }

    #[test]
    fn test_pack_offset_shape() {
        assert_eq!(spawn_pack_offset(5, 0), 5);
        assert_eq!(spawn_pack_offset(0, 5), -5);
        assert_eq!(spawn_pack_offset(3, 3), 0);
        // Triangular step always fits in [-5, 5] for spread6 draws.
        for a in 0..6 {
            for b in 0..6 {
                let d = spawn_pack_offset(a, b);
                assert!((-5..=5).contains(&d));
            }
        }
    }

    #[test]
    fn test_too_close_to_spawn() {
        // On the spawn point: excluded.
        assert!(spawn_too_close_to_spawn(0.5, 64.0, 0.5, 0, 64, 0));
        // Clearly outside: allowed.
        assert!(!spawn_too_close_to_spawn(100.5, 64.0, 100.5, 0, 64, 0));
        // Boundary is exclusive: exactly 24 blocks away (576.0) is allowed.
        assert!(!spawn_too_close_to_spawn(24.0, 64.0, 0.0, 0, 64, 0));
        // Just inside: excluded.
        assert!(spawn_too_close_to_spawn(23.5, 64.0, 0.0, 0, 64, 0));
    }

    // Batch-driver tests with a scripted fake world. Each scenario owns
    // its fake (no shared counters), so draw scripts stay local.
    mod driver {
        use super::*;

        struct FakeWorld {
            try_calls: i32,
            jockey_calls: i32,
            next_id: i32,
        }

        fn fake() -> FakeWorld {
            FakeWorld { try_calls: 0, jockey_calls: 0, next_id: 1 }
        }

        impl SpawnerWorld for FakeWorld {
            // All RNG draws return 0: every 1/N roll succeeds, kinds/origins are 0.
            fn spawn_next_int(&mut self, bound: i32) -> i32 {
                assert!(bound > 0);
                0
            }
            fn spawn_next_float(&mut self, _lo: f32, _hi: f32) -> f32 {
                0.0
            }
            fn spawn_chunk_exists(&mut self, _x: i32, _z: i32) -> bool {
                true
            }
            // Solid ground only at y == -1, air everywhere, no liquid.
            fn spawn_is_solid(&mut self, _x: i32, y: i32, _z: i32) -> bool {
                y == -1
            }
            fn spawn_is_air(&mut self, _x: i32, _y: i32, _z: i32) -> bool {
                true
            }
            fn spawn_is_liquid(&mut self, _x: i32, _y: i32, _z: i32) -> bool {
                false
            }
            fn spawn_try_spawn(
                &mut self,
                _hostile: bool,
                _kind: u8,
                _fx: f32,
                _fy: f32,
                _fz: f32,
                _yaw: f32,
            ) -> Option<(i32, i32)> {
                self.try_calls += 1;
                let id = self.next_id;
                self.next_id += 1;
                Some((id, 4))
            }
            fn spawn_jockey(&mut self, _fx: f32, _fy: f32, _fz: f32, _yaw: f32, _host: i32) -> bool {
                self.jockey_calls += 1;
                true
            }
        }

        #[test]
        fn test_driver_scenarios() {
            // Player at origin; world spawn far away so nothing is excluded.
            let px = [0.5f64];
            let py = [64.0f64];
            let pz = [0.5f64];

            // No players, no chunks, no spawns.
            let mut world = fake();
            assert_eq!(
                spawn_hostile(&mut world, &[], &[], &[], 0, [1000, 64, 1000], 128),
                0
            );
            assert_eq!(world.try_calls, 0);

            // Cap gate: 1 player -> 289 chunks -> max 112 hostile; 113 blocks everything.
            let mut world = fake();
            assert_eq!(
                spawn_hostile(&mut world, &px, &py, &pz, 113, [1000, 64, 1000], 128),
                0
            );
            assert_eq!(world.try_calls, 0);

            // Happy hostile path: every chunk attempts (roll always 0), each
            // fills its group of 4 (max_in_chunk), kind 0 (spider) always
            // triggers the per-spawn jockey roll (always 0).
            // 17x17 = 289 chunks -> 289*4 primary + 289*4 jockeys.
            let mut world = fake();
            let n = spawn_hostile(&mut world, &px, &py, &pz, 0, [1000, 64, 1000], 128);
            assert_eq!(n, 289 * 4);
            assert_eq!(world.try_calls, 289 * 4);
            assert_eq!(world.jockey_calls, 289 * 4);

            // Happy passive path: same totals, no jockeys.
            let mut world = fake();
            let n = spawn_passive(&mut world, &px, &py, &pz, 0, [1000, 64, 1000], 128);
            assert_eq!(n, 289 * 4);
            assert_eq!(world.try_calls, 289 * 4);
            assert_eq!(world.jockey_calls, 0);

            // World-spawn exclusion: spawn at y=0 inside the eligible area
            // removes candidates (strictly fewer spawns than the happy path).
            let mut world = fake();
            let n_excl = spawn_hostile(&mut world, &px, &py, &pz, 0, [8, 0, 8], 128);
            assert!(n_excl < 289 * 4);
            assert_eq!(world.try_calls, n_excl);
        }
    }
}
