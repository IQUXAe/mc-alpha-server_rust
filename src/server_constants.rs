//! Port of C++ `core/ServerConstants.h`.
//!
//! Every value matches the C++ `constexpr` exactly; integer constants
//! are `i32`, single-precision constants are `f32`, the rest are `f64`.

pub const TICKS_PER_SECOND: i32 = 20;
pub const MILLISECONDS_PER_TICK: i32 = 50;

pub const VIEW_DISTANCE_MIN: i32 = 3;
pub const VIEW_DISTANCE_MAX: i32 = 15;
pub const VIEW_DISTANCE_DEFAULT: i32 = 10;

pub const CHUNKS_PER_TICK: i32 = 15;
/// Fresh chunk generations per tick per player (a cold generate costs
/// ~12 ms; without a budget exploring new terrain blows the 50 ms tick).
pub const CHUNK_GEN_PER_TICK: i32 = 2;
pub const CHUNK_GENERATION_EXTRA_RADIUS: i32 = 2;

pub const MAX_PACKET_SIZE: i32 = 65536;
pub const SEND_QUEUE_MAX_BYTES: i32 = 1048576;
pub const READ_TIMEOUT_TICKS: i32 = 1200;
pub const KEEPALIVE_INTERVAL_TICKS: i32 = 20;

pub const CHUNK_DATA_SEND_DELAY_TICKS: i32 = 50;

pub const PLAYER_EYE_HEIGHT: f32 = 1.62;
pub const PLAYER_WIDTH: f32 = 0.6;
pub const PLAYER_HEIGHT: f32 = 1.8;

pub const TRACKING_DISTANCE: f64 = 64.0;
pub const ENTITY_TRACKING_INTERVAL_TICKS: i32 = 3;

pub const GRAVITY: f64 = 0.08;
pub const TERMINAL_VELOCITY: f64 = 78.4;
pub const FRICTION: f64 = 0.91;

pub const CHUNK_SIZE_X: i32 = 16;
pub const CHUNK_SIZE_Y: i32 = 128;
pub const CHUNK_SIZE_Z: i32 = 16;
pub const SPAWN_Y_DEFAULT: i32 = 64;

pub const AUTO_SAVE_INTERVAL_TICKS_DEFAULT: i32 = 6000;

pub const MAX_CHAT_MESSAGE_LENGTH: i32 = 100;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_timing() {
        assert_eq!(TICKS_PER_SECOND, 20);
        assert_eq!(MILLISECONDS_PER_TICK, 50);
        assert_eq!(TICKS_PER_SECOND * MILLISECONDS_PER_TICK, 1000);
    }

    #[test]
    fn view_distance() {
        assert_eq!(VIEW_DISTANCE_MIN, 3);
        assert_eq!(VIEW_DISTANCE_MAX, 15);
        assert_eq!(VIEW_DISTANCE_DEFAULT, 10);
        assert!(VIEW_DISTANCE_MIN <= VIEW_DISTANCE_DEFAULT);
        assert!(VIEW_DISTANCE_DEFAULT <= VIEW_DISTANCE_MAX);
    }

    #[test]
    fn chunk_loading() {
        assert_eq!(CHUNKS_PER_TICK, 15);
        assert_eq!(CHUNK_GEN_PER_TICK, 2);
        assert_eq!(CHUNK_GENERATION_EXTRA_RADIUS, 2);
    }

    #[test]
    fn network_limits() {
        assert_eq!(MAX_PACKET_SIZE, 65536);
        assert_eq!(SEND_QUEUE_MAX_BYTES, 1048576);
        assert_eq!(READ_TIMEOUT_TICKS, 1200);
        assert_eq!(KEEPALIVE_INTERVAL_TICKS, 20);
    }

    #[test]
    fn chunk_data_delay() {
        assert_eq!(CHUNK_DATA_SEND_DELAY_TICKS, 50);
    }

    #[test]
    fn player_dims() {
        assert_eq!(PLAYER_EYE_HEIGHT, 1.62_f32);
        assert_eq!(PLAYER_WIDTH, 0.6_f32);
        assert_eq!(PLAYER_HEIGHT, 1.8_f32);
        assert!(PLAYER_EYE_HEIGHT < PLAYER_HEIGHT);
    }

    #[test]
    fn entity_tracking() {
        assert_eq!(TRACKING_DISTANCE, 64.0);
        assert_eq!(ENTITY_TRACKING_INTERVAL_TICKS, 3);
    }

    #[test]
    fn physics() {
        assert_eq!(GRAVITY, 0.08);
        assert_eq!(TERMINAL_VELOCITY, 78.4);
        assert_eq!(FRICTION, 0.91);
    }

    #[test]
    fn world_dims() {
        assert_eq!(CHUNK_SIZE_X, 16);
        assert_eq!(CHUNK_SIZE_Y, 128);
        assert_eq!(CHUNK_SIZE_Z, 16);
        assert_eq!(SPAWN_Y_DEFAULT, 64);
    }

    #[test]
    fn autosave_and_chat() {
        assert_eq!(AUTO_SAVE_INTERVAL_TICKS_DEFAULT, 6000);
        assert_eq!(MAX_CHAT_MESSAGE_LENGTH, 100);
    }
}
