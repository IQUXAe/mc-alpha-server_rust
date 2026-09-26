//! Per-player chunk queue pump (mirrors the handler `tick` chunk half).
//! Split out of `server.rs`; behavior unchanged.

use std::collections::HashSet;
use crate::server::sessions::{Session, SessionState};
use crate::server::{ConnId, Server, chunk_key, chunk_of_key};
use crate::server_constants::{CHUNKS_PER_TICK, CHUNK_GEN_PER_TICK};
use crate::session::{pkt_map_chunk, pkt_pre_chunk, tile_packet};

pub struct PlayStream {
    pub sent: HashSet<i64>,
    pub queue: Vec<(i32, i32)>,
    pub last_cx: i32,
    pub last_cz: i32,
}

/// One connection: socket plus login/play state and idle accounting.
impl Server {
    /// Initial chunk queue: the view square sorted center-out (Java
    /// `PlayerManager` sends the view, not padding — the 3x3 populate
    /// neighborhood is ensured per send, not queued).
    pub(crate) fn initial_stream(pcx: i32, pcz: i32, view: i32) -> PlayStream {
        let mut queue = Vec::new();
        for cx in pcx - view..=pcx + view {
            for cz in pcz - view..=pcz + view {
                queue.push((cx, cz));
            }
        }
        queue.sort_by_key(|(cx, cz)| (cx - pcx) * (cx - pcx) + (cz - pcz) * (cz - pcz));
        PlayStream { sent: HashSet::new(), queue, last_cx: pcx, last_cz: pcz }
    }

