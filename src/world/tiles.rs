//! Block-entity rows and entity string-id helpers, split out of `world.rs`.
//! Re-exported from `world` so `crate::world::TileData` keeps working.

use crate::entity::table::{AnimalKind, LivingBody, MobKind};

/// Mob spawner tile-entity state (`TileEntityMobSpawner.java`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MobSpawnerState {
    pub entity_id: [u8; 16],
    pub delay: i16,
}

impl MobSpawnerState {
    pub fn new(entity_id: &str) -> Self {
        let mut state = Self {
            entity_id: [0u8; 16],
            delay: 20,
        };
        state.set_entity_id(entity_id);
        state
    }

    pub fn set_entity_id(&mut self, id: &str) {
        self.entity_id = [0u8; 16];
        let bytes = id.as_bytes();
        let len = bytes.len().min(16);
        self.entity_id[..len].copy_from_slice(&bytes[..len]);
    }

    pub fn entity_id_str(&self) -> &str {
        let len = self
            .entity_id
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.entity_id.len());
        std::str::from_utf8(&self.entity_id[..len]).unwrap_or("Pig")
    }
}

/// Block-entity data by cell (mirrors the C++ per-chunk `TileEntity`
/// objects, stored flat until the tile tick slice needs behavior).
#[derive(Clone, Copy, Debug)]
// Chest/Furnace states are large but Copy and cells are few; boxing would
// kill Copy for a lint without measured gain. Revisit with profiling.
#[allow(clippy::large_enum_variant)]
pub enum TileData {
    Furnace(crate::tile_entity::furnace::FurnaceState),
    Chest(crate::tile_entity::chest::ChestState),
    Sign(crate::tile_entity::sign::SignState),
    MobSpawner(MobSpawnerState),
}

type ChunkTileMap = std::collections::HashMap<(i32, i32), Vec<(i32, i32, i32)>>;

/// Container for world block-entity data, indexed both by global cell
/// coordinate `(x, y, z)` and by chunk coordinate `(cx, cz)`.
#[derive(Clone, Debug, Default)]
pub struct WorldTiles {
    tiles: std::collections::HashMap<(i32, i32, i32), TileData>,
    by_chunk: ChunkTileMap,
}

pub struct TileEntry<'a> {
    pos: (i32, i32, i32),
    tiles: &'a mut WorldTiles,
}

impl<'a> TileEntry<'a> {
    pub fn or_insert_with<F: FnOnce() -> TileData>(self, default: F) -> &'a mut TileData {
        if !self.tiles.tiles.contains_key(&self.pos) {
            let data = default();
            self.tiles.insert(self.pos, data);
        }
        self.tiles.tiles.get_mut(&self.pos).unwrap()
    }
}

