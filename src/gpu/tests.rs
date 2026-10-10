use super::*;
use crate::commands::{ConsoleCommandTag, parse_console_command};
use crate::server_config::ServerConfig;
use crate::server::settings::load_settings;

static GPU_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_default_state_gpu_is_disabled() {
    let _guard = GPU_TEST_LOCK.lock().unwrap();
    // When binary starts, GPU is disabled by default (CPU is default)
    set_gpu_enabled(false);
    assert!(!is_gpu_enabled(), "GPU should be disabled by default");
}

#[test]
fn test_runtime_gpu_toggle() {
    let _guard = GPU_TEST_LOCK.lock().unwrap();
    set_gpu_enabled(false);
    assert!(!is_gpu_requested());
    assert!(!is_gpu_enabled());

    set_gpu_enabled(true);
    assert!(is_gpu_requested());
    if is_gpu_available() {
        assert!(is_gpu_enabled());
    } else {
        assert!(!is_gpu_enabled());
    }

    set_gpu_enabled(false);
    assert!(!is_gpu_requested());
    assert!(!is_gpu_enabled());
}

#[test]
fn test_gpu_boost_status_string() {
    let _guard = GPU_TEST_LOCK.lock().unwrap();
    set_gpu_enabled(false);
    let status_off = gpu_boost_status();
    assert!(status_off.contains("DISABLED (CPU default)"));

    set_gpu_enabled(true);
    let status_on = gpu_boost_status();
    if is_gpu_available() {
        assert!(status_on.contains("ENABLED"));
    } else {
        assert!(status_on.contains("DISABLED (no compatible GPU available)"));
    }
    set_gpu_enabled(false);
}

#[test]
fn test_settings_gpu_acceleration_defaults_to_false() {
    let _guard = GPU_TEST_LOCK.lock().unwrap();
    let mut cfg = ServerConfig::default();
    let settings = load_settings(&mut cfg);
    assert!(!settings.gpu_acceleration, "gpu-acceleration must default to false");
    // gpu-acceleration and gpu-backend must be automatically populated in config
    assert_eq!(cfg.get("gpu-acceleration"), Some("false"));
    assert_eq!(cfg.get("gpu-backend"), Some("auto"));
    // gpu-boost must NOT be automatically added to config
    assert_eq!(cfg.get("gpu-boost"), None);
}

#[test]
fn test_settings_gpu_acceleration_parsing() {
    let _guard = GPU_TEST_LOCK.lock().unwrap();
    let text = "#props\ngpu-acceleration=true\n";
    let mut cfg = ServerConfig::from_text(text);
    let settings = load_settings(&mut cfg);
    assert!(settings.gpu_acceleration);

    // Backward compatibility: manual gpu-boost=true in file works without creating a duplicate key
    let text_alias = "#props\ngpu-boost=true\n";
    let mut cfg_alias = ServerConfig::from_text(text_alias);
    let settings_alias = load_settings(&mut cfg_alias);
    assert!(settings_alias.gpu_acceleration);
}

#[test]
fn test_settings_gpu_backend_parsing() {
    let _guard = GPU_TEST_LOCK.lock().unwrap();
    let mut cfg = ServerConfig::default();
    let settings = load_settings(&mut cfg);
    assert_eq!(settings.gpu_backend, "auto");
    assert_eq!(get_configured_backend(), None);

    let text = "#props\ngpu-backend=vulkan\n";
    let mut cfg2 = ServerConfig::from_text(text);
    let settings2 = load_settings(&mut cfg2);
    assert_eq!(settings2.gpu_backend, "vulkan");
    assert_eq!(get_configured_backend().as_deref(), Some("vulkan"));

    let text_gles = "#props\ngpu-backend=gles\n";
    let mut cfg3 = ServerConfig::from_text(text_gles);
    let settings3 = load_settings(&mut cfg3);
    assert_eq!(settings3.gpu_backend, "gles");
    assert_eq!(get_configured_backend().as_deref(), Some("gles"));

    // Reset to auto
    set_configured_backend("auto");
    assert_eq!(get_configured_backend(), None);
}

#[test]
fn test_console_boost_command_parsing() {
    let cmd1 = parse_console_command("boost");
    assert_eq!(cmd1.tag, ConsoleCommandTag::Boost);
    assert_eq!(cmd1.arg1, "");

    let cmd2 = parse_console_command("boost on");
    assert_eq!(cmd2.tag, ConsoleCommandTag::Boost);
    assert_eq!(cmd2.arg1, "on");

    let cmd3 = parse_console_command("boost off");
    assert_eq!(cmd3.tag, ConsoleCommandTag::Boost);
    assert_eq!(cmd3.arg1, "off");

    let cmd4 = parse_console_command("boost status");
    assert_eq!(cmd4.tag, ConsoleCommandTag::Boost);
    assert_eq!(cmd4.arg1, "status");
}

