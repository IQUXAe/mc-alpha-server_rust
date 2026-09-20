//! Native world state: storage, clock and spawn point (mirrors Java `World`
//! storage semantics). Chunks reuse the `chunk` module, block facts come from
//! the `block` table, and materials from `material`.
//!
//! MAP OF THE WORLD MODULE TREE (`impl World`, one responsibility each):
//! - `mod` (this file) — `World` struct, block/material/light access,
//!   collision boxes, shared ids/helpers.
//! - `physics` — body movement, items, falling blocks, boats.
//! - `living` — damage pipeline, death/drops, living+player ticks.
//! - `ai` — sensing, targeting, pathing, mob/animal ticks.
//! - `combat` — line of sight, mob attacks, explosions, arrows, TNT.
//! - `spawning` — spawn fitness, hostile/passive passes, world tick.
//! - `blocks` — placement, neighbor updates, drops, tick scheduling.
//! - `gen` — on-demand chunk generation/population.
//! - `tiles` — `TileData`, entity string-id helpers.
//! - `tests` — unit tests (same module-tree access).
//!
//! Fields and helpers marked `pub(crate)` are module-tree-internal state
//! shared between the submodules (same pattern as the `player_*`
//! and `entity_*` splits); behavior is unchanged from the single file.

pub mod ai;
pub mod blocks;
pub mod combat;
pub mod gen;
pub mod living;
pub mod physics;
pub mod pos;
pub mod spawning;
pub mod tiles;

use crate::aabb::AxisAlignedBB;
use crate::block::table::{block_properties_get, BlockMaterial, BlockType};
use crate::chunk::Chunk;
use crate::entity::table::{EntityId, EntityTable};
use crate::material::Material;
use crate::random::JavaRandom;
use crate::tracker::Tracker;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const WORLD_HEIGHT: i32 = 128;

/// Per-phase wall-clock breakdown of the last [`World::tick_world`], in
/// microseconds. Read-only diagnostics (perf harness, lag warnings).
#[derive(Clone, Copy, Debug, Default)]
pub struct TickStats {
    pub spawners_us: u64,
    pub furnaces_us: u64,
    pub scheduled_us: u64,
    pub random_us: u64,
    pub entities_us: u64,
    pub pickup_us: u64,
    pub light_us: u64,
    pub total_us: u64,
}

/// Alpha grass block id (C++ `Block::grass->blockID`); shared by AI
/// path-weights and the animal spawn fitness check.
pub(crate) const GRASS_BLOCK_ID: u8 = 2;

/// Block ids with no collision box (mirrors the `null` returns from
/// `getCollisionBoundingBoxFromPool`: fluids, plants, torches, saplings,
/// crops, fire by type, plus rails/plates/buttons/signs/snow/reed/portal
/// by id — all `null` in Java but `Normal` in our table).
pub(crate) fn has_collision_box(block_type: u8) -> bool {
    !matches!(
        block_type,
        x if x == BlockType::Fluid as u8
            || x == BlockType::Flower as u8
            || x == BlockType::TallGrass as u8
            || x == BlockType::Mushroom as u8
            || x == BlockType::Torch as u8
            || x == BlockType::Sapling as u8
            || x == BlockType::Crops as u8
            || x == BlockType::Fire as u8
    )
}

/// Id-level no-collision extras (Java `null` boxes our table types as Normal).
pub(crate) fn has_collision_id(bid: u8) -> bool {
    !matches!(bid, 55 | 63 | 65 | 66 | 68 | 69 | 70 | 72 | 75 | 76 | 77 | 78 | 83 | 90)
}

/// True when a block id has air material (mirrors the `== &Material::air`
/// identity check: `Material` compares by capability flags, so all-false
/// materials like circuits would alias `Material::AIR` — compare the
/// material id byte instead).
pub(crate) fn is_air_material(bid: u8) -> bool {
    block_properties_get(bid as u32).material == BlockMaterial::Air as u8
}

