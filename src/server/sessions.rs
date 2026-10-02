//! Connections and the login pump: accept, join/leave, packet fan-out.
//! Split out of `server.rs`; behavior unchanged.

use std::collections::{HashMap, HashSet};
use crate::entity::table::{Entity, EntityId, PlayerEnt};
use crate::network::PacketData;
use crate::persist::ChunkStore;
use crate::server::streaming::PlayStream;
use crate::server::{ChunkMapKey, ConnId, MAX_CONNECTIONS_PER_IP, MAX_PACKETS_PER_TICK, READ_TIMEOUT_TICKS, Server, chunk_map_key};
use crate::server::settings::{load_settings, read_list};
use crate::server_admin::admin_normalize;
use crate::server_config::ServerConfig;
use crate::server_log as log;
use crate::session::{
    pkt_arm, pkt_chat, pkt_health, pkt_kick, pkt_login_response, pkt_spawn_pos, pkt_time, Conn,
    ConnEvent, LoginEvent, LoginSession, PlaySession, SessionBroadcast, SessionCtx, SessionOutcome,
    tile_packet,
};
use crate::tracker::{Outbox, TrackedEntity};
use crate::world::World;

pub struct Session {
    pub conn: Conn,
    pub state: SessionState,
    pub idle: u32,
}

/// Login handshake versus authenticated play (mirrors the pending/active
/// split in `NetworkListenThread`).
pub enum SessionState {
    Login(LoginSession),
    Play(PlaySession, PlayStream),
}

/// The native server (mirrors `MinecraftServer` + friends; see module docs).
/// `remote` is `SocketAddr` text (`1.2.3.4:25565` or `[::1]:25565`);
/// brackets are stripped so IPv6 hosts match bans and per-IP limits.
fn ip_of(remote: &str) -> &str {
    if let Some(stripped) = remote.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            return &stripped[..end];
        }
        return remote;
    }
    match remote.rfind(':') {
        // Guard against a bare IPv6 literal without port (`::1` has
        // multiple colons and no brackets): only strip when there is
        // exactly one colon left.
        Some(i) if remote[i + 1..].find(':').is_none() => &remote[..i],
        _ => remote,
    }
}

