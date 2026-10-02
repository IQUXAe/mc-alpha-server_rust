//! Native persistence: gzip-NBT chunk blobs (byte-identical schema to the
//! legacy `c.*.dat` files and the LevelDB values), `level.dat`, player
//! files, and the LevelDB chunk store via `rusty-leveldb`.
//!
//! Tag schema mirrors the legacy chunk/player/`level.dat` codecs
//! field-for-field; round-trip tests plus a live-DB decode prove parity.
//!
//! Deliberate scope notes:
//! - Tile ticking (furnaces) is the server slice's; storage rides here.
//! - `savedHeldItemId` round-trips through the file like C++ but only
//!   feeds the login path (network slice), not entity state.
//! - Autosave timing belongs to the server loop; this module offers the
//!   primitives.

use std::collections::BTreeMap;
use std::io::Write;

use crate::chunk::{
    Chunk, PendingBoat, PendingCreature, PendingItem, PendingMinecart, CHUNK_AREA, CHUNK_NIBBLE_BYTES, CHUNK_VOLUME,
};
use crate::entity::table::Entity;
use crate::inventory::ItemStack;
use crate::nbt::{NbtCompound, NbtList, NbtTag, read_root, write_root};
use crate::tile_entity::sign::SIGN_LINES;
use crate::world::{TileData, World};

// ---- chunk keys (our own LevelDB layout: high u32 = x, low u32 = z) ----

/// LevelDB key bytes for a chunk (little-endian u64).
/// NOTE: this intentionally differs from the in-memory `server::chunk_key`
/// (low = x); the store never shares keys with the network layer.
pub fn chunk_key_bytes(cx: i32, cz: i32) -> [u8; 8] {
    (((cx as u32 as u64) << 32) | (cz as u32 as u64)).to_le_bytes()
}

// ---- LevelDB chunk store ----

/// LevelDB-backed chunk blob store (chunk key -> gzip NBT value).
pub struct ChunkStore {
    db: rusty_leveldb::DB,
}

impl ChunkStore {
    /// Default LevelDB options tuned for embedded / low-memory router profiles:
    /// 512 KB write buffer and 512 KB block cache instead of default 4MB + 4MB.
    pub fn default_options() -> rusty_leveldb::Options {
        let write_buffer_size = std::env::var("ALPHA_LEVELDB_WRITE_BUFFER_KB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|kb| kb * 1024)
            .unwrap_or(512 * 1024);
        let block_cache_capacity_bytes = std::env::var("ALPHA_LEVELDB_CACHE_KB")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|kb| kb * 1024)
            .unwrap_or(512 * 1024);
        rusty_leveldb::Options {
            create_if_missing: true,
            write_buffer_size,
            block_cache_capacity_bytes,
            ..Default::default()
        }
    }

    /// Open (creating) the database at `path` (the `db` directory itself).
    pub fn open(path: &str) -> Result<Self, String> {
        Self::open_with_options(path, Self::default_options())
    }

    /// Open (creating) the database at `path` with custom LevelDB options.
    pub fn open_with_options(path: &str, opts: rusty_leveldb::Options) -> Result<Self, String> {
        rusty_leveldb::DB::open(path, opts)
            .map(|db| Self { db })
            .map_err(|e| e.to_string())
    }

    pub fn put_chunk(&mut self, cx: i32, cz: i32, blob: &[u8]) -> Result<(), String> {
        self.db.put(&chunk_key_bytes(cx, cz), blob).map_err(|e| e.to_string())
    }

    /// Raw blob bytes, or `None` when the chunk was never stored.
    pub fn get_chunk(&mut self, cx: i32, cz: i32) -> Option<Vec<u8>> {
        self.db.get(&chunk_key_bytes(cx, cz)).map(|b| b.to_vec())
    }

    pub fn flush(&mut self) -> Result<(), String> {
        self.db.flush().map_err(|e| e.to_string())
    }
}

// ---- gzip helpers (flate2 default level, like the existing codecs) ----

fn gzip(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut encoder =
        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).ok()?;
    encoder.finish().ok()
}

fn gunzip(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut out = Vec::new();
    // Valid chunk blobs decompress to ~100KB; cap before OOM on zip bombs.
    decoder
        .take((MAX_DECOMPRESSED + 1) as u64)
        .read_to_end(&mut out)
        .ok()?;
    if out.is_empty() || out.len() > MAX_DECOMPRESSED {
        return None;
    }
    Some(out)
}

/// Max decompressed bytes for one chunk/player blob (valid data is
/// ~100KB; the cap only rejects zip bombs / corrupt values).
pub const MAX_DECOMPRESSED: usize = 8 * 1024 * 1024;

/// Blob decompression with magic dispatch (mirrors `decompressChunkData`:
/// zstd frames for modern values, gzip otherwise).
fn decompress_blob(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() >= 4 {
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&bytes[..4]);
        if u32::from_le_bytes(magic) == 0xFD2FB528 {
            use std::io::Read;
            match zstd::stream::Decoder::new(bytes) {
                Ok(dec) => {
                    let mut out = Vec::new();
                    dec.take((MAX_DECOMPRESSED + 1) as u64)
                        .read_to_end(&mut out)
                        .ok()?;
                    return if out.is_empty() || out.len() > MAX_DECOMPRESSED {
                        None
                    } else {
                        Some(out)
                    };
                }
                Err(_) => return None,
            }
        }
    }
    gunzip(bytes)
}

// ---- small NBT field readers (mirror the load-side defaults) ----

fn get_int(m: &BTreeMap<String, NbtTag>, k: &str) -> i32 {
    match m.get(k) {
        Some(NbtTag::Int(v)) => *v,
        _ => 0,
    }
}

fn get_short(m: &BTreeMap<String, NbtTag>, k: &str) -> i16 {
    match m.get(k) {
        Some(NbtTag::Short(v)) => *v,
        _ => 0,
    }
}

fn get_long(m: &BTreeMap<String, NbtTag>, k: &str) -> i64 {
    match m.get(k) {
        Some(NbtTag::Long(v)) => *v,
        _ => 0,
    }
}

fn get_byte(m: &BTreeMap<String, NbtTag>, k: &str) -> i8 {
    match m.get(k) {
        Some(NbtTag::Byte(v)) => *v,
        _ => 0,
    }
}

fn get_float(m: &BTreeMap<String, NbtTag>, k: &str) -> f32 {
    match m.get(k) {
        Some(NbtTag::Float(v)) => *v,
        _ => 0.0,
    }
}

fn get_double(m: &BTreeMap<String, NbtTag>, k: &str) -> f64 {
    match m.get(k) {
        Some(NbtTag::Double(v)) => *v,
        _ => 0.0,
    }
}

fn get_string(m: &BTreeMap<String, NbtTag>, k: &str) -> String {
    match m.get(k) {
        Some(NbtTag::String(v)) => v.clone(),
        _ => String::new(),
    }
}

