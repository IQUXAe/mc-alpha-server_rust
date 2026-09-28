//! Chunk data + light (mirrors Java `Chunk.java`).
//!
//! Mapping notes:
//! - Dimensions match Alpha 1.2.6 exactly: 16 x 128 x 16, volume 32768.
//! - Index formula: `(x << 11) | (z << 7) | y`.
//! - `blocks` is an owned `[u8; 32768]`; `data` / `skylight` / `blocklight`
//!   reuse [`crate::nibble::NibbleArray`] (16384 bytes each, same nibble layout).
//! - `height_map` is an owned `[u8; 256]`, indexed `(z << 4) | x`.
//! - `lightOpacity` / `lightValue` are NOT duplicated here: they are read from
//!   [`crate::block`] via `block_properties_get` (single source of truth).
//! - Out-of-range policy: reads return `0`, writes are no-ops returning
//!   `false`. Vanilla performs no bounds check and would write out of
//!   bounds; this port deliberately hardens that path.
//! - `is_modified` is set on any successful in-bounds mutation
//!   (equivalent to the world-present path). OOB no-ops never dirty the chunk.
//!
//! Deferred (require `World`, entities, or I/O — intentionally not here):
//! - `World` back-pointer / cross-chunk lookup
//! - `TileEntity` map
//! - Auto `generateSkylightMap()` call inside block set (the caller decides)
//! - `isTerrainPopulated` is stored but never acted on (needs generator).

use crate::block::table::block_properties_get;
use crate::nibble::NibbleArray;
use std::collections::VecDeque;

/// Chunk dimensions (Alpha 1.2.6).
pub const CHUNK_SIZE_X: i32 = 16;
/// Chunk height (Alpha 1.2.6).
pub const CHUNK_SIZE_Y: i32 = 128;
/// Chunk depth (Alpha 1.2.6).
pub const CHUNK_SIZE_Z: i32 = 16;
/// 16 * 128 * 16.
pub const CHUNK_VOLUME: usize = 32768;
/// 16 * 16.
pub const CHUNK_AREA: usize = 256;
/// Backing bytes per nibble array (32768 nibbles).
pub const CHUNK_NIBBLE_BYTES: usize = CHUNK_VOLUME / 2;
/// Skylight type id for [`Chunk::get_saved_light_value`].
pub const SKY_LIGHT: i32 = 0;
/// Blocklight type id for [`Chunk::get_saved_light_value`].
pub const BLOCK_LIGHT: i32 = 1;

fn light_opacity(id: u8) -> i32 {
    block_properties_get(u32::from(id)).light_opacity
}

fn light_value(id: u8) -> i32 {
    block_properties_get(u32::from(id)).light_value
}

fn is_transparent(id: u8) -> bool {
    light_opacity(id) < 15
}

fn vertical_opacity(id: u8) -> i32 {
    light_opacity(id)
}

fn bfs_opacity(id: u8) -> i32 {
    let op = light_opacity(id);
    if op < 1 {
        1
    } else {
        op
    }
}

#[derive(Clone, Copy)]
struct LightNode {
    x: i32,
    y: i32,
    z: i32,
}

/// Serialized loose item waiting for its chunk to load (mirrors
/// `ChunkEntityData`; age freezes while unloaded).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingItem {
    pub item_id: i32,
    pub count: i32,
    pub damage: i32,
    pub age: i32,
    pub pickup_delay: i32,
    pub pos: [f64; 3],
}

/// Serialized creature waiting for its chunk to load (mirrors
/// `ChunkAnimalData`, shared by animals and monsters).
#[derive(Clone, Debug, PartialEq)]
pub struct PendingCreature {
    pub string_id: String,
    pub pos: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub health: i16,
    pub max_health: i16,
    pub saddled: bool,
    pub sheared: bool,
    pub egg_timer: i32,
}

/// Serialized boat waiting for its chunk to load (mirrors `ChunkBoatData`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PendingBoat {
    pub pos: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub time_since_hit: i32,
    pub damage_taken: i32,
    pub forward_dir: i32,
}

/// Owned chunk data + lighting (no `World`, no entities, no I/O).
pub struct Chunk {
    /// Chunk X position (mirrors C++ `xPosition`).
    pub x_position: i32,
    /// Chunk Z position (mirrors C++ `zPosition`).
    pub z_position: i32,
    /// Mirrors C++ `isTerrainPopulated` (stored, never acted on here).
    pub is_terrain_populated: bool,
    /// Mirrors C++ `isModified` (plain bool; single-threaded port).
    pub is_modified: bool,
    /// Unload spill (mirrors `pendingItems` / `pendingAnimals` /
    /// `pendingMonsters` / `pendingBoats`): entities frozen with their
    /// chunk, restored on load.
    pub pending_items: Vec<PendingItem>,
    pub pending_animals: Vec<PendingCreature>,
    pub pending_monsters: Vec<PendingCreature>,
    pub pending_boats: Vec<PendingBoat>,
    blocks: [u8; CHUNK_VOLUME],
    data: NibbleArray,
    skylight: NibbleArray,
    blocklight: NibbleArray,
    height_map: [u8; CHUNK_AREA],
    cached_map_compressed: std::sync::Mutex<Option<Vec<u8>>>,
}

