//! Implementation of GPU acceleration using wgpu.

use std::sync::OnceLock;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct TerrainUniforms {
    num_chunks: u32,
    sea_level: i32,
    _pad0: u32,
    _pad1: u32,
}

pub struct TerrainPipeline {
    pub pipeline: wgpu::ComputePipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
}

pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter_info: wgpu::AdapterInfo,
    pub terrain: TerrainPipeline,
}

static GPU_CTX: OnceLock<Option<GpuContext>> = OnceLock::new();

pub fn get_gpu_context() -> Option<&'static GpuContext> {
    GPU_CTX.get_or_init(init_gpu_context).as_ref()
}

pub fn is_gpu_available() -> bool {
    get_gpu_context().is_some()
}

pub fn gpu_device_info() -> String {
    match get_gpu_context() {
        Some(ctx) => format!(
            "{} ({:?}, {:?})",
            ctx.adapter_info.name, ctx.adapter_info.backend, ctx.adapter_info.device_type
        ),
        None => "No compatible GPU device available".to_string(),
    }
}

fn init_gpu_context() -> Option<GpuContext> {
    // Check environment variable ALPHA_GPU_BACKEND or server.properties gpu-backend: vulkan, gl, gles, dx12, metal, all
    let backend_env = std::env::var("ALPHA_GPU_BACKEND")
        .ok()
        .or_else(crate::gpu::get_configured_backend)
        .unwrap_or_default()
        .to_lowercase();
    let requested_backends = match backend_env.as_str() {
        "vulkan" => wgpu::Backends::VULKAN,
        "gl" | "gles" | "opengl" => wgpu::Backends::GL,
        "dx12" => wgpu::Backends::DX12,
        "metal" => wgpu::Backends::METAL,
        _ => wgpu::Backends::all(),
    };

    let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle();
    instance_desc.backends = requested_backends;
    instance_desc.flags = wgpu::InstanceFlags::empty();
    instance_desc.backend_options.gl.gles_minor_version = wgpu::Gles3MinorVersion::Version1;
    let instance = wgpu::Instance::new(instance_desc);

    // Enumerate candidate adapters
    let mut candidates = pollster::block_on(instance.enumerate_adapters(requested_backends));

    // Fall back to request_adapter if enumeration returned nothing
    if candidates.is_empty() {
        if let Ok(a) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            ..Default::default()
        })) {
            candidates.push(a);
        } else if let Ok(a) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            ..Default::default()
        })) {
            candidates.push(a);
        } else if let Ok(a) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
            force_fallback_adapter: true,
            ..Default::default()
        })) {
            candidates.push(a);
        }
    }

    if candidates.is_empty() {
        crate::server_log::warning("No GPU adapters detected; using CPU generator default.");
        return None;
    }

    // Sort candidates: prefer devices with compute shaders, then Discrete > Integrated > Other > Cpu
    candidates.sort_by_key(|a| {
        let dl = a.get_downlevel_capabilities();
        let has_compute = dl.flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS);
        let info = a.get_info();
        let rank = match info.device_type {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            wgpu::DeviceType::VirtualGpu => 2,
            wgpu::DeviceType::Other => 3,
            wgpu::DeviceType::Cpu => 4,
        };
        (!has_compute, rank)
    });

    for adapter in candidates {
        let adapter_info = adapter.get_info();
        let downlevel = adapter.get_downlevel_capabilities();
        if !downlevel.flags.contains(wgpu::DownlevelFlags::COMPUTE_SHADERS) {
            crate::server_log::warning(&format!(
                "GPU adapter {} ({:?}) does not support compute shaders, skipping.",
                adapter_info.name, adapter_info.backend
            ));
            continue;
        }

        // Try downlevel limits first, fallback to adapter limits if needed
        let dev_result = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("alpha_server_gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            ..Default::default()
        }))
        .or_else(|_| {
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("alpha_server_gpu_adapter_limits"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                ..Default::default()
            }))
        });

        let (device, queue) = match dev_result {
            Ok(pair) => pair,
            Err(e) => {
                crate::server_log::warning(&format!(
                    "Failed to create GPU device for {} ({:?}): {e:?}",
                    adapter_info.name, adapter_info.backend
                ));
                continue;
            }
        };

        // Set uncaptured error handler to avoid aborting the server process
        device.on_uncaptured_error(std::sync::Arc::new(|err| {
            crate::server_log::warning(&format!("GPU uncaptured error: {err}"));
        }));

        // Compile terrain compute shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("terrain.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terrain_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terrain_pipeline_layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // Use error scopes to ensure pipeline compiles cleanly without panicking
        let guard_val = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let guard_int = device.push_error_scope(wgpu::ErrorFilter::Internal);

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("terrain_compute_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let err_int = pollster::block_on(guard_int.pop());
        let err_val = pollster::block_on(guard_val.pop());
        if err_int.is_some() || err_val.is_some() {
            crate::server_log::warning(&format!(
                "Compute pipeline creation failed on {} ({:?}): {:?} {:?}",
                adapter_info.name, adapter_info.backend, err_int, err_val
            ));
            continue;
        }

        let ctx = GpuContext {
            device,
            queue,
            adapter_info: adapter_info.clone(),
            terrain: TerrainPipeline {
                pipeline,
                bind_group_layout,
            },
        };

        // Smoke test: verify a single 1-chunk dispatch to guarantee end-to-end execution
        let dummy_density = vec![0.0f32; 425];
        let dummy_temp = vec![0.5f32; 256];
        if let Err(e) = gpu_interpolate_single_dispatch(&ctx, &dummy_density, &dummy_temp, 1, 64) {
            crate::server_log::warning(&format!(
                "GPU smoke test failed on {} ({:?}): {e}; skipping.",
                adapter_info.name, adapter_info.backend
            ));
            continue;
        }

        crate::server_log::info(&format!(
            "GPU context initialized: {} ({:?}, {:?})",
            adapter_info.name, adapter_info.backend, adapter_info.device_type
        ));

        return Some(ctx);
    }

    crate::server_log::warning(
        "No GPU adapter with working compute shader support found; using CPU generator default.",
    );
    None
}