/// Material for a block-table material id (mirrors `materialFromId`).
pub fn material_of(material_id: u8) -> Material {
    match material_id {
        x if x == BlockMaterial::Air as u8 => Material::AIR,
        x if x == BlockMaterial::Ground as u8 => Material::GROUND,
        x if x == BlockMaterial::Wood as u8 => Material::WOOD,
        x if x == BlockMaterial::Rock as u8 => Material::ROCK,
        x if x == BlockMaterial::Iron as u8 => Material::IRON,
        x if x == BlockMaterial::Water as u8 => Material::WATER,
        x if x == BlockMaterial::Lava as u8 => Material::LAVA,
        x if x == BlockMaterial::Leaves as u8 => Material::LEAVES,
        x if x == BlockMaterial::Plants as u8 => Material::PLANTS,
        x if x == BlockMaterial::Sponge as u8 => Material::SPONGE,
        x if x == BlockMaterial::Cloth as u8 => Material::CLOTH,
        x if x == BlockMaterial::Fire as u8 => Material::FIRE,
        x if x == BlockMaterial::Sand as u8 => Material::SAND,
        x if x == BlockMaterial::Circuits as u8 => Material::CIRCUITS,
        x if x == BlockMaterial::Glass as u8 => Material::GLASS,
        x if x == BlockMaterial::Tnt as u8 => Material::TNT,
        x if x == BlockMaterial::Ice as u8 => Material::ICE,
        x if x == BlockMaterial::Snow as u8 => Material::SNOW,
        x if x == BlockMaterial::BuiltSnow as u8 => Material::BUILT_SNOW,
        x if x == BlockMaterial::Cactus as u8 => Material::CACTUS,
        x if x == BlockMaterial::Clay as u8 => Material::CLAY,
        x if x == BlockMaterial::Pumpkin as u8 => Material::PUMPKIN,
        x if x == BlockMaterial::Portal as u8 => Material::PORTAL,
        _ => Material::AIR,
    }
}
pub struct World {
    pub seed: i64,
    pub time: i64,
    pub spawn: [i32; 3],
    /// Level name from `level-name` (used for `level.dat` LevelName).
    pub level_name: String,
    /// Peaceful/easy/normal/hard (0..3, mirrors server difficulty;
    /// scales mob-vs-player damage; default normal like the C++ server).
    pub difficulty: i32,
    /// Spawn switches (mirror `isSpawnMonsters/isSpawnAnimals`).
    pub spawn_monsters: bool,
    pub spawn_animals: bool,
    /// Scheduled block updates keyed by (time, x, y, z, id) like the Java
    /// `scheduledTickTreeSet` (the id is part of the entry identity, same
    /// as `scheduledTickSet`).
    pub(crate) scheduled: BTreeMap<(i64, i32, i32, i32, u8), ()>,
    /// Membership mirror of `scheduled` for O(1) dedup by (x, y, z, id)
    /// (Java `scheduledTickSet`); the BTreeMap scan per schedule was
    /// quadratic under fluid/fire load and stalled the tick loop.
    pub(crate) scheduled_set: std::collections::HashSet<(i32, i32, i32, u8)>,
    /// Leaves-decay search guard (mirrors the singleton `BlockLeaves`
    /// instance field threaded through the decay drivers).
    pub(crate) leaves_guard: i32,
    /// Chunk unload radius in chunks (mirrors view distance + 2).
    pub unload_radius: i32,
    /// Unloaded chunks with their entity spill (mirrors the leveldb
    /// round-trip: evicted here, thawed back on recall; disk eviction
    /// arrives with the persistence slice).
    pub(crate) unloaded: HashMap<(i32, i32), Chunk>,
    /// Block-entity storage by cell (mirrors the chunk `TileEntity` map;
    /// furnaces tick in [`World::tick_furnaces`], NBT here).
    pub tiles: HashMap<(i32, i32, i32), TileData>,
    /// Cells changed since the last server tick (id or metadata writes
    /// through [`World::set_block_id`] / [`World::set_block_meta`], plus
    /// tree growth via the world accessor); the server tick drains these
    /// and fans out block changes to chunk-loaded players.
    pub block_updates: Vec<[i32; 3]>,
    /// Full pickups since the last server tick as `(item, player)` pairs
    /// (mirrors the collect packet in `EntityPlayerMP.onUpdate`); the
    /// server tick drains these into `Packet22Collect` fan-out plus an
    /// inventory sync for the picker. Recorded only on full takes, like
    /// vanilla (partial merges leave the item down with no packet).
    pub item_pickups: Vec<(EntityId, EntityId)>,
    /// Deaths since the last server tick (mirrors the status-3 broadcast
    /// in `EntityLiving.onDeath`); the server tick drains these before
    /// the tracker retires the rows, so the animation precedes destroy.
    pub death_events: Vec<EntityId>,
    /// Generic entity statuses since the last tick as `(id, status)`
    /// (mirrors `WorldServer.func_9206_a`): creeper fuse 4/5, etc.
    /// Drained like `death_events` before the tracker tick.
    pub status_events: Vec<(EntityId, i8)>,
    /// Primed TNT pending blasts as `(x, y, z, ticks_left)` (our stand-in
    /// for `EntityTNTPrimed`: the block is already air, the blast lands at
    /// radius 4 when the fuse runs out — 80 ticks hand-lit, 10..30 chained).
    pub pending_tnt: Vec<(i32, i32, i32, i32)>,
    /// Per-tick cache of live player positions for despawn distance checks
    /// (rebuilt once per tick; per-mob scans allocated a Vec each and made
    /// the mob pass quadratic).
    pub(crate) player_pos_cache: (i64, Vec<[f64; 3]>),
    /// Chunks with stale skylight after id writes through
    /// [`World::set_block_id`]; the end of the world tick regenerates each
    /// once instead of once per write (lava/water storms write thousands
    /// of cells per tick).
    pub(crate) light_dirty: HashSet<(i32, i32)>,
    /// Population guard (mirrors `World::isPopulating`): decoration
    /// sets bypass skylight regen (the write-back regenerates explicitly
    /// instead).
    pub(crate) populating: bool,
    /// Terrain generator, built lazily (eleven octave tables; tests that
    /// never generate pay nothing; skipped in `Debug` dumps).
    pub(crate) generator: Option<crate::generator::ChunkProvider>,
    pub(crate) chunks: HashMap<(i32, i32), Chunk>,
    pub entities: EntityTable,
    pub tracker: Tracker,
    pub(crate) rng: JavaRandom,
    /// Breakdown of the last [`World::tick_world`] by phase.
    pub last_tick_stats: TickStats,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("seed", &self.seed)
            .field("time", &self.time)
            .field("spawn", &self.spawn)
            .field("difficulty", &self.difficulty)
            .field("chunks", &self.chunks.len())
            .field("entities", &self.entities.len())
            .field("scheduled", &self.scheduled.len())
            .finish_non_exhaustive()
    }
}

