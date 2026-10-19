//! Entity physics kernel (mirrors Java `Entity`).
//!
//! The world-dependent half lives in `world`: gathering colliding boxes,
//! the `onFall` damage event, and velocity application. Rust owns the pure
//! resolution math, so collision order (Y, then X, then Z, with box offsets
//! between passes) has a single tested source of truth.
//!
//! AABB ops reuse the `aabb` module.

use crate::aabb::AxisAlignedBB;
use crate::math_helper::{abs_max, sqrt_double};

/// Plain box copy of `AxisAlignedBB` fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min_x: f64,
    pub min_y: f64,
    pub min_z: f64,
    pub max_x: f64,
    pub max_y: f64,
    pub max_z: f64,
}

impl From<AxisAlignedBB> for Aabb {
    fn from(b: AxisAlignedBB) -> Self {
        Self {
            min_x: b.min_x,
            min_y: b.min_y,
            min_z: b.min_z,
            max_x: b.max_x,
            max_y: b.max_y,
            max_z: b.max_z,
        }
    }
}

impl From<Aabb> for AxisAlignedBB {
    fn from(b: Aabb) -> Self {
        Self::get_bounding_box(b.min_x, b.min_y, b.min_z, b.max_x, b.max_y, b.max_z)
    }
}

// NOTE: the old Y-X-Z `entity_resolve_move` helper was removed —
// `World::move_body` owns the single collision+step implementation now
// (a second copy had already drifted: no step height here). The axis
// order is covered by move_body's behavior.

/// Fall-state step (mirrors `Entity::updateFallState`). Returns the new
/// `fallDistance` plus the landing event distance, if any.
pub fn entity_fall_step(on_ground: bool, dy: f64, fall_distance: f32) -> (f32, Option<f32>) {
    if on_ground {
        if fall_distance > 0.0 {
            (0.0, Some(fall_distance))
        } else {
            (fall_distance, None)
        }
    } else if dy < 0.0 {
        // Promotes to double before subtracting, then narrows back.
        ((fall_distance as f64 - dy) as f32, None)
    } else {
        (fall_distance, None)
    }
}

/// Push impulse between two entities (mirrors `Entity::applyEntityCollision`
/// after the identity/pushable guards). Returns both velocity deltas for
/// the caller to apply; `None` when the pair is too close to push.
#[derive(Clone, Copy, Debug)]
pub struct PushOut {
    pub dvx1: f64,
    pub dvz1: f64,
    pub dvx2: f64,
    pub dvz2: f64,
}

pub fn entity_push(
    x1: f64,
    z1: f64,
    x2: f64,
    z2: f64,
    pushable1: bool,
    pushable2: bool,
) -> Option<PushOut> {
    if !pushable1 || !pushable2 {
        return None;
    }
    let dx = x2 - x1;
    let dz = z2 - z1;
    let max_abs = abs_max(dx, dz);
    if max_abs < 0.01 {
        return None;
    }
    let norm = sqrt_double(max_abs) as f64;
    let mut scale = 1.0 / max_abs;
    if scale > 1.0 {
        scale = 1.0;
    }
    let factor = (scale * 0.05) / norm;
    let (ix, iz) = (dx * factor, dz * factor);
    Some(PushOut { dvx1: -ix, dvz1: -iz, dvx2: ix, dvz2: iz })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_box(x0: f64, y0: f64, z0: f64, x1: f64, y1: f64, z1: f64) -> Aabb {
        Aabb { min_x: x0, min_y: y0, min_z: z0, max_x: x1, max_y: y1, max_z: z1 }
    }

    #[test]
    fn test_box_roundtrip() {
        let b = make_box(-0.3, 2.0, -0.3, 0.3, 3.8, 0.3);
        let back: AxisAlignedBB = b.into();
        let fwd = Aabb::from(back);
        assert_eq!(b, fwd);
    }

    #[test]
    fn test_fall_step_landing_fires_event() {
        assert_eq!(entity_fall_step(true, -3.0, 3.5), (0.0, Some(3.5)));
    }

    #[test]
    fn test_fall_step_accumulates_in_air() {
        let (next, ev) = entity_fall_step(false, -2.0, 1.5);
        assert_eq!(ev, None);
        assert!((next - 3.5).abs() < 1e-6);
    }

    #[test]
    fn test_fall_step_rising_keeps_distance() {
        assert_eq!(entity_fall_step(false, 1.0, 2.0), (2.0, None));
    }

    #[test]
    fn test_push_impulse() {
        // e1 at origin, e2 at (3,4): norm 2, scale 1/4, factor 0.05.
        let out = entity_push(0.0, 0.0, 3.0, 4.0, true, true).unwrap();
        assert!((out.dvx2 - 0.01875).abs() < 1e-9);
        assert!((out.dvz2 - 0.025).abs() < 1e-9);
        assert!((out.dvx1 + 0.01875).abs() < 1e-9);
        assert!((out.dvz1 + 0.025).abs() < 1e-9);
    }

    #[test]
    fn test_push_too_close_or_locked() {
        assert!(entity_push(0.0, 0.0, 0.005, 0.0, true, true).is_none());
        assert!(entity_push(0.0, 0.0, 3.0, 4.0, false, true).is_none());
    }
}