impl Clone for Chunk {
    fn clone(&self) -> Self {
        Self {
            x_position: self.x_position,
            z_position: self.z_position,
            is_terrain_populated: self.is_terrain_populated,
            is_modified: self.is_modified,
            pending_items: self.pending_items.clone(),
            pending_animals: self.pending_animals.clone(),
            pending_monsters: self.pending_monsters.clone(),
            pending_boats: self.pending_boats.clone(),
            blocks: self.blocks,
            data: self.data.clone(),
            skylight: self.skylight.clone(),
            blocklight: self.blocklight.clone(),
            height_map: self.height_map,
            cached_map_compressed: std::sync::Mutex::new(
                self.cached_map_compressed
                    .lock()
                    .ok()
                    .and_then(|c| c.clone()),
            ),
        }
    }
}

impl std::fmt::Debug for Chunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Chunk")
            .field("x_position", &self.x_position)
            .field("z_position", &self.z_position)
            .field("is_terrain_populated", &self.is_terrain_populated)
            .field("is_modified", &self.is_modified)
            .field("pending_items", &self.pending_items.len())
            .field("pending_animals", &self.pending_animals.len())
            .field("pending_monsters", &self.pending_monsters.len())
            .field("pending_boats", &self.pending_boats.len())
            .finish_non_exhaustive()
    }
}

impl Chunk {
    /// New all-air chunk (mirrors `Chunk(world, x, z)` allocation, minus `World`).
    pub fn new(x: i32, z: i32) -> Self {
        Self {
            x_position: x,
            z_position: z,
            is_terrain_populated: false,
            is_modified: false,
            pending_items: Vec::new(),
            pending_animals: Vec::new(),
            pending_monsters: Vec::new(),
            pending_boats: Vec::new(),
            blocks: [0u8; CHUNK_VOLUME],
            data: NibbleArray::with_nibbles(CHUNK_VOLUME),
            skylight: NibbleArray::with_nibbles(CHUNK_VOLUME),
            blocklight: NibbleArray::with_nibbles(CHUNK_VOLUME),
            height_map: [0u8; CHUNK_AREA],
            cached_map_compressed: std::sync::Mutex::new(None),
        }
    }

    #[inline]
    fn invalidate_cache(&self) {
        if let Ok(mut lock) = self.cached_map_compressed.lock() {
            *lock = None;
        }
    }

    /// Explicitly clear the cached compressed map to free memory (e.g. on chunk unload or after streaming).
    #[inline]
    pub fn clear_compressed_cache(&self) {
        self.invalidate_cache();
    }

    /// Reference to raw chunk blocks array.
    #[inline]
    pub fn blocks(&self) -> &[u8; CHUNK_VOLUME] {
        &self.blocks
    }

    /// Mutable reference to raw chunk blocks array. Invalidates cached compression.
    #[inline]
    pub fn blocks_mut(&mut self) -> &mut [u8; CHUNK_VOLUME] {
        self.invalidate_cache();
        &mut self.blocks
    }

    /// Index formula matching Alpha exactly: `x << 11 | z << 7 | y`.
    ///
    /// Callers must bounds-check first; the value is meaningless for OOB
    /// coordinates (mirrors C++ arithmetic without the UB on use).
    #[inline]
    pub fn get_index(&self, x: i32, y: i32, z: i32) -> usize {
        ((x as usize) << 11) | ((z as usize) << 7) | (y as usize)
    }

    /// Same formula as an associated function (no `self` needed).
    #[inline]
    pub fn index(x: i32, y: i32, z: i32) -> usize {
        ((x as usize) << 11) | ((z as usize) << 7) | (y as usize)
    }

    fn in_bounds(x: i32, y: i32, z: i32) -> bool {
        (0..CHUNK_SIZE_X).contains(&x)
            && (0..CHUNK_SIZE_Y).contains(&y)
            && (0..CHUNK_SIZE_Z).contains(&z)
    }

    fn column_in_bounds(x: i32, z: i32) -> bool {
        (0..CHUNK_SIZE_X).contains(&x) && (0..CHUNK_SIZE_Z).contains(&z)
    }

    fn block_index(x: i32, y: i32, z: i32) -> Option<usize> {
        if Self::in_bounds(x, y, z) {
            Some(Self::index(x, y, z))
        } else {
            None
        }
    }

    fn height_index(x: i32, z: i32) -> Option<usize> {
        if Self::column_in_bounds(x, z) {
            Some((((z as usize) << 4) | (x as usize)) & (CHUNK_AREA - 1))
        } else {
            None
        }
    }

    /// Mirrors `getBlockID`: OOB returns `0`.
    pub fn get_block_id(&self, x: i32, y: i32, z: i32) -> u8 {
        match Self::block_index(x, y, z) {
            Some(idx) => match self.blocks.get(idx) {
                Some(v) => *v,
                None => 0,
            },
            None => 0,
        }
    }

    /// Mirrors `setBlockID` (hardened: OOB is a no-op returning `false`).
    ///
    /// On success the height column is recalculated and `is_modified` is set
    /// (the C++ world-present path; the C++ null-world path leaves it clean).
    pub fn set_block_id(&mut self, x: i32, y: i32, z: i32, block_id: u8) -> bool {
        // Mirrors `setBlockIDWithMetadata(id, 0)`: the metadata nibble
        // resets alongside the id (the native world has no lighting sim,
        // so no skylight rebuild either).
        self.set_block_id_with_metadata(x, y, z, block_id, 0)
    }

