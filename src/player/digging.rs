//! Digging state machine (mirrors Java `ItemInWorldManager`).
//!
//! This module owns the progressive-digging state (`cur_damage`,
//! `initial_cooldown`, current target block).
//!
//! Design notes:
//! - Hardness is computed inside via `mining_check_hardness`.
//! - No allocation, no strings, no `unwrap()`: with `panic = "abort"`
//!   any panic would kill the whole server, so all paths are total.
//! - State is a plain `Copy` value stored inline on the session.

use crate::player::mining::mining_check_hardness;

/// Digging state, stored inline on the server side.
///
/// Mirrors `ItemInWorldManager::{curblockDamage,
/// initialDamage, partiallyDestroyedBlockX/Y/Z}` plus a `has_target`
/// flag (vanilla used `(0,0,0)` as implicit "no target", which collides
/// with a real block at the origin).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DigState {
    pub cur_damage: f32,
    pub initial_cooldown: i32,
    pub target_x: i32,
    pub target_y: i32,
    pub target_z: i32,
    pub has_target: bool,
}

/// Everything the digging simulation needs for one decision.
/// `block_id == 0` means air (nothing to dig).
#[derive(Clone, Copy, Debug)]
pub struct DigInput {
    pub block_id: i32,
    pub held_item_id: i32,
    pub in_water: bool,
    pub on_ground: bool,
}

/// Fresh digging state (no target, no damage).
pub fn dig_state_new() -> DigState {
    DigState {
        cur_damage: 0.0,
        initial_cooldown: 0,
        target_x: 0,
        target_y: 0,
        target_z: 0,
        has_target: false,
    }
}

/// Reset digging progress (mirrors `ItemInWorldManager::cancelRemoving`).
pub fn dig_cancel(s: &mut DigState) {
    s.cur_damage = 0.0;
    s.initial_cooldown = 0;
}

/// Start-of-dig predicate (mirrors `ItemInWorldManager::onBlockClicked`).
///
/// Returns `true` when the block breaks instantly (`hardness >= 1.0`,
/// e.g. torches/flowers) and the caller must harvest immediately.
/// Does not mutate `state`: progressive digging is driven by
/// `dig_on_tick`.
pub fn dig_on_click(input: DigInput) -> bool {
    if input.block_id <= 0 {
        return false;
    }
    let hardness = mining_check_hardness(
        input.block_id,
        input.held_item_id,
        input.in_water,
        input.on_ground,
    );
    hardness >= 1.0
}

