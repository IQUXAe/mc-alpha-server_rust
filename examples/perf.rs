//! Perf harness: deterministic world on seed 12345.
//!
//! Run: `cargo run --release --example perf [radius] [ticks]`
//! Defaults: radius 5 (11x11 chunks), 200 ticks.
//!
//! Phases measured separately (all wall-clock):
//! 1. chunk generation (`ensure_area`)
//! 2. full `tick_world` loop (avg/max per tick)
//! 3. micro: `get_block_id`, `colliding_boxes`, `take_block_updates`

use std::time::Instant;

use alpha_server::chunk::Chunk;
use alpha_server::entity::table::{AnimalKind, Entity, MobKind};
use alpha_server::entity::table::{MobEnt, PlayerEnt};
use alpha_server::world::World;

const SEED: i64 = 12345;

fn add_player(w: &mut World, x: f64, y: f64, z: f64) {
    let id = w.entities.alloc_id();
    let mut p = PlayerEnt::new(id, "perf");
    p.living.body.set_position(x, y, z);
    p.respawn_ticks = 0;
    w.entities.insert(Entity::Player(p));
}

fn add_mob(w: &mut World, kind: MobKind, x: f64, y: f64, z: f64) {
    let id = w.entities.alloc_id();
    let mut m = MobEnt::new(id, kind);
    m.living.body.set_position(x, y, z);
    w.entities.insert(Entity::Mob(m));
}

fn ground_y(w: &World, x: i32, z: i32) -> f64 {
    (w.get_height_value(x, z) + 1).max(65) as f64
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let radius: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(5);
    let ticks: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
    let mobs: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(24);

    // ---- Phase 1: generation ----
    let mut w = World::new(SEED);
    let t0 = Instant::now();
    w.ensure_area(0, 0, radius);
    let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let n_chunks = w.chunk_count();
    println!("gen: radius={radius} chunks={n_chunks} time={gen_ms:.1}ms");

    // Sanity: chunk (0,0) must exist and have terrain.
    assert!(w.has_chunk(0, 0), "chunk (0,0) missing after ensure_area");
    let top = w.get_height_value(8, 8);
    println!("sanity: height(8,8)={top}");
    assert!(top > 0, "terrain looks empty");

    // ---- Populate: 1 player + deterministic mob/animal crowd ----
    let px = 8.0;
    let pz = 8.0;
    let py = ground_y(&w, 8, 8);
    add_player(&mut w, px, py, pz);
    let kinds = [
        MobKind::Zombie,
        MobKind::Skeleton,
        MobKind::Spider,
        MobKind::Creeper,
    ];
    let animals = [
        AnimalKind::Pig,
        AnimalKind::Sheep,
        AnimalKind::Cow,
        AnimalKind::Chicken,
    ];
    for i in 0..mobs {
        let dx = ((i * 37) % 161) as f64 - 80.0;
        let dz = ((i * 53) % 161) as f64 - 80.0;
        let gy = ground_y(&w, (px + dx) as i32, (pz + dz) as i32);
        if i % 2 == 0 {
            add_mob(&mut w, kinds[i % kinds.len()], px + dx, gy, pz + dz);
        } else {
            let id = w.entities.alloc_id();
            let mut a = alpha_server::entity::table::AnimalEnt::new(id, animals[i % animals.len()]);
            a.living.body.set_position(px + dx, gy, pz + dz);
            w.entities.insert(Entity::Animal(a));
        }
    }
    // A stressor chest block + a few scheduled updates so the tick has work.
    // (Public API only: the example is an external crate.)
    w.set_block_id(8, py as i32, 9, 54);
    for i in 0..8 {
        w.schedule_block_update(8 + i, py as i32 + 1, 8, 51, 10);
    }
    println!("actors: {} entities", w.entities.alive_ids().len());

    // ---- Phase 2: ticks ----
    for _ in 0..20 {
        w.tick_world(); // warmup (caches, first-time paths)
    }
    let mut total = 0.0f64;
    let mut max = 0.0f64;
    let mut max_phases = [0u64; 7];
    let mut acc = [0u64; 8];
    for _ in 0..ticks {
        let t = Instant::now();
        w.tick_world();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        total += ms;
        let s = w.last_tick_stats;
        let phases = [
            s.spawners_us,
            s.furnaces_us,
            s.scheduled_us,
            s.random_us,
            s.entities_us,
            s.pickup_us,
            s.light_us,
        ];
        if ms > max {
            max = ms;
            max_phases = phases;
        }
        for (a, v) in acc.iter_mut().zip([
            s.spawners_us,
            s.furnaces_us,
            s.scheduled_us,
            s.random_us,
            s.entities_us,
            s.pickup_us,
            s.light_us,
            s.total_us,
        ]) {
            *a += v;
        }
    }
    let n = ticks as f64;
    println!(
        "tick: n={ticks} avg={:.3}ms max={:.3}ms total={:.1}ms time={}",
        total / n,
        max,
        total,
        w.time
    );
    println!(
        "phases avg us: spawners={:.0} furnaces={:.0} scheduled={:.0} random={:.0} entities={:.0} pickup={:.0} light={:.0}",
        acc[0] as f64 / n,
        acc[1] as f64 / n,
        acc[2] as f64 / n,
        acc[3] as f64 / n,
        acc[4] as f64 / n,
        acc[5] as f64 / n,
        acc[6] as f64 / n,
    );
    println!(
        "max-tick phases us: spawners={} furnaces={} scheduled={} random={} entities={} pickup={} light={}",
        max_phases[0],
        max_phases[1],
        max_phases[2],
        max_phases[3],
        max_phases[4],
        max_phases[5],
        max_phases[6],
    );

    // ---- Phase 3: micros ----
    let t = Instant::now();
    let mut acc = 0u64;
    for i in 0..200_000 {
        let x = (i % 176) - 80;
        let z = ((i / 176) % 176) - 80;
        acc += w.get_block_id(x, 64, z) as u64;
    }
    println!(
        "micro get_block_id x200k: {:.2}ms (acc={acc})",
        t.elapsed().as_secs_f64() * 1000.0
    );

    let t = Instant::now();
    let mut boxes = 0usize;
    for i in 0..20_000 {
        let x = ((i % 160) as f64) - 80.0;
        let z = (((i / 160) % 160) as f64) - 80.0;
        let mask = alpha_server::aabb::AxisAlignedBB::get_bounding_box(
            x,
            py - 2.0,
            z,
            x + 1.0,
            py + 1.0,
            z + 1.0,
        );
        boxes += w.colliding_boxes(&mask).len();
    }
    println!(
        "micro colliding_boxes x20k: {:.2}ms (boxes={boxes})",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Block-write batch shape (queue 2k writes through the public API).
    let t = Instant::now();
    for i in 0..2000 {
        w.set_block_id(8 + (i % 32), 70, 8 + ((i / 32) % 32), 1);
    }
    println!(
        "micro set_block_id x2k: {:.3}ms",
        t.elapsed().as_secs_f64() * 1000.0
    );

    // Keep Chunk import used in release builds without warnings.
    let _ = Chunk::new(0, 0);
}