    /// Mirrors `setBlockIDWithMetadata` (hardened: OOB is a no-op `false`).
    ///
    /// Unlike C++ this never triggers an automatic skylight rebuild: C++
    /// only rebuilds when `worldObj` is present, and the isolated port has no
    /// `World`. Call [`Chunk::generate_skylight_map`] explicitly if needed.
    pub fn set_block_id_with_metadata(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        block_id: u8,
        metadata: u8,
    ) -> bool {
        if !Self::in_bounds(x, y, z) {
            return false;
        }
        let old_id = self.get_block_id(x, y, z);
        let old_meta = self.data.get_nibble(x, y, z);
        if old_id == block_id && old_meta == (metadata & 0xF) {
            return false;
        }
        let idx = match Self::block_index(x, y, z) {
            Some(idx) => idx,
            None => return false,
        };
        if let Some(slot) = self.blocks.get_mut(idx) {
            *slot = block_id;
        } else {
            return false;
        }
        self.data.set_nibble(x, y, z, metadata);
        self.recalculate_height_column(x, z);
        self.invalidate_cache();
        self.is_modified = true;
        true
    }

    /// Mirrors `getBlockMetadata` (OOB-safe via [`NibbleArray`]).
    pub fn get_block_metadata(&self, x: i32, y: i32, z: i32) -> u8 {
        self.data.get_nibble(x, y, z)
    }

    /// Mirrors `setBlockMetadata` (OOB no-op; only in-bounds writes dirty).
    pub fn set_block_metadata(&mut self, x: i32, y: i32, z: i32, metadata: u8) {
        if !Self::in_bounds(x, y, z) {
            return;
        }
        self.data.set_nibble(x, y, z, metadata);
        self.invalidate_cache();
        self.is_modified = true;
    }

    /// Bulk export into flat byte arrays (generator canvas layout
    /// `x << 11 | z << 7 | y`, metadata unpacked from nibbles).
    pub fn fill_arrays(&self, blocks: &mut [u8; CHUNK_VOLUME], meta: &mut [u8; CHUNK_VOLUME]) {
        blocks.copy_from_slice(&self.blocks);
        for x in 0..CHUNK_SIZE_X {
            for z in 0..CHUNK_SIZE_Z {
                for y in 0..CHUNK_SIZE_Y {
                    meta[Self::index(x, y, z)] = self.data.get_nibble(x, y, z);
                }
            }
        }
    }

    /// Bulk import from flat byte arrays (metadata packed into nibbles)
    /// plus a height-map rebuild, for generated or restored chunks.
    pub fn load_arrays(&mut self, blocks: &[u8; CHUNK_VOLUME], meta: &[u8; CHUNK_VOLUME]) {
        self.blocks.copy_from_slice(blocks);
        for x in 0..CHUNK_SIZE_X {
            for z in 0..CHUNK_SIZE_Z {
                for y in 0..CHUNK_SIZE_Y {
                    self.data.set_nibble(x, y, z, meta[Self::index(x, y, z)] & 0xF);
                }
            }
        }
        self.generate_height_map();
        self.invalidate_cache();
        self.is_modified = true;
    }

    /// Bulk light/height import from blob layout (packed nibbles like the
    /// NBT arrays; even cell index is the low nibble). Length-checked.
    /// Height map layout is `(z << 4) | x` like [`Chunk::height_index`].
    pub fn load_light_maps(&mut self, sky: &[u8], block: &[u8], height: &[u8]) -> bool {
        if sky.len() != CHUNK_NIBBLE_BYTES
            || block.len() != CHUNK_NIBBLE_BYTES
            || height.len() != CHUNK_AREA
        {
            return false;
        }
        for x in 0..CHUNK_SIZE_X {
            for z in 0..CHUNK_SIZE_Z {
                for y in 0..CHUNK_SIZE_Y {
                    let idx = Self::index(x, y, z);
                    self.skylight.set_nibble(x, y, z, (sky[idx / 2] >> (4 * (idx % 2))) & 0xF);
                    self.blocklight.set_nibble(x, y, z, (block[idx / 2] >> (4 * (idx % 2))) & 0xF);
                }
                let hi = (((z as usize) << 4) | (x as usize)) & (CHUNK_AREA - 1);
                if let Some(slot) = self.height_map.get_mut(hi) {
                    *slot = height[hi];
                }
            }
        }
        self.invalidate_cache();
        self.is_modified = true;
        true
    }

    /// Bulk export into blob layout (packed nibbles + height map).
    pub fn export_light_maps(
        &self,
        sky: &mut [u8; CHUNK_NIBBLE_BYTES],
        block: &mut [u8; CHUNK_NIBBLE_BYTES],
        height: &mut [u8; CHUNK_AREA],
    ) {
        for x in 0..CHUNK_SIZE_X {
            for z in 0..CHUNK_SIZE_Z {
                for y in 0..CHUNK_SIZE_Y {
                    let idx = Self::index(x, y, z);
                    let s = self.skylight.get_nibble(x, y, z) & 0xF;
                    let b = self.blocklight.get_nibble(x, y, z) & 0xF;
                    if idx % 2 == 0 {
                        sky[idx / 2] = s;
                        block[idx / 2] = b;
                    } else {
                        sky[idx / 2] |= s << 4;
                        block[idx / 2] |= b << 4;
                    }
                }
                let hi = (((z as usize) << 4) | (x as usize)) & (CHUNK_AREA - 1);
                height[hi] = self.height_map[hi];
            }
        }
    }