fn get_doubles(m: &BTreeMap<String, NbtTag>, k: &str) -> [f64; 3] {
    match m.get(k) {
        Some(NbtTag::List(l)) if l.elements.len() >= 3 => [
            match l.elements[0] {
                NbtTag::Double(v) => v,
                _ => 0.0,
            },
            match l.elements[1] {
                NbtTag::Double(v) => v,
                _ => 0.0,
            },
            match l.elements[2] {
                NbtTag::Double(v) => v,
                _ => 0.0,
            },
        ],
        _ => [0.0; 3],
    }
}

fn get_floats2(m: &BTreeMap<String, NbtTag>, k: &str) -> (f32, f32) {
    match m.get(k) {
        Some(NbtTag::List(l)) if l.elements.len() >= 2 => (
            match l.elements[0] {
                NbtTag::Float(v) => v,
                _ => 0.0,
            },
            match l.elements[1] {
                NbtTag::Float(v) => v,
                _ => 0.0,
            },
        ),
        _ => (0.0, 0.0),
    }
}

fn dbl_list(v: [f64; 3]) -> NbtTag {
    NbtTag::List(NbtList {
        tag_type: 6,
        elements: vec![NbtTag::Double(v[0]), NbtTag::Double(v[1]), NbtTag::Double(v[2])],
    })
}

fn rot_list(yaw: f32, pitch: f32) -> NbtTag {
    NbtTag::List(NbtList {
        tag_type: 5,
        elements: vec![NbtTag::Float(yaw), NbtTag::Float(pitch)],
    })
}

fn write_stack(m: &mut BTreeMap<String, NbtTag>, s: &ItemStack) {
    m.insert("id".to_string(), NbtTag::Short(s.item_id as i16));
    m.insert("Count".to_string(), NbtTag::Byte(s.count as i8));
    m.insert("Damage".to_string(), NbtTag::Short(s.damage as i16));
}

pub(crate) fn read_stack(m: &BTreeMap<String, NbtTag>) -> ItemStack {
    ItemStack::new(
        get_short(m, "id") as i32,
        get_byte(m, "Count") as i32,
        get_short(m, "Damage") as i32,
    )
}

// ---- chunk blob encode ----