    /// Per-tick chunk pump for one play session (mirrors the handler
    /// `tick` chunk half: unload on chunk change, queue rebuild, up to
    /// 15 sends with 3x3 population, tile packets on send).
    pub(crate) fn stream_tick(&mut self, _cid: ConnId, sess: &mut Session) {
        let (play, stream) = match &mut sess.state {
            SessionState::Play(p, s) => (p, s),
            SessionState::Login(_) => return,
        };
        let (pcx, pcz) = match self.world.entities.get(play.player) {
            Some(e) => (
                (e.body().pos[0].floor() as i32).div_euclid(16),
                (e.body().pos[2].floor() as i32).div_euclid(16),
            ),
            None => return,
        };
        let view = self.settings.view_distance;
        if stream.last_cx != pcx || stream.last_cz != pcz {
            stream.last_cx = pcx;
            stream.last_cz = pcz;
            // Unload what fell out of view (mirrors the C++ removal pass).
            let stale: Vec<i64> = stream
                .sent
                .iter()
                .copied()
                .filter(|k| {
                    let (sx, sz) = chunk_of_key(*k);
                    (sx - pcx).abs() > view || (sz - pcz).abs() > view
                })
                .collect();
            for key in stale {
                let (sx, sz) = chunk_of_key(key);
                play.outbox.push(pkt_pre_chunk(sx, sz, false));
                stream.sent.remove(&key);
                if let Some(set) = self.players_by_chunk.get_mut(&key) {
                    set.remove(&play.player);
                    if set.is_empty() {
                        self.players_by_chunk.remove(&key);
                    }
                }
            }
            // Rebuild: visible-not-sent first, then surviving queue tail.
            let gen = view + 3;
            let mut needed = Vec::new();
            for cx in pcx - gen..=pcx + gen {
                for cz in pcz - gen..=pcz + gen {
                    if (cx - pcx).abs() > view || (cz - pcz).abs() > view {
                        continue;
                    }
                    if !stream.sent.contains(&chunk_key(cx, cz)) {
                        needed.push((cx, cz));
                    }
                }
            }
            needed.sort_by_key(|(cx, cz)| (cx - pcx) * (cx - pcx) + (cz - pcz) * (cz - pcz));
            let mut queued: HashSet<i64> = needed.iter().map(|(cx, cz)| chunk_key(*cx, *cz)).collect();
            let mut merged = needed;
            for (cx, cz) in std::mem::take(&mut stream.queue) {
                let key = chunk_key(cx, cz);
                if !queued.contains(&key) && !stream.sent.contains(&key) {
                    queued.insert(key);
                    merged.push((cx, cz));
                }
            }
            stream.queue = merged;
        }

        // Drain any completed background chunk generations.
        if let Some(ref worker) = self.chunk_worker {
            while let Ok(raw) = worker.resp_rx.try_recv() {
                self.pending_chunk_gens.remove(&(raw.cx, raw.cz));
                if !self.world.has_chunk(raw.cx, raw.cz) {
                    let mut c = crate::chunk::Chunk::new(raw.cx, raw.cz);
                    let meta = [0u8; 32768];
                    c.load_arrays(&raw.blocks, &meta);
                    c.generate_skylight_map();
                    self.world.insert_chunk(c);
                }
            }
        }

        // Send within budget (mirrors the 15-chunk pass; the native
        // ensure path loads the store first, then generates). Fresh
        // generation is separately budgeted: a cold generate costs ~12 ms,
        // so unbounded ensures while exploring blow the 50 ms tick.
        // Skipped chunks stay queued for the next ticks.
        let mut sent_now = 0;
        let mut generated = 0;
        let mut i = 0;
        while i < stream.queue.len() && sent_now < CHUNKS_PER_TICK as usize {
            let (qx, qz) = stream.queue[i];
            let mut deferred = false;
            'nb: for dx in -1..=1 {
                for dz in -1..=1 {
                    let (nx, nz) = (qx + dx, qz + dz);
                    if !self.world.has_chunk(nx, nz) && !self.world.recall_chunk(nx, nz) {
                        let _ = self.world.load_chunk_from(&mut self.store, nx, nz);
                        if !self.world.has_chunk(nx, nz) {
                            if let Some(ref worker) = self.chunk_worker {
                                if !self.pending_chunk_gens.contains(&(nx, nz))
                                    && self.pending_chunk_gens.len() < 64
                                {
                                    self.pending_chunk_gens.insert((nx, nz));
                                    let _ = worker.req_tx.send((nx, nz));
                                }
                                deferred = true;
                                break 'nb;
                            } else {
                                generated += 1;
                                if generated > CHUNK_GEN_PER_TICK {
                                    deferred = true;
                                    break 'nb;
                                }
                            }
                        }
                    }
                    self.world.ensure_chunk(nx, nz);
                }
            }
            if deferred {
                i += 1;
                continue;
            }
            let populated = self
                .world
                .chunk_ref(qx, qz)
                .map(|c| c.is_terrain_populated)
                .unwrap_or(false);
            if !populated {
                i += 1;
                continue;
            }
            play.outbox.push(pkt_pre_chunk(qx, qz, true));
            if let Some(chunk) = self.world.chunk_ref(qx, qz) {
                play.outbox.push(pkt_map_chunk(
                    qx * 16,
                    0,
                    qz * 16,
                    16,
                    128,
                    16,
                    &chunk.map_compressed(),
                ));
            }
            // Tile entities ride the chunk like C++ (row order is map
            // order on both sides).
            for &(x, y, z) in self.world.tiles.chunk_cells(qx, qz) {
                if let Some(tile) = self.world.tiles.get(&(x, y, z)) {
                    play.outbox.push(tile_packet(x, y, z, tile));
                    if matches!(tile, crate::world::TileData::Furnace(_)) {
                        if let Some(sc) =
                            crate::session_packets::pkt_subchunk_block(&self.world, x, y, z)
                        {
                            play.outbox.push(sc);
                        }
                    }
                }
            }
            stream.sent.insert(chunk_key(qx, qz));
            self.players_by_chunk.entry(chunk_key(qx, qz)).or_default().insert(play.player);
            stream.queue.remove(i);
            sent_now += 1;
        }
    }
}