    /// Raw Packet51 payload length: 32768 block ids + 16384 packed
    /// metadata + 16384 blocklight + 16384 skylight nibbles (mirrors the
    /// C++ `CHUNK_VOLUME * 5 / 2` buffer).
    pub const MAP_RAW_LEN: usize = CHUNK_VOLUME + 3 * CHUNK_NIBBLE_BYTES;

    /// Mirrors `Chunk::getChunkData` before compression: block ids, then
    /// the packed metadata, blocklight, and skylight stores in that order
    /// (1:1 with the four C++ `std::copy` ranges).
    pub fn map_raw(&self) -> Vec<u8> {
        let mut raw = vec![0u8; Self::MAP_RAW_LEN];
        raw[..CHUNK_VOLUME].copy_from_slice(&self.blocks);
        let (meta, rest) = raw[CHUNK_VOLUME..].split_at_mut(CHUNK_NIBBLE_BYTES);
        meta.copy_from_slice(self.data.view());
        let (block, sky) = rest.split_at_mut(CHUNK_NIBBLE_BYTES);
        block.copy_from_slice(self.blocklight.view());
        sky.copy_from_slice(self.skylight.view());
        raw
    }

    /// View the cached or freshly-compressed chunk map data via closure,
    /// avoiding intermediate `Vec<u8>` cloning.
    pub fn with_map_compressed<R, F: FnOnce(&[u8]) -> R>(&self, f: F) -> R {
        if let Ok(guard) = self.cached_map_compressed.lock() {
            if let Some(ref cached) = *guard {
                return f(cached);
            }
        }
        use std::io::Read;
        let raw = self.map_raw();
        let mut encoder =
            flate2::read::ZlibEncoder::new(raw.as_slice(), flate2::Compression::new(1));
        let mut out = Vec::new();
        if encoder.read_to_end(&mut out).is_ok() {
            if let Ok(mut guard) = self.cached_map_compressed.lock() {
                *guard = Some(out);
                if let Some(ref cached) = *guard {
                    return f(cached);
                }
            }
        }
        f(&[])
    }

    /// zlib-compressed map payload at level 1 (mirrors the
    /// Compressed tail of chunk data;
    /// empty on I/O failure like the other codecs here).
    /// Result is cached until chunk blocks or lighting change.
    pub fn map_compressed(&self) -> Vec<u8> {
        self.with_map_compressed(|data| data.to_vec())
    }


    /// Mirrors `getSavedLightValue`: `0` = sky, anything else = block.
    pub fn get_saved_light_value(&self, light_type: i32, x: i32, y: i32, z: i32) -> u8 {
        if light_type == SKY_LIGHT {
            self.skylight.get_nibble(x, y, z)
        } else {
            self.blocklight.get_nibble(x, y, z)
        }
    }

    /// Mirrors `setLightValue` (OOB no-op; only in-bounds writes dirty).
    pub fn set_light_value(&mut self, light_type: i32, x: i32, y: i32, z: i32, value: u8) {
        if !Self::in_bounds(x, y, z) {
            return;
        }
        if light_type == SKY_LIGHT {
            self.skylight.set_nibble(x, y, z, value);
        } else {
            self.blocklight.set_nibble(x, y, z, value);
        }
        self.invalidate_cache();
        self.is_modified = true;
    }

    /// Convenience: skylight read.
    pub fn get_skylight(&self, x: i32, y: i32, z: i32) -> u8 {
        self.get_saved_light_value(SKY_LIGHT, x, y, z)
    }

    /// Convenience: blocklight read.
    pub fn get_blocklight(&self, x: i32, y: i32, z: i32) -> u8 {
        self.get_saved_light_value(BLOCK_LIGHT, x, y, z)
    }

    /// Mirrors `getHeightValue`: OOB returns `0`.
    pub fn get_height_value(&self, x: i32, z: i32) -> i32 {
        match Self::height_index(x, z) {
            Some(idx) => match self.height_map.get(idx) {
                Some(v) => i32::from(*v),
                None => 0,
            },
            None => 0,
        }
    }

    /// Clear the dirty flag (mirrors the save-thread consume path).
    pub fn clear_modified(&mut self) {
        self.is_modified = false;
    }

    fn recalculate_height_column(&mut self, x: i32, z: i32) {
        if !Self::column_in_bounds(x, z) {
            return;
        }
        let mut y: i32 = CHUNK_SIZE_Y - 1;
        while y > 0 {
            let below = self.get_block_id(x, y - 1, z);
            if light_opacity(below) != 0 {
                break;
            }
            y -= 1;
        }
        if let Some(idx) = Self::height_index(x, z) {
            if let Some(slot) = self.height_map.get_mut(idx) {
                let clamped = if y < 0 {
                    0u8
                } else if y > 255 {
                    255u8
                } else {
                    y as u8
                };
                *slot = clamped;
            }
        }
    }