/// Encode one loaded chunk (blocks, light, tiles, spill, and live rows in
/// the chunk) to an NBT blob, gzip- or zstd-compressed like
/// `chunk_nbt_serialize` (zstd for LevelDB values, gzip for legacy
/// files). Live rows mirror the C++ save gather (pending spill plus
/// entities standing in the chunk).
pub fn encode_chunk_blob(world: &World, cx: i32, cz: i32, zstd: bool) -> Option<Vec<u8>> {
    let chunk = world.chunk_ref(cx, cz)?;
    let mut blocks = [0u8; CHUNK_VOLUME];
    let mut meta = [0u8; CHUNK_VOLUME];
    chunk.fill_arrays(&mut blocks, &mut meta);
    let mut meta_packed = [0u8; CHUNK_NIBBLE_BYTES];
    for (i, b) in meta.iter().enumerate() {
        if i % 2 == 0 {
            meta_packed[i / 2] = b & 0xF;
        } else {
            meta_packed[i / 2] |= (b & 0xF) << 4;
        }
    }
    let mut sky = [0u8; CHUNK_NIBBLE_BYTES];
    let mut light = [0u8; CHUNK_NIBBLE_BYTES];
    let mut height = [0u8; CHUNK_AREA];
    chunk.export_light_maps(&mut sky, &mut light, &mut height);

    let mut level = BTreeMap::new();
    level.insert("xPos".to_string(), NbtTag::Int(cx));
    level.insert("zPos".to_string(), NbtTag::Int(cz));
    level.insert("LastUpdate".to_string(), NbtTag::Long(world.time));
    level.insert("Blocks".to_string(), NbtTag::ByteArray(blocks.to_vec()));
    level.insert("Data".to_string(), NbtTag::ByteArray(meta_packed.to_vec()));
    level.insert("SkyLight".to_string(), NbtTag::ByteArray(sky.to_vec()));
    level.insert("BlockLight".to_string(), NbtTag::ByteArray(light.to_vec()));
    level.insert("HeightMap".to_string(), NbtTag::ByteArray(height.to_vec()));
    level.insert(
        "TerrainPopulated".to_string(),
        NbtTag::Byte(if chunk.is_terrain_populated { 1 } else { 0 }),
    );

    // Tiles in this chunk.
    let mut tiles = Vec::new();
    let mut tile_cells: Vec<((i32, i32, i32), TileData)> = world
        .tiles
        .chunk_cells(cx, cz)
        .iter()
        .filter_map(|&pos| world.tiles.get(&pos).map(|&tile| (pos, tile)))
        .collect();
    tile_cells.sort_by_key(|(k, _)| *k);
    for ((x, y, z), tile) in tile_cells {
        tiles.push(NbtTag::Compound(tile_nbt(x, y, z, &tile)));
    }
    level.insert("TileEntities".to_string(), NbtTag::List(NbtList { tag_type: 10, elements: tiles }));

    // Entities: spill lists plus live rows standing in the chunk.
    let mut entities = Vec::new();
    let in_chunk = |pos: (f64, f64)| {
        (pos.0.floor() as i32).div_euclid(16) == cx && (pos.1.floor() as i32).div_euclid(16) == cz
    };
    let mut item_rows: Vec<(i32, i32, i32, i32, i32, [f64; 3])> = chunk
        .pending_items
        .iter()
        .map(|p| (p.item_id, p.count, p.damage, p.age, p.pickup_delay, p.pos))
        .collect();
    let mut animal_rows: Vec<PendingCreature> = chunk.pending_animals.clone();
    let mut monster_rows: Vec<PendingCreature> = chunk.pending_monsters.clone();
    let mut boat_rows: Vec<PendingBoat> = chunk.pending_boats.clone();
    let mut minecart_rows: Vec<PendingMinecart> = chunk.pending_minecarts.clone();
    let mut live: Vec<Entity> = Vec::new();
    for oid in world.entities.alive_ids() {
        if let Some(e) = world.entities.get(oid) {
            if !e.body().dead && in_chunk((e.body().pos[0], e.body().pos[2])) {
                live.push(e.clone());
            }
        }
    }
    live.sort_by_key(|e| e.id());
    for e in &live {
        match e {
            Entity::Item(it) => item_rows.push((
                it.item_id, it.count, it.damage, it.age, it.pickup_delay, it.body.pos,
            )),
            Entity::Animal(a) => animal_rows.push(PendingCreature {
                string_id: crate::world::animal_string_id(a.kind),
                pos: a.living.body.pos,
                motion: a.living.body.motion,
                yaw: a.living.body.yaw,
                pitch: a.living.body.pitch,
                health: a.living.health,
                max_health: a.living.max_health,
                saddled: a.saddled,
                sheared: a.sheared,
                egg_timer: a.egg_timer,
                slime_size: 1,
                anger: 0,
            }),
            Entity::Mob(m) => monster_rows.push(PendingCreature {
                string_id: crate::world::mob_string_id(m.kind),
                pos: m.living.body.pos,
                motion: m.living.body.motion,
                yaw: m.living.body.yaw,
                pitch: m.living.body.pitch,
                health: m.living.health,
                max_health: m.living.max_health,
                saddled: false,
                sheared: false,
                egg_timer: 0,
                slime_size: m.slime_size,
                anger: m.anger,
            }),
            Entity::Boat(b) => boat_rows.push(PendingBoat {
                pos: b.body.pos,
                motion: b.body.motion,
                yaw: b.body.yaw,
                pitch: b.body.pitch,
                time_since_hit: b.time_since_hit,
                damage_taken: b.damage_taken,
                forward_dir: b.forward_dir,
            }),
            Entity::Minecart(c) => minecart_rows.push(PendingMinecart {
                pos: c.body.pos,
                motion: c.body.motion,
                yaw: c.body.yaw,
                pitch: c.body.pitch,
                cart_type: c.cart_type,
                damage_taken: c.damage_taken,
                time_since_hit: c.time_since_hit,
            }),
            _ => {}
        }
    }
    for (item_id, count, damage, age, delay, pos) in item_rows {
        let mut m = BTreeMap::new();
        m.insert("id".to_string(), NbtTag::String("Item".to_string()));
        m.insert("Pos".to_string(), dbl_list(pos));
        let mut im = BTreeMap::new();
        im.insert("id".to_string(), NbtTag::Short(item_id as i16));
        im.insert("Count".to_string(), NbtTag::Byte(count as i8));
        im.insert("Damage".to_string(), NbtTag::Short(damage as i16));
        m.insert("Item".to_string(), NbtTag::Compound(NbtCompound { map: im }));
        m.insert("Age".to_string(), NbtTag::Short(age as i16));
        m.insert("PickupDelay".to_string(), NbtTag::Short(delay as i16));
        entities.push(NbtTag::Compound(NbtCompound { map: m }));
    }
    for c in animal_rows.iter().chain(monster_rows.iter()) {
        let mut m = BTreeMap::new();
        m.insert("id".to_string(), NbtTag::String(c.string_id.clone()));
        m.insert("Pos".to_string(), dbl_list(c.pos));
        m.insert("Motion".to_string(), dbl_list(c.motion));
        m.insert("Rotation".to_string(), rot_list(c.yaw, c.pitch));
        m.insert("Health".to_string(), NbtTag::Short(c.health));
        m.insert("MaxHealth".to_string(), NbtTag::Short(c.max_health));
        m.insert("Saddle".to_string(), NbtTag::Byte(if c.saddled { 1 } else { 0 }));
        m.insert("Sheared".to_string(), NbtTag::Byte(if c.sheared { 1 } else { 0 }));
        m.insert("EggLayTime".to_string(), NbtTag::Int(c.egg_timer));
        // Slime `Size` (stored size-1) and pig `Anger` ride the same
        // rows vanilla puts them on; other kinds ignore them on read.
        m.insert("Size".to_string(), NbtTag::Int(c.slime_size as i32 - 1));
        m.insert("Anger".to_string(), NbtTag::Short(c.anger as i16));
        entities.push(NbtTag::Compound(NbtCompound { map: m }));
    }
    for b in &boat_rows {
        let mut m = BTreeMap::new();
        m.insert("id".to_string(), NbtTag::String("Boat".to_string()));
        m.insert("Pos".to_string(), dbl_list(b.pos));
        m.insert("Motion".to_string(), dbl_list(b.motion));
        m.insert("Rotation".to_string(), rot_list(b.yaw, b.pitch));
        m.insert("TimeSinceHit".to_string(), NbtTag::Int(b.time_since_hit));
        m.insert("DamageTaken".to_string(), NbtTag::Int(b.damage_taken));
        m.insert("ForwardDirection".to_string(), NbtTag::Int(b.forward_dir));
        entities.push(NbtTag::Compound(NbtCompound { map: m }));
    }
    for c in &minecart_rows {
        let mut m = BTreeMap::new();
        m.insert("id".to_string(), NbtTag::String("Minecart".to_string()));
        m.insert("Pos".to_string(), dbl_list(c.pos));
        m.insert("Motion".to_string(), dbl_list(c.motion));
        m.insert("Rotation".to_string(), rot_list(c.yaw, c.pitch));
        m.insert("Type".to_string(), NbtTag::Int(c.cart_type));
        m.insert("DamageTaken".to_string(), NbtTag::Int(c.damage_taken));
        m.insert("TimeSinceHit".to_string(), NbtTag::Int(c.time_since_hit));
        entities.push(NbtTag::Compound(NbtCompound { map: m }));
    }
    level.insert("Entities".to_string(), NbtTag::List(NbtList { tag_type: 10, elements: entities }));

    let mut root = BTreeMap::new();
    root.insert("Level".to_string(), NbtTag::Compound(NbtCompound { map: level }));
    let mut raw = Vec::new();
    write_root(&mut raw, "", &NbtCompound { map: root }).ok()?;
    if zstd {
        zstd::stream::encode_all(raw.as_slice(), 1).ok()
    } else {
        gzip(&raw)
    }
}

// ---- chunk blob decode ----

/// Owned decode of one chunk blob (spill-shaped, like the NBT lists).
pub struct DecodedChunk {
    pub cx: i32,
    pub cz: i32,
    pub last_update: i64,
    pub populated: bool,
    pub blocks: Vec<u8>,
    pub meta: Vec<u8>,
    pub sky: Vec<u8>,
    pub light: Vec<u8>,
    pub height: Vec<u8>,
    pub furnaces: Vec<((i32, i32, i32), crate::tile_entity::furnace::FurnaceState)>,
    pub chests: Vec<((i32, i32, i32), crate::tile_entity::chest::ChestState)>,
    pub signs: Vec<((i32, i32, i32), crate::tile_entity::sign::SignState)>,
    pub spawners: Vec<((i32, i32, i32), crate::world::tiles::MobSpawnerState)>,
    pub items: Vec<PendingItem>,
    pub animals: Vec<PendingCreature>,
    pub monsters: Vec<PendingCreature>,
    pub boats: Vec<PendingBoat>,
    pub minecarts: Vec<PendingMinecart>,
}