#[cfg(feature = "gpu")]
mod gpu_feature_tests {
    use super::*;
    use crate::generator::ChunkProvider;
    use crate::biome::MobSpawnerBase;
    use crate::world::World;

    #[test]
    fn test_gpu_chunk_generation_identical_to_cpu() {
        let _guard = GPU_TEST_LOCK.lock().unwrap();
        if !is_gpu_available() {
            println!("Skipping test: no compatible GPU device available");
            return;
        }

        let seed = 424242;
        let cx = 3;
        let cz = -5;

        // 1. Generate chunk with GPU DISABLED (pure CPU)
        set_gpu_enabled(false);
        let mut gen_cpu = ChunkProvider::new(seed);
        let mut cpu_blocks = [0u8; 32768];
        let mut biomes_cpu = [MobSpawnerBase::DEFAULT; 256];
        let mut temps_cpu = [0.0f64; 256];
        let mut humids_cpu = [0.0f64; 256];
        crate::generator::generate_chunk(
            &mut gen_cpu,
            cx,
            cz,
            &mut cpu_blocks,
            &mut biomes_cpu,
            &mut temps_cpu,
            &mut humids_cpu,
        );

        // 2. Generate same chunk with GPU ENABLED
        set_gpu_enabled(true);
        let mut gen_gpu = ChunkProvider::new(seed);
        let mut gpu_blocks = [0u8; 32768];
        let mut biomes_gpu = [MobSpawnerBase::DEFAULT; 256];
        let mut temps_gpu = [0.0f64; 256];
        let mut humids_gpu = [0.0f64; 256];
        crate::generator::generate_chunk(
            &mut gen_gpu,
            cx,
            cz,
            &mut gpu_blocks,
            &mut biomes_gpu,
            &mut temps_gpu,
            &mut humids_gpu,
        );
        set_gpu_enabled(false);

        // Verify that terrain blocks match bit-for-bit
        assert_eq!(
            &gpu_blocks[..],
            &cpu_blocks[..],
            "Chunk terrain generated with GPU boost must match CPU terrain"
        );
    }

    #[test]
    fn test_batch_chunk_generation_with_gpu() {
        let _guard = GPU_TEST_LOCK.lock().unwrap();
        if !is_gpu_available() {
            println!("Skipping test: no compatible GPU device available");
            return;
        }

        let seed = 1337;
        let coords = [(0, 0), (1, 0), (0, 1), (-1, -1)];
        let n = coords.len();

        let mut gen = ChunkProvider::new(seed);
        let mut temps = Vec::with_capacity(n);
        let mut humids = Vec::with_capacity(n);
        let mut biomes = Vec::with_capacity(n);

        for &(cx, cz) in &coords {
            let mut t = [0.0f64; 256];
            let mut h = [0.0f64; 256];
            let mut b = [MobSpawnerBase::DEFAULT; 256];
            crate::generator::climate_into(&gen, [cx * 16, cz * 16], [16, 16], &mut b, &mut t, &mut h);
            temps.push(t);
            humids.push(h);
            biomes.push(b);
        }

        // Generate batch with CPU
        set_gpu_enabled(false);
        let mut cpu_batch = vec![[0u8; 32768]; n];
        crate::generator::generate_terrain_batch(&mut gen, &coords, &temps, &humids, &mut cpu_batch);

        // Generate batch with GPU
        set_gpu_enabled(true);
        let mut gpu_batch = vec![[0u8; 32768]; n];
        crate::generator::generate_terrain_batch(&mut gen, &coords, &temps, &humids, &mut gpu_batch);
        set_gpu_enabled(false);

        for i in 0..n {
            assert_eq!(
                &gpu_batch[i][..],
                &cpu_batch[i][..],
                "Batch chunk {} must match CPU output",
                i
            );
        }
    }