    /// Mirrors `generateHeightMap` 1:1.
    pub fn generate_height_map(&mut self) {
        let mut x: i32 = 0;
        while x < CHUNK_SIZE_X {
            let mut z: i32 = 0;
            while z < CHUNK_SIZE_Z {
                self.recalculate_height_column(x, z);
                z += 1;
            }
            x += 1;
        }
    }

    /// Single-chunk port of `generateSkylightMap` 1:1 (order + attenuation).
    ///
    /// STEP 1 (vertical pass) is identical. STEP 2/3 (BFS spread) keep the
    /// exact C++ order (`-x, +x, -y, +y, -z, +z`), the same
    /// `isTransparent = opacity < 15` gate, vertical attenuation
    /// (`currentSky -= opacity`, floor 0) and BFS attenuation
    /// (`newLight = current - max(1, opacity)`, propagate iff greater).
    ///
    /// Difference: C++ spreads across chunk borders via
    /// `worldObj->getChunkFromBlockCoords(..., false)`. Without a `World`
    /// this port stops at the chunk border (equivalent to C++ with all
    /// neighbours missing).
    pub fn generate_skylight_map(&mut self) {
        self.invalidate_cache();
        let mut sky_queue: VecDeque<LightNode> = VecDeque::new();
        let mut block_queue: VecDeque<LightNode> = VecDeque::new();

        // STEP 1: vertical initial pass + emitter detection.
        let mut x: i32 = 0;
        while x < CHUNK_SIZE_X {
            let mut z: i32 = 0;
            while z < CHUNK_SIZE_Z {
                let mut current_sky: i32 = 15;
                let mut y: i32 = CHUNK_SIZE_Y - 1;
                while y >= 0 {
                    let id = self.get_block_id(x, y, z);
                    if !is_transparent(id) {
                        current_sky = 0;
                    } else {
                        current_sky -= vertical_opacity(id);
                        if current_sky < 0 {
                            current_sky = 0;
                        }
                    }
                    let sky_byte = if current_sky > 15 {
                        15u8
                    } else if current_sky < 0 {
                        0u8
                    } else {
                        current_sky as u8
                    };
                    self.skylight.set_nibble(x, y, z, sky_byte);
                    if current_sky > 0 {
                        sky_queue.push_back(LightNode { x, y, z });
                    }

                    let emission = light_value(id);
                    let emit_byte = if emission < 0 {
                        0u8
                    } else if emission > 15 {
                        15u8
                    } else {
                        emission as u8
                    };
                    self.blocklight.set_nibble(x, y, z, emit_byte);
                    if emission > 0 {
                        block_queue.push_back(LightNode { x, y, z });
                    }
                    y -= 1;
                }
                z += 1;
            }
            x += 1;
        }

        // STEP 2: skylight BFS (C++ neighbour order preserved).
        while let Some(node) = sky_queue.pop_front() {
            let current_light = i32::from(self.get_saved_light_value(SKY_LIGHT, node.x, node.y, node.z));
            if current_light <= 1 {
                continue;
            }
            const OFFSETS: [(i32, i32, i32); 6] = [
                (-1, 0, 0),
                (1, 0, 0),
                (0, -1, 0),
                (0, 1, 0),
                (0, 0, -1),
                (0, 0, 1),
            ];
            let mut i: usize = 0;
            while i < OFFSETS.len() {
                let (dx, dy, dz) = OFFSETS[i];
                let nx = node.x + dx;
                let ny = node.y + dy;
                let nz = node.z + dz;
                if Self::in_bounds(nx, ny, nz) {
                    let id = self.get_block_id(nx, ny, nz);
                    if is_transparent(id) {
                        let opacity = bfs_opacity(id);
                        let neighbor_light =
                            i32::from(self.get_saved_light_value(SKY_LIGHT, nx, ny, nz));
                        let new_light = current_light - opacity;
                        if new_light > neighbor_light {
                            let byte = if new_light < 0 {
                                0u8
                            } else if new_light > 15 {
                                15u8
                            } else {
                                new_light as u8
                            };
                            // Direct nibble write without dirtying (mirrors C++
                            // cross-chunk writes that bypass the local flag path
                            // per-neighbour; final dirty state set once below).
                            self.skylight.set_nibble(nx, ny, nz, byte);
                            sky_queue.push_back(LightNode {
                                x: nx,
                                y: ny,
                                z: nz,
                            });
                        }
                    }
                }
                i += 1;
            }
        }

        // STEP 3: blocklight BFS (same order/rules, type = 1).
        while let Some(node) = block_queue.pop_front() {
            let current_light =
                i32::from(self.get_saved_light_value(BLOCK_LIGHT, node.x, node.y, node.z));
            if current_light <= 1 {
                continue;
            }
            const OFFSETS: [(i32, i32, i32); 6] = [
                (-1, 0, 0),
                (1, 0, 0),
                (0, -1, 0),
                (0, 1, 0),
                (0, 0, -1),
                (0, 0, 1),
            ];
            let mut i: usize = 0;
            while i < OFFSETS.len() {
                let (dx, dy, dz) = OFFSETS[i];
                let nx = node.x + dx;
                let ny = node.y + dy;
                let nz = node.z + dz;
                if Self::in_bounds(nx, ny, nz) {
                    let id = self.get_block_id(nx, ny, nz);
                    if is_transparent(id) {
                        let opacity = bfs_opacity(id);
                        let neighbor_light =
                            i32::from(self.get_saved_light_value(BLOCK_LIGHT, nx, ny, nz));
                        let new_light = current_light - opacity;
                        if new_light > neighbor_light {
                            let byte = if new_light < 0 {
                                0u8
                            } else if new_light > 15 {
                                15u8
                            } else {
                                new_light as u8
                            };
                            self.blocklight.set_nibble(nx, ny, nz, byte);
                            block_queue.push_back(LightNode {
                                x: nx,
                                y: ny,
                                z: nz,
                            });
                        }
                    }
                }
                i += 1;
            }
        }

        self.is_modified = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Mirrors of test/TestChunk.cpp (except zlib getChunkData cases) ----

    #[test]
    fn index_formula() {
        let chunk = Chunk::new(0, 0);
        assert_eq!(chunk.get_index(0, 0, 0), 0);
        assert_eq!(chunk.get_index(0, 1, 0), 1);
        assert_eq!(chunk.get_index(1, 0, 0), 1 << 11);
        assert_eq!(chunk.get_index(0, 0, 1), 1 << 7);
        assert_eq!(Chunk::index(0, 0, 0), 0);
        assert_eq!(Chunk::index(1, 0, 0), 1 << 11);
    }

    #[test]
    fn initial_all_air() {
        let chunk = Chunk::new(0, 0);
        for y in 0..16 {
            for x in 0..16 {
                for z in 0..16 {
                    assert_eq!(chunk.get_block_id(x, y, z), 0);
                }
            }
        }
    }

    #[test]
    fn set_and_get_block_id() {
        let mut chunk = Chunk::new(0, 0);
        assert!(chunk.set_block_id(5, 40, 7, 1));
        assert_eq!(chunk.get_block_id(5, 40, 7), 1);
    }

    #[test]
    fn set_block_id_returns_false_for_same() {
        let mut chunk = Chunk::new(0, 0);
        assert!(chunk.set_block_id(3, 20, 3, 1));
        assert!(!chunk.set_block_id(3, 20, 3, 1));
    }

    #[test]
    fn set_block_with_metadata() {
        let mut chunk = Chunk::new(0, 0);
        assert!(chunk.set_block_id_with_metadata(7, 50, 9, 5, 3));
        assert_eq!(chunk.get_block_id(7, 50, 9), 5);
        assert_eq!(chunk.get_block_metadata(7, 50, 9), 3);
    }

    #[test]
    fn set_block_with_metadata_returns_false_for_same() {
        let mut chunk = Chunk::new(0, 0);
        assert!(chunk.set_block_id_with_metadata(7, 50, 9, 5, 3));
        assert!(!chunk.set_block_id_with_metadata(7, 50, 9, 5, 3));
    }

    #[test]
    fn get_block_id_out_of_range() {
        let chunk = Chunk::new(0, 0);
        assert_eq!(chunk.get_block_id(-1, 0, 0), 0);
        assert_eq!(chunk.get_block_id(16, 0, 0), 0);
        assert_eq!(chunk.get_block_id(0, -1, 0), 0);
        assert_eq!(chunk.get_block_id(0, 128, 0), 0);
        assert_eq!(chunk.get_block_id(0, 0, -1), 0);
        assert_eq!(chunk.get_block_id(0, 0, 16), 0);
    }

    #[test]
    fn set_and_get_metadata() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block_metadata(2, 30, 4, 0xF);
        assert_eq!(chunk.get_block_metadata(2, 30, 4), 0xF);
        assert_eq!(chunk.get_block_metadata(2, 31, 4), 0);
    }

