use crate::random::JavaRandom;
use crate::noise::{NoiseGeneratorOctaves, NoiseGeneratorOctaves2};
use crate::biome::{BiomeType, MobSpawnerBase, get_biome_from_lookup};
use crate::caves::MapGenCaves;
use crate::decorators::{BlockAccess, CanvasAccess, decorate_chunk};

pub struct ChunkProvider {
    pub world_seed: i64,
    pub min_noise: Box<NoiseGeneratorOctaves>,
    pub max_noise: Box<NoiseGeneratorOctaves>,
    pub main_noise: Box<NoiseGeneratorOctaves>,
    pub biome_noise: Box<NoiseGeneratorOctaves>,
    pub stone_noise: Box<NoiseGeneratorOctaves>,
    pub depth_noise: Box<NoiseGeneratorOctaves>,
    pub scale_noise: Box<NoiseGeneratorOctaves>,
    pub tree_noise: Box<NoiseGeneratorOctaves>,
    
    // Biome generators (WorldChunkManager)
    pub temp_noise_gen: Box<NoiseGeneratorOctaves2>,
    pub humid_noise_gen: Box<NoiseGeneratorOctaves2>,
    pub noise_gen3: Box<NoiseGeneratorOctaves2>,
}

impl ChunkProvider {
    pub fn new(seed: i64) -> Self {
        let mut rand = JavaRandom::new(seed);
        let min_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 16));
        let max_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 16));
        let main_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 8));
        let biome_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 4));
        let stone_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 4));
        let depth_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 10));
        let scale_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 16));
        let tree_noise = Box::new(NoiseGeneratorOctaves::new(&mut rand, 8));

        let temp_noise_gen = Box::new(NoiseGeneratorOctaves2::new(&mut JavaRandom::new(seed.wrapping_mul(9871)), 4));
        let humid_noise_gen = Box::new(NoiseGeneratorOctaves2::new(&mut JavaRandom::new(seed.wrapping_mul(39811)), 4));
        let noise_gen3 = Box::new(NoiseGeneratorOctaves2::new(&mut JavaRandom::new(seed.wrapping_mul(543321)), 2));

        Self {
            world_seed: seed,
            min_noise,
            max_noise,
            main_noise,
            biome_noise,
            stone_noise,
            depth_noise,
            scale_noise,
            tree_noise,
            temp_noise_gen,
            humid_noise_gen,
            noise_gen3,
        }
    }
}

/// Climate slice (mirrors `WorldChunkManager.loadBlockGeneratorData`):
/// temperature, humidity and biome lookup for a `w` by `h` area at block
/// origin `(x0, z0)`, layout `idx = x * h + z` like the terrain pass.
/// The noise tables are pure in coordinates, so populate-time queries
/// below evaluate bit-identical values at identical points.
pub fn climate_into(
    gen: &ChunkProvider,
    origin: [i32; 2],
    size: [usize; 2],
    biomes: &mut [MobSpawnerBase],
    temperatures: &mut [f64],
    humidities: &mut [f64],
) {
    let [x0, z0] = origin;
    let [w, h] = size;
    let n = w * h;
    debug_assert_eq!(biomes.len(), n);
    debug_assert_eq!(temperatures.len(), n);
    debug_assert_eq!(humidities.len(), n);
    // Java passes (double)0.025F / (double)0.05F (float widened), NOT the
    // decimal f64 — the simplex lattice shifts otherwise (biome borders).
    gen.temp_noise_gen.fill2_default(temperatures, x0 as f64, z0 as f64, w, h, 0.025f32 as f64, 0.025f32 as f64, 0.25);
    gen.humid_noise_gen.fill2_default(humidities, x0 as f64, z0 as f64, w, h, 0.05f32 as f64, 0.05f32 as f64, 1.0 / 3.0);

    let mut scratch = vec![0.0; n];
    gen.noise_gen3.fill2_default(&mut scratch, x0 as f64, z0 as f64, w, h, 0.25, 0.25, 0.5882352941176471);

    for idx in 0..n {
        let noise = scratch[idx] * 1.1 + 0.5;

        let d1_t = 0.01;
        let d2_t = 1.0 - d1_t;
        let mut temp = (temperatures[idx] * 0.15 + 0.7) * d2_t + noise * d1_t;

        let d1_h = 0.002;
        let d2_h = 1.0 - d1_h;
        let mut humid = (humidities[idx] * 0.15 + 0.5) * d2_h + noise * d1_h;

        temp = 1.0 - (1.0 - temp) * (1.0 - temp);
        if temp < 0.0 { temp = 0.0; }
        if humid < 0.0 { humid = 0.0; }
        if temp > 1.0 { temp = 1.0; }
        if humid > 1.0 { humid = 1.0; }

        temperatures[idx] = temp;
        humidities[idx] = humid;
        biomes[idx] = get_biome_from_lookup(temp, humid);
    }
}