static GPU_DISPATCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Maximum number of chunks processed in a single GPU dispatch.
/// Keeps buffer allocations modest (~2 MB) and dispatch latency minimal (<5 ms),
/// avoiding OS driver timeouts (TDR) and staying well within legacy hardware limits.
pub const MAX_CHUNKS_PER_DISPATCH: usize = 64;

/// GPU-accelerated batch terrain trilinear block generation.
///
/// Converts 5x17x5 density fields + 16x16 temperatures into 32,768 block IDs per chunk.
/// Large batches are automatically chunked into manageable dispatches to protect
/// older GPU drivers from memory exhaustion or execution timeouts.
pub fn gpu_interpolate_terrain_batch(
    ctx: &GpuContext,
    density_fields: &[f32],
    temperatures: &[f32],
    num_chunks: usize,
    sea_level: i32,
) -> Result<Vec<[u8; 32768]>, String> {
    if num_chunks == 0 {
        return Ok(Vec::new());
    }
    if density_fields.len() != num_chunks * 425 {
        return Err(format!(
            "Invalid density fields length: expected {}, got {}",
            num_chunks * 425,
            density_fields.len()
        ));
    }
    if temperatures.len() != num_chunks * 256 {
        return Err(format!(
            "Invalid temperatures length: expected {}, got {}",
            num_chunks * 256,
            temperatures.len()
        ));
    }

    // Synchronize dispatches to prevent concurrent queue access on finicky drivers
    let _guard = GPU_DISPATCH_LOCK.lock().unwrap();

    if num_chunks <= MAX_CHUNKS_PER_DISPATCH {
        return gpu_interpolate_single_dispatch(ctx, density_fields, temperatures, num_chunks, sea_level);
    }

    let mut result = Vec::with_capacity(num_chunks);
    let mut offset = 0;
    while offset < num_chunks {
        let count = (num_chunks - offset).min(MAX_CHUNKS_PER_DISPATCH);
        let d_slice = &density_fields[offset * 425..(offset + count) * 425];
        let t_slice = &temperatures[offset * 256..(offset + count) * 256];
        let sub = gpu_interpolate_single_dispatch(ctx, d_slice, t_slice, count, sea_level)?;
        result.extend(sub);
        offset += count;
    }

    Ok(result)
}