    #[test]
    fn height_map_after_set_block() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block_id(4, 60, 4, 1);
        assert!(chunk.get_height_value(4, 4) >= 60);
    }

    #[test]
    fn height_map_default() {
        let chunk = Chunk::new(0, 0);
        assert_eq!(chunk.get_height_value(0, 0), 0);
        assert_eq!(chunk.get_height_value(15, 15), 0);
    }

    #[test]
    fn set_block_preserves_other_blocks() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block_id(0, 10, 0, 1);
        chunk.set_block_id(15, 120, 15, 2);
        assert_eq!(chunk.get_block_id(0, 10, 0), 1);
        assert_eq!(chunk.get_block_id(15, 120, 15), 2);
        assert_eq!(chunk.get_block_id(8, 64, 8), 0);
    }

    #[test]
    fn generate_height_map_all_air() {
        let mut chunk = Chunk::new(0, 0);
        chunk.generate_height_map();
        assert_eq!(chunk.get_height_value(0, 0), 0);
        assert_eq!(chunk.get_height_value(7, 7), 0);
    }

    #[test]
    fn height_map_out_of_range() {
        let chunk = Chunk::new(0, 0);
        assert_eq!(chunk.get_height_value(-1, 0), 0);
        assert_eq!(chunk.get_height_value(0, 16), 0);
    }

    // ---- Own coverage: OOB, heightmap tracking, light ----

    #[test]
    fn oob_set_is_noop_and_stays_clean() {
        let mut chunk = Chunk::new(0, 0);
        assert!(!chunk.set_block_id(-1, 0, 0, 1));
        assert!(!chunk.set_block_id(16, 0, 0, 1));
        assert!(!chunk.set_block_id(0, -1, 0, 1));
        assert!(!chunk.set_block_id(0, 128, 0, 1));
        assert!(!chunk.set_block_id(0, 0, -1, 1));
        assert!(!chunk.set_block_id(0, 0, 16, 1));
        assert!(!chunk.set_block_id_with_metadata(-1, 0, 0, 1, 1));
        assert!(!chunk.set_block_id_with_metadata(0, 200, 0, 1, 1));
        // No successful write happened, so the chunk must still be clean.
        assert!(!chunk.is_modified);
        // Neighbouring in-bounds cell untouched.
        assert_eq!(chunk.get_block_id(0, 0, 0), 0);
    }

    #[test]
    fn oob_metadata_and_light_are_safe() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block_metadata(-1, 0, 0, 0xF);
        chunk.set_block_metadata(0, 200, 0, 0xF);
        assert_eq!(chunk.get_block_metadata(-1, 0, 0), 0);
        assert_eq!(chunk.get_block_metadata(0, 200, 0), 0);
        assert_eq!(chunk.get_block_metadata(0, 0, 0), 0);
        chunk.set_light_value(0, -1, 0, 0, 15);
        chunk.set_light_value(1, 0, 200, 0, 15);
        assert_eq!(chunk.get_saved_light_value(0, -1, 0, 0), 0);
        assert_eq!(chunk.get_saved_light_value(1, 0, 200, 0), 0);
        assert!(!chunk.is_modified);
    }

    #[test]
    fn heightmap_tracks_highest_opaque_after_set() {
        let mut chunk = Chunk::new(0, 0);
        chunk.set_block_id(4, 60, 4, 1);
        assert_eq!(chunk.get_height_value(4, 4), 61);
        // Higher opaque block moves the height up.
        chunk.set_block_id(4, 70, 4, 1);
        assert_eq!(chunk.get_height_value(4, 4), 71);
        // Removing the top block drops back to the lower one.
        chunk.set_block_id(4, 70, 4, 0);
        assert_eq!(chunk.get_height_value(4, 4), 61);
        // Removing the last opaque block returns to zero.
        chunk.set_block_id(4, 60, 4, 0);
        assert_eq!(chunk.get_height_value(4, 4), 0);
        // Transparent blocks (glass, id 20, opacity 0) do not raise height.
        chunk.set_block_id(4, 80, 4, 20);
        assert_eq!(chunk.get_height_value(4, 4), 0);
    }

    #[test]
    fn skylight_all_air_is_full_brightness() {
        let mut chunk = Chunk::new(0, 0);
        chunk.generate_skylight_map();
        assert_eq!(chunk.get_skylight(0, 127, 0), 15);
        assert_eq!(chunk.get_skylight(8, 64, 8), 15);
        assert_eq!(chunk.get_skylight(15, 0, 15), 15);
    }

    #[test]
    fn skylight_blocked_by_stone_floor_wall() {
        // Opaque floor across the whole chunk: light must not pass through.
        let mut chunk = Chunk::new(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block_id(x, 64, z, 1);
            }
        }
        chunk.generate_skylight_map();
        // Just above the floor: full sky.
        assert_eq!(chunk.get_skylight(8, 65, 8), 15);
        // The floor cell itself and everything below: dark.
        assert_eq!(chunk.get_skylight(8, 64, 8), 0);
        assert_eq!(chunk.get_skylight(8, 63, 8), 0);
        assert_eq!(chunk.get_skylight(0, 0, 0), 0);
        // Sideways BFS must not leak around inside the same chunk either:
        // every column is capped, so the bottom layer stays dark.
        for x in 0..16 {
            for z in 0..16 {
                assert_eq!(chunk.get_skylight(x, 0, z), 0);
            }
        }
    }

    #[test]
    fn skylight_attenuates_through_leaves_water_glass() {
        // Full layers (no sideways leak): vertical pass 15 - opacity must hold.
        // Leaves opacity 1 -> 14, water opacity 3 -> 12, glass opacity 0 -> 15.
        let mut leaves = Chunk::new(0, 0);
        let mut water = Chunk::new(0, 0);
        let mut glass = Chunk::new(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                leaves.set_block_id(x, 100, z, 18);
                water.set_block_id(x, 100, z, 8);
                glass.set_block_id(x, 100, z, 20);
            }
        }
        leaves.generate_skylight_map();
        water.generate_skylight_map();
        glass.generate_skylight_map();
        assert_eq!(leaves.get_skylight(8, 101, 8), 15);
        assert_eq!(water.get_skylight(8, 101, 8), 15);
        assert_eq!(glass.get_skylight(8, 101, 8), 15);
        assert_eq!(leaves.get_skylight(8, 100, 8), 14);
        assert_eq!(water.get_skylight(8, 100, 8), 12);
        assert_eq!(glass.get_skylight(8, 100, 8), 15);
        assert_eq!(leaves.get_skylight(8, 99, 8), 14);
        assert_eq!(water.get_skylight(8, 99, 8), 12);
    }

    #[test]
    fn blocklight_without_emitters_stays_zero() {
        // Plain air chunk must produce zero blocklight; a torch must seed
        // emission (light_value 14) through the shared BFS rules.
        let mut chunk = Chunk::new(0, 0);
        chunk.generate_skylight_map();
        assert_eq!(chunk.get_blocklight(8, 64, 8), 0);
        assert_eq!(chunk.get_blocklight(0, 127, 0), 0);
        chunk.set_block_id(8, 64, 8, 50);
        chunk.generate_skylight_map();
        assert_eq!(chunk.get_blocklight(8, 64, 8), 14);
    }

    #[test]
    fn successful_write_marks_modified() {
        let mut chunk = Chunk::new(0, 0);
        assert!(!chunk.is_modified);
        assert!(chunk.set_block_id(1, 2, 3, 1));
        assert!(chunk.is_modified);
        chunk.clear_modified();
        assert!(!chunk.is_modified);
        chunk.set_block_metadata(1, 2, 3, 5);
        assert!(chunk.is_modified);
    }

    // ---- Packet51 map payload (mirrors Chunk::getChunkData) ----

    #[test]
    fn map_raw_layout_order() {
        let mut chunk = Chunk::new(0, 0);
        // (0,0,0) -> flat index 0, meta low nibble of byte 0.
        chunk.set_block_id_with_metadata(0, 0, 0, 7, 0xA);
        // (0,1,0) -> flat index 1, meta high nibble of byte 0.
        chunk.set_block_metadata(0, 1, 0, 0xB);
        chunk.set_light_value(BLOCK_LIGHT, 0, 0, 0, 5);
        chunk.set_light_value(SKY_LIGHT, 0, 1, 0, 12);
        let raw = chunk.map_raw();
        assert_eq!(raw.len(), Chunk::MAP_RAW_LEN);
        assert_eq!(Chunk::MAP_RAW_LEN, 32768 + 3 * 16384);
        // Blocks first.
        assert_eq!(raw[Chunk::index(0, 0, 0)], 7);
        assert_eq!(raw[Chunk::index(5, 5, 5)], 0);
        // Packed metadata second: byte 0 = high 0xB, low 0xA.
        assert_eq!(raw[CHUNK_VOLUME], 0xBA);
        // Blocklight third, skylight fourth (same nibble packing).
        assert_eq!(raw[CHUNK_VOLUME + CHUNK_NIBBLE_BYTES] & 0xF, 5);
        assert_eq!((raw[CHUNK_VOLUME + 2 * CHUNK_NIBBLE_BYTES] >> 4) & 0xF, 12);
    }

    #[test]
    fn map_compressed_roundtrips_through_zlib() {
        use std::io::Read;
        let mut chunk = Chunk::new(3, -2);
        chunk.set_block_id_with_metadata(4, 60, 4, 1, 3);
        chunk.set_light_value(BLOCK_LIGHT, 4, 60, 4, 9);
        chunk.set_light_value(SKY_LIGHT, 4, 61, 4, 15);
        let raw = chunk.map_raw();
        let compressed = chunk.map_compressed();
        assert!(!compressed.is_empty());
        assert!(compressed.len() < raw.len());
        let mut decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        assert_eq!(back, raw);
    }

    #[test]
    fn map_compressed_empty_chunk_roundtrips() {
        use std::io::Read;
        let chunk = Chunk::new(0, 0);
        let compressed = chunk.map_compressed();
        let mut decoder = flate2::read::ZlibDecoder::new(compressed.as_slice());
        let mut back = Vec::new();
        decoder.read_to_end(&mut back).unwrap();
        assert_eq!(back, chunk.map_raw());
    }

    #[test]
    fn test_map_compressed_caching_and_invalidation() {
        let mut chunk = Chunk::new(0, 0);
        // Initially cache is empty
        assert!(chunk.cached_map_compressed.lock().unwrap().is_none());

        // First call populates cache
        let comp1 = chunk.map_compressed();
        assert!(chunk.cached_map_compressed.lock().unwrap().is_some());

        // Second call matches cached
        let comp2 = chunk.map_compressed();
        assert_eq!(comp1, comp2);

        // Setting a block invalidates cache
        chunk.set_block_id(1, 10, 1, 1);
        assert!(chunk.cached_map_compressed.lock().unwrap().is_none());
        let comp3 = chunk.map_compressed();
        assert_ne!(comp1, comp3);

        // Setting light invalidates cache
        chunk.set_light_value(SKY_LIGHT, 1, 10, 1, 14);
        assert!(chunk.cached_map_compressed.lock().unwrap().is_none());

        // generate_skylight_map invalidates cache
        let _ = chunk.map_compressed();
        assert!(chunk.cached_map_compressed.lock().unwrap().is_some());
        chunk.generate_skylight_map();
        assert!(chunk.cached_map_compressed.lock().unwrap().is_none());

        // with_map_compressed reads without cloning
        let len = chunk.with_map_compressed(|data| data.len());
        assert!(len > 0);
        assert!(chunk.cached_map_compressed.lock().unwrap().is_some());

        // clear_compressed_cache frees memory
        chunk.clear_compressed_cache();
        assert!(chunk.cached_map_compressed.lock().unwrap().is_none());

        // blocks_mut invalidates cache
        let _ = chunk.map_compressed();
        assert!(chunk.cached_map_compressed.lock().unwrap().is_some());
        chunk.blocks_mut()[0] = 7;
        assert!(chunk.cached_map_compressed.lock().unwrap().is_none());
        assert_eq!(chunk.blocks()[0], 7);
    }
}
