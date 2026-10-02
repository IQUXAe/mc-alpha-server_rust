//! Per-player chunk queue pump (mirrors the handler `tick` chunk half).
//! Split out of `server.rs`; behavior unchanged.
//!
//! Dual-dimension: each [`PlayStream`] is pinned to one dimension
//! (`dim` 0/-1); chunk keys carry the dimension so overworld and hell
//! sets never alias. Portal travel rebuilds the stream (see
//! `Server::switch_dimension`).

use std::collections::HashSet;
use crate::server::sessions::{Session, SessionState};
use crate::server::{ChunkMapKey, ConnId, Server, chunk_map_key};
use crate::server_constants::{CHUNKS_PER_TICK, CHUNK_GEN_PER_TICK};
use crate::session::{pkt_map_chunk, pkt_pre_chunk, tile_packet};

pub struct PlayStream {
    pub sent: HashSet<ChunkMapKey>,
    pub queue: Vec<(i32, i32)>,
    pub last_cx: i32,
    pub last_cz: i32,
    /// Dimension this stream serves (0 overworld, -1 hell).
    pub dim: i32,
}

/// One connection: socket plus login/play state and idle accounting.
impl Server {
    /// Initial chunk queue: the view square sorted center-out (Java
    /// `PlayerManager` sends the view, not padding — the 3x3 populate
    /// neighborhood is ensured per send, not queued).
    pub(crate) fn initial_stream(pcx: i32, pcz: i32, view: i32, dim: i32) -> PlayStream {
        let mut queue = Vec::new();
        for cx in pcx - view..=pcx + view {
            for cz in pcz - view..=pcz + view {
                queue.push((cx, cz));
            }
        }
        queue.sort_by_key(|(cx, cz)| (cx - pcx) * (cx - pcx) + (cz - pcz) * (cz - pcz));
        PlayStream { sent: HashSet::new(), queue, last_cx: pcx, last_cz: pcz, dim }
    }

    /// Load chunk from the dimension store with negative caching (avoids
    /// repeatedly querying LevelDB for missing chunks).
    pub(crate) fn load_chunk_dim(&mut self, dim: i32, cx: i32, cz: i32) -> bool {
        if dim == -1 && self.hell.is_some() {
            let Some(hell) = self.hell.as_mut() else { return false };
            let Some(store) = self.hell_store.as_mut() else { return false };
            if self.store_missing_hell.contains(&(cx, cz)) {
                return false;
            }
            let ok = hell.load_chunk_from(store, cx, cz);
            if !ok {
                if self.store_missing_hell.len() >= 16384 {
                    self.store_missing_hell.clear();
                }
                self.store_missing_hell.insert((cx, cz));
            }
            return ok;
        }
        self.load_chunk(cx, cz)
    }

    /// Ensure chunk in the given dimension is loaded (from store or generated).
    pub(crate) fn ensure_chunk_dim(&mut self, dim: i32, cx: i32, cz: i32) {
        if dim == -1 && self.hell.is_some() {
            let Some(hell) = self.hell.as_mut() else { return };
            if !hell.has_chunk(cx, cz) && !hell.recall_chunk(cx, cz) {
                if !self.load_chunk_dim(-1, cx, cz) {
                    if let Some(hell) = self.hell.as_mut() {
                        hell.ensure_chunk(cx, cz);
                    }
                }
            }
        } else {
            if !self.world.has_chunk(cx, cz) && !self.world.recall_chunk(cx, cz) {
                if !self.load_chunk(cx, cz) {
                    self.world.ensure_chunk(cx, cz);
                }
            }
        }
    }

