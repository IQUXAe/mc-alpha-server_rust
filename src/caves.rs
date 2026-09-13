//! NOTE: long scalar arg lists here mirror the Java originals 1:1
//! (var-for-var parameter order); grouping them would obscure the
//! decompiled mapping. Allowed deliberately.
#![allow(clippy::too_many_arguments)]

use crate::math_helper::{cos as mcos, sin as msin};
use crate::random::JavaRandom;

pub struct MapGenCaves {
    range: i32,
    rand: JavaRandom,
}

impl Default for MapGenCaves {
    fn default() -> Self {
        Self::new()
    }
}

impl MapGenCaves {
    pub fn new() -> Self {
        Self {
            range: 8,
            rand: JavaRandom::new(0),
        }
    }

    pub fn generate(&mut self, world_seed: i64, chunk_x: i32, chunk_z: i32, blocks: &mut [u8]) {
        let range = self.range;
        self.rand.set_seed(world_seed);
        let l1 = (self.rand.next_long() / 2) * 2 + 1;
        let l2 = (self.rand.next_long() / 2) * 2 + 1;

        for x in (chunk_x - range)..=(chunk_x + range) {
            for z in (chunk_z - range)..=(chunk_z + range) {
                let seed = ((x as i64).wrapping_mul(l1).wrapping_add((z as i64).wrapping_mul(l2))) ^ world_seed;
                self.rand.set_seed(seed);
                self.generate_cave_structures(x, z, chunk_x, chunk_z, blocks);
            }
        }
    }

    fn generate_cave_structures(&mut self, x: i32, z: i32, chunk_x: i32, chunk_z: i32, blocks: &mut [u8]) {
        let step1 = self.rand.next_int_bound(40) + 1;
        let step2 = self.rand.next_int_bound(step1) + 1;
        let mut var7 = self.rand.next_int_bound(step2);
        if self.rand.next_int_bound(15) != 0 {
            var7 = 0;
        }

        for _ in 0..var7 {
            let var9 = (x * 16 + self.rand.next_int_bound(16)) as f64;
            let step1_y = self.rand.next_int_bound(120) + 8;
            let var11 = self.rand.next_int_bound(step1_y) as f64;
            let var13 = (z * 16 + self.rand.next_int_bound(16)) as f64;
            let mut var15 = 1;
            if self.rand.next_int_bound(4) == 0 {
                self.carve_room(chunk_x, chunk_z, blocks, var9, var11, var13);
                var15 += self.rand.next_int_bound(4);
            }
            for _ in 0..var15 {
                let var17 = self.rand.next_float() * std::f32::consts::PI * 2.0;
                let var18 = (self.rand.next_float() - 0.5) * 2.0 / 8.0;
                let var19 = self.rand.next_float() * 2.0 + self.rand.next_float();
                self.carve_tunnel(chunk_x, chunk_z, blocks, var9, var11, var13, var19, var17, var18, 0, 0, 1.0);
            }
        }
    }

    fn carve_room(&mut self, var1: i32, var2: i32, var3: &mut [u8], var4: f64, var6: f64, var8: f64) {
        let var10 = 1.0 + self.rand.next_float() * 6.0;
        self.carve_tunnel(var1, var2, var3, var4, var6, var8, var10, 0.0, 0.0, -1, -1, 0.5);
    }

