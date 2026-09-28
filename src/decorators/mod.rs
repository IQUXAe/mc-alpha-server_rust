pub mod ores;
pub mod trees;
pub mod misc;

use std::collections::HashMap;
use crate::chunk::Chunk;
use crate::random::JavaRandom;
use crate::noise::NoiseGeneratorOctaves;
use crate::biome::BiomeType;
use crate::block::table::block_properties_get;
use crate::world::material_of;
use crate::world::{World, is_air_material};

use ores::{WorldGenMinable, WorldGenClay};
use trees::{WorldGenTrees, WorldGenBigTree};
use misc::{WorldGenLakes, WorldGenFlowers, WorldGenReed, WorldGenCactus, WorldGenPumpkin, WorldGenLiquids, WorldGenDungeons};

/// Block access for decoration. The populate canvas and the live world are the two backends;
///
/// tests implement it on a scripted fake.
pub trait BlockAccess {
    fn get_block_id(&mut self, x: i32, y: i32, z: i32) -> u8;
    fn set_block_id(&mut self, x: i32, y: i32, z: i32, id: u8);
    fn get_block_meta(&mut self, x: i32, y: i32, z: i32) -> u8;
    fn set_block_meta(&mut self, x: i32, y: i32, z: i32, meta: u8);
    fn allows_attachment(&mut self, x: i32, y: i32, z: i32) -> bool;
    fn is_block_solid(&mut self, x: i32, y: i32, z: i32) -> bool;
    fn get_height_value(&mut self, x: i32, z: i32) -> i32;
    /// Queue one dungeon-chest loot stack. Only the canvas backend
    /// collects (dungeons generate during populate); the default is a
    /// no-op — trees never make chests, so the live path stays silent.
    fn push_dungeon_loot(&mut self, _x: i32, _y: i32, _z: i32, _slot: i32, _item: i32, _count: i32) {
    }
}

/// Live-world backend (sapling growth path): the chunk map plus the
/// population flag, sharing the `*_in` flows with `World` (one formula).
/// `queue` collects written cells for the broadcast fan-out when `Some`
/// (live ticks); chunk population passes `None` (fresh chunks stream whole).
pub struct WorldAccess<'a> {
    pub chunks: &'a mut HashMap<(i32, i32), Box<Chunk>>,
    pub populating: bool,
    pub queue: Option<&'a mut Vec<[i32; 3]>>,
}

impl<'a> BlockAccess for WorldAccess<'a> {
    fn get_block_id(&mut self, x: i32, y: i32, z: i32) -> u8 {
        World::block_id_in(self.chunks, x, y, z)
    }
    fn set_block_id(&mut self, x: i32, y: i32, z: i32, id: u8) {
        if World::set_block_id_in(self.chunks, self.populating, x, y, z, id, true) {
            if let Some(q) = self.queue.as_deref_mut() {
                q.push([x, y, z]);
            }
        }
    }
    fn get_block_meta(&mut self, x: i32, y: i32, z: i32) -> u8 {
        World::block_meta_in(self.chunks, x, y, z)
    }
    fn set_block_meta(&mut self, x: i32, y: i32, z: i32, meta: u8) {
        if World::set_block_meta_in(self.chunks, x, y, z, meta) {
            if let Some(q) = self.queue.as_deref_mut() {
                q.push([x, y, z]);
            }
        }
    }
    fn allows_attachment(&mut self, x: i32, y: i32, z: i32) -> bool {
        // Block::allowsAttachmentArr: registered plus the allowsAttachment flag.
        let bid = self.get_block_id(x, y, z);
        bid != 0
            && !is_air_material(bid)
            && crate::block::table::block_properties_get(bid as u32).allows_attachment
    }
    fn is_block_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        World::is_solid_in(self.chunks, x, y, z)
    }
    fn get_height_value(&mut self, x: i32, z: i32) -> i32 {
        World::height_in(self.chunks, x, z)
    }
}

/// Canvas backend for chunk population: the 2x2 decorate arrays with a
/// live fallback for out-of-canvas reads/writes. Chest/spawner blocks live
/// in the canvas like vanilla; chest loot is buffered via `push_dungeon_loot`
/// and spawner RNG is burned in order (tiles materialize on write-back).
pub struct CanvasAccess<'a> {
    blocks: &'a mut [[[u8; 32768]; 2]; 2],
    metadata: &'a mut [[[u8; 32768]; 2]; 2],
    chunk_x: i32,
    chunk_z: i32,
    fallback: &'a mut dyn BlockAccess,
    loot: Vec<(i32, i32, i32, i32, i32, i32)>,
}