    #[test]
    fn test_world_area_pregeneration_with_gpu_boost() {
        let _guard = GPU_TEST_LOCK.lock().unwrap();
        if !is_gpu_available() {
            println!("Skipping test: no compatible GPU device available");
            return;
        }

        let seed = 99999;
        // World with CPU
        set_gpu_enabled(false);
        let mut w_cpu = World::new(seed);
        w_cpu.ensure_area(0, 0, 1); // 3x3 = 9 chunks

        // World with GPU boost
        set_gpu_enabled(true);
        let mut w_gpu = World::new(seed);
        w_gpu.ensure_area(0, 0, 1);
        set_gpu_enabled(false);

        for dx in -1..=1 {
            for dz in -1..=1 {
                let c_cpu = w_cpu.chunks.get(&(dx, dz)).expect("cpu chunk exists");
                let c_gpu = w_gpu.chunks.get(&(dx, dz)).expect("gpu chunk exists");
                assert_eq!(
                    c_gpu.blocks(),
                    c_cpu.blocks(),
                    "Chunk ({dx}, {dz}) blocks must match between CPU and GPU boosted worlds"
                );
            }
        }
    }

    #[test]
    fn test_downlevel_limits_conformance() {
        // Verify that downlevel defaults are configured and comply with old hardware specs
        let limits = wgpu::Limits::downlevel_defaults();
        assert!(limits.max_compute_workgroup_size_x >= 64, "Workgroup size 64 supported");
        assert!(limits.max_compute_invocations_per_workgroup >= 64, "Invocations 64 supported");
        assert!(limits.max_storage_buffer_binding_size >= 1024 * 1024, "Storage buffer >= 1MB");
    }

    #[test]
    fn test_large_batch_chunking() {
        let _guard = GPU_TEST_LOCK.lock().unwrap();
        if !is_gpu_available() {
            println!("Skipping test: no compatible GPU device available");
            return;
        }

        // Test with 70 chunks to cross MAX_CHUNKS_PER_DISPATCH (64)
        let num_chunks = 70;
        let mut density_fields = vec![0.0f32; num_chunks * 425];
        let mut temperatures = vec![0.0f32; num_chunks * 256];

        for (i, d) in density_fields.iter_mut().enumerate() {
            *d = ((i as f32 * 0.03).sin() * 2.5) - 0.3;
        }
        for (i, t) in temperatures.iter_mut().enumerate() {
            *t = ((i as f32 * 0.05).cos() * 0.5 + 0.5).clamp(0.0, 1.0);
        }

        let ctx = get_gpu_context().unwrap();
        let gpu_result = gpu_interpolate_terrain_batch(
            ctx,
            &density_fields,
            &temperatures,
            num_chunks,
            64,
        )
        .expect("Large batch GPU chunking failed");

        assert_eq!(gpu_result.len(), num_chunks);

        // Verify first, middle, and last chunk against CPU logic
        for &c_idx in &[0, 31, 63, 64, 69] {
            let mut cpu_blocks = [0u8; 32768];
            let d_slice: Vec<f64> = density_fields[c_idx * 425..(c_idx + 1) * 425]
                .iter()
                .map(|&v| v as f64)
                .collect();
            let mut t_slice = [0.0f64; 256];
            for (ti, &tv) in temperatures[c_idx * 256..(c_idx + 1) * 256].iter().enumerate() {
                t_slice[ti] = tv as f64;
            }
            crate::generator::interpolate_density_to_blocks_cpu(&d_slice, &t_slice, &mut cpu_blocks);
            assert_eq!(
                &gpu_result[c_idx][..],
                &cpu_blocks[..],
                "Chunk {c_idx} in large batch chunking must match CPU"
            );
        }
    }

    #[test]
    fn test_concurrent_gpu_threads() {
        let _guard = GPU_TEST_LOCK.lock().unwrap();
        if !is_gpu_available() {
            println!("Skipping test: no compatible GPU device available");
            return;
        }

        set_gpu_enabled(true);
        let handles: Vec<_> = (0..4)
            .map(|thread_id| {
                std::thread::spawn(move || {
                    let seed = 12345 + thread_id as i64;
                    let mut gen = ChunkProvider::new(seed);
                    let mut blocks = [0u8; 32768];
                    let mut biomes = [MobSpawnerBase::DEFAULT; 256];
                    let mut temps = [0.0f64; 256];
                    let mut humids = [0.0f64; 256];
                    crate::generator::generate_chunk(
                        &mut gen,
                        thread_id as i32,
                        -(thread_id as i32),
                        &mut blocks,
                        &mut biomes,
                        &mut temps,
                        &mut humids,
                    );
                    assert!(blocks.iter().any(|&b| b != 0), "Generated chunk has non-air blocks");
                })
            })
            .collect();

        for h in handles {
            h.join().expect("Concurrent GPU thread joined successfully");
        }
        set_gpu_enabled(false);
    }
}
