//! NOTE: long scalar arg lists here mirror the Java originals 1:1
//! (var-for-var parameter order); grouping them would obscure the
//! decompiled mapping. Allowed deliberately.
#![allow(clippy::too_many_arguments)]

use crate::random::JavaRandom;

fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

fn grad(hash: i32, x: f64, y: f64, z: f64) -> f64 {
    let h = hash & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 { y } else if h == 12 || h == 14 { x } else { z };
    let res_u = if (h & 1) == 0 { u } else { -u };
    let res_v = if (h & 2) == 0 { v } else { -v };
    res_u + res_v
}

fn grad2(hash: i32, x: f64, z: f64) -> f64 {
    let h = hash & 15;
    let u = (1.0 - ((h & 8) >> 3) as f64) * x;
    let v = if h < 4 { 0.0 } else if h != 12 && h != 14 { z } else { x };
    let res_u = if (h & 1) == 0 { u } else { -u };
    let res_v = if (h & 2) == 0 { v } else { -v };
    res_u + res_v
}

#[derive(Clone, Debug)]
pub struct NoiseGeneratorPerlin {
    pub x_coord: f64,
    pub y_coord: f64,
    pub z_coord: f64,
    pub permutations: [i32; 512],
}

impl NoiseGeneratorPerlin {
    pub fn new(rand: &mut JavaRandom) -> Self {
        let x_coord = rand.next_double() * 256.0;
        let y_coord = rand.next_double() * 256.0;
        let z_coord = rand.next_double() * 256.0;
        let mut permutations = [0i32; 512];
        for (i, p) in permutations.iter_mut().enumerate().take(256) {
            *p = i as i32;
        }
        for i in 0..256 {
            let j = rand.next_int_bound(256 - i as i32) + i as i32;
            permutations.swap(i, j as usize);
            permutations[i + 256] = permutations[i];
        }
        Self {
            x_coord,
            y_coord,
            z_coord,
            permutations,
        }
    }

    pub fn noise(&self, mut x: f64, mut y: f64, mut z: f64) -> f64 {
        let floor_x = x.floor();
        let floor_y = y.floor();
        let floor_z = z.floor();

        let x_idx = (floor_x as i32 & 255) as usize;
        let y_idx = (floor_y as i32 & 255) as usize;
        let z_idx = (floor_z as i32 & 255) as usize;

        x -= floor_x;
        y -= floor_y;
        z -= floor_z;

        let u = x * x * x * (x * (x * 6.0 - 15.0) + 10.0);
        let v = y * y * y * (y * (y * 6.0 - 15.0) + 10.0);
        let w = z * z * z * (z * (z * 6.0 - 15.0) + 10.0);

        let a = self.permutations[x_idx] as usize + y_idx;
        let aa = self.permutations[a] as usize + z_idx;
        let ab = self.permutations[a + 1] as usize + z_idx;
        let b = self.permutations[x_idx + 1] as usize + y_idx;
        let ba = self.permutations[b] as usize + z_idx;
        let bb = self.permutations[b + 1] as usize + z_idx;

        lerp(
            w,
            lerp(
                v,
                lerp(
                    u,
                    grad(self.permutations[aa], x, y, z),
                    grad(self.permutations[ba], x - 1.0, y, z),
                ),
                lerp(
                    u,
                    grad(self.permutations[ab], x, y - 1.0, z),
                    grad(self.permutations[bb], x - 1.0, y - 1.0, z),
                ),
            ),
            lerp(
                v,
                lerp(
                    u,
                    grad(self.permutations[aa + 1], x, y, z - 1.0),
                    grad(self.permutations[ba + 1], x - 1.0, y, z - 1.0),
                ),
                lerp(
                    u,
                    grad(self.permutations[ab + 1], x, y - 1.0, z - 1.0),
                    grad(self.permutations[bb + 1], x - 1.0, y - 1.0, z - 1.0),
                ),
            ),
        )
    }

    pub fn noise_shifted(&self, x: f64, y: f64, z: f64) -> f64 {
        self.noise(x + self.x_coord, y + self.y_coord, z + self.z_coord)
    }

    /// 2D entry used by tree density (`NoiseGeneratorOctaves.sample2_octaves` →
    /// `Perlin.sample2(x,z)` → `generateNoise(x,z,0)`): samples the XY
    /// plane `noise(x+xo, z+yo, zo)`, NOT the XZ plane.
    pub fn sample2(&self, x: f64, z: f64) -> f64 {
        self.noise(x + self.x_coord, z + self.y_coord, self.z_coord)
    }