/// Temperature-only slice (mirrors `WorldChunkManager.getTemperatures`),
/// evaluated at an arbitrary origin. Populate samples snow temperatures
/// at `(var4 + 8, var5 + 8)`, reaching 8 blocks past the chunk corner.
pub fn chunk_temperatures(
    gen: &ChunkProvider,
    x0: i32,
    z0: i32,
    out: &mut [f64; 256],
) {
    gen.temp_noise_gen.fill2_default(out, x0 as f64, z0 as f64, 16, 16, 0.025f32 as f64, 0.025f32 as f64, 0.25);
    let mut scratch = [0.0f64; 256];
    gen.noise_gen3.fill2_default(&mut scratch, x0 as f64, z0 as f64, 16, 16, 0.25, 0.25, 0.5882352941176471);
    for idx in 0..256 {
        let noise = scratch[idx] * 1.1 + 0.5;
        let mut temp = (out[idx] * 0.15 + 0.7) * 0.99 + noise * 0.01;
        temp = 1.0 - (1.0 - temp) * (1.0 - temp);
        out[idx] = temp.clamp(0.0, 1.0);
    }
}

/// Single-point biome (mirrors `WorldChunkManager.func_4067_a`).
/// Populate samples one biome for the whole chunk at its far corner
/// `(var4 + 16, var5 + 16)`, not the center.
pub fn point_biome(gen: &ChunkProvider, x: i32, z: i32) -> MobSpawnerBase {
    let mut biomes = [MobSpawnerBase::DEFAULT; 1];
    let mut temps = [0.0f64; 1];
    let mut humids = [0.0f64; 1];
    climate_into(gen, [x, z], [1, 1], &mut biomes, &mut temps, &mut humids);
    biomes[0]
}

