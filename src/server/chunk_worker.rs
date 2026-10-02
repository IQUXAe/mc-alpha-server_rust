//! Asynchronous background chunk generation worker.
//! Offloads heavy Perlin noise terrain and cave carving off the main tick thread.
//!
//! Dual-dimension: requests carry the dimension (0 overworld, -1 hell);
//! the worker owns both providers and builds terrain + caves + height +
//! skylight for either. Populate (decorations) stays on the main thread:
//! overworld populate needs the 2x2 canvas, hell populate needs live
//! world access for cross-chunk lookups.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{Builder, JoinHandle};

/// Output from the background worker: a fully constructed Box<Chunk> with
/// terrain generated, height map built, skylight calculated, and pre-compressed.
/// `dim` mirrors the request dimension. Populate/decorations are NOT done
/// here (main thread finishes them on insert).
pub struct GeneratedRawChunk {
    pub dim: i32,
    pub cx: i32,
    pub cz: i32,
    pub chunk: Box<crate::chunk::Chunk>,
}

/// Background worker running terrain generation off-thread.
pub struct ChunkGenWorker {
    pub req_tx: Sender<(i32, i32, i32)>,
    pub resp_rx: Receiver<GeneratedRawChunk>,
    _handle: Option<JoinHandle<()>>,
}

impl ChunkGenWorker {
    /// Start a worker thread with the given world seed.
    pub fn start(seed: i64) -> Self {
        let (req_tx, req_rx) = channel::<(i32, i32, i32)>();
        let (resp_tx, resp_rx) = channel::<GeneratedRawChunk>();

        let handle = Builder::new()
            .name("chunk-gen".to_string())
            .spawn(move || {
                use crate::biome::MobSpawnerBase;
                use crate::generator::{generate_chunk, ChunkProvider};

                let mut gen = ChunkProvider::new(seed);
                let mut hell = crate::hell_gen::HellProvider::new(seed);
                let mut biomes = [MobSpawnerBase::DEFAULT; 256];
                let mut temps = [0.0f64; 256];
                let mut humids = [0.0f64; 256];
                while let Ok((dim, cx, cz)) = req_rx.recv() {
                    let mut chunk = Box::new(crate::chunk::Chunk::new(cx, cz));
                    if dim == -1 {
                        crate::hell_gen::generate_hell_chunk(
                            &mut hell,
                            cx,
                            cz,
                            chunk.blocks_mut(),
                        );
                    } else {
                        generate_chunk(
                            &mut gen,
                            cx,
                            cz,
                            chunk.blocks_mut(),
                            &mut biomes,
                            &mut temps,
                            &mut humids,
                        );
                    }
                    chunk.generate_height_map();
                    chunk.generate_skylight_map();
                    let _ = chunk.map_compressed();
                    if resp_tx.send(GeneratedRawChunk { dim, cx, cz, chunk }).is_err() {
                        break;
                    }
                }
            })
            .ok();

        Self {
            req_tx,
            resp_rx,
            _handle: handle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::MobSpawnerBase;
    use crate::generator::{generate_chunk, ChunkProvider};

    #[test]
    fn test_async_chunk_worker_matches_sync_generation() {
        let seed = 987654321;
        let worker = ChunkGenWorker::start(seed);

        let (cx, cz) = (2, -3);
        worker.req_tx.send((0, cx, cz)).unwrap();

        let result = worker.resp_rx.recv().unwrap();
        assert_eq!(result.cx, cx);
        assert_eq!(result.cz, cz);

        // Generate synchronously with the same seed to verify bit-exact determinism
        let mut gen = ChunkProvider::new(seed);
        let mut sync_blocks = Box::new([0u8; 32768]);
        let mut biomes = [MobSpawnerBase::DEFAULT; 256];
        let mut temps = [0.0f64; 256];
        let mut humids = [0.0f64; 256];
        generate_chunk(
            &mut gen,
            cx,
            cz,
            &mut sync_blocks,
            &mut biomes,
            &mut temps,
            &mut humids,
        );

        assert_eq!(result.chunk.blocks(), &*sync_blocks);
    }

    #[test]
    fn test_async_hell_worker_matches_sync_generation() {
        let seed = 123456789;
        let worker = ChunkGenWorker::start(seed);

        let (cx, cz) = (-1, 4);
        worker.req_tx.send((-1, cx, cz)).unwrap();

        let result = worker.resp_rx.recv().unwrap();
        assert_eq!(result.dim, -1);
        assert_eq!(result.cx, cx);
        assert_eq!(result.cz, cz);

        let mut hell = crate::hell_gen::HellProvider::new(seed);
        let mut sync_blocks = [0u8; 32768];
        crate::hell_gen::generate_hell_chunk(&mut hell, cx, cz, &mut sync_blocks);
        assert_eq!(result.chunk.blocks(), &sync_blocks);
    }
}
