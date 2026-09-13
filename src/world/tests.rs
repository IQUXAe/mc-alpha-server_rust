
    use super::*;
    use crate::entity::table::{Body, MobEnt, PlayerEnt};

    fn world_with_floor() -> World {
        let mut w = World::new(1234);
        let mut c = Chunk::new(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                c.set_block_id(x, 63, z, 1); // stone floor
            }
        }
        c.generate_height_map();
        w.insert_chunk(c);
        w
    }

    fn add_player(w: &mut World, name: &str, x: f64, y: f64, z: f64) -> EntityId {
        let id = w.entities.alloc_id();
        let mut p = PlayerEnt::new(id, name);
        p.living.body.set_position(x, y, z);
        p.respawn_ticks = 0; // tests fight immediately; spawns get immunity
        w.entities.insert(crate::entity::table::Entity::Player(p));
        id
    }

    #[test]
    fn test_block_access_and_missing_chunks() {
        let mut w = world_with_floor();
        assert_eq!(w.get_block_id(3, 63, 4), 1);
        assert_eq!(w.get_block_id(1000, 63, 1000), 0);
        assert_eq!(w.get_block_id(0, 200, 0), 0);
        assert!(w.set_block_id(3, 64, 4, 5));
        assert_eq!(w.get_block_id(3, 64, 4), 5);
        assert!(!w.set_block_id(1000, 64, 1000, 5));
        assert!(!w.set_block_id(0, 200, 0, 5));
        assert_eq!(w.get_height_value(3, 4), 65);
    }

    #[test]
    fn test_set_block_regenerates_skylight() {
        let mut w = world_with_floor();
        // Lone pillar: the stone goes dark and the cell below gets
        // sideways leak (the fresh world starts fully dark, so any light
        // proves the pass). Regen is coalesced to refresh_light.
        w.set_block_id(3, 70, 4, 1);
        w.refresh_light();
        assert_eq!(w.saved_light_value(0, 3, 70, 4), 0);
        assert_eq!(w.saved_light_value(0, 3, 69, 4), 14);
        // Pull the pillar: full sky again.
        w.set_block_id(3, 70, 4, 0);
        w.refresh_light();
        assert_eq!(w.saved_light_value(0, 3, 69, 4), 15);
        assert_eq!(w.saved_light_value(0, 3, 127, 4), 15);
    }

    #[test]
    fn test_light_coalesces_per_chunk() {
        let mut w = world_with_floor();
        // Three writes, one chunk: a single regen covers them all.
        w.set_block_id(3, 70, 4, 1);
        w.set_block_id(4, 70, 4, 1);
        w.set_block_id(3, 71, 5, 1);
        assert_eq!(w.light_dirty.len(), 1);
        w.refresh_light();
        assert!(w.light_dirty.is_empty());
        assert_eq!(w.saved_light_value(0, 3, 69, 4), 14);
    }

    fn ceiling_world() -> World {
        // Two floored chunks with a two-wide stone lid at y=100 hugging
        // the border from the center side (x=14,15).
        let mut w = World::new(99);
        for (cx, cz) in [(0, 0), (1, 0)] {
            let mut c = Chunk::new(cx, cz);
            for x in 0..16 {
                for z in 0..16 {
                    c.set_block_id(x, 63, z, 1);
                }
            }
            c.generate_height_map();
            w.insert_chunk(c);
        }
        for lx in [14, 15] {
            w.chunks.get_mut(&(0, 0)).unwrap().set_block_id(lx, 100, 8, 1);
        }
        for (cx, cz) in [(0, 0), (1, 0)] {
            w.chunks.get_mut(&(cx, cz)).unwrap().generate_skylight_map();
        }
        w
    }

    #[test]
    fn test_set_block_on_border_refreshes_center() {
        // Same border edit two ways: the world path regenerates the
        // center chunk (fringe opens 14 -> 15); the chunk-direct path
        // leaves it stale. The neighbor chunk agrees on both paths by
        // design: the native BFS stays single-chunk (documented seam;
        // C++ spreads across, a future pass may teach it).
        let mut a = ceiling_world();
        a.set_block_id(15, 100, 8, 0);
        a.refresh_light();
        let mut b = ceiling_world();
        b.chunks.get_mut(&(0, 0)).unwrap().set_block_id(15, 100, 8, 0);
        assert_eq!(a.saved_light_value(0, 15, 99, 8), 15);
        assert_eq!(b.saved_light_value(0, 15, 99, 8), 14);
        assert_eq!(a.saved_light_value(0, 16, 99, 8), 15);
        assert_eq!(b.saved_light_value(0, 16, 99, 8), 15);
    }

    #[test]
    fn test_material_queries() {
        let w = world_with_floor();
        assert!(w.is_solid(3, 63, 4));
        // ID-list rule, not material: torch counts as solid here.
        assert!(!w.is_solid(3, 70, 4));
        assert!(!w.is_water(3, 63, 4));
        assert_eq!(w.material_at(9, 9, 9), Material::AIR);
    }

    #[test]
    fn test_light_defaults() {
        let w = world_with_floor();
        assert_eq!(w.saved_light_value(0, 1000, 64, 1000), 0);
        assert_eq!(w.saved_light_value(1, 1000, 64, 1000), 0);
        assert_eq!(w.saved_light_value(0, 0, 200, 0), 15);
        assert_eq!(w.saved_light_value(1, 0, 200, 0), 0);
        assert_eq!(w.saved_light_value(0, 0, -5, 0), 0);
        assert_eq!(w.block_light_value(1000, 64, 1000), 0);
    }

    #[test]
    fn test_colliding_boxes_floor() {
        let w = world_with_floor();
        // Box straddling the floor top picks up the stone cells beneath.
        let mask = AxisAlignedBB::get_bounding_box(3.2, 63.5, 4.2, 3.8, 64.5, 4.8);
        let boxes = w.colliding_boxes(&mask);
        assert!(!boxes.is_empty());
        assert!(boxes.iter().all(|b| b.max_y <= 64.0 + 1e-9));
        // Air mask collects nothing.
        let air = AxisAlignedBB::get_bounding_box(3.2, 70.0, 4.2, 3.8, 71.0, 4.8);
        assert!(w.colliding_boxes(&air).is_empty());
    }

    #[test]
    fn test_closest_player_strict_range() {
        let mut w = World::new(7);
        let a = add_player(&mut w, "a", 0.0, 64.0, 0.0);
        let _b = add_player(&mut w, "b", 100.0, 64.0, 0.0);
        assert_eq!(w.closest_player(3.0, 64.0, 4.0, 24.0), Some(a));
        assert_eq!(w.closest_player(3.0, 64.0, 4.0, 4.0), None);
        // Boundary is exclusive like C++ (strict <).
        assert_eq!(w.closest_player(24.0, 64.0, 0.0, 24.0), None);
    }

    fn add_item(w: &mut World, x: f64, y: f64, z: f64) -> EntityId {
        w.spawn_item_entity(35, 1, 0, x, y, z)
    }

    #[test]
    fn test_move_body_lands_on_floor() {
        let mut w = world_with_floor();
        let id = add_item(&mut w, 3.5, 70.0, 4.5);
        // Fall until resting: big steps converge on y=64 top face.
        for _ in 0..40 {
            w.move_body(id, 0.0, -3.0, 0.0);
        }
        let b = w.entities.get(id).unwrap().body().clone();
        assert!(b.on_ground);
        assert!((b.pos[1] - 64.125).abs() < 1e-6);
    }

    #[test]
    fn test_tick_item_falls_and_ages_out() {
        let mut w = world_with_floor();
        let id = add_item(&mut w, 3.5, 66.0, 4.5);
        for _ in 0..60 {
            w.tick_item(id);
            if w.entities.get(id).unwrap().body().on_ground {
                break;
            }
        }
        assert!(w.entities.get(id).unwrap().body().on_ground);
        // Age-out kills at 6000 regardless of rest.
        if let Some(crate::entity::table::Entity::Item(e)) = w.entities.get_mut(id) {
            e.age = 5999;
        }
        w.tick_item(id);
        assert!(w.entities.get(id).unwrap().body().dead);
    }

    #[test]
    fn test_tick_falling_places_on_landing() {
        use crate::entity::table::{Body, FallingEnt};
        let mut w = world_with_floor();
        let id = w.entities.alloc_id();
        let mut b = Body::new(id, 0.98, 0.98, 0.49);
        b.set_position(3.5, 70.0, 4.5);
        w.entities.insert(crate::entity::table::Entity::Falling(FallingEnt {
            body: b,
            block_id: 12,
            fall_time: 0,
        }));
        for _ in 0..60 {
            w.tick_falling(id);
            if w.entities.get(id).unwrap().body().dead {
                break;
            }
        }
        assert!(w.entities.get(id).unwrap().body().dead);
        // Landed on the floor top (y=64) and placed sand there.
        assert_eq!(w.get_block_id(3, 64, 4), 12);
    }

    #[test]
    fn test_is_replaceable_table() {
        assert!(is_replaceable(37));
        assert!(is_replaceable(83));
        assert!(is_replaceable(50));
        assert!(!is_replaceable(39));
        assert!(!is_replaceable(12));
        assert!(!is_replaceable(0));
    }

    fn add_zombie(w: &mut World, x: f64, y: f64, z: f64) -> EntityId {
    use crate::entity::table::{LivingBody, MobEnt};
        let id = w.entities.alloc_id();
        let mut l = LivingBody::new(id, 0.6, 1.9, 0.0);
        l.body.set_position(x, y, z);
        w.entities.insert(crate::entity::table::Entity::Mob(MobEnt {
            living: l,
            kind: crate::entity::table::MobKind::Zombie,
            target: None,
            attack_cooldown: 0,
            target_timer: 0,
            burn_ticks: 0,
            age: 0,
            path: Vec::new(),
            path_index: 0,
            swell_time: 0,
            swell_dir: -1,
        }));
        id
    }

    #[test]
    fn test_attack_kills_and_drops() {
        let mut w = world_with_floor();
        let id = add_zombie(&mut w, 3.5, 65.0, 4.5);
        let before = w.entities.len();
        w.attack_living(id, 100, None);
        assert!(w.entities.get(id).unwrap().body().dead);
        // Zombie drops 0..2 feathers: all spawns are items.
        let after = w.entities.len();
        assert!(after >= before && after <= before + 2);
        for oid in w.entities.alive_ids() {
            if oid == id {
                continue;
            }
            assert!(matches!(
                w.entities.get(oid).unwrap(),
                crate::entity::table::Entity::Item(_)
            ));
        }
    }

    #[test]
    fn test_attack_resist_and_knockback() {
        let mut w = world_with_floor();
        let id = add_zombie(&mut w, 3.5, 65.0, 4.5);
        let atk = add_zombie(&mut w, 8.5, 65.0, 4.5);
        w.attack_living(id, 6, Some(atk));
        let l = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.clone(),
            _ => unreachable!(),
        };
        assert_eq!((l.health, l.last_damage, l.hurt_time), (14, 6, 10));
        // Knocked away from the attacker (attacker east => push west).
        assert!(l.body.motion[0] < 0.0);
    }

    #[test]
    fn test_tick_living_drowns() {
        let mut w = world_with_floor();
        // Water column instead of air above the floor.
        for y in 64..68 {
            w.set_block_id(3, y, 4, 8);
        }
        let id = add_zombie(&mut w, 3.5, 65.0, 4.5);
        // Force air to the edge: one tick must drown for 2 damage.
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(id) {
            m.living.body.air = -19;
        }
        let hp_before = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        };
        w.tick_living(id);
        let hp_after = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        };
        assert_eq!(hp_before - hp_after, 2);
    }

    fn add_boat(w: &mut World, x: f64, y: f64, z: f64) -> EntityId {
        use crate::entity::table::BoatEnt;
        let id = w.entities.alloc_id();
        let mut b = Body::new(id, 1.5, 0.6, 0.3);
        b.set_position(x, y, z);
        w.entities.insert(crate::entity::table::Entity::Boat(BoatEnt {
            body: b,
            time_since_hit: 0,
            damage_taken: 0,
            forward_dir: 1,
        }));
        id
    }

    fn add_water_pool(w: &mut World) {
        // 4x2x4 pool at y 63..64 inside the floor chunk.
        for x in 6..10 {
            for z in 6..10 {
                w.set_block_id(x, 62, z, 1);
                w.set_block_id(x, 63, z, 8);
                w.set_block_id(x, 64, z, 8);
            }
        }
    }

    #[test]
    fn test_boat_floats_in_water() {
        let mut w = world_with_floor();
        add_water_pool(&mut w);
        let id = add_boat(&mut w, 8.0, 64.0, 8.0);
        w.tick_boat(id);
        let b = w.entities.get(id).unwrap().body().clone();
        assert!(!b.dead);
        assert!(b.motion[1] > 0.0);
    }

    #[test]
    fn test_boat_crash_drops_and_dies() {
        let mut w = world_with_floor();
        // Wall column east of the boat.
        for y in 64..67 {
            w.set_block_id(10, y, 8, 1);
        }
        // Motion clamps to ±0.4 before moving: start close enough to hit.
        let id = add_boat(&mut w, 9.0, 65.0, 8.0);
        if let Some(crate::entity::table::Entity::Boat(b)) = w.entities.get_mut(id) {
            b.body.motion = [3.0, 0.0, 3.0];
        }
        let before = w.entities.len();
        w.tick_boat(id);
        assert!(w.entities.get(id).unwrap().body().dead);
        // 3 planks + 2 sticks spawned.
        assert_eq!(w.entities.len(), before + 5);
    }

    #[test]
    fn test_boat_damage_breaks_past_40() {
        let mut w = world_with_floor();
        let id = add_boat(&mut w, 8.0, 65.0, 8.0);
        assert!(!w.damage_boat(id, 3));
        let b = w.entities.get(id).unwrap();
        let (dir, time) = match b {
            crate::entity::table::Entity::Boat(b) => (b.forward_dir, b.time_since_hit),
            _ => unreachable!(),
        };
        assert_eq!((dir, time), (-1, 10));
        assert!(w.damage_boat(id, 5));
        assert!(w.entities.get(id).unwrap().body().dead);
    }

    use crate::entity::table::{AnimalEnt, AnimalKind, MobKind};

    fn add_mob(w: &mut World, kind: MobKind, x: f64, y: f64, z: f64) -> EntityId {
        let id = w.entities.alloc_id();
        let mut m = MobEnt::new(id, kind);
        m.living.body.set_position(x, y, z);
        w.entities.insert(crate::entity::table::Entity::Mob(m));
        id
    }

    fn add_animal(w: &mut World, kind: AnimalKind, x: f64, y: f64, z: f64) -> EntityId {
        let id = w.entities.alloc_id();
        let mut a = AnimalEnt::new(id, kind);
        a.living.body.set_position(x, y, z);
        w.entities.insert(crate::entity::table::Entity::Animal(a));
        id
    }

    fn add_floor_chunk(w: &mut World, cx: i32, cz: i32) {
        let mut c = Chunk::new(cx, cz);
        for x in 0..16 {
            for z in 0..16 {
                c.set_block_id(x, 63, z, 1);
            }
        }
        c.generate_height_map();
        w.insert_chunk(c);
    }

    fn set_sky(w: &mut World, x: i32, y: i32, z: i32, v: u8) {
        if let Some(c) = w.chunks.get_mut(&(x.div_euclid(16), z.div_euclid(16))) {
            c.set_light_value(0, x.rem_euclid(16), y, z.rem_euclid(16), v);
        }
    }

    fn mob_health(w: &World, id: EntityId) -> i16 {
        match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_dayclock_and_sky_queries() {
        let mut w = world_with_floor();
        w.time = 0;
        assert!(w.is_daytime());
        w.time = 11999;
        assert!(w.is_daytime());
        w.time = 12000;
        assert!(!w.is_daytime());
        w.time = 18000;
        assert!(!w.is_daytime());
        // Floor top (y=64) sees sky, the stone itself does not.
        assert!(w.can_see_sky(3, 64, 4));
        assert!(!w.can_see_sky(3, 63, 4));
        assert!(!w.can_see_sky(1000, 64, 1000));
        assert!(w.can_see_sky(3, 200, 3));
        assert!(!w.can_see_sky(3, -1, 4));
        // Fresh chunks are dark; setting skylight lifts brightness to full.
        assert_eq!(w.brightness(3, 64, 4), 0.0);
        set_sky(&mut w, 3, 64, 4, 15);
        assert_eq!(w.brightness(3, 64, 4), 1.0);
    }

    #[test]
    fn test_mob_acquires_and_chases_player() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 10.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 3.5, 64.0, 4.5);
        w.tick_mob(zombie);
        assert_eq!(
            match w.entities.get(zombie).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.target,
                _ => unreachable!(),
            },
            Some(player)
        );
        for _ in 0..40 {
            w.tick_mob(zombie);
        }
        let x = w.entities.get(zombie).unwrap().body().pos[0];
        assert!(x > 3.5, "zombie should walk east toward the player, x={x}");
    }

    #[test]
    fn test_mob_wanders_without_target() {
        let mut w = world_with_floor();
        let zombie = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 8.5);
        for _ in 0..300 {
            w.tick_mob(zombie);
        }
        let p = w.entities.get(zombie).unwrap().body().pos;
        let moved = (p[0] - 8.5).abs() + (p[2] - 8.5).abs();
        assert!(moved > 0.3, "targetless zombie should wander, moved={moved}");
        assert_eq!(mob_health(&w, zombie), 20);
    }

    #[test]
    fn test_mob_burn_schedule_and_small_fire_rule() {
        let mut w = world_with_floor();
        let zombie = add_mob(&mut w, MobKind::Zombie, 3.5, 64.0, 4.5);
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(zombie) {
            m.burn_ticks = 21;
        }
        w.tick_mob(zombie);
        let (burn, fire) = match w.entities.get(zombie).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.burn_ticks, m.living.body.fire),
            _ => unreachable!(),
        };
        assert_eq!((burn, fire), (20, 20));
        assert_eq!(mob_health(&w, zombie), 20); // 21 % 20 != 0: no hit yet
        w.tick_mob(zombie);
        assert_eq!(mob_health(&w, zombie), 19); // 20 % 20 == 0: one burn damage
        // Small fires go out once the burn ends.
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(zombie) {
            m.burn_ticks = 0;
            m.living.body.fire = 10;
        }
        w.tick_mob(zombie);
        assert_eq!(
            match w.entities.get(zombie).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.living.body.fire,
                _ => unreachable!(),
            },
            0
        );
    }

    #[test]
    fn test_zombie_ignites_in_daylight() {
        let mut w = world_with_floor();
        w.time = 6000; // noon
        set_sky(&mut w, 3, 64, 4, 15);
        let zombie = add_mob(&mut w, MobKind::Zombie, 3.5, 64.0, 4.5);
        for _ in 0..500 {
            // Pin to the lit column: untethered it wanders off into the
            // dark, which is correct AI but a useless ignition test.
            if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(zombie) {
                m.living.body.set_position(3.5, 64.0, 4.5);
            }
            w.tick_mob(zombie);
            let burn = match w.entities.get(zombie).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.burn_ticks,
                _ => unreachable!(),
            };
            if burn > 0 {
                return;
            }
        }
        panic!("daylit zombie should have caught fire within 500 ticks");
    }

    #[test]
    fn test_spider_hunts_only_in_dark() {
        let mut w = world_with_floor();
        let _player = add_player(&mut w, "steve", 6.5, 64.0, 4.5);
        let spider = add_mob(&mut w, MobKind::Spider, 3.5, 64.0, 4.5);
        set_sky(&mut w, 3, 64, 4, 15);
        w.tick_mob(spider);
        assert_eq!(
            match w.entities.get(spider).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.target,
                _ => unreachable!(),
            },
            None
        );
        // Night falls: the next acquire (5-tick refresh) locks on.
        set_sky(&mut w, 3, 64, 4, 0);
        for _ in 0..6 {
            w.tick_mob(spider);
        }
        assert!(
            match w.entities.get(spider).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.target,
                _ => unreachable!(),
            }
            .is_some()
        );
    }

    #[test]
    fn test_mob_takes_fall_damage() {
        let mut w = world_with_floor();
        let zombie = add_mob(&mut w, MobKind::Zombie, 8.5, 75.0, 8.5);
        for _ in 0..200 {
            w.tick_mob(zombie);
            if w.entities.get(zombie).unwrap().body().on_ground {
                break;
            }
        }
        assert!(w.entities.get(zombie).unwrap().body().on_ground);
        assert!(mob_health(&w, zombie) < 20);
    }

    #[test]
    fn test_chicken_lays_eggs_and_ignores_fall() {
        let mut w = world_with_floor();
        let chicken = add_animal(&mut w, AnimalKind::Chicken, 8.5, 70.0, 8.5);
        if let Some(crate::entity::table::Entity::Animal(a)) = w.entities.get_mut(chicken) {
            a.egg_timer = 2;
        }
        for _ in 0..200 {
            w.tick_animal(chicken);
            if w.entities.get(chicken).unwrap().body().on_ground {
                break;
            }
        }
        assert!(w.entities.get(chicken).unwrap().body().on_ground);
        // Fell ~6 blocks: a zombie would be hurt, the chicken is unharmed
        // (chicken max HP is 4 per Java EntityChicken.java:16).
        assert_eq!(
            match w.entities.get(chicken).unwrap() {
                crate::entity::table::Entity::Animal(a) => a.living.health,
                _ => unreachable!(),
            },
            4
        );
        // ...and laid an egg somewhere along the way.
        let eggs = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|oid| {
                matches!(
                    w.entities.get(*oid).unwrap(),
                    crate::entity::table::Entity::Item(e) if e.item_id == 344
                )
            })
            .count();
        assert!(eggs >= 1);
        assert!(
            match w.entities.get(chicken).unwrap() {
                crate::entity::table::Entity::Animal(a) => a.egg_timer,
                _ => unreachable!(),
            } >= 6000
        );
    }

    #[test]
    fn test_push_neighbors_shoves() {
        let mut w = world_with_floor();
        let z1 = add_mob(&mut w, MobKind::Zombie, 3.5, 64.0, 4.5);
        let z2 = add_mob(&mut w, MobKind::Zombie, 3.7, 64.0, 4.5);
        w.tick_mob(z1);
        let m2 = w.entities.get(z2).unwrap().body().motion;
        assert!(
            m2[0] != 0.0 || m2[2] != 0.0,
            "overlapping neighbor should be shoved, motion={m2:?}"
        );
    }



    #[test]
    fn test_native_path_points() {
        let mut w = world_with_floor();
        let zombie = add_mob(&mut w, MobKind::Zombie, 3.5, 64.0, 4.5);
        let pts = w.path_block_points(zombie, [10, 64, 4], 16.0);
        assert!(!pts.is_empty());
        // Path to our own block is empty (start == end, like nullptr).
        assert!(w.path_block_points(zombie, [3, 64, 4], 16.0).is_empty());
    }

    #[test]
    fn test_tick_world_advances_and_dispatches() {
        let mut w = world_with_floor();
        let item = w.spawn_item_entity(3, 1, 0, 3.5, 66.0, 4.5);
        w.tick_world();
        assert_eq!(w.time, 1);
        // The loose item ticked (gravity pulled it down).
        assert!(w.entities.get(item).unwrap().body().pos[1] < 66.0);
    }

    #[test]
    fn test_tick_world_pickup_and_purge() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        let item = w.spawn_item_entity(3, 5, 0, 3.5, 64.2, 4.5);
        if let Some(crate::entity::table::Entity::Item(e)) = w.entities.get_mut(item) {
            e.pickup_delay = 0;
        }
        let zombie = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 4.5);
        w.attack_living(zombie, 100, None);
        assert!(w.entities.get(zombie).unwrap().body().dead);
        w.tick_world();
        // Dirt merged into the held slot, the item row is gone...
        assert_eq!(
            w.player_held(player).map(|s| (s.item_id, s.count)),
            Some((3, 5))
        );
        let dirt: i32 = match w.entities.get(player).unwrap() {
            crate::entity::table::Entity::Player(p) => {
                p.inventory.main.iter().filter_map(|s| *s).map(|s| s.count).sum()
            }
            _ => unreachable!(),
        };
        assert_eq!(dirt, 5);
        // ...and the corpse row is retained 20 ticks for the death animation
        // (purge_dead holds dead mobs; destroy follows the status-3).
        assert!(w.entities.get(zombie).unwrap().body().dead);
        for _ in 0..20 {
            w.tick_world();
        }
        assert!(w.entities.get(zombie).is_none());
        assert!(w.entities.get(item).is_none());
        // The full take was recorded for the server tick's collect fan-out.
        assert_eq!(w.item_pickups, vec![(item, player)]);
    }

    #[test]
    fn test_hand_hit_knocks_pig_back() {
        // Hand hit from the east must shove the pig west immediately and
        // displace it on the next tick (knockback pipeline + heading).
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 5.5, 64.0, 4.5);
        let pig = add_animal(&mut w, AnimalKind::Pig, 3.5, 64.0, 4.5);
        w.attack_living(pig, 1, Some(player));
        let motion = match w.entities.get(pig).unwrap() {
            crate::entity::table::Entity::Animal(a) => a.living.body.motion,
            _ => unreachable!(),
        };
        assert!(motion[0] < -0.3, "westward shove, got {motion:?}");
        assert_eq!(motion[1], 0.4);
        w.tick_world();
        let after = match w.entities.get(pig).unwrap() {
            crate::entity::table::Entity::Animal(a) => a.living.body.pos[0],
            _ => unreachable!(),
        };
        assert!(after < 3.5 - 0.15, "pig displaced west, now at {after}");
    }

    #[test]
    fn test_native_drop_ids_spot_checks() {
        // Vanilla idDropped/quantityDropped spot checks (Java Block*).
        assert_eq!(World::native_drop_ids(63), (323, 1, 0));
        assert_eq!(World::native_drop_ids(68), (323, 1, 0));
        assert_eq!(World::native_drop_ids(62), (61, 1, 0));
        assert_eq!(World::native_drop_ids(82), (337, 4, 0));
        assert_eq!(World::native_drop_ids(43), (44, 1, 0));
        assert_eq!(World::native_drop_ids(1), (4, 1, 0));
        assert_eq!(World::native_drop_ids(16), (263, 1, 0));
        assert_eq!(World::native_drop_ids(56), (264, 1, 0));
        assert_eq!(World::native_drop_ids(39), (39, 1, 0));
        assert_eq!(World::native_drop_ids(40), (40, 1, 0));
        for bid in [20, 47, 52, 79] {
            assert_eq!(World::native_drop_ids(bid).1, 0, "block {bid} drops nothing");
        }
        // Snow harvest: layer -> 1 snowball, block -> 4 snowballs.
        assert_eq!(World::native_drop_ids(78), (332, 1, 0));
        assert_eq!(World::native_drop_ids(80), (332, 4, 0));
    }

    #[test]
    fn test_rolled_drops_doors_redstone_gravel_leaves() {
        let mut w = world_with_floor();
        // Doors: upper half nothing, lower wood 324 / iron 330.
        assert_eq!(w.rolled_drop_ids(64, 8), (0, 0));
        assert_eq!(w.rolled_drop_ids(64, 0), (324, 1));
        assert_eq!(w.rolled_drop_ids(71, 0), (330, 1));
        // Redstone dust comes 4-5.
        for _ in 0..20 {
            let (d, q) = w.rolled_drop_ids(73, 0);
            assert_eq!(d, 331);
            assert!((4..=5).contains(&q), "{q}");
        }
        // Gravel flints or stays gravel; leaves sapling or nothing.
        for _ in 0..50 {
            assert!(matches!(w.rolled_drop_ids(13, 0), (318, 1) | (13, 1)));
            assert!(matches!(w.rolled_drop_ids(18, 0), (6, 1) | (0, 0)));
        }
    }

    #[test]
    fn test_cactus_contact_hurts() {
        // Standing in cactus deals 1 through the pipeline (BlockCactus).
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        w.set_block_id(3, 64, 4, 81);
        w.move_body(player, 0.0, 0.0, 0.0);
        let hp = match w.entities.get(player).unwrap() {
            crate::entity::table::Entity::Player(p) => p.living.health,
            _ => unreachable!(),
        };
        assert_eq!(hp, 19);
    }

    fn furnace_tile_with(input: (i32, i32), fuel: (i32, i32)) -> TileData {
        use crate::inventory::ItemStack;
        let mut s = crate::tile_entity::furnace::furnace_create();
        s.slots[0] = ItemStack::new(input.0, input.1, 0);
        s.slots[1] = ItemStack::new(fuel.0, fuel.1, 0);
        TileData::Furnace(s)
    }

    #[test]
    fn test_tick_furnaces_swaps_idle_to_lit() {
        let mut w = world_with_floor();
        w.set_block_id(2, 64, 2, 61);
        w.set_block_meta(2, 64, 2, 3);
        w.tiles.insert((2, 64, 2), furnace_tile_with((4, 1), (5, 1)));
        w.take_block_updates();
        w.tick_furnaces();
        // Lit, facing (meta 3) preserved, update queued for broadcast.
        assert_eq!(w.get_block_id(2, 64, 2), 62);
        assert_eq!(w.get_block_meta(2, 64, 2), 3);
        assert_eq!(w.take_block_updates(), vec![[2, 64, 2]]);
        // Tile row survives the swap.
        assert!(matches!(w.tiles.get(&(2, 64, 2)), Some(TileData::Furnace(_))));
        // Steady burn: no further swap, no new update.
        w.tick_furnaces();
        assert_eq!(w.get_block_id(2, 64, 2), 62);
        assert!(w.take_block_updates().is_empty());
    }

    #[test]
    fn test_tick_furnaces_swaps_lit_to_idle_when_fuel_runs_out() {
        let mut w = world_with_floor();
        w.set_block_id(2, 64, 2, 62);
        w.tiles.insert((2, 64, 2), furnace_tile_with((4, 1), (280, 1)));
        w.take_block_updates();
        // Light it (stick: 100 ticks of burn).
        w.tick_furnaces();
        assert!(w.take_block_updates().is_empty()); // already lit: no flip
        for _ in 0..200 {
            w.tick_furnaces();
        }
        // Burnt out mid-run: back to idle, one update queued.
        assert_eq!(w.get_block_id(2, 64, 2), 61);
        assert_eq!(w.take_block_updates(), vec![[2, 64, 2]]);
    }

    #[test]
    fn test_tick_furnaces_skips_chest_and_sign_tiles() {
        let mut w = world_with_floor();
        w.set_block_id(2, 64, 2, 54);
        w.tiles.insert(
            (2, 64, 2),
            TileData::Chest(crate::tile_entity::chest::chest_create()),
        );
        w.take_block_updates();
        w.tick_furnaces();
        assert_eq!(w.get_block_id(2, 64, 2), 54);
        assert!(w.take_block_updates().is_empty());
    }

    #[test]
    fn test_tick_world_runs_furnaces() {
        let mut w = world_with_floor();
        w.set_block_id(2, 64, 2, 61);
        w.tiles.insert((2, 64, 2), furnace_tile_with((12, 1), (263, 1)));
        w.take_block_updates();
        w.tick_world();
        assert_eq!(w.get_block_id(2, 64, 2), 62);
        assert_eq!(w.take_block_updates(), vec![[2, 64, 2]]);
    }

    #[test]
    fn test_block_updates_queue_on_write_and_dedup_on_take() {
        let mut w = world_with_floor();
        w.take_block_updates();
        // No-op writes (same id, missing chunk) queue nothing.
        assert!(w.set_block_id(3, 64, 4, 1));
        w.take_block_updates();
        assert!(!w.set_block_id(3, 64, 4, 1));
        assert!(w.take_block_updates().is_empty());
        assert!(!w.set_block_id(1000, 64, 1000, 5));
        assert!(w.take_block_updates().is_empty());
        // Repeated writes to one cell collapse to a single entry.
        assert!(w.set_block_id(3, 64, 4, 0));
        assert!(w.set_block_meta(3, 64, 4, 2));
        assert!(w.set_block_id(3, 64, 4, 5));
        assert_eq!(w.take_block_updates(), vec![[3, 64, 4]]);
        assert!(w.take_block_updates().is_empty());
    }

    #[test]
    fn test_tick_player_decays_respawn() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        if let Some(crate::entity::table::Entity::Player(p)) = w.entities.get_mut(player) {
            p.respawn_ticks = 5;
        }
        w.tick_player(player);
        assert_eq!(
            match w.entities.get(player).unwrap() {
                crate::entity::table::Entity::Player(p) => p.respawn_ticks,
                _ => unreachable!(),
            },
            4
        );
    }

    #[test]
    fn test_spawn_hostile_mobs() {
        let mut w = world_with_floor();
        let _p = add_player(&mut w, "steve", 8.5, 64.0, 8.5);
        // Far, dark floor chunks: inside the 24-block exclusion near the
        // player nothing may spawn, so the pens sit out at x 64+.
        for cx in 4..8 {
            for cz in -2..3 {
                add_floor_chunk(&mut w, cx, cz);
            }
        }
        let mut spawned = 0;
        for _ in 0..600 {
            spawned += w.spawn_hostile_mobs();
            if spawned > 0 {
                break;
            }
        }
        assert!(spawned > 0, "dark pens should yield hostile spawns");
        for oid in w.entities.alive_ids() {
            if !matches!(w.entities.get(oid).unwrap(), crate::entity::table::Entity::Player(_)) {
                assert!(matches!(
                    w.entities.get(oid).unwrap(),
                    crate::entity::table::Entity::Mob(_)
                ));
            }
        }
    }

    #[test]
    fn test_spawn_passive_mobs() {
        let mut w = world_with_floor();
        let _p = add_player(&mut w, "steve", 8.5, 64.0, 8.5);
        for cx in 4..8 {
            for cz in -2..3 {
                add_floor_chunk(&mut w, cx, cz);
            }
        }
        // Lit grass pens for the herds. Origins must sit exactly one
        // above the grass (y is drawn uniform over 128), so allow a wide
        // pass budget; the seeded stream keeps it deterministic.
        for x in 64..96 {
            for z in -16..16 {
                w.set_block_id(x, 63, z, 2);
                set_sky(&mut w, x, 64, z, 15);
                set_sky(&mut w, x, 65, z, 15);
            }
        }
        let mut spawned = 0;
        for _ in 0..3000 {
            spawned += w.spawn_passive_mobs();
            if spawned > 0 {
                break;
            }
        }
        assert!(spawned > 0, "lit grass pens should yield passive spawns");
    }

    #[test]
    fn test_sand_falls_on_schedule() {
        let mut w = world_with_floor();
        // Schedules need loaded surroundings (radius 8 like vanilla).
        for cx in -1..=0 {
            for cz in -1..=0 {
                if cx == 0 && cz == 0 {
                    continue;
                }
                add_floor_chunk(&mut w, cx, cz);
            }
        }
        w.set_block_id(3, 66, 4, 12);
        w.schedule_block_update(3, 66, 4, 12, 1);
        w.tick_world();
        // The tick fired: sand left and a falling row took over.
        assert_eq!(w.get_block_id(3, 66, 4), 0);
        assert!(w.entities.alive_ids().into_iter().any(|oid| matches!(
            w.entities.get(oid).unwrap(),
            crate::entity::table::Entity::Falling(f) if f.block_id == 12
        )));
    }

    #[test]
    fn test_scheduled_queue_gates_and_stales() {
        let mut w = world_with_floor();
        for cx in -1..=0 {
            for cz in -1..=0 {
                if cx == 0 && cz == 0 {
                    continue;
                }
                add_floor_chunk(&mut w, cx, cz);
            }
        }
        w.set_block_id(3, 66, 4, 12);
        // Future entry waits.
        w.schedule_block_update(3, 66, 4, 12, 5);
        assert_eq!(w.scheduled.len(), 1);
        w.tick_world();
        assert_eq!(w.scheduled.len(), 1);
        assert_eq!(w.get_block_id(3, 66, 4), 12);
        // Stale entry (wrong id) is dropped without effect; the future
        // entry from above is still queued.
        w.schedule_block_update(5, 66, 5, 12, 0);
        w.tick_world();
        assert_eq!(w.scheduled.len(), 1);
        assert_eq!(w.get_block_id(5, 66, 5), 0);
        // Same cell, different ids: Java keeps both (identity includes
        // the id); same cell+id twice keeps one.
        w.schedule_block_update(7, 66, 7, 13, 2);
        w.schedule_block_update(7, 66, 7, 12, 2);
        w.schedule_block_update(7, 66, 7, 13, 2);
        let mut dups: Vec<u8> = w
            .scheduled
            .iter()
            .filter(|((_, x, y, z, _), _)| (*x, *y, *z) == (7, 66, 7))
            .map(|((_, _, _, _, id), _)| *id)
            .collect();
        dups.sort_unstable();
        assert_eq!(dups, vec![12, 13]);
        // Processed entries free the dedup slot: re-scheduling the same
        // cell+id afterwards queues again (guards the set/map sync).
        for _ in 0..5 {
            w.tick_world();
        }
        assert!(w.scheduled.is_empty());
        w.schedule_block_update(7, 66, 7, 12, 0);
        assert_eq!(w.scheduled.len(), 1);
    }



    #[test]
    fn test_torch_pops_when_dug() {
        let mut w = world_with_floor();
        w.set_block_id(3, 64, 4, 50);
        w.set_block_meta(3, 64, 4, 5); // floor mount, like placement sets
        w.apply_set_notify(3, 63, 4, 0);
        assert_eq!(w.get_block_id(3, 64, 4), 0);
        assert!(w.entities.alive_ids().into_iter().any(|oid| matches!(
            w.entities.get(oid).unwrap(),
            crate::entity::table::Entity::Item(e) if e.item_id == 50
        )));
    }

    #[test]
    fn test_sign_pops_without_support() {
        let mut w = world_with_floor();
        // Wall sign facing +z support.
        w.set_block_id(3, 65, 5, 1);
        w.set_block_id(3, 65, 4, 68);
        w.set_block_meta(3, 65, 4, 2);
        w.apply_set_notify(3, 65, 5, 0);
        assert_eq!(w.get_block_id(3, 65, 4), 0);
        assert!(w.entities.alive_ids().into_iter().any(|oid| matches!(
            w.entities.get(oid).unwrap(),
            crate::entity::table::Entity::Item(e) if e.item_id == 323
        )));
        // Supported post sign stays.
        w.set_block_id(5, 64, 5, 63);
        w.neighbor_changed(5, 64, 5);
        assert_eq!(w.get_block_id(5, 64, 5), 63);
    }

    #[test]
    fn test_flowers_pop_in_dark_random_ticks() {
        let mut w = world_with_floor();
        let _p = add_player(&mut w, "steve", 8.5, 64.0, 8.5);
        for x in 0..16 {
            for z in 0..16 {
                w.set_block_id(x, 64, z, 37);
            }
        }
        for _ in 0..300 {
            w.random_block_ticks();
        }
        let left: usize = (0..16)
            .flat_map(|x| (0..16).map(move |z| (x, z)))
            .filter(|(x, z)| w.get_block_id(*x, 64, *z) == 37)
            .count();
        assert!(left < 256, "dark flowers should pop under random ticks, left={left}");
    }

    #[test]
    fn test_sapling_grows_or_restores() {
        let mut w = world_with_floor();
        w.set_block_id(8, 63, 8, 3);
        for x in 0..16 {
            for z in 0..16 {
                for y in 64..80 {
                    set_sky(&mut w, x, y, z, 15);
                }
            }
        }
        let mut grew = 0;
        for seed in 0..20 {
            w.set_block_id(8, 64, 8, 6);
            w.grow_sapling(8, 64, 8, 6, seed);
            let logs: usize = (0..16)
                .flat_map(|x| (0..16).flat_map(move |z| (64..96).map(move |y| (x, y, z))))
                .filter(|(x, y, z)| w.get_block_id(*x, *y, *z) == 17)
                .count();
            if logs > 0 {
                grew += 1;
            } else {
                // Failed generation restores the sapling.
                assert_eq!(w.get_block_id(8, 64, 8), 6);
            }
            // Scrub the tree for the next seed.
            for x in 0..16 {
                for z in 0..16 {
                    for y in 64..96 {
                        let id = w.get_block_id(x, y, z);
                        if id == 17 || id == 18 {
                            w.set_block_id(x, y, z, 0);
                        }
                    }
                }
            }
        }
        assert!(grew > 0, "open lit saplings should grow trees");
    }

    #[test]
    fn test_unload_spills_and_recalls() {
        let mut w = world_with_floor();
        let _p = add_player(&mut w, "steve", 8.5, 64.0, 8.5);
        // Tighten the unload radius so entities can sit in an unloading
        // chunk yet stay inside the 128-block despawn range (vanilla kills
        // far mobs before unload would ever spill them).
        w.unload_radius = 5;
        add_floor_chunk(&mut w, 7, 0);
        let item = w.spawn_item_entity(3, 2, 0, 7.0 * 16.0 + 8.5, 65.0, 8.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 7.0 * 16.0 + 8.5, 65.0, 8.5);
        w.time = 99;
        w.tick_world();
        assert_eq!(w.time, 100);
        assert!(!w.has_chunk(7, 0));
        assert!(w.entities.get(item).is_none());
        assert!(w.entities.get(zombie).is_none());
        // Spawn chunks stay put.
        assert!(w.has_chunk(0, 0));
        assert!(w.recall_chunk(7, 0));
        assert!(w.has_chunk(7, 0));
        let items = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|oid| {
                matches!(
                    w.entities.get(*oid).unwrap(),
                    crate::entity::table::Entity::Item(e) if e.item_id == 3 && e.count == 2
                )
            })
            .count();
        let mobs = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|oid| {
                matches!(
                    w.entities.get(*oid).unwrap(),
                    crate::entity::table::Entity::Mob(m) if m.kind == MobKind::Zombie
                )
            })
            .count();
        assert_eq!((items, mobs), (1, 1));
    }

    #[test]
    fn test_ensure_chunk_builds_terrain() {
        let mut w = World::new(1234);
        assert!(!w.has_chunk(0, 0));
        w.ensure_chunk(0, 0);
        assert!(w.has_chunk(0, 0));
        let h = w.get_height_value(8, 8);
        assert!((1..127).contains(&h), "generated column should have terrain, h={h}");
        // Stone body under the surface.
        let mut stone = false;
        for y in 0..h {
            if w.get_block_id(8, y, 8) == 1 {
                stone = true;
                break;
            }
        }
        assert!(stone);
        assert!(w.chunks.get(&(0, 0)).unwrap().is_terrain_populated);
    }

    #[test]
    fn test_ensure_chunk_deterministic_and_idempotent() {
        let sample = |w: &mut World| -> Vec<u8> {
            w.ensure_chunk(3, -2);
            let mut v = Vec::new();
            for x in (0..16).step_by(3) {
                for z in (0..16).step_by(5) {
                    for y in (0..128).step_by(7) {
                        v.push(w.get_block_id(3 * 16 + x, y, -2 * 16 + z));
                    }
                }
            }
            v
        };
        let mut a = World::new(777);
        let va = sample(&mut a);
        let mut b = World::new(777);
        assert_eq!(sample(&mut b), va);
        // Second ensure leaves blocks untouched (no double decoration).
        assert_eq!(sample(&mut a), va);
    }

    #[test]
    fn test_ensure_area_populates_square() {
        let mut w = World::new(4242);
        w.ensure_area(0, 0, 1);
        for dx in -1..=1 {
            for dz in -1..=1 {
                assert!(w.has_chunk(dx, dz));
                assert!(w.chunks.get(&(dx, dz)).unwrap().is_terrain_populated);
            }
        }
    }

    fn player_health(w: &World, id: EntityId) -> i16 {
        match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Player(p) => p.living.health,
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_ray_trace_clear() {
        let mut w = world_with_floor();
        // Open air reads visible.
        assert!(w.ray_trace_clear([3.5, 66.0, 4.5], [10.5, 66.0, 4.5]));
        // Stone wall blocks.
        for y in 64..68 {
            w.set_block_id(7, y, 4, 1);
        }
        assert!(!w.ray_trace_clear([3.5, 66.0, 4.5], [10.5, 66.0, 4.5]));
        // Fluids let sight through; flowers block (canCollideCheck is only
        // false for BlockFluid).
        w.set_block_id(7, 66, 4, 8);
        assert!(w.ray_trace_clear([3.5, 66.0, 4.5], [10.5, 66.0, 4.5]));
        w.set_block_id(7, 66, 4, 37);
        assert!(!w.ray_trace_clear([3.5, 66.0, 4.5], [10.5, 66.0, 4.5]));
        // NaN reads visible like the C++ early-out.
        assert!(w.ray_trace_clear([f64::NAN, 66.0, 4.5], [10.5, 66.0, 4.5]));
    }

    #[test]
    fn test_zombie_punches_player() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 4.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 3.5, 64.0, 4.5);
        w.tick_mob(zombie);
        assert_eq!(player_health(&w, player), 15);
        assert_eq!(
            match w.entities.get(zombie).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.attack_cooldown,
                _ => unreachable!(),
            },
            20
        );
        // Cooldown gates the next punch.
        w.tick_mob(zombie);
        assert_eq!(player_health(&w, player), 15);
    }

    #[test]
    fn test_skeleton_looses_arrow() {
        let mut w = world_with_floor();
        let _player = add_player(&mut w, "steve", 10.5, 64.0, 4.5);
        let skel = add_mob(&mut w, MobKind::Skeleton, 3.5, 64.0, 4.5);
        w.tick_mob(skel);
        let arrows: Vec<EntityId> = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|oid| matches!(w.entities.get(*oid).unwrap(), crate::entity::table::Entity::Arrow(_)))
            .collect();
        assert_eq!(arrows.len(), 1);
        let (shooter, start) = match w.entities.get(arrows[0]).unwrap() {
            crate::entity::table::Entity::Arrow(a) => (a.shooter_id, a.body.pos),
            _ => unreachable!(),
        };
        assert_eq!(shooter, skel);
        for _ in 0..5 {
            w.tick_arrow(arrows[0]);
        }
        let end = w.entities.get(arrows[0]).unwrap().body().pos;
        assert!(end[0] > start[0], "arrow should fly east, {start:?} -> {end:?}");
    }

    #[test]
    fn test_spider_pounces_in_range() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 7.5, 64.0, 4.5);
        let spider = add_mob(&mut w, MobKind::Spider, 3.5, 64.0, 4.5);
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(spider) {
            m.living.body.on_ground = true;
        }
        let mut snap = w.creature_snapshot(spider).unwrap();
        for _ in 0..200 {
            w.mob_attack(spider, MobKind::Spider, player, 4.0, &mut snap);
            let my = w.entities.get(spider).unwrap().body().motion[1];
            if my == 0.4 {
                return; // pounce fired (rng(10) gate passed)
            }
        }
        panic!("spider should pounce from 4 blocks within 200 tries");
    }

    #[test]
    fn test_creeper_explodes_next_to_player() {
        // Deterministic ballistics: blast directly at d=1 with clear LOS.
        // Java formula: v=(1-1/3)=2/3 -> (v²+v)/2*8*3+1 = 14 damage.
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 4.5, 64.0, 4.5);
        let creeper = add_mob(&mut w, MobKind::Creeper, 3.5, 64.0, 4.5);
        w.blast(3.5, 64.0, 4.5, 3.0, Some(creeper));
        assert_eq!(player_health(&w, player), 6);
        // The fuse path still kills the creeper (fresh world, no knockback).
        let mut w2 = world_with_floor();
        let _p2 = add_player(&mut w2, "steve", 4.5, 64.0, 4.5);
        let c2 = add_mob(&mut w2, MobKind::Creeper, 3.5, 64.0, 4.5);
        for _ in 0..60 {
            // Pin: knockback from the eventual blast must not matter here;
            // the fuse needs dist<3 to light and <7 to hold.
            if let Some(e) = w2.entities.get_mut(c2) {
                if e.body().dead {
                    break;
                }
                e.body_mut().set_position(3.5, 64.0, 4.5);
            }
            w2.tick_mob(c2);
            if w2.entities.get(c2).unwrap().body().dead {
                break;
            }
        }
        assert!(w2.entities.get(c2).unwrap().body().dead);
    }

    #[test]
    fn test_creeper_blast_scatters_container() {
        let mut w = world_with_floor();
        // Stocked chest with the creeper inside its cell (d=0 destroys
        // with chance 1, no RNG involved).
        w.set_block_id(4, 64, 4, 54);
        let mut ch = crate::tile_entity::chest::chest_create();
        ch.slots[0] = stk(3, 7, 0);
        w.tiles.insert((4, 64, 4), TileData::Chest(ch));
        let creeper = add_mob(&mut w, MobKind::Creeper, 4.5, 64.0, 4.5);
        w.creeper_explode(creeper);
        // Chest block and tile row are gone, dirt scattered as items
        // (mirrors the C++ removal hook on the blast path).
        assert_eq!(w.get_block_id(4, 64, 4), 0);
        assert!(w.tiles.get(&(4, 64, 4)).is_none());
        assert!(w.entities.alive_ids().iter().any(|oid| matches!(
            w.entities.get(*oid),
            Some(crate::entity::table::Entity::Item(e)) if e.item_id == 3
        )));
    }

    #[test]
    fn test_arrow_sticks_in_wall_and_pops_out() {
        use crate::entity::table::{ArrowEnt, Body};
        let mut w = world_with_floor();
        for y in 64..67 {
            w.set_block_id(10, y, 8, 1);
        }
        let id = w.entities.alloc_id();
        let mut b = Body::new(id, 0.5, 0.5, 0.0);
        b.set_position(8.5, 65.0, 8.5);
        b.motion = [1.0, 0.0, 0.0];
        w.entities.insert(crate::entity::table::Entity::Arrow(ArrowEnt {
            body: b,
            in_ground: false,
            shake: 0,
            ticks_in_ground: 0,
            ticks_in_air: 0,
            shooter_id: -1,
            tile: [-1, -1, -1],
            in_tile: 0,
        }));
        w.tick_arrow(id);
        w.tick_arrow(id);
        let (stuck, tile, shake) = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Arrow(a) => (a.in_ground, a.tile, a.shake),
            _ => unreachable!(),
        };
        assert!(stuck);
        // Boundary graze: the shared face clips y=64 first and strict `<`
        // keeps it, exactly like the C++ sweep order.
        assert_eq!((tile, shake), ([10, 64, 8], 7));
        // Removing the block pops the arrow back out.
        w.set_block_id(10, 64, 8, 0);
        w.tick_arrow(id);
        assert!(!matches!(
            w.entities.get(id).unwrap(),
            crate::entity::table::Entity::Arrow(a) if a.in_ground
        ));
    }

    #[test]
    fn test_sheep_shear_on_living_hit() {
        let mut w = world_with_floor();
        let sheep = add_animal(&mut w, AnimalKind::Sheep, 3.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 5.5, 64.0, 4.5);
        w.attack_living(sheep, 3, Some(zombie));
        assert!(matches!(
            w.entities.get(sheep).unwrap(),
            crate::entity::table::Entity::Animal(a) if a.sheared
        ));
        let wool = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|oid| {
                matches!(
                    w.entities.get(*oid).unwrap(),
                    crate::entity::table::Entity::Item(e) if e.item_id == 35
                )
            })
            .count();
        assert!((1..=3).contains(&wool), "shear drops 1..3 wool, got {wool}");
        // No living attacker, no shear.
        let sheep2 = add_animal(&mut w, AnimalKind::Sheep, 8.5, 64.0, 8.5);
        w.attack_living(sheep2, 3, None);
        assert!(matches!(
            w.entities.get(sheep2).unwrap(),
            crate::entity::table::Entity::Animal(a) if !a.sheared
        ));
    }

    #[test]
    fn test_player_damage_and_empty_death() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 4.5);
        w.attack_living(player, 5, Some(zombie));
        assert_eq!(player_health(&w, player), 15);
        let before = w.entities.len();
        w.attack_living(player, 100, Some(zombie));
        assert!(w.entities.get(player).unwrap().body().dead);
        // Empty inventory scatters nothing.
        assert_eq!(w.entities.len(), before);
    }

    fn stk(item_id: i32, count: i32, damage: i32) -> crate::inventory::ItemStack {
        crate::inventory::ItemStack::new(item_id, count, damage)
    }

    fn set_slot(w: &mut World, id: EntityId, bank: u8, slot: usize, s: crate::inventory::ItemStack) {
        if let Some(crate::entity::table::Entity::Player(p)) = w.entities.get_mut(id) {
            let bank = match bank {
                0 => &mut p.inventory.main[..],
                1 => &mut p.inventory.armor[..],
                _ => &mut p.inventory.crafting[..],
            };
            bank[slot] = Some(s);
        }
    }

    fn player_items(w: &World) -> Vec<(i32, i32, i32)> {
        // (item_id, count, pickup_delay) of every live loose item.
        let mut out: Vec<(i32, i32, i32)> = w
            .entities
            .alive_ids()
            .into_iter()
            .filter_map(|oid| match w.entities.get(oid).unwrap() {
                crate::entity::table::Entity::Item(e) => Some((e.item_id, e.count, e.pickup_delay)),
                _ => None,
            })
            .collect();
        out.sort_unstable();
        out
    }

    #[test]
    fn test_player_death_scatters_inventory() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 4.5);
        set_slot(&mut w, player, 0, 0, stk(3, 10, 0)); // dirt x10
        set_slot(&mut w, player, 1, 0, stk(306, 1, 0)); // iron helm
        set_slot(&mut w, player, 2, 2, stk(280, 5, 0)); // sticks x5
        w.attack_living(player, 100, Some(zombie));
        assert!(w.entities.get(player).unwrap().body().dead);
        assert_eq!(player_items(&w), vec![(3, 10, 40), (280, 5, 40), (306, 1, 40)]);
        // Spawn height is feet + 0.5 with an upward toss.
        for oid in w.entities.alive_ids() {
            if let crate::entity::table::Entity::Item(e) = w.entities.get(oid).unwrap() {
                assert_eq!(e.body.pos[1], 64.5);
                assert!(e.body.motion[1] > 0.0);
            }
        }
        // All banks cleared.
        assert!(matches!(
            w.entities.get(player).unwrap(),
            crate::entity::table::Entity::Player(p)
                if p.inventory.main.iter().all(|s| s.is_none())
                    && p.inventory.armor.iter().all(|s| s.is_none())
                    && p.inventory.crafting.iter().all(|s| s.is_none())
        ));
    }

    #[test]
    fn test_player_respawn_immunity() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 4.5, 64.0, 4.5);
        if let Some(crate::entity::table::Entity::Player(p)) = w.entities.get_mut(player) {
            p.respawn_ticks = 10;
        }
        w.attack_living(player, 5, Some(zombie));
        assert_eq!(player_health(&w, player), 20);
    }

    #[test]
    fn test_player_armor_absorbs_and_wears() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 4.5);
        // Full iron: 3 + 8 + 6 + 3 = 20 points at full durability.
        set_slot(&mut w, player, 1, 0, stk(306, 1, 0));
        set_slot(&mut w, player, 1, 1, stk(307, 1, 0));
        set_slot(&mut w, player, 1, 2, stk(308, 1, 0));
        set_slot(&mut w, player, 1, 3, stk(309, 1, 0));
        w.attack_living(player, 10, Some(zombie));
        // scaled = 10 * (25 - 20) = 50 -> 2 damage through, carry 0.
        assert_eq!(player_health(&w, player), 18);
        assert!(matches!(
            w.entities.get(player).unwrap(),
            crate::entity::table::Entity::Player(p)
                if p.armor_carry == 0
                    && p.inventory.armor.iter().all(|s| s.map(|x| x.damage) == Some(10))
        ));
    }

    #[test]
    fn test_player_peaceful_ignores_mob_hit() {
        let mut w = world_with_floor();
        w.difficulty = 0;
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 4.5, 64.0, 4.5);
        w.attack_living(player, 5, Some(zombie));
        assert_eq!(player_health(&w, player), 20);
    }

    #[test]
    fn test_player_pickup_merges_and_overflows() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        assert_eq!(w.player_add_item(player, stk(3, 10, 0)), 0);
        assert_eq!(w.player_add_item(player, stk(3, 60, 0)), 0);
        let main: Vec<Option<(i32, i32)>> = match w.entities.get(player).unwrap() {
            crate::entity::table::Entity::Player(p) => {
                p.inventory.main.iter().take(3).map(|s| s.map(|x| (x.item_id, x.count))).collect()
            }
            _ => unreachable!(),
        };
        assert_eq!(main, vec![Some((3, 64)), Some((3, 6)), None]);
        // Bad ids refuse.
        assert_eq!(w.player_add_item(player, stk(0, 5, 0)), 5);
        assert_eq!(w.player_add_item(player, stk(32000, 5, 0)), 5);
        assert_eq!(w.player_add_item(player, stk(3, 0, 0)), 0);
    }

    #[test]
    fn test_player_held_slot() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        assert_eq!(w.player_held(player), None);
        set_slot(&mut w, player, 0, 2, stk(5, 3, 0));
        if let Some(crate::entity::table::Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.current = 2;
        }
        assert_eq!(w.player_held(player).map(|s| (s.item_id, s.count)), Some((5, 3)));
        if let Some(crate::entity::table::Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.current = 99;
        }
        assert_eq!(w.player_held(player), None);
    }