/// Single tile-entity compound (shared by chunk blobs and packet 59).
pub(crate) fn tile_nbt(x: i32, y: i32, z: i32, tile: &TileData) -> NbtCompound {
    let mut m = BTreeMap::new();
    m.insert("x".to_string(), NbtTag::Int(x));
    m.insert("y".to_string(), NbtTag::Int(y));
    m.insert("z".to_string(), NbtTag::Int(z));
    match tile {
        TileData::Furnace(s) => {
            m.insert("id".to_string(), NbtTag::String("Furnace".to_string()));
            m.insert("BurnTime".to_string(), NbtTag::Short(s.burn_time));
            m.insert("CookTime".to_string(), NbtTag::Short(s.cook_time));
            m.insert("ItemBurnTime".to_string(), NbtTag::Short(s.current_item_burn_time));
            let mut items = Vec::new();
            for (i, slot) in s.slots.iter().enumerate() {
                if slot.count > 0 {
                    let mut im = BTreeMap::new();
                    write_stack(&mut im, slot);
                    im.insert("Slot".to_string(), NbtTag::Byte(i as i8));
                    items.push(NbtTag::Compound(NbtCompound { map: im }));
                }
            }
            m.insert("Items".to_string(), NbtTag::List(NbtList { tag_type: 10, elements: items }));
        }
        TileData::Chest(s) => {
            m.insert("id".to_string(), NbtTag::String("Chest".to_string()));
            let mut items = Vec::new();
            for (i, slot) in s.slots.iter().enumerate() {
                if slot.count > 0 {
                    let mut im = BTreeMap::new();
                    write_stack(&mut im, slot);
                    im.insert("Slot".to_string(), NbtTag::Byte(i as i8));
                    items.push(NbtTag::Compound(NbtCompound { map: im }));
                }
            }
            m.insert("Items".to_string(), NbtTag::List(NbtList { tag_type: 10, elements: items }));
        }
        TileData::Sign(s) => {
            m.insert("id".to_string(), NbtTag::String("Sign".to_string()));
            for (i, line) in s.lines.iter().enumerate() {
                let len = line.iter().position(|&c| c == 0).unwrap_or(SIGN_LINES);
                // Vanilla caps sign lines at 15 chars.
                let text = String::from_utf8_lossy(&line[..len.min(15)]).to_string();
                m.insert(format!("Text{}", i + 1), NbtTag::String(text));
            }
        }
        TileData::MobSpawner(s) => {
            m.insert("id".to_string(), NbtTag::String("MobSpawner".to_string()));
            m.insert("EntityId".to_string(), NbtTag::String(s.entity_id_str().to_string()));
            m.insert("Delay".to_string(), NbtTag::Short(s.delay));
        }
    }
    NbtCompound { map: m }
}

fn read_stack_slots(
    m: &BTreeMap<String, NbtTag>,
    out: &mut [ItemStack],
) {
    if let Some(NbtTag::List(l)) = m.get("Items") {
        for elem in &l.elements {
            if let NbtTag::Compound(im) = elem {
                let slot = get_byte(&im.map, "Slot") as usize;
                if slot < out.len() {
                    out[slot] = read_stack(&im.map);
                }
            }
        }
    }
}

/// Decode a gzip NBT blob; `None` on any structural problem (missing
/// arrays fail like the C++ loader, short light maps zero-fill through
/// the length-checked import).
pub fn decode_chunk_blob(bytes: &[u8], cx: i32, cz: i32) -> Option<DecodedChunk> {
    let raw = decompress_blob(bytes)?;
    let mut cursor = std::io::Cursor::new(raw);
    let (_, root) = read_root(&mut cursor).ok()?;
    let level = match root.map.get("Level") {
        Some(NbtTag::Compound(c)) => c,
        _ => return None,
    };
    let bytes_of = |k: &str| -> Option<Vec<u8>> {
        match level.map.get(k) {
            Some(NbtTag::ByteArray(v)) => Some(v.clone()),
            _ => None,
        }
    };
    let blocks = bytes_of("Blocks")?;
    let data = bytes_of("Data")?;
    let sky = bytes_of("SkyLight")?;
    let bl = bytes_of("BlockLight")?;
    if blocks.len() != CHUNK_VOLUME || data.len() != CHUNK_NIBBLE_BYTES {
        return None;
    }
    let height = bytes_of("HeightMap").unwrap_or_else(|| vec![0; CHUNK_AREA]);
    // Unpack data nibbles (even index = low nibble, like NibbleArray).
    let mut meta = vec![0u8; CHUNK_VOLUME];
    for (i, m) in meta.iter_mut().enumerate() {
        *m = (data[i / 2] >> (4 * (i % 2))) & 0xF;
    }

    let mut furnaces = Vec::new();
    let mut chests = Vec::new();
    let mut signs = Vec::new();
    let mut spawners = Vec::new();
    if let Some(NbtTag::List(l)) = level.map.get("TileEntities") {
        for elem in &l.elements {
            let NbtTag::Compound(c) = elem else { continue };
            let (x, y, z) = (get_int(&c.map, "x"), get_int(&c.map, "y"), get_int(&c.map, "z"));
            match get_string(&c.map, "id").as_str() {
                "Furnace" => {
                    let mut s = crate::tile_entity::furnace::furnace_create();
                    s.burn_time = get_short(&c.map, "BurnTime");
                    s.cook_time = get_short(&c.map, "CookTime");
                    s.current_item_burn_time = get_short(&c.map, "ItemBurnTime");
                    read_stack_slots(&c.map, &mut s.slots);
                    if !c.map.contains_key("ItemBurnTime") && s.burn_time > 0 {
                        let slot_fuel =
                            if s.slots[crate::tile_entity::furnace::SLOT_FUEL].count > 0 {
                                crate::tile_entity::furnace::fuel_burn_time(
                                    s.slots[crate::tile_entity::furnace::SLOT_FUEL].item_id,
                                )
                            } else {
                                0
                            };
                        s.current_item_burn_time =
                            crate::tile_entity::furnace::infer_furnace_max_burn_time(
                                s.burn_time,
                                0,
                                slot_fuel,
                            ) as i16;
                    }
                    furnaces.push(((x, y, z), s));
                }
                "Chest" => {
                    let mut s = crate::tile_entity::chest::chest_create();
                    read_stack_slots(&c.map, &mut s.slots);
                    chests.push(((x, y, z), s));
                }
                "Sign" => {
                    let mut s = crate::tile_entity::sign::sign_create();
                    for i in 0..SIGN_LINES {
                        let text = get_string(&c.map, &format!("Text{}", i + 1));
                        crate::tile_entity::sign::sign_set_line(&mut s, i as i32, &text);
                    }
                    signs.push(((x, y, z), s));
                }
                "MobSpawner" => {
                    let entity_id = get_string(&c.map, "EntityId");
                    let mut s = crate::world::tiles::MobSpawnerState::new(if entity_id.is_empty() {
                        "Pig"
                    } else {
                        &entity_id
                    });
                    s.delay = get_short(&c.map, "Delay");
                    spawners.push(((x, y, z), s));
                }
                _ => {}
            }
        }
    }

    let mut items = Vec::new();
    let mut animals = Vec::new();
    let mut monsters = Vec::new();
    let mut boats = Vec::new();
    let mut minecarts = Vec::new();
    if let Some(NbtTag::List(l)) = level.map.get("Entities") {
        for elem in &l.elements {
            let NbtTag::Compound(c) = elem else { continue };
            let id = get_string(&c.map, "id");
            let pos = get_doubles(&c.map, "Pos");
            let motion = get_doubles(&c.map, "Motion");
            let (yaw, pitch) = get_floats2(&c.map, "Rotation");
            match id.as_str() {
                "Item" => {
                    let (item_id, count, damage) = match c.map.get("Item") {
                        Some(NbtTag::Compound(im)) => (
                            get_short(&im.map, "id") as i32,
                            get_byte(&im.map, "Count") as i32,
                            get_short(&im.map, "Damage") as i32,
                        ),
                        _ => continue,
                    };
                    items.push(PendingItem {
                        item_id,
                        count,
                        damage,
                        age: get_short(&c.map, "Age") as i32,
                        pickup_delay: get_short(&c.map, "PickupDelay") as i32,
                        pos,
                    });
                }
                "Boat" => boats.push(PendingBoat {
                    pos,
                    motion,
                    yaw,
                    pitch,
                    time_since_hit: get_int(&c.map, "TimeSinceHit"),
                    damage_taken: get_int(&c.map, "DamageTaken"),
                    forward_dir: get_int(&c.map, "ForwardDirection"),
                }),
                "Minecart" => minecarts.push(PendingMinecart {
                    pos,
                    motion,
                    yaw,
                    pitch,
                    cart_type: get_int(&c.map, "Type"),
                    damage_taken: get_int(&c.map, "DamageTaken"),
                    time_since_hit: get_int(&c.map, "TimeSinceHit"),
                }),
                _ => {
                    let creature = PendingCreature {
                        string_id: id.clone(),
                        pos,
                        motion,
                        yaw,
                        pitch,
                        health: get_short(&c.map, "Health"),
                        max_health: get_short(&c.map, "MaxHealth"),
                        saddled: get_byte(&c.map, "Saddle") != 0,
                        sheared: get_byte(&c.map, "Sheared") != 0,
                        egg_timer: get_int(&c.map, "EggLayTime"),
                        slime_size: (get_int(&c.map, "Size") + 1).clamp(1, 4) as u8,
                        anger: get_short(&c.map, "Anger") as i32,
                    };
                    // Animal ids are the pig-90 family; the rest are mobs
                    // (unknown ids are dropped by the restore step).
                    match id.as_str() {
                        "Pig" | "Sheep" | "Cow" | "Chicken" => animals.push(creature),
                        _ => monsters.push(creature),
                    }
                }
            }
        }
    }

    Some(DecodedChunk {
        cx,
        cz,
        last_update: get_long(&level.map, "LastUpdate"),
        populated: get_byte(&level.map, "TerrainPopulated") != 0,
        blocks,
        meta,
        sky,
        light: bl,
        height,
        furnaces,
        chests,
        signs,
        spawners,
        items,
        animals,
        monsters,
        boats,
        minecarts,
    })
}

