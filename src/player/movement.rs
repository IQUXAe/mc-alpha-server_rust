#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MovementValidationStatus {
    Ok = 0,
    IllegalStance = 1,
    IllegalPosition = 2,
    MovedTooQuickly = 3,
    MovedWrongly = 4, // Soft reject (> 225.0, requires teleport reset)
}

#[derive(Clone, Copy, Debug)]
pub struct MovementInput {
    pub from_x: f64,
    pub from_y: f64,
    pub from_z: f64,
    pub to_x: f64,
    pub to_y: f64,
    pub to_z: f64,
    pub stance: f64,
    pub on_ground: bool,
    pub is_in_water: bool,
    pub on_ladder: bool,
    pub fall_distance: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MovementResult {
    pub status: u8,
    pub new_fall_distance: f32,
    pub fall_damage: i32,
    pub move_sq: f64,
}

pub const SOFT_MOVEMENT_REJECT_SQ: f64 = 16.0;
pub const HARD_MOVEMENT_REJECT_SQ: f64 = 100.0;
pub const MAX_COORDINATE: f64 = 3.2e7;
/// Vanilla residual tolerance (NetServerHandler: res² > 1/16 → moved wrongly).
/// Used by the session layer after move_body; kept here for shared reference.
pub const VANILLA_WRONGLY_SQ: f64 = 1.0 / 16.0;

/// Validates movement packet inputs and updates fall distance / damage according to Alpha 1.2.6 rules.
pub fn movement_validate(input: &MovementInput) -> MovementResult {
    let inp = input;

    // 0. Non-finite coordinates poison pos (NaN passes every `>` check).
    // Java has no guard here, but a single NaN offsets the whole body to
    // NaN via move_body. Kick early as Illegal position.
    if !inp.to_x.is_finite()
        || !inp.to_y.is_finite()
        || !inp.to_z.is_finite()
        || !inp.stance.is_finite()
        || !inp.from_x.is_finite()
        || !inp.from_y.is_finite()
        || !inp.from_z.is_finite()
    {
        return MovementResult {
            status: MovementValidationStatus::IllegalPosition as u8,
            new_fall_distance: inp.fall_distance,
            fall_damage: 0,
            move_sq: 0.0,
        };
    }

    // 1. Check stance: stance - y must be within [0.1, 1.65]
    let stance_diff = inp.stance - inp.to_y;
    if !(0.1..=1.65).contains(&stance_diff) {
        return MovementResult {
            status: MovementValidationStatus::IllegalStance as u8,
            new_fall_distance: inp.fall_distance,
            fall_damage: 0,
            move_sq: 0.0,
        };
    }

    // 2. Check world boundary. Y included: the old X/Z-only check let a
    // client park at y = 1e300 forever (finite, valid stance), stuck
    // in an unreachable position with saturating tracker encodes.
    if inp.to_x.abs() > MAX_COORDINATE
        || inp.to_y.abs() > MAX_COORDINATE
        || inp.to_z.abs() > MAX_COORDINATE
    {
        return MovementResult {
            status: MovementValidationStatus::IllegalPosition as u8,
            new_fall_distance: inp.fall_distance,
            fall_damage: 0,
            move_sq: 0.0,
        };
    }

    // 3. Movement delta squared
    let dx = inp.to_x - inp.from_x;
    let dy = inp.to_y - inp.from_y;
    let dz = inp.to_z - inp.from_z;
    let move_sq = dx * dx + dy * dy + dz * dz;

    if move_sq > HARD_MOVEMENT_REJECT_SQ {
        return MovementResult {
            status: MovementValidationStatus::MovedTooQuickly as u8,
            new_fall_distance: inp.fall_distance,
            fall_damage: 0,
            move_sq,
        };
    }

    if move_sq > SOFT_MOVEMENT_REJECT_SQ {
        return MovementResult {
            status: MovementValidationStatus::MovedWrongly as u8,
            new_fall_distance: inp.fall_distance,
            fall_damage: 0,
            move_sq,
        };
    }

    // 4. Fall distance and damage calculations.
    // Vanilla Entity.func_84_k: water resets fall unconditionally, even mid-air.
    // The old code only reset on ground, so swimming down accumulated fall
    // and dealt it on the next dry landing.
    let mut fall_distance = inp.fall_distance;
    let mut fall_damage = 0;

    if inp.is_in_water {
        fall_distance = 0.0;
        return MovementResult {
            status: MovementValidationStatus::Ok as u8,
            new_fall_distance: fall_distance,
            fall_damage,
            move_sq,
        };
    }

    // Java `EntityLiving.onUpdate`: standing on a ladder resets the
    // fall distance every tick — sliding down or stepping off
    // mid-height must never accumulate a drop (the old port only reset
    // for mobs, so players took full-height fall damage at ladder
    // bottoms).
    if inp.on_ladder {
        fall_distance = 0.0;
    }

    if inp.on_ground {
        if fall_distance > 0.0 {
            let dmg = (fall_distance - 3.0).ceil() as i32;
            if dmg > 0 {
                fall_damage = dmg;
            }
            fall_distance = 0.0;
        }
    } else if dy < 0.0 {
        fall_distance -= dy as f32;
    }

    MovementResult {
        status: MovementValidationStatus::Ok as u8,
        new_fall_distance: fall_distance,
        fall_damage,
        move_sq,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_movement_on_ground() {
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 0.2,
            to_y: 64.0,
            to_z: 0.0,
            stance: 65.62,
            on_ground: true,
            is_in_water: false,
            on_ladder: false,
            fall_distance: 0.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::Ok as u8);
        assert_eq!(res.fall_damage, 0);
        assert_eq!(res.new_fall_distance, 0.0);
    }

    #[test]
    fn test_illegal_stance() {
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 0.0,
            to_y: 64.0,
            to_z: 0.0,
            stance: 64.0, // difference = 0.0 < 0.1
            on_ground: true,
            is_in_water: false,
            on_ladder: false,
            fall_distance: 0.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::IllegalStance as u8);
    }

