//! Small-entity kernels (mirrors Java `EntityItem`,
//! `EntityFallingSand`, `EntityBoat`, `EntityArrow`).
//!
//! Only closed-form pieces move: push-out side selection, friction damping,
//! falling-sand landing decisions, water-fraction scan math, and yaw/rider
//! geometry. Entity storage, world mutation, and drops live in `world`.

/// Push-out side for an item stuck in a solid block (mirrors
/// `EntityItem::pushOutOfBlocks` scoring). Free-face flags come in
/// W,E,D,U,N,S order with the fractional position; returns the side
/// (0..5) or -1 when fully buried.
pub fn item_push_side(free: [bool; 6], frac: [f64; 3]) -> i8 {
    let [free_w, free_e, free_d, free_u, free_n, free_s] = free;
    let [lx, ly, lz] = frac;
    let mut best_side: i8 = -1;
    let mut best = 9999.0f64;
    if free_w && lx < best {
        best = lx;
        best_side = 0;
    }
    if free_e && 1.0 - lx < best {
        best = 1.0 - lx;
        best_side = 1;
    }
    if free_d && ly < best {
        best = ly;
        best_side = 2;
    }
    if free_u && 1.0 - ly < best {
        best = 1.0 - ly;
        best_side = 3;
    }
    if free_n && lz < best {
        best = lz;
        best_side = 4;
    }
    if free_s && 1.0 - lz < best {
        best_side = 5;
    }
    best_side
}

/// In-place motion for the item friction block (mirrors `EntityItem::tick`
/// damping; gravity and aging stay in C++).
#[derive(Clone, Copy, Debug)]
pub struct ItemMotion {
    pub mx: f64,
    pub my: f64,
    pub mz: f64,
}

pub fn item_damp(on_ground: bool, io: &mut ItemMotion) -> bool {
    // Java: 0.98 air, slipperiness(0.6) * 0.98 on ground.
    let var1: f32 = if on_ground { 0.6f32 * 0.98f32 } else { 0.98f32 };
    io.mx *= var1 as f64;
    io.my *= 0.98;
    io.mz *= var1 as f64;
    if on_ground {
        io.my *= -0.5;
    }
    true
}

/// Falling-sand landing decision (mirrors `EntityFallingSand::tick` tail).
/// Fact gathering (block id, replaceability, registry presence) stays in
/// C++; returns 0 = keep falling, 1 = place block, 2 = drop as item,
/// 3 = die (air id).
pub fn falling_land(
    block_id: i32,
    on_ground: bool,
    by: i32,
    land_id: i32,
    land_replaceable: bool,
    have_block: bool,
    fall_time: i32,
) -> u8 {
    if block_id == 0 {
        return 3;
    }
    if on_ground {
        if (0..128).contains(&by) && (land_id == 0 || land_replaceable) && have_block {
            return 1;
        }
        return 2;
    }
    if fall_time > 100 {
        return 2;
    }
    0
}

/// Water fraction under a box (mirrors `EntityBoat::computeWaterFraction`).
/// Five vertical slices of the box shrunk by 0.125; a slice counts when any
/// cell in it is water. `is_water` answers per-cell.
pub fn water_fraction_scan(
    min_x: f64,
    min_y: f64,
    min_z: f64,
    max_x: f64,
    max_y: f64,
    max_z: f64,
    mut is_water: impl FnMut(i32, i32, i32) -> bool,
) -> f64 {
    let mut fraction = 0.0;
    for slice in 0..5 {
        let slice_min_y = min_y + (max_y - min_y) * (slice as f64) / 5.0 - 0.125;
        let slice_max_y = min_y + (max_y - min_y) * ((slice + 1) as f64) / 5.0 - 0.125;
        let min_by = slice_min_y.floor() as i32;
        let max_by = slice_max_y.floor() as i32;
        let mut wet = false;
        'scan: for x in (min_x.floor() as i32)..=(max_x.floor() as i32) {
            for y in min_by..=max_by {
                for z in (min_z.floor() as i32)..=(max_z.floor() as i32) {
                    if is_water(x, y, z) {
                        wet = true;
                        break 'scan;
                    }
                }
            }
        }
        if wet {
            fraction += 1.0 / 5.0;
        }
    }
    fraction
}