pub fn generate_chunk(
    gen: &mut ChunkProvider,
    chunk_x: i32,
    chunk_z: i32,
    blocks: &mut [u8; 32768],
    biomes: &mut [MobSpawnerBase; 256],
    temperatures: &mut [f64; 256],
    humidities: &mut [f64; 256],
) {
    let mut rand = JavaRandom::new(
        (chunk_x as i64)
            .wrapping_mul(341873128712)
            .wrapping_add((chunk_z as i64).wrapping_mul(132897987541)),
    );

    // 1. Load block generator data (biomes & temperatures)
    climate_into(
        gen,
        [chunk_x * 16, chunk_z * 16],
        [16, 16],
        biomes,
        temperatures,
        humidities,
    );

    // 2. Generate terrain
    let var6 = 4;
    let var7 = 64;
    let var8 = var6 + 1;
    let var9 = 17;
    let var10 = var6 + 1;

    let mut density_field = vec![0.0; (var8 * var9 * var10) as usize];
    crate::density::density_generate_field(
        &mut density_field,
        chunk_x * var6,
        0,
        chunk_z * var6,
        var8,
        var9,
        var10,
        temperatures,
        humidities,
        &gen.depth_noise,
        &gen.scale_noise,
        &gen.main_noise,
        &gen.min_noise,
        &gen.max_noise,
    );

    for var11 in 0..var6 {
        for var12 in 0..var6 {
            for var13 in 0..16 {
                let var14 = 0.125;
                let mut var16 = density_field[((var11 * var10 + var12) * var9 + var13) as usize];
                let mut var18 = density_field[((var11 * var10 + var12 + 1) * var9 + var13) as usize];
                let mut var20 = density_field[(((var11 + 1) * var10 + var12) * var9 + var13) as usize];
                let mut var22 = density_field[(((var11 + 1) * var10 + var12 + 1) * var9 + var13) as usize];
                let var24 = (density_field[((var11 * var10 + var12) * var9 + var13 + 1) as usize] - var16) * var14;
                let var26 = (density_field[((var11 * var10 + var12 + 1) * var9 + var13 + 1) as usize] - var18) * var14;
                let var28 = (density_field[(((var11 + 1) * var10 + var12) * var9 + var13 + 1) as usize] - var20) * var14;
                let var30 = (density_field[(((var11 + 1) * var10 + var12 + 1) * var9 + var13 + 1) as usize] - var22) * var14;

                for var32 in 0..8 {
                    let var33 = 0.25;
                    let mut var35 = var16;
                    let mut var37 = var18;
                    let var39 = (var20 - var16) * var33;
                    let var41 = (var22 - var18) * var33;

                    for var43 in 0..4 {
                        let mut var44 = ((var43 + var11 * 4) << 11) | ((var12 * 4) << 7) | (var13 * 8 + var32);
                        let var45 = 128;
                        let var46 = 0.25;
                        let mut var48 = var35;
                        let var50 = (var37 - var35) * var46;

                        for var52 in 0..4 {
                            let temp_val = temperatures[((var11 * 4 + var43) * 16 + var12 * 4 + var52) as usize];
                            let mut var55 = 0;
                            if var13 * 8 + var32 < var7 {
                                if temp_val < 0.5 && var13 * 8 + var32 >= var7 - 1 {
                                    var55 = 79; // ice
                                } else {
                                    var55 = 9; // waterMoving
                                }
                            }

                            if var48 > 0.0 {
                                var55 = 1; // stone
                            }

                            blocks[var44 as usize] = var55;
                            var44 += var45;
                            var48 += var50;
                        }

                        var35 += var39;
                        var37 += var41;
                    }

                    var16 += var24;
                    var18 += var26;
                    var20 += var28;
                    var22 += var30;
                }
            }
        }
    }

    // 3. Replace blocks for biomes
    let var5_biome = 64;
    let var6_biome = 1.0 / 32.0;

    let mut biome_r = vec![0.0; 256];
    gen.biome_noise.fill3_octaves(&mut biome_r, (chunk_x * 16) as f64, (chunk_z * 16) as f64, 0.0, 16, 16, 1, var6_biome, var6_biome, 1.0);

    let mut biome_s = vec![0.0; 256];
    gen.biome_noise.fill3_octaves(&mut biome_s, (chunk_z * 16) as f64, 109.0134, (chunk_x * 16) as f64, 16, 1, 16, var6_biome, 1.0, var6_biome);

    let mut stone_t = vec![0.0; 256];
    gen.stone_noise.fill3_octaves(&mut stone_t, (chunk_x * 16) as f64, (chunk_z * 16) as f64, 0.0, 16, 16, 1, var6_biome * 2.0, var6_biome * 2.0, var6_biome * 2.0);

    for var8_b in 0..16 {
        for var9_b in 0..16 {
            let var10_b = &biomes[(var8_b * 16 + var9_b) as usize];
            let var11_b = biome_r[(var8_b + var9_b * 16) as usize] + rand.next_double() * 0.2 > 0.0;
            let var12_b = biome_s[(var8_b + var9_b * 16) as usize] + rand.next_double() * 0.2 > 3.0;
            let var13_b = (stone_t[(var8_b + var9_b * 16) as usize] / 3.0 + 3.0 + rand.next_double() * 0.25) as i32;
            let mut var14_b = -1;
            let mut var15_b = var10_b.top_block;
            let mut var16_b = var10_b.filler_block;

            for var17_b in (0..=127).rev() {
                let var18_b = ((var8_b * 16 + var9_b) * 128 + var17_b) as usize;
                if var17_b <= rand.next_int_bound(5) {
                    blocks[var18_b] = 7; // bedrock
                } else {
                    let var19_b = blocks[var18_b];
                    if var19_b == 0 {
                        var14_b = -1;
                    } else if var19_b == 1 { // stone
                        if var14_b == -1 {
                            if var13_b <= 0 {
                                var15_b = 0;
                                var16_b = 1;
                            } else if var17_b >= var5_biome - 4 && var17_b <= var5_biome + 1 {
                                var15_b = var10_b.top_block;
                                var16_b = var10_b.filler_block;
                                if var12_b { var15_b = 0; }
                                if var12_b { var16_b = 13; } // gravel
                                if var11_b { var15_b = 12; } // sand
                                if var11_b { var16_b = 12; } // sand
                            }

                            if var17_b < var5_biome && var15_b == 0 {
                                var15_b = 9; // waterMoving
                            }

                            var14_b = var13_b;
                            if var17_b >= var5_biome - 1 {
                                blocks[var18_b] = var15_b;
                            } else {
                                blocks[var18_b] = var16_b;
                            }
                        } else if var14_b > 0 {
                            var14_b -= 1;
                            blocks[var18_b] = var16_b;
                        }
                    }
                }
            }
        }
    }

    // 4. Cave generation (Rust caves gen)
    let mut gen_caves = MapGenCaves::new();
    gen_caves.generate(gen.world_seed, chunk_x, chunk_z, blocks);
}