    #[test]
    fn test_illegal_position() {
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 4.0e7,
            to_y: 64.0,
            to_z: 0.0,
            stance: 65.62,
            on_ground: true,
            is_in_water: false,
            on_ladder: false,
            fall_distance: 0.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::IllegalPosition as u8);
    }

    #[test]
    fn test_moved_too_quickly() {
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 40.0, // 40^2 = 1600 > 900
            to_y: 64.0,
            to_z: 0.0,
            stance: 65.62,
            on_ground: true,
            is_in_water: false,
            on_ladder: false,
            fall_distance: 0.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::MovedTooQuickly as u8);
    }

    #[test]
    fn test_fall_damage() {
        // Fall 10 blocks in air, one realistic packet at a time (client
        // sends ~20 packets/sec; a single 10-block jump would trip the
        // 4-block soft reject, so step it like the real client).
        let mut fall = 0.0f32;
        let mut y: f64 = 74.0;
        while y > 64.0 {
            let ny = (y - 1.0).max(64.0);
            let input = MovementInput {
                from_x: 0.0,
                from_y: y,
                from_z: 0.0,
                to_x: 0.0,
                to_y: ny,
                to_z: 0.0,
                stance: ny + 1.62,
                on_ground: false,
                is_in_water: false,
                on_ladder: false,
                fall_distance: fall,
            };
            let res = movement_validate(&input);
            assert_eq!(res.status, MovementValidationStatus::Ok as u8);
            assert_eq!(res.fall_damage, 0);
            fall = res.new_fall_distance;
            y = ny;
        }
        assert_eq!(fall, 10.0);

        // Land on ground: damage = ceil(10.0 - 3.0) = 7
        let input_landing = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 0.0,
            to_y: 64.0,
            to_z: 0.0,
            stance: 65.62,
            on_ground: true,
            is_in_water: false,
            on_ladder: false,
            fall_distance: fall,
        };
        let res2 = movement_validate(&input_landing);
        assert_eq!(res2.status, MovementValidationStatus::Ok as u8);
        assert_eq!(res2.fall_damage, 7);
        assert_eq!(res2.new_fall_distance, 0.0);
    }

    #[test]
    fn test_water_landing_negates_fall() {
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 0.0,
            to_y: 64.0,
            to_z: 0.0,
            stance: 65.62,
            on_ground: true,
            is_in_water: true,
            on_ladder: false,
            fall_distance: 20.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::Ok as u8);
        assert_eq!(res.fall_damage, 0);
        assert_eq!(res.new_fall_distance, 0.0);
    }

    #[test]
    fn test_nan_rejected_as_illegal_position() {
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: f64::NAN,
            to_y: 64.0,
            to_z: 0.0,
            stance: 65.62,
            on_ground: false,
            is_in_water: false,
            on_ladder: false,
            fall_distance: 0.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::IllegalPosition as u8);
    }

    #[test]
    fn test_water_resets_fall_midair() {
        // Swimming down must not accumulate fall (vanilla func_84_k resets
        // unconditionally in water; the old code only reset on ground).
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 0.0,
            to_y: 60.0,
            to_z: 0.0,
            stance: 61.62,
            on_ground: false,
            is_in_water: true,
            on_ladder: false,
            fall_distance: 5.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::Ok as u8);
        assert_eq!(res.new_fall_distance, 0.0);
        assert_eq!(res.fall_damage, 0);
    }

    #[test]
    fn test_soft_reject_at_four_blocks() {
        // 5-block jump in one packet (25 sq > 16) → teleport-back, not kick.
        let input = MovementInput {
            from_x: 0.0,
            from_y: 64.0,
            from_z: 0.0,
            to_x: 5.0,
            to_y: 64.0,
            to_z: 0.0,
            stance: 65.62,
            on_ground: false,
            is_in_water: false,
            on_ladder: false,
            fall_distance: 0.0,
        };
        let res = movement_validate(&input);
        assert_eq!(res.status, MovementValidationStatus::MovedWrongly as u8);
    }
}