// ---- level.dat ----

/// Encode `level.dat` (gzip NBT mirroring `encodeLevelDat`: seed, spawn,
/// time, zero size-on-disk, version 19132, level name).
pub fn encode_level_dat(world: &World) -> Option<Vec<u8>> {
    let mut data = BTreeMap::new();
    data.insert("RandomSeed".to_string(), NbtTag::Long(world.seed));
    data.insert("SpawnX".to_string(), NbtTag::Int(world.spawn[0]));
    data.insert("SpawnY".to_string(), NbtTag::Int(world.spawn[1]));
    data.insert("SpawnZ".to_string(), NbtTag::Int(world.spawn[2]));
    data.insert("Time".to_string(), NbtTag::Long(world.time));
    data.insert("SizeOnDisk".to_string(), NbtTag::Long(0));
    data.insert("version".to_string(), NbtTag::Int(19132));
    data.insert("LevelName".to_string(), NbtTag::String(world.level_name.clone()));
    let mut inner = BTreeMap::new();
    inner.insert("Data".to_string(), NbtTag::Compound(NbtCompound { map: data }));
    let mut raw = Vec::new();
    write_root(&mut raw, "", &NbtCompound { map: inner }).ok()?;
    gzip(&raw)
}

/// Decode `level.dat` into seed/spawn/time (mirrors the load side).
pub fn decode_level_dat(bytes: &[u8]) -> Option<(i64, [i32; 3], i64)> {
    let raw = gunzip(bytes)?;
    let mut cursor = std::io::Cursor::new(raw);
    let (_, root) = read_root(&mut cursor).ok()?;
    let data = match root.map.get("Data") {
        Some(NbtTag::Compound(c)) => c,
        _ => return None,
    };
    Some((
        get_long(&data.map, "RandomSeed"),
        [
            get_int(&data.map, "SpawnX"),
            get_int(&data.map, "SpawnY"),
            get_int(&data.map, "SpawnZ"),
        ],
        get_long(&data.map, "Time"),
    ))
}

// ---- player files ----

/// Decoded player file (slot tags map to banks exactly like
/// `InventoryPlayer::readFromNBT`).
pub struct DecodedPlayer {
    pub username: String,
    pub pos: [f64; 3],
    pub motion: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub fall: f32,
    pub fire: i32,
    pub air: i32,
    pub on_ground: bool,
    pub health: i16,
    pub hurt_time: i16,
    pub death_time: i16,
    pub attack_time: i16,
    pub dimension: i32,
    pub score: i32,
    pub held_item_id: i32,
    pub main: [Option<ItemStack>; 36],
    pub armor: [Option<ItemStack>; 4],
    pub crafting: [Option<ItemStack>; 4],
}

/// Encode one player row to a `.dat` file image (schema mirrors
/// `save_player_data`, including the legacy Pos/Rotation doubles).
pub fn encode_player(world: &World, id: crate::entity::table::EntityId) -> Option<Vec<u8>> {
    let p = match world.entities.get(id) {
        Some(Entity::Player(p)) => p,
        _ => return None,
    };
    let b = &p.living.body;
    let mut root = BTreeMap::new();
    root.insert("Pos".to_string(), dbl_list(b.pos));
    root.insert("PosX".to_string(), NbtTag::Double(b.pos[0]));
    root.insert("PosY".to_string(), NbtTag::Double(b.pos[1]));
    root.insert("PosZ".to_string(), NbtTag::Double(b.pos[2]));
    root.insert("Motion".to_string(), dbl_list(b.motion));
    root.insert("Rotation".to_string(), rot_list(b.yaw, b.pitch));
    root.insert("RotationYaw".to_string(), NbtTag::Float(b.yaw));
    root.insert("RotationPitch".to_string(), NbtTag::Float(b.pitch));
    root.insert("FallDistance".to_string(), NbtTag::Float(b.fall_distance));
    root.insert("Fire".to_string(), NbtTag::Short(b.fire as i16));
    root.insert("Air".to_string(), NbtTag::Short(b.air as i16));
    root.insert("OnGround".to_string(), NbtTag::Byte(if b.on_ground { 1 } else { 0 }));
    root.insert("Health".to_string(), NbtTag::Short(p.living.health));
    root.insert("HurtTime".to_string(), NbtTag::Short(p.living.hurt_time as i16));
    root.insert("DeathTime".to_string(), NbtTag::Short(p.living.death_time as i16));
    root.insert("AttackTime".to_string(), NbtTag::Short(p.living.attack_time as i16));
    root.insert("Dimension".to_string(), NbtTag::Int(b.dimension));
    root.insert("Score".to_string(), NbtTag::Int(p.score));
    root.insert("HeldItemId".to_string(), NbtTag::Int(p.held_item_id));
    let mut inv = Vec::new();
    let mut push_bank = |bank: &[Option<ItemStack>], base: i32| {
        for (i, slot) in bank.iter().enumerate() {
            if let Some(s) = slot {
                if s.count > 0 {
                    let mut im = BTreeMap::new();
                    im.insert("Slot".to_string(), NbtTag::Byte((base + i as i32) as i8));
                    write_stack(&mut im, s);
                    inv.push(NbtTag::Compound(NbtCompound { map: im }));
                }
            }
        }
    };
    push_bank(&p.inventory.main, 0);
    push_bank(&p.inventory.crafting, 80);
    push_bank(&p.inventory.armor, 100);
    root.insert("Inventory".to_string(), NbtTag::List(NbtList { tag_type: 10, elements: inv }));
    let mut raw = Vec::new();
    write_root(&mut raw, "Player", &NbtCompound { map: root }).ok()?;
    gzip(&raw)
}

