//! Optional GPU acceleration module for world generation.
//!
//! Supports Vulkan, OpenGL ES (GLES), Metal, DirectX 12 via `wgpu`.
//! Tuned for older graphics cards and integrated GPUs (downlevel profiles,
//! relaxed limits, graceful CPU fallback).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::RwLock;

static GPU_ENABLED: AtomicBool = AtomicBool::new(false);
static CONFIGURED_BACKEND: RwLock<Option<String>> = RwLock::new(None);

/// Sets the configured GPU graphics backend preference (e.g. from server.properties `gpu-backend`).
/// Supported: "auto", "vulkan", "gl", "gles", "opengl", "dx12", "metal".
pub fn set_configured_backend(backend: &str) {
    if let Ok(mut lock) = CONFIGURED_BACKEND.write() {
        let b = backend.trim().to_lowercase();
        if b.is_empty() || b == "auto" || b == "all" {
            *lock = None;
        } else {
            *lock = Some(b);
        }
    }
}

/// Returns the configured GPU graphics backend preference, if any.
pub fn get_configured_backend() -> Option<String> {
    CONFIGURED_BACKEND.read().ok().and_then(|lock| lock.clone())
}

/// Returns whether GPU acceleration is active at runtime.
/// Default is FALSE (CPU is default).
/// Returns true only if explicitly requested AND a compatible GPU adapter is available.
pub fn is_gpu_enabled() -> bool {
    GPU_ENABLED.load(Ordering::Relaxed) && is_gpu_available()
}

/// Enables or disables GPU acceleration at runtime.
pub fn set_gpu_enabled(enabled: bool) {
    GPU_ENABLED.store(enabled, Ordering::SeqCst);
}

/// Returns whether GPU acceleration was requested (via config, env, or command).
pub fn is_gpu_requested() -> bool {
    GPU_ENABLED.load(Ordering::Relaxed)
}

/// Returns a human-readable status string for GPU acceleration.
pub fn gpu_boost_status() -> String {
    let state = if is_gpu_enabled() {
        "ENABLED"
    } else if GPU_ENABLED.load(Ordering::Relaxed) {
        "DISABLED (no compatible GPU available)"
    } else {
        "DISABLED (CPU default)"
    };
    format!("{state} [{}]", gpu_device_info())
}

#[cfg(feature = "gpu")]
mod gpu_impl;

#[cfg(feature = "gpu")]
pub use gpu_impl::*;

#[cfg(not(feature = "gpu"))]
pub fn is_gpu_available() -> bool {
    false
}

#[cfg(not(feature = "gpu"))]
pub fn gpu_device_info() -> String {
    "GPU feature disabled at compile-time".to_string()
}

#[cfg(test)]
mod tests;