    fn carve_tunnel(
        &mut self,
        var1: i32,
        var2: i32,
        var3: &mut [u8],
        mut var4: f64,
        mut var6: f64,
        mut var8: f64,
        var10: f32,
        mut var11: f32,
        mut var12: f32,
        mut var13: i32,
        var14_orig: i32,
        var15: f64,
    ) {
        let var17 = (var1 * 16 + 8) as f64;
        let var19 = (var2 * 16 + 8) as f64;
        let mut var21 = 0.0f32;
        let mut var22 = 0.0f32;
        let mut var23 = JavaRandom::new(self.rand.next_long());

        let mut var14 = var14_orig;
        if var14 <= 0 {
            let var24 = self.range * 16 - 16;
            var14 = var24 - var23.next_int_bound(var24 / 4);
        }
        let mut var52 = false;
        if var13 == -1 {
            var13 = var14 / 2;
            var52 = true;
        }
        let var25 = var23.next_int_bound(var14 / 2) + var14 / 4;
        let var26 = var23.next_int_bound(6) == 0;

        while var13 < var14 {
            // Java MathHelper table sin/cos (not libm): tunnel headings must
            // match the vanilla lattice bit-for-bit at the same seed.
            let var27 = 1.5 + ((msin((var13 as f32) * std::f32::consts::PI / (var14 as f32)) * var10 * 1.0) as f64);
            let var29 = var27 * var15;
            let var31 = mcos(var12);
            let var32 = msin(var12);
            var4 += (mcos(var11) * var31) as f64;
            var6 += var32 as f64;
            var8 += (msin(var11) * var31) as f64;

            if var26 {
                var12 *= 0.92;
            } else {
                var12 *= 0.7;
            }

            var12 += var22 * 0.1;
            var11 += var21 * 0.1;
            var22 *= 0.9;
            var21 *= 12.0 / 16.0;
            var22 += (var23.next_float() - var23.next_float()) * var23.next_float() * 2.0;
            var21 += (var23.next_float() - var23.next_float()) * var23.next_float() * 4.0;

            if !var52 && var13 == var25 && var10 > 1.0 {
                self.carve_tunnel(
                    var1,
                    var2,
                    var3,
                    var4,
                    var6,
                    var8,
                    var23.next_float() * 0.5 + 0.5,
                    var11 - std::f32::consts::PI * 0.5,
                    var12 / 3.0,
                    var13,
                    var14,
                    1.0,
                );
                self.carve_tunnel(
                    var1,
                    var2,
                    var3,
                    var4,
                    var6,
                    var8,
                    var23.next_float() * 0.5 + 0.5,
                    var11 + std::f32::consts::PI * 0.5,
                    var12 / 3.0,
                    var13,
                    var14,
                    1.0,
                );
                return;
            }

            if var52 || var23.next_int_bound(4) != 0 {
                let var33 = var4 - var17;
                let var35 = var8 - var19;
                let var37 = (var14 - var13) as f64;
                let var39 = (var10 + 2.0 + 16.0) as f64;
                if var33 * var33 + var35 * var35 - var37 * var37 > var39 * var39 {
                    return;
                }

                if var4 >= var17 - 16.0 - var27 * 2.0
                    && var8 >= var19 - 16.0 - var27 * 2.0
                    && var4 <= var17 + 16.0 + var27 * 2.0
                    && var8 <= var19 + 16.0 + var27 * 2.0
                {
                    let mut var53 = ((var4 - var27).floor() as i32) - var1 * 16 - 1;
                    let mut var34 = ((var4 + var27).floor() as i32) - var1 * 16 + 1;
                    let mut var54 = ((var6 - var29).floor() as i32) - 1;
                    let mut var36 = ((var6 + var29).floor() as i32) + 1;
                    let mut var55 = ((var8 - var27).floor() as i32) - var2 * 16 - 1;
                    let mut var38 = ((var8 + var27).floor() as i32) - var2 * 16 + 1;

                    if var53 < 0 {
                        var53 = 0;
                    }
                    if var34 > 16 {
                        var34 = 16;
                    }
                    if var54 < 1 {
                        var54 = 1;
                    }
                    if var36 > 120 {
                        var36 = 120;
                    }
                    if var55 < 0 {
                        var55 = 0;
                    }
                    if var38 > 16 {
                        var38 = 16;
                    }

                    let mut var56 = false;
                    for var40 in var53..var34 {
                        for var41 in var55..var38 {
                            let mut var42 = var36 + 1;
                            while var42 >= var54 - 1 {
                                if var56 {
                                    break;
                                }
                                let var43 = ((var40 * 16 + var41) * 128 + var42) as usize;
                                if (0..128).contains(&var42) {
                                    if var3[var43] == 9 || var3[var43] == 8 {
                                        var56 = true;
                                    }
                                    if var42 != var54 - 1
                                        && var40 != var53
                                        && var40 != var34 - 1
                                        && var41 != var55
                                        && var41 != var38 - 1
                                    {
                                        var42 = var54;
                                    }
                                }
                                var42 -= 1;
                            }
                        }
                    }

                    if !var56 {
                        for var40 in var53..var34 {
                            let var57 = (((var40 + var1 * 16) as f64) + 0.5 - var4) / var27;
                            for var43 in var55..var38 {
                                let var44 = (((var43 + var2 * 16) as f64) + 0.5 - var8) / var27;
                                let mut var46 = ((var40 * 16 + var43) * 128 + var36) as usize;
                                let mut var47 = false;
                                for var48 in (var54..var36).rev() {
                                    let var49 = ((var48 as f64) + 0.5 - var6) / var29;
                                    if var49 > -0.7 && var57 * var57 + var49 * var49 + var44 * var44 < 1.0 {
                                        let var51 = var3[var46];
                                        if var51 == 2 {
                                            var47 = true;
                                        }
                                        if var51 == 1 || var51 == 3 || var51 == 2 {
                                            if var48 < 10 {
                                                var3[var46] = 10; // lavaStill
                                            } else {
                                                var3[var46] = 0; // air
                                                if var47 && var3[var46 - 1] == 3 {
                                                    var3[var46 - 1] = 2; // grass
                                                }
                                            }
                                        }
                                    }
                                    var46 -= 1;
                                }
                            }
                        }
                        if var52 {
                            break;
                        }
                    }
                }
            }
            var13 += 1;
        }
    }
}