    /// Per-tick chunk pump for one play session (mirrors the handler
    /// `tick` chunk half: unload on chunk change, queue rebuild, up to
    /// 15 sends with 3x3 population, tile packets on send).
    pub(crate) fn stream_tick(&mut self, _cid: ConnId, sess: &mut Session) {
        let (play, stream) = match &mut sess.state {
            SessionState::Play(p, s) => (p, s),
            SessionState::Login(_) => return,
        };
        let dim = stream.dim;
        // Resolve the player's chunk from the right world.
        let (pcx, pcz) = {
            let world = if dim == -1 && self.hell.is_some() {
                let Some(h) = self.hell.as_ref() else { return };
                h
            } else {
                &self.world
            };
            match world.entities.get(play.player) {
                Some(e) => (
                    (e.body().pos[0].floor() as i32).div_euclid(16),
                    (e.body().pos[2].floor() as i32).div_euclid(16),
                ),
                None => return,
            }
        };
        let view = self.settings.view_distance;
        if stream.last_cx != pcx || stream.last_cz != pcz {
            stream.last_cx = pcx;
            stream.last_cz = pcz;
            // Unload what fell out of view (mirrors the C++ removal pass).
            let stale: Vec<ChunkMapKey> = stream
                .sent
                .iter()
                .copied()
                .filter(|k| {
                    (k.0 != dim)
                        || (k.1 - pcx).abs() > view
                        || (k.2 - pcz).abs() > view
                })
                .collect();
            for key in stale {
                let (_, sx, sz) = key;
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
                    if !stream.sent.contains(&chunk_map_key(dim, cx, cz)) {
                        needed.push((cx, cz));
                    }
                }
            }
            needed.sort_by_key(|(cx, cz)| (cx - pcx) * (cx - pcx) + (cz - pcz) * (cz - pcz));
            let mut queued: HashSet<ChunkMapKey> =
                needed.iter().map(|(cx, cz)| chunk_map_key(dim, *cx, *cz)).collect();
            let mut merged = needed;
            for (cx, cz) in std::mem::take(&mut stream.queue) {
                let key = chunk_map_key(dim, cx, cz);
                if !queued.contains(&key) && !stream.sent.contains(&key) {
                    queued.insert(key);
                    merged.push((cx, cz));
                }
            }
            stream.queue = merged;
        }

        // Drain any completed background chunk generations into the
        // matching dimension (shared 64-slot pending cap across dims so
        // one explorer can't starve the other world's queue).
        let completed: Vec<_> = match self.chunk_worker {
            Some(ref worker) => {
                let mut list = Vec::new();
                while let Ok(raw) = worker.resp_rx.try_recv() {
                    list.push(raw);
                }
                list
            }
            None => Vec::new(),
        };
        for raw in completed {
            self.pending_chunk_gens.remove(&(raw.dim, raw.cx, raw.cz));
            if raw.dim == -1 && self.hell.is_some() {
                if let Some(hell) = self.hell.as_mut() {
                    if !hell.has_chunk(raw.cx, raw.cz) && !hell.recall_chunk(raw.cx, raw.cz) {
                        hell.insert_chunk_boxed(raw.chunk);
                    }
                }
            } else if !self.world.has_chunk(raw.cx, raw.cz) && !self.world.recall_chunk(raw.cx, raw.cz) {
                self.world.insert_chunk_boxed(raw.chunk);
            }
        }