pub fn boat_water_fraction(
    min_x: f64,
    min_y: f64,
    min_z: f64,
    max_x: f64,
    max_y: f64,
    max_z: f64,
    is_water: Option<fn(x: i32, y: i32, z: i32) -> bool>,
) -> f64 {
    let Some(is_water) = is_water else {
        return 0.0;
    };
    water_fraction_scan(min_x, min_y, min_z, max_x, max_y, max_z, is_water)
}

/// Boat yaw steering (mirrors the yaw block in `EntityBoat::tick`).
/// Writes the new yaw; returns false when still. Threshold `0.001`
/// (squared movement) and the ±20 clamp are preserved exactly, including
/// the asymmetric wrap (`>= 180` first, then `< -180`).
pub fn boat_steer(delta_x: f64, delta_z: f64, cur_yaw: f32, out_yaw: &mut f32) -> bool {
    if delta_x * delta_x + delta_z * delta_z <= 0.001 {
        return false;
    }
    let target = (delta_z.atan2(delta_x) * 180.0 / std::f64::consts::PI) as f32 - 90.0;
    let mut delta = target - cur_yaw;
    while delta >= 180.0 {
        delta -= 360.0;
    }
    while delta < -180.0 {
        delta += 360.0;
    }
    delta = delta.clamp(-20.0, 20.0);
    *out_yaw = cur_yaw + delta;
    true
}

/// Rider seat offset (mirrors `EntityBoat::updateRiderPosition` geometry;
/// rider lookup and positioning stay in C++). Plain `std` trig like C++.
pub fn boat_rider_offset(yaw: f32, out_x: &mut f64, out_z: &mut f64) -> bool {
    let radians = yaw as f64 * std::f64::consts::PI / 180.0;
    *out_x = radians.cos() * 0.4;
    *out_z = radians.sin() * 0.4;
    true
}

/// Arrow launch (mirrors `EntityArrow::shoot`): normalize the aim, add
/// per-axis jitter (`(d1 - d2) * 0.0075 * inaccuracy`, x/y/z draw order),
/// then scale by velocity. Returns the launch motion, or `None` for a
/// degenerate aim like C++ (which leaves motion untouched).
pub fn arrow_shoot_run(
    vx: f64,
    vy: f64,
    vz: f64,
    velocity: f32,
    inaccuracy: f32,
    next_f01: &mut dyn FnMut() -> f64,
) -> Option<[f64; 3]> {
    let length = (vx * vx + vy * vy + vz * vz).sqrt();
    if length < 1.0e-7 {
        return None;
    }
    let (mut nx, mut ny, mut nz) = (vx / length, vy / length, vz / length);
    let jitter = 0.0075 * inaccuracy as f64;
    nx += (next_f01() - next_f01()) * jitter;
    ny += (next_f01() - next_f01()) * jitter;
    nz += (next_f01() - next_f01()) * jitter;
    let v = velocity as f64;
    Some([nx * v, ny * v, nz * v])
}

