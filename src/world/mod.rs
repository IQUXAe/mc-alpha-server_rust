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

/// World explosion event payload `(x, y, z, radius, destroyed_cells)`.
pub type ExplosionEvent = (f64, f64, f64, f32, Vec<(i32, i32, i32)>);

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
    !matches!(bid, 55 | 63 | 66 | 68 | 69 | 70 | 72 | 75 | 76 | 77 | 78 | 83 | 90)
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
    /// Current skylight subtraction (0..=11, mirrors `World.skylightSubtracted`).
    pub skylight_subtracted: u8,
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
    pub(crate) unloaded: HashMap<(i32, i32), Box<Chunk>>,
    /// Block-entity storage by cell (mirrors the chunk `TileEntity` map;
    /// furnaces tick in [`World::tick_furnaces`], NBT here).
    pub tiles: WorldTiles,
    /// Cells changed since the last server tick (id or metadata writes
    /// through [`World::set_block_id`] / [`World::set_block_meta`], plus
    /// tree growth via the world accessor); the server tick drains these
    /// and fans out block changes to chunk-loaded players.
    pub block_updates: Vec<[i32; 3]>,
    /// Tile entities changed since the last server tick (e.g. furnace smelt
    /// completion or burn change); drained by the server tick to broadcast
    /// `Packet59ComplexEntity`.
    pub tile_updates: Vec<[i32; 3]>,
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
    /// Entity velocity impulses since the last tick as `(id, motion)`
    /// (mirrors `EntityTrackerEntry` `field_9078_E` -> `Packet28` broadcast).
    pub velocity_events: Vec<(EntityId, [f64; 3])>,
    /// Explosions since the last tick as `(x, y, z, radius, cells)`
    /// (mirrors `WorldServer.func_12015_a` -> `Packet60` broadcast).
    pub explosion_events: Vec<ExplosionEvent>,
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
    /// Redstone torch flip timestamps `(x, y, z, world_time)` for the
    /// 8-flips-in-100-ticks burnout rule (`BlockRedstoneTorch.torchUpdates`).
    pub(crate) torch_burnouts: Vec<(i32, i32, i32, i64)>,
    /// Population guard (mirrors `World::isPopulating`): decoration
    /// sets bypass skylight regen (the write-back regenerates explicitly
    /// instead).
    pub(crate) populating: bool,
    /// Terrain generator, built lazily (eleven octave tables; tests that
    /// never generate pay nothing; skipped in `Debug` dumps).
    pub(crate) generator: Option<crate::generator::ChunkProvider>,
    pub(crate) chunks: HashMap<(i32, i32), Box<Chunk>>,
    pub entities: EntityTable,
    pub tracker: Tracker,
    pub(crate) rng: JavaRandom,
    /// Breakdown of the last [`World::tick_world`] by phase.
    pub last_tick_stats: TickStats,
    /// Scratch buffer for spawn anchor coordinate projections across ticks.
    pub(crate) spawn_anchors: (Vec<f64>, Vec<f64>, Vec<f64>),
    /// Scratch buffer for collecting and sorting player positions in spawn passes.
    pub(crate) spawn_anchor_rows: Vec<(EntityId, [f64; 3])>,
    /// Scratch buffer for random block tick chunk coordinates across ticks.
    pub(crate) random_tick_keys: Vec<(i32, i32)>,
    /// Scratch buffer for alive entity IDs in tick_world across ticks.
    pub(crate) tick_ids: Vec<EntityId>,
    /// Scratch buffer for item IDs in pickup_items across ticks.
    pub(crate) pickup_items_scratch: Vec<EntityId>,
    /// Scratch buffer for player IDs in pickup_items across ticks.
    pub(crate) pickup_players_scratch: Vec<EntityId>,
    /// Scratch buffer for candidate entities in push_neighbors across ticks.
    pub(crate) push_others_scratch: Vec<(EntityId, f64, f64)>,
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
            skylight_subtracted: 0,
            spawn: [0, 64, 0],
            level_name: "world".to_string(),
            difficulty: 2,
            spawn_monsters: true,
            spawn_animals: true,
            scheduled: BTreeMap::new(),
            scheduled_set: std::collections::HashSet::new(),
            leaves_guard: 0,
            unload_radius: 12,
            unloaded: HashMap::new(),
            tiles: WorldTiles::new(),
            block_updates: Vec::new(),
            tile_updates: Vec::new(),
            item_pickups: Vec::new(),
            death_events: Vec::new(),
            status_events: Vec::new(),
            velocity_events: Vec::new(),
            explosion_events: Vec::new(),
            pending_tnt: Vec::new(),
            player_pos_cache: (-1, Vec::new()),
            light_dirty: HashSet::new(),
            torch_burnouts: Vec::new(),
            populating: false,
            generator: None,
            chunks: HashMap::new(),
            entities: EntityTable::new(),
            tracker: Tracker::new(),
            rng: JavaRandom::new(seed),
            last_tick_stats: TickStats::default(),
            spawn_anchors: (Vec::new(), Vec::new(), Vec::new()),
            spawn_anchor_rows: Vec::new(),
            random_tick_keys: Vec::new(),
            tick_ids: Vec::new(),
            pickup_items_scratch: Vec::new(),
            pickup_players_scratch: Vec::new(),
            push_others_scratch: Vec::new(),
        }
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) {
        self.chunks.insert((chunk.x_position, chunk.z_position), Box::new(chunk));
    }

    pub fn insert_chunk_boxed(&mut self, chunk: Box<Chunk>) {
        self.chunks.insert((chunk.x_position, chunk.z_position), chunk);
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Crate-visible chunk lookup for persistence.
    pub(crate) fn chunk_ref(&self, cx: i32, cz: i32) -> Option<&Chunk> {
        self.chunks
            .get(&(cx, cz))
            .map(|c| c.as_ref())
            .or_else(|| self.unloaded.get(&(cx, cz)).map(|c| c.as_ref()))
    }

    /// Crate-visible mutable chunk lookup (the server clears the
    /// save-dirty flag after flushing a chunk to the store).
    pub(crate) fn chunk_ref_mut(&mut self, cx: i32, cz: i32) -> Option<&mut Chunk> {
        if self.chunks.contains_key(&(cx, cz)) {
            self.chunks.get_mut(&(cx, cz)).map(|c| c.as_mut())
        } else {
            self.unloaded.get_mut(&(cx, cz)).map(|c| c.as_mut())
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
    pub(crate) fn block_id_in(chunks: &HashMap<(i32, i32), Box<Chunk>>, x: i32, y: i32, z: i32) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return 0;
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        chunks.get(&(cx, cz)).map(|c| c.get_block_id(lx, y, lz)).unwrap_or(0)
    }

    /// Chunk-map half of [`World::get_block_meta`] (same split as
    /// `block_id_in`; also reused by the decorator tree accessor).
    pub(crate) fn block_meta_in(chunks: &HashMap<(i32, i32), Box<Chunk>>, x: i32, y: i32, z: i32) -> u8 {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return 0;
        }
        let (cx, cz, lx, lz) = Self::chunk_of(x, z);
        chunks.get(&(cx, cz)).map(|c| c.get_block_metadata(lx, y, lz)).unwrap_or(0)
    }

    /// Chunk-map half of [`World::set_block_meta`] (same split as
    /// `block_meta_in`; also reused by the decorator tree accessor).
    pub(crate) fn set_block_meta_in(
        chunks: &mut HashMap<(i32, i32), Box<Chunk>>,
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
        chunks: &mut HashMap<(i32, i32), Box<Chunk>>,
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
    pub(crate) fn is_solid_in(chunks: &HashMap<(i32, i32), Box<Chunk>>, x: i32, y: i32, z: i32) -> bool {
        if !(0..WORLD_HEIGHT).contains(&y) {
            return false;
        }
        !matches!(
            Self::block_id_in(chunks, x, y, z),
            0 | 8 | 9 | 10 | 11 | 78 | 37 | 38 | 39 | 40 | 83 | 51 | 6
        )
    }

    /// Chunk-map half of [`World::get_height_value`] (same split).
    pub(crate) fn height_in(chunks: &HashMap<(i32, i32), Box<Chunk>>, x: i32, z: i32) -> i32 {
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

    /// Regenerate skylight once per dirty chunk and propagate light across
    /// loaded chunk borders. Runs at the end of the world tick so chunk
    /// packets go out with fresh light.
    pub(crate) fn refresh_light(&mut self) {
        let dirty = std::mem::take(&mut self.light_dirty);
        if dirty.is_empty() {
            return;
        }
        let mut regen_set = dirty.clone();
        for &(cx, cz) in &dirty {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    if self.chunks.contains_key(&(cx + dx, cz + dz)) {
                        regen_set.insert((cx + dx, cz + dz));
                    }
                }
            }
        }
        for &(cx, cz) in &regen_set {
            if let Some(c) = self.chunks.get_mut(&(cx, cz)) {
                c.generate_skylight_map();
            }
        }
        self.propagate_cross_chunk_light(&regen_set);
    }

    /// Propagate sky (0) and block (1) light across shared boundaries of
    /// `chunks_to_check` and their loaded neighbors.
    pub(crate) fn propagate_cross_chunk_light(&mut self, chunks_to_check: &HashSet<(i32, i32)>) {
        use std::collections::VecDeque;
        const OFFSETS: [(i32, i32, i32); 6] = [
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ];
        for kind in [0u8, 1u8] {
            let mut queue: VecDeque<(i32, i32, i32)> = VecDeque::new();
            for &(cx, cz) in chunks_to_check {
                if !self.chunks.contains_key(&(cx, cz)) {
                    continue;
                }
                if self.chunks.contains_key(&(cx + 1, cz)) {
                    let xa = cx * 16 + 15;
                    let xb = xa + 1;
                    for z in (cz * 16)..(cz * 16 + 16) {
                        for y in 0..WORLD_HEIGHT {
                            let la = self.saved_light_value(kind, xa, y, z) as i32;
                            let lb = self.saved_light_value(kind, xb, y, z) as i32;
                            if la > lb + 1 {
                                queue.push_back((xa, y, z));
                            } else if lb > la + 1 {
                                queue.push_back((xb, y, z));
                            }
                        }
                    }
                }
                if self.chunks.contains_key(&(cx - 1, cz))
                    && !chunks_to_check.contains(&(cx - 1, cz))
                {
                    let xa = cx * 16 - 1;
                    let xb = xa + 1;
                    for z in (cz * 16)..(cz * 16 + 16) {
                        for y in 0..WORLD_HEIGHT {
                            let la = self.saved_light_value(kind, xa, y, z) as i32;
                            let lb = self.saved_light_value(kind, xb, y, z) as i32;
                            if la > lb + 1 {
                                queue.push_back((xa, y, z));
                            } else if lb > la + 1 {
                                queue.push_back((xb, y, z));
                            }
                        }
                    }
                }
                if self.chunks.contains_key(&(cx, cz + 1)) {
                    let za = cz * 16 + 15;
                    let zb = za + 1;
                    for x in (cx * 16)..(cx * 16 + 16) {
                        for y in 0..WORLD_HEIGHT {
                            let la = self.saved_light_value(kind, x, y, za) as i32;
                            let lb = self.saved_light_value(kind, x, y, zb) as i32;
                            if la > lb + 1 {
                                queue.push_back((x, y, za));
                            } else if lb > la + 1 {
                                queue.push_back((x, y, zb));
                            }
                        }
                    }
                }
                if self.chunks.contains_key(&(cx, cz - 1))
                    && !chunks_to_check.contains(&(cx, cz - 1))
                {
                    let za = cz * 16 - 1;
                    let zb = za + 1;
                    for x in (cx * 16)..(cx * 16 + 16) {
                        for y in 0..WORLD_HEIGHT {
                            let la = self.saved_light_value(kind, x, y, za) as i32;
                            let lb = self.saved_light_value(kind, x, y, zb) as i32;
                            if la > lb + 1 {
                                queue.push_back((x, y, za));
                            } else if lb > la + 1 {
                                queue.push_back((x, y, zb));
                            }
                        }
                    }
                }
            }
            while let Some((x, y, z)) = queue.pop_front() {
                let cur = self.saved_light_value(kind, x, y, z) as i32;
                if cur <= 1 {
                    continue;
                }
                for (dx, dy, dz) in OFFSETS {
                    let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                    if !(0..WORLD_HEIGHT).contains(&ny) {
                        continue;
                    }
                    let (ncx, ncz, lx, lz) = Self::chunk_of(nx, nz);
                    if !self.chunks.contains_key(&(ncx, ncz)) {
                        continue;
                    }
                    let id = self.get_block_id(nx, ny, nz);
                    let op = block_properties_get(id as u32).light_opacity;
                    if op >= 15 {
                        continue;
                    }
                    let step = op.max(1);
                    let new_light = cur - step;
                    if new_light > self.saved_light_value(kind, nx, ny, nz) as i32 {
                        if let Some(c) = self.chunks.get_mut(&(ncx, ncz)) {
                            c.set_light_value(kind as i32, lx, ny, lz, new_light as u8);
                        }
                        queue.push_back((nx, ny, nz));
                    }
                }
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

    /// Drain queued tile entity changes, deduplicated.
    pub(crate) fn take_tile_updates(&mut self) -> Vec<[i32; 3]> {
        let mut out = std::mem::take(&mut self.tile_updates);
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Calculate skylight subtracted for `self.time` (`0..=11`), mirroring
    /// `World::calculateSkylightSubtracted(1.0F)` and `WorldProvider::func_4089_a`.
    pub fn calculate_skylight_subtracted(&self) -> u8 {
        let t = self.time.rem_euclid(24000) as f32;
        let mut angle = (t + 1.0) / 24000.0 - 0.25;
        if angle < 0.0 {
            angle += 1.0;
        }
        if angle > 1.0 {
            angle -= 1.0;
        }
        let prev = angle;
        angle = 1.0 - (((angle as f64 * std::f64::consts::PI).cos() + 1.0) / 2.0) as f32;
        angle = prev + (angle - prev) / 3.0;
        let mut sub = 1.0
            - (crate::math_helper::cos(angle * std::f32::consts::PI * 2.0) * 2.0 + 0.5);
        sub = sub.clamp(0.0, 1.0);
        (sub * 11.0) as u8
    }

    /// Update `self.skylight_subtracted` from `self.time` (mirrors
    /// `World::calculateInitialSkylight` and the per-tick update in `World::tick`).
    pub fn update_skylight_subtracted(&mut self) {
        self.skylight_subtracted = self.calculate_skylight_subtracted();
    }

    pub fn material_at(&self, x: i32, y: i32, z: i32) -> Material {
        Self::material_in(&self.chunks, x, y, z)
    }

    /// Chunk-map half of [`World::material_at`] (see `block_id_in`).
    pub(crate) fn material_in(chunks: &HashMap<(i32, i32), Box<Chunk>>, x: i32, y: i32, z: i32) -> Material {
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
    pub(crate) fn saved_light_in(chunks: &HashMap<(i32, i32), Box<Chunk>>, kind: u8, x: i32, y: i32, z: i32) -> u8 {
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

    /// Combined light (mirrors `World::getBlockLightValue` with `skylightSubtracted`).
    pub fn block_light_value(&self, x: i32, y: i32, z: i32) -> u8 {
        Self::block_light_sub_in(&self.chunks, self.skylight_subtracted, x, y, z)
    }

    /// Chunk-map half of [`World::block_light_value`] without skylight subtraction.
    #[allow(dead_code)]
    pub(crate) fn block_light_in(chunks: &HashMap<(i32, i32), Box<Chunk>>, x: i32, y: i32, z: i32) -> u8 {
        Self::block_light_sub_in(chunks, 0, x, y, z)
    }

    fn block_light_raw_in(
        chunks: &HashMap<(i32, i32), Box<Chunk>>,
        sky_sub: u8,
        x: i32,
        y: i32,
        z: i32,
    ) -> u8 {
        if y < 0 {
            return 0;
        }
        if y >= WORLD_HEIGHT {
            return 15u8.saturating_sub(sky_sub);
        }
        Self::saved_light_in(chunks, 0, x, y, z)
            .saturating_sub(sky_sub)
            .max(Self::saved_light_in(chunks, 1, x, y, z))
    }

    /// Chunk-map half of [`World::block_light_value`] with `sky_sub` subtracted from sky light.
    pub(crate) fn block_light_sub_in(
        chunks: &HashMap<(i32, i32), Box<Chunk>>,
        sky_sub: u8,
        x: i32,
        y: i32,
        z: i32,
    ) -> u8 {
        let id = Self::block_id_in(chunks, x, y, z);
        if id == 44 || id == 60 {
            let l_up = Self::block_light_raw_in(chunks, sky_sub, x, y + 1, z);
            let l_px = Self::block_light_raw_in(chunks, sky_sub, x + 1, y, z);
            let l_nx = Self::block_light_raw_in(chunks, sky_sub, x - 1, y, z);
            let l_pz = Self::block_light_raw_in(chunks, sky_sub, x, y, z + 1);
            let l_nz = Self::block_light_raw_in(chunks, sky_sub, x, y, z - 1);
            return l_up.max(l_px).max(l_nx).max(l_pz).max(l_nz);
        }
        Self::block_light_raw_in(chunks, sky_sub, x, y, z)
    }

    /// Compute the collision bounding box for block `id` at `(x, y, z)`,
    /// handling metadata-dependent door rotation (`BlockDoor.func_273_b`),
    /// farmland full height, and ladder wall orientation (`BlockLadder`).
    pub(crate) fn block_collision_box(&self, x: i32, y: i32, z: i32, id: u8) -> AxisAlignedBB {
        if id == 60 {
            return AxisAlignedBB::get_bounding_box(
                x as f64,
                y as f64,
                z as f64,
                x as f64 + 1.0,
                y as f64 + 1.0,
                z as f64 + 1.0,
            );
        }
        if id == 65 {
            let meta = self.get_block_meta(x, y, z);
            let f = 0.125;
            let (min_x, min_z, max_x, max_z) = match meta {
                2 => (0.0, 1.0 - f, 1.0, 1.0),
                3 => (0.0, 0.0, 1.0, f),
                4 => (1.0 - f, 0.0, 1.0, 1.0),
                5 => (0.0, 0.0, f, 1.0),
                _ => (0.0, 0.0, 1.0, 1.0),
            };
            return AxisAlignedBB::get_bounding_box(
                x as f64 + min_x,
                y as f64,
                z as f64 + min_z,
                x as f64 + max_x,
                y as f64 + 1.0,
                z as f64 + max_z,
            );
        }
        if id == 64 || id == 71 {
            let meta = self.get_block_meta(x, y, z) as i32;
            let state = if (meta & 4) == 0 {
                (meta - 1) & 3
            } else {
                meta & 3
            };
            let t = 3.0 / 16.0;
            let (min_x, min_z, max_x, max_z) = match state {
                0 => (0.0, 0.0, 1.0, t),
                1 => (1.0 - t, 0.0, 1.0, 1.0),
                2 => (0.0, 1.0 - t, 1.0, 1.0),
                _ => (0.0, 0.0, t, 1.0),
            };
            return AxisAlignedBB::get_bounding_box(
                x as f64 + min_x,
                y as f64,
                z as f64 + min_z,
                x as f64 + max_x,
                y as f64 + 1.0,
                z as f64 + max_z,
            );
        }
        let props = block_properties_get(id as u32);
        AxisAlignedBB::get_bounding_box(
            x as f64 + props.min_x as f64,
            y as f64 + props.min_y as f64,
            z as f64 + props.min_z as f64,
            x as f64 + props.max_x as f64,
            y as f64 + props.max_y as f64,
            z as f64 + props.max_z as f64,
        )
    }

    /// Compute one or two collision bounding boxes for block `id` at `(x, y, z)`,
    /// matching `BlockStairs.getCollidingBoundingBoxes` for stairs (53, 67).
    pub(crate) fn block_collision_boxes(&self, x: i32, y: i32, z: i32, id: u8, out: &mut Vec<AxisAlignedBB>) {
        if id == 53 || id == 67 {
            let meta = self.get_block_meta(x, y, z) as i32;
            let fx = x as f64;
            let fy = y as f64;
            let fz = z as f64;
            match meta {
                0 => {
                    // Ascending East
                    out.push(AxisAlignedBB::get_bounding_box(fx, fy, fz, fx + 0.5, fy + 0.5, fz + 1.0));
                    out.push(AxisAlignedBB::get_bounding_box(fx + 0.5, fy, fz, fx + 1.0, fy + 1.0, fz + 1.0));
                }
                1 => {
                    // Ascending West
                    out.push(AxisAlignedBB::get_bounding_box(fx, fy, fz, fx + 0.5, fy + 1.0, fz + 1.0));
                    out.push(AxisAlignedBB::get_bounding_box(fx + 0.5, fy, fz, fx + 1.0, fy + 0.5, fz + 1.0));
                }
                2 => {
                    // Ascending South
                    out.push(AxisAlignedBB::get_bounding_box(fx, fy, fz, fx + 1.0, fy + 0.5, fz + 0.5));
                    out.push(AxisAlignedBB::get_bounding_box(fx, fy, fz + 0.5, fx + 1.0, fy + 1.0, fz + 1.0));
                }
                3 => {
                    // Ascending North
                    out.push(AxisAlignedBB::get_bounding_box(fx, fy, fz, fx + 1.0, fy + 1.0, fz + 0.5));
                    out.push(AxisAlignedBB::get_bounding_box(fx, fy, fz + 0.5, fx + 1.0, fy + 0.5, fz + 1.0));
                }
                _ => {
                    out.push(AxisAlignedBB::get_bounding_box(fx, fy, fz, fx + 1.0, fy + 1.0, fz + 1.0));
                }
            }
            return;
        }
        out.push(self.block_collision_box(x, y, z, id));
    }

    /// Collision boxes of blocks overlapping `mask` (mirrors
    /// `World::getCollidingBoundingBoxes` over loaded chunks only).
    pub fn colliding_boxes(&self, mask: &AxisAlignedBB) -> Vec<AxisAlignedBB> {
        let mut out = Vec::new();
        if !mask.min_x.is_finite()
            || !mask.max_x.is_finite()
            || !mask.min_y.is_finite()
            || !mask.max_y.is_finite()
            || !mask.min_z.is_finite()
            || !mask.max_z.is_finite()
        {
            return out;
        }
        // min_by starts at floor(min_y) - 1 so blocks with max_y > 1.0 (such as fences with 1.5)
        // below the player are checked, as in World.java:799.
        let min_by = (mask.min_y.floor() as i32 - 1).max(0);
        let max_by = (mask.max_y.floor() as i32).min(WORLD_HEIGHT - 1);
        if min_by > max_by {
            return out;
        }
        let cx = ((mask.min_x + mask.max_x) * 0.5).floor() as i32;
        let cz = ((mask.min_z + mask.max_z) * 0.5).floor() as i32;
        let min_bx = (mask.min_x.floor() as i32).max(cx.saturating_sub(16));
        let max_bx = (mask.max_x.floor() as i32).min(cx.saturating_add(16));
        let min_bz = (mask.min_z.floor() as i32).max(cz.saturating_sub(16));
        let max_bz = (mask.max_z.floor() as i32).min(cz.saturating_add(16));
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
                    if id == 53 || id == 67 {
                        let mut stairs_boxes = Vec::with_capacity(2);
                        self.block_collision_boxes(x, y, z, id, &mut stairs_boxes);
                        for bb in stairs_boxes {
                            if mask.intersects_with(&bb) {
                                out.push(bb);
                            }
                        }
                    } else {
                        let bb = self.block_collision_box(x, y, z, id);
                        if mask.intersects_with(&bb) {
                            out.push(bb);
                        }
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
        let max_dist_sq = max_dist * max_dist;
        for (&id, e) in self.entities.iter() {
            if e.body().dead || !matches!(e, crate::entity::table::Entity::Player(_)) {
                continue;
            }
            let d = e.body().distance_sq(x, y, z);
            if d < max_dist_sq {
                match best {
                    None => best = Some((id, d)),
                    Some((best_id, best_d)) => {
                        if d < best_d || (d == best_d && id < best_id) {
                            best = Some((id, d));
                        }
                    }
                }
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

pub use self::tiles::{TileData, WorldTiles};
pub(crate) use self::tiles::{animal_kind_of, animal_string_id, mob_kind_of, mob_string_id, pending_creature};

#[cfg(test)]
mod tests;
