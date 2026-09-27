//! On-demand chunk generation/population on [`World`].

use crate::chunk::Chunk;
use crate::world::World;
use crate::world::tiles::TileData;

impl World {
    /// Generate and populate one chunk on demand: raw terrain for the 2x2
    /// canvas, decoration of the requested chunk with the world as fallback
    /// accessor, then insert/write-back with fresh height and sky maps.
    /// Populated chunks are left alone; each chunk is decorated once.
    pub fn ensure_chunk(&mut self, cx: i32, cz: i32) {
        self.recall_chunk(cx, cz);
        if self.chunks.get(&(cx, cz)).map(|c| c.is_terrain_populated).unwrap_or(false) {
            return;
        }
        use crate::biome::MobSpawnerBase;
        use crate::generator::{generate_chunk, populate_batch};
        // 1. Stage the 2x2 canvas (existing/recalled chunks copied, missing generated).
        let (center_biome, center_temps) = {
            let gen = self.generator();
            let mut center_temps = [0.0f64; 256];
            crate::generator::chunk_temperatures(gen, cx * 16 + 8, cz * 16 + 8, &mut center_temps);
            let center_biome = crate::generator::point_biome(gen, cx * 16 + 16, cz * 16 + 16);
            (center_biome, center_temps)
        };
        let mut stage_blocks = [[[0u8; 32768]; 2]; 2];
        let mut stage_meta = [[[0u8; 32768]; 2]; 2];
        for dx in 0..2usize {
            for dz in 0..2usize {
                let (nx, nz) = (cx + dx as i32, cz + dz as i32);
                self.recall_chunk(nx, nz);
                if let Some(c) = self.chunks.get(&(nx, nz)) {
                    c.fill_arrays(&mut stage_blocks[dx][dz], &mut stage_meta[dx][dz]);
                } else {
                    let gen = self.generator();
                    let mut blocks = [0u8; 32768];
                    let mut biomes = [MobSpawnerBase::DEFAULT; 256];
                    let mut temps = [0.0f64; 256];
                    let mut humids = [0.0f64; 256];
                    generate_chunk(
                        gen,
                        nx,
                        nz,
                        &mut blocks,
                        &mut biomes,
                        &mut temps,
                        &mut humids,
                    );
                    stage_blocks[dx][dz] = blocks;
                }
            }
        }
        // 2. Decorate the requested chunk over the canvas.
        let dungeon_loot = {
            self.populating = true;
            if self.generator.is_none() {
                let seed = self.seed;
                self.generator =
                    Some(crate::generator::ChunkProvider::new(seed));
            }
            let gen = self.generator.as_mut().unwrap();
            let mut fallback = crate::decorators::WorldAccess {
                chunks: &mut self.chunks,
                populating: self.populating,
                queue: None,
            };
            let loot = populate_batch(
                gen,
                &mut stage_blocks,
                &mut stage_meta,
                &mut fallback,
                [cx, cz],
                center_biome.biome_type,
                &center_temps,
            );
            self.populating = false;
            loot
        };
        // 3. Write back: insert missing, refresh present (tree spillover);
        // only the requested chunk is flagged (canvas neighbors decorate
        // on their own request, exactly once each).
        let mut canvas_set = std::collections::HashSet::with_capacity(4);
        for dx in 0..2usize {
            for dz in 0..2usize {
                let (nx, nz) = (cx + dx as i32, cz + dz as i32);
                let requested = dx == 0 && dz == 0;
                match self.chunks.get_mut(&(nx, nz)) {
                    Some(c) => {
                        c.load_arrays(&stage_blocks[dx][dz], &stage_meta[dx][dz]);
                        c.generate_skylight_map();
                        if requested {
                            c.is_terrain_populated = true;
                        }
                    }
                    None => {
                        let mut c = Chunk::new(nx, nz);
                        c.load_arrays(&stage_blocks[dx][dz], &stage_meta[dx][dz]);
                        c.generate_skylight_map();
                        c.is_terrain_populated = requested;
                        self.chunks.insert((nx, nz), c);
                    }
                }
                self.light_dirty.insert((nx, nz));
                canvas_set.insert((nx, nz));
            }
        }
        self.propagate_cross_chunk_light(&canvas_set);
        // 4. Dungeon-chest loot & spawner tiles: the canvas holds no tiles, so
        // materialize chest and mob-spawner rows on write-back.
        for (lx, ly, lz, slot, item, count) in dungeon_loot {
            if slot == -1 {
                if self.get_block_id(lx, ly, lz) == 52 {
                    let entity_id = match item {
                        51 => "Skeleton",
                        52 => "Spider",
                        54 => "Zombie",
                        _ => "Pig",
                    };
                    self.tiles.insert(
                        (lx, ly, lz),
                        TileData::MobSpawner(crate::world::tiles::MobSpawnerState::new(entity_id)),
                    );
                }
                continue;
            }
            if self.get_block_id(lx, ly, lz) != 54 {
                continue;
            }
            let tile = self.tiles.entry((lx, ly, lz)).or_insert_with(|| {
                TileData::Chest(crate::tile_entity::chest::chest_create())
            });
            if let TileData::Chest(ch) = tile {
                if slot >= 0 && (slot as usize) < ch.slots.len() {
                    ch.slots[slot as usize] = crate::inventory::ItemStack::new(item, count, 0);
                }
            }
        }
    }

    /// Lazily built terrain generator (eleven octave tables; tests that
    /// never generate pay nothing).
    fn generator(&mut self) -> &mut crate::generator::ChunkProvider {
        if self.generator.is_none() {
            let seed = self.seed;
            self.generator =
                Some(crate::generator::ChunkProvider::new(seed));
        }
        self.generator.as_mut().unwrap()
    }

    /// Generate a square of chunks around a center with progress feedback.
    pub fn ensure_area_with_progress<F>(&mut self, cx: i32, cz: i32, radius: i32, mut on_progress: F)
    where
        F: FnMut(usize, usize),
    {
        let side = (2 * radius + 1) as usize;
        let total = side * side;
        let mut count = 0;
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                self.ensure_chunk(cx + dx, cz + dz);
                count += 1;
                on_progress(count, total);
            }
        }
    }

    /// Generate a square of chunks around a center (join/respawn path for
    /// the network slice).
    pub fn ensure_area(&mut self, cx: i32, cz: i32, radius: i32) {
        self.ensure_area_with_progress(cx, cz, radius, |_, _| {});
    }
}
