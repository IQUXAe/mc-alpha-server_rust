//! Math helpers (mirrors Java `MathHelper.java`).
//!
//! Bit-for-bit notes:
//! - `SIN_TABLE` fill matches the vanilla `init()` loop:
//!   `sin(i * PI * 2 / 65536)` computed in `f64`, narrowed to `f32`.
//! - `sin` / `cos` indexing matches vanilla:
//!   `(int)(v * 10430.378f [+ 16384.0f]) & 0xFFFF`.
//!   The `as i32` cast truncates toward zero like the Java cast for
//!   in-range values.
//! - `floor_float` / `floor_double` use truncation plus adjust, not
//!   `.floor()`, to mirror the vanilla quirk exactly.
//! - `abs` uses a manual branch so `-0.0` stays `-0.0`, like vanilla
//!   `v >= 0 ? v : -v` (whereas `f32::abs` maps `-0.0` to `+0.0`).
//! - `sqrt_float` widens to `f64` first, matching C++
//!   `sqrt((double)v)` narrowed back to `f32`.

use core::f64::consts::PI;
use std::sync::OnceLock;

static SIN_TABLE: OnceLock<Vec<f32>> = OnceLock::new();

fn build_sin_table() -> Vec<f32> {
    let mut v = Vec::with_capacity(65536);
    let mut i: usize = 0;
    while i < 65536 {
        v.push(((i as f64) * PI * 2.0 / 65536.0).sin() as f32);
        i += 1;
    }
    v
}

/// Fill the sine lookup table. Idempotent; mirrors C++ `MathHelper::init`.
pub fn init() {
    let _ = SIN_TABLE.get_or_init(build_sin_table);
}

fn table_len() -> usize {
    if let Some(t) = SIN_TABLE.get() {
        t.len()
    } else {
        0
    }
}

/// Number of entries in the table once initialized (65536).
/// Returns 0 before [`init`] has run, mirroring zero-initialized C++ storage.
pub fn table_size() -> usize {
    table_len()
}

fn lookup(idx: usize) -> f32 {
    // Lazy init: the C++ side requires an explicit MathHelper::init() at
    // startup, but Rust callers (e.g. entity steering) must never observe a
    // zeroed table just because nobody called init() on this path yet.
    let t = SIN_TABLE.get_or_init(build_sin_table);
    if idx < t.len() {
        return t[idx];
    }
    0.0
}

/// C++ `MathHelper::sin`.
pub fn sin(v: f32) -> f32 {
    let idx = ((v * 10430.378_f32) as i32 & 0xFFFF) as usize;
    lookup(idx)
}

/// C++ `MathHelper::cos` (table shifted by 16384 entries).
pub fn cos(v: f32) -> f32 {
    let idx = ((v * 10430.378_f32 + 16384.0_f32) as i32 & 0xFFFF) as usize;
    lookup(idx)
}

/// C++ `MathHelper::sqrt_float`: `sqrt` in `f64`, narrowed to `f32`.
pub fn sqrt_float(v: f32) -> f32 {
    f64::from(v).sqrt() as f32
}

/// C++ `MathHelper::sqrt_double`: returns `f32` despite the name.
pub fn sqrt_double(v: f64) -> f32 {
    v.sqrt() as f32
}

/// C++ `MathHelper::floor_float` via truncation plus adjust.
pub fn floor_float(v: f32) -> i32 {
    let i = v as i32;
    if v < i as f32 {
        i.wrapping_sub(1)
    } else {
        i
    }
}

/// C++ `MathHelper::floor_double` via truncation plus adjust.
pub fn floor_double(v: f64) -> i32 {
    let i = v as i32;
    if v < i as f64 {
        i.wrapping_sub(1)
    } else {
        i
    }
}

/// C++ `MathHelper::abs` with `-0.0` preserved.
pub fn abs(v: f32) -> f32 {
    if v >= 0.0 { v } else { -v }
}

/// C++ `MathHelper::abs_max`.
pub fn abs_max(mut a: f64, mut b: f64) -> f64 {
    if a < 0.0 {
        a = -a;
    }
    if b < 0.0 {
        b = -b;
    }
    if a > b { a } else { b }
}

#[cfg(test)]
#[allow(clippy::approx_constant)]
mod tests {
    use super::*;