impl WorldTiles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, pos: (i32, i32, i32), data: TileData) -> Option<TileData> {
        let chunk_pos = (pos.0.div_euclid(16), pos.2.div_euclid(16));
        let chunk_vec = self.by_chunk.entry(chunk_pos).or_default();
        if !chunk_vec.contains(&pos) {
            chunk_vec.push(pos);
        }
        self.tiles.insert(pos, data)
    }

    pub fn remove(&mut self, pos: &(i32, i32, i32)) -> Option<TileData> {
        let chunk_pos = (pos.0.div_euclid(16), pos.2.div_euclid(16));
        if let Some(chunk_vec) = self.by_chunk.get_mut(&chunk_pos) {
            chunk_vec.retain(|p| p != pos);
            if chunk_vec.is_empty() {
                self.by_chunk.remove(&chunk_pos);
            }
        }
        self.tiles.remove(pos)
    }

    pub fn remove_chunk(&mut self, cx: i32, cz: i32) {
        if let Some(cells) = self.by_chunk.remove(&(cx, cz)) {
            for pos in cells {
                self.tiles.remove(&pos);
            }
        }
    }

    pub fn get(&self, pos: &(i32, i32, i32)) -> Option<&TileData> {
        self.tiles.get(pos)
    }

    pub fn get_mut(&mut self, pos: &(i32, i32, i32)) -> Option<&mut TileData> {
        self.tiles.get_mut(pos)
    }

    pub fn contains_key(&self, pos: &(i32, i32, i32)) -> bool {
        self.tiles.contains_key(pos)
    }

    pub fn keys(&self) -> std::collections::hash_map::Keys<'_, (i32, i32, i32), TileData> {
        self.tiles.keys()
    }

    pub fn values(&self) -> std::collections::hash_map::Values<'_, (i32, i32, i32), TileData> {
        self.tiles.values()
    }

    pub fn values_mut(&mut self) -> std::collections::hash_map::ValuesMut<'_, (i32, i32, i32), TileData> {
        self.tiles.values_mut()
    }

    pub fn iter(&self) -> std::collections::hash_map::Iter<'_, (i32, i32, i32), TileData> {
        self.tiles.iter()
    }

    pub fn iter_mut(&mut self) -> std::collections::hash_map::IterMut<'_, (i32, i32, i32), TileData> {
        self.tiles.iter_mut()
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
        self.by_chunk.clear();
    }

    pub fn retain<F>(&mut self, mut f: F)
    where
        F: FnMut(&(i32, i32, i32), &mut TileData) -> bool,
    {
        self.tiles.retain(&mut f);
        self.by_chunk.clear();
        for &pos in self.tiles.keys() {
            let chunk_pos = (pos.0.div_euclid(16), pos.2.div_euclid(16));
            self.by_chunk.entry(chunk_pos).or_default().push(pos);
        }
    }

    pub fn entry(&mut self, pos: (i32, i32, i32)) -> TileEntry<'_> {
        TileEntry { pos, tiles: self }
    }

    pub fn chunk_cells(&self, cx: i32, cz: i32) -> &[(i32, i32, i32)] {
        self.by_chunk.get(&(cx, cz)).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

impl<'a> IntoIterator for &'a WorldTiles {
    type Item = (&'a (i32, i32, i32), &'a TileData);
    type IntoIter = std::collections::hash_map::Iter<'a, (i32, i32, i32), TileData>;

    fn into_iter(self) -> Self::IntoIter {
        self.tiles.iter()
    }
}

impl<'a> IntoIterator for &'a mut WorldTiles {
    type Item = (&'a (i32, i32, i32), &'a mut TileData);
    type IntoIter = std::collections::hash_map::IterMut<'a, (i32, i32, i32), TileData>;

    fn into_iter(self) -> Self::IntoIter {
        self.tiles.iter_mut()
    }
}

/// String id for spill/restore (mirrors `getEntityStringId`).
pub(crate) fn mob_string_id(kind: MobKind) -> String {
    match kind {
        MobKind::Spider => "Spider",
        MobKind::Zombie => "Zombie",
        MobKind::Skeleton => "Skeleton",
        MobKind::Creeper => "Creeper",
    }
    .to_string()
}

/// String id for spill/restore (mirrors `getEntityStringId`).
pub(crate) fn animal_string_id(kind: AnimalKind) -> String {
    match kind {
        AnimalKind::Sheep => "Sheep",
        AnimalKind::Pig => "Pig",
        AnimalKind::Chicken => "Chicken",
        AnimalKind::Cow => "Cow",
    }
    .to_string()
}

pub(crate) fn mob_kind_of(id: &str) -> Option<MobKind> {
    match id {
        "Spider" => Some(MobKind::Spider),
        "Zombie" => Some(MobKind::Zombie),
        "Skeleton" => Some(MobKind::Skeleton),
        "Creeper" => Some(MobKind::Creeper),
        _ => None,
    }
}

pub(crate) fn animal_kind_of(id: &str) -> Option<AnimalKind> {
    match id {
        "Sheep" => Some(AnimalKind::Sheep),
        "Pig" => Some(AnimalKind::Pig),
        "Chicken" => Some(AnimalKind::Chicken),
        "Cow" => Some(AnimalKind::Cow),
        _ => None,
    }
}

pub(crate) fn pending_creature(
    string_id: String,
    living: &LivingBody,
    saddled: bool,
    sheared: bool,
    egg_timer: i32,
) -> crate::chunk::PendingCreature {
    crate::chunk::PendingCreature {
        string_id,
        pos: living.body.pos,
        motion: living.body.motion,
        yaw: living.body.yaw,
        pitch: living.body.pitch,
        health: living.health,
        max_health: living.max_health,
        saddled,
        sheared,
        egg_timer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_world_tiles_chunk_indexing() {
        let mut tiles = WorldTiles::new();
        // Chunk (0, 0)
        tiles.insert((5, 64, 5), TileData::Sign(crate::tile_entity::sign::sign_create()));
        tiles.insert((10, 64, 10), TileData::Sign(crate::tile_entity::sign::sign_create()));

        // Chunk (1, 0)
        tiles.insert((20, 64, 5), TileData::Sign(crate::tile_entity::sign::sign_create()));

        assert_eq!(tiles.chunk_cells(0, 0).len(), 2);
        assert_eq!(tiles.chunk_cells(1, 0).len(), 1);
        assert_eq!(tiles.chunk_cells(2, 0).len(), 0);

        // Remove one from chunk (0, 0)
        tiles.remove(&(5, 64, 5));
        assert_eq!(tiles.chunk_cells(0, 0).len(), 1);
        assert_eq!(tiles.chunk_cells(0, 0)[0], (10, 64, 10));

        // remove_chunk
        tiles.remove_chunk(0, 0);
        assert!(tiles.chunk_cells(0, 0).is_empty());
        assert!(!tiles.contains_key(&(10, 64, 10)));
        assert_eq!(tiles.chunk_cells(1, 0).len(), 1);
    }

    #[test]
    fn test_world_tiles_entry_and_retain() {
        let mut tiles = WorldTiles::new();
        tiles.entry((35, 64, 35)).or_insert_with(|| {
            TileData::Sign(crate::tile_entity::sign::sign_create())
        });
        // 35 div 16 = 2
        assert_eq!(tiles.chunk_cells(2, 2).len(), 1);
        assert!(tiles.contains_key(&(35, 64, 35)));

        // retain keeping nothing
        tiles.retain(|_, _| false);
        assert_eq!(tiles.len(), 0);
        assert!(tiles.chunk_cells(2, 2).is_empty());
    }
}

