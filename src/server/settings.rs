//! Runtime settings from `server.properties` plus ops/ban list files.
//! Split out of `server.rs`; behavior unchanged.

use std::collections::BTreeSet;
use crate::server_admin::{admin_parse_list, admin_render_list};
use crate::server_config::ServerConfig;
use crate::server_constants::{
    AUTO_SAVE_INTERVAL_TICKS_DEFAULT, VIEW_DISTANCE_DEFAULT, VIEW_DISTANCE_MAX, VIEW_DISTANCE_MIN,
};

pub struct Settings {
    pub server_ip: String,
    pub port: i32,
    pub online_mode: bool,
    pub spawn_animals: bool,
    pub spawn_monsters: bool,
    pub pvp: bool,
    pub difficulty: i32,
    pub view_distance: i32,
    pub auto_save_interval: i32,
    pub spawn_protection: i32,
    pub max_players: i32,
    /// Hard cap on total TCP connections (authenticated or not). Vanilla
    /// has no accept gate at all — `max-players` only fires at join — so
    /// distinct IPs can pile unauthenticated sockets during the 30s login
    /// window and exhaust fds/memory. Clamped to at least `max_players`
    /// so the server can always fill up.
    pub max_connections: i32,
    pub level_name: String,
    pub seed: i64,
    pub dimension: i8,
    /// In vanilla Alpha 1.2.6, the server exclusively runs either Nether
    /// (`hellworld=true`, dimension -1) or Overworld (`hellworld=false`, dimension 0).
    pub hellworld: bool,
    pub hell_enabled: bool,
    pub auth_server_url: String,
    pub gpu_acceleration: bool,
    pub gpu_backend: String,
    pub proxy_protocol: bool,
    pub trusted_proxies: Vec<String>,
    /// Fresh chunk generations per tick per player (mirrors
    /// `CHUNK_GEN_PER_TICK`; a cold generate costs ~12 ms on x86,
    /// 30-50 ms on Cortex-A53, so exploring new terrain blows the
    /// 50 ms tick without a budget). Default 2, S912 preset 1.
    pub chunk_gen_per_tick: i32,
    /// Full-chunk skylight regenerations per world tick (mirrors
    /// `MAX_LIGHT_REGENS_PER_TICK`). Default 16, S912 preset 4.
    /// Timing-only: over-budget chunks refresh next tick.
    pub light_regens_per_tick: usize,
    /// Chunks encoded+stored per tick during autosave/unload flush.
    /// Default 8, S912 preset 4 (eMMC-friendly).
    pub autosave_batch: usize,
    /// Low-power preset (Amlogic S912 and similar weak boxes). Only
    /// changes the *defaults* of the three budgets above when their
    /// keys are absent; explicit keys always win, so other machines
    /// are unaffected unless they opt in via `low-power=true` or
    /// `ALPHA_S912=1` / `ALPHA_LOW_POWER=1`.
    pub low_power: bool,
}

/// Leading-integer seed parse (mirrors `std::stoll`: optional `-`,
/// digits, trailing garbage ignored, emptyjunk is an error).
fn parse_seed_int(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let mut i = 0;
    let neg = if b.first() == Some(&b'-') {
        i = 1;
        true
    } else {
        false
    };
    if i >= b.len() || !b[i].is_ascii_digit() {
        return None;
    }
    let mut acc: i64 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        let d = (b[i] - b'0') as i64;
        if neg {
            acc = acc.checked_mul(10)?.checked_sub(d)?;
        } else {
            acc = acc.checked_mul(10)?.checked_add(d)?;
        }
        i += 1;
    }
    Some(acc)
}

fn java_string_hash(s: &str) -> i64 {
    let mut h: i32 = 0;
    for byte in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(byte as i8 as i32);
    }
    h as i64
}