/// Decode a `.dat` file image (accepts gzip or raw like the C++ loader).
pub fn decode_player(bytes: &[u8], username: &str) -> Option<DecodedPlayer> {
    let raw = gunzip(bytes).or_else(|| {
        if bytes.is_empty() {
            None
        } else {
            Some(bytes.to_vec())
        }
    })?;
    let mut cursor = std::io::Cursor::new(raw);
    let (_, root) = read_root(&mut cursor).ok()?;
    let pos = match root.map.get("Pos") {
        Some(NbtTag::List(_)) => get_doubles(&root.map, "Pos"),
        _ => [get_double(&root.map, "PosX"), get_double(&root.map, "PosY"), get_double(&root.map, "PosZ")],
    };
    let (yaw, pitch) = match root.map.get("Rotation") {
        Some(NbtTag::List(_)) => get_floats2(&root.map, "Rotation"),
        _ => (get_float(&root.map, "RotationYaw"), get_float(&root.map, "RotationPitch")),
    };
    let mut main = [None; 36];
    let mut armor = [None; 4];
    let mut crafting = [None; 4];
    if let Some(NbtTag::List(l)) = root.map.get("Inventory") {
        for elem in &l.elements {
            if let NbtTag::Compound(im) = elem {
                let slot = get_byte(&im.map, "Slot") as u8;
                let stack = read_stack(&im.map);
                if stack.count <= 0 {
                    continue;
                }
                if slot < 36 {
                    main[slot as usize] = Some(stack);
                } else if (80..84).contains(&slot) {
                    crafting[(slot - 80) as usize] = Some(stack);
                } else if (100..104).contains(&slot) {
                    armor[(slot - 100) as usize] = Some(stack);
                }
            }
        }
    }
    Some(DecodedPlayer {
        username: username.to_string(),
        pos,
        motion: get_doubles(&root.map, "Motion"),
        yaw,
        pitch,
        fall: get_float(&root.map, "FallDistance"),
        fire: get_short(&root.map, "Fire") as i32,
        air: get_short(&root.map, "Air") as i32,
        on_ground: get_byte(&root.map, "OnGround") != 0,
        health: get_short(&root.map, "Health"),
        hurt_time: get_short(&root.map, "HurtTime"),
        death_time: get_short(&root.map, "DeathTime"),
        attack_time: get_short(&root.map, "AttackTime"),
        dimension: get_int(&root.map, "Dimension"),
        score: get_int(&root.map, "Score"),
        held_item_id: get_int(&root.map, "HeldItemId"),
        main,
        armor,
        crafting,
    })
}

// ---- high-level world IO ----

impl World {
    /// Save one loaded chunk into the store (zstd value like C++).
    pub fn save_chunk_to(&mut self, store: &mut ChunkStore, cx: i32, cz: i32) -> bool {
        let blob = match encode_chunk_blob(self, cx, cz, true) {
            Some(b) => b,
            None => return false,
        };
        store.put_chunk(cx, cz, &blob).is_ok()
    }

    /// Load one chunk from the store (entities thaw live through the
    /// spill path, tiles replace the chunk's cells).
    pub fn load_chunk_from(&mut self, store: &mut ChunkStore, cx: i32, cz: i32) -> bool {
        let blob = match store.get_chunk(cx, cz) {
            Some(b) => b,
            None => return false,
        };
        let d = match decode_chunk_blob(&blob, cx, cz) {
            Some(d) => d,
            None => return false,
        };
        self.apply_decoded_chunk(d)
    }

    /// Apply a decoded blob: blocks, light, tiles (replacing the chunk's
    /// cells), spill, then thaw.
    pub fn apply_decoded_chunk(&mut self, d: DecodedChunk) -> bool {
        let (cx, cz) = (d.cx, d.cz);
        let blocks: [u8; CHUNK_VOLUME] = match d.blocks.try_into() {
            Ok(b) => b,
            Err(_) => return false,
        };
        let meta: [u8; CHUNK_VOLUME] = match d.meta.try_into() {
            Ok(m) => m,
            Err(_) => return false,
        };
        let mut chunk = Chunk::new(cx, cz);
        chunk.load_arrays(&blocks, &meta);
        if !chunk.load_light_maps(&d.sky, &d.light, &d.height) {
            return false;
        }
        chunk.is_terrain_populated = d.populated;
        chunk.pending_items = d.items;
        chunk.pending_animals = d.animals;
        chunk.pending_monsters = d.monsters;
        chunk.pending_boats = d.boats;
        chunk.pending_minecarts = d.minecarts;
        chunk.clear_modified();
        self.insert_chunk(chunk);
        // Tiles replace this chunk's cells.
        self.tiles.remove_chunk(cx, cz);
        for ((x, y, z), s) in d.furnaces {
            self.tiles.insert((x, y, z), TileData::Furnace(s));
        }
        for ((x, y, z), s) in d.chests {
            self.tiles.insert((x, y, z), TileData::Chest(s));
        }
        for ((x, y, z), s) in d.signs {
            self.tiles.insert((x, y, z), TileData::Sign(s));
        }
        for ((x, y, z), s) in d.spawners {
            self.tiles.insert((x, y, z), TileData::MobSpawner(s));
        }
        self.restore_chunk_entities(cx, cz);
        true
    }

    /// Write `level.dat` into `dir` (atomic rename like the player save).
    pub fn save_level_to(&self, dir: &str) -> bool {
        let blob = match encode_level_dat(self) {
            Some(b) => b,
            None => return false,
        };
        write_atomic(&std::path::Path::new(dir).join("level.dat"), &blob)
    }

