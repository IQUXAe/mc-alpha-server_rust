//! NOTE: long scalar arg lists here mirror the Java originals 1:1
//! (var-for-var parameter order); grouping them would obscure the
//! decompiled mapping. Allowed deliberately.
#![allow(clippy::too_many_arguments)]

use crate::noise::NoiseGeneratorOctaves;

pub fn density_generate_field(
    field: &mut [f64],
    var2: i32,
    var3: i32,
    var4: i32,
    var5: i32,
    var6: i32,
    var7: i32,
    temperatures: &[f64],
    humidities: &[f64],
    depth_gen: &NoiseGeneratorOctaves,
    scale_gen: &NoiseGeneratorOctaves,
    main_gen: &NoiseGeneratorOctaves,
    min_gen: &NoiseGeneratorOctaves,
    max_gen: &NoiseGeneratorOctaves,
) {
    let var8 = 684.412;
    let var10 = 684.412;

    let mut noise_depth = vec![0.0; (var5 * var7) as usize];
    let mut noise_scale = vec![0.0; (var5 * var7) as usize];
    let mut noise_main = vec![0.0; (var5 * var6 * var7) as usize];
    let mut noise_min = vec![0.0; (var5 * var6 * var7) as usize];
    let mut noise_max = vec![0.0; (var5 * var6 * var7) as usize];

    depth_gen.fill_slice(&mut noise_depth, var2, var4, var5 as usize, var7 as usize, 1.121, 1.121);
    scale_gen.fill_slice(&mut noise_scale, var2, var4, var5 as usize, var7 as usize, 200.0, 200.0);
    main_gen.fill3_octaves(&mut noise_main, var2 as f64, var3 as f64, var4 as f64, var5 as usize, var6 as usize, var7 as usize, var8 / 80.0, var10 / 160.0, var8 / 80.0);
    min_gen.fill3_octaves(&mut noise_min, var2 as f64, var3 as f64, var4 as f64, var5 as usize, var6 as usize, var7 as usize, var8, var10, var8);
    max_gen.fill3_octaves(&mut noise_max, var2 as f64, var3 as f64, var4 as f64, var5 as usize, var6 as usize, var7 as usize, var8, var10, var8);

    let mut var14 = 0;
    let mut var15 = 0;
    if var5 <= 0 {
        return;
    }
    let var16 = 16 / var5;

    for var17 in 0..var5 {
        let var18 = var17 * var16 + var16 / 2;
        for var19 in 0..var7 {
            let var20 = var19 * var16 + var16 / 2;
            let var21 = temperatures[(var18 * 16 + var20) as usize];
            let var23 = humidities[(var18 * 16 + var20) as usize] * var21;
            let mut var25 = 1.0 - var23;
            var25 *= var25;
            var25 *= var25;
            var25 = 1.0 - var25;
            let mut var27 = (noise_depth[var15] + 256.0) / 512.0;
            var27 *= var25;
            if var27 > 1.0 { var27 = 1.0; }

            let mut var29 = noise_scale[var15] / 8000.0;
            if var29 < 0.0 { var29 = -var29 * 0.3; }

            var29 = var29 * 3.0 - 2.0;
            if var29 < 0.0 {
                var29 /= 2.0;
                if var29 < -1.0 { var29 = -1.0; }
                var29 /= 1.4;
                var29 /= 2.0;
                var27 = 0.0;
            } else {
                if var29 > 1.0 { var29 = 1.0; }
                var29 /= 8.0;
            }

            if var27 < 0.0 { var27 = 0.0; }

            var27 += 0.5;
            var29 = var29 * var6 as f64 / 16.0;
            let var31 = var6 as f64 / 2.0 + var29 * 4.0;
            var15 += 1;

            for var33 in 0..var6 {
                let mut var34;
                let mut var36 = (var33 as f64 - var31) * 12.0 / var27;
                if var36 < 0.0 { var36 *= 4.0; }

                let var38 = noise_min[var14] / 512.0;
                let var40 = noise_max[var14] / 512.0;
                let var42 = (noise_main[var14] / 10.0 + 1.0) / 2.0;

                if var42 < 0.0 {
                    var34 = var38;
                } else if var42 > 1.0 {
                    var34 = var40;
                } else {
                    var34 = var38 + (var40 - var38) * var42;
                }

                var34 -= var36;
                if var33 > var6 - 4 {
                    let var44 = (var33 - (var6 - 4)) as f64 / 3.0;
                    var34 = var34 * (1.0 - var44) + -10.0 * var44;
                }

                field[var14] = var34;
                var14 += 1;
            }
        }
    }
}