/// Yaw/pitch from velocity (mirrors the arrow orientation init:
/// `atan2(vx, vz)` yaw, `atan2(vy, horizontal)` pitch with `sqrt_float`).
pub fn arrow_face_velocity(
    mx: f64,
    my: f64,
    mz: f64,
    out_yaw: &mut f32,
    out_pitch: &mut f32,
) -> bool {
    let horizontal = crate::math_helper::sqrt_float((mx * mx + mz * mz) as f32);
    *out_yaw = (mx.atan2(mz) * 180.0 / std::f64::consts::PI) as f32;
    *out_pitch = (my.atan2(horizontal as f64) * 180.0 / std::f64::consts::PI) as f32;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_side_picks_nearest_free_face() {
        // Free west face, close to it.
        assert_eq!(
            item_push_side([true, false, false, false, false, false], [0.1, 0.5, 0.5]),
            0
        );
        // Buried: nothing free.
        assert_eq!(
            item_push_side([false, false, false, false, false, false], [0.1, 0.5, 0.5]),
            -1
        );
        // East closer than west.
        assert_eq!(
            item_push_side([true, true, false, false, false, false], [0.9, 0.5, 0.5]),
            1
        );
        // Down wins over far west.
        assert_eq!(
            item_push_side([true, false, true, false, false, false], [0.4, 0.1, 0.5]),
            2
        );
    }

    #[test]
    fn test_item_damp_ground_and_air() {
        let mut m = ItemMotion { mx: 1.0, my: -2.0, mz: 1.0 };
        assert!(item_damp(false, &mut m));
        assert!((m.mx - 0.98).abs() < 1e-6);
        assert!((m.my + 1.96).abs() < 1e-9);
        let mut m = ItemMotion { mx: 1.0, my: -2.0, mz: 1.0 };
        assert!(item_damp(true, &mut m));
        assert!((m.mx - 0.588).abs() < 1e-6);
        assert!((m.my - 0.98).abs() < 1e-6);
    }

    #[test]
    fn test_falling_land_table() {
        assert_eq!(falling_land(0, false, 60, 0, false, true, 0), 3);
        assert_eq!(falling_land(12, false, 60, 0, false, true, 0), 0);
        assert_eq!(falling_land(12, false, 60, 0, false, true, 101), 2);
        assert_eq!(falling_land(12, true, 60, 0, false, true, 5), 1);
        assert_eq!(falling_land(12, true, 60, 3, false, true, 5), 2);
        assert_eq!(falling_land(12, true, 60, 3, true, true, 5), 1);
        assert_eq!(falling_land(12, true, 60, 3, true, false, 5), 2);
        assert_eq!(falling_land(12, true, 200, 0, false, true, 5), 2);
    }

    fn wet_everywhere(_x: i32, _y: i32, _z: i32) -> bool {
        true
    }
    fn dry_everywhere(_x: i32, _y: i32, _z: i32) -> bool {
        false
    }

    #[test]
    fn test_water_fraction_extremes() {
        let full = boat_water_fraction(0.0, 62.0, 0.0, 2.0, 63.0, 2.0, Some(wet_everywhere));
        assert!((full - 1.0).abs() < 1e-9);
        let dry = boat_water_fraction(0.0, 62.0, 0.0, 2.0, 63.0, 2.0, Some(dry_everywhere));
        assert_eq!(dry, 0.0);
        assert_eq!(boat_water_fraction(0.0, 62.0, 0.0, 2.0, 63.0, 2.0, None), 0.0);
    }

    #[test]
    fn test_boat_steer() {
        let mut yaw = 0.0f32;
        // Still: no update.
        assert!(!boat_steer(0.0, 0.0, 10.0, &mut yaw));
        assert_eq!(yaw, 0.0);
        // Eastward motion from yaw 0: target -90, clamped to -20.
        assert!(boat_steer(1.0, 0.0, 0.0, &mut yaw));
        assert_eq!(yaw, -20.0);
    }

    #[test]
    fn test_rider_offset() {
        let (mut ox, mut oz) = (0.0f64, 0.0f64);
        assert!(boat_rider_offset(0.0, &mut ox, &mut oz));
        assert!((ox - 0.4).abs() < 1e-9);
        assert!(oz.abs() < 1e-9);
    }

    #[test]
    fn test_arrow_face_velocity() {
        let (mut yaw, mut pitch) = (0.0f32, 0.0f32);
        assert!(arrow_face_velocity(0.0, 0.0, 2.0, &mut yaw, &mut pitch));
        assert!(yaw.abs() < 1e-6);
        assert!(pitch.abs() < 1e-6);
    }

    #[test]
    fn test_arrow_shoot_scales_and_jitters_in_order() {
        // Fixed draws: x gets (0.6-0.5), y (0.7-0.4), z (0.8-0.3).
        // Velocity/inaccuracy are exactly representable (0.5/8.0) so the
        // expectation is not polluted by f32->f64 widening.
        let mut seq = [0.6, 0.5, 0.7, 0.4, 0.8, 0.3].into_iter();
        let mut next = || seq.next().unwrap();
        let m = arrow_shoot_run(3.0, 0.0, 4.0, 0.5, 8.0, &mut next).unwrap();
        // Aim (0.6, 0, 0.8) plus jitter 0.0075*8=0.06 per axis diff.
        assert!((m[0] - (0.6 + 0.1 * 0.06) * 0.5).abs() < 1e-9);
        assert!((m[1] - (0.0 + 0.3 * 0.06) * 0.5).abs() < 1e-9);
        assert!((m[2] - (0.8 + 0.5 * 0.06) * 0.5).abs() < 1e-9);
        // Degenerate aim leaves motion alone (None).
        let mut never = || panic!("no draws on degenerate aim");
        assert!(arrow_shoot_run(0.0, 0.0, 0.0, 0.6, 12.0, &mut never).is_none());
    }
}