pub fn load_settings(cfg: &mut ServerConfig) -> Settings {
    let difficulty = cfg.get_int("difficulty", 2).clamp(0, 3);
    let view_distance = cfg.get_int("view-distance", VIEW_DISTANCE_DEFAULT)
        .clamp(VIEW_DISTANCE_MIN, VIEW_DISTANCE_MAX);
    let spawn_protection = if let Some(v) = cfg.get("spawn-protection").and_then(|s| s.trim().parse::<i32>().ok()) {
        v.max(0)
    } else {
        cfg.get_int("spawn-protection-radius", 16).max(0)
    };
    let level_seed = cfg.get_string("level-seed", "");
    let seed = if level_seed.is_empty() {
        0
    } else if let Some(n) = parse_seed_int(&level_seed) {
        n
    } else {
        java_string_hash(&level_seed)
    };
    let is_hell = cfg.get_bool("hellworld", false)
        || cfg.get("dimension").map(|s| s.trim() == "-1").unwrap_or(false)
        || cfg.get_bool("hell-enabled", false);
    let low_power = cfg.get_bool("low-power", false)
        || std::env::var("ALPHA_S912")
            .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        || std::env::var("ALPHA_LOW_POWER")
            .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    // Preset only supplies defaults for missing keys; stored values win.
    let chunk_gen_default = if low_power {
        1
    } else {
        crate::server_constants::CHUNK_GEN_PER_TICK
    };
    let light_regens_default = if low_power {
        4
    } else {
        crate::world::MAX_LIGHT_REGENS_PER_TICK as i32
    };
    let autosave_batch_default = if low_power { 4 } else { 8 };
    if low_power {
        crate::server_log::info(
            "Low-power mode enabled (S912 preset): conservative tick budgets as defaults",
        );
    }
    let gpu_backend = cfg.get_string("gpu-backend", "auto");
    crate::gpu::set_configured_backend(&gpu_backend);

    let gpu_acceleration = cfg.get_bool("gpu-acceleration", false)
        || cfg.get("gpu-boost").map(|s| s == "true").unwrap_or(false)
        || std::env::var("ALPHA_GPU_BOOST")
            .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
        || std::env::var("ALPHA_GPU")
            .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
    if gpu_acceleration {
        if crate::gpu::is_gpu_available() {
            crate::gpu::set_gpu_enabled(true);
            crate::server_log::info(&format!(
                "GPU acceleration enabled: {}",
                crate::gpu::gpu_device_info()
            ));
        } else {
            crate::server_log::warning(
                "GPU acceleration requested in config/env, but no compatible GPU adapter is available; using CPU default.",
            );
        }
    }

    Settings {
        server_ip: cfg.get_string("server-ip", ""),
        port: cfg.get_int("server-port", 25565),
        online_mode: cfg.get_bool("online-mode", true),
        spawn_animals: cfg.get_bool("spawn-animals", true),
        spawn_monsters: cfg.get_bool("spawn-monsters", true),
        pvp: cfg.get_bool("pvp", true),
        difficulty,
        view_distance,
        auto_save_interval: cfg.get_int("auto-save-interval", AUTO_SAVE_INTERVAL_TICKS_DEFAULT),
        spawn_protection,
        max_players: cfg.get_int("max-players", 20),
        max_connections: cfg
            .get_int("max-connections", 256)
            .max(cfg.get_int("max-players", 20))
            .max(1),
        level_name: cfg.get_string("level-name", "world"),
        seed,
        dimension: if is_hell { -1 } else { 0 },
        hellworld: is_hell,
        hell_enabled: is_hell,
        auth_server_url: cfg.get_string(
            "auth-server-url",
            crate::session::login::DEFAULT_AUTH_URL,
        ),
        gpu_acceleration,
        gpu_backend,
        proxy_protocol: cfg.get_bool("proxy-protocol", false)
            || cfg.get_bool("haproxy", false)
            || cfg.get_bool("use-proxy-protocol", false)
            || std::env::var("ALPHA_PROXY_PROTOCOL")
                .map(|s| s == "1" || s.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        trusted_proxies: cfg
            .get_string("trusted-proxies", "127.0.0.1,::1")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        chunk_gen_per_tick: cfg
            .get_int("chunk-gen-per-tick", chunk_gen_default)
            .clamp(1, 8),
        light_regens_per_tick: cfg
            .get_int("light-regens-per-tick", light_regens_default)
            .clamp(1, 64) as usize,
        autosave_batch: cfg
            .get_int("autosave-batch", autosave_batch_default)
            .clamp(1, 64) as usize,
        low_power,
    }
}

pub(crate) fn read_list(path: &str) -> BTreeSet<String> {
    match std::fs::read_to_string(path) {
        Ok(body) => admin_parse_list(&body),
        Err(_) => BTreeSet::new(),
    }
}

pub(crate) fn write_list(path: &str, list: &BTreeSet<String>) {
    // Admin-list writes must not fail silently (lost ops/bans on restart).
    if let Err(e) = std::fs::write(path, admin_render_list(list)) {
        crate::server_log::warning(&format!("cannot write {path}: {e}"));
    }
}