    pub fn fill3(
        &self,
        arr: &mut [f64],
        x_base: f64,
        y_base: f64,
        z_base: f64,
        x_size: usize,
        y_size: usize,
        z_size: usize,
        x_freq: f64,
        y_freq: f64,
        z_freq: f64,
        amplitude: f64,
    ) {
        let mut index = 0;
        let inv_amp = 1.0 / amplitude;
        if y_size == 1 {
            for dx in 0..x_size {
                let x = (x_base + dx as f64) * x_freq + self.x_coord;
                let mut ix = x as i32;
                if x < ix as f64 {
                    ix -= 1;
                }
                let x_idx = (ix & 255) as usize;
                let x_frac = x - ix as f64;
                let u = x_frac * x_frac * x_frac * (x_frac * (x_frac * 6.0 - 15.0) + 10.0);
                for dz in 0..z_size {
                    let z = (z_base + dz as f64) * z_freq + self.z_coord;
                    let mut iz = z as i32;
                    if z < iz as f64 {
                        iz -= 1;
                    }
                    let z_idx = (iz & 255) as usize;
                    let z_frac = z - iz as f64;
                    let w = z_frac * z_frac * z_frac * (z_frac * (z_frac * 6.0 - 15.0) + 10.0);
                    let a = self.permutations[x_idx] as usize;
                    let aa = self.permutations[a] as usize + z_idx;
                    let b = self.permutations[x_idx + 1] as usize;
                    let ba = self.permutations[b] as usize + z_idx;
                    let res1 = lerp(
                        u,
                        grad2(self.permutations[aa], x_frac, z_frac),
                        grad(self.permutations[ba], x_frac - 1.0, 0.0, z_frac),
                    );
                    let res2 = lerp(
                        u,
                        grad(self.permutations[aa + 1], x_frac, 0.0, z_frac - 1.0),
                        grad(self.permutations[ba + 1], x_frac - 1.0, 0.0, z_frac - 1.0),
                    );
                    let final_res = lerp(w, res1, res2);
                    arr[index] += final_res * inv_amp;
                    index += 1;
                }
            }
        } else {
            let mut prev_y = -1;
            let mut res_a1 = 0.0;
            let mut res_a2 = 0.0;
            let mut res_b1 = 0.0;
            let mut res_b2 = 0.0;
            for dx in 0..x_size {
                let x = (x_base + dx as f64) * x_freq + self.x_coord;
                let mut ix = x as i32;
                if x < ix as f64 {
                    ix -= 1;
                }
                let x_idx = (ix & 255) as usize;
                let x_frac = x - ix as f64;
                let u = x_frac * x_frac * x_frac * (x_frac * (x_frac * 6.0 - 15.0) + 10.0);

                for dz in 0..z_size {
                    let z = (z_base + dz as f64) * z_freq + self.z_coord;
                    let mut iz = z as i32;
                    if z < iz as f64 {
                        iz -= 1;
                    }
                    let z_idx = (iz & 255) as usize;
                    let z_frac = z - iz as f64;
                    let w = z_frac * z_frac * z_frac * (z_frac * (z_frac * 6.0 - 15.0) + 10.0);

                    for dy in 0..y_size {
                        let y = (y_base + dy as f64) * y_freq + self.y_coord;
                        let mut iy = y as i32;
                        if y < iy as f64 {
                            iy -= 1;
                        }
                        let y_idx = (iy & 255) as usize;
                        let y_frac = y - iy as f64;
                        let v = y_frac * y_frac * y_frac * (y_frac * (y_frac * 6.0 - 15.0) + 10.0);

                        if dy == 0 || y_idx as i32 != prev_y {
                            prev_y = y_idx as i32;
                            let a = self.permutations[x_idx] as usize + y_idx;
                            let aa = self.permutations[a] as usize + z_idx;
                            let ab = self.permutations[a + 1] as usize + z_idx;
                            let b = self.permutations[x_idx + 1] as usize + y_idx;
                            let ba = self.permutations[b] as usize + z_idx;
                            let bb = self.permutations[b + 1] as usize + z_idx;

                            res_a1 = lerp(
                                u,
                                grad(self.permutations[aa], x_frac, y_frac, z_frac),
                                grad(self.permutations[ba], x_frac - 1.0, y_frac, z_frac),
                            );
                            res_a2 = lerp(
                                u,
                                grad(self.permutations[ab], x_frac, y_frac - 1.0, z_frac),
                                grad(self.permutations[bb], x_frac - 1.0, y_frac - 1.0, z_frac),
                            );
                            res_b1 = lerp(
                                u,
                                grad(self.permutations[aa + 1], x_frac, y_frac, z_frac - 1.0),
                                grad(self.permutations[ba + 1], x_frac - 1.0, y_frac, z_frac - 1.0),
                            );
                            res_b2 = lerp(
                                u,
                                grad(self.permutations[ab + 1], x_frac, y_frac - 1.0, z_frac - 1.0),
                                grad(self.permutations[bb + 1], x_frac - 1.0, y_frac - 1.0, z_frac - 1.0),
                            );
                        }

                        let res1 = lerp(v, res_a1, res_a2);
                        let res2 = lerp(v, res_b1, res_b2);
                        let final_res = lerp(w, res1, res2);
                        arr[index] += final_res * inv_amp;
                        index += 1;
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct NoiseGeneratorOctaves {
    pub generator_collection: Vec<NoiseGeneratorPerlin>,
    pub octaves: usize,
}

impl NoiseGeneratorOctaves {
    pub fn new(rand: &mut JavaRandom, octaves: usize) -> Self {
        let mut generator_collection = Vec::with_capacity(octaves);
        for _ in 0..octaves {
            generator_collection.push(NoiseGeneratorPerlin::new(rand));
        }
        Self {
            generator_collection,
            octaves,
        }
    }

    pub fn generate_noise(&self, x: f64, z: f64) -> f64 {
        let mut result = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        for i in 0..self.octaves {
            result += self.generator_collection[i].noise_shifted(x * frequency, 0.0, z * frequency) * amplitude;
            amplitude *= 0.5;
            frequency *= 2.0;
        }
        result
    }

    pub fn generate_noise_3d(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut result = 0.0;
        let mut amplitude = 1.0;
        let mut frequency = 1.0;
        for i in 0..self.octaves {
            result += self.generator_collection[i].noise_shifted(x * frequency, y * frequency, z * frequency) * amplitude;
            amplitude /= 2.0;
            frequency *= 2.0;
        }
        result
    }

    pub fn fill3_octaves(
        &self,
        arr: &mut [f64],
        x: f64,
        y: f64,
        z: f64,
        x_size: usize,
        y_size: usize,
        z_size: usize,
        x_freq: f64,
        y_freq: f64,
        z_freq: f64,
    ) {
        for val in arr.iter_mut() {
            *val = 0.0;
        }

        let mut amp = 1.0;
        for i in 0..self.octaves {
            self.generator_collection[i].fill3(
                arr,
                x,
                y,
                z,
                x_size,
                y_size,
                z_size,
                x_freq * amp,
                y_freq * amp,
                z_freq * amp,
                amp,
            );
            amp /= 2.0;
        }
    }

    pub fn fill_slice(
        &self,
        arr: &mut [f64],
        x: i32,
        z: i32,
        x_size: usize,
        z_size: usize,
        x_scale: f64,
        z_scale: f64,
    ) {
        self.fill3_octaves(
            arr,
            x as f64,
            10.0,
            z as f64,
            x_size,
            1,
            z_size,
            x_scale,
            1.0,
            z_scale,
        );
    }

    pub fn sample2_octaves(&self, x: f64, z: f64) -> f64 {
        let mut result = 0.0;
        let mut scale = 1.0;
        for i in 0..self.octaves {
            result += self.generator_collection[i].sample2(x * scale, z * scale) / scale;
            scale /= 2.0;
        }
        result
    }
}

const GRAD3: [[i32; 3]; 12] = [
    [1, 1, 0], [-1, 1, 0], [1, -1, 0], [-1, -1, 0],
    [1, 0, 1], [-1, 0, 1], [1, 0, -1], [-1, 0, -1],
    [0, 1, 1], [0, -1, 1], [0, 1, -1], [0, -1, -1]
];

fn fast_floor(v: f64) -> i32 {
    if v > 0.0 {
        v as i32
    } else {
        v as i32 - 1
    }
}

fn dot2(g: &[i32; 3], x: f64, y: f64) -> f64 {
    g[0] as f64 * x + g[1] as f64 * y
}

#[derive(Clone, Debug)]
pub struct NoiseGenerator2 {
    pub x_coord: f64,
    pub y_coord: f64,
    pub z_coord: f64,
    pub perm: [i32; 512],
}

const F2: f64 = 0.3660254037844386; // 0.5 * (sqrt(3.0) - 1.0)
const G2: f64 = 0.21132486540518713; // (3.0 - sqrt(3.0)) / 6.0

impl NoiseGenerator2 {
    pub fn new(rand: &mut JavaRandom) -> Self {
        let x_coord = rand.next_double() * 256.0;
        let y_coord = rand.next_double() * 256.0;
        let z_coord = rand.next_double() * 256.0;
        let mut perm = [0i32; 512];
        for (i, p) in perm.iter_mut().enumerate().take(256) {
            *p = i as i32;
        }
        for i in 0..256 {
            let j = rand.next_int_bound(256 - i as i32) + i as i32;
            perm.swap(i, j as usize);
            perm[i + 256] = perm[i];
        }
        Self {
            x_coord,
            y_coord,
            z_coord,
            perm,
        }
    }

    pub fn fill2_single(
        &self,
        arr: &mut [f64],
        x_base: f64,
        y_base: f64,
        x_size: usize,
        y_size: usize,
        x_scale: f64,
        y_scale: f64,
        amplitude: f64,
    ) {
        let mut index = 0;
        for ix in 0..x_size {
            let x = (x_base + ix as f64) * x_scale + self.x_coord;
            for iy in 0..y_size {
                let y = (y_base + iy as f64) * y_scale + self.y_coord;

                let s = (x + y) * F2;
                let i = fast_floor(x + s);
                let j = fast_floor(y + s);
                let t = (i + j) as f64 * G2;
                let x0_coord = i as f64 - t;
                let y0_coord = j as f64 - t;
                let x0 = x - x0_coord;
                let y0 = y - y0_coord;

                let (i1, j1) = if x0 > y0 {
                    (1, 0)
                } else {
                    (0, 1)
                };

                let x1 = x0 - i1 as f64 + G2;
                let y1 = y0 - j1 as f64 + G2;
                let x2 = x0 - 1.0 + 2.0 * G2;
                let y2 = y0 - 1.0 + 2.0 * G2;

                let ii = (i & 255) as usize;
                let jj = (j & 255) as usize;
                let gi0 = (self.perm[ii + self.perm[jj] as usize] % 12) as usize;
                let gi1 = (self.perm[ii + i1 + self.perm[jj + j1] as usize] % 12) as usize;
                let gi2 = (self.perm[ii + 1 + self.perm[jj + 1] as usize] % 12) as usize;

                let mut t0 = 0.5 - x0 * x0 - y0 * y0;
                let n0 = if t0 < 0.0 {
                    0.0
                } else {
                    t0 *= t0;
                    t0 * t0 * dot2(&GRAD3[gi0], x0, y0)
                };

                let mut t1 = 0.5 - x1 * x1 - y1 * y1;
                let n1 = if t1 < 0.0 {
                    0.0
                } else {
                    t1 *= t1;
                    t1 * t1 * dot2(&GRAD3[gi1], x1, y1)
                };

                let mut t2 = 0.5 - x2 * x2 - y2 * y2;
                let n2 = if t2 < 0.0 {
                    0.0
                } else {
                    t2 *= t2;
                    t2 * t2 * dot2(&GRAD3[gi2], x2, y2)
                };

                arr[index] += 70.0 * (n0 + n1 + n2) * amplitude;
                index += 1;
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct NoiseGeneratorOctaves2 {
    pub generators: Vec<NoiseGenerator2>,
    pub num_octaves: usize,
}

impl NoiseGeneratorOctaves2 {
    pub fn new(rand: &mut JavaRandom, octaves: usize) -> Self {
        let mut generators = Vec::with_capacity(octaves);
        for _ in 0..octaves {
            generators.push(NoiseGenerator2::new(rand));
        }
        Self {
            generators,
            num_octaves: octaves,
        }
    }

    pub fn fill2_default(
        &self,
        arr: &mut [f64],
        x: f64,
        y: f64,
        x_size: usize,
        y_size: usize,
        x_scale: f64,
        y_scale: f64,
        amplitude: f64,
    ) {
        self.fill2_lacunar(arr, x, y, x_size, y_size, x_scale, y_scale, amplitude, 0.5);
    }

    pub fn fill2_lacunar(
        &self,
        arr: &mut [f64],
        x: f64,
        y: f64,
        x_size: usize,
        y_size: usize,
        mut x_scale: f64,
        mut y_scale: f64,
        amplitude: f64,
        lacunarity: f64,
    ) {
        x_scale /= 1.5;
        y_scale /= 1.5;

        for val in arr.iter_mut() {
            *val = 0.0;
        }

        let mut persistence_sum = 1.0;
        let mut freq_mul = 1.0;

        for i in 0..self.num_octaves {
            self.generators[i].fill2_single(
                arr,
                x,
                y,
                x_size,
                y_size,
                x_scale * freq_mul,
                y_scale * freq_mul,
                0.55 / persistence_sum,
            );
            freq_mul *= amplitude;
            persistence_sum *= lacunarity;
        }
    }
}
