
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use crate::entity::table::{Entity, MobEnt, MobKind};
    use crate::server_config::ServerConfig;

    static TMP: AtomicU64 = AtomicU64::new(0);

    fn tmpdir(tag: &str) -> String {
        let id = TMP.fetch_add(1, Ordering::Relaxed);
        let dir = format!("/tmp/opencode_srv_{}_{}_{}", tag, std::process::id(), id);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mk_server(props_extra: &str) -> Server {
        let base = tmpdir("srv");
        let props = format!("{base}/server.properties");
        std::fs::write(
            &props,
            format!("online-mode=false\nlevel-seed=7\nspawn-monsters=false\nspawn-animals=false\nview-distance=3\n{props_extra}"),
        )
        .unwrap();
        let mut s = Server::open(
            &props,
            &format!("{base}/world"),
            &format!("{base}/world/players"),
            &format!("{base}/ops.txt"),
            &format!("{base}/banned-players.txt"),
            &format!("{base}/banned-ips.txt"),
        )
        .unwrap();
        // No unload churn under the streaming square (corners sit past
        // the default radius); production keeps the configured radius.
        s.world.unload_radius = 20;
        // Pre-populate the streaming square so tests never pay for
        // generation: queue is (view 3 + pad 3) and each send ensures
        // a 3x3 around it, so cover spawn +/- 9. Generation is
        // synchronous here; a miss would stall the tick for seconds.
        let (scx, scz) = (s.world.spawn[0].div_euclid(16), s.world.spawn[2].div_euclid(16));
        for cx in scx - 9..=scx + 9 {
            for cz in scz - 9..=scz + 9 {
                if !s.world.has_chunk(cx, cz) {
                    let mut c = crate::chunk::Chunk::new(cx, cz);
                    c.is_terrain_populated = true;
                    s.world.insert_chunk(c);
                } else if let Some(c) = s.world.chunk_ref_mut(cx, cz) {
                    c.is_terrain_populated = true;
                }
            }
        }
        if let Some(hell) = s.hell.as_mut() {
            hell.unload_radius = 20;
            let (hcx, hcz) = (hell.spawn[0].div_euclid(16), hell.spawn[2].div_euclid(16));
            for cx in hcx - 9..=hcx + 9 {
                for cz in hcz - 9..=hcz + 9 {
                    if !hell.has_chunk(cx, cz) {
                        let mut c = crate::chunk::Chunk::new(cx, cz);
                        c.is_terrain_populated = true;
                        hell.insert_chunk(c);
                    } else if let Some(c) = hell.chunk_ref_mut(cx, cz) {
                        c.is_terrain_populated = true;
                    }
                }
            }
        }
        s
    }

    fn pair(srv: &mut Server) -> (TcpStream, ConnId) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (stream, _) = l.accept().unwrap();
        let cid = srv.accept(stream).unwrap();
        (client, cid)
    }

    fn w_str(buf: &mut Vec<u8>, s: &str) {
        buf.extend_from_slice(&(s.len() as u16).to_be_bytes());
        buf.extend_from_slice(s.as_bytes());
    }

    fn cli_handshake(user: &str) -> Vec<u8> {
        let mut b = vec![2];
        w_str(&mut b, user);
        b
    }

    fn cli_login(proto: i32, user: &str) -> Vec<u8> {
        let mut b = vec![1];
        b.extend_from_slice(&proto.to_be_bytes());
        w_str(&mut b, user);
        w_str(&mut b, "");
        b.extend_from_slice(&0i64.to_be_bytes());
        b.push(0);
        b
    }

    fn cli_chat(msg: &str) -> Vec<u8> {
        let mut b = vec![3];
        w_str(&mut b, msg);
        b
    }

    /// Tick until the client has something, then take it (kills the
    /// write-then-tick race under parallel-test load).
    fn pump_until(client: &mut TcpStream, srv: &mut Server) -> (u8, String) {
        for _ in 0..200 {
            srv.tick();
            if let Some(p) = next_pkt_opt(client, Duration::from_millis(50)) {
                return p;
            }
        }
        panic!("no packet after 200 ticks");
    }

    /// Tick until a packet matches (tracker chatter never quiesces while
    /// two players stand together, so targeted reads skip ahead).
    fn pump_match(
        client: &mut TcpStream,
        srv: &mut Server,
        want: &dyn Fn(&(u8, String)) -> bool,
    ) -> (u8, String) {
        for _ in 0..400 {
            srv.tick();
            while let Some(p) = next_pkt_opt(client, Duration::from_millis(50)) {
                if want(&p) {
                    return p;
                }
            }
        }
        panic!("no matching packet after 400 ticks");
    }

    /// Drain everything buffered, keeping pre/map chunk pairs together;
    /// time and keep-alive interleave freely. Returns packet count.
    fn drain_all(client: &mut TcpStream) -> usize {
        let mut n = 0;
        while let Some((id, _)) = next_pkt_opt(client, Duration::from_millis(200)) {
            n += 1;
            if id == 50 {
                loop {
                    let (nid, _) = next_pkt(client);
                    n += 1;
                    if nid == 51 {
                        break;
                    }
                    assert!(nid == 4 || nid == 0, "between pre/map chunk: {nid}");
                }
            }
        }
        n
    }

    fn r_u16(c: &mut TcpStream) -> u16 {
        let mut b = [0u8; 2];
        c.read_exact(&mut b).unwrap();
        u16::from_be_bytes(b)
    }

    fn r_str(c: &mut TcpStream) -> String {
        let n = r_u16(c) as usize;
        let mut b = vec![0u8; n];
        c.read_exact(&mut b).unwrap();
        String::from_utf8_lossy(&b).into_owned()
    }

    fn r_skip(c: &mut TcpStream, n: usize) {
        let mut b = vec![0u8; n];
        c.read_exact(&mut b).unwrap();
    }

    /// Next packet: id plus captured string for text packets.
    /// The id byte waits up to `wait`; the body then gets 5 seconds
    /// (it is always already in flight on loopback).
    fn next_pkt_opt(c: &mut TcpStream, wait: Duration) -> Option<(u8, String)> {
        c.set_read_timeout(Some(wait)).ok()?;
        let mut id = [0u8; 1];
        c.read_exact(&mut id).ok()?;
        c.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let text = match id[0] {
            0 => String::new(),
            1 => {
                r_skip(c, 4);
                r_str(c);
                r_str(c);
                r_skip(c, 8);
                let mut dim = [0u8; 1];
                c.read_exact(&mut dim).unwrap();
                format!("{}", dim[0] as i8)
            }
            2 | 3 | 255 => r_str(c),
            4 => {
                r_skip(c, 8);
                String::new()
            }
            5 => {
                r_skip(c, 4);
                let n = r_u16(c) as usize;
                // Vanilla slot layout: bare short(-1) when empty,
                // else short id + byte count + short damage.
                for _ in 0..n {
                    let mut b = [0u8; 2];
                    c.read_exact(&mut b).unwrap();
                    if i16::from_be_bytes(b) >= 0 {
                        r_skip(c, 3);
                    }
                }
                String::new()
            }
            6 => {
                r_skip(c, 12);
                String::new()
            }
            8 => {
                let mut b = [0u8; 1];
                c.read_exact(&mut b).unwrap();
                b[0].to_string()
            }
            9 => String::new(),
            13 => {
                r_skip(c, 41);
                String::new()
            }
            16 => {
                r_skip(c, 6);
                String::new()
            }
            18 => {
                r_skip(c, 5);
                String::new()
            }
            20 => {
                r_skip(c, 4);
                let name = r_str(c);
                r_skip(c, 16);
                name
            }
            21 => {
                r_skip(c, 4 + 2 + 1 + 12 + 3);
                String::new()
            }
            22 => {
                r_skip(c, 8);
                String::new()
            }
            23 => {
                r_skip(c, 17);
                String::new()
            }
            24 => {
                r_skip(c, 4 + 1 + 12 + 2);
                String::new()
            }
            28 => {
                r_skip(c, 10);
                String::new()
            }
            29 => {
                r_skip(c, 4);
                String::new()
            }
            30 => {
                r_skip(c, 4);
                String::new()
            }
            31 => {
                r_skip(c, 7);
                String::new()
            }
            32 => {
                r_skip(c, 6);
                String::new()
            }
            33 => {
                r_skip(c, 9);
                String::new()
            }
            34 => {
                r_skip(c, 18);
                String::new()
            }
            38 => {
                r_skip(c, 5);
                String::new()
            }
            39 => {
                r_skip(c, 8);
                String::new()
            }
            50 => {
                r_skip(c, 9);
                String::new()
            }
            51 => {
                r_skip(c, 4 + 2 + 4 + 3);
                let n = {
                    let mut b = [0u8; 4];
                    c.read_exact(&mut b).unwrap();
                    i32::from_be_bytes(b) as usize
                };
                r_skip(c, n);
                String::new()
            }
            53 => {
                r_skip(c, 11);
                String::new()
            }
            59 => {
                r_skip(c, 4 + 2 + 4);
                let n = r_u16(c) as usize;
                r_skip(c, n);
                String::new()
            }
            60 => {
                r_skip(c, 8 + 8 + 8 + 4);
                let n = {
                    let mut b = [0u8; 4];
                    c.read_exact(&mut b).unwrap();
                    i32::from_be_bytes(b) as usize
                };
                r_skip(c, n * 3);
                String::new()
            }
            other => panic!("unexpected packet id {other}"),
        };
        Some((id[0], text))
    }

    fn next_pkt(c: &mut TcpStream) -> (u8, String) {
        next_pkt_opt(c, Duration::from_secs(5)).expect("packet within timeout")
    }

    /// Join a player up to the join burst (through the time packet).
    /// Chunk streaming continues in the background; use [`drain_chunks`]
    /// when the test needs a quiet buffer afterwards.
    fn join_burst(srv: &mut Server, client: &mut TcpStream, name: &str) {
        client.write_all(&cli_handshake(name)).unwrap();
        assert_eq!(pump_until(client, srv).0, 2);
        client.write_all(&cli_login(6, name)).unwrap();
        let mut ids = Vec::new();
        for _ in 0..8 {
            ids.push(pump_until(client, srv).0);
        }
        // The newcomer never hears its own join chat (mirrors the C++
        // broadcast before playerLoggedIn, which excludes the newcomer).
        assert_eq!(ids, vec![1, 6, 13, 8, 5, 5, 5, 4], "join burst for {name}");
    }

    /// Flush one session's chunk queue, draining packets as they arrive
    /// (tracker chatter is ignored here).
    fn drain_chunks(srv: &mut Server, client: &mut TcpStream) {
        for _ in 0..60 {
            srv.tick();
            let got = drain_all(client);
            let empty = match srv.sessions.values().find_map(|s| match &s.state {
                SessionState::Play(_, stream) => Some(stream.queue.is_empty()),
                SessionState::Login(_) => None,
            }) {
                Some(e) => e,
                None => break,
            };
            if empty && got == 0 {
                break;
            }
        }
        drain_all(client);
    }

    /// Join a player, returning after the join burst (through the time
    /// packet); chunk packets that follow are drained.
    fn join(srv: &mut Server, client: &mut TcpStream, name: &str) {
        join_burst(srv, client, name);
        drain_chunks(srv, client);
    }

    #[test]
    fn settings_parse_clamps_and_seed_rules() {
        let base = tmpdir("props");
        let props = format!("{base}/server.properties");
        std::fs::write(
            &props,
            "view-distance=99\ndifficulty=9\nlevel-seed=123abc\nmax-players=3\n",
        )
        .unwrap();
        let mut cfg = ServerConfig::open(&props);
        let s = load_settings(&mut cfg);
        assert_eq!(s.view_distance, 15);
        assert_eq!(s.difficulty, 3);
        assert_eq!(s.seed, 123);
        assert_eq!(s.max_players, 3);
        // String seeds hash like Java; empty means random-at-open.
        std::fs::write(&props, "level-seed=abc\n").unwrap();
        let mut cfg = ServerConfig::open(&props);
        assert_eq!(load_settings(&mut cfg).seed, 96354);
        std::fs::write(&props, "").unwrap();
        let mut cfg = ServerConfig::open(&props);
        assert_eq!(load_settings(&mut cfg).seed, 0);
        // Negative distances floor at the minimum.
        std::fs::write(&props, "view-distance=1\nspawn-protection-radius=-5\n").unwrap();
        let mut cfg = ServerConfig::open(&props);
        let s = load_settings(&mut cfg);
        assert_eq!(s.view_distance, 3);
        assert_eq!(s.spawn_protection, 0);
    }

    #[test]
    fn offline_login_sends_join_burst() {
        let mut srv = mk_server("");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");
        assert_eq!(srv.players.len(), 1);
    }

    #[test]
    fn dead_mob_is_destroyed_for_watchers() {
        // Killed mobs must vanish client-side: death status (38/3) then
        // destroy (29). Before the tracker fix the prune was silent and
        // corpses hung around frozen.
        let mut srv = mk_server("");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");
        let eid = *srv.players.keys().next().unwrap();
        let pos = match srv.world.entities.get(eid) {
            Some(e) => e.body().pos,
            _ => panic!("player row"),
        };
        let mid = srv.world.entities.alloc_id();
        let mut m = MobEnt::new(mid, MobKind::Zombie);
        m.living.body.set_position(pos[0], pos[1], pos[2]);
        srv.world.entities.insert(Entity::Mob(m));
        assert_eq!(pump_match(&mut client, &mut srv, &|p| p.0 == 24).0, 24);
        srv.world.attack_living(mid, 100, None);
        assert_eq!(pump_match(&mut client, &mut srv, &|p| p.0 == 29).0, 29);
    }

    #[test]
    fn fall_damage_pushes_health_packet() {
        //Like a 9-block fall: 6 damage must reach the HUD as 0x08 on the
        // next tick (mirrors the Packet8 diff-check in EntityPlayerMP).
        let mut srv = mk_server("");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");
        let eid = *srv.players.keys().next().unwrap();
        if let Some(Entity::Player(p)) = srv.world.entities.get_mut(eid) {
            p.respawn_ticks = 0;
        }
        srv.world.attack_living(eid, 6, None);
        let (id, text) =
            pump_match(&mut client, &mut srv, &|p| p.0 == 8);
        assert_eq!(id, 8);
        assert_eq!(text, "14");
        assert_eq!(srv.world.entities.get(eid).map(|e| e.body().dead), Some(false));
    }

    #[test]
    fn pickup_sends_collect_and_inventory() {
        // Dirt dropped at the player's feet must arrive as Packet22Collect
        // (pop sound + fly-over animation) plus a full inventory sync.
        let mut srv = mk_server("");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");
        let eid = *srv.players.keys().next().unwrap();
        let pos = match srv.world.entities.get(eid) {
            Some(e) => e.body().pos,
            _ => panic!("player row"),
        };
        let item = srv.world.spawn_item_entity(3, 5, 0, pos[0], pos[1], pos[2]);
        if let Some(Entity::Item(e)) = srv.world.entities.get_mut(item) {
            e.pickup_delay = 0;
        }
        assert_eq!(pump_match(&mut client, &mut srv, &|p| p.0 == 22).0, 22);
        assert_eq!(pump_match(&mut client, &mut srv, &|p| p.0 == 5).0, 5);
        let dirt: i32 = match srv.world.entities.get(eid).unwrap() {
            Entity::Player(p) => p.inventory.main.iter().filter_map(|s| *s).map(|s| s.count).sum(),
            _ => unreachable!(),
        };
        assert_eq!(dirt, 5);
    }

    /// Login attempt that must end in a kick with the given reason.
    fn login_kicked(props_extra: &str, name: &str, proto: i32, reason_part: &str) {
        let mut srv = mk_server(props_extra);
        let (mut client, _cid) = pair(&mut srv);
        client.write_all(&cli_handshake(name)).unwrap();
        assert_eq!(pump_until(&mut client, &mut srv).0, 2);
        client.write_all(&cli_login(proto, name)).unwrap();
        let (id, text) = pump_until(&mut client, &mut srv);
        assert_eq!(id, 255);
        assert!(text.contains(reason_part), "kick text: {text}");
        assert!(srv.players.is_empty());
    }

    #[test]
    fn login_rejects_bad_protocol_and_names() {
        login_kicked("", "Steve", 5, "Outdated client!");
        login_kicked("", "Steve", 7, "Outdated server!");
        login_kicked("", "Bad Name!", 6, "Invalid username characters");
        login_kicked("", "", 6, "Invalid username length");
    }

    #[test]
    fn login_rejects_banned_and_full() {
        // Banned name (file holds lowercase like C++).
        let base = tmpdir("ban");
        let props = format!("{base}/server.properties");
        std::fs::write(&props, "online-mode=false\nlevel-seed=7\n").unwrap();
        std::fs::write(format!("{base}/banned-players.txt"), "steve\n").unwrap();
        let mut banned = Server::open(
            &props,
            &format!("{base}/world"),
            &format!("{base}/world/players"),
            &format!("{base}/ops.txt"),
            &format!("{base}/banned-players.txt"),
            &format!("{base}/banned-ips.txt"),
        )
        .unwrap();
        let (mut client, _cid) = pair(&mut banned);
        client.write_all(&cli_handshake("Steve")).unwrap();
        assert_eq!(pump_until(&mut client, &mut banned).0, 2);
        client.write_all(&cli_login(6, "Steve")).unwrap();
        let (id, text) = pump_until(&mut client, &mut banned);
        assert_eq!(id, 255);
        assert!(text.contains("banned"), "kick text: {text}");
    }

    #[test]
    fn login_rejects_full_server() {
        let mut srv = mk_server("max-players=1\n");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let (mut b, _cid) = pair(&mut srv);
        b.write_all(&cli_handshake("Alex")).unwrap();
        assert_eq!(pump_until(&mut b, &mut srv).0, 2);
        b.write_all(&cli_login(6, "Alex")).unwrap();
        let (id, text) = pump_until(&mut b, &mut srv);
        assert_eq!(id, 255);
        assert!(text.contains("full"), "kick text: {text}");
        assert_eq!(srv.players.len(), 1);
    }

    #[test]
    fn chat_broadcasts_to_everyone() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let (mut b, _cid) = pair(&mut srv);
        join(&mut srv, &mut b, "Alex");
        // B's join notice landed on A first (past any chatter).
        assert_eq!(
            pump_match(&mut a, &mut srv, &|p| p == &(3, "§eAlex joined the game.".to_string())),
            (3, "§eAlex joined the game.".to_string())
        );
        a.write_all(&cli_chat("hi all")).unwrap();
        // Sender and newcomer both hear it, formatted once by the session.
        // Tracker keep-alives may sit ahead in the buffers; skip to chat.
        let mut heard_a = false;
        let mut heard_b = false;
        for _ in 0..40 {
            srv.tick();
            while let Some(p) = next_pkt_opt(&mut a, Duration::from_millis(50)) {
                if p == (3, "<Steve> hi all".to_string()) {
                    heard_a = true;
                }
            }
            while let Some(p) = next_pkt_opt(&mut b, Duration::from_millis(50)) {
                if p == (3, "<Steve> hi all".to_string()) {
                    heard_b = true;
                }
            }
            if heard_a && heard_b {
                break;
            }
        }
        assert!(heard_a && heard_b);
    }

    #[test]
    fn quit_broadcasts_leave_and_saves() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let (mut b, _cid) = pair(&mut srv);
        join(&mut srv, &mut b, "Alex");
        assert_eq!(
            pump_match(&mut a, &mut srv, &|p| p == &(3, "§eAlex joined the game.".to_string())),
            (3, "§eAlex joined the game.".to_string())
        );
        // Client quit (255 ends the socket; the server sees Dropped).
        let mut bye = vec![255u8];
        w_str(&mut bye, "Quitting");
        b.write_all(&bye).unwrap();
        let mut saw_leave = false;
        for _ in 0..40 {
            srv.tick();
            while let Some(p) = next_pkt_opt(&mut a, Duration::from_millis(50)) {
                if p.0 == 3 && p.1.contains("Alex") && p.1.contains("left the game") {
                    saw_leave = true;
                }
            }
            if saw_leave {
                break;
            }
        }
        assert!(saw_leave);
        assert_eq!(srv.players.len(), 1);
        // The quitter's row was saved (lowercase name like C++).
        assert!(std::path::Path::new(&format!("{}/alex.dat", srv.player_dir)).exists());
    }

    #[test]
    fn idle_connection_times_out() {
        let mut srv = mk_server("");
        let (mut a, cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        if let Some(sess) = srv.sessions.get_mut(&cid) {
            sess.idle = READ_TIMEOUT_TICKS - 1;
        }
        srv.tick();
        let (id, text) = next_pkt(&mut a);
        assert_eq!(id, 255);
        assert!(text.contains("Timed out"), "kick text: {text}");
        assert!(srv.players.is_empty());
    }

    #[test]
    fn time_broadcasts_every_second_and_autosave_runs() {
        let mut srv = mk_server("auto-save-interval=5\n");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        for _ in 0..20 {
            srv.tick();
        }
        // Drain to the time packet (keep-alive may also appear).
        let mut saw_time = false;
        while let Some((id, _)) = next_pkt_opt(&mut a, Duration::from_millis(300)) {
            if id == 4 {
                saw_time = true;
                break;
            }
        }
        assert!(saw_time);
        // Autosave wrote the player file (lowercase name like C++).
        let mut found = false;
        for entry in std::fs::read_dir(format!(
            "{}/world/players",
            srv.level_dir.rsplit_once('/').map(|(b, _)| b).unwrap_or("")
        ))
        .unwrap()
        {
            let entry = entry.unwrap();
            if entry.file_name().to_string_lossy() == "steve.dat" {
                found = true;
            }
        }
        assert!(found);
    }

    #[test]
    fn furnace_flips_reach_loaded_players() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        // Idle furnace with fuel right next to spawn.
        let (fx, fz) = (srv.world.spawn[0], srv.world.spawn[2]);
        srv.world.set_block_id(fx, 64, fz, 61);
        let mut f = crate::tile_entity::furnace::furnace_create();
        f.slots[0] = crate::inventory::ItemStack::new(4, 1, 0);
        f.slots[1] = crate::inventory::ItemStack::new(5, 1, 0);
        srv.world.tiles.insert((fx, 64, fz), crate::world::TileData::Furnace(f));
        // Drain first (streaming/tracker noise), then tick into the flip.
        while next_pkt_opt(&mut a, Duration::from_millis(200)).is_some() {}
        srv.tick();
        let mut flip_pkts = Vec::new();
        while let Some((id, _)) = next_pkt_opt(&mut a, Duration::from_millis(100)) {
            flip_pkts.push(id);
        }
        assert!(flip_pkts.contains(&53), "must send Packet53BlockChange on ignition: {flip_pkts:?}");
        assert_eq!(
            flip_pkts.iter().filter(|&&id| id == 59).count(),
            1,
            "ignition tick must send Packet59 once (not duplicated by drain_tile_updates): {flip_pkts:?}"
        );
        assert_eq!(srv.world.get_block_id(fx, 64, fz), 62);
        // Tick again and verify dynamic progress updates (Packet59ComplexEntity, id 59)
        // arrive followed by a Packet51MapChunk (id 51) subchunk that preserves facing metadata.
        srv.tick();
        let mut cook_pkts = Vec::new();
        while let Some((id, _)) = next_pkt_opt(&mut a, Duration::from_millis(100)) {
            cook_pkts.push(id);
        }
        assert!(cook_pkts.contains(&59), "cooking ticks must stream Packet59 updates: {cook_pkts:?}");
        assert!(cook_pkts.contains(&51), "cooking Packet59 must be followed by Packet51 subchunk: {cook_pkts:?}");
    }

    #[test]
    fn tracker_introduces_players_to_each_other() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let (mut b, _cid) = pair(&mut srv);
        // Burst only: the introductions below happen while B streams,
        // and a full drain would throw them away.
        join_burst(&mut srv, &mut b, "Alex");
        // Both stand at spawn: tick until each sees a named spawn.
        let mut saw_a = false;
        let mut saw_b = false;
        for _ in 0..40 {
            srv.tick();
            while let Some((id, text)) = next_pkt_opt(&mut a, Duration::from_millis(200)) {
                if id == 20 && text == "Alex" {
                    saw_a = true;
                }
            }
            while let Some((id, text)) = next_pkt_opt(&mut b, Duration::from_millis(200)) {
                if id == 20 && text == "Steve" {
                    saw_b = true;
                }
            }
            if saw_a && saw_b {
                break;
            }
        }
        assert!(saw_a && saw_b);
    }

    #[test]
    fn console_op_unlocks_give_and_persists() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        // Mixed-case console op lands lowercased on disk like C++.
        srv.queue_console("op Steve".to_string());
        srv.tick();
        let ops_dir = srv.ops_path.rsplit_once('/').map(|(b, _)| b).unwrap_or(".");
        let ops_body = std::fs::read_to_string(format!("{ops_dir}/ops.txt")).unwrap();
        assert!(ops_body.contains("steve"));
        // ...and gates /give through the session driver.
        a.write_all(&cli_chat("/give 3 5")).unwrap();
        let mut gave = false;
        for _ in 0..20 {
            srv.tick();
            for eid in srv.world.entities.alive_ids() {
                if let Some(Entity::Item(e)) = srv.world.entities.get(eid) {
                    if e.item_id == 3 && e.count == 5 {
                        gave = true;
                    }
                }
            }
            if gave {
                break;
            }
        }
        assert!(gave);
        // De-op closes the gate again.
        srv.queue_console("deop Steve".to_string());
        srv.tick();
        assert!(!srv.ops.contains("steve"));
    }

    #[test]
    fn console_ban_blocks_next_login() {
        let mut srv = mk_server("");
        srv.queue_console("ban Steve".to_string());
        srv.tick();
        let (mut client, _cid) = pair(&mut srv);
        client.write_all(&cli_handshake("Steve")).unwrap();
        assert_eq!(pump_until(&mut client, &mut srv).0, 2);
        client.write_all(&cli_login(6, "Steve")).unwrap();
        let (id, text) = pump_until(&mut client, &mut srv);
        assert_eq!(id, 255);
        assert!(text.contains("banned"));
        // Pardon re-opens the door.
        srv.queue_console("pardon Steve".to_string());
        srv.tick();
        let (mut client2, _cid) = pair(&mut srv);
        join(&mut srv, &mut client2, "Steve");
        assert_eq!(srv.players.len(), 1);
    }

    #[test]
    fn console_kick_stop_save_list() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        srv.queue_console("kick Steve".to_string());
        srv.tick();
        // Background block changes can arrive first; wait for the kick.
        let (id, text) = pump_match(&mut a, &mut srv, &|(id, _)| *id == 255);
        assert_eq!(id, 255);
        assert!(text.contains("Kicked by admin"));
        assert!(srv.players.is_empty());
        // Unknown names kick nothing and never crash.
        srv.queue_console("kick Nobody".to_string());
        srv.queue_console("help".to_string());
        srv.queue_console("frobnicate".to_string());
        srv.queue_console("list".to_string());
        srv.tick();
        srv.queue_console("save-all".to_string());
        srv.tick();
        assert!(std::path::Path::new(&format!("{}/level.dat", srv.level_dir)).exists());
        srv.queue_console("stop".to_string());
        srv.tick();
        assert!(!srv.running);
    }

    #[test]
    fn console_say_tell_tp_summon() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let (mut b, _cid) = pair(&mut srv);
        join(&mut srv, &mut b, "Alex");
        srv.queue_console("say hello".to_string());
        srv.tick();
        let want = (3, "§d[Server] hello".to_string());
        assert_eq!(pump_match(&mut a, &mut srv, &|p| p == &want), want);
        assert_eq!(pump_match(&mut b, &mut srv, &|p| p == &want), want);
        // Tell reaches only its target.
        srv.queue_console("tell Alex psst".to_string());
        srv.tick();
        let want_tell = (3, "§7CONSOLE whispers psst".to_string());
        assert_eq!(pump_match(&mut b, &mut srv, &|p| p == &want_tell), want_tell);
        srv.queue_console("tell Nobody psst".to_string());
        srv.tick();
        // Tp moves Steve onto Alex with a teleport packet.
        srv.queue_console("tp Steve Alex".to_string());
        srv.tick();
        assert_eq!(pump_match(&mut a, &mut srv, &|p| p.0 == 13).0, 13);
        let (pa, pb) = (
            match srv.world.entities.get(srv.entity_named("Steve").unwrap()).unwrap() {
                Entity::Player(p) => p.living.body.pos,
                _ => unreachable!(),
            },
            match srv.world.entities.get(srv.entity_named("Alex").unwrap()).unwrap() {
                Entity::Player(p) => p.living.body.pos,
                _ => unreachable!(),
            },
        );
        assert_eq!(pa, pb);
        srv.queue_console("tp Steve Nobody".to_string());
        srv.tick();
        // Summon drops two pigs by Steve; unknown mobs are refused.
        let pigs_before = srv
            .world
            .entities
            .alive_ids()
            .iter()
            .filter(|eid| matches!(srv.world.entities.get(**eid), Some(Entity::Animal(_))))
            .count();
        srv.queue_console("summon pig 2".to_string());
        srv.tick();
        let pigs_after = srv
            .world
            .entities
            .alive_ids()
            .iter()
            .filter(|eid| matches!(srv.world.entities.get(**eid), Some(Entity::Animal(_))))
            .count();
        assert_eq!(pigs_after, pigs_before + 2);
        srv.queue_console("summon dragon".to_string());
        srv.queue_console("summon pig 2 Nobody".to_string());
        srv.tick();
    }

    #[test]
    fn console_summon_without_players_is_safe() {
        let mut srv = mk_server("");
        srv.queue_console("summon pig".to_string());
        srv.queue_console("summon".to_string());
        srv.tick();
        assert!(srv.world.entities.alive_ids().is_empty());
    }

    #[test]
    fn duplicate_login_kicks_old_and_keeps_new() {
        let mut srv = mk_server("");
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let (mut b, _cid) = pair(&mut srv);
        b.write_all(&cli_handshake("Steve")).unwrap();
        assert_eq!(pump_until(&mut b, &mut srv).0, 2);
        b.write_all(&cli_login(6, "steve")).unwrap();
        // Old socket got the duplicate kick (leftover chunk/tracker
        // chatter may sit ahead of it in the buffer).
        let (id, text) = pump_match(&mut a, &mut srv, &|p| p.0 == 255);
        assert_eq!(id, 255);
        assert!(text.contains("another location"), "kick text: {text}");
        // ...and the new login completed (starts with the login burst).
        let mut ids = Vec::new();
        for _ in 0..8 {
            ids.push(pump_until(&mut b, &mut srv).0);
        }
        assert_eq!(ids, vec![1, 6, 13, 8, 5, 5, 5, 4]);
        assert_eq!(srv.players.len(), 1);
    }

    #[test]
    fn animal_hit_delivers_immediate_status_and_velocity() {
        let mut srv = mk_server("");
        // Build a solid platform under spawn so Steve and pig stand on ground without void/fall damage.
        for x in -5..=5 {
            for z in -5..=5 {
                srv.world.set_block_id(x, 63, z, 1);
                srv.world.set_block_id(x, 64, z, 1);
                srv.world.set_block_id(x, 65, z, 0);
                srv.world.set_block_id(x, 66, z, 0);
            }
        }

        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let player_id = srv.entity_named("Steve").unwrap();

        // Summon a pig at Steve's location.
        srv.queue_console("summon pig 1 Steve".to_string());
        srv.tick();
        srv.tick(); // tracker introduces pig to Steve (Packet24)

        let pig_id = srv
            .world
            .entities
            .alive_ids()
            .iter()
            .copied()
            .find(|eid| matches!(srv.world.entities.get(*eid), Some(Entity::Animal(_))))
            .expect("pig must exist");

        // Place Steve and pig directly on the floor at y=65.0 with clear line-of-sight.
        srv.world.entities.get_mut(player_id).unwrap().body_mut().set_position(0.5, 65.0, 0.5);
        srv.world.entities.get_mut(pig_id).unwrap().body_mut().set_position(0.5, 65.0, 1.5);

        // Drain any pending chatter (including mob spawn packet).
        drain_all(&mut a);

        // Player attacks pig (left click).
        let mut pkt7 = Vec::new();
        pkt7.push(7);
        pkt7.extend_from_slice(&player_id.to_be_bytes());
        pkt7.extend_from_slice(&pig_id.to_be_bytes());
        pkt7.push(1); // left click
        a.write_all(&pkt7).unwrap();

        // Process hit and produce responses on the exact same tick.
        srv.poll_network(Duration::from_millis(50));
        srv.tick();

        // Read all packets sent back to player on this tick.
        let mut pids = Vec::new();
        while let Some((pid, _)) = next_pkt_opt(&mut a, Duration::from_millis(50)) {
            pids.push(pid);
        }

        assert!(pids.contains(&38), "Packet38 (EntityStatus hurt) must arrive on the same tick, got: {pids:?}");
        assert!(pids.contains(&28), "Packet28 (EntityVelocity knockback) must arrive on the same tick, got: {pids:?}");
    }

    #[test]
    fn creeper_explosion_sends_packet60_and_destroy_entity() {
        let mut srv = mk_server("");
        for x in -5..=5 {
            for z in -5..=5 {
                srv.world.set_block_id(x, 63, z, 1);
                srv.world.set_block_id(x, 64, z, 1);
                srv.world.set_block_id(x, 65, z, 0);
                srv.world.set_block_id(x, 66, z, 0);
            }
        }
        let (mut a, _cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let player_id = srv.entity_named("Steve").unwrap();

        srv.queue_console("summon creeper 1 Steve".to_string());
        srv.tick();
        srv.tick(); // tracker introduces creeper

        let creeper_id = srv
            .world
            .entities
            .alive_ids()
            .iter()
            .copied()
            .find(|eid| matches!(srv.world.entities.get(*eid), Some(Entity::Mob(m)) if m.kind == MobKind::Creeper))
            .expect("creeper must exist");

        srv.world.entities.get_mut(player_id).unwrap().body_mut().set_position(0.5, 65.0, 0.5);
        srv.world.entities.get_mut(creeper_id).unwrap().body_mut().set_position(0.5, 65.0, 2.5);
        drain_all(&mut a);

        if let Some(Entity::Mob(m)) = srv.world.entities.get_mut(creeper_id) {
            m.swell_time = 29;
            m.swell_dir = 1;
            m.target = Some(player_id);
        }
        srv.tick();

        let mut pids = Vec::new();
        while let Some((pid, _)) = next_pkt_opt(&mut a, Duration::from_millis(50)) {
            pids.push(pid);
        }
        assert!(pids.contains(&60), "Packet60 (explosion) must be sent on creeper explosion, got: {pids:?}");
        assert!(pids.contains(&29), "Packet29 (destroy entity) must be sent to destroy exploded creeper, got: {pids:?}");
    }

    /// Raw accept without unwrapping: keeps the client socket alive and
    /// reports whether the server took the connection.
    fn try_pair(srv: &mut Server) -> (TcpStream, Option<ConnId>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let client = TcpStream::connect(addr).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (stream, _) = l.accept().unwrap();
        let cid = srv.accept(stream);
        (client, cid)
    }

    #[test]
    fn max_connections_parses_and_clamps() {
        let base = tmpdir("maxconn");
        let props = format!("{base}/server.properties");
        std::fs::write(&props, "max-players=3\nmax-connections=2\n").unwrap();
        let mut cfg = ServerConfig::open(&props);
        // Floor: the server must always be able to fill max-players.
        assert_eq!(load_settings(&mut cfg).max_connections, 3);
        std::fs::write(&props, "").unwrap();
        let mut cfg = ServerConfig::open(&props);
        assert_eq!(load_settings(&mut cfg).max_connections, 256);
    }

    #[test]
    fn tcp_global_cap_and_login_throttle() {
        // Global cap: with max-connections=3 the 4th TCP accept is
        // refused even though nobody joined (max-players gate is later).
        // (max-players=2 keeps the max-connections floor out of the way.)
        let mut srv = mk_server("max-players=2\nmax-connections=3\n");
        let (_c1, cid1) = try_pair(&mut srv);
        let (_c2, cid2) = try_pair(&mut srv);
        let (_c3, cid3) = try_pair(&mut srv);
        assert!(cid1.is_some() && cid2.is_some() && cid3.is_some());
        let (_c4, cid4) = try_pair(&mut srv);
        assert!(cid4.is_none());
        assert_eq!(srv.sessions.len(), 3);
    }

    #[test]
    fn login_rate_limit_blocks_burst_then_recovers() {
        // 5 rapid accepts pass; the 6th from the same IP is throttled
        // even after the sockets are gone (per-IP count is free again,
        // the window is not). A new window admits again.
        let mut srv = mk_server("");
        let mut held = Vec::new();
        for _ in 0..5 {
            let (c, cid) = try_pair(&mut srv);
            assert!(cid.is_some());
            held.push((c, cid.unwrap()));
        }
        // Free the IP slots without touching the throttle window.
        for (_, cid) in held {
            if let Some(sess) = srv.sessions.remove(&cid) {
                srv.remove_session(cid, sess);
            }
        }
        let (_c6, cid6) = try_pair(&mut srv);
        assert!(cid6.is_none());
        // Advance past the window and prune: admission recovers.
        srv.tick_count += crate::server::LOGIN_WINDOW_TICKS;
        srv.prune_login_attempts();
        let (_c7, cid7) = try_pair(&mut srv);
        assert!(cid7.is_some());
    }

    #[test]
    fn chat_does_not_leak_to_login_sockets() {
        // A connected-but-not-joined socket must not receive game chat
        // (vanilla `sendPacketToAllPlayers` has no login-phase target).
        let mut srv = mk_server("");
        let (mut a, _a_cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        // Second socket: handshake only, still in login.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let mut b = TcpStream::connect(addr).unwrap();
        b.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (stream, _) = l.accept().unwrap();
        let b_cid = srv.accept(stream).unwrap();
        assert!(!srv.is_play(b_cid));
        b.write_all(&cli_handshake("Alex")).unwrap();
        for _ in 0..5 {
            srv.tick();
        }
        drain_all(&mut b);
        // Steve chats; Alex-in-login must see nothing.
        a.write_all(&cli_chat("hello world")).unwrap();
        for _ in 0..10 {
            srv.tick();
        }
        assert!(next_pkt_opt(&mut b, Duration::from_millis(200)).is_none());
    }

    #[test]
    fn dead_idle_player_times_out_and_frees_slot() {
        let mut srv = mk_server("");
        let (mut a, cid) = pair(&mut srv);
        join(&mut srv, &mut a, "Steve");
        let pid = *srv.players.keys().next().unwrap();
        if let Some(crate::entity::table::Entity::Player(p)) = srv.world.entities.get_mut(pid) {
            p.living.health = 0;
            p.living.body.dead = true;
        }
        if let Some(sess) = srv.sessions.get_mut(&cid) {
            sess.idle = READ_TIMEOUT_TICKS - 1;
        }
        srv.tick();
        let (id, text) = next_pkt(&mut a);
        assert_eq!(id, 255);
        assert!(text.contains("Timed out"), "dead idle player must time out: {text}");
        assert!(srv.players.is_empty());
        assert!(srv.sessions.is_empty());
    }

    #[test]
    fn flush_unloaded_chunks_skips_clean_and_save_world_persists_live_entities() {
        let mut srv = mk_server("");
        // 1. Insert a clean (is_modified = false) chunk at (50, 50) into unloaded and call flush_unloaded_chunks:
        let mut clean = crate::chunk::Chunk::new(50, 50);
        clean.is_modified = false;
        srv.world.unloaded.insert((50, 50), Box::new(clean));
        srv.flush_unloaded_chunks();
        assert!(
            srv.store.get_chunk(50, 50).is_none(),
            "clean spilled chunk must not be written to LMDB store"
        );

        // 2. Insert a chunk at (20, 20), clear its is_modified flag, spawn a live item in it, and call save_world():
        let mut ch = crate::chunk::Chunk::new(20, 20);
        ch.is_modified = false;
        srv.world.chunks.insert((20, 20), Box::new(ch));
        let _item = srv.world.spawn_item_entity(264, 3, 0, 20.0 * 16.0 + 8.5, 64.0, 20.0 * 16.0 + 8.5);
        // Even if is_modified were manually cleared before save_world, save_world marks chunks with live entities dirty:
        srv.world.chunks.get_mut(&(20, 20)).unwrap().is_modified = false;
        srv.save_world();
        let blob = srv
            .store
            .get_chunk(20, 20)
            .expect("chunk with live item entity must be saved by save_world");
        let decoded = crate::persist::decode_chunk_blob(&blob, 20, 20).unwrap();
        assert_eq!(decoded.items.len(), 1, "saved chunk blob must contain the live item entity");
    }

    #[test]
    fn test_scratch_buffers_retained_across_ticks() {
        let mut srv = mk_server("");
        // Pre-reserve capacities in scratch buffers
        srv.tracker_scratch.out.reserve(64);
        let out_cap_before = srv.tracker_scratch.out.capacity();
        assert!(out_cap_before >= 64);

        srv.world.tick_ids.reserve(32);
        let tick_cap_before = srv.world.tick_ids.capacity();
        assert!(tick_cap_before >= 32);

        srv.world.pickup_items_scratch.reserve(16);
        let pickup_items_cap_before = srv.world.pickup_items_scratch.capacity();
        assert!(pickup_items_cap_before >= 16);

        srv.world.pickup_players_scratch.reserve(16);
        let pickup_players_cap_before = srv.world.pickup_players_scratch.capacity();
        assert!(pickup_players_cap_before >= 16);

        // Run multiple ticks
        for _ in 0..5 {
            srv.tick();
        }

        // Verify capacities were not thrown away or replaced with 0-capacity vecs
        assert_eq!(srv.tracker_scratch.out.capacity(), out_cap_before);
        assert_eq!(srv.world.tick_ids.capacity(), tick_cap_before);
        assert_eq!(srv.world.pickup_items_scratch.capacity(), pickup_items_cap_before);
        assert_eq!(srv.world.pickup_players_scratch.capacity(), pickup_players_cap_before);
    }


    #[test]
    fn overworld_mode_by_default() {
        let mut srv = mk_server("");
        assert_eq!(srv.world.dimension, 0);
        assert_eq!(srv.settings.dimension, 0);
        let (mut client, _cid) = pair(&mut srv);
        client.write_all(&cli_handshake("Steve")).unwrap();
        assert_eq!(pump_until(&mut client, &mut srv).0, 2);
        client.write_all(&cli_login(6, "Steve")).unwrap();
        let lp = pump_until(&mut client, &mut srv);
        assert_eq!(lp.0, 1);
        assert_eq!(lp.1, "0", "Packet1Login must send dimension 0");
    }

    #[test]
    fn hellworld_property_enables_nether_mode() {
        let mut srv = mk_server("hellworld=true\n");
        assert_eq!(srv.world.dimension, -1);
        assert_eq!(srv.settings.dimension, -1);
        assert_eq!(srv.world.calculate_skylight_subtracted(), 7);
        let (mut client, _cid) = pair(&mut srv);
        client.write_all(&cli_handshake("Steve")).unwrap();
        assert_eq!(pump_until(&mut client, &mut srv).0, 2);
        client.write_all(&cli_login(6, "Steve")).unwrap();
        let lp = pump_until(&mut client, &mut srv);
        assert_eq!(lp.0, 1);
        assert_eq!(lp.1, "-1", "Packet1Login must send dimension -1");
    }

    #[test]
    fn hellworld_disabled_by_property() {
        let mut srv = mk_server("hellworld=false\n");
        assert_eq!(srv.world.dimension, 0);
        assert_eq!(srv.settings.dimension, 0);
        let (mut client, _cid) = pair(&mut srv);
        client.write_all(&cli_handshake("Steve")).unwrap();
        assert_eq!(pump_until(&mut client, &mut srv).0, 2);
        client.write_all(&cli_login(6, "Steve")).unwrap();
        let lp = pump_until(&mut client, &mut srv);
        assert_eq!(lp.0, 1);
        assert_eq!(lp.1, "0", "Packet1Login must send dimension 0");
    }

    #[test]
    fn portal_collision_does_not_teleport_in_smp() {
        let mut srv = mk_server("");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");
        let eid = *srv.players.keys().next().unwrap();
        let (px, py, pz) = match srv.world.entities.get(eid) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2]),
            _ => panic!("player row"),
        };
        let (bx, by, bz) = (px.floor() as i32, py.floor() as i32, pz.floor() as i32);
        srv.world.set_block_id(bx, by, bz, 90);
        srv.world.set_block_id(bx, by + 1, bz, 90);
        // Dwell past the 60-tick threshold
        for _ in 0..70 {
            srv.tick();
        }
        // In vanilla Alpha 1.2.6 SMP, portals do not teleport
        assert!(srv.world.entities.get(eid).is_some(), "row must stay in overworld");
        assert_eq!(srv.world.entities.get(eid).unwrap().body().dimension, 0);
    }

    #[test]
    fn portal_frame_validation_leaves_no_orphan_obsidian() {
        let mut srv = mk_server("");
        let fx = 10;
        let iy = 64;
        let fz = 10;
        // Place stone at one of the frame positions:
        srv.world.set_block_id(fx + 2, iy + 3, fz, 1);
        let ok = srv.try_portal_frame(0, fx, iy, fz);
        assert!(!ok, "try_portal_frame should fail when frame is obstructed by stone");
        // Verify no obsidian was placed anywhere around the frame:
        for ih in -1..=2 {
            for iv in -1..=3 {
                let b = srv.world.get_block_id(fx + ih, iy + iv, fz);
                assert_ne!(b, 49, "no obsidian should be placed on failed portal frame");
            }
        }
    }

    #[test]
    fn console_summon_nether_uses_anchor_y() {
        let mut srv = mk_server("hellworld=true\n");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");
        let eid = *srv.players.keys().next().unwrap();
        // Move player into hell at Y = 40.0
        if let Some(e) = srv.world.entities.get_mut(eid) {
            e.body_mut().set_position(10.0, 40.0, 10.0);
        }

        // Summon a pigzombie targeting Steve
        srv.queue_console("summon pigzombie 1 Steve".to_string());
        srv.tick();

        let summoned = srv.world
            .entities
            .alive_ids()
            .iter()
            .filter(|id| **id != eid)
            .find_map(|id| srv.world.entities.get(*id))
            .expect("pigzombie should be spawned in hell");

        let sy = summoned.body().pos[1];
        assert_eq!(sy, 40.0, "summon in hell must use player's anchor Y, not heightmap");
        assert_eq!(summoned.body().dimension, -1);
    }

    #[test]
    fn test_respawn_in_hellworld() {
        let mut srv = mk_server("hellworld=true\n");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");
        let eid = *srv.players.keys().next().unwrap();

        // Simulate death
        if let Some(Entity::Player(ref mut pl)) = srv.world.entities.get_mut(eid) {
            pl.living.health = 0;
            pl.living.body.dead = true;
            pl.living.body.set_position(100.0, 50.0, 100.0);
        }

        // Send Respawn packet (Packet 9)
        client.write_all(&[9u8]).unwrap();

        for _ in 0..5 {
            srv.tick();
        }

        let player = srv.world.entities.get(eid).expect("Player should be alive in world");
        assert_eq!(player.body().dimension, -1, "Player dimension must be -1 (Hell)");
        assert!(!player.body().dead);
        if let Entity::Player(ref pl) = player {
            assert_eq!(pl.living.health, 20);
        }
        let sp = srv.world.spawn;
        let pos = player.body().pos;
        assert_eq!(pos[0].floor() as i32, sp[0]);
        assert_eq!(pos[2].floor() as i32, sp[2]);
    }

    #[test]
    fn test_portal_placement_requires_solid_ground() {
        let mut srv = mk_server("");
        let fx = 20;
        let iy = 64;
        let fz = 20;

        // Clear area around portal (make it all air, including under feet)
        for dx in -2..=3 {
            for dy in -2..=5 {
                for dz in -2..=2 {
                    srv.world.set_block_id(fx + dx, iy + dy, fz + dz, 0);
                }
            }
        }

        // Without solid ground under portal opening (iy - 1 is air), try_portal_frame must fail
        assert!(!srv.try_portal_frame(0, fx, iy, fz), "Portal must fail when floor is air");

        // Now place solid stone under the 4-block base (ih in -1..=2 at iy - 1)
        for ih in -1..=2 {
            srv.world.set_block_id(fx + ih, iy - 1, fz, 1);
        }

        // Now try_portal_frame should succeed
        assert!(srv.try_portal_frame(0, fx, iy, fz), "Portal should succeed with solid ground under base");
    }

    #[test]
    fn test_hellworld_block_change_broadcast_to_players() {
        let mut srv = mk_server("hellworld=true\n");
        let (mut client, _cid) = pair(&mut srv);
        join(&mut srv, &mut client, "Steve");

        let sp = srv.world.spawn;
        srv.world.apply_set_notify(sp[0], 64, sp[2], 1);
        srv.tick();

        let mut has_block_change = false;
        while let Some((id, _)) = next_pkt_opt(&mut client, Duration::from_millis(50)) {
            if id == 53 {
                has_block_change = true;
                break;
            }
        }
        assert!(has_block_change, "In hellworld=true, players in the chunk must receive block change updates!");
    }

    #[test]
    fn test_hellworld_fire_extinguish_broadcasts_to_watchers() {
        let mut srv = mk_server("hellworld=true\nspawn-protection=0\n");
        srv.ops.insert("steve".to_string());
        let (mut c1, _cid1) = pair(&mut srv);
        join(&mut srv, &mut c1, "Steve");
        let (mut c2, _cid2) = pair(&mut srv);
        join(&mut srv, &mut c2, "Alex");

        let eid = *srv.players.iter().find(|(_, &cid)| cid == _cid1).map(|(e, _)| e).unwrap();
        let steve_pos = srv.world.entities.get(eid).unwrap().body().pos;
        let (bx, by, bz) = (
            steve_pos[0].floor() as i32 + 1,
            steve_pos[1].floor() as i32 - 1,
            steve_pos[2].floor() as i32,
        );
        let fire_y = by + 1;
        // Netherrack at by, fire on top at fire_y
        srv.world.set_block_id(bx, by, bz, 87);
        srv.world.set_block_id(bx, fire_y, bz, 51);

        // Steve clicks support block with face 1 (top) to extinguish fire: Packet 14 (status = 0, x, y, z, face = 1)
        let mut dig_pkt = vec![14u8, 0];
        dig_pkt.extend_from_slice(&bx.to_be_bytes());
        dig_pkt.push(by as u8);
        dig_pkt.extend_from_slice(&bz.to_be_bytes());
        dig_pkt.push(1u8);
        c1.write_all(&dig_pkt).unwrap();

        for _ in 0..40 {
            srv.tick();
            if srv.world.get_block_id(bx, fire_y, bz) == 0 {
                break;
            }
        }

        assert_eq!(srv.world.get_block_id(bx, fire_y, bz), 0, "Fire must be extinguished in hellworld");
        assert_eq!(srv.world.get_block_id(bx, by, bz), 87, "Netherrack must not be broken");

        // Alex (watcher) must receive Packet 53 showing fire is extinguished
        let mut alex_got_bc = false;
        for _ in 0..40 {
            srv.tick();
            while let Some((id, _)) = next_pkt_opt(&mut c2, Duration::from_millis(50)) {
                if id == 53 {
                    alex_got_bc = true;
                    break;
                }
            }
            if alex_got_bc {
                break;
            }
        }
        assert!(alex_got_bc, "Watchers in hellworld must receive block change when fire is extinguished");
    }