impl<'a> CanvasAccess<'a> {
    /// Build over populate-batch arrays.
    pub fn new(
        blocks: &'a mut [[[u8; 32768]; 2]; 2],
        metadata: &'a mut [[[u8; 32768]; 2]; 2],
        chunk_x: i32,
        chunk_z: i32,
        fallback: &'a mut dyn BlockAccess,
    ) -> Self {
        Self { blocks, metadata, chunk_x, chunk_z, fallback, loot: Vec::new() }
    }

    /// Drain the dungeon-chest loot queued during decoration (the canvas
    /// holds no tiles; the world materializes chest rows on write-back).
    pub fn take_loot(&mut self) -> Vec<(i32, i32, i32, i32, i32, i32)> {
        std::mem::take(&mut self.loot)
    }

    /// (dx, dz, local x, local z) for in-canvas columns.
    fn canvas_slot(&self, x: i32, z: i32) -> Option<(usize, usize, usize, usize)> {
        let rel_x = x - self.chunk_x * 16;
        let rel_z = z - self.chunk_z * 16;
        if !(0..32).contains(&rel_x) || !(0..32).contains(&rel_z) {
            return None;
        }
        Some((
            (rel_x >> 4) as usize,
            (rel_z >> 4) as usize,
            (rel_x & 15) as usize,
            (rel_z & 15) as usize,
        ))
    }
}

impl<'a> BlockAccess for CanvasAccess<'a> {
    fn get_block_id(&mut self, x: i32, y: i32, z: i32) -> u8 {
        match self.canvas_slot(x, z) {
            Some((dx, dz, lx, lz)) if (0..128).contains(&y) => {
                self.blocks[dx][dz][(lx << 11) | (lz << 7) | (y as usize)]
            }
            _ => self.fallback.get_block_id(x, y, z),
        }
    }
    fn set_block_id(&mut self, x: i32, y: i32, z: i32, id: u8) {
        // Vanilla writes chest (54) and spawner (52) into the world like
        // any other block during populate; tiles are created afterwards
        // from the block id. The canvas holds no tiles, so blocks always
        // go to the canvas here — chest loot is buffered via
        // `push_dungeon_loot` and materialized on write-back in
        // `world::gen` (same 8-attempt / 8-roll / RNG order as vanilla
        // `WorldGenDungeons`). Writing to the live fallback instead would
        // be overwritten by the canvas write-back (empty dungeon rooms).
        match self.canvas_slot(x, z) {
            Some((dx, dz, lx, lz)) if (0..128).contains(&y) => {
                self.blocks[dx][dz][(lx << 11) | (lz << 7) | (y as usize)] = id;
            }
            _ => self.fallback.set_block_id(x, y, z, id),
        }
    }
    fn get_block_meta(&mut self, x: i32, y: i32, z: i32) -> u8 {
        // Canvas metadata is unpacked (one byte per cell), matching
        // `Chunk::fill_arrays` / `load_arrays` — not packed nibbles.
        match self.canvas_slot(x, z) {
            Some((dx, dz, lx, lz)) if (0..128).contains(&y) => {
                let idx = (lx << 11) | (lz << 7) | (y as usize);
                self.metadata[dx][dz][idx] & 0xF
            }
            _ => self.fallback.get_block_meta(x, y, z),
        }
    }
    fn set_block_meta(&mut self, x: i32, y: i32, z: i32, meta: u8) {
        match self.canvas_slot(x, z) {
            Some((dx, dz, lx, lz)) if (0..128).contains(&y) => {
                let idx = (lx << 11) | (lz << 7) | (y as usize);
                self.metadata[dx][dz][idx] = meta & 0xF;
            }
            _ => self.fallback.set_block_meta(x, y, z, meta),
        }
    }
    fn allows_attachment(&mut self, x: i32, y: i32, z: i32) -> bool {
        // Read the canvas first: during populate the live fallback is
        // stale (fresh terrain lives only in the canvas). Same rule as
        // the live backend (`Block::allowsAttachmentArr` + flag).
        let bid = self.get_block_id(x, y, z);
        if self.canvas_slot(x, z).is_some() && (0..128).contains(&y) {
            return bid != 0
                && !is_air_material(bid)
                && crate::block::table::block_properties_get(bid as u32).allows_attachment;
        }
        self.fallback.allows_attachment(x, y, z)
    }
    fn is_block_solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        // Same canvas-first reason: chest placement in `WorldGenDungeons`
        // must see freshly placed cobble, not the stale live chunk.
        // ID list mirrors `World::is_solid_in` (vanilla
        // `isBlockSolidNoChunkLoad`).
        if self.canvas_slot(x, z).is_some() && (0..128).contains(&y) {
            let bid = self.get_block_id(x, y, z);
            return !matches!(
                bid,
                0 | 8 | 9 | 10 | 11 | 78 | 37 | 38 | 39 | 40 | 83 | 51 | 6
            );
        }
        self.fallback.is_block_solid(x, y, z)
    }
    fn get_height_value(&mut self, x: i32, z: i32) -> i32 {
        match self.canvas_slot(x, z) {
            Some((dx, dz, lx, lz)) => {
                // Vanilla height uses `lightOpacity != 0` (glass/leaves/
                // water count), not `blocks != 0`.
                for y in (0..128).rev() {
                    let bid = self.blocks[dx][dz][(lx << 11) | (lz << 7) | y];
                    if block_properties_get(u32::from(bid)).light_opacity != 0 {
                        return (y + 1) as i32;
                    }
                }
                0
            }
            _ => self.fallback.get_height_value(x, z),
        }
    }
    fn push_dungeon_loot(&mut self, x: i32, y: i32, z: i32, slot: i32, item: i32, count: i32) {
        self.loot.push((x, y, z, slot, item, count));
    }
}