fn gpu_interpolate_single_dispatch(
    ctx: &GpuContext,
    density_fields: &[f32],
    temperatures: &[f32],
    num_chunks: usize,
    sea_level: i32,
) -> Result<Vec<[u8; 32768]>, String> {
    let uniforms = TerrainUniforms {
        num_chunks: num_chunks as u32,
        sea_level,
        _pad0: 0,
        _pad1: 0,
    };

    let uniform_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain_uniform_buffer"),
        contents: bytemuck::bytes_of(&uniforms),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let density_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain_density_buffer"),
        contents: bytemuck::cast_slice(density_fields),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let temp_buffer = ctx.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("terrain_temp_buffer"),
        contents: bytemuck::cast_slice(temperatures),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let total_words = (num_chunks * 8192) as u64;
    let byte_size = total_words * 4;

    let output_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain_output_buffer"),
        size: byte_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });

    let staging_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("terrain_staging_buffer"),
        size: byte_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terrain_bind_group"),
        layout: &ctx.terrain.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: density_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: temp_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: output_buffer.as_entire_binding(),
            },
        ],
    });

    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("terrain_encoder"),
    });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("terrain_compute_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&ctx.terrain.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        let workgroups = (total_words as u32).div_ceil(64);
        pass.dispatch_workgroups(workgroups, 1, 1);
    }

    encoder.copy_buffer_to_buffer(&output_buffer, 0, &staging_buffer, 0, byte_size);
    ctx.queue.submit(Some(encoder.finish()));

    let buffer_slice = staging_buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });

    ctx.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("GPU device poll error: {e:?}"))?;

    rx.recv()
        .map_err(|e| format!("Channel receive error: {e:?}"))?
        .map_err(|e| format!("Buffer map async error: {e:?}"))?;

    let mapped = buffer_slice
        .get_mapped_range()
        .map_err(|e| format!("Get mapped range error: {e:?}"))?;
    let raw_bytes: &[u8] = &mapped;

    let mut result_chunks = Vec::with_capacity(num_chunks);
    for chunk_idx in 0..num_chunks {
        let start = chunk_idx * 32768;
        let end = start + 32768;
        let mut chunk_blocks = [0u8; 32768];
        chunk_blocks.copy_from_slice(&raw_bytes[start..end]);
        result_chunks.push(chunk_blocks);
    }

    drop(mapped);
    staging_buffer.unmap();

    Ok(result_chunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gpu_terrain_interpolation_matches_cpu() {
        if !is_gpu_available() {
            println!("Skipping GPU test: no compatible GPU device available");
            return;
        }

        let ctx = get_gpu_context().unwrap();
        println!("Testing GPU on device: {}", gpu_device_info());

        // Prepare test data for 2 chunks
        let num_chunks = 2;
        let mut density_fields = vec![0.0f32; num_chunks * 425];
        let mut temperatures = vec![0.0f32; num_chunks * 256];

        for (i, d) in density_fields.iter_mut().enumerate() {
            *d = ((i as f32 * 0.13).sin() * 2.0) - 0.5;
        }
        for (i, t) in temperatures.iter_mut().enumerate() {
            *t = ((i as f32 * 0.07).cos() * 0.5 + 0.5).clamp(0.0, 1.0);
        }

        let gpu_result = gpu_interpolate_terrain_batch(
            ctx,
            &density_fields,
            &temperatures,
            num_chunks,
            64,
        )
        .expect("GPU interpolation failed");

        assert_eq!(gpu_result.len(), num_chunks);

        // Compute reference on CPU using exact same logic
        for chunk_idx in 0..num_chunks {
            let mut cpu_blocks = [0u8; 32768];
            let d_slice = &density_fields[chunk_idx * 425..(chunk_idx + 1) * 425];
            let t_slice = &temperatures[chunk_idx * 256..(chunk_idx + 1) * 256];

            for var11 in 0..4usize {
                for var12 in 0..4usize {
                    for var13 in 0..16usize {
                        let var14 = 0.125f32;
                        let var10 = 5usize;
                        let var9 = 17usize;
                        let mut var16 = d_slice[(var11 * var10 + var12) * var9 + var13];
                        let mut var18 = d_slice[(var11 * var10 + var12 + 1) * var9 + var13];
                        let mut var20 = d_slice[((var11 + 1) * var10 + var12) * var9 + var13];
                        let mut var22 = d_slice[((var11 + 1) * var10 + var12 + 1) * var9 + var13];
                        let var24 = (d_slice[(var11 * var10 + var12) * var9 + var13 + 1] - var16) * var14;
                        let var26 = (d_slice[(var11 * var10 + var12 + 1) * var9 + var13 + 1] - var18) * var14;
                        let var28 = (d_slice[((var11 + 1) * var10 + var12) * var9 + var13 + 1] - var20) * var14;
                        let var30 = (d_slice[((var11 + 1) * var10 + var12 + 1) * var9 + var13 + 1] - var22) * var14;

                        for var32 in 0..8usize {
                            let var33 = 0.25f32;
                            let mut var35 = var16;
                            let mut var37 = var18;
                            let var39 = (var20 - var16) * var33;
                            let var41 = (var22 - var18) * var33;

                            for var43 in 0..4usize {
                                let mut var44 = ((var43 + var11 * 4) << 11) | ((var12 * 4) << 7) | (var13 * 8 + var32);
                                let var45 = 128usize;
                                let var46 = 0.25f32;
                                let mut var48 = var35;
                                let var50 = (var37 - var35) * var46;

                                for var52 in 0..4usize {
                                    let temp_val = t_slice[(var11 * 4 + var43) * 16 + var12 * 4 + var52];
                                    let mut var55 = 0u8;
                                    let y_coord = var13 * 8 + var32;
                                    if y_coord < 64 {
                                        if temp_val < 0.5 && y_coord >= 63 {
                                            var55 = 79; // ice
                                        } else {
                                            var55 = 9; // waterMoving
                                        }
                                    }

                                    if var48 > 0.0 {
                                        var55 = 1; // stone
                                    }

                                    cpu_blocks[var44] = var55;
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

            assert_eq!(
                &gpu_result[chunk_idx][..],
                &cpu_blocks[..],
                "Mismatch in chunk {chunk_idx}"
            );
        }
        println!("GPU interpolation verified bit-exact against CPU!");
    }
}