impl Server {
    /// Open a server: properties, admin lists, chunk store, and the world
    /// (level.dat when present, else fresh seed + spawn search + prewarm,
    /// mirroring `initialize`).
    pub fn open(
        props_path: &str,
        level_dir: &str,
        player_dir: &str,
        ops_path: &str,
        banned_players_path: &str,
        banned_ips_path: &str,
    ) -> Result<Self, String> {
        let mut cfg = ServerConfig::open(props_path);
        let settings = load_settings(&mut cfg);
        let seed = if settings.seed == 0 {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| (d.as_nanos() as i64).wrapping_add(1))
                .unwrap_or(0x9E3779B97F4A7C15u64 as i64)
                .wrapping_add(1)
        } else {
            settings.seed
        };
        let is_hell = settings.dimension == -1;
        let mut world = if is_hell {
            World::new_hell(seed)
        } else {
            World::new(seed)
        };
        world.difficulty = settings.difficulty;
        world.spawn_monsters = settings.spawn_monsters;
        world.spawn_animals = if is_hell { false } else { settings.spawn_animals };
        world.level_name = settings.level_name.clone();
        world.unload_radius = settings.view_distance + 2;
        let store =
            ChunkStore::open(&format!("{level_dir}/db")).map_err(|e| format!("cannot open chunk store: {e}"))?;
        if !world.load_level_from(level_dir) {
            if is_hell {
                Self::find_hell_spawn(&mut world, [0, 64, 0]);
            } else {
                Self::find_safe_spawn(&mut world);
            }
            let (scx, scz) = (world.spawn[0].div_euclid(16), world.spawn[2].div_euclid(16));
            let radius = settings.view_distance;
            crate::server_log::info(&format!(
                "Preparing start region for level \"{}\"",
                settings.level_name
            ));
            let mut last_pct = 0;
            world.ensure_area_with_progress(scx, scz, radius, |done, total| {
                let pct = (done * 100) / total;
                if pct >= last_pct + 10 || done == total {
                    crate::server_log::info(&format!("Preparing spawn area: {pct}%"));
                    last_pct = pct;
                }
            });
            world.save_level_to(level_dir);
        }
        if let Err(e) = std::fs::create_dir_all(player_dir) {
            crate::server_log::warning(&format!("cannot create {player_dir}: {e}"));
        }
        let poll = mio::Poll::new().map_err(|e| format!("cannot create mio poll: {e}"))?;
        let events = mio::Events::with_capacity(1024);
        let waker = std::sync::Arc::new(
            mio::Waker::new(poll.registry(), crate::server::WAKER_TOKEN)
                .map_err(|e| format!("cannot create mio waker: {e}"))?,
        );
        let world_seed = world.seed;
        let hell_enabled = is_hell;
        let (hell, hell_store) = (None, None);
        let hell_level_dir = format!("{level_dir}/DIM-1");
        Ok(Self {
            settings,
            world,
            store,
            hell,
            hell_store,
            hell_enabled,
            level_dir: level_dir.to_string(),
            hell_level_dir,
            player_dir: player_dir.to_string(),
            ops: read_list(ops_path).into_iter().collect(),
            banned_players: read_list(banned_players_path),
            banned_ips: read_list(banned_ips_path),
            ops_path: ops_path.to_string(),
            banned_players_path: banned_players_path.to_string(),
            banned_ips_path: banned_ips_path.to_string(),
            sessions: HashMap::new(),
            players: HashMap::new(),
            players_by_chunk: HashMap::new(),
            next_conn: 1,
            ip_count: HashMap::new(),
            login_attempts: HashMap::new(),
            tick_count: 0,
            console: Vec::new(),
            running: true,
            poll,
            events,
            waker,
            listener: None,
            chunk_worker: Some(crate::server::chunk_worker::ChunkGenWorker::start(world_seed)),
            pending_chunk_gens: HashSet::new(),
            tracker_scratch: crate::server::TrackerScratch::default(),
            cids_scratch: Vec::new(),
            health_scratch: Vec::new(),
            sent_tiles_scratch: HashSet::new(),
            store_missing: HashSet::new(),
            store_missing_hell: HashSet::new(),
            chunks_generated_this_tick: 0,
        })
    }

    /// Hell spawn: scaled overworld spawn (x/8, z/8, like portal
    /// arrivals), lifted to the first 2-high air gap. Deterministic and
    /// single-chunk: a random walk like the overworld search never
    /// terminates here (the bedrock ceiling puts the heightmap at 127
    /// almost everywhere, and lava seas flood the lowlands).
    fn find_hell_spawn(world: &mut World, over_spawn: [i32; 3]) {
        let (sx, sz) = (over_spawn[0] / 8, over_spawn[2] / 8);
        world.ensure_chunk(sx.div_euclid(16), sz.div_euclid(16));
        let (x, z) = (sx.clamp(-3_000_000, 3_000_000), sz.clamp(-3_000_000, 3_000_000));
        for y in 40..=126 {
            if world.get_block_id(x, y, z) == 0
                && world.get_block_id(x, y + 1, z) == 0
                && world.is_solid(x, y - 1, z)
            {
                world.spawn = [x, y, z];
                return;
            }
        }
        // No floor found (deep lava lake): hover above the sea; the
        // portal-arrival lift below will still find air on travel.
        world.spawn = [x, 70, z];
    }

    /// Load chunk from store with negative caching (avoids repeatedly querying LevelDB for missing chunks).
    pub(crate) fn load_chunk(&mut self, cx: i32, cz: i32) -> bool {
        if self.store_missing.contains(&(cx, cz)) {
            return false;
        }
        let ok = self.world.load_chunk_from(&mut self.store, cx, cz);
        if !ok {
            if self.store_missing.len() >= 16384 {
                self.store_missing.clear();
            }
            self.store_missing.insert((cx, cz));
        }
        ok
    }

    /// Fresh-world spawn search (mirrors `findSafeSpawnPoint`: random
    /// walk over chunk columns, first grass/sand top at height >= 1;
    /// the native RNG stream stands in for the C++ mt19937).
    fn find_safe_spawn(world: &mut World) {
        let (mut sx, mut sz) = (0, 0);
        for _ in 0..10000 {
            world.ensure_chunk(sx, sz);
            let mut found = None;
            for x in 0..16 {
                for z in 0..16 {
                    let y = world.get_height_value(sx * 16 + x, sz * 16 + z);
                    if y < 1 {
                        continue;
                    }
                    let ground = world.get_block_id(sx * 16 + x, y - 1, sz * 16 + z);
                    if ground == 2 || ground == 12 {
                        found = Some([sx * 16 + x, y, sz * 16 + z]);
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            if let Some(sp) = found {
                world.spawn = sp;
                return;
            }
            sx += world.rng_next_int(3) - 1;
            sz += world.rng_next_int(3) - 1;
        }
    }

    /// Queue a console line (mirrors `addCommand`).
    pub fn queue_console(&mut self, line: String) {
        self.console.push(line);
    }

    /// Accept one socket (mirrors the accept loop gate: per-IP cap,
    /// silently dropped past it). Returns the new connection id.
    pub fn accept(&mut self, stream: std::net::TcpStream) -> Option<ConnId> {
        let conn = Conn::new(stream).ok()?;
        self.register_conn(conn)
    }

    /// Accept an already-constructed mio socket.
    pub fn accept_mio(&mut self, stream: mio::net::TcpStream) -> Option<ConnId> {
        let remote = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        let conn = Conn::from_mio(stream, remote);
        self.register_conn(conn)
    }

    fn register_conn(&mut self, conn: Conn) -> Option<ConnId> {
        // Global TCP cap (fd/memory bound). Deliberate addition — vanilla
        // gates only at join (`max-players`), leaving the 30s login window
        // open to fd exhaustion from distinct IPs.
        if self.sessions.len() >= self.settings.max_connections.max(1) as usize {
            log::info("Connection limit reached (server full at TCP level)");
            return None;
        }
        let ip = ip_of(&conn.remote).to_string();
        if self.login_rate_limited(&ip) {
            log::info(&format!("Login rate limit reached for IP {ip}"));
            return None;
        }
        let count = self.ip_count.get(&ip).copied().unwrap_or(0);
        if count >= MAX_CONNECTIONS_PER_IP {
            log::info(&format!("Connection limit reached for IP {ip}"));
            return None;
        }
        let id = self.next_conn;
        self.next_conn += 1;
        *self.ip_count.entry(ip).or_insert(0) += 1;
        // A failed poll registration used to be swallowed (`let _`),
        // leaving a ghost session that held its fd but was never polled.
        // Drop the connection and roll the IP slot back instead.
        let registered = conn.with_stream_mut(|s| {
            self.poll.registry().register(
                s,
                mio::Token(id as usize),
                mio::Interest::READABLE | mio::Interest::WRITABLE,
            )
        });
        if registered.is_err() {
            log::warning(&format!("Failed to register connection {id} with mio; dropping"));
            if let Some(n) = self.ip_count.get_mut(ip_of(&conn.remote)) {
                *n -= 1;
                if *n <= 0 {
                    self.ip_count.remove(ip_of(&conn.remote));
                }
            }
            return None;
        }
        let login = LoginSession::new(self.settings.online_mode)
            .with_auth_url(self.settings.auth_server_url.clone());
        self.sessions.insert(id, Session { conn, state: SessionState::Login(login), idle: 0 });
        Some(id)
    }

    /// Per-IP login throttle: at most `LOGIN_ATTEMPTS_MAX` accepts per
    /// `LOGIN_WINDOW_TICKS`. Sliding window on the tick clock; stale
    /// entries are pruned in `tick` so the map can't grow via IP spoofing.
    fn login_rate_limited(&mut self, ip: &str) -> bool {
        use crate::server::{LOGIN_ATTEMPTS_MAX, LOGIN_WINDOW_TICKS};
        let now = self.tick_count;
        let entry = self.login_attempts.entry(ip.to_string()).or_insert((now, 0));
        if now.wrapping_sub(entry.0) >= LOGIN_WINDOW_TICKS {
            *entry = (now, 1);
            return false;
        }
        entry.1 += 1;
        entry.1 > LOGIN_ATTEMPTS_MAX
    }

    /// Drop throttle entries outside the current window (runs every window
    /// from `tick`).
    pub(crate) fn prune_login_attempts(&mut self) {
        use crate::server::LOGIN_WINDOW_TICKS;
        let now = self.tick_count;
        self.login_attempts
            .retain(|_, (start, _)| now.wrapping_sub(*start) < LOGIN_WINDOW_TICKS);
    }

    /// Bind a mio TcpListener to addr and register with the poll loop.
    pub fn bind_listener(&mut self, addr: &str) -> Result<(), String> {
        let std_listener = std::net::TcpListener::bind(addr).map_err(|e| format!("cannot bind listener: {e}"))?;
        std_listener.set_nonblocking(true).map_err(|e| format!("cannot set nonblocking: {e}"))?;
        let mut listener = mio::net::TcpListener::from_std(std_listener);
        self.poll
            .registry()
            .register(&mut listener, crate::server::LISTENER_TOKEN, mio::Interest::READABLE)
            .map_err(|e| format!("cannot register listener with mio: {e}"))?;
        self.listener = Some(listener);
        Ok(())
    }

    /// Wake up any thread waiting in `poll_network`.
    pub fn wake(&self) {
        let _ = self.waker.wake();
    }

    /// Poll network events with a given timeout.
    pub fn poll_network(&mut self, timeout: std::time::Duration) {
        match self.poll.poll(&mut self.events, Some(timeout)) {
            Ok(()) => {}
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => return,
            Err(e) => {
                log::severe(&format!("mio poll error: {e}"));
                return;
            }
        }

        let mut accepted = Vec::new();
        for event in self.events.iter() {
            let token = event.token();
            if token == crate::server::LISTENER_TOKEN {
                if let Some(listener) = &self.listener {
                    loop {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                accepted.push(stream);
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => break,
                        }
                    }
                }
            } else if token == crate::server::WAKER_TOKEN {
                // Woken up by waker
            } else {
                let cid = token.0 as ConnId;
                if let Some(sess) = self.sessions.get(&cid) {
                    if event.is_readable() {
                        sess.conn.poll_read();
                    }
                    if event.is_writable() && sess.conn.has_pending_outbound() {
                        sess.conn.flush_outbound();
                    }
                }
            }
        }
        for s in accepted {
            self.accept_mio(s);
        }
    }

    /// Is this connection authenticated play?
    pub(crate) fn is_play(&self, cid: ConnId) -> bool {
        matches!(self.sessions.get(&cid).map(|s| &s.state), Some(SessionState::Play(_, _)))
    }
}

impl Session {
    /// Ship queued bytes to the socket.
    pub(crate) fn flush(&mut self) {
        let outbox = match &mut self.state {
            SessionState::Login(l) => &mut l.outbox,
            SessionState::Play(p, _) => &mut p.outbox,
        };
        for msg in outbox.drain(..) {
            self.conn.send(msg);
        }
    }
}

impl Server {
    /// Drop a removed session: close the socket, release its IP slot.
    pub(crate) fn remove_session(&mut self, _cid: ConnId, sess: Session) {
        let ip = ip_of(&sess.conn.remote).to_string();
        let _ = sess.conn.with_stream_mut(|s| {
            self.poll.registry().deregister(s)
        });
        sess.conn.close();
        if let Some(n) = self.ip_count.get_mut(&ip) {
            *n -= 1;
            if *n <= 0 {
                self.ip_count.remove(&ip);
            }
        }
    }

    /// Send and drain tracker bytes without dropping the caller buffer capacity.
    pub(crate) fn route_outbox_drain(&mut self, out: &mut Vec<Outbox>) {
        for o in out.drain(..) {
            if let Some(cid) = self.players.get(&o.to).copied() {
                if let Some(sess) = self.sessions.get(&cid) {
                    sess.conn.send(o.bytes);
                }
            }
        }
    }

    /// Chat to every authenticated player (mirrors `broadcastPacket`).
    pub(crate) fn broadcast_chat(&mut self, msg: String) {
        let bytes = pkt_chat(&msg);
        let cids: Vec<ConnId> = self.sessions.keys().copied().collect();
        for cid in cids {
            if self.is_play(cid) {
                if let Some(sess) = self.sessions.get(&cid) {
                    sess.conn.send(bytes.clone());
                }
            }
        }
    }

    /// Fan out one session's broadcasts (chat, swing, tile updates).
    /// Swing goes to watchers only (mirrors `swingItem` broadcast, which
    /// skips the source player); tiles go to chunk-loaded players.
    /// NOTE: the source session is held by the caller (out of the map),
    /// so chat/tiles are delivered to it directly: C++ broadcasts to
    /// every player including the source.
    fn fan_out(&mut self, sess: &mut Session, bcast: Vec<SessionBroadcast>) {
        for b in bcast {
            match b {
                SessionBroadcast::Chat(msg) => {
                    let bytes = pkt_chat(&msg);
                    let cids: Vec<ConnId> = self.sessions.keys().copied().collect();
                    // Play-only (mirrors vanilla `sendPacketToAllPlayers`:
                    // login-phase sockets have no player row and must not
                    // see game chat).
                    for cid in cids {
                        if self.is_play(cid) {
                            if let Some(other) = self.sessions.get(&cid) {
                                other.conn.send(bytes.clone());
                            }
                        }
                    }
                    sess.conn.send(bytes);
                }
                SessionBroadcast::Tell { target, text } => {
                    // Java /tell: whisper to the named player, miss note
                    // back to the sender when offline.
                    let bytes = pkt_chat(&text);
                    let mut delivered = false;
                    if let Some(eid) = self.entity_named(&target) {
                        if let Some(cid) = self.players.get(&eid).copied() {
                            if let Some(other) = self.sessions.get(&cid) {
                                other.conn.send(bytes.clone());
                                delivered = true;
                            }
                        }
                    }
                    if !delivered {
                        sess.conn.send(pkt_chat("§cThere's no player by that name online."));
                    }
                }
                SessionBroadcast::ArmSwing(eid) => {
                    let bytes = pkt_arm(eid, 1);
                    let cids: Vec<ConnId> = self
                        .world
                        .tracker
                        .watchers(eid)
                        .iter()
                        .filter_map(|pid| self.players.get(pid).copied())
                        .collect();
                    for cid in cids {
                        if let Some(other) = self.sessions.get(&cid) {
                            other.conn.send(bytes.clone());
                        }
                    }
                }
                SessionBroadcast::TileChanged(x, y, z) => {
                    // Route to the dimension that owns the player's row:
                    // portal travelers keep their eid, only the world moves.
                    let dim = match &sess.state {
                        SessionState::Play(play, _) => {
                            let eid = play.player;
                            if self.world.entities.get(eid).is_some() {
                                0
                            } else {
                                -1
                            }
                        }
                        SessionState::Login(_) => 0,
                    };
                    let key = chunk_map_key(dim, x.div_euclid(16), z.div_euclid(16));
                    // Snapshot the tile packet bytes first (world borrow
                    // ends before the sessions walk).
                    let tile_bytes = if dim == -1 {
                        self.hell.as_ref().and_then(|hell| {
                            hell.tiles.get(&(x, y, z)).map(|t| tile_packet(x, y, z, t))
                        })
                    } else {
                        self.world.tiles.get(&(x, y, z)).map(|t| tile_packet(x, y, z, t))
                    };
                    for cid in self.conns_with_chunk(key) {
                        if let Some(other) = self.sessions.get_mut(&cid) {
                            if let SessionState::Play(play, _) = &mut other.state {
                                if let Some(ref bytes) = tile_bytes {
                                    play.outbox.push(bytes.clone());
                                }
                            }
                            other.flush();
                        }
                    }
                    // ...plus the source session itself when loaded.
                    if let SessionState::Play(play, stream) = &mut sess.state {
                        if stream.sent.contains(&key) {
                            if dim == -1 {
                                if let Some(hell) = &self.hell {
                                    play.send_tile(hell, x, y, z);
                                }
                            } else {
                                play.send_tile(&self.world, x, y, z);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Connections whose player has this chunk loaded.
    pub(crate) fn conns_with_chunk(&self, key: ChunkMapKey) -> Vec<ConnId> {
        Self::conns_in_chunk(&self.players_by_chunk, &self.players, key)
    }

    /// Static fan-out over explicit maps (usable while a world is
    /// mutably borrowed elsewhere on `self`).
    pub(crate) fn conns_in_chunk(
        players_by_chunk: &HashMap<ChunkMapKey, HashSet<EntityId>>,
        players: &HashMap<EntityId, ConnId>,
        key: ChunkMapKey,
    ) -> Vec<ConnId> {
        match players_by_chunk.get(&key) {
            Some(set) => set.iter().filter_map(|eid| players.get(eid).copied()).collect(),
            None => Vec::new(),
        }
    }

    /// Pump one connection: feed inbound packets, run the session leg,
    /// flush replies, fan out, stream chunks, apply outcomes.
    pub(crate) fn pump_one(&mut self, cid: ConnId) {
        let mut sess = match self.sessions.remove(&cid) {
            Some(s) => s,
            None => return,
        };
        let events = sess.conn.drain();
        if events.is_empty() {
            sess.idle = sess.idle.saturating_add(1);
        } else {
            sess.idle = 0;
        }
        let mut packets = Vec::new();
        let mut dropped = false;
        let mut over_rate = false;
        for ev in events {
            match ev {
                ConnEvent::Packet(p) => {
                    packets.push(p);
                    if packets.len() > MAX_PACKETS_PER_TICK {
                        over_rate = true;
                        break;
                    }
                }
                ConnEvent::Dropped => {
                    dropped = true;
                    break;
                }
            }
        }
        // Rate-limit shutdown mirrors C++: the error path logs out with
        // a leave message when a player row exists behind the socket.
        if over_rate {
            if let SessionState::Play(play, _) = &sess.state {
                let (eid, held) = (play.player, play.held_id);
                sess.flush();
                self.logout(eid, held, true);
            }
            self.remove_session(cid, sess);
            return;
        }
        if let SessionState::Login(login) = &mut sess.state {
            if dropped {
                login.on_drop();
            }
            for p in packets {
                login.on_packet(p);
            }
            let event = login.poll();
            sess.flush();
            match event {
                None => {
                    self.sessions.insert(cid, sess);
                }
                Some(LoginEvent::Done) => {
                    self.remove_session(cid, sess);
                }
                Some(LoginEvent::Accepted { username }) => {
                    self.join(cid, sess, username);
                }
            }
            return;
        }
        self.play_leg(cid, sess, packets, dropped);
    }

    /// In vanilla Alpha 1.2.6 SMP, portal collision does not teleport
    /// (BlockPortal.java:134: `if (!var1.multiplayerWorld)`).
    /// The unmodded Alpha 1.2.6 client does not support dynamic dimension switching.
    fn portal_tick(&mut self, _sess: &mut Session) {}

    /// True when the entity's feet cell in `world` is a portal block.
    #[allow(dead_code)]
    fn stands_in_portal(world: &World, eid: crate::entity::table::EntityId) -> bool {
        let (fx, fy, fz) = match world.entities.get(eid) {
            Some(e) if !e.body().dead => {
                let p = e.body().pos;
                (
                    p[0].floor() as i32,
                    p[1].floor() as i32,
                    p[2].floor() as i32,
                )
            }
            _ => return false,
        };
        world.get_block_id(fx, fy, fz) == 90 || world.get_block_id(fx, fy + 1, fz) == 90
    }

    /// Search loaded chunks within 128 blocks of `(tx, tz)` in `dim` for
    /// an existing portal block (id 90), mirroring `Teleporter.java:func_4106_b`.
    /// Returns `(px, py, pz)` of the center-bottom of the closest portal found.
    #[allow(dead_code)]
    pub(crate) fn find_portal(&self, dim: i32, tx: f64, ty: f64, tz: f64) -> Option<(f64, f64, f64)> {
        let world = if dim == -1 {
            self.hell.as_ref()?
        } else {
            &self.world
        };
        let ix = tx.floor() as i32;
        let iz = tz.floor() as i32;
        let mut best = None;
        let mut best_dist_sq = f64::MAX;
        for (&(cx, cz), chunk) in &world.chunks {
            let chunk_bx = cx * 16;
            let chunk_bz = cz * 16;
            if (chunk_bx + 8 - ix).abs() > 136 || (chunk_bz + 8 - iz).abs() > 136 {
                continue;
            }
            for lx in 0..16 {
                let gx = chunk_bx + lx;
                let dx = gx as f64 + 0.5 - tx;
                for lz in 0..16 {
                    let gz = chunk_bz + lz;
                    let dz = gz as f64 + 0.5 - tz;
                    for gy in (1..127).rev() {
                        if chunk.get_block_id(lx, gy, lz) == 90 {
                            let mut base_y = gy;
                            while base_y > 1 && chunk.get_block_id(lx, base_y - 1, lz) == 90 {
                                base_y -= 1;
                            }
                            let dy = base_y as f64 + 0.5 - ty;
                            let dist_sq = dx * dx + dy * dy + dz * dz;
                            if dist_sq < best_dist_sq {
                                best_dist_sq = dist_sq;
                                best = Some((gx as f64 + 0.5, base_y as f64, gz as f64 + 0.5));
                            }
                        }
                    }
                }
            }
        }
        best
    }

    /// Attempt one 4x5 obsidian frame (opening 2x3) at `(fx, iy, fz)`.
    #[allow(dead_code)]
    pub(crate) fn try_portal_frame(&mut self, dim: i32, fx: i32, iy: i32, fz: i32) -> bool {
        if !(1..=121).contains(&iy) {
            return false;
        }
        let world: &mut World = match (dim, self.hell.as_mut()) {
            (-1, Some(h)) => h,
            (-1, None) => return false,
            _ => &mut self.world,
        };
        // 1. Ground check: the 4-block base under the portal (ih in -1..=2 at iy - 1)
        // must have solid support and not be air or lava (matching Teleporter.java).
        for ih in -1..=2 {
            let b = world.get_block_id(fx + ih, iy - 1, fz);
            if b == 0 || b == 10 || b == 11 || (!world.is_solid(fx + ih, iy - 1, fz) && b != 49) {
                return false;
            }
        }
        // 2. Interior check: 2 wide x 3 high opening at feet (iv in 0..=2) must be air or portal.
        for ih in 0..=1 {
            for iv in 0..=2 {
                let b = world.get_block_id(fx + ih, iy + iv, fz);
                if b != 0 && b != 90 {
                    return false;
                }
            }
        }
        // 3. Frame sides and top (iv in 0..=3) must be air or existing obsidian:
        // - Sides: ih == -1 || ih == 2, iv in 0..=2
        // - Lintel: ih in -1..=2, iv == 3
        for ih in -1..=2 {
            for iv in 0..=3 {
                let is_frame = ih == -1 || ih == 2 || iv == 3;
                if is_frame {
                    let (bx, by, bz) = (fx + ih, iy + iv, fz);
                    if !(0..128).contains(&by) {
                        return false;
                    }
                    let b = world.get_block_id(bx, by, bz);
                    if b != 0 && b != 49 {
                        return false;
                    }
                }
            }
        }
        // 4. Build 4-wide x 5-tall frame (bottom base at iy - 1 doubles as floor).
        for ih in -1..=2 {
            for iv in -1..=3 {
                let is_frame = ih == -1 || ih == 2 || iv == -1 || iv == 3;
                let (bx, by, bz) = (fx + ih, iy + iv, fz);
                if is_frame {
                    if world.get_block_id(bx, by, bz) != 49 {
                        world.apply_set_notify(bx, by, bz, 49);
                    }
                } else if world.get_block_id(bx, by, bz) != 90 {
                    world.apply_set_notify(bx, by, bz, 90);
                }
            }
        }
        true
    }

    /// Force-build a 4x5 obsidian frame with a clearance safety platform,
    /// matching vanilla Teleporter.java fallback when no natural opening fits.
    #[allow(dead_code)]
    pub(crate) fn force_portal_frame(&mut self, dim: i32, fx: i32, iy: i32, fz: i32) -> bool {
        if !(1..=121).contains(&iy) {
            return false;
        }
        let world: &mut World = match (dim, self.hell.as_mut()) {
            (-1, Some(h)) => h,
            (-1, None) => return false,
            _ => &mut self.world,
        };
        // 1. Safety floor: 4-wide x 3-deep obsidian platform under portal feet
        for ih in -1..=2 {
            for id in -1..=1 {
                world.apply_set_notify(fx + ih, iy - 1, fz + id, 49);
            }
        }
        // 2. Clear clearance space: 1 block in front and behind
        for ih in 0..2 {
            for iv in 0..=2 {
                for id in [-1, 1] {
                    world.apply_set_notify(fx + ih, iy + iv, fz + id, 0);
                }
            }
        }
        // 3. Build 4x5 obsidian frame with 2x3 portal opening
        for ih in -1..=2 {
            for iv in -1..=3 {
                let is_frame = ih == -1 || ih == 2 || iv == -1 || iv == 3;
                let (bx, by, bz) = (fx + ih, iy + iv, fz);
                if is_frame {
                    world.apply_set_notify(bx, by, bz, 49);
                } else {
                    world.apply_set_notify(bx, by, bz, 90);
                }
            }
        }
        true
    }

    /// Build a return portal frame at/near the arrival column. Tries natural
    /// placements on ground across horizontal offsets, falling back to
    /// forced placement with a carved clearance platform matching vanilla
    /// Teleporter.java:func_4108_c.
    #[allow(dead_code)]
    fn build_return_portal(&mut self, dim: i32, ix: i32, iy0: i32, iz: i32) -> Option<(f64, f64, f64)> {
        let offsets = [
            (0, 0), (2, 0), (-2, 0), (0, 2), (0, -2),
            (4, 0), (-4, 0), (0, 4), (0, -4),
            (8, 0), (-8, 0), (0, 8), (0, -8),
        ];
        if dim == 0 {
            // Overworld destination: ground is at get_height_value(fx, fz).
            // Natural placement stands directly on the surface (grass/dirt/stone).
            for (ox, oz) in offsets {
                let (fx, fz) = (ix + ox, iz + oz);
                let surface_y = self.world.get_height_value(fx, fz).clamp(10, 115);
                if self.try_portal_frame(0, fx, surface_y, fz) {
                    return Some((fx as f64 + 0.5, surface_y as f64, fz as f64 + 0.5));
                }
            }
            // Fallback: force-build on the surface at (ix, iz)
            let surface_y = self.world.get_height_value(ix, iz).clamp(10, 115);
            if self.force_portal_frame(0, ix, surface_y, iz) {
                return Some((ix as f64 + 0.5, surface_y as f64, iz as f64 + 0.5));
            }
        } else {
            // Nether destination: scan down from cavern ceiling (115) down to
            // above the lava sea (32) to locate a solid floor.
            for (ox, oz) in offsets {
                let (fx, fz) = (ix + ox, iz + oz);
                let candidates: Vec<i32> = if let Some(hell) = self.hell.as_ref() {
                    let mut cands = Vec::new();
                    for y in (32..=115).rev() {
                        let under = hell.get_block_id(fx, y - 1, fz);
                        if under != 0 && under != 10 && under != 11 && hell.is_solid(fx, y - 1, fz) {
                            cands.push(y);
                        }
                    }
                    cands
                } else {
                    return None;
                };
                for y in candidates {
                    if self.try_portal_frame(-1, fx, y, fz) {
                        return Some((fx as f64 + 0.5, y as f64, fz as f64 + 0.5));
                    }
                }
            }
            // Fallback: scan near iy0 for any solid floor with air
            let mut fallback_y = 32;
            let start = iy0.clamp(32, 110);
            if let Some(hell) = self.hell.as_ref() {
                for y in (32..=start).rev() {
                    let under = hell.get_block_id(ix, y - 1, iz);
                    if under != 0 && under != 10 && under != 11 && hell.is_solid(ix, y - 1, iz) {
                        fallback_y = y;
                        break;
                    }
                }
            }
            if self.force_portal_frame(-1, ix, fallback_y, iz) {
                return Some((ix as f64 + 0.5, fallback_y as f64, iz as f64 + 0.5));
            }
        }
        None
    }

    /// Move a player row across dimensions, preserving the entity id.
    /// Target coords scale 1:8 (overworld->hell divides), Y clamped and
    /// lifted to the first free 2-high gap; the stream is rebuilt so the
    /// new dimension re-streams from scratch next ticks.
    #[allow(dead_code)]
    fn switch_dimension(
        &mut self,
        sess: &mut Session,
        eid: crate::entity::table::EntityId,
        dim: i32,
    ) {
        if !self.hell_enabled || self.hell.is_none() {
            return;
        }
        let target_dim = if dim == -1 { 0 } else { -1 };
        // Snapshot source row.
        let (pos, yaw, pitch) = {
            let src = if dim == -1 {
                match self.hell.as_ref() {
                    Some(h) => h,
                    None => return,
                }
            } else {
                &self.world
            };
            match src.entities.get(eid) {
                Some(e) => (e.body().pos, e.body().yaw, e.body().pitch),
                None => return,
            }
        };
        let scale = crate::server::NETHER_SCALE;
        let (mut tx, mut ty, mut tz) = if target_dim == -1 {
            (pos[0] / scale, pos[1], pos[2] / scale)
        } else {
            (pos[0] * scale, pos[1], pos[2] * scale)
        };
        tx = tx.clamp(-3.0e7, 3.0e7);
        tz = tz.clamp(-3.0e7, 3.0e7);
        ty = ty.clamp(1.0, 126.0);
        // Remove from source first (id-preserving move).
        let mut row = if dim == -1 {
            match self.hell.as_mut() {
                Some(h) => match h.entities.remove(eid) {
                    Some(r) => r,
                    None => return,
                },
                None => return,
            }
        } else {
            match self.world.entities.remove(eid) {
                Some(r) => r,
                None => return,
            }
        };
        // Tracker destroy in the old world so watchers drop the ghost.
        {
            let mut out = Vec::new();
            if dim == -1 {
                if let Some(h) = self.hell.as_mut() {
                    h.tracker.remove(eid, &mut out);
                }
            } else {
                self.world.tracker.remove(eid, &mut out);
            }
            self.route_outbox_drain(&mut out);
        }
        // 1. Ensure 3x3 chunks around arrival are loaded/generated in the target world.
        let (tcx, tcz) = (
            (tx.floor() as i32).div_euclid(16),
            (tz.floor() as i32).div_euclid(16),
        );
        for dcx in -1..=1 {
            for dcz in -1..=1 {
                self.ensure_chunk_dim(target_dim, tcx + dcx, tcz + dcz);
            }
        }
        // 2. Link to an existing portal in target dimension if one exists within
        // radius 128, matching Teleporter.java. Otherwise, build a return portal.
        let arrival_pos = if let Some(existing) = self.find_portal(target_dim, tx, ty, tz) {
            existing
        } else {
            let (ix, iy0, iz) = (tx.floor() as i32, ty.floor() as i32, tz.floor() as i32);
            self.build_return_portal(target_dim, ix, iy0, iz)
                .unwrap_or((tx, ty, tz))
        };
        tx = arrival_pos.0;
        ty = arrival_pos.1;
        tz = arrival_pos.2;
        row.body_mut().set_position(tx, ty, tz);
        row.body_mut().yaw = yaw;
        row.body_mut().pitch = pitch;
        row.body_mut().motion = [0.0; 3];
        row.body_mut().fall_distance = 0.0;
        row.body_mut().dimension = target_dim;
        // Insert preserving the id; keep the allocator ahead.
        if target_dim == -1 {
            if let Some(h) = self.hell.as_mut() {
                h.entities.reserve_id(eid);
                h.entities.insert(row);
            }
        } else {
            self.world.entities.reserve_id(eid);
            self.world.entities.insert(row);
        }
        // Rebuild the stream: unload everything, queue the new view.
        if let SessionState::Play(play, stream) = &mut sess.state {
            for key in stream.sent.iter().copied().collect::<Vec<_>>() {
                play.outbox.push(crate::session::pkt_pre_chunk(key.1, key.2, false));
                if let Some(set) = self.players_by_chunk.get_mut(&key) {
                    set.remove(&eid);
                    if set.is_empty() {
                        self.players_by_chunk.remove(&key);
                    }
                }
            }
            stream.sent.clear();
            stream.queue.clear();
            stream.dim = target_dim;
            let (pcx, pcz) = ((tx.floor() as i32).div_euclid(16), (tz.floor() as i32).div_euclid(16));
            *stream = Self::initial_stream(pcx, pcz, self.settings.view_distance, target_dim);
            // Preserve the play row's other state across the struct swap.
            play.portal_ticks = 0;
            play.travel_cooldown = crate::server::PORTAL_COOLDOWN_TICKS;
            play.last = [tx, ty, tz];
            play.has_moved = false;
            play.teleport_wait = Some([tx, ty, tz]);
            // In Alpha 1.2.6 SMP, we must NOT send Packet1Login during portal travel:
            // NetClientHandler.handleLogin unconditionally displays GuiDownloadTerrain,
            // but field_1210_g is already true from the initial connection, so
            // handleFlying never dismisses it, permanently freezing the client on
            // "Downloading terrain". Omitting Packet1Login lets the client smoothly
            // receive chunk unloads, teleport, and new dimension chunks without getting stuck.
            let tspawn = if target_dim == -1 {
                self.hell.as_ref().map(|h| h.spawn).unwrap_or(self.world.spawn)
            } else {
                self.world.spawn
            };
            play.outbox.push(crate::session::pkt_spawn_pos(tspawn[0], tspawn[1], tspawn[2]));
            play.outbox.push(crate::session::pkt_teleport(tx, ty, tz, yaw, pitch));
            let hp = if target_dim == -1 {
                self.hell.as_ref().and_then(|h| h.entities.get(eid)).map(|e| match e {
                    crate::entity::table::Entity::Player(p) => p.living.health as i8,
                    _ => 20,
                }).unwrap_or(20)
            } else {
                self.world.entities.get(eid).map(|e| match e {
                    crate::entity::table::Entity::Player(p) => p.living.health as i8,
                    _ => 20,
                }).unwrap_or(20)
            };
            play.outbox.push(crate::session::pkt_health(hp));
            play.last_health = hp;
            if target_dim == -1 {
                if let Some(h) = &self.hell {
                    play.send_inventory(h);
                }
            } else {
                play.send_inventory(&self.world);
            }
            play.outbox.push(crate::session::pkt_time(self.world.time));
            crate::server_log::info(&format!(
                "Player {eid} traveled {dim}->{target_dim} ({:.1},{:.1},{:.1})->({tx:.1},{ty:.1},{tz:.1})",
                pos[0], pos[1], pos[2]
            ));
        }
    }

    /// Respawn a player who died in the Nether back to the Overworld spawn point.
    /// Moves the player entity from hell to world, cleans up the Nether tracker and chunks,
    /// restores full health, and streams Overworld spawn chunks.
    pub(crate) fn respawn_hell_to_overworld(
        &mut self,
        sess: &mut Session,
        eid: crate::entity::table::EntityId,
    ) {
        let mut row = match self.hell.as_mut().and_then(|h| h.entities.remove(eid)) {
            Some(r) => r,
            None => return,
        };
        // Remove from hell tracker so Nether players drop the entity.
        {
            let mut out = Vec::new();
            if let Some(h) = self.hell.as_mut() {
                h.tracker.remove(eid, &mut out);
            }
            self.route_outbox_drain(&mut out);
        }
        let username = match &row {
            crate::entity::table::Entity::Player(p) => p.username.clone(),
            _ => String::new(),
        };
        // Reset player vital stats for fresh spawn.
        if let crate::entity::table::Entity::Player(p) = &mut row {
            p.living.body.dead = false;
            p.living.health = p.living.max_health;
            p.living.hurt_time = 0;
            p.living.death_time = 0;
            p.living.body.fire = 0;
            p.living.body.air = 300;
            p.living.body.fall_distance = 0.0;
            p.living.body.motion = [0.0; 3];
            p.living.body.dimension = 0;
            p.respawn_ticks = 60;
        }
        let sp = self.world.spawn;
        let (sx, mut sy, sz) = (sp[0] as f64 + 0.5, sp[1] as f64, sp[2] as f64 + 0.5);
        row.body_mut().set_position(sx, sy, sz);
        row.body_mut().yaw = 0.0;
        row.body_mut().pitch = 0.0;
        row.body_mut().dimension = 0;

        while !self.world.colliding_boxes(&row.body().bounding_box).is_empty()
            && sy < crate::world::WORLD_HEIGHT as f64
        {
            sy += 1.0;
            row.body_mut().set_position(sx, sy, sz);
        }

        // Ensure 3x3 chunks around Overworld spawn are loaded/generated.
        let (scx, scz) = (
            (sx.floor() as i32).div_euclid(16),
            (sz.floor() as i32).div_euclid(16),
        );
        for dcx in -1..=1 {
            for dcz in -1..=1 {
                self.ensure_chunk_dim(0, scx + dcx, scz + dcz);
            }
        }

        // Insert into Overworld entities preserving the ID, and add to tracker.
        self.world.entities.reserve_id(eid);
        self.world.entities.insert(row);
        self.world.tracker.add(&crate::tracker::TrackedEntity::player(
            eid,
            &username,
            [sx, sy, sz],
            0,
        ));

        // Rebuild the chunk stream from Nether (-1) to Overworld (0).
        if let SessionState::Play(play, stream) = &mut sess.state {
            for key in stream.sent.iter().copied().collect::<Vec<_>>() {
                play.outbox.push(crate::session::pkt_pre_chunk(key.1, key.2, false));
                if let Some(set) = self.players_by_chunk.get_mut(&key) {
                    set.remove(&eid);
                    if set.is_empty() {
                        self.players_by_chunk.remove(&key);
                    }
                }
            }
            stream.sent.clear();
            stream.queue.clear();
            stream.dim = 0;
            *stream = Self::initial_stream(scx, scz, self.settings.view_distance, 0);

            play.portal_ticks = 0;
            play.travel_cooldown = crate::server::PORTAL_COOLDOWN_TICKS;
            play.last = [sx, sy, sz];
            play.has_moved = false;
            play.teleport_wait = Some([sx, sy, sz]);
            play.outbox.push(crate::session::pkt_respawn());
            play.outbox.push(crate::session::pkt_spawn_pos(sp[0], sp[1], sp[2]));
            play.outbox.push(crate::session::pkt_teleport(sx, sy, sz, 0.0, 0.0));
            play.outbox.push(crate::session::pkt_health(20));
            play.last_health = 20;
            play.send_inventory(&self.world);
            play.outbox.push(crate::session::pkt_time(self.world.time));
            crate::server_log::info(&format!(
                "Player {eid} respawned from Nether to Overworld at ({sx:.1},{sy:.1},{sz:.1})"
            ));
        }
    }

    /// Authenticated packet leg: dispatch, keep-alive, idle timeout,
    /// streaming, then outcome handling. Dim-aware: the packet handlers
    /// run against the world that owns the player row.
    fn play_leg(&mut self, cid: ConnId, mut sess: Session, packets: Vec<PacketData>, dropped: bool) {
        let mut outcome = if dropped { Some(SessionOutcome::Gone) } else { None };
        let mut bcast = Vec::new();
        let mut hell_respawn_eid = None;
        if outcome.is_none() {
            if let SessionState::Play(play, _) = &mut sess.state {
                let dim = if self.hell.is_some() {
                    if self.world.entities.get(play.player).is_some() { 0 } else { -1 }
                } else {
                    self.world.dimension as i32
                };
                if dim == -1 && self.hell.is_none() && self.world.dimension != -1 {
                    // Hell disabled mid-session: fall back to overworld row.
                    outcome = Some(SessionOutcome::Gone);
                } else if dim == -1 && self.hell.is_some() {
                    let hell = self.hell.as_mut().unwrap();
                    let mut ctx = SessionCtx {
                        world: hell,
                        ops: &self.ops,
                        spawn_protection: self.settings.spawn_protection,
                        pvp: self.settings.pvp,
                        broadcast: &mut bcast,
                    };
                    for p in packets {
                        if matches!(p, PacketData::Respawn) {
                            let is_dead = ctx.world.entities.get(play.player).map(|e| match e {
                                crate::entity::table::Entity::Player(pl) => pl.living.health <= 0,
                                _ => false,
                            }).unwrap_or(false);
                            if is_dead {
                                hell_respawn_eid = Some(play.player);
                                continue;
                            }
                        }
                        if let Some(o) = play.pump(&mut ctx, p) {
                            outcome = Some(o);
                            break;
                        }
                    }
                    if outcome.is_none() && hell_respawn_eid.is_none() {
                        let hell = self.hell.as_mut().unwrap();
                        let mut ctx = SessionCtx {
                            world: hell,
                            ops: &self.ops,
                            spawn_protection: self.settings.spawn_protection,
                            pvp: self.settings.pvp,
                            broadcast: &mut bcast,
                        };
                        play.tick(&mut ctx);
                    }
                } else {
                    let mut ctx = SessionCtx {
                        world: &mut self.world,
                        ops: &self.ops,
                        spawn_protection: self.settings.spawn_protection,
                        pvp: self.settings.pvp,
                        broadcast: &mut bcast,
                    };
                    for p in packets {
                        if let Some(o) = play.pump(&mut ctx, p) {
                            outcome = Some(o);
                            break;
                        }
                    }
                    if outcome.is_none() {
                        let mut ctx = SessionCtx {
                            world: &mut self.world,
                            ops: &self.ops,
                            spawn_protection: self.settings.spawn_protection,
                            pvp: self.settings.pvp,
                            broadcast: &mut bcast,
                        };
                        play.tick(&mut ctx);
                    }
                }
            }
        }
        if let Some(eid) = hell_respawn_eid {
            self.respawn_hell_to_overworld(&mut sess, eid);
        }
        // Idle timeout mirrors the read timeout (applies to all idle sessions,
        // including players sitting on the death screen without sending packets).
        if outcome.is_none() && sess.idle >= READ_TIMEOUT_TICKS {
            if let SessionState::Play(play, _) = &mut sess.state {
                play.outbox.push(pkt_kick("Timed out"));
            }
            outcome = Some(SessionOutcome::Gone);
        }
        sess.flush();
        self.fan_out(&mut sess, bcast);
        let still_play = matches!(sess.state, SessionState::Play(_, _));
        if still_play && outcome.is_none() {
            // Portal dwell check (custom travel; vanilla Alpha servers
            // never travel — the portal collision is singleplayer-only).
            // Runs before streaming so the new dimension streams this tick.
            self.portal_tick(&mut sess);
            self.stream_tick(cid, &mut sess);
            sess.flush();
        }
        match outcome {
            None => {
                self.sessions.insert(cid, sess);
            }
            // Session kicks carry their reason in the flushed bytes;
            // admin kicks never print a leave message (mirrors C++).
            Some(SessionOutcome::Kick(_)) => {
                if let SessionState::Play(play, _) = &sess.state {
                    let (eid, held) = (play.player, play.held_id);
                    self.logout(eid, held, false);
                }
                self.remove_session(cid, sess);
            }
            Some(SessionOutcome::Gone) => {
                if let SessionState::Play(play, _) = &sess.state {
                    let (eid, held) = (play.player, play.held_id);
                    self.logout(eid, held, true);
                }
                self.remove_session(cid, sess);
            }
        }
    }
}

impl Server {
    /// Kick a not-yet-joined login (mirrors `kickUser`): kick bytes,
    /// socket close, entry drop.
    fn drop_login(&mut self, cid: ConnId, mut sess: Session, username: &str, remote: &str, reason: &str) {
        log::info(&format!("Disconnecting {username} [{remote}]: {reason}"));
        if let SessionState::Login(login) = &mut sess.state {
            login.outbox.push(pkt_kick(reason));
        }
        sess.flush();
        self.remove_session(cid, sess);
    }

    /// Complete a login (mirrors `configManager->login` + `doLogin`):
    /// validate, ban/full/duplicate gates, row load-or-create, tracker
    /// add, and the join packet sequence. Failures kick and drop.
    fn join(&mut self, cid: ConnId, mut sess: Session, username: String) {
        let remote = sess.conn.remote.clone();
        if username.is_empty() || username.len() > 16 {
            self.drop_login(cid, sess, &username, &remote, "Invalid username length");
            return;
        }
        if !username.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.drop_login(cid, sess, &username, &remote, "Invalid username characters");
            return;
        }
        let lower = admin_normalize(&username);
        if self.banned_players.contains(&lower) {
            self.drop_login(cid, sess, &username, &remote, "You are banned from this server!");
            return;
        }
        if self.banned_ips.contains(ip_of(&remote)) {
            self.drop_login(
                cid,
                sess,
                &username,
                &remote,
                "Your IP address is banned from this server!",
            );
            return;
        }
        if self.players.len() >= self.settings.max_players as usize {
            self.drop_login(cid, sess, &username, &remote, "The server is full!");
            return;
        }
        // Duplicate login: the old session is fully logged out (save +
        // tracker destroy, no leave chat) and its socket closed. C++
        // deletes the row without saving and ghosts it on watchers.
        // Dim-aware: rows live in exactly one world.
        if let Some(old_cid) =
            self.players.iter().find_map(|(eid, ocid)| {
                let hit = match self.world.entities.get(*eid) {
                    Some(Entity::Player(p)) if admin_normalize(&p.username) == lower => true,
                    _ => matches!(
                        self.hell.as_ref().and_then(|h| h.entities.get(*eid)),
                        Some(Entity::Player(p)) if admin_normalize(&p.username) == lower
                    ),
                };
                if hit { Some(*ocid) } else { None }
            })
        {
            if let Some(mut old) = self.sessions.remove(&old_cid) {
                if let SessionState::Play(play, _) = &mut old.state {
                    play.outbox.push(pkt_kick("You logged in from another location"));
                    let (eid, held) = (play.player, play.held_id);
                    old.flush();
                    log::info(&format!(
                        "Disconnecting {username}: You logged in from another location"
                    ));
                    self.logout(eid, held, false);
                }
                self.remove_session(old_cid, old);
            }
        }
        // Row: saved position wins like C++ (spawn set, then the file
        // overwrites); fresh rows start at spawn. Saved dimension wins:
        // hell logouts rejoin in hell (overworld otherwise).
        let eid = match self.world.load_player_from(&self.player_dir, &username) {
            Some(id) => id,
            None => {
                let id = self.world.entities.alloc_id();
                let mut p = PlayerEnt::new(id, &username);
                let sp = self.world.spawn;
                p.living.body.set_position(sp[0] as f64 + 0.5, sp[1] as f64, sp[2] as f64 + 0.5);
                while !self.world.colliding_boxes(&p.living.body.bounding_box).is_empty()
                    && p.living.body.pos[1] < crate::world::WORLD_HEIGHT as f64
                {
                    let cur_y = p.living.body.pos[1];
                    p.living.body.set_position(p.living.body.pos[0], cur_y + 1.0, p.living.body.pos[2]);
                }
                self.world.entities.insert(Entity::Player(p));
                id
            }
        };
        // Route the row to its saved dimension (hell rows move over if secondary
        // hell world exists, keeping the id; the allocator is kept ahead).
        let dim = if self.hell.is_some() {
            let saved = self.world.entities.get(eid).map(|e| e.body().dimension).unwrap_or(0);
            if saved == -1 {
                if let Some(row) = self.world.entities.remove(eid) {
                    if let Some(h) = self.hell.as_mut() {
                        h.entities.reserve_id(eid);
                        h.entities.insert(row);
                    } else {
                        self.world.entities.insert(row);
                    }
                }
                -1
            } else {
                0
            }
        } else {
            let d = self.world.dimension as i32;
            if let Some(Entity::Player(p)) = self.world.entities.get_mut(eid) {
                p.living.body.dimension = self.world.dimension;
            }
            d
        };
        let (pos, held_saved) = if dim == -1 && self.hell.is_some() {
            match self.hell.as_ref().and_then(|h| h.entities.get(eid)) {
                Some(Entity::Player(p)) => (p.living.body.pos, p.held_item_id),
                _ => ([0.0, 64.0, 0.0], 0),
            }
        } else {
            match self.world.entities.get(eid) {
                Some(Entity::Player(p)) => (p.living.body.pos, p.held_item_id),
                _ => ([0.0, 64.0, 0.0], 0),
            }
        };
        log::info(&format!("{username} [{remote}] logged in with entity id {eid}"));
        // Join chat goes to the players already online (mirrors the C++
        // broadcast before playerLoggedIn, which excludes the newcomer).
        self.broadcast_chat(format!("§e{username} joined the game."));
        self.players.insert(eid, cid);
        if dim == -1 && self.hell.is_some() {
            if let Some(h) = self.hell.as_mut() {
                h.tracker.add(&TrackedEntity::player(eid, &username, pos, 0));
            }
        } else {
            self.world.tracker.add(&TrackedEntity::player(eid, &username, pos, 0));
        }
        // Initial chunk queue (mirrors `sendChunks`).
        let (pcx, pcz) = (pos[0].floor() as i32, pos[2].floor() as i32);
        let stream = Self::initial_stream(pcx.div_euclid(16), pcz.div_euclid(16), self.settings.view_distance, dim);
        sess.state = SessionState::Play(PlaySession::new(eid), stream);
        sess.idle = 0;
        if let SessionState::Play(play, _) = &mut sess.state {
            // Packet1Login sends dimension (-1 for Nether, 0 for Overworld)
            play.outbox.push(pkt_login_response(eid, self.world.seed, dim as i8));
            let sp = if dim == -1 && self.hell.is_some() {
                self.hell.as_ref().map(|h| h.spawn).unwrap_or(self.world.spawn)
            } else {
                self.world.spawn
            };
            play.outbox.push(pkt_spawn_pos(sp[0], sp[1], sp[2]));
            if dim == -1 && self.hell.is_some() {
                if let Some(h) = self.hell.as_mut() {
                    if held_saved > 0 {
                        play.restore_held(h, held_saved);
                    }
                    let (ppos, yaw, pitch) = match h.entities.get(eid) {
                        Some(e) => (e.body().pos, e.body().yaw, e.body().pitch),
                        None => (pos, 0.0, 0.0),
                    };
                    play.teleport_to(h, eid, ppos, yaw, pitch);
                    let hp = match h.entities.get(eid) {
                        Some(Entity::Player(p)) => p.living.health as i8,
                        _ => 20,
                    };
                    play.outbox.push(pkt_health(hp));
                    play.last_health = hp;
                    play.send_inventory(h);
                    play.outbox.push(pkt_time(self.world.time));
                }
            } else {
                if held_saved > 0 {
                    play.restore_held(&mut self.world, held_saved);
                }
                let (ppos, yaw, pitch) = match self.world.entities.get(eid) {
                    Some(e) => (e.body().pos, e.body().yaw, e.body().pitch),
                    None => (pos, 0.0, 0.0),
                };
                play.teleport_to(&mut self.world, eid, ppos, yaw, pitch);
                let hp = match self.world.entities.get(eid) {
                    Some(Entity::Player(p)) => p.living.health as i8,
                    _ => 20,
                };
                play.outbox.push(pkt_health(hp));
                play.last_health = hp;
                play.send_inventory(&self.world);
                play.outbox.push(pkt_time(self.world.time));
            }
        }
        sess.flush();
        self.sessions.insert(cid, sess);
    }
}
