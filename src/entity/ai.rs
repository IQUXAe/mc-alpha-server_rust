//! Creature steering math (mirrors Java `EntityCreature`/`EntityLiving.func_147_b`).
//!
//! Closed-form angle math plus the pure AI decision kernels (wander pick,
//! chase-speed factor). RNG draws stay with the caller via `JavaRandom`.
//!
//! Angle convention notes: yaw is degrees, `atan2(dz, dx)` in `f64`
//! narrowed to `f32`, and the strafe helpers use the quantized
//! `math_helper` sin/cos tables.

use crate::math_helper::{cos, sin};

/// Turn clamp (mirrors `EntityCreature::clampAngle`).
pub fn ai_clamp_angle(current: f32, target: f32, max_delta: f32) -> f32 {
    let mut delta = target - current;
    while delta < -180.0 {
        delta += 360.0;
    }
    while delta >= 180.0 {
        delta -= 360.0;
    }
    if delta > max_delta {
        delta = max_delta;
    }
    if delta < -max_delta {
        delta = -max_delta;
    }
    current + delta
}

/// Facing computation (mirrors `EntityCreature::faceEntity` after the
/// caller resolves `dy`: living eye height above, else bounding-box center).
/// Returns `(new_yaw, new_pitch)`; the pitch already includes the leading
/// negation.
pub fn face_run(dx: f64, dz: f64, dy: f64, cur_yaw: f32, cur_pitch: f32, max_turn: f32) -> (f32, f32) {
    let dist = crate::math_helper::sqrt_double(dx * dx + dz * dz);
    let yaw = (dz.atan2(dx) * 180.0 / std::f64::consts::PI) as f32 - 90.0;
    let pitch = (dy.atan2(dist as f64) * 180.0 / std::f64::consts::PI) as f32;
    (clamp_inner(cur_yaw, yaw, max_turn), -clamp_inner(cur_pitch, pitch, max_turn))
}

fn clamp_inner(current: f32, target: f32, max_delta: f32) -> f32 {
    let mut delta = target - current;
    while delta < -180.0 {
        delta += 360.0;
    }
    while delta >= 180.0 {
        delta -= 360.0;
    }
    if delta > max_delta {
        delta = max_delta;
    }
    if delta < -max_delta {
        delta = -max_delta;
    }
    current + delta
}

/// Wander weights (mirrors `getBlockPathWeight`): animals prefer grass
/// (10.0), else light brightness (0..1 float) minus a half; mobs score
/// 0.5 minus brightness, so the darkest candidate wins.
pub fn ai_animal_path_weight(below_grass: bool, brightness: f32) -> f32 {
    if below_grass {
        10.0
    } else {
        brightness - 0.5
    }
}

pub fn ai_mob_path_weight(brightness: f32) -> f32 {
    0.5 - brightness
}

/// Path-point steering intent (mirrors the steering block in
/// `EntityCreature::followPath`). `forward_in` is the pre-steering forward
/// value (C++ passes its just-zeroed `moveForward_`; the formula is kept
/// general). The caller still applies `moveSpeed` afterwards, exactly like
/// C++.
#[derive(Clone, Copy, Debug)]
pub struct SteerOut {
    pub new_yaw: f32,
    pub strafe: f32,
    pub forward: f32,
    pub jump: bool,
}

/// Steering input bundled to keep the arity clippy-clean.
#[derive(Clone, Copy, Debug)]
pub struct SteerIn {
    pub delta: [f64; 3],
    pub cur_yaw: f32,
    pub attacking: bool,
    pub has_target: bool,
    pub target: [f64; 2],
    pub forward_in: f32,
}

/// Pure steering core
/// (mirrors the steering block in `EntityCreature::followPath`).
pub fn steer_run(v: SteerIn) -> SteerOut {
    let (dx, dy, dz) = (v.delta[0], v.delta[1], v.delta[2]);
    let (tgt_dx, tgt_dz) = (v.target[0], v.target[1]);
    let target_yaw = (dz.atan2(dx) * 180.0 / std::f64::consts::PI) as f32 - 90.0;
    let mut yaw_delta = target_yaw - v.cur_yaw;
    while yaw_delta < -180.0 {
        yaw_delta += 360.0;
    }
    while yaw_delta >= 180.0 {
        yaw_delta -= 360.0;
    }
    yaw_delta = yaw_delta.clamp(-30.0, 30.0);
    let new_yaw = v.cur_yaw + yaw_delta;

    let (mut strafe, mut forward) = (0.0f32, v.forward_in);
    if v.attacking && v.has_target {
        let backup_yaw = new_yaw;
        let face_yaw = (tgt_dz.atan2(tgt_dx) * 180.0 / std::f64::consts::PI) as f32 - 90.0;
        let strafe_angle = (backup_yaw - face_yaw + 90.0) * (std::f32::consts::PI / 180.0);
        strafe = -sin(strafe_angle) * v.forward_in;
        forward = cos(strafe_angle) * v.forward_in;
    }

    SteerOut { new_yaw, strafe, forward, jump: dy > 0.0 }
}

