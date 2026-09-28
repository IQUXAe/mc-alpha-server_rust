//! Connections and the login pump: accept, join/leave, packet fan-out.
//! Split out of `server.rs`; behavior unchanged.

use std::collections::{HashMap, HashSet};
use crate::entity::table::{Entity, PlayerEnt};
use crate::network::PacketData;
use crate::persist::ChunkStore;
use crate::server::streaming::PlayStream;
use crate::server::{ConnId, MAX_CONNECTIONS_PER_IP, MAX_PACKETS_PER_TICK, READ_TIMEOUT_TICKS, Server, chunk_key};
use crate::server::settings::{load_settings, read_list};
use crate::server_admin::admin_normalize;
use crate::server_config::ServerConfig;
use crate::server_log as log;
use crate::session::{
    pkt_arm, pkt_chat, pkt_health, pkt_kick, pkt_login_response, pkt_spawn_pos, pkt_time, Conn,
    ConnEvent, LoginEvent, LoginSession, PlaySession, SessionBroadcast, SessionCtx, SessionOutcome,
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
        let mut world = World::new(seed);
        world.difficulty = settings.difficulty;
        world.spawn_monsters = settings.spawn_monsters;
        world.spawn_animals = settings.spawn_animals;
        world.level_name = settings.level_name.clone();
        world.unload_radius = settings.view_distance + 2;
        let store =
            ChunkStore::open(&format!("{level_dir}/db")).map_err(|e| format!("cannot open chunk store: {e}"))?;
        if !world.load_level_from(level_dir) {
            Self::find_safe_spawn(&mut world);
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
        Ok(Self {
            settings,
            world,
            store,
            level_dir: level_dir.to_string(),
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
        })
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

    /// Send owned tracker bytes to the owning connections (addressed to
    /// player ids; rows gone mid-tick are dropped like C++).
    pub(crate) fn route_outbox(&mut self, out: Vec<Outbox>) {
        for o in out {
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
                    let key = chunk_key(x.div_euclid(16), z.div_euclid(16));
                    for cid in self.conns_with_chunk(key) {
                        if let Some(other) = self.sessions.get_mut(&cid) {
                            if let SessionState::Play(play, _) = &mut other.state {
                                play.send_tile(&self.world, x, y, z);
                            }
                            other.flush();
                        }
                    }
                    // ...plus the source session itself when loaded.
                    if let SessionState::Play(play, stream) = &mut sess.state {
                        if stream.sent.contains(&key) {
                            play.send_tile(&self.world, x, y, z);
                        }
                    }
                }
            }
        }
    }

    /// Connections whose player has this chunk loaded.
    pub(crate) fn conns_with_chunk(&self, key: i64) -> Vec<ConnId> {
        match self.players_by_chunk.get(&key) {
            Some(set) => set.iter().filter_map(|eid| self.players.get(eid).copied()).collect(),
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

    /// Authenticated packet leg: dispatch, keep-alive, idle timeout,
    /// streaming, then outcome handling.
    fn play_leg(&mut self, cid: ConnId, mut sess: Session, packets: Vec<PacketData>, dropped: bool) {
        let mut outcome = if dropped { Some(SessionOutcome::Gone) } else { None };
        let mut bcast = Vec::new();
        if outcome.is_none() {
            if let SessionState::Play(play, _) = &mut sess.state {
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
        if let Some(old_cid) =
            self.players.iter().find_map(|(eid, ocid)| match self.world.entities.get(*eid) {
                Some(Entity::Player(p)) if admin_normalize(&p.username) == lower => Some(*ocid),
                _ => None,
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
        // overwrites); fresh rows start at spawn.
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
        let (pos, held_saved) = match self.world.entities.get(eid) {
            Some(Entity::Player(p)) => (p.living.body.pos, p.held_item_id),
            _ => ([0.0, 64.0, 0.0], 0),
        };
        log::info(&format!("{username} [{remote}] logged in with entity id {eid}"));
        // Join chat goes to the players already online (mirrors the C++
        // broadcast before playerLoggedIn, which excludes the newcomer).
        self.broadcast_chat(format!("§e{username} joined the game."));
        self.players.insert(eid, cid);
        self.world.tracker.add(&TrackedEntity::player(eid, &username, pos, 0));
        // Initial chunk queue (mirrors `sendChunks`).
        let (pcx, pcz) = (pos[0].floor() as i32, pos[2].floor() as i32);
        let stream = Self::initial_stream(pcx.div_euclid(16), pcz.div_euclid(16), self.settings.view_distance);
        sess.state = SessionState::Play(PlaySession::new(eid), stream);
        sess.idle = 0;
        if let SessionState::Play(play, _) = &mut sess.state {
            play.outbox.push(pkt_login_response(eid, self.world.seed, self.settings.dimension));
            let sp = self.world.spawn;
            play.outbox.push(pkt_spawn_pos(sp[0], sp[1], sp[2]));
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
        sess.flush();
        self.sessions.insert(cid, sess);
    }
}
