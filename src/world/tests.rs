
    use super::*;
    use crate::block::BlockPos;
    use crate::entity::table::{Body, Entity, MobEnt, PlayerEnt};
    use crate::session::PlaySession;

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

    /// Tick-start id snapshot (what `tick_world` hands to `tick_mob`/`tick_animal`).
    fn snap(w: &World) -> Vec<EntityId> {
        let mut v = w.entities.alive_ids();
        v.sort_unstable();
        v
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

        // Oversized mask (10,000 blocks) should not hang and should return safely
        let huge = AxisAlignedBB::get_bounding_box(-5000.0, -100.0, -5000.0, 5000.0, 200.0, 5000.0);
        let huge_boxes = w.colliding_boxes(&huge);
        assert!(!huge_boxes.is_empty());

        // Non-finite mask returns empty immediately
        let nan_box = AxisAlignedBB::get_bounding_box(f64::NAN, 0.0, 0.0, 1.0, 1.0, 1.0);
        assert!(w.colliding_boxes(&nan_box).is_empty());
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

    #[test]
    fn test_water_contact_deals_no_damage_and_no_fire() {
        // Regression: `Material` compared flags, so water aliased lava and
        // swimming dealt lava contact damage (without igniting, because
        // the tick then extinguished the fire as "liquid").
        let mut w = world_with_floor();
        for y in 64..68 {
            w.set_block_id(3, y, 4, 8);
        }
        assert!(w.is_water(3, 65, 4));
        assert!(!w.is_lava(3, 65, 4));
        let id = add_zombie(&mut w, 3.5, 65.0, 4.5);
        let hp_before = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        };
        w.move_body(id, 0.0, 0.0, 0.0);
        w.tick_living(id);
        let (hp_after, fire) = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.living.health, m.living.body.fire),
            _ => unreachable!(),
        };
        assert_eq!(hp_before, hp_after);
        assert_eq!(fire, 0);
    }

    #[test]
    fn test_lava_contact_damages_ignites_and_burns() {
        // Vanilla `World.func_523_c` in `moveEntity`: lava deals 1 and ignites
        // for 300 ticks; then `Entity.handleLavaMovement` in `tick_living`
        // deals 4 (upgrading 1 -> 4 during hurt_resist) and sets fire = 600.
        let mut w = world_with_floor();
        for y in 64..68 {
            w.set_block_id(3, y, 4, 10);
        }
        assert!(w.is_lava(3, 65, 4));
        assert!(!w.is_water(3, 65, 4));
        let id = add_zombie(&mut w, 3.5, 65.0, 4.5);
        let hp_before = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        };
        w.move_body(id, 0.0, 0.0, 0.0);
        let (hp_after_contact, fire_after_contact) = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.living.health, m.living.body.fire),
            _ => unreachable!(),
        };
        assert_eq!(hp_before - hp_after_contact, 1);
        assert_eq!(fire_after_contact, 300);
        w.tick_living(id);
        let (hp_after, fire) = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.living.health, m.living.body.fire),
            _ => unreachable!(),
        };
        assert_eq!(hp_before - hp_after, 4);
        assert_eq!(fire, 600, "lava must set fire to 600 and not extinguish it");
    }

    #[test]
    fn test_fire_block_contact_damages_and_ignites() {
        // Vanilla `func_523_c` covers fire (51) too, not just lava.
        let mut w = world_with_floor();
        w.set_block_id(3, 64, 4, 51);
        let id = add_zombie(&mut w, 3.5, 64.2, 4.5);
        w.move_body(id, 0.0, 0.0, 0.0);
        let (hp_after_contact, fire) = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.living.health, m.living.body.fire),
            _ => unreachable!(),
        };
        assert!(hp_after_contact < 20);
        assert_eq!(fire, 300);
    }

    #[test]
    fn test_water_extinguishes_burning() {
        // Vanilla `Entity.onUpdate`: water kills the fire (with a fizz).
        let mut w = world_with_floor();
        for y in 64..68 {
            w.set_block_id(3, y, 4, 8);
        }
        let id = add_zombie(&mut w, 3.5, 65.0, 4.5);
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(id) {
            m.living.body.fire = 100;
        }
        w.tick_living(id);
        let fire = match w.entities.get(id).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.body.fire,
            _ => unreachable!(),
        };
        assert_eq!(fire, 0);
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
        { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
        assert_eq!(
            match w.entities.get(zombie).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.target,
                _ => unreachable!(),
            },
            Some(player)
        );
        for _ in 0..40 {
            { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
        }
        let x = w.entities.get(zombie).unwrap().body().pos[0];
        assert!(x > 3.5, "zombie should walk east toward the player, x={x}");
    }

    #[test]
    fn test_mob_wanders_without_target() {
        let mut w = world_with_floor();
        let zombie = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 8.5);
        for _ in 0..300 {
            { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
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
        { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
        let (burn, fire) = match w.entities.get(zombie).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.burn_ticks, m.living.body.fire),
            _ => unreachable!(),
        };
        assert_eq!((burn, fire), (20, 20));
        assert_eq!(mob_health(&w, zombie), 20); // 21 % 20 != 0: no hit yet
        { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
        assert_eq!(mob_health(&w, zombie), 19); // 20 % 20 == 0: one burn damage
        // Small fires go out once the burn ends.
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(zombie) {
            m.burn_ticks = 0;
            m.living.body.fire = 10;
        }
        { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
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
            { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
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
        { let __ids = snap(&w); w.tick_mob(spider, &__ids); }
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
            { let __ids = snap(&w); w.tick_mob(spider, &__ids); }
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
            { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
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
            { let __ids = snap(&w); w.tick_animal(chicken, &__ids); }
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
        { let __ids = snap(&w); w.tick_mob(z1, &__ids); }
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

    #[test]
    fn test_ensure_area_with_progress() {
        let mut w = World::new(4242);
        let mut calls = Vec::new();
        w.ensure_area_with_progress(0, 0, 1, |done, total| {
            calls.push((done, total));
        });
        assert_eq!(calls.len(), 9);
        assert_eq!(calls.last(), Some(&(9, 9)));
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
        { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
        assert_eq!(player_health(&w, player), 15);
        assert_eq!(
            match w.entities.get(zombie).unwrap() {
                crate::entity::table::Entity::Mob(m) => m.attack_cooldown,
                _ => unreachable!(),
            },
            20
        );
        // Cooldown gates the next punch.
        { let __ids = snap(&w); w.tick_mob(zombie, &__ids); }
        assert_eq!(player_health(&w, player), 15);
    }

    #[test]
    fn test_skeleton_looses_arrow() {
        let mut w = world_with_floor();
        let _player = add_player(&mut w, "steve", 10.5, 64.0, 4.5);
        let skel = add_mob(&mut w, MobKind::Skeleton, 3.5, 64.0, 4.5);
        { let __ids = snap(&w); w.tick_mob(skel, &__ids); }
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
            { let __ids = snap(&w2); w2.tick_mob(c2, &__ids); }
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
        assert!(!w.tiles.contains_key(&(4, 64, 4)));
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

    #[test]
    fn test_skylight_subtracted_and_night_surface_spawning() {
        let mut w = world_with_floor();
        for x in 0..16 {
            for z in 0..16 {
                for y in 64..128 {
                    set_sky(&mut w, x, y, z, 15);
                }
            }
        }
        let candidate = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 8.5);
        // Noon (time = 6000): skylight_subtracted == 0, block_light_value == 15 -> spawner_mob_ok false
        w.time = 6000;
        w.update_skylight_subtracted();
        assert_eq!(w.skylight_subtracted, 0);
        assert_eq!(w.block_light_value(8, 64, 8), 15);
        assert!(!w.spawner_mob_ok(candidate));

        // Midnight (time = 18000): skylight_subtracted == 11, block_light_value == 4 -> spawner_mob_ok true when roll >= 4
        w.time = 18000;
        w.update_skylight_subtracted();
        assert_eq!(w.skylight_subtracted, 11);
        assert_eq!(w.block_light_value(8, 64, 8), 4);
        let mut spawned_ok = false;
        for _ in 0..100 {
            if w.spawner_mob_ok(candidate) {
                spawned_ok = true;
                break;
            }
        }
        assert!(spawned_ok, "surface hostile mob must be able to spawn at midnight under skylight 15");

        // Collision check: placing another living entity at (8.5, 64.0, 8.5) blocks spawning at (8, 64, 8)
        let _blocker = add_mob(&mut w, MobKind::Zombie, 8.5, 64.0, 8.5);
        for _ in 0..20 {
            assert!(!w.spawner_mob_ok(candidate));
        }
    }

    #[test]
    fn test_pathfinder_uses_target_feet_min_y_and_explosion_resistance() {
        let mut w = world_with_floor();
        let mob = add_mob(&mut w, MobKind::Zombie, 2.5, 64.0, 2.5);
        // Player on ground has pos[1] = 65.62 (eye level) and bounding_box.min_y = 64.0.
        let player = add_player(&mut w, "steve", 6.5, 64.0, 2.5);
        let pts = w.path_target_points(mob, player, 16.0);
        assert!(!pts.is_empty());
        assert_eq!(pts.last().unwrap()[1], 64, "pathfinder must target feet Y=64, not head Y=65");

        // Explosion resistance: stone (1) and dirt (3) are destroyed, obsidian (49) and water (9) survive.
        w.set_block_id(8, 64, 8, 1);
        w.set_block_id(9, 64, 8, 3);
        w.set_block_id(8, 64, 9, 49);
        w.set_block_id(9, 64, 9, 9);
        w.blast(8.5, 64.5, 8.5, 4.0, None);
        assert_eq!(w.get_block_id(8, 64, 8), 0);
        assert_eq!(w.get_block_id(9, 64, 8), 0);
        assert_eq!(w.get_block_id(8, 64, 9), 49, "obsidian must resist explosion");
        assert_eq!(w.get_block_id(9, 64, 9), 9, "water must resist explosion");

        // Raycast explosion attenuation: a 2-block-thick stone wall shields dirt behind it from a creeper blast (radius 3.0).
        let mut w2 = world_with_floor();
        for dy in 64..=66 {
            for dz in 7..=9 {
                w2.set_block_id(9, dy, dz, 1); // first stone layer
                w2.set_block_id(10, dy, dz, 1); // second stone layer
            }
        }
        w2.set_block_id(11, 65, 8, 3); // dirt behind the stone wall (distance 2.5 < radius 3.0)
        w2.blast(8.5, 65.5, 8.5, 3.0, None);
        assert_eq!(w2.get_block_id(9, 65, 8), 0, "front stone layer should be destroyed");
        assert_eq!(w2.get_block_id(11, 65, 8), 3, "dirt behind stone wall must be shielded by ray attenuation");
    }

    #[test]
    fn test_skeleton_arrow_height_and_mob_single_burn_damage() {
        let mut w = world_with_floor();
        let skel = add_mob(&mut w, MobKind::Skeleton, 2.5, 64.0, 2.5);
        let player = add_player(&mut w, "steve", 6.5, 64.0, 2.5);
        w.spawn_skeleton_arrow(skel, player);
        let arrow = w
            .entities
            .alive_ids()
            .into_iter()
            .find(|&id| matches!(w.entities.get(id), Some(crate::entity::table::Entity::Arrow(_))))
            .unwrap();
        let ay = w.entities.get(arrow).unwrap().body().pos[1];
        // Skeleton eye is ~64.0 + 1.62 = 65.62, arrow spawns at eye - 0.1 (~65.52), NOT ~66.92
        assert!(ay < 66.0 && ay > 65.0, "skeleton arrow y={ay} must be near eye height");

        // Burning zombie with both burn_ticks and body.fire only takes 1 damage at the 20-tick mark
        let zom = add_mob(&mut w, MobKind::Zombie, 4.5, 64.0, 4.5);
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(zom) {
            m.burn_ticks = 20;
            m.living.body.fire = 20;
        }
        w.tick_mob(zom, &[zom]);
        let hp = match w.entities.get(zom).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        };
        assert_eq!(hp, 19, "burning mob must take 1 damage (not 2) on the 20-tick fire boundary");
    }

    #[test]
    fn test_mob_spawner_ticks_and_spawns() {
        let mut w = world_with_floor();
        w.spawn = [100, 64, 100]; // keep spawn protection away from (8, 64, 8)
        let _p = add_player(&mut w, "steve", 2.5, 64.0, 2.5);
        w.set_block_id(8, 65, 8, 52);
        let mut sp = crate::world::tiles::MobSpawnerState::new("Zombie");
        sp.delay = 0;
        w.tiles.insert((8, 65, 8), crate::world::TileData::MobSpawner(sp));
        w.tick_mob_spawners();
        let zombies = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|&id| matches!(w.entities.get(id), Some(crate::entity::table::Entity::Mob(m)) if m.kind == MobKind::Zombie))
            .count();
        assert!(zombies >= 1, "MobSpawner must spawn Zombie when delay expires near player");
    }

    #[test]
    fn test_fluids_doors_redstone_slabs_soul_sand_portal_and_cross_chunk_light() {
        let mut w = world_with_floor();
        for cx in -1..=1 {
            for cz in -1..=1 {
                if cx != 0 || cz != 0 {
                    add_floor_chunk(&mut w, cx, cz);
                }
            }
        }

        // 1. Infinite water source: two water sources at (4,64,5) and (6,64,5) convert flowing (5,64,5) to source (id=9, meta=0)
        w.set_block_id(4, 64, 5, 9);
        w.set_block_meta(4, 64, 5, 0);
        w.set_block_id(6, 64, 5, 9);
        w.set_block_meta(6, 64, 5, 0);
        w.set_block_id(5, 64, 5, 8);
        w.set_block_meta(5, 64, 5, 1);
        crate::block::ticks::block_fluid_tick(&mut w, 8, false, crate::block::pos::BlockPos::new(5, 64, 5));
        assert_eq!(w.get_block_id(5, 64, 5), 9);
        assert_eq!(w.get_block_meta(5, 64, 5), 0);

        // 2. Un-fed flowing water at (10,64,10) dries up to air
        w.set_block_id(10, 64, 10, 8);
        w.set_block_meta(10, 64, 10, 3);
        crate::block::ticks::block_fluid_tick(&mut w, 8, false, crate::block::pos::BlockPos::new(10, 64, 10));
        assert_eq!(w.get_block_id(10, 64, 10), 0);

        // 3. Iron door (71) + Lever (69): toggling lever opens and closes adjacent iron door
        w.set_block_id(2, 64, 2, 71);
        w.set_block_meta(2, 64, 2, 0);
        w.set_block_id(2, 65, 2, 71);
        w.set_block_meta(2, 65, 2, 8);
        w.set_block_id(3, 64, 2, 69);
        w.set_block_meta(3, 64, 2, 5);
        assert!(w.toggle_lever(3, 64, 2));
        assert_eq!(w.get_block_meta(2, 64, 2) & 4, 4, "powered lever must open adjacent iron door");
        assert!(w.toggle_lever(3, 64, 2));
        assert_eq!(w.get_block_meta(2, 64, 2) & 4, 0, "unpowered lever must close adjacent iron door");

        // 3b. Redstone wire (55) signal propagation + Redstone torch (76/75) inversion + Pressure plate (70)
        w.apply_set_notify(3, 64, 3, 55);
        w.apply_set_notify(4, 64, 3, 55);
        w.apply_set_notify(5, 64, 3, 1); // support block for redstone torch
        w.apply_set_meta_notify(5, 65, 3, 76, 5); // lit redstone torch on top of (5,64,3)
        assert_eq!(w.get_block_meta(3, 64, 3), 0);
        // Toggle lever at (3,64,2) ON -> wire (3,64,3) becomes 15, wire (4,64,3) becomes 14, powering (5,64,3)
        assert!(w.toggle_lever(3, 64, 2));
        assert_eq!(w.get_block_meta(3, 64, 3), 15);
        assert_eq!(w.get_block_meta(4, 64, 3), 14);
        for _ in 0..3 {
            w.tick_world();
        }
        assert_eq!(w.get_block_id(5, 65, 3), 75, "powered support block must invert redstone torch 76 -> 75");
        // Toggle lever OFF -> wire drops to 0 and redstone torch relights 75 -> 76
        assert!(w.toggle_lever(3, 64, 2));
        assert_eq!(w.get_block_meta(3, 64, 3), 0);
        for _ in 0..3 {
            w.tick_world();
        }
        assert_eq!(w.get_block_id(5, 65, 3), 76, "unpowered support block must relight redstone torch 75 -> 76");

        // Stone pressure plate (70) at (8, 64, 2): stepping on it sets meta=1, moving off resets meta=0 after 20 ticks
        w.apply_set_notify(8, 64, 2, 70);
        let p_id = add_player(&mut w, "plate_tester", 8.5, 64.0, 2.5);
        w.move_body(p_id, 0.0, -0.01, 0.0);
        assert_eq!(w.get_block_meta(8, 64, 2), 1, "stepping on stone pressure plate must depress it");
        if let Some(e) = w.entities.get_mut(p_id) {
            e.body_mut().set_position(0.5, 64.0, 0.5);
        }
        for _ in 0..25 {
            w.tick_world();
        }
        assert_eq!(w.get_block_meta(8, 64, 2), 0, "pressure plate must release after entity leaves");

        // 4. Collision boxes: single slab (44 -> max_y 0.5), fence (85 -> max_y 1.5), soul sand (88 -> max_y 0.875)
        w.set_block_id(5, 64, 8, 44);
        w.set_block_id(6, 64, 8, 85);
        w.set_block_id(7, 64, 8, 88);
        assert_eq!(w.block_collision_box(5, 64, 8, 44).max_y, 64.5);
        assert_eq!(w.block_collision_box(6, 64, 8, 85).max_y, 65.5);
        assert_eq!(w.block_collision_box(7, 64, 8, 88).max_y, 64.875);

        // 5. Nether portal ignition on 4x5 obsidian frame
        for dx in 0..4 {
            w.set_block_id(2 + dx, 70, 12, 49);
            w.set_block_id(2 + dx, 74, 12, 49);
        }
        for dy in 0..5 {
            w.set_block_id(2, 70 + dy, 12, 49);
            w.set_block_id(5, 70 + dy, 12, 49);
        }
        w.apply_set_notify(3, 71, 12, 51);
        assert_eq!(w.get_block_id(3, 71, 12), 90);
        assert_eq!(w.get_block_id(4, 73, 12), 90);

        // 6. Cross-chunk blocklight propagation AND removal (including diagonal chunk (1,1)):
        w.set_block_id(15, 64, 15, 50);
        w.refresh_light();
        assert_eq!(w.saved_light_value(1, 15, 64, 15), 14);
        assert_eq!(w.saved_light_value(1, 16, 64, 15), 13, "blocklight must propagate across +X chunk border");
        assert_eq!(w.saved_light_value(1, 16, 64, 16), 12, "blocklight must propagate across diagonal (1,1) chunk border");
        // Removing the torch must clear blocklight in both (0,0), (1,0), and (1,1):
        w.set_block_id(15, 64, 15, 0);
        w.refresh_light();
        assert_eq!(w.saved_light_value(1, 15, 64, 15), 0, "broken torch cell must return to 0 blocklight");
        assert_eq!(w.saved_light_value(1, 16, 64, 15), 0, "neighbor chunk (1,0) must clear stale blocklight");
        assert_eq!(w.saved_light_value(1, 16, 64, 16), 0, "diagonal chunk (1,1) must clear stale blocklight");
    }

    #[test]
    fn test_mob_parity_zombie_reach_spider_stop_creeper_explosion_and_armor_iframes() {
        // 1. Zombie reach on Alpha 1.2.6 server (EntityMobs.java:50 dist < 2.5):
        // On flat ground, at 2.6 blocks horizontally (dx = 2.6, dist = 2.6 >= 2.5), zombie must NOT hit.
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 6.1, 64.0, 4.5);
        let zombie = add_mob(&mut w, MobKind::Zombie, 3.5, 64.0, 4.5);
        { let ids = snap(&w); w.tick_mob(zombie, &ids); }
        assert_eq!(player_health(&w, player), 20, "zombie at 2.6 blocks horizontally must not hit player yet");

        // Move zombie to 2.1 blocks horizontally (dist = 2.1 < 2.5): zombie hits!
        if let Some(e) = w.entities.get_mut(zombie) {
            e.body_mut().set_position(4.0, 64.0, 4.5);
        }
        { let ids = snap(&w); w.tick_mob(zombie, &ids); }
        assert_eq!(player_health(&w, player), 15, "zombie at 2.1 blocks horizontally must hit player");

        // Height elevation: player standing 1 block higher (y=65.0, zombie at y=64.0, dx=1.5):
        // dy = 1.0, dist = sqrt(1.5^2 + 1.0^2) = 1.80 < 2.5. Vertical overlap is true. Zombie can hit!
        let mut w_elev = world_with_floor();
        for x in 0..10 {
            for z in 0..10 {
                w_elev.set_block_id(x, 64, z, 1);
            }
        }
        let p_elev = add_player(&mut w_elev, "steve", 5.0, 65.0, 4.5);
        let z_elev = add_mob(&mut w_elev, MobKind::Zombie, 3.5, 64.0, 4.5);
        { let ids = snap(&w_elev); w_elev.tick_mob(z_elev, &ids); }
        assert_eq!(player_health(&w_elev, p_elev), 15, "zombie must hit player elevated by 1 block within reach");

        // 2. Spider stops path-driving within width * 2.0 = 2.8 blocks (instead of charging at 0.8 moveSpeed)
        // and mounted_y_offset matches EntitySpider (height * 0.75 - 0.5).
        let mut w2 = world_with_floor();
        let p2 = add_player(&mut w2, "steve", 5.5, 64.0, 4.5);
        let spider = add_mob(&mut w2, MobKind::Spider, 3.5, 64.0, 4.5);
        let off = w2.entities.get(spider).unwrap().mounted_y_offset();
        assert!((off - (0.9f32 as f64 * 0.75 - 0.5)).abs() < 1e-6);
        if let Some(crate::entity::table::Entity::Mob(m)) = w2.entities.get_mut(spider) {
            m.target = Some(p2);
            m.target_timer = 5;
            // Path point at [5, 64, 4] has center (5.5, 64.0, 4.5), which is 2.0 blocks from (3.5, 64.0, 4.5) (< 2.8).
            m.path = vec![[5, 64, 4]];
            m.path_index = 0;
        }
        let (_strafe, forward) = w2.update_mob_ai(spider, MobKind::Spider, false);
        assert_eq!(forward, 0.0, "spider within 2.8 blocks of final path point must stop path-driving");

        // 3. Creeper swells, stops advancing when path is exhausted, defuses with status 5 when LOS is blocked,
        // and disappears IMMEDIATELY (0-tick purge) with Packet60 explosion_events when exploding!
        let mut w3 = world_with_floor();
        let _p3 = add_player(&mut w3, "steve", 5.0, 64.0, 4.5);
        let creeper = add_mob(&mut w3, MobKind::Creeper, 3.5, 64.0, 4.5);
        { let ids = snap(&w3); w3.tick_mob(creeper, &ids); }
        assert!(w3.status_events.contains(&(creeper, 4)), "creeper ignite must emit status 4");
        w3.status_events.clear();
        // Place a stone wall between creeper (x=3.5) and player (x=5.0) at x=4 to block LOS:
        for y in 64..67 {
            w3.set_block_id(4, y, 4, 1);
        }
        { let ids = snap(&w3); w3.tick_mob(creeper, &ids); }
        assert!(w3.status_events.contains(&(creeper, 5)), "breaking LOS must defuse creeper and emit status 5");
        let (swell_time, swell_dir) = match w3.entities.get(creeper).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.swell_time, m.swell_dir),
            _ => unreachable!(),
        };
        assert_eq!((swell_time, swell_dir), (0, -1));

        // Clear wall and let creeper explode:
        for y in 64..67 {
            w3.set_block_id(4, y, 4, 0);
        }
        if let Some(crate::entity::table::Entity::Mob(m)) = w3.entities.get_mut(creeper) {
            m.swell_time = 29;
            m.swell_dir = 1;
            m.living.body.set_position(3.5, 64.0, 4.5);
        }
        w3.explosion_events.clear();
        w3.tick_world();
        assert!(
            w3.entities.get(creeper).is_none(),
            "exploded creeper must be purged immediately on the explosion tick (no 20-tick corpse delay)"
        );
        assert_eq!(w3.explosion_events.len(), 1, "creeper explosion must queue Packet60 explosion event");
        assert!(!w3.explosion_events[0].4.is_empty(), "Packet60 explosion event must include blast cells");

        // 4. Armor durability is NOT damaged by rejected hits during invulnerability frames (hurt_resist > 10),
        // and attacking a mob sets its retaliation target.
        let mut w4 = world_with_floor();
        let p4 = add_player(&mut w4, "steve", 3.5, 64.0, 4.5);
        let z4 = add_mob(&mut w4, MobKind::Zombie, 5.5, 64.0, 4.5);
        set_slot(&mut w4, p4, 1, 0, stk(306, 1, 0)); // iron helmet
        w4.attack_living(p4, 5, Some(z4));
        let dmg_after_first = match w4.entities.get(p4).unwrap() {
            crate::entity::table::Entity::Player(p) => p.inventory.armor[0].unwrap().damage,
            _ => unreachable!(),
        };
        assert_eq!(dmg_after_first, 5);
        // Second identical hit during hurt_resist window is ignored and must NOT wear armor:
        w4.attack_living(p4, 5, Some(z4));
        let dmg_after_second = match w4.entities.get(p4).unwrap() {
            crate::entity::table::Entity::Player(p) => p.inventory.armor[0].unwrap().damage,
            _ => unreachable!(),
        };
        assert_eq!(dmg_after_second, 5, "blocked hit during invulnerability window must not damage armor");

        // Attacking z4 sets z4.target = Some(p4):
        w4.attack_living(z4, 2, Some(p4));
        let z4_target = match w4.entities.get(z4).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.target,
            _ => unreachable!(),
        };
        assert_eq!(z4_target, Some(p4), "damaged mob must target its attacker");
    }

    #[test]
    fn test_spider_retaliates_in_daylight_when_attacked() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 5.0, 64.0, 4.5);
        let spider = add_mob(&mut w, MobKind::Spider, 3.5, 64.0, 4.5);
        // Daylight: sky light = 15 everywhere
        set_sky(&mut w, 3, 64, 4, 15);
        set_sky(&mut w, 5, 64, 4, 15);

        // Before being attacked, spider ignores the player in daylight:
        { let ids = snap(&w); w.tick_mob(spider, &ids); }
        let target_before = match w.entities.get(spider).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.target,
            _ => unreachable!(),
        };
        assert_eq!(target_before, None, "unprovoked spider in daylight must not target player");

        // Give spider an active wander path away from player:
        if let Some(crate::entity::table::Entity::Mob(m)) = w.entities.get_mut(spider) {
            m.path = vec![[0, 64, 4]];
            m.path_index = 0;
        }

        // Player attacks spider in broad daylight:
        w.attack_living(spider, 2, Some(player));
        let (target_after_hit, path_len_after_hit) = match w.entities.get(spider).unwrap() {
            crate::entity::table::Entity::Mob(m) => (m.target, m.path.len()),
            _ => unreachable!(),
        };
        assert_eq!(target_after_hit, Some(player), "spider must target attacker upon being damaged");
        assert_eq!(path_len_after_hit, 0, "spider must clear old wander path when provoked");

        // Tick spider in daylight: spider must RETAIN the player target, repath, and bite!
        // At distance 1.5 blocks, spider can bite (dist < 2.5)
        for _ in 0..5 {
            let ids = snap(&w);
            w.tick_mob(spider, &ids);
        }
        let target_after_ticks = match w.entities.get(spider).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.target,
            _ => unreachable!(),
        };
        assert_eq!(target_after_ticks, Some(player), "spider must not discard retaliation target in daylight");
        assert_eq!(player_health(&w, player), 18, "retaliating spider must attack player for 2 damage");

        // If player teleports far away (> 24 blocks), spider drops target:
        if let Some(e) = w.entities.get_mut(player) {
            e.body_mut().set_position(50.0, 64.0, 4.5);
        }
        { let ids = snap(&w); w.tick_mob(spider, &ids); }
        let target_after_far = match w.entities.get(spider).unwrap() {
            crate::entity::table::Entity::Mob(m) => m.target,
            _ => unreachable!(),
        };
        assert_eq!(target_after_far, None, "spider must lose target when player is beyond 24 blocks");
    }

    #[test]
    fn test_ensure_chunk_recalls_unloaded_and_unload_chunks_protects_actual_spawn() {
        let mut w = World::new(12345);
        w.spawn = [160, 64, 160]; // spawn at chunk (10, 10)
        // Populate chunk (0, 0), place a diamond block at (5, 70, 5), and unload it via unload_chunks:
        add_floor_chunk(&mut w, 0, 0);
        w.set_block_id(5, 70, 5, 57);
        w.time = 100;
        w.unload_chunks();
        assert!(!w.has_chunk(0, 0));

        // Calling ensure_chunk(0, 0) must recall the spilled chunk from unloaded instead of regenerating over it!
        w.ensure_chunk(0, 0);
        assert!(w.has_chunk(0, 0));
        assert_eq!(w.get_block_id(5, 70, 5), 57, "ensure_chunk must recall unloaded chunk rather than regenerating");

        // Also add chunk at spawn (10, 10) and far chunk (30, 30) with 0 players online:
        add_floor_chunk(&mut w, 10, 10);
        add_floor_chunk(&mut w, 30, 30);
        w.time = 200;
        w.unload_chunks();
        assert!(w.has_chunk(10, 10), "spawn chunk (10, 10) must be protected from unload");
        assert!(!w.has_chunk(0, 0), "chunk (0, 0) away from spawn (10, 10) must be unloaded when 0 players online");
        assert!(!w.has_chunk(30, 30), "far chunk (30, 30) must be unloaded when 0 players online");
    }

    #[test]
    fn test_entity_dirty_tracking_on_spawn_move_and_despawn() {
        let mut w = world_with_floor();
        add_floor_chunk(&mut w, 1, 0);
        w.chunks.get_mut(&(0, 0)).unwrap().is_modified = false;
        w.chunks.get_mut(&(1, 0)).unwrap().is_modified = false;

        // 1. Spawning an item marks chunk (0, 0) modified:
        let item = w.spawn_item_entity(264, 1, 0, 15.5, 64.25, 8.5);
        assert!(w.chunks.get(&(0, 0)).unwrap().is_modified, "spawning item must mark chunk modified");

        // Clear modified flags, then move item across chunk boundary into chunk (1, 0) (x=16.5):
        w.chunks.get_mut(&(0, 0)).unwrap().is_modified = false;
        w.chunks.get_mut(&(1, 0)).unwrap().is_modified = false;
        w.move_body(item, 1.0, 0.0, 0.0);
        assert!(w.chunks.get(&(0, 0)).unwrap().is_modified, "leaving chunk (0, 0) must mark it modified");
        assert!(w.chunks.get(&(1, 0)).unwrap().is_modified, "entering chunk (1, 0) must mark it modified");

        // Clear modified flag on (1, 0), then age-despawn item at age 5999 -> 6000:
        w.chunks.get_mut(&(1, 0)).unwrap().is_modified = false;
        if let Some(crate::entity::table::Entity::Item(it)) = w.entities.get_mut(item) {
            it.age = 5999;
        }
        w.tick_item(item);
        assert!(w.chunks.get(&(1, 0)).unwrap().is_modified, "despawning item must mark chunk modified");
    }

    #[test]
    fn test_plants_and_farmland_do_not_schedule_infinite_tick_loops() {
        let mut w = world_with_floor();
        for cx in -1..=1 {
            for cz in -1..=1 {
                if (cx, cz) != (0, 0) {
                    w.insert_chunk(Chunk::new(cx, cz));
                }
            }
        }
        // Place sand at (4, 63, 4) and cactus at (4, 64, 4), farmland at (6, 63, 6) and crops at (6, 64, 6)
        w.set_block_id(4, 63, 4, 12);
        w.apply_set_notify(4, 64, 4, 81);
        w.set_block_id(6, 63, 6, 60);
        w.apply_set_notify(6, 64, 6, 59);
        w.neighbor_changed(4, 64, 4);
        w.neighbor_changed(6, 63, 6);
        w.neighbor_changed(6, 64, 6);
        assert!(
            !w.scheduled_set.iter().any(|&(_, _, _, id)| matches!(id, 6 | 18 | 59 | 60 | 81 | 83)),
            "placing or notifying cactus/farmland/crops must not schedule 1-tick self-rescheduling loops"
        );
        // Running random ticks on cactus/crops/farmland also schedules no plant/farmland ticks:
        crate::block::ticks::block_cactus_random_tick(
            &mut w,
            81,
            crate::block::pos::DropSpec::new(81, 1, 0),
            crate::block::pos::BlockPos::new(4, 64, 4),
        );
        crate::block::ticks::block_soil_tick(&mut w, 60, crate::block::pos::BlockPos::new(6, 63, 6));
        crate::block::ticks::block_crops_tick(
            &mut w,
            crate::block::ticks::CropIds { block: 59, crop: 59, wheat: 296, seeds: 295 },
            crate::block::pos::BlockPos::new(6, 64, 6),
        );
        assert!(
            !w.scheduled_set.iter().any(|&(_, _, _, id)| matches!(id, 6 | 18 | 59 | 60 | 81 | 83)),
            "ticking cactus/farmland/crops must not re-schedule themselves"
        );

        // Placing a non-opaque solid block (wooden stairs, id 53) above farmland (6, 63, 6) converts farmland to dirt (3):
        w.apply_set_notify(6, 64, 6, 53);
        assert_eq!(w.get_block_id(6, 63, 6), 3, "any solid block above farmland must revert farmland to dirt");
    }

    #[test]
    fn test_water_optimal_drop_path_and_lava_destroys_without_drop() {
        let mut w = world_with_floor();
        // Carve a drop-off hole at (6, 63, 4) (2 blocks +X from (4, 64, 4)):
        w.set_block_id(6, 63, 4, 0);
        // Place water source at (4, 64, 4) and run fluid tick:
        w.set_block_id(4, 64, 4, 8);
        w.set_block_meta(4, 64, 4, 0);
        crate::block::ticks::block_fluid_tick(&mut w, 8, false, crate::block::pos::BlockPos::new(4, 64, 4));
        // Water must flow ONLY toward +X (5, 64, 4) where the 2-step drop to (6, 63, 4) is, NOT -X/+Z/-Z:
        assert_eq!(w.get_block_id(5, 64, 4), 8, "water must flow toward nearby drop-off");
        assert_eq!(w.get_block_id(3, 64, 4), 0, "water must not spread away from optimal drop-off");
        assert_eq!(w.get_block_id(4, 64, 5), 0, "water must not spread away from optimal drop-off");
        assert_eq!(w.get_block_id(4, 64, 3), 0, "water must not spread away from optimal drop-off");

        // Lava flowing into a torch (50) destroys it WITHOUT spawning an item drop:
        let mut w2 = world_with_floor();
        w2.set_block_id(9, 64, 8, 50); // torch
        w2.set_block_meta(9, 64, 8, 5);
        w2.set_block_id(8, 64, 8, 10); // flowing lava source at center of chunk (0, 0)
        w2.set_block_meta(8, 64, 8, 0);
        let items_before = w2
            .entities
            .alive_ids()
            .into_iter()
            .filter(|&id| matches!(w2.entities.get(id), Some(crate::entity::table::Entity::Item(_))))
            .count();
        crate::block::ticks::block_fluid_tick(&mut w2, 10, true, crate::block::pos::BlockPos::new(8, 64, 8));
        let items_after = w2
            .entities
            .alive_ids()
            .into_iter()
            .filter(|&id| matches!(w2.entities.get(id), Some(crate::entity::table::Entity::Item(_))))
            .count();
        assert_eq!(w2.get_block_id(9, 64, 8), 10, "lava must flow into torch cell");
        assert_eq!(items_after, items_before, "lava must destroy torch without dropping item");
    }

    #[test]
    fn test_redstone_indirect_power_diagonal_cut_and_torch_burnout() {
        let mut w = world_with_floor();

        // 1. Indirect power through a solid block:
        // Redstone wire at (4, 64, 4) powered by redstone torch at (3, 64, 4), pointing into stone block at (5, 64, 4),
        // with a redstone torch attached to the other side of the stone block at (6, 64, 4) (meta=1, attached to x-1):
        w.apply_set_meta_notify(3, 64, 4, 76, 5);
        w.apply_set_notify(4, 64, 4, 55);
        w.apply_set_notify(5, 64, 4, 1); // solid stone block
        w.apply_set_meta_notify(6, 64, 4, 76, 1); // attached to (5, 64, 4)
        assert!(w.get_block_meta(4, 64, 4) > 0, "wire at (4, 64, 4) must be powered");
        assert!(w.is_block_powered(5, 64, 4), "stone block at (5, 64, 4) must be powered by wire pointing into it");

        // 2. Solid block cuts diagonal vertical wire connection:
        let mut w2 = world_with_floor();
        w2.apply_set_meta_notify(3, 64, 4, 76, 5); // torch powering (4, 64, 4)
        w2.apply_set_notify(4, 64, 4, 55); // lower wire
        w2.apply_set_notify(5, 64, 4, 1);  // step stone
        w2.apply_set_notify(5, 65, 4, 55); // upper wire on step
        assert!(w2.get_block_meta(5, 65, 4) > 0, "upper wire connects diagonally when air above lower wire");
        // Now place solid stone at (4, 65, 4) above the lower wire — cuts diagonal connection!
        w2.apply_set_notify(4, 65, 4, 1);
        assert_eq!(w2.get_block_meta(5, 65, 4), 0, "solid block above lower wire must cut diagonal wire power");

        // 3. Redstone torch burnout after 8 flips within 100 ticks:
        let mut w3 = world_with_floor();
        for cx in -1..=1 {
            for cz in -1..=1 {
                if (cx, cz) != (0, 0) {
                    w3.insert_chunk(Chunk::new(cx, cz));
                }
            }
        }
        w3.apply_set_notify(4, 64, 4, 1); // support stone at (4, 64, 4)
        w3.apply_set_meta_notify(5, 64, 4, 76, 1); // torch at (5, 64, 4) attached to (4, 64, 4)
        w3.apply_set_notify(5, 64, 5, 55); // wire powered by torch
        w3.apply_set_notify(4, 64, 5, 55); // wire leading back
        w3.apply_set_notify(4, 65, 4, 55); // wire on top of support stone (4, 64, 4)
        for _ in 0..40 {
            w3.time += 1;
            w3.process_scheduled_ticks();
        }
        assert_eq!(w3.get_block_id(5, 64, 4), 75, "short-circuit redstone torch must burn out to unlit (75)");
    }

    #[test]
    fn test_push_neighbors_skips_arrows_and_items_and_blast_flaming_ignites_fire() {
        use crate::entity::table::{ArrowEnt, Body, Entity};
        let mut w = world_with_floor();
        let mob = add_mob(&mut w, MobKind::Zombie, 4.5, 64.0, 4.5);
        let item = w.spawn_item_entity(264, 1, 0, 4.55, 64.0, 4.5);
        let aid = w.entities.alloc_id();
        let mut ab = Body::new(aid, 0.5, 0.5, 0.0);
        ab.set_position(4.55, 64.0, 4.5);
        w.entities.insert(Entity::Arrow(ArrowEnt {
            body: ab,
            tile: [-1, -1, -1],
            in_tile: 0,
            in_ground: false,
            shake: 0,
            shooter_id: -1,
            ticks_in_ground: 0,
            ticks_in_air: 0,
        }));
        if let Some(e) = w.entities.get_mut(item) {
            e.body_mut().motion = [0.0, 0.0, 0.0];
        }
        w.tick_mob(mob, &[mob, item, aid]);
        assert_eq!(w.entities.get(item).unwrap().body().motion, [0.0, 0.0, 0.0], "items must not be pushed by mobs");
        assert_eq!(w.entities.get(aid).unwrap().body().motion, [0.0, 0.0, 0.0], "arrows must not be pushed by mobs");

        // Flaming blast ignites fire (51) on air cells above solid floor:
        w.blast_flaming(8.5, 64.5, 8.5, 3.0, None, true);
        let mut found_fire = false;
        for x in 4..=12 {
            for y in 60..=66 {
                for z in 4..=12 {
                    if w.get_block_id(x, y, z) == 51 {
                        found_fire = true;
                    }
                }
            }
        }
        assert!(found_fire, "blast_flaming(is_flaming=true) must ignite fire (id 51) in crater");
    }

    #[test]
    fn test_apply_decoded_chunk_clears_modified_and_slab_farmland_light_value() {
        let mut w = world_with_floor();
        let encoded = crate::persist::encode_chunk_blob(&w, 0, 0, true).expect("chunk must encode");
        let decoded = crate::persist::decode_chunk_blob(&encoded, 0, 0).expect("chunk must decode");
        let mut w2 = World::new(12345);
        w2.apply_decoded_chunk(decoded);
        assert!(
            !w2.chunks.get(&(0, 0)).unwrap().is_modified,
            "chunk loaded from store must start with is_modified == false"
        );

        // Slab (44) and farmland (60) have opacity 255 in Chunk, so their own cell has light 0,
        // but World.block_light_value must return the max of (y+1, x+1, x-1, z+1, z-1):
        w.set_block_id(8, 63, 8, 44);
        w.set_block_id(10, 63, 8, 60);
        if let Some(ch) = w.chunks.get_mut(&(0, 0)) {
            ch.generate_skylight_map();
        }
        assert_eq!(w.block_light_value(8, 63, 8), 15, "slab at surface must inherit max neighbor light (15 from y+1)");
        assert_eq!(w.block_light_value(10, 63, 8), 15, "farmland at surface must inherit max neighbor light (15 from y+1)");
    }

    #[test]
    fn test_pressure_plates_block_water_and_lava_upward_fire_spread() {
        use crate::block::pos::BlockPos;
        let mut w = world_with_floor();
        // In Alpha 1.2.6 (BlockPressurePlate.java:10), both stone (70) and wooden (72) pressure plates
        // have Material.rock, so BlockFlowing.func_309_k returns true and water does not wash them away:
        w.set_block_id(8, 64, 8, 8);
        w.set_block_id(9, 64, 8, 70);
        w.set_block_id(7, 64, 8, 72);
        crate::block::ticks::block_fluid_tick(&mut w, 8, false, BlockPos::new(8, 64, 8));
        assert_eq!(w.get_block_id(9, 64, 8), 70, "stone pressure plate (70) blocks water in Alpha 1.2.6");
        assert_eq!(w.get_block_id(7, 64, 8), 72, "wooden pressure plate (72) blocks water in Alpha 1.2.6");

        // Place stationary lava (11) at (4, 64, 4) and flammable planks (5) above at (4, 65, 5) with air at (4, 65, 4):
        let mut w2 = world_with_floor();
        w2.set_block_id(4, 64, 4, 11);
        w2.set_block_id(4, 65, 5, 5);
        let mut ignited = false;
        for _ in 0..200 {
            crate::block::ticks::block_fluid_tick(&mut w2, 11, true, BlockPos::new(4, 64, 4));
            for dx in -2..=2 {
                for dy in 0..=3 {
                    for dz in -2..=2 {
                        if w2.get_block_id(4 + dx, 64 + dy, 4 + dz) == 51 {
                            ignited = true;
                        }
                    }
                }
            }
            if ignited {
                break;
            }
        }
        assert!(ignited, "lava must ignite air cells adjacent to flammable blocks above it");
    }


    #[test]
    fn test_block_props_and_void_damage_living_and_non_living() {
        use crate::block::table::{block_properties_get, BlockMaterial};
        use crate::entity::table::Entity;
        assert!(!block_properties_get(63).allows_attachment, "signPost (63) must have allows_attachment == false");
        assert_eq!(block_properties_get(66).material, BlockMaterial::Circuits as u8, "minecartTrack (66) must have BlockMaterial::Circuits");
        assert_eq!(block_properties_get(80).material, BlockMaterial::BuiltSnow as u8, "blockSnow (80) must have BlockMaterial::BuiltSnow");

        let mut w = world_with_floor();
        let mob = add_mob(&mut w, MobKind::Zombie, 4.5, -65.0, 4.5);
        let item = w.spawn_item_entity(264, 1, 0, 4.5, -65.0, 4.5);
        let hp_before = match w.entities.get(mob).unwrap() {
            Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        };
        w.tick_mob(mob, &[mob]);
        let hp_after = match w.entities.get(mob).unwrap() {
            Entity::Mob(m) => m.living.health,
            _ => unreachable!(),
        };
        assert_eq!(hp_before - hp_after, 4, "living entity below y=-64 must take 4 void damage per tick");

        w.entities.tick_base(item);
        assert!(w.entities.get(item).unwrap().body().dead, "non-living entity below y=-64 must die in tick_base");
    }

    #[test]
    fn test_snow_block_no_melt_and_stationary_lava_damage() {
        use crate::block::pos::BlockPos;
        use crate::entity::table::Entity;

        let mut w = world_with_floor();
        // Place snow layer (78) at (4, 64, 4) and snow block (80) at (6, 64, 4) with blocklight = 15:
        w.set_block_id(4, 64, 4, 78);
        w.set_block_id(6, 64, 4, 80);
        if let Some(c) = w.chunks.get_mut(&(0, 0)) {
            c.set_light_value(1, 4, 64, 4, 15);
            c.set_light_value(1, 6, 64, 4, 15);
        }

        w.update_block_tick(4, 64, 4);
        w.update_block_tick(6, 64, 4);
        assert_eq!(w.get_block_id(4, 64, 4), 0, "snow layer (78) must melt when blocklight > 11");
        assert_eq!(w.get_block_id(6, 64, 4), 80, "snow block (80) must NOT melt when blocklight > 11");

        // Bare stone cave around stationary lava (11) must never spawn fire (51):
        let mut w_cave = world_with_floor();
        w_cave.set_block_id(8, 64, 8, 11);
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            w_cave.set_block_id(8 + dx, 63, 8 + dz, 1);
            w_cave.set_block_id(8 + dx, 64, 8 + dz, 1);
        }
        for _ in 0..100 {
            crate::block::ticks::block_fluid_tick(&mut w_cave, 11, true, BlockPos::new(8, 64, 8));
        }
        for dx in -2..=2 {
            for dy in 0..=3 {
                for dz in -2..=2 {
                    assert_ne!(
                        w_cave.get_block_id(8 + dx, 64 + dy, 8 + dz),
                        51,
                        "lava in bare stone cave without burnable blocks must not spawn fire"
                    );
                }
            }
        }

        // Stationary player in lava (move_body NOT called) must take 4 damage and fire = 600 in tick_living:
        let mut w_lava = world_with_floor();
        w_lava.set_block_id(3, 64, 4, 11);
        let player = add_player(&mut w_lava, "steve", 3.5, 64.0, 4.5);
        w_lava.tick_player(player);
        let (hp, fire) = match w_lava.entities.get(player).unwrap() {
            Entity::Player(p) => (p.living.health, p.living.body.fire),
            _ => unreachable!(),
        };
        assert_eq!(hp, 16, "stationary player in lava must take 4 damage from handleLavaMovement");
        assert_eq!(fire, 600, "stationary player in lava must have fire set to 600");

        // Multi-tick hurt_resist cycle with armor: ticks 2..=10 are blocked by hurt_resist > 10,
        // and tick 11 deals the next 4-damage lava hit:
        set_slot(&mut w_lava, player, 1, 0, stk(306, 1, 0)); // iron helmet (3 armor pts)
        for _ in 0..9 {
            w_lava.tick_player(player);
        }
        let (hp_mid, helm_dmg_mid) = match w_lava.entities.get(player).unwrap() {
            Entity::Player(p) => (p.living.health, p.inventory.armor[0].unwrap().damage),
            _ => unreachable!(),
        };
        assert_eq!(hp_mid, 16, "ticks 2..=10 in lava must be blocked by hurt_resist > 10");
        assert_eq!(helm_dmg_mid, 0, "armor must not wear during hurt_resist > 10");
        w_lava.tick_player(player); // tick 11: hurt_resist == 10 <= 10 -> next 4-damage lava hit lands
        let (hp_next, helm_dmg_next) = match w_lava.entities.get(player).unwrap() {
            Entity::Player(p) => (p.living.health, p.inventory.armor[0].unwrap().damage),
            _ => unreachable!(),
        };
        assert!(hp_next < 16, "tick 11 in lava must deal damage once hurt_resist reaches 10");
        assert_eq!(helm_dmg_next, 4, "iron helmet must take 4 durability wear on tick 11 lava hit");
    }

    #[test]
    fn test_torch_suffocation_immunity_and_stone_suffocation() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        // Steve's eye position is y = 64.0 + 1.62 = 65.62, inside block (3, 65, 4).
        // With a torch at (3, 65, 4), Steve must not suffocate.
        w.set_block_id(3, 65, 4, 50);
        w.tick_player(player);
        let hp = match w.entities.get(player).unwrap() {
            Entity::Player(p) => p.living.health,
            _ => unreachable!(),
        };
        assert_eq!(hp, 20, "torch at eye level must not cause suffocation");

        // With stone at (3, 65, 4), Steve must take suffocation damage.
        w.set_block_id(3, 65, 4, 1);
        w.tick_player(player);
        let hp = match w.entities.get(player).unwrap() {
            Entity::Player(p) => p.living.health,
            _ => unreachable!(),
        };
        assert_eq!(hp, 19, "stone at eye level must cause suffocation");
    }

    #[test]
    fn test_collision_boxes_stairs_ladder_farmland_fence() {
        let mut w = world_with_floor();
        // 1. Stairs (53) with meta 0 (Ascending East) must return 2 collision boxes:
        w.set_block_id(3, 64, 4, 53);
        w.set_block_meta(3, 64, 4, 0);
        let mask = AxisAlignedBB::get_bounding_box(2.0, 63.0, 3.0, 5.0, 66.0, 5.0);
        let boxes = w.colliding_boxes(&mask);
        // Filter boxes to the stairs at (3, 64, 4):
        let stairs_boxes: Vec<_> = boxes
            .into_iter()
            .filter(|b| b.min_x >= 3.0 && b.max_x <= 4.0 && b.min_y >= 64.0 && b.max_y <= 65.0)
            .collect();
        assert_eq!(stairs_boxes.len(), 2, "stairs must produce 2 collision boxes");
        assert_eq!(stairs_boxes[0].max_y, 64.5);
        assert_eq!(stairs_boxes[1].max_y, 65.0);

        // 2. Farmland (60) must have a full 1.0 collision box height:
        w.set_block_id(5, 64, 5, 60);
        let farm_box = w.block_collision_box(5, 64, 5, 60);
        assert_eq!(farm_box.min_y, 64.0);
        assert_eq!(farm_box.max_y, 65.0, "farmland collision box must be full 1.0 height");

        // 3. Ladder (65) oriented collision box:
        w.set_block_id(7, 64, 7, 65);
        w.set_block_meta(7, 64, 7, 2); // meta 2: attached to z=8 (+z face)
        let ladder_box = w.block_collision_box(7, 64, 7, 65);
        assert_eq!(ladder_box.min_z, 7.875);
        assert_eq!(ladder_box.max_z, 8.0);

        // 4. Fence (85) at y=63 with height 1.5 must collide with an entity bounding box at y=64.0..65.8:
        w.set_block_id(9, 63, 9, 85);
        let feet_box = AxisAlignedBB::get_bounding_box(9.1, 64.0, 9.1, 9.7, 65.8, 9.7);
        let fence_coll = w.colliding_boxes(&feet_box);
        assert!(
            fence_coll.iter().any(|b| b.min_y == 63.0 && b.max_y == 64.5),
            "fence below player feet must be detected by colliding_boxes"
        );
    }

    #[test]
    fn test_ladder_drops_when_support_broken() {
        let mut w = world_with_floor();
        w.set_block_id(3, 64, 5, 1); // wall support
        w.set_block_id(3, 64, 4, 65); // ladder
        w.set_block_meta(3, 64, 4, 2); // attached to wall at z=5
        w.apply_set_notify(3, 64, 5, 0); // break wall
        assert_eq!(w.get_block_id(3, 64, 4), 0, "ladder must break when support block is removed");
        assert!(w.entities.alive_ids().into_iter().any(|oid| matches!(
            w.entities.get(oid).unwrap(),
            crate::entity::table::Entity::Item(e) if e.item_id == 65
        )));
    }

    #[test]
    fn test_walking_contact_redstone_ore() {
        let mut w = world_with_floor();
        for cx in -1..=1 {
            for cz in -1..=1 {
                if cx == 0 && cz == 0 {
                    continue;
                }
                add_floor_chunk(&mut w, cx, cz);
            }
        }
        w.set_block_id(3, 63, 4, 73); // idle redstone ore under feet
        let player = add_player(&mut w, "steve", 3.5, 66.0, 4.5);
        w.move_body(player, 0.0, -3.0, 0.0);
        assert_eq!(w.get_block_id(3, 63, 4), 74, "walking on redstone ore must light it up to id 74");
        // Scheduled update after 30 ticks must revert it to 73:
        for _ in 0..35 {
            w.time += 1;
            w.process_scheduled_ticks();
        }
        assert_eq!(w.get_block_id(3, 63, 4), 73, "redstone ore must revert to 73 after 30 ticks");
    }

    #[test]
    fn test_walking_contact_farmland_trample() {
        let mut w = world_with_floor();
        w.set_block_id(3, 63, 4, 60); // farmland under feet
        // Seed RNG so next_int_bound(4) returns 0 or try a loop
        let player = add_player(&mut w, "steve", 3.5, 66.0, 4.5);
        w.move_body(player, 0.0, -3.0, 0.0);
        // Over multiple steps, 1 in 4 chance will trample farmland to dirt (3):
        let mut trampled = w.get_block_id(3, 63, 4) == 3;
        for _ in 0..20 {
            if trampled {
                break;
            }
            w.move_body(player, 0.0, -0.01, 0.0);
            if w.get_block_id(3, 63, 4) == 3 {
                trampled = true;
            }
        }
        assert!(trampled, "walking on farmland must eventually trample it to dirt (3)");
    }

    #[test]
    fn test_non_opaque_blocks_suffocation_immunity() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 64.0, 4.5);
        // Steve's eye position is y = 64.0 + 1.62 = 65.62, inside block (3, 65, 4).
        for bid in [50, 52, 53, 79, 85] {
            w.set_block_id(3, 65, 4, bid);
            w.tick_player(player);
            let hp = match w.entities.get(player).unwrap() {
                Entity::Player(p) => p.living.health,
                _ => unreachable!(),
            };
            assert_eq!(hp, 20, "block id {bid} at eye level must not cause suffocation");
        }
    }

    #[test]
    fn test_torch_cannot_attach_to_fence_and_fence_cannot_stack() {
        let mut w = world_with_floor();
        // Fence has allows_attachment = false:
        assert!(!w.attach_at(BlockPos::new(3, 64, 4)));
        // Torch on side of fence must not be supported:
        w.set_block_id(3, 63, 4, 0); // clear floor under torch
        w.set_block_id(3, 64, 5, 85); // fence
        assert!(!crate::block::ticks::block_torch_can_stay(&w, BlockPos::new(3, 64, 4)));

        // Fence placement: can place on stone floor, but CANNOT place on top of another fence
        let player = add_player(&mut w, "steve", 5.5, 64.0, 5.5);
        let mut sess = PlaySession::new(player);
        // Can stay on solid floor:
        {
            let mut u = crate::item_verbs::ItemUseWorld {
                world: &mut w,
                session: &mut sess,
            };
            assert!(crate::item_verbs::item_block_use(
                &mut u,
                crate::item_verbs::BlockPlace { block_id: 85, stack_count: 1, side: 1, yaw: 0.0 },
                BlockPos::new(4, 63, 4) // floor at 63, places at 64
            ));
        }
        assert_eq!(w.get_block_id(4, 64, 4), 85);

        // Cannot place another fence on top of the fence at (4, 64, 4):
        {
            let mut u = crate::item_verbs::ItemUseWorld {
                world: &mut w,
                session: &mut sess,
            };
            assert!(!crate::item_verbs::item_block_use(
                &mut u,
                crate::item_verbs::BlockPlace { block_id: 85, stack_count: 1, side: 1, yaw: 0.0 },
                BlockPos::new(4, 64, 4) // target would be 65
            ));
        }
        assert_eq!(w.get_block_id(4, 65, 4), 0, "fence cannot be stacked on top of another fence");
    }

    #[test]
    fn test_pumpkin_placement_facing_metadata() {
        let mut w = world_with_floor();
        let player = add_player(&mut w, "steve", 3.5, 66.0, 4.5);
        let mut sess = PlaySession::new(player);
        // Alpha 1.2.6 formula: floor(yaw * 4 / 360 + 0.5) & 3
        let yaws_and_metas = [(0.0, 0), (90.0, 1), (180.0, 2), (270.0, 3)];
        for (i, (yaw, expected_meta)) in yaws_and_metas.iter().enumerate() {
            let x = 3 + i as i32;
            {
                let mut u = crate::item_verbs::ItemUseWorld {
                    world: &mut w,
                    session: &mut sess,
                };
                assert!(crate::item_verbs::item_block_use(
                    &mut u,
                    crate::item_verbs::BlockPlace { block_id: 86, stack_count: 1, side: 1, yaw: *yaw },
                    BlockPos::new(x, 63, 4)
                ));
            }
            assert_eq!(w.get_block_id(x, 64, 4), 86);
            assert_eq!(w.get_block_meta(x, 64, 4), *expected_meta, "pumpkin placed at yaw {yaw} must have meta {expected_meta}");
        }
    }
