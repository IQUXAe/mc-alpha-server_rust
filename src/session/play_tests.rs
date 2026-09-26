
    use super::*;
    use std::collections::HashSet;
    use crate::entity::table::{AnimalEnt, AnimalKind, MobEnt, MobKind, PlayerEnt};
    use crate::session::SessionBroadcast;
    use crate::session_packets::{pkt_map_chunk, pkt_pre_chunk};
    use crate::world::World;
    use crate::world::tiles::TileData;

    fn floor_world() -> World {
        let mut w = World::new(7);
        let mut c = crate::chunk::Chunk::new(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                c.set_block_id(x, 63, z, 1);
            }
        }
        c.generate_height_map();
        w.insert_chunk(c);
        w
    }

    fn spawn_player(w: &mut World, name: &str, x: f64, y: f64, z: f64) -> EntityId {
        let id = w.entities.alloc_id();
        let mut p = PlayerEnt::new(id, name);
        p.living.body.set_position(x, y, z);
        p.respawn_ticks = 0;
        w.entities.insert(Entity::Player(p));
        id
    }

    fn spawn_mob(w: &mut World, kind: MobKind, x: f64, y: f64, z: f64) -> EntityId {
        let id = w.entities.alloc_id();
        let mut m = MobEnt::new(id, kind);
        m.living.body.set_position(x, y, z);
        w.entities.insert(Entity::Mob(m));
        id
    }

    fn ctx<'a>(
        w: &'a mut World,
        ops: &'a HashSet<String>,
        bcast: &'a mut Vec<SessionBroadcast>,
    ) -> SessionCtx<'a> {
        SessionCtx { world: w, ops, spawn_protection: 0, pvp: true, broadcast: bcast }
    }

    fn no_ops() -> HashSet<String> {
        HashSet::new()
    }

    fn health_of(w: &World, id: EntityId) -> i16 {
        match w.entities.get(id).unwrap() {
            Entity::Mob(m) => m.living.health,
            Entity::Player(p) => p.living.health,
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_walk_and_teleport_kick() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        // One-block step east passes.
        let out = sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::PlayerLookMove {
                x: 4.5, y: 64.0, stance: 65.62, z: 4.5, yaw: 0.0, pitch: 0.0, on_ground: true,
            },
        );
        assert!(out.is_none());
        assert!((w.entities.get(player).unwrap().body().pos[0] - 4.5).abs() < 1e-6);
        // Cross-map jump is snapped back, never kicked (vanilla has no
        // speed kick): the server teleports home and waits for the echo.
        let out = sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::PlayerLookMove {
                x: 1000.0, y: 64.0, stance: 65.62, z: 1000.0, yaw: 0.0, pitch: 0.0,
                on_ground: true,
            },
        );
        assert!(out.is_none());
        assert!(sess.teleport_wait.is_some());
        assert_eq!(sess.outbox.last().unwrap()[0], 13);
        // The client echo clears the wait and resumes validation.
        let out = sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::PlayerLookMove {
                x: 4.5, y: 64.0, stance: 65.62, z: 4.5, yaw: 0.0, pitch: 0.0, on_ground: true,
            },
        );
        assert!(out.is_none());
        assert!(sess.teleport_wait.is_none());
    }

    #[test]
    fn test_dig_instant_torch() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        w.set_block_id(3, 64, 4, 50);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::BlockDig { status: 0, x: 3, y: 64, z: 4, face: 1 },
        );
        assert_eq!(w.get_block_id(3, 64, 4), 0);
        assert!(w.entities.alive_ids().into_iter().any(|oid| matches!(
            w.entities.get(oid).unwrap(),
            Entity::Item(e) if e.item_id == 50
        )));
        // The break is queued for broadcast so observers see it too.
        assert!(w.take_block_updates().contains(&[3, 64, 4]));
    }

    #[test]
    fn test_dig_dirt_progressive_with_hands() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        w.set_block_id(3, 64, 4, 3);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        for _ in 0..500 {
            sess.pump(
                &mut ctx(&mut w, &ops, &mut bc),
                PacketData::BlockDig { status: 1, x: 3, y: 64, z: 4, face: 1 },
            );
            if w.get_block_id(3, 64, 4) == 0 {
                break;
            }
        }
        assert_eq!(w.get_block_id(3, 64, 4), 0);
        // Bare hands harvest dirt.
        assert!(w.entities.alive_ids().into_iter().any(|oid| matches!(
            w.entities.get(oid).unwrap(),
            Entity::Item(e) if e.item_id == 3
        )));
        // The break is queued for broadcast so observers see it too.
        assert!(w.take_block_updates().contains(&[3, 64, 4]));
    }

    #[test]
    fn test_dig_stone_wears_pick() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        w.set_block_id(3, 64, 4, 1);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[0] = Some(ItemStack::new(270, 1, 0));
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        for _ in 0..3000 {
            sess.pump(
                &mut ctx(&mut w, &ops, &mut bc),
                PacketData::BlockDig { status: 1, x: 3, y: 64, z: 4, face: 1 },
            );
            if w.get_block_id(3, 64, 4) == 0 {
                break;
            }
        }
        assert_eq!(w.get_block_id(3, 64, 4), 0);
        let dmg = match w.entities.get(player).unwrap() {
            Entity::Player(p) => p.inventory.main[0].map(|s| s.damage),
            _ => unreachable!(),
        };
        assert!(dmg.unwrap_or(0) > 0, "stone pick should wear, dmg={dmg:?}");
    }

    #[test]
    fn test_dig_cancel_and_protection() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        w.set_block_id(3, 64, 4, 1);
        // Stone near spawn (3,4 vs spawn 0,64,0): protected at radius 16.
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        {
            let mut c = ctx(&mut w, &ops, &mut bc);
            c.spawn_protection = 16;
            sess.pump(&mut c, PacketData::BlockDig { status: 0, x: 3, y: 64, z: 4, face: 1 });
        }
        assert_eq!(w.get_block_id(3, 64, 4), 1);
        // Ops dig through.
        let mut ops_set = HashSet::new();
        ops_set.insert("steve".to_string());
        w.set_block_id(3, 64, 4, 50);
        {
            let mut c = SessionCtx {
                world: &mut w, ops: &ops_set, spawn_protection: 16, pvp: true, broadcast: &mut bc,
            };
            sess.pump(&mut c, PacketData::BlockDig { status: 0, x: 3, y: 64, z: 4, face: 1 });
        }
        assert_eq!(w.get_block_id(3, 64, 4), 0);
        // Cancel resets progress.
        w.set_block_id(5, 64, 5, 1);
        {
            let mut c = ctx(&mut w, &ops, &mut bc);
            sess.pump(&mut c, PacketData::BlockDig { status: 1, x: 5, y: 64, z: 5, face: 1 });
        }
        assert!(sess.dig.has_target);
        {
            let mut c = ctx(&mut w, &ops, &mut bc);
            sess.pump(&mut c, PacketData::BlockDig { status: 2, x: 5, y: 64, z: 5, face: 1 });
        }
        assert_eq!(sess.dig.cur_damage, 0.0);
    }

    #[test]
    fn test_place_dirt_and_rollback() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[0] = Some(ItemStack::new(3, 10, 0));
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Place { item_id: 3, x: 3, y: 63, z: 4, direction: 1 },
        );
        assert_eq!(w.get_block_id(3, 64, 4), 3);
        let count = match w.entities.get(player).unwrap() {
            Entity::Player(p) => p.inventory.main[0].map(|s| s.count),
            _ => unreachable!(),
        };
        assert_eq!(count, Some(9));
        // Rollback + inventory sync packets queued (3 sections + 2 changes).
        assert!(sess.outbox.len() >= 5);
        assert_eq!(sess.outbox[sess.outbox.len() - 2][0], 53);
    }

    #[test]
    fn test_eat_apple_heals() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        w.attack_living(player, 5, None);
        assert_eq!(health_of(&w, player), 15);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[0] = Some(ItemStack::new(260, 3, 0));
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Place { item_id: 260, x: 0, y: 0, z: 0, direction: -1 },
        );
        assert_eq!(health_of(&w, player), 19);
        let count = match w.entities.get(player).unwrap() {
            Entity::Player(p) => p.inventory.main[0].map(|s| s.count),
            _ => unreachable!(),
        };
        assert_eq!(count, Some(2));
    }

    #[test]
    fn test_punch_zombie_and_milk_cow() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let zombie = spawn_mob(&mut w, MobKind::Zombie, 4.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::UseEntity {
                player_entity_id: player, target_entity_id: zombie, is_left_click: true,
            },
        );
        assert_eq!(health_of(&w, zombie), 19);
        // Milk with a held bucket.
        let cid = w.entities.alloc_id();
        let mut cow = AnimalEnt::new(cid, AnimalKind::Cow);
        cow.living.body.set_position(4.5, 64.0, 5.5);
        w.entities.insert(Entity::Animal(cow));
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[0] = Some(ItemStack::new(325, 1, 0));
        }
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::UseEntity {
                player_entity_id: player, target_entity_id: cid, is_left_click: false,
            },
        );
        let held = match w.entities.get(player).unwrap() {
            Entity::Player(p) => p.inventory.main[0].map(|s| s.item_id),
            _ => unreachable!(),
        };
        assert_eq!(held, Some(335));
    }

    #[test]
    fn test_boat_mount_toggle() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let bid = w.entities.alloc_id();
        let mut b = crate::entity::table::Body::new(bid, 1.5, 0.6, 0.3);
        b.set_position(4.5, 64.0, 4.5);
        w.entities.insert(Entity::Boat(crate::entity::table::BoatEnt {
            body: b, time_since_hit: 0, damage_taken: 0, forward_dir: 1,
        }));
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        let use_boat = PacketData::UseEntity {
            player_entity_id: player, target_entity_id: bid, is_left_click: false,
        };
        sess.pump(&mut ctx(&mut w, &ops, &mut bc), use_boat);
        // PacketData is consumed; rebuild for the toggle.
        let riding = w.entities.get(player).unwrap().body().riding;
        assert_eq!(riding, bid);
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::UseEntity {
                player_entity_id: player, target_entity_id: bid, is_left_click: false,
            },
        );
        assert_eq!(w.entities.get(player).unwrap().body().riding, -1);
    }

    #[test]
    fn test_saddled_pig_mounts_on_interact() {
        use crate::entity::table::{AnimalEnt, AnimalKind};
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let pid = w.entities.alloc_id();
        let mut pig = AnimalEnt::new(pid, AnimalKind::Pig);
        pig.living.body.set_position(4.5, 64.0, 4.5);
        pig.saddled = true;
        w.entities.insert(Entity::Animal(pig));
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::UseEntity {
                player_entity_id: player, target_entity_id: pid, is_left_click: false,
            },
        );
        // Java EntityPig.interact boards the rider; the pig keeps the
        // saddle row and the rider position follows.
        assert_eq!(w.entities.get(player).unwrap().body().riding, pid);
        assert_eq!(w.entities.get(pid).unwrap().body().ridden_by, player);
        // Unsaddled pigs ignore the right-click.
        let qid = w.entities.alloc_id();
        let mut plain = AnimalEnt::new(qid, AnimalKind::Pig);
        plain.living.body.set_position(6.5, 64.0, 4.5);
        w.entities.insert(Entity::Animal(plain));
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::UseEntity {
                player_entity_id: player, target_entity_id: qid, is_left_click: false,
            },
        );
        assert_eq!(w.entities.get(player).unwrap().body().riding, pid);
    }

    #[test]
    fn test_chat_and_give() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Chat { message: "hi all".to_string() },
        );
        assert!(bc.iter().any(|b| matches!(b, SessionBroadcast::Chat(t) if t == "<Steve> hi all")));
        // Non-op /give spawns nothing.
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Chat { message: "/give 3 5".to_string() },
        );
        assert!(w.entities.alive_ids().into_iter().all(|oid| !matches!(
            w.entities.get(oid).unwrap(),
            Entity::Item(_)
        )));
        // Op /give drops dirt at the player.
        let mut ops_set = HashSet::new();
        ops_set.insert("steve".to_string());
        {
            let mut c = SessionCtx {
                world: &mut w, ops: &ops_set, spawn_protection: 0, pvp: true, broadcast: &mut bc,
            };
            sess.pump(&mut c, PacketData::Chat { message: "/give 3 5".to_string() });
        }
        assert!(w.entities.alive_ids().into_iter().any(|oid| matches!(
            w.entities.get(oid).unwrap(),
            Entity::Item(e) if e.item_id == 3 && e.count == 5
        )));
    }

    #[test]
    fn test_respawn_resets() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 30.5, 64.0, 30.5);
        w.attack_living(player, 100, None);
        assert!(w.entities.get(player).unwrap().body().dead);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(&mut ctx(&mut w, &ops, &mut bc), PacketData::Respawn);
        let p = w.entities.get(player).unwrap().body().clone();
        assert!(!p.dead);
        assert_eq!((p.pos[0], p.pos[2]), (0.5, 0.5));
        assert_eq!(health_of(&w, player), 20);
        assert_eq!(sess.outbox[0], vec![9]);
    }

    #[test]
    fn test_respawn_after_tick() {
        // Live scenario: death, a server tick (purge must spare the player
        // row), then the respawn packet. Before the purge fix the row was
        // gone and the button silently did nothing.
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 30.5, 64.0, 30.5);
        w.attack_living(player, 100, None);
        assert!(w.entities.get(player).unwrap().body().dead);
        w.tick_world();
        assert!(w.entities.get(player).is_some(), "dead player row survives purge");
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        assert!(sess.pump(&mut ctx(&mut w, &ops, &mut bc), PacketData::Respawn).is_none());
        assert!(!w.entities.get(player).unwrap().body().dead);
        assert_eq!(health_of(&w, player), 20);
    }

    #[test]
    fn test_fall_damage_on_stationary_packet() {
        // A landing reported without position change (Flying/Look) still
        // converts carried fall distance to damage like vanilla.
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.living.body.fall_distance = 10.0;
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        assert!(sess.pump(&mut ctx(&mut w, &ops, &mut bc), PacketData::Flying { on_ground: true }).is_none());
        assert_eq!(health_of(&w, player), 13);
    }

    #[test]
    fn test_eating_pork_heals_instead_of_killing() {
        // Regression probe for "pork kills at 2 hearts": with sane state
        // the eat path strictly heals (4 + 3 = 7). A death here would mean
        // max_health drifted to 0, not a food bug.
        use crate::item_data::ITEM_PORK_RAW;
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.living.health = 4;
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        let pork = crate::inventory::ItemStack::new(ITEM_PORK_RAW, 1, 0);
        assert!(sess.use_item_air(&mut ctx(&mut w, &ops, &mut bc), pork));
        assert_eq!(health_of(&w, player), 7);
        assert!(!w.entities.get(player).unwrap().body().dead);
    }

    #[test]
    fn test_held_switch_and_sneak() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[2] = Some(ItemStack::new(5, 4, 0));
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::BlockItemSwitch { entity_id: player, item_id: 5 },
        );
        assert_eq!(
            match w.entities.get(player).unwrap() {
                Entity::Player(p) => p.inventory.current,
                _ => unreachable!(),
            },
            2
        );
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::ArmAnimation { entity_id: player, animate: 104 },
        );
        assert!(matches!(
            w.entities.get(player).unwrap(),
            Entity::Player(p) if p.living.sneaking
        ));
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::ArmAnimation { entity_id: player, animate: 1 },
        );
        assert!(bc.iter().any(|b| matches!(b, SessionBroadcast::ArmSwing(id) if *id == player)));
    }

    #[test]
    fn test_respawn_stale_position_held_not_kicked() {
        // Death far from spawn, respawn, then a stale death-spot packet
        // arrives before the client processes the teleport: vanilla holds
        // it for the echo instead of kicking for speed.
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 30.5, 64.0, 30.5);
        w.attack_living(player, 100, None);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        assert!(sess.pump(&mut ctx(&mut w, &ops, &mut bc), PacketData::Respawn).is_none());
        let out = sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::PlayerLookMove {
                x: 30.5, y: 64.0, stance: 65.62, z: 30.5, yaw: 0.0, pitch: 0.0,
                on_ground: true,
            },
        );
        assert!(out.is_none(), "stale packet held, not kicked");
        assert!(sess.teleport_wait.is_some());
        // The echo clears the wait and play resumes.
        let out = sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::PlayerLookMove {
                x: 0.5, y: 64.0, stance: 65.62, z: 0.5, yaw: 0.0, pitch: 0.0,
                on_ground: true,
            },
        );
        assert!(out.is_none());
        assert!(sess.teleport_wait.is_none());
    }

    #[test]
    fn test_slot_35_survives_client_echo() {
        // The last main slot is real storage: client echoes apply to it
        // and the held dance never wipes it (it used to be skipped and
        // cleared, eating whatever the player parked there).
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[35] = Some(crate::inventory::ItemStack::new(3, 5, 0));
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        let mut slots =
            vec![crate::network::SlotData { item_id: -1, count: 0, damage: 0 }; 36];
        slots[35] = crate::network::SlotData { item_id: 3, count: 5, damage: 0 };
        assert!(sess
            .pump(&mut ctx(&mut w, &ops, &mut bc), PacketData::PlayerInventory {
                inventory_type: -1,
                slots,
            })
            .is_none());
        let kept = match w.entities.get(player).unwrap() {
            Entity::Player(p) => p.inventory.main[35].map(|s| (s.item_id, s.count)),
            _ => unreachable!(),
        };
        assert_eq!(kept, Some((3, 5)));
    }

    #[test]
    fn test_held_switch_finds_slot_35() {
        // Selecting the stack parked in slot 35 points current at it
        // instead of fabricating a ghost and wiping the slot.
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[35] = Some(crate::inventory::ItemStack::new(323, 1, 0));
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::BlockItemSwitch { entity_id: player, item_id: 323 },
        );
        let (cur, kept) = match w.entities.get(player).unwrap() {
            Entity::Player(p) => (p.inventory.current, p.inventory.main[35].map(|s| s.item_id)),
            _ => unreachable!(),
        };
        assert_eq!(cur, 35);
        assert_eq!(kept, Some(323));
    }

    #[test]
    fn test_sign_update_round_trip() {
        use crate::nbt::{NbtCompound, NbtTag, write_root};
        use std::collections::BTreeMap;
        use std::io::Write as _;
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        w.set_block_id(3, 64, 4, 63);
        w.tiles.insert(
            (3, 64, 4),
            TileData::Sign(crate::tile_entity::sign::sign_create()),
        );
        // Client sends gzipped sign NBT.
        let mut map = BTreeMap::new();
        map.insert("x".to_string(), NbtTag::Int(3));
        map.insert("y".to_string(), NbtTag::Int(64));
        map.insert("z".to_string(), NbtTag::Int(4));
        map.insert("Text1".to_string(), NbtTag::String("hello".to_string()));
        let mut raw = Vec::new();
        write_root(&mut raw, "", &NbtCompound { map }).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&raw).unwrap();
        let gz = enc.finish().unwrap();
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::ComplexEntity { x: 3, y: 64, z: 4, nbt_data: gz },
        );
        assert!(matches!(
            w.tiles.get(&(3, 64, 4)),
            Some(TileData::Sign(s)) if &s.lines[0][..5] == b"hello"
        ));
        assert!(bc.iter().any(|b| matches!(b, SessionBroadcast::TileChanged(3, 64, 4))));
        // Wrong coords are rejected.
        let mut raw = Vec::new();
        let mut map = BTreeMap::new();
        map.insert("x".to_string(), NbtTag::Int(9));
        map.insert("y".to_string(), NbtTag::Int(64));
        map.insert("z".to_string(), NbtTag::Int(4));
        write_root(&mut raw, "", &NbtCompound { map }).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(&raw).unwrap();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::ComplexEntity { x: 3, y: 64, z: 4, nbt_data: enc.finish().unwrap() },
        );
        assert!(matches!(
            w.tiles.get(&(3, 64, 4)),
            Some(TileData::Sign(s)) if &s.lines[1][..1] == b"\x00"
        ));
    }

    #[test]
    fn test_restore_held_selects_slot_or_fallback() {
        use crate::inventory::ItemStack;
        let mut w = floor_world();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[2] = Some(ItemStack::new(3, 5, 0));
        }
        let mut sess = PlaySession::new(player);
        // Saved id present: point at its slot.
        sess.restore_held(&mut w, 3);
        assert_eq!(sess.held_id, 3);
        assert!(sess.held_fallback.is_none());
        match w.entities.get(player).unwrap() {
            Entity::Player(p) => assert_eq!(p.inventory.current, 2),
            _ => unreachable!(),
        }
        // Saved id gone: fallback copy in the last slot.
        sess.restore_held(&mut w, 9999);
        match w.entities.get(player).unwrap() {
            Entity::Player(p) => assert_eq!(p.inventory.current, 35),
            _ => unreachable!(),
        }
        assert_eq!(sess.held_fallback.map(|s| s.item_id), Some(9999));
        // Non-positive id resets to the first slot.
        sess.restore_held(&mut w, 0);
        match w.entities.get(player).unwrap() {
            Entity::Player(p) => assert_eq!(p.inventory.current, 0),
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_kick_and_drop_end_session() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        let out = sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::KickDisconnect { reason: "bye".to_string() },
        );
        assert!(matches!(out, Some(SessionOutcome::Gone)));
        assert!(sess.pump(&mut ctx(&mut w, &ops, &mut bc), PacketData::Respawn).is_none());
    }

    #[test]
    fn test_pre_chunk_bytes_match_encoder() {
        assert_eq!(pkt_pre_chunk(3, -2, true), vec![50, 0, 0, 0, 3, 255, 255, 255, 254, 1]);
        assert_eq!(pkt_pre_chunk(0, 0, false).last(), Some(&0));
    }

    #[test]
    fn test_inventory_section_empty_slots_are_bare() {
        use crate::inventory::ItemStack;
        let dirt = Some(ItemStack::new(3, 5, 0));
        let pkt = pkt_inventory_section(-1, &[None, dirt, None]);
        // id 5, type -1, count 3, then FF FF | 00 03 05 00 00 | FF FF.
        // Empty slots are a bare short(-1): the 5-byte form desyncs
        // vanilla parsing and NPEs real clients rendering the hotbar.
        assert_eq!(
            pkt,
            vec![5, 255, 255, 255, 255, 0, 3, 255, 255, 0, 3, 5, 0, 0, 255, 255]
        );
    }

    #[test]
    fn test_map_chunk_bytes_match_send_map_chunk() {
        let data = vec![0x78, 0x9C, 0x01];
        let pkt = pkt_map_chunk(16, 0, -16, 16, 128, 16, &data);
        // id 51, x=16, y=0 (short), z=-16, sizes-1 (15,127,15), len=3.
        let head = vec![
            51, 0, 0, 0, 16, 0, 0, 255, 255, 255, 240, 15, 127, 15, 0, 0, 0, 3,
        ];
        assert_eq!(&pkt[..18], &head[..]);
        assert_eq!(&pkt[18..], &data[..]);
    }

    #[test]
    fn test_map_chunk_carries_compressed_chunk() {
        use std::io::Read;
        let mut c = crate::chunk::Chunk::new(0, 0);
        c.set_block_id(1, 64, 1, 1);
        let payload = c.map_compressed();
        let pkt = pkt_map_chunk(0, 0, 0, 16, 128, 16, &payload);
        let len = i32::from_be_bytes(pkt[14..18].try_into().unwrap());
        assert_eq!(len as usize, payload.len());
        let mut decoder = flate2::read::ZlibDecoder::new(&pkt[18..]);
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        assert_eq!(back, c.map_raw());
    }

    #[test]
    fn test_pickup_spawn_drops_item_entity_and_updates_inventory() {
        use crate::entity::table::Entity;
        use crate::inventory::ItemStack;
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        sess.held_id = 3;
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[0] = Some(ItemStack::new(3, 5, 0));
            p.inventory.current = 0;
        }
        let mut bc = Vec::new();
        let pkt = PacketData::PickupSpawn {
            entity_id: -1,
            item_id: 3,
            count: 1,
            x: (3.5 * 32.0) as i32,
            y: (64.0 * 32.0) as i32,
            z: (4.5 * 32.0) as i32,
            rotation: 10,
            pitch: 20,
            roll: 5,
        };
        sess.pump(&mut ctx(&mut w, &ops, &mut bc), pkt);

        // Player inventory slot 0 should be decremented from 5 to 4
        if let Some(Entity::Player(p)) = w.entities.get(player) {
            assert_eq!(p.inventory.main[0].as_ref().map(|s| s.count), Some(4));
        }

        // An item entity should have been spawned in the world
        let items: Vec<_> = w
            .entities
            .alive_ids()
            .into_iter()
            .filter_map(|id| match w.entities.get(id) {
                Some(Entity::Item(it)) => Some((id, it.item_id, it.count, it.pickup_delay, it.body.motion)),
                _ => None,
            })
            .collect();
        assert_eq!(items.len(), 1);
        let (_, item_id, count, pickup_delay, motion) = items[0];
        assert_eq!(item_id, 3);
        assert_eq!(count, 1);
        assert_eq!(pickup_delay, 10);
        assert!((motion[0] - 10.0 / 128.0).abs() < 1e-5);
        assert!((motion[1] - 20.0 / 128.0).abs() < 1e-5);
        assert!((motion[2] - 5.0 / 128.0).abs() < 1e-5);
    }

    #[test]
    fn test_pickup_spawn_rejects_dropping_more_than_inventory() {
        use crate::entity::table::Entity;
        use crate::inventory::ItemStack;
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        sess.held_id = 3;
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[0] = Some(ItemStack::new(3, 1, 0));
            p.inventory.current = 0;
        }
        let mut bc = Vec::new();
        let pkt = PacketData::PickupSpawn {
            entity_id: -1,
            item_id: 3,
            count: 64, // Player only has 1!
            x: (3.5 * 32.0) as i32,
            y: (64.0 * 32.0) as i32,
            z: (4.5 * 32.0) as i32,
            rotation: 0,
            pitch: 0,
            roll: 0,
        };
        sess.pump(&mut ctx(&mut w, &ops, &mut bc), pkt);

        // Inventory should remain 1
        if let Some(Entity::Player(p)) = w.entities.get(player) {
            assert_eq!(p.inventory.main[0].as_ref().map(|s| s.count), Some(1));
        }
        // No item entity should spawn
        let items_count = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|id| matches!(w.entities.get(*id), Some(Entity::Item(_))))
            .count();
        assert_eq!(items_count, 0);
    }

    #[test]
    fn test_pickup_spawn_rejects_missing_item_even_if_held() {
        use crate::entity::table::Entity;
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        sess.held_id = 264; // Diamond, but not in inventory
        let mut bc = Vec::new();
        let pkt = PacketData::PickupSpawn {
            entity_id: -1,
            item_id: 264,
            count: 1,
            x: (3.5 * 32.0) as i32,
            y: (64.0 * 32.0) as i32,
            z: (4.5 * 32.0) as i32,
            rotation: 0,
            pitch: 0,
            roll: 0,
        };
        sess.pump(&mut ctx(&mut w, &ops, &mut bc), pkt);

        let items_count = w
            .entities
            .alive_ids()
            .into_iter()
            .filter(|id| matches!(w.entities.get(*id), Some(Entity::Item(_))))
            .count();
        assert_eq!(items_count, 0);
    }

    #[test]
    fn test_sign_update_rejects_oversized_gzip_payload() {
        use std::io::Write;
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        w.set_block_id(3, 64, 4, 63);
        w.tiles.insert((3, 64, 4), TileData::Sign(crate::tile_entity::sign::sign_create()));
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        // 600 KB of zeroes compresses down to a few hundred bytes, but exceeds 524288 limit
        enc.write_all(&vec![0u8; 600_000]).unwrap();
        let gz = enc.finish().unwrap();

        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::ComplexEntity { x: 3, y: 64, z: 4, nbt_data: gz },
        );
        // Sign should not be updated or corrupted
        assert!(matches!(
            w.tiles.get(&(3, 64, 4)),
            Some(TileData::Sign(s)) if s.lines[0][0] == 0
        ));
    }



    #[test]
    fn test_strict_inventory_clamps_hacked_stacks() {
        // Vanilla assigns client stacks verbatim; strict mode clamps:
        // 127 diamonds -> 64, over-damage stone sword (max 64) -> 64,
        // unknown id 31000 -> dropped, 5x diamond sword (max 1) -> 1.
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        let mut slots =
            vec![crate::network::SlotData { item_id: -1, count: 0, damage: 0 }; 36];
        slots[0] = crate::network::SlotData { item_id: 264, count: 127, damage: 0 };
        slots[1] = crate::network::SlotData { item_id: 272, count: 1, damage: 500 };
        slots[2] = crate::network::SlotData { item_id: 31000, count: 10, damage: 0 };
        slots[3] = crate::network::SlotData { item_id: 276, count: 5, damage: 0 };
        assert!(sess
            .pump(&mut ctx(&mut w, &ops, &mut bc), PacketData::PlayerInventory {
                inventory_type: -1,
                slots,
            })
            .is_none());
        let got = match w.entities.get(player).unwrap() {
            Entity::Player(p) => [
                p.inventory.main[0].map(|s| (s.item_id, s.count, s.damage)),
                p.inventory.main[1].map(|s| (s.item_id, s.count, s.damage)),
                p.inventory.main[2].map(|s| (s.item_id, s.count, s.damage)),
                p.inventory.main[3].map(|s| (s.item_id, s.count, s.damage)),
            ],
            _ => unreachable!(),
        };
        assert_eq!(got[0], Some((264, 64, 0)));
        assert_eq!(got[1], Some((272, 1, 64)));
        assert_eq!(got[2], None);
        assert_eq!(got[3], Some((276, 1, 0)));
    }

    #[test]
    fn test_strict_held_switch_rejects_unknown_id() {
        // Unknown held ids clear the selection instead of arming a ghost
        // for an item that can never exist server-side.
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::BlockItemSwitch { entity_id: player, item_id: 31000 },
        );
        assert_eq!(sess.held_id, 0);
        assert!(sess.held_fallback.is_none());
    }

    #[test]
    fn test_strict_furnace_clamps_burn_and_cook() {
        // BurnTime 30000 -> 20000 (lava bucket max), CookTime 500 -> 200,
        // ItemBurnTime -5 -> 0; over-stacked furnace slots clamp to 64.
        use crate::nbt::{NbtCompound, NbtList, NbtTag, write_root};
        use std::collections::BTreeMap;
        use std::io::Write as _;
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        w.set_block_id(3, 64, 4, 61);
        w.tiles.insert((3, 64, 4), TileData::Furnace(crate::tile_entity::furnace::furnace_create()));
        let mut items = NbtList::with_type(10);
        let mut im = BTreeMap::new();
        im.insert("Slot".to_string(), NbtTag::Byte(0));
        im.insert("id".to_string(), NbtTag::Short(15));
        im.insert("Count".to_string(), NbtTag::Byte(100));
        im.insert("Damage".to_string(), NbtTag::Short(0));
        items.push(NbtTag::Compound(crate::nbt::NbtCompound { map: im }));
        let mut map = BTreeMap::new();
        map.insert("x".to_string(), NbtTag::Int(3));
        map.insert("y".to_string(), NbtTag::Int(64));
        map.insert("z".to_string(), NbtTag::Int(4));
        map.insert("BurnTime".to_string(), NbtTag::Short(30000));
        map.insert("CookTime".to_string(), NbtTag::Short(500));
        map.insert("ItemBurnTime".to_string(), NbtTag::Short(-5));
        map.insert("Items".to_string(), NbtTag::List(items));
        let mut raw = Vec::new();
        write_root(&mut raw, "", &NbtCompound { map }).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        let gz = enc.finish().unwrap();
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::ComplexEntity { x: 3, y: 64, z: 4, nbt_data: gz },
        );
        match w.tiles.get(&(3, 64, 4)) {
            Some(TileData::Furnace(s)) => {
                assert_eq!(s.burn_time, 20000);
                assert_eq!(s.cook_time, 200);
                assert_eq!(s.current_item_burn_time, 0);
                assert_eq!(s.slots[0].count, 64);
            }
            _ => panic!("furnace tile missing"),
        }
    }

    #[test]
    fn test_furnace_tile_packet_scales_burn_time_for_client_gui() {
        use crate::nbt::{NbtTag, read_root};
        use std::io::Read as _;

        // 1 coal (1600 ticks) consumed -> slot 1 is now empty (client uses 200 as max).
        // At 800 ticks remaining (50%), wire BurnTime must be 100 so client computes 100*12/200 = 6.
        let mut f = crate::tile_entity::furnace::furnace_create();
        f.slots[0] = ItemStack::new(15, 1, 0);
        f.burn_time = 800;
        f.cook_time = 50;
        f.current_item_burn_time = 1600;
        let pkt = crate::session_packets::tile_packet(3, 64, 4, &TileData::Furnace(f));
        assert_eq!(pkt[0], 59);
        let gz_len = u16::from_be_bytes([pkt[11], pkt[12]]) as usize;
        let mut dec = flate2::read::GzDecoder::new(&pkt[13..13 + gz_len]);
        let mut raw = Vec::new();
        dec.read_to_end(&mut raw).unwrap();
        let (_, root) = read_root(&mut std::io::Cursor::new(raw)).unwrap();
        assert_eq!(root.map.get("BurnTime"), Some(&NbtTag::Short(100)));
        assert_eq!(root.map.get("CookTime"), Some(&NbtTag::Short(50)));

        // Lava bucket (20000 ticks) at 1 tick left with empty fuel slot clamps to 1 (still burning).
        f.burn_time = 1;
        f.current_item_burn_time = 20000;
        let pkt = crate::session_packets::tile_packet(3, 64, 4, &TileData::Furnace(f));
        let gz_len = u16::from_be_bytes([pkt[11], pkt[12]]) as usize;
        let mut dec = flate2::read::GzDecoder::new(&pkt[13..13 + gz_len]);
        let mut raw = Vec::new();
        dec.read_to_end(&mut raw).unwrap();
        let (_, root) = read_root(&mut std::io::Cursor::new(raw)).unwrap();
        assert_eq!(root.map.get("BurnTime"), Some(&NbtTag::Short(1)));

        // Legacy/vanilla NBT furnace where current_item_burn_time == 0 and burn_time == 800
        // (coal, 1600 ticks max) with empty fuel slot must infer server_max = 1600 and scale
        // wire BurnTime to 100 instead of clamping at 200 (100%).
        f.burn_time = 800;
        f.current_item_burn_time = 0;
        let pkt = crate::session_packets::tile_packet(3, 64, 4, &TileData::Furnace(f));
        let gz_len = u16::from_be_bytes([pkt[11], pkt[12]]) as usize;
        let mut dec = flate2::read::GzDecoder::new(&pkt[13..13 + gz_len]);
        let mut raw = Vec::new();
        dec.read_to_end(&mut raw).unwrap();
        let (_, root) = read_root(&mut std::io::Cursor::new(raw)).unwrap();
        assert_eq!(root.map.get("BurnTime"), Some(&NbtTag::Short(100)));
    }

    #[test]
    fn test_active_furnace_slot_update_preserves_server_timers() {
        use crate::nbt::{NbtCompound, NbtList, NbtTag, write_root};
        use std::collections::BTreeMap;
        use std::io::Write as _;

        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 4.5);
        w.set_block_id(3, 64, 4, 62);
        let mut f = crate::tile_entity::furnace::furnace_create();
        f.burn_time = 1400;
        f.cook_time = 120;
        f.current_item_burn_time = 1600;
        w.tiles.insert((3, 64, 4), TileData::Furnace(f));

        // Client sends writeToNBT (no ItemBurnTime) with scaled BurnTime=175 and new input item.
        let mut items = NbtList::with_type(10);
        let mut im = BTreeMap::new();
        im.insert("Slot".to_string(), NbtTag::Byte(0));
        im.insert("id".to_string(), NbtTag::Short(15));
        im.insert("Count".to_string(), NbtTag::Byte(4));
        im.insert("Damage".to_string(), NbtTag::Short(0));
        items.push(NbtTag::Compound(NbtCompound { map: im }));
        let mut map = BTreeMap::new();
        map.insert("x".to_string(), NbtTag::Int(3));
        map.insert("y".to_string(), NbtTag::Int(64));
        map.insert("z".to_string(), NbtTag::Int(4));
        map.insert("BurnTime".to_string(), NbtTag::Short(175));
        map.insert("CookTime".to_string(), NbtTag::Short(0));
        map.insert("Items".to_string(), NbtTag::List(items));
        let mut raw = Vec::new();
        write_root(&mut raw, "", &NbtCompound { map }).unwrap();
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).unwrap();
        let gz = enc.finish().unwrap();

        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::ComplexEntity { x: 3, y: 64, z: 4, nbt_data: gz },
        );
        match w.tiles.get(&(3, 64, 4)) {
            Some(TileData::Furnace(s)) => {
                assert_eq!(s.burn_time, 1400);
                assert_eq!(s.cook_time, 120);
                assert_eq!(s.current_item_burn_time, 1600);
                assert_eq!(s.slots[0].item_id, 15);
                assert_eq!(s.slots[0].count, 4);
            }
            _ => panic!("furnace tile missing"),
        }
    }

    #[test]
    fn test_chest_placement_rejects_triple_chest_and_furnace_preserves_yaw_facing() {
        let mut w = floor_world();
        let ops = no_ops();
        let player = spawn_player(&mut w, "Steve", 3.5, 64.0, 6.5);
        if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
            p.inventory.main[0] = Some(ItemStack::new(54, 5, 0));
            p.inventory.main[1] = Some(ItemStack::new(61, 4, 0));
        }
        let mut sess = PlaySession::new(player);
        let mut bc = Vec::new();

        // Place first chest at (2, 64, 4) and second at (3, 64, 4) (double chest).
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Place { item_id: 54, x: 2, y: 63, z: 4, direction: 1 },
        );
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Place { item_id: 54, x: 3, y: 63, z: 4, direction: 1 },
        );
        assert_eq!(w.get_block_id(2, 64, 4), 54);
        assert_eq!(w.get_block_id(3, 64, 4), 54);

        // Third adjacent chest at (4, 64, 4) must be rejected.
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Place { item_id: 54, x: 4, y: 63, z: 4, direction: 1 },
        );
        assert_eq!(w.get_block_id(4, 64, 4), 0);

        // Place furnaces with 4 different player yaws and verify facing metadata (2, 5, 3, 4).
        for (fx, yaw, expected_meta) in [
            (5, 0.0f32, 2u8),
            (6, 90.0f32, 5u8),
            (7, 180.0f32, 3u8),
            (8, 270.0f32, 4u8),
        ] {
            if let Some(Entity::Player(p)) = w.entities.get_mut(player) {
                p.living.body.set_position(fx as f64 + 0.5, 64.0, 6.5);
                p.living.body.yaw = yaw;
                p.inventory.current = 1;
            }
            sess.pump(
                &mut ctx(&mut w, &ops, &mut bc),
                PacketData::Place { item_id: 61, x: fx, y: 63, z: 4, direction: 1 },
            );
            assert_eq!(w.get_block_id(fx, 64, 4), 61);
            assert_eq!(w.get_block_meta(fx, 64, 4), expected_meta);
        }

        // Right-clicking an existing furnace sends one Packet59 (not two duplicate Packet59s).
        sess.outbox.clear();
        sess.pump(
            &mut ctx(&mut w, &ops, &mut bc),
            PacketData::Place { item_id: -1, x: 5, y: 64, z: 4, direction: 1 },
        );
        let tile_pkts = sess.outbox.iter().filter(|p| p.first() == Some(&59)).count();
        assert_eq!(tile_pkts, 1, "activating a furnace must queue exactly one Packet59");
    }

    #[test]
    fn test_pkt_subchunk_furnace_light_includes_propagated_blocklight() {
        use std::io::Read as _;
        let mut w = floor_world();
        w.set_block_id(8, 64, 8, 62);
        w.set_block_meta(8, 64, 8, 4);
        w.refresh_light();

        let subchunks = crate::session_packets::pkt_subchunk_furnace_light(&w, 8, 64, 8);
        assert_eq!(subchunks.len(), 1);
        let ((cx, cz), pkt) = &subchunks[0];
        assert_eq!((*cx, *cz), (0, 0));
        assert_eq!(pkt[0], 51);
        let sx = pkt[11] as usize + 1;
        let sy = pkt[12] as usize + 1;
        let sz = pkt[13] as usize + 1;
        let mut dec = flate2::read::ZlibDecoder::new(&pkt[18..]);
        let mut raw = Vec::new();
        dec.read_to_end(&mut raw).unwrap();
        let vol = sx * sy * sz;
        assert_eq!(raw.len(), vol * 5 / 2);
        // Blocklight section starts at offset `vol + vol / 2` and has non-zero light around (8, 64, 8).
        let bl_slice = &raw[vol + vol / 2..vol + vol];
        assert!(
            bl_slice.iter().any(|&b| (b & 0xF) == 12 || (b >> 4) == 12),
            "subchunk blocklight must include neighbor light=12 around lit furnace"
        );
    }