pub fn populate_batch(
    generator: &mut ChunkProvider,
    stage_blocks: &mut [[[u8; 32768]; 2]; 2],
    stage_meta: &mut [[[u8; 32768]; 2]; 2],
    fallback: &mut dyn BlockAccess,
    chunk: [i32; 2],
    biome: BiomeType,
    temperatures: &[f64],
) -> Vec<(i32, i32, i32, i32, i32, i32)> {
    let [chunk_x, chunk_z] = chunk;
    let mut canvas = CanvasAccess::new(stage_blocks, stage_meta, chunk_x, chunk_z, fallback);

    // Call the existing decorator logic, then hand the queued
    // dungeon-chest loot back (the canvas holds no tiles).
    decorate_chunk(
        &mut canvas,
        generator.world_seed,
        chunk_x,
        chunk_z,
        biome,
        &mut generator.tree_noise,
        temperatures,
    );
    canvas.take_loot()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn populate_climate_matches_terrain_on_overlap() {
        // The populate snow slice at (cx*16+8) must agree bit-exactly
        // with the terrain temperature slice on their 8x8 overlap, and
        // the corner biome must match the neighbor chunk's cell (0,0):
        // all three evaluate the same noise at the same coordinates.
        let mut gen = ChunkProvider::new(12345);
        let (cx, cz) = (3, -2);
        let mut blocks = [0u8; 32768];
        let mut biomes = [MobSpawnerBase::DEFAULT; 256];
        let mut temps = [0.0f64; 256];
        let mut humids = [0.0f64; 256];
        generate_chunk(&mut gen, cx, cz, &mut blocks, &mut biomes, &mut temps, &mut humids);

        let mut snow = [0.0f64; 256];
        chunk_temperatures(&gen, cx * 16 + 8, cz * 16 + 8, &mut snow);
        for i in 8..16 {
            for j in 8..16 {
                assert_eq!(snow[(i - 8) * 16 + (j - 8)], temps[i * 16 + j], "cell {i},{j}");
            }
        }

        let mut nb = [MobSpawnerBase::DEFAULT; 256];
        let mut nt = [0.0f64; 256];
        let mut nh = [0.0f64; 256];
        climate_into(
            &gen,
            [(cx + 1) * 16, (cz + 1) * 16],
            [16, 16],
            &mut nb,
            &mut nt,
            &mut nh,
        );
        assert_eq!(
            nb[0].biome_type,
            point_biome(&gen, cx * 16 + 16, cz * 16 + 16).biome_type
        );
    }
}