impl Default for World {
    fn default() -> Self {
        World::new(0)
    }
}

impl World {
    pub fn new(seed: i64) -> Self {
        World {
            seed,
            time: 0,
            spawn: [0, 64, 0],
            level_name: "world".to_string(),
            difficulty: 2,
            spawn_monsters: true,
            spawn_animals: true,
            scheduled: BTreeMap::new(),
            scheduled_set: std::collections::HashSet::new(),
            leaves_guard: 0,
            unload_radius: 10,
            unloaded: HashMap::new(),
            tiles: HashMap::new(),
            block_updates: Vec::new(),
            item_pickups: Vec::new(),
            death_events: Vec::new(),
            status_events: Vec::new(),
            pending_tnt: Vec::new(),
            player_pos_cache: (-1, Vec::new()),
            light_dirty: HashSet::new(),
            populating: false,
            generator: None,
            chunks: HashMap::new(),
            entities: EntityTable::new(),
            tracker: Tracker::new(),
            rng: JavaRandom::new(seed),
            last_tick_stats: TickStats::default(),
        }
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert((chunk.x_position, chunk.z_position), chunk);
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Crate-visible chunk lookup for persistence.
    pub(crate) fn chunk_ref(&self, cx: i32, cz: i32) -> Option<&Chunk> {
        self.chunks.get(&(cx, cz)).or_else(|| self.unloaded.get(&(cx, cz)))
    }

    /// Crate-visible mutable chunk lookup (the server clears the
    /// save-dirty flag after flushing a chunk to the store).
    pub(crate) fn chunk_ref_mut(&mut self, cx: i32, cz: i32) -> Option<&mut Chunk> {
        if self.chunks.contains_key(&(cx, cz)) {
            self.chunks.get_mut(&(cx, cz))
        } else {
            self.unloaded.get_mut(&(cx, cz))
        }
    }

    /// Loaded chunk coordinates (mirrors the `chunks_` snapshot at the
    /// head of the C++ `saveWorld`).
    pub(crate) fn loaded_chunk_coords(&self) -> Vec<(i32, i32)> {
        self.chunks.keys().copied().collect()
    }

    /// Reseed the world RNG stream from the current seed (mirrors the C++
    /// world loading its seed before use; `load_level_from` calls this so
    /// a loaded world does not keep the constructor stream).
    pub(crate) fn reseed(&mut self) {
        self.rng = JavaRandom::new(self.seed);
    }

    /// Crate-visible RNG draws (drivers share the world stream).
    pub(crate) fn rng_next_int(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        self.rng.next_int_bound(bound)
    }

    pub(crate) fn rng_next_f64(&mut self) -> f64 {
        self.rng.next_double()
    }

    pub(crate) fn rng_next_f32(&mut self) -> f32 {
        self.rng.next_float()
    }

    pub(crate) fn rng_next_u64(&mut self) -> u64 {
        self.rng.next_long() as u64
    }

    pub fn has_chunk(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&(cx, cz))
    }