    fn approx_eq_f32(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn sin_zero() {
        init();
        assert!(approx_eq_f32(sin(0.0), 0.0, 1e-6));
    }

    #[test]
    fn sin_pi_half() {
        init();
        assert!(approx_eq_f32(sin(3.141_592_7_f32 / 2.0), 1.0, 1e-4));
    }

    #[test]
    fn sin_pi() {
        init();
        assert!(approx_eq_f32(sin(3.141_592_7_f32), 0.0, 1e-4));
    }

    #[test]
    fn cos_zero() {
        init();
        assert!(approx_eq_f32(cos(0.0), 1.0, 1e-6));
    }

    #[test]
    fn cos_pi() {
        init();
        assert!(approx_eq_f32(cos(3.141_592_7_f32), -1.0, 1e-4));
    }

    #[test]
    fn cos_pi_half() {
        init();
        assert!(approx_eq_f32(cos(3.141_592_7_f32 / 2.0), 0.0, 1e-4));
    }

    #[test]
    fn floor_float_positive() {
        assert_eq!(floor_float(3.7), 3);
        assert_eq!(floor_float(3.0), 3);
    }

    #[test]
    fn floor_float_negative() {
        assert_eq!(floor_float(-3.7), -4);
        assert_eq!(floor_float(-3.0), -3);
    }

    #[test]
    fn floor_double_positive() {
        assert_eq!(floor_double(3.7), 3);
        assert_eq!(floor_double(3.0), 3);
    }

    #[test]
    fn floor_double_negative() {
        assert_eq!(floor_double(-3.7), -4);
        assert_eq!(floor_double(-3.0), -3);
    }

    #[test]
    fn sqrt_float_cases() {
        assert!(approx_eq_f32(sqrt_float(25.0), 5.0, 1e-6));
        assert!(approx_eq_f32(sqrt_float(2.0), 1.414_213_5, 1e-5));
    }

    #[test]
    fn sqrt_double_cases() {
        let a = sqrt_double(25.0);
        let b = sqrt_double(2.0);
        assert!(((a - 5.0) as f64).abs() < 1e-6);
        assert!(((b - 1.414_213_5) as f64).abs() < 1e-6);
    }

    #[test]
    fn abs_cases() {
        assert_eq!(abs(5.0), 5.0);
        assert_eq!(abs(-5.0), 5.0);
        assert_eq!(abs(0.0), 0.0);
    }

    #[test]
    fn abs_max_cases() {
        assert_eq!(abs_max(3.0, -5.0), 5.0);
        assert_eq!(abs_max(-2.0, 4.0), 4.0);
        assert_eq!(abs_max(0.0, 0.0), 0.0);
    }

    #[test]
    fn sin_cos_identity() {
        init();
        let angle = 1.234_f32;
        let s = sin(angle);
        let c = cos(angle);
        assert!(approx_eq_f32(s * s + c * c, 1.0, 1e-4));
    }

    // Extra edge coverage beyond the C++ suite.

    #[test]
    fn table_has_full_size_after_init() {
        init();
        assert_eq!(table_size(), 65536);
    }

    #[test]
    fn floor_truncation_adjust_edges() {
        // Values just below / above integer boundaries.
        assert_eq!(floor_float(0.9999999), 0);
        assert_eq!(floor_float(-0.9999999), -1);
        assert_eq!(floor_float(0.0), 0);
        assert_eq!(floor_float(-0.0), 0);
        assert_eq!(floor_double(0.9999999), 0);
        assert_eq!(floor_double(-0.9999999), -1);
        assert_eq!(floor_double(-1.0), -1);
        assert_eq!(floor_double(1.0), 1);
    }

    #[test]
    fn abs_preserves_negative_zero_like_cpp() {
        // C++ `v >= 0 ? v : -v` returns -0.0 for -0.0 input.
        let r = abs(-0.0);
        assert_eq!(r.to_bits(), (-0.0_f32).to_bits());
    }

    #[test]
    fn sin_negative_angle_wraps_via_mask() {
        init();
        // sin(-x) == -sin(x) through the masked table lookup.
        let a = sin(-1.234_f32);
        let b = sin(1.234_f32);
        assert!(approx_eq_f32(a, -b, 1e-6));
    }

    #[test]
    fn cos_matches_sin_shifted_by_quarter_turn() {
        init();
        // cos(v) must equal the table entry 16384 slots ahead of sin(v).
        // Quarter turn in table units: PI/2 * 10430.378 ~= 16384.
        let v = 0.7_f32;
        let c = cos(v);
        let s_shifted = sin(v + 3.141_592_7_f32 / 2.0);
        assert!(approx_eq_f32(c, s_shifted, 2e-4));
    }

    #[test]
    fn sqrt_zero_and_one() {
        assert_eq!(sqrt_float(0.0), 0.0);
        assert_eq!(sqrt_float(1.0), 1.0);
        assert_eq!(sqrt_double(0.0), 0.0);
        assert_eq!(sqrt_double(1.0), 1.0);
    }
}