/// Wander destination pick (mirrors `EntityCreature::pickWanderDestination`):
/// best of 10 random points in the 13x7x13 area around `base`, strict `>`
/// comparison from `bestWeight = -99999.0`, so ties keep the first
/// candidate. Draw order per candidate is x-size, y-size, x-size like C++.
/// Returns `None` only when zero iterations run (never with the C++ count).
pub fn wander_pick(
    base: [i32; 3],
    next: &mut dyn FnMut(i32) -> i32,
    weight: &mut dyn FnMut(i32, i32, i32) -> f32,
) -> Option<[i32; 3]> {
    let mut best: Option<[i32; 3]> = None;
    let mut best_weight = -99999.0f32;
    for _ in 0..10 {
        let cx = base[0] + next(13) - 6;
        let cy = base[1] + next(7) - 3;
        let cz = base[2] + next(13) - 6;
        let w = weight(cx, cy, cz);
        if w > best_weight {
            best_weight = w;
            best = Some([cx, cy, cz]);
        }
    }
    best
}

/// Chase speed: vanilla `EntityCreature` uses `moveSpeed` directly
/// (`field_9130_bp = field_9126_bt`); no 1.2x/0.85x factor exists.
/// Kept as a function so call sites stay explicit.
pub fn chase_speed(base: f32, _dist: f32, _reach: f32) -> f32 {
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clamp_angle() {
        assert_eq!(ai_clamp_angle(0.0, 10.0, 30.0), 10.0);
        assert_eq!(ai_clamp_angle(0.0, 100.0, 30.0), 30.0);
        assert_eq!(ai_clamp_angle(0.0, -100.0, 30.0), -30.0);
        // Wrap: 170 -> -170 turns +20, clamped to 30.
        assert_eq!(ai_clamp_angle(170.0, -170.0, 30.0), 190.0);
        // Exact 180 wraps to -180 first.
        assert_eq!(ai_clamp_angle(0.0, 180.0, 30.0), -30.0);
    }

    #[test]
    fn test_face_angles_east() {
        // Target due east (+x): yaw = atan2(0,1)*180/pi - 90 = -90.
        let (yaw, pitch) = face_run(5.0, 0.0, 0.0, 0.0, 0.0, 30.0);
        assert_eq!(yaw, -30.0); // clamped turn toward -90
        assert_eq!(pitch, -0.0);
    }

    #[test]
    fn test_steer_straight_ahead() {
        // Point due east of a creature facing east (yaw -90): no turn.
        let out = steer_run(SteerIn {
            delta: [5.0, 0.0, 0.0],
            cur_yaw: -90.0,
            attacking: false,
            has_target: false,
            target: [0.0, 0.0],
            forward_in: 0.0,
        });
        assert!((out.new_yaw + 90.0).abs() < 1e-4);
        assert_eq!(out.strafe, 0.0);
        assert!(!out.jump);
    }

    #[test]
    fn test_steer_turn_rate_limited() {
        // Point due west while facing east: 180 normalised to -180,
        // clamped to a -30 turn.
        let out = steer_run(SteerIn {
            delta: [-5.0, 1.0, 0.0],
            cur_yaw: -90.0,
            attacking: false,
            has_target: false,
            target: [0.0, 0.0],
            forward_in: 0.0,
        });
        assert!((out.new_yaw + 120.0).abs() < 1e-4);
        assert!(out.jump);
    }

        #[test]
    fn test_steer_strafe_uses_input_forward() {
        // Attacking: strafe formula applied to the passed forward value.
        let out = steer_run(SteerIn {
            delta: [5.0, 0.0, 0.0],
            cur_yaw: -90.0,
            attacking: true,
            has_target: true,
            target: [5.0, 0.0],
            forward_in: 0.7,
        });
        // Target straight ahead: strafe angle 90deg -> sin=1, cos~0.
        assert!((out.strafe + 0.7).abs() < 1e-4);
        assert!(out.forward.abs() < 1e-4);
    }

    #[test]
    fn test_face_run() {
        assert_eq!(face_run(5.0, 0.0, 0.0, 0.0, 0.0, 30.0), (-30.0, -0.0));
    }

    #[test]
    fn test_steer_run() {
        // Point east-north-east: yaw turns partway toward it, jump from dy > 0.
        let a = steer_run(SteerIn {
            delta: [5.0, 1.0, 1.0],
            cur_yaw: -90.0,
            attacking: true,
            has_target: true,
            target: [5.0, 0.0],
            forward_in: 0.7,
        });
        assert!((a.new_yaw + 78.69006).abs() < 1e-4);
        assert!(a.jump);
    }

    #[test]
    fn test_wander_pick_prefers_best_weight() {
        // Candidate k sits at base + (k, 0, k); weight grows with x.
        // Draws arrive bound-first: 13, 7, 13 per candidate.
        let mut seq = (0..30).map(|i| match i % 3 {
            1 => 3,
            _ => 6 + (i / 3),
        }).collect::<Vec<_>>().into_iter();
        let mut next = |bound: i32| {
            let v = seq.next().unwrap();
            assert!(bound == 13 || bound == 7);
            v
        };
        let mut weight = |x: i32, _: i32, _: i32| x as f32;
        assert_eq!(
            wander_pick([100, 64, 100], &mut next, &mut weight),
            Some([109, 64, 109])
        );
    }

    #[test]
    fn test_wander_pick_ties_keep_first() {
        let mut next = |bound: i32| bound / 2; // 13->6, 7->3: offset 0 every time
        let mut weight = |_: i32, _: i32, _: i32| 0.0; // mob-style flat score
        assert_eq!(
            wander_pick([10, 64, 10], &mut next, &mut weight),
            Some([10, 64, 10])
        );
    }

    #[test]
    fn test_chase_speed_is_base() {
        // Vanilla uses moveSpeed directly, no distance factor.
        assert_eq!(chase_speed(0.5, 5.0, 2.5), 0.5);
        assert_eq!(chase_speed(0.5, 3.0, 2.5), 0.5);
    }
}