    pub(crate) fn chunk_of(x: i32, z: i32) -> (i32, i32, i32, i32) {
        (x.div_euclid(16), z.div_euclid(16), x.rem_euclid(16), z.rem_euclid(16))
    }

    pub fn get_block_id(&self, x: i32, y: i32, z: i32) -> u8 {
        Self::block_id_in(&self.chunks, x, y, z)
    }

    /// Chunk-map half of [`World::get_block_id`]: split out so AI closures
    /// can borrow the map while the RNG field is borrowed mutably elsewhere
    /// (disjoint field borrows; same formula, one flow).
    pub(crate) fn block_id_in(chunks: &HashMap<(i32, i32), Chunk>, x: i32, y: i32, z: i32) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return 0;
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        chunks.get(&(cx, cz)).map(|c| c.get_block_id(lx, y, lz)).unwrap_or(0)
    }

    /// Chunk-map half of [`World::get_block_meta`] (same split as
    /// `block_id_in`; also reused by the decorator tree accessor).
    pub(crate) fn block_meta_in(chunks: &HashMap<(i32, i32), Chunk>, x: i32, y: i32, z: i32) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return 0;
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        chunks.get(&(cx, cz)).map(|c| c.get_block_metadata(lx, y, lz)).unwrap_or(0)
    }

    /// Chunk-map half of [`World::set_block_meta`] (same split as
    /// `block_meta_in`; also reused by the decorator tree accessor).
    pub(crate) fn set_block_meta_in(
        chunks: &mut HashMap<(i32, i32), Chunk>,
        x: i32,
        y: i32,
        z: i32,
        meta: u8,
    ) -> bool {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return false;
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        chunks
            .get_mut(&(cx, cz))
            .map(|c| {
                c.set_block_metadata(lx, y, lz, meta);
                true
            })
            .unwrap_or(false)
    }

    /// Chunk-map half of [`World::set_block_id`]: the `populating` flag
    /// travels explicitly so the decorator tree accessor shares one flow.
    /// Skylight regen is the caller's job (`regen = true` for immediate
    /// single writes like tree growth; the world tick coalesces bulk
    /// writes and regenerates each dirty chunk once via
    /// [`World::refresh_light`]).
    pub(crate) fn set_block_id_in(
        chunks: &mut HashMap<(i32, i32), Chunk>,
        populating: bool,
        x: i32,
        y: i32,
        z: i32,
        id: u8,
        regen: bool,
    ) -> bool {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return false;
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        let changed =
            chunks.get_mut(&(cx, cz)).map(|c| c.set_block_id(lx, y, lz, id)).unwrap_or(false);
        if changed && !populating && regen {
            if let Some(c) = chunks.get_mut(&(cx, cz)) {
                c.generate_skylight_map();
            }
        }
        changed
    }

    /// Chunk-map half of [`World::is_solid`] (same split; the id list
    /// mirrors `isBlockSolidNoChunkLoad` exactly).
    pub(crate) fn is_solid_in(chunks: &HashMap<(i32, i32), Chunk>, x: i32, y: i32, z: i32) -> bool {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return false;
        }
        !matches!(
            Self::block_id_in(chunks, x, y, z),
            0 | 8 | 9 | 10 | 11 | 78 | 37 | 38 | 39 | 40 | 83 | 51 | 6
        )
    }

    /// Chunk-map half of [`World::get_height_value`] (same split).
    pub(crate) fn height_in(chunks: &HashMap<(i32, i32), Chunk>, x: i32, z: i32) -> i32 {
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        chunks.get(&(cx, cz)).map(|c| c.get_height_value(lx, lz)).unwrap_or(0)
    }

    pub fn get_block_meta(&self, x: i32, y: i32, z: i32) -> u8 {
        Self::block_meta_in(&self.chunks, x, y, z)
    }

    /// Missing chunk or out-of-range Y: no-op returning false (mirrors the
    /// NoChunkLoad setters swallowing silently). Every real change lands
    /// in [`World::block_updates`] for broadcast and marks its chunk in
    /// `light_dirty`; skylight itself regenerates once per chunk in
    /// [`World::refresh_light`] at the end of the world tick.
    pub fn set_block_id(&mut self, x: i32, y: i32, z: i32, id: u8) -> bool {
        if Self::set_block_id_in(&mut self.chunks, self.populating, x, y, z, id, false) {
            self.block_updates.push([x, y, z]);
            if !self.populating {
                let (cx, cz, _, _) = Self::chunk_of(x, z);
                self.light_dirty.insert((cx, cz));
            }
            true
        } else {
            false
        }
    }

    /// Regenerate skylight once per dirty chunk. Runs at the end of the
    /// world tick so chunk packets go out with fresh light.
    pub(crate) fn refresh_light(&mut self) {
        for (cx, cz) in std::mem::take(&mut self.light_dirty) {
            if let Some(c) = self.chunks.get_mut(&(cx, cz)) {
                c.generate_skylight_map();
            }
        }
    }

    pub fn set_block_meta(&mut self, x: i32, y: i32, z: i32, meta: u8) -> bool {
        if Self::set_block_meta_in(&mut self.chunks, x, y, z, meta) {
            self.block_updates.push([x, y, z]);
            true
        } else {
            false
        }
    }

    /// Drain queued block changes, deduplicated (one packet per cell per
    /// tick no matter how many writes hit it).
    pub(crate) fn take_block_updates(&mut self) -> Vec<[i32; 3]> {
        let mut out = std::mem::take(&mut self.block_updates);
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn material_at(&self, x: i32, y: i32, z: i32) -> Material {
        Self::material_in(&self.chunks, x, y, z)
    }

    /// Chunk-map half of [`World::material_at`] (see `block_id_in`).
    pub(crate) fn material_in(chunks: &HashMap<(i32, i32), Chunk>, x: i32, y: i32, z: i32) -> Material {
        let id = Self::block_id_in(chunks, x, y, z);
        if id == 0 {
            return Material::AIR;
        }
        material_of(block_properties_get(id as u32).material)
    }

    pub fn is_solid(&self, x: i32, y: i32, z: i32) -> bool {
        // ID list mirrors World::isBlockSolidNoChunkLoad exactly
        // (NOT material-based: torches and the like count as solid here).
        Self::is_solid_in(&self.chunks, x, y, z)
    }

    pub fn is_water(&self, x: i32, y: i32, z: i32) -> bool {
        self.material_at(x, y, z) == Material::WATER
    }

    pub fn is_lava(&self, x: i32, y: i32, z: i32) -> bool {
        self.material_at(x, y, z) == Material::LAVA
    }

    pub fn get_height_value(&self, x: i32, z: i32) -> i32 {
        Self::height_in(&self.chunks, x, z)
    }

    /// Saved light by type (mirrors `World::getSavedLightValue`:
    /// 0 = sky, 1 = block). Out-of-range and missing chunks read 0,
    /// except above the world where sky reads 15.
    pub fn saved_light_value(&self, kind: u8, x: i32, y: i32, z: i32) -> u8 {
        Self::saved_light_in(&self.chunks, kind, x, y, z)
    }

    /// Chunk-map half of [`World::saved_light_value`] (see `block_id_in`).
    pub(crate) fn saved_light_in(chunks: &HashMap<(i32, i32), Chunk>, kind: u8, x: i32, y: i32, z: i32) -> u8 {
        if y < 0 {
            return 0;
        }
        if y >= WORLD_HEIGHT {
            return if kind == 0 { 15 } else { 0 };
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        chunks
            .get(&(cx, cz))
            .map(|c| c.get_saved_light_value(kind as i32, lx, y, lz))
            .unwrap_or(0)
    }

    /// Combined light (mirrors `World::getBlockLightValue`).
    pub fn block_light_value(&self, x: i32, y: i32, z: i32) -> u8 {
        Self::block_light_in(&self.chunks, x, y, z)
    }

    /// Chunk-map half of [`World::block_light_value`] (see `block_id_in`).
    pub(crate) fn block_light_in(chunks: &HashMap<(i32, i32), Chunk>, x: i32, y: i32, z: i32) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return 0;
        }
        Self::saved_light_in(chunks, 0, x, y, z).max(Self::saved_light_in(chunks, 1, x, y, z))
    }

    /// Collision boxes of blocks overlapping `mask` (mirrors
    /// `World::getCollidingBoundingBoxes` over loaded chunks only).
    pub fn colliding_boxes(&self, mask: &AxisAlignedBB) -> Vec<AxisAlignedBB> {
        let mut out = Vec::new();
        let min_bx = mask.min_x.floor() as i32;
        let max_bx = mask.max_x.floor() as i32;
        let min_by = mask.min_y.floor() as i32;
        let max_by = mask.max_y.floor() as i32;
        let min_bz = mask.min_z.floor() as i32;
        let max_bz = mask.max_z.floor() as i32;
        for x in min_bx..=max_bx {
            for y in min_by..=max_by {
                for z in min_bz..=max_bz {
                    let id = self.get_block_id(x, y, z);
                    if id == 0 {
                        continue;
                    }
                    let props = block_properties_get(id as u32);
                    if !has_collision_box(props.block_type) || !has_collision_id(id) {
                        continue;
                    }
                    let bb = AxisAlignedBB::get_bounding_box(
                        x as f64 + props.min_x as f64,
                        y as f64 + props.min_y as f64,
                        z as f64 + props.min_z as f64,
                        x as f64 + props.max_x as f64,
                        y as f64 + props.max_y as f64,
                        z as f64 + props.max_z as f64,
                    );
                    if mask.intersects_with(&bb) {
                        out.push(bb);
                    }
                }
            }
        }
        out
    }

    /// Closest living-or-not player within range, like `getClosestPlayer`
    /// (strict `<`, first minimum wins; ids resolved by the caller).
    pub fn closest_player(&self, x: f64, y: f64, z: f64, max_dist: f64) -> Option<EntityId> {
        let mut best: Option<(EntityId, f64)> = None;
        let mut ids: Vec<EntityId> = self.entities.alive_ids();
        ids.sort_unstable();
        for id in ids {
            let e = self.entities.get(id)?;
            if !matches!(e, crate::entity::table::Entity::Player(_)) {
                continue;
            }
            let d = e.body().distance_sq(x, y, z);
            if d < max_dist * max_dist && best.map(|(_, b)| d < b).unwrap_or(true) {
                best = Some((id, d));
            }
        }
        best.map(|(id, _)| id)
    }
}

/// Block types C++ reports as replaceable (mirrors the `isReplaceable`
/// overrides: flower, tall grass, torch, reed, sapling, crops — notably
/// NOT mushroom, cactus, leaves, soil, or fluids).
pub fn is_replaceable(block_id: u8) -> bool {
    if block_id == 0 {
        return false;
    }
    matches!(
        block_properties_get(block_id as u32).block_type,
        x if x == BlockType::Flower as u8
            || x == BlockType::TallGrass as u8
            || x == BlockType::Torch as u8
            || x == BlockType::Reed as u8
            || x == BlockType::Sapling as u8
            || x == BlockType::Crops as u8
    )
}

pub use self::tiles::TileData;
pub(crate) use self::tiles::{animal_kind_of, animal_string_id, mob_kind_of, mob_string_id, pending_creature};

#[cfg(test)]
mod tests;