        // Send within budget (mirrors the 15-chunk pass; the native
        // ensure path loads the store first, then generates). Fresh
        // generation is separately budgeted: a cold generate costs ~12 ms,
        // so unbounded ensures while exploring blow the 50 ms tick.
        // Skipped chunks stay queued for the next ticks. The generation
        // budget is shared across dimensions for the same reason.
        let mut sent_now = 0;
        let mut deferred_count = 0;
        let mut i = 0;
        while i < stream.queue.len() && sent_now < CHUNKS_PER_TICK as usize {
            let (qx, qz) = stream.queue[i];
            // Borrow the dimension world for this item; field-disjoint
            // accesses below (worker, pending, budget) stay legal.
            let mut deferred = false;
            'nb: for dx in -1..=1 {
                for dz in -1..=1 {
                    let (bx, bz) = (qx + dx, qz + dz);
                    let present = if dim == -1 && self.hell.is_some() {
                        self.hell.as_ref().map(|h| h.has_chunk(bx, bz)).unwrap_or(false)
                    } else {
                        self.world.has_chunk(bx, bz)
                    };
                    if !present {
                        let recalled = if dim == -1 && self.hell.is_some() {
                            self.hell.as_mut().map(|h| h.recall_chunk(bx, bz)).unwrap_or(false)
                        } else {
                            self.world.recall_chunk(bx, bz)
                        };
                        if !recalled {
                            let _ = self.load_chunk_dim(dim, bx, bz);
                        }
                    }
                    let populated = if dim == -1 && self.hell.is_some() {
                        self.hell
                            .as_ref()
                            .and_then(|h| h.chunk_ref(bx, bz))
                            .map(|c| c.is_terrain_populated)
                            .unwrap_or(false)
                    } else {
                        self.world
                            .chunk_ref(bx, bz)
                            .map(|c| c.is_terrain_populated)
                            .unwrap_or(false)
                    };
                    if populated {
                        continue;
                    }
                    for cdx in 0..=1 {
                        for cdz in 0..=1 {
                            let (nx, nz) = (bx + cdx, bz + cdz);
                            let present_n = if dim == -1 && self.hell.is_some() {
                                self.hell.as_ref().map(|h| h.has_chunk(nx, nz)).unwrap_or(false)
                            } else {
                                self.world.has_chunk(nx, nz)
                            };
                            let mut ready = present_n;
                            if !present_n {
                                let recalled_n = if dim == -1 && self.hell.is_some() {
                                    self.hell.as_mut().map(|h| h.recall_chunk(nx, nz)).unwrap_or(false)
                                } else {
                                    self.world.recall_chunk(nx, nz)
                                };
                                ready = recalled_n;
                                if !recalled_n {
                                    ready = self.load_chunk_dim(dim, nx, nz);
                                }
                            }
                            if !ready {
                                if let Some(ref worker) = self.chunk_worker {
                                    if !self.pending_chunk_gens.contains(&(dim, nx, nz))
                                        && self.pending_chunk_gens.len() < 64
                                    {
                                        self.pending_chunk_gens.insert((dim, nx, nz));
                                        let _ = worker.req_tx.send((dim, nx, nz));
                                    }
                                    deferred = true;
                                } else if self.chunks_generated_this_tick >= CHUNK_GEN_PER_TICK {
                                    deferred = true;
                                    break 'nb;
                                } else {
                                    self.chunks_generated_this_tick += 1;
                                }
                            }
                        }
                    }
                    if deferred {
                        break 'nb;
                    }
                    if self.chunks_generated_this_tick >= CHUNK_GEN_PER_TICK {
                        deferred = true;
                        break 'nb;
                    }
                    if dim == -1 && self.hell.is_some() {
                        if let Some(hell) = self.hell.as_mut() {
                            hell.ensure_chunk(bx, bz);
                        }
                    } else {
                        self.world.ensure_chunk(bx, bz);
                    }
                    self.chunks_generated_this_tick += 1;
                }
            }
            if deferred {
                if self.chunks_generated_this_tick >= CHUNK_GEN_PER_TICK
                    || (self.chunk_worker.is_some() && self.pending_chunk_gens.len() >= 64)
                {
                    break;
                }
                deferred_count += 1;
                if deferred_count >= 8 {
                    break;
                }
                i += 1;
                continue;
            }
            // Refresh + send from the dimension world.
            if dim == -1 && self.hell.is_some() {
                let Some(hell) = self.hell.as_mut() else {
                    i += 1;
                    continue;
                };
                if !hell.light_dirty.is_empty() {
                    hell.refresh_light();
                }
                let populated = hell
                    .chunk_ref(qx, qz)
                    .map(|c| c.is_terrain_populated)
                    .unwrap_or(false);
                if !populated {
                    i += 1;
                    continue;
                }
                play.outbox.push(pkt_pre_chunk(qx, qz, true));
                if let Some(chunk) = hell.chunk_ref(qx, qz) {
                    chunk.with_map_compressed(|data| {
                        play.outbox.push(pkt_map_chunk(
                            qx * 16,
                            0,
                            qz * 16,
                            16,
                            128,
                            16,
                            data,
                        ));
                    });
                    chunk.clear_compressed_cache();
                }
                for &(x, y, z) in hell.tiles.chunk_cells(qx, qz) {
                    if let Some(tile) = hell.tiles.get(&(x, y, z)) {
                        play.outbox.push(tile_packet(x, y, z, tile));
                        if matches!(tile, crate::world::TileData::Furnace(_)) {
                            if let Some(sc) =
                                crate::session_packets::pkt_subchunk_block(hell, x, y, z)
                            {
                                play.outbox.push(sc);
                            }
                        }
                    }
                }
            } else {
                if !self.world.light_dirty.is_empty() {
                    self.world.refresh_light();
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
                    chunk.with_map_compressed(|data| {
                        play.outbox.push(pkt_map_chunk(
                            qx * 16,
                            0,
                            qz * 16,
                            16,
                            128,
                            16,
                            data,
                        ));
                    });
                    chunk.clear_compressed_cache();
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
            }
            stream.sent.insert(chunk_map_key(dim, qx, qz));
            self.players_by_chunk.entry(chunk_map_key(dim, qx, qz)).or_default().insert(play.player);
            stream.queue.remove(i);
            sent_now += 1;
        }
    }
}