/// Top snow-support cell (mirrors `World.func_4075_e`): the first air
/// cell above the highest occluding-or-liquid block, or -1 when the
/// column has none. Flowers, torches and snow itself are seen through
/// (their materials do not occlude); leaves and ice do occlude.
fn snow_top_y(accessor: &mut dyn BlockAccess, x: i32, z: i32) -> i32 {
    let mut mat_at = |y: i32| {
        let id = accessor.get_block_id(x, y, z);
        (id, material_of(block_properties_get(id as u32).material))
    };
    let mut y = 127;
    while y > 0 && mat_at(y).1.is_solid() {
        y -= 1;
    }
    while y > 0 {
        let (id, m) = mat_at(y);
        if id != 0 && (m.is_solid() || m.is_liquid()) {
            return y + 1;
        }
        y -= 1;
    }
    -1
}

pub fn decorate_chunk(
    accessor: &mut dyn BlockAccess,
    seed: i64,
    chunk_x: i32,
    chunk_z: i32,
    biome_type: BiomeType,
    noise_gen_713: &mut NoiseGeneratorOctaves,
    temperatures: &[f64],
) {
    let var4 = chunk_x * 16;
    let var5 = chunk_z * 16;

    let mut rand = JavaRandom::new(seed);
    let var7 = rand.next_long() / 2 * 2 + 1;
    let var9 = rand.next_long() / 2 * 2 + 1;
    rand.set_seed((chunk_x as i64).wrapping_mul(var7).wrapping_add((chunk_z as i64).wrapping_mul(var9)) ^ seed);

    // --- Water lakes ---
    if rand.next_int_bound(4) == 0 {
        let var13 = var4 + rand.next_int_bound(16) + 8;
        let var14 = rand.next_int_bound(128);
        let var15 = var5 + rand.next_int_bound(16) + 8;
        WorldGenLakes::new(9).generate(&mut *accessor, &mut rand, var13, var14, var15);
    }

    // --- Lava lakes ---
    if rand.next_int_bound(8) == 0 {
        let var13 = var4 + rand.next_int_bound(16) + 8;
        let step1 = rand.next_int_bound(120) + 8;
        let var14 = rand.next_int_bound(step1);
        let var15 = var5 + rand.next_int_bound(16) + 8;
        if var14 < 64 || rand.next_int_bound(10) == 0 {
            WorldGenLakes::new(11).generate(&mut *accessor, &mut rand, var13, var14, var15);
        }
    }

    // --- Dungeons ---
    for _ in 0..8 {
        let dx = var4 + rand.next_int_bound(16) + 8;
        let dy = rand.next_int_bound(128);
        let dz = var5 + rand.next_int_bound(16) + 8;
        WorldGenDungeons::new().generate(&mut *accessor, &mut rand, dx, dy, dz);
    }

    // --- Clay ---
    for _ in 0..10 {
        let cx = var4 + rand.next_int_bound(16);
        let cy = rand.next_int_bound(128);
        let cz = var5 + rand.next_int_bound(16);
        WorldGenClay::new(32).generate(&mut *accessor, &mut rand, cx, cy, cz);
    }

    // --- Dirt veins ---
    for _ in 0..20 {
        let mx = var4 + rand.next_int_bound(16);
        let my = rand.next_int_bound(128);
        let mz = var5 + rand.next_int_bound(16);
        WorldGenMinable::new(3, 32).generate(&mut *accessor, &mut rand, mx, my, mz);
    }

    // --- Gravel veins ---
    for _ in 0..10 {
        let mx = var4 + rand.next_int_bound(16);
        let my = rand.next_int_bound(128);
        let mz = var5 + rand.next_int_bound(16);
        WorldGenMinable::new(13, 32).generate(&mut *accessor, &mut rand, mx, my, mz);
    }

    // --- Coal ore ---
    for _ in 0..20 {
        let mx = var4 + rand.next_int_bound(16);
        let my = rand.next_int_bound(128);
        let mz = var5 + rand.next_int_bound(16);
        WorldGenMinable::new(16, 16).generate(&mut *accessor, &mut rand, mx, my, mz);
    }

    // --- Iron ore ---
    for _ in 0..20 {
        let mx = var4 + rand.next_int_bound(16);
        let my = rand.next_int_bound(64);
        let mz = var5 + rand.next_int_bound(16);
        WorldGenMinable::new(15, 8).generate(&mut *accessor, &mut rand, mx, my, mz);
    }

    // --- Gold ore ---
    for _ in 0..2 {
        let mx = var4 + rand.next_int_bound(16);
        let my = rand.next_int_bound(32);
        let mz = var5 + rand.next_int_bound(16);
        WorldGenMinable::new(14, 8).generate(&mut *accessor, &mut rand, mx, my, mz);
    }

    // --- Redstone ore ---
    for _ in 0..8 {
        let mx = var4 + rand.next_int_bound(16);
        let my = rand.next_int_bound(16);
        let mz = var5 + rand.next_int_bound(16);
        WorldGenMinable::new(73, 7).generate(&mut *accessor, &mut rand, mx, my, mz);
    }

    // --- Diamond ore ---
    for _ in 0..1 {
        let mx = var4 + rand.next_int_bound(16);
        let my = rand.next_int_bound(16);
        let mz = var5 + rand.next_int_bound(16);
        WorldGenMinable::new(56, 7).generate(&mut *accessor, &mut rand, mx, my, mz);
    }

    // --- Trees ---
    let var11d = 0.5;
    let noise_val = noise_gen_713.sample2_octaves((var4 as f64) * var11d, (var5 as f64) * var11d);

    let var13t = ((noise_val / 8.0 + rand.next_double() * 4.0 + 4.0) / 3.0) as i32;
    let mut var14t = 0;
    if rand.next_int_bound(10) == 0 {
        var14t += 1;
    }

    match biome_type {
        BiomeType::Forest | BiomeType::Rainforest | BiomeType::Taiga => {
            var14t += var13t + 5;
        }
        BiomeType::SeasonalForest => {
            var14t += var13t + 2;
        }
        BiomeType::Desert | BiomeType::Tundra | BiomeType::Plains => {
            var14t -= 20;
        }
        _ => {}
    }

    // Tree type selection
    let mut use_big_tree = false;
    if rand.next_int_bound(10) == 0 {
        use_big_tree = true;
    }
    if biome_type == BiomeType::Rainforest && rand.next_int_bound(3) == 0 {
        use_big_tree = true;
    }

    let mut big_tree_gen = if use_big_tree {
        let mut bt = WorldGenBigTree::new();
        bt.configure(1.0, 1.0, 1.0);
        Some(bt)
    } else {
        None
    };
    let mut normal_tree_gen = if !use_big_tree {
        Some(WorldGenTrees::new())
    } else {
        None
    };

    for _ in 0..var14t {
        let tx = var4 + rand.next_int_bound(16) + 8;
        let tz = var5 + rand.next_int_bound(16) + 8;
        let ty = accessor.get_height_value(tx, tz);
        if let Some(ref mut big_tree) = big_tree_gen {
            big_tree.configure(1.0, 1.0, 1.0);
            big_tree.generate(&mut *accessor, &mut rand, tx, ty, tz);
        } else if let Some(ref mut normal_tree) = normal_tree_gen {
            normal_tree.generate(&mut *accessor, &mut rand, tx, ty, tz);
        }
    }

    // --- Yellow flowers ---
    for _ in 0..2 {
        let fx = var4 + rand.next_int_bound(16) + 8;
        let fy = rand.next_int_bound(128);
        let fz = var5 + rand.next_int_bound(16) + 8;
        WorldGenFlowers::new(37).generate(&mut *accessor, &mut rand, fx, fy, fz);
    }

    // --- Red flower ---
    if rand.next_int_bound(2) == 0 {
        let fx = var4 + rand.next_int_bound(16) + 8;
        let fy = rand.next_int_bound(128);
        let fz = var5 + rand.next_int_bound(16) + 8;
        WorldGenFlowers::new(38).generate(&mut *accessor, &mut rand, fx, fy, fz);
    }

    // --- Brown mushroom ---
    if rand.next_int_bound(4) == 0 {
        let fx = var4 + rand.next_int_bound(16) + 8;
        let fy = rand.next_int_bound(128);
        let fz = var5 + rand.next_int_bound(16) + 8;
        WorldGenFlowers::new(39).generate(&mut *accessor, &mut rand, fx, fy, fz);
    }

    // --- Red mushroom ---
    if rand.next_int_bound(8) == 0 {
        let fx = var4 + rand.next_int_bound(16) + 8;
        let fy = rand.next_int_bound(128);
        let fz = var5 + rand.next_int_bound(16) + 8;
        WorldGenFlowers::new(40).generate(&mut *accessor, &mut rand, fx, fy, fz);
    }

    // --- Reed ---
    for _ in 0..10 {
        let rx = var4 + rand.next_int_bound(16) + 8;
        let ry = rand.next_int_bound(128);
        let rz = var5 + rand.next_int_bound(16) + 8;
        WorldGenReed::new().generate(&mut *accessor, &mut rand, rx, ry, rz);
    }

    // --- Pumpkin ---
    if rand.next_int_bound(32) == 0 {
        let px = var4 + rand.next_int_bound(16) + 8;
        let py = rand.next_int_bound(128);
        let pz = var5 + rand.next_int_bound(16) + 8;
        WorldGenPumpkin::new().generate(&mut *accessor, &mut rand, px, py, pz);
    }

    // --- Cactus ---
    let mut cactus_count = 0;
    if biome_type == BiomeType::Desert {
        cactus_count += 10;
    }
    for _ in 0..cactus_count {
        let cx = var4 + rand.next_int_bound(16) + 8;
        let cy = rand.next_int_bound(128);
        let cz = var5 + rand.next_int_bound(16) + 8;
        WorldGenCactus::new().generate(&mut *accessor, &mut rand, cx, cy, cz);
    }

    // --- Underground water springs ---
    for _ in 0..50 {
        let sx = var4 + rand.next_int_bound(16) + 8;
        let step1 = rand.next_int_bound(120) + 8;
        let sy = rand.next_int_bound(step1);
        let sz = var5 + rand.next_int_bound(16) + 8;
        WorldGenLiquids::new(8).generate(&mut *accessor, &mut rand, sx, sy, sz);
    }

    // --- Underground lava springs ---
    for _ in 0..20 {
        let sx = var4 + rand.next_int_bound(16) + 8;
        let step1 = rand.next_int_bound(112) + 8;
        let step2 = rand.next_int_bound(step1) + 8;
        let sy = rand.next_int_bound(step2);
        let sz = var5 + rand.next_int_bound(16) + 8;
        WorldGenLiquids::new(10).generate(&mut *accessor, &mut rand, sx, sy, sz);
    }

    // --- Snow ---
    // `temperatures` covers (var4 + 8, var5 + 8)..+16 like Java's fresh
    // `getTemperatures` slice, so the (var19, var20) index below samples
    // the decorated column itself, not the chunk 8 blocks back.
    {
        let temps_slice = temperatures;
        for var17 in (var4 + 8)..(var4 + 8 + 16) {
            for var18 in (var5 + 8)..(var5 + 8 + 16) {
                let var19 = var17 - (var4 + 8);
                let var20 = var18 - (var5 + 8);
                let var21 = snow_top_y(&mut *accessor, var17, var18);
                let var22 = temps_slice[(var19 * 16 + var20) as usize] - ((var21 - 64) as f64) / 64.0 * 0.3;

                if var22 < 0.5 && var21 > 0 && var21 < 128
                    && accessor.get_block_id(var17, var21, var18) == 0
                {
                    // Short-circuit above guarantees var21 - 1 >= 0.
                    let below_id = accessor.get_block_id(var17, var21 - 1, var18);
                    let below_mat =
                        material_of(block_properties_get(below_id as u32).material);
                    if below_mat.is_solid() && below_id != 79 {
                        // occluding ground that is not ice
                        accessor.set_block_id(var17, var21, var18, 78); // snow layer
                    }
                }
            }
        }
    }
}
