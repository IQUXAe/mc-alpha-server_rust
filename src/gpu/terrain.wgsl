struct Uniforms {
    num_chunks: u32,
    sea_level: i32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

// 425 f32 elements per chunk: 5 * 5 * 17
@group(0) @binding(1)
var<storage, read> density_fields: array<f32>;

// 256 f32 elements per chunk: 16 * 16
@group(0) @binding(2)
var<storage, read> temperatures: array<f32>;

// 8192 u32 elements (32768 bytes) per chunk
@group(0) @binding(3)
var<storage, read_write> output_blocks: array<u32>;

fn sample_block(chunk_idx: u32, bx: u32, by: u32, bz: u32) -> u32 {
    let var11 = bx / 4u;
    let var43 = bx % 4u;
    let var12 = bz / 4u;
    let var52 = bz % 4u;
    let var13 = by / 8u;
    let var32 = by % 8u;

    let var10 = 5u;
    let var9 = 17u;

    let density_base = chunk_idx * 425u;
    let idx00 = ((var11 * var10 + var12) * var9 + var13);
    let idx01 = idx00 + 1u;
    let idx10 = (((var11 * var10 + var12 + 1u)) * var9 + var13);
    let idx11 = idx10 + 1u;
    let idx20 = ((((var11 + 1u) * var10 + var12)) * var9 + var13);
    let idx21 = idx20 + 1u;
    let idx30 = ((((var11 + 1u) * var10 + var12 + 1u)) * var9 + var13);
    let idx31 = idx30 + 1u;

    let d00 = density_fields[density_base + idx00];
    let d01 = density_fields[density_base + idx01];
    let d10 = density_fields[density_base + idx10];
    let d11 = density_fields[density_base + idx11];
    let d20 = density_fields[density_base + idx20];
    let d21 = density_fields[density_base + idx21];
    let d30 = density_fields[density_base + idx30];
    let d31 = density_fields[density_base + idx31];

    let y_frac = f32(var32) * 0.125;
    let v16 = d00 + (d01 - d00) * y_frac;
    let v18 = d10 + (d11 - d10) * y_frac;
    let v20 = d20 + (d21 - d20) * y_frac;
    let v22 = d30 + (d31 - d30) * y_frac;

    let x_frac = f32(var43) * 0.25;
    let v35 = v16 + (v20 - v16) * x_frac;
    let v37 = v18 + (v22 - v18) * x_frac;

    let z_frac = f32(var52) * 0.25;
    let dens = v35 + (v37 - v35) * z_frac;

    let temp_val = temperatures[chunk_idx * 256u + bx * 16u + bz];
    var block_id: u32 = 0u;
    if (i32(by) < uniforms.sea_level) {
        if (temp_val < 0.5 && i32(by) >= uniforms.sea_level - 1) {
            block_id = 79u; // Ice
        } else {
            block_id = 9u;  // Water
        }
    }
    if (dens > 0.0) {
        block_id = 1u; // Stone
    }
    return block_id;
}

@compute @workgroup_size(64, 1, 1)
fn main(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let total_words = uniforms.num_chunks * 8192u;
    let word_index = global_id.x;
    if (word_index >= total_words) {
        return;
    }

    let chunk_idx = word_index / 8192u;
    let local_word = word_index % 8192u;

    let bx = (local_word >> 9u) & 15u;
    let bz = (local_word >> 5u) & 15u;
    let y_group = local_word & 31u;
    let base_y = y_group * 4u;

    let b0 = sample_block(chunk_idx, bx, base_y + 0u, bz);
    let b1 = sample_block(chunk_idx, bx, base_y + 1u, bz);
    let b2 = sample_block(chunk_idx, bx, base_y + 2u, bz);
    let b3 = sample_block(chunk_idx, bx, base_y + 3u, bz);

    let packed_blocks = b0 | (b1 << 8u) | (b2 << 16u) | (b3 << 24u);
    output_blocks[word_index] = packed_blocks;
}