    /// Read `level.dat` from `dir` into seed/spawn/time.
    pub fn load_level_from(&mut self, dir: &str) -> bool {
        let bytes = match std::fs::read(std::path::Path::new(dir).join("level.dat")) {
            Ok(b) => b,
            Err(_) => return false,
        };
        let (seed, spawn, time) = match decode_level_dat(&bytes) {
            Some(v) => v,
            None => return false,
        };
        self.seed = seed;
        self.spawn = spawn;
        self.time = time;
        self.update_skylight_subtracted();
        self.reseed();
        true
    }

    /// Write one player's `.dat` file into `dir` (lowercase name, atomic).
    pub fn save_player_to(&self, dir: &str, id: crate::entity::table::EntityId) -> bool {
        let username = match self.entities.get(id) {
            Some(Entity::Player(p)) => p.username.clone(),
            _ => return false,
        };
        let blob = match encode_player(self, id) {
            Some(b) => b,
            None => return false,
        };
        write_atomic(
            &std::path::Path::new(dir).join(format!("{}.dat", username.to_lowercase())),
            &blob,
        )
    }

    /// Read one player's `.dat` file and insert a live row (login path
    /// for the network slice; respawn immunity comes fresh from the
    /// constructor like C++).
    pub fn load_player_from(&mut self, dir: &str, username: &str) -> Option<crate::entity::table::EntityId> {
        let bytes = std::fs::read(
            std::path::Path::new(dir).join(format!("{}.dat", username.to_lowercase())),
        )
        .ok()?;
        let d = decode_player(&bytes, username)?;
        let id = self.entities.alloc_id();
        let mut p = crate::entity::table::PlayerEnt::new(id, &d.username);
        p.living.body.set_position(d.pos[0], d.pos[1], d.pos[2]);
        p.living.body.motion = d.motion;
        p.living.body.yaw = d.yaw;
        p.living.body.pitch = d.pitch;
        p.living.body.fall_distance = d.fall;
        p.living.body.fire = d.fire;
        p.living.body.air = d.air;
        p.living.body.on_ground = d.on_ground;
        p.living.body.dimension = d.dimension;
        p.living.health = d.health;
        p.living.hurt_time = d.hurt_time as i32;
        p.living.death_time = d.death_time as i32;
        p.living.attack_time = d.attack_time as i32;
        p.score = d.score;
        p.held_item_id = d.held_item_id;
        p.inventory.main = d.main;
        p.inventory.armor = d.armor;
        p.inventory.crafting = d.crafting;
        self.entities.insert(Entity::Player(p));
        Some(id)
    }
}

fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> bool {
    let parent = path.parent();
    if let Some(p) = parent {
        if std::fs::create_dir_all(p).is_err() {
            return false;
        }
    }
    let tmp = path.with_extension("dat_tmp");
    let write_ok = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    })()
    .is_ok();

    if !write_ok {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    if let Some(p) = parent {
        if let Ok(dir) = std::fs::File::open(p) {
            let _ = dir.sync_all();
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::table::{AnimalEnt, AnimalKind, MobEnt, MobKind, PlayerEnt};
    use crate::inventory::ItemStack;
    use crate::world::World;

    fn tmp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("opencode_persist_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn stk(item_id: i32, count: i32, damage: i32) -> ItemStack {
        ItemStack::new(item_id, count, damage)
    }

    /// World with a furnished chunk: torch w/ meta, furnace + chest +
    /// sign tiles, a sheep, a zombie, an item, and a stocked player.
    fn furnished_world() -> (World, crate::entity::table::EntityId) {
        let mut w = World::new(99);
        let mut c = Chunk::new(0, 0);
        for x in 0..16 {
            for z in 0..16 {
                c.set_block_id(x, 63, z, 1);
            }
        }
        c.generate_height_map();
        w.insert_chunk(c);
        w.set_block_id(3, 64, 4, 50);
        w.set_block_meta(3, 64, 4, 5);
        // Furnace with fuel+input, chest with dirt, labeled sign.
        let mut f = crate::tile_entity::furnace::furnace_create();
        f.slots[0] = stk(15, 3, 0);
        f.slots[1] = stk(263, 2, 0);
        f.burn_time = 40;
        w.tiles.insert((4, 64, 4), TileData::Furnace(f));
        w.set_block_id(4, 64, 4, 61);
        let mut ch = crate::tile_entity::chest::chest_create();
        ch.slots[0] = stk(3, 7, 0);
        w.tiles.insert((5, 64, 4), TileData::Chest(ch));
        w.set_block_id(5, 64, 4, 54);
        let mut sign = crate::tile_entity::sign::sign_create();
        sign.lines[0][..5].copy_from_slice(b"hello");
        w.tiles.insert((6, 64, 4), TileData::Sign(sign));
        w.set_block_id(6, 64, 4, 63);
        // Sheep (shorn), zombie (hurt), loose item, stocked player.
        let sid = w.entities.alloc_id();
        let mut sheep = AnimalEnt::new(sid, AnimalKind::Sheep);
        sheep.living.body.set_position(7.5, 64.0, 4.5);
        sheep.sheared = true;
        sheep.egg_timer = 1234;
        w.entities.insert(Entity::Animal(sheep));
        let zid = w.entities.alloc_id();
        let mut zombie = MobEnt::new(zid, MobKind::Zombie);
        zombie.living.body.set_position(9.5, 64.0, 4.5);
        zombie.living.health = 11;
        w.entities.insert(Entity::Mob(zombie));
        w.spawn_item_entity(280, 2, 0, 10.5, 64.2, 4.5);
        let mid = w.entities.alloc_id();
        let mut cart = crate::entity::table::MinecartEnt {
            body: crate::entity::table::Body::new(mid, 0.98, 0.7, 0.0),
            cart_type: 0,
            damage_taken: 10,
            time_since_hit: 5,
        };
        cart.body.set_position(12.5, 64.0, 4.5);
        w.entities.insert(Entity::Minecart(cart));
        let pid = w.entities.alloc_id();
        let mut p = PlayerEnt::new(pid, "Steve");
        p.living.body.set_position(11.5, 64.0, 4.5);
        p.living.health = 17;
        p.score = 42;
        p.inventory.main[0] = Some(stk(3, 10, 0));
        p.inventory.armor[0] = Some(stk(306, 1, 5));
        p.inventory.crafting[1] = Some(stk(280, 4, 0));
        p.held_item_id = 280;
        p.respawn_ticks = 0;
        w.entities.insert(Entity::Player(p));
        (w, pid)
    }

    #[test]
    fn test_chunk_blob_round_trip() {
        let (w, _) = furnished_world();
        let blob = encode_chunk_blob(&w, 0, 0, false).unwrap();
        let d = decode_chunk_blob(&blob, 0, 0).unwrap();
        assert_eq!((d.cx, d.cz), (0, 0));
        assert_eq!(d.blocks.len(), CHUNK_VOLUME);
        // Torch + tiles survived.
        let mut w2 = World::new(99);
        assert!(w2.apply_decoded_chunk(d));
        assert_eq!(w2.get_block_id(3, 64, 4), 50);
        assert_eq!(w2.get_block_meta(3, 64, 4), 5);
        assert!(matches!(w2.tiles.get(&(4, 64, 4)), Some(TileData::Furnace(f)) if f.burn_time == 40 && f.slots[0].item_id == 15));
        assert!(matches!(w2.tiles.get(&(5, 64, 4)), Some(TileData::Chest(c)) if c.slots[0].count == 7));
        assert!(matches!(w2.tiles.get(&(6, 64, 4)), Some(TileData::Sign(_))));
        // Entities thawed live with state.
        let mut sheep_ok = false;
        let mut zombie_ok = false;
        let mut sticks_ok = false;
        let mut cart_ok = false;
        for oid in w2.entities.alive_ids() {
            match w2.entities.get(oid).unwrap() {
                Entity::Animal(a) if a.kind == AnimalKind::Sheep => {
                    sheep_ok = a.sheared && a.egg_timer == 1234;
                }
                Entity::Mob(m) if m.kind == MobKind::Zombie => {
                    zombie_ok = m.living.health == 11;
                }
                Entity::Item(e) if e.item_id == 280 => {
                    sticks_ok = e.count == 2;
                }
                Entity::Minecart(c) => {
                    cart_ok = c.damage_taken == 10 && c.time_since_hit == 5;
                }
                _ => {}
            }
        }
        assert!(sheep_ok && zombie_ok && sticks_ok && cart_ok);
        // Re-encode is byte-stable (idempotent codec).
        let blob2 = encode_chunk_blob(&w2, 0, 0, false).unwrap();
        assert_eq!(blob, blob2);
    }

    #[test]
    fn test_chunk_store_round_trip() {
        let dir = tmp_dir("store");
        let mut store = ChunkStore::open(dir.join("db").to_str().unwrap()).unwrap();
        assert_eq!(store.get_chunk(1, 2), None);
        store.put_chunk(1, 2, &[9, 8, 7]).unwrap();
        assert_eq!(store.get_chunk(1, 2), Some(vec![9, 8, 7]));
        let (mut w, _) = furnished_world();
        assert!(w.save_chunk_to(&mut store, 0, 0));
        let mut w2 = World::new(99);
        assert!(w2.load_chunk_from(&mut store, 0, 0));
        assert_eq!(w2.get_block_id(3, 64, 4), 50);
        assert!(!w2.load_chunk_from(&mut store, 9, 9));
    }

    #[test]
    fn test_level_dat_round_trip() {
        let dir = tmp_dir("level");
        let mut w = World::new(4242);
        w.time = 777;
        w.spawn = [10, 65, -3];
        assert!(w.save_level_to(dir.to_str().unwrap()));
        let mut w2 = World::new(0);
        assert!(w2.load_level_from(dir.to_str().unwrap()));
        assert_eq!((w2.seed, w2.spawn, w2.time), (4242, [10, 65, -3], 777));
        assert!(!w2.load_level_from("/nonexistent_dir_xyz"));
    }

    #[test]
    fn test_player_file_round_trip() {
        let dir = tmp_dir("player");
        let (w, pid) = furnished_world();
        assert!(w.save_player_to(dir.to_str().unwrap(), pid));
        let mut w2 = World::new(99);
        let nid = w2.load_player_from(dir.to_str().unwrap(), "Steve").unwrap();
        let (health, score, dirt, helm, sticks) = match w2.entities.get(nid).unwrap() {
            Entity::Player(p) => (
                p.living.health,
                p.score,
                p.inventory.main[0],
                p.inventory.armor[0],
                p.inventory.crafting[1],
            ),
            _ => unreachable!(),
        };
        assert_eq!(health, 17);
        assert_eq!(score, 42);
        assert_eq!(dirt.map(|s| (s.item_id, s.count)), Some((3, 10)));
        assert_eq!(helm.map(|s| (s.item_id, s.damage)), Some((306, 5)));
        assert_eq!(sticks.map(|s| (s.item_id, s.count)), Some((280, 4)));
        // Held selection rides the file like C++ savedHeldItemId.
        assert_eq!(
            match w2.entities.get(nid).unwrap() {
                Entity::Player(p) => p.held_item_id,
                _ => unreachable!(),
            },
            280
        );
        // Username lookup is case-insensitive on disk like C++.
        assert!(w2.load_player_from(dir.to_str().unwrap(), "STEVE").is_some());
        assert!(w2.load_player_from(dir.to_str().unwrap(), "Nobody").is_none());
    }

    #[test]
    fn test_decode_rejects_garbage() {
        assert!(decode_chunk_blob(&[], 0, 0).is_none());
        assert!(decode_chunk_blob(&[1, 2, 3], 0, 0).is_none());
        assert!(decode_level_dat(&[0u8; 10]).is_none());
        assert!(decode_player(&[], "x").is_none());
    }

    /// Live captured C++ blob (zstd value from the running server's
    /// LevelDB, chunk (0,0)): the decoder must accept real data with
    /// matching coordinates and plausible terrain. This is the
    /// non-circular compat anchor — everything else is round-trip.
    #[test]
    fn test_decode_live_cpp_blob() {
        let bytes = include_bytes!("../testdata/live_chunk_0_0.bin");
        let d = decode_chunk_blob(bytes, 0, 0).expect("live C++ blob must decode");
        assert_eq!((d.cx, d.cz), (0, 0));
        assert_eq!(d.blocks.len(), CHUNK_VOLUME);
        let air = d.blocks.iter().filter(|b| **b == 0).count();
        assert!(
            (8_000..28_000).contains(&air),
            "live terrain should be part air, got {air}"
        );
        // Applies cleanly into a live world.
        let mut w = World::new(0);
        assert!(w.apply_decoded_chunk(d));
        assert!(w.has_chunk(0, 0));
    }

    #[test]
    fn test_write_atomic_and_chunk_store_flush() {
        let dir = tmp_dir("atomic_and_flush");
        let target_file = dir.join("atomic_test.dat");
        assert!(write_atomic(&target_file, b"durability_test"));
        assert_eq!(std::fs::read(&target_file).unwrap(), b"durability_test");

        let db_dir = dir.join("db");
        let mut store = ChunkStore::open(db_dir.to_str().unwrap()).unwrap();
        store.put_chunk(0, 0, b"chunk_payload").unwrap();
        assert!(store.flush().is_ok());
        assert_eq!(store.get_chunk(0, 0).unwrap(), b"chunk_payload");
    }

    #[test]
    fn test_chunk_store_default_and_env_options() {
        let opts = ChunkStore::default_options();
        assert_eq!(opts.write_buffer_size, 512 * 1024);
        assert_eq!(opts.block_cache_capacity_bytes, 512 * 1024);
    }
}