/// Progressive-digging tick (mirrors `ItemInWorldManager::blockRemoving`).
///
/// Returns `true` when accumulated damage reaches `1.0` and the caller
/// must call `harvestBlock(x, y, z)` now (which also resets the
/// accumulator and installs the 5-tick `initial_cooldown`).
///
/// Exact C++ semantics preserved:
/// - `initial_cooldown > 0` ticks down first, no progress.
/// - A different target resets accumulators and latches the new target.
/// - Air (`block_id == 0`) on the current target is a no-op (matches
///   the early `return` in C++; progress is kept, not reset).
pub fn dig_on_tick(
    s: &mut DigState,
    x: i32,
    y: i32,
    z: i32,
    input: DigInput,
) -> bool {
    if s.initial_cooldown > 0 {
        s.initial_cooldown -= 1;
        return false;
    }

    let same_target = s.has_target && s.target_x == x && s.target_y == y && s.target_z == z;
    if !same_target {
        s.cur_damage = 0.0;
        s.target_x = x;
        s.target_y = y;
        s.target_z = z;
        s.has_target = true;
        return false;
    }

    if input.block_id <= 0 {
        return false;
    }

    let hardness_tick = mining_check_hardness(
        input.block_id,
        input.held_item_id,
        input.in_water,
        input.on_ground,
    );
    // Unbreakable (bedrock, hardness < 0 -> 0.0 tick): never accumulates.
    if hardness_tick <= 0.0 {
        return false;
    }
    s.cur_damage += hardness_tick;
    if s.cur_damage >= 1.0 - 1e-4 {
        s.cur_damage = 0.0;
        s.initial_cooldown = 5;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(block_id: i32, held: i32) -> DigInput {
        DigInput {
            block_id,
            held_item_id: held,
            in_water: false,
            on_ground: true,
        }
    }

    #[test]
    fn test_click_instant_break_torch() {
        // Torch (50): hardness 0.0 -> check returns 1.0 -> instant.
        assert!(dig_on_click(input(50, 0)));
    }

    #[test]
    fn test_click_stone_not_instant() {
        assert!(!dig_on_click(input(1, 278)));
    }

    #[test]
    fn test_click_air_never_breaks() {
        assert!(!dig_on_click(input(0, 278)));
    }

    #[test]
    fn test_tick_stone_diamond_pick_takes_six_ticks() {
        // Stone (1) + diamond pick (278): 0.1777/tick -> 6 ticks.
        // First tick latches the target, then 6 progress ticks.
        let mut s = dig_state_new();
        let inp = input(1, 278);
        assert!(!dig_on_tick(&mut s, 10, 64, 10, inp)); // latch
        for _ in 0..5 {
            assert!(!dig_on_tick(&mut s, 10, 64, 10, inp));
        }
        assert!(dig_on_tick(&mut s, 10, 64, 10, inp)); // broke
        assert_eq!(s.initial_cooldown, 5);
        assert_eq!(s.cur_damage, 0.0);
    }

    #[test]
    fn test_tick_cooldown_blocks_progress() {
        let mut s = dig_state_new();
        let inp = input(50, 0); // instant hardness, but cooldown gates tick path
        s.initial_cooldown = 2;
        s.has_target = true;
        s.target_x = 1;
        s.target_y = 2;
        s.target_z = 3;
        assert!(!dig_on_tick(&mut s, 1, 2, 3, inp));
        assert_eq!(s.initial_cooldown, 1);
        assert!(!dig_on_tick(&mut s, 1, 2, 3, inp));
        assert_eq!(s.initial_cooldown, 0);
    }

    #[test]
    fn test_tick_target_switch_resets() {
        let mut s = dig_state_new();
        let inp = input(1, 278);
        assert!(!dig_on_tick(&mut s, 0, 64, 0, inp));
        assert!(!dig_on_tick(&mut s, 0, 64, 0, inp));
        assert!(s.cur_damage > 0.0);
        // Different block: reset, no progress, new target latched.
        assert!(!dig_on_tick(&mut s, 5, 64, 5, inp));
        assert_eq!(s.cur_damage, 0.0);
        assert_eq!((s.target_x, s.target_y, s.target_z), (5, 64, 5));
    }

    #[test]
    fn test_tick_bedrock_never_breaks() {
        let mut s = dig_state_new();
        let inp = input(7, 278);
        assert!(!dig_on_tick(&mut s, 0, 1, 0, inp)); // latch
        for _ in 0..50 {
            assert!(!dig_on_tick(&mut s, 0, 1, 0, inp));
        }
    }

    #[test]
    fn test_cancel_resets() {
        let mut s = dig_state_new();
        s.cur_damage = 0.5;
        s.initial_cooldown = 3;
        dig_cancel(&mut s);
        assert_eq!(s.cur_damage, 0.0);
        assert_eq!(s.initial_cooldown, 0);
    }

    #[test]
    fn test_fresh_state_cancel_resets() {
        // Null states are unrepresentable now (safe &mut only); a fresh
        // state cancel is the closest meaningful case.
        let mut s = dig_state_new();
        s.cur_damage = 0.5;
        dig_cancel(&mut s);
        assert_eq!(s.cur_damage, 0.0);
        assert_eq!(s.initial_cooldown, 0);
    }
}
