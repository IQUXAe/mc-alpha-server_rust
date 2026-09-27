//! Position/look validation on `PlaySession` (mirrors processMovement).
//! Split out of `session.rs`; behavior unchanged.

use crate::player::movement::{MovementInput, movement_validate};
use crate::session::play::PlaySession;
use crate::session::{SessionCtx, SessionOutcome};

impl PlaySession {
    // ---- movement (mirrors processMovement) ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn movement(
        &mut self,
        ctx: &mut SessionCtx,
        x: f64,
        y: f64,
        stance: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        mut moving: bool,
        rotating: bool,
        on_ground: bool,
    ) -> Option<SessionOutcome> {
        if (moving && (!x.is_finite() || !y.is_finite() || !stance.is_finite() || !z.is_finite()))
            || (rotating && (!yaw.is_finite() || !pitch.is_finite()))
        {
            return self.kick("Illegal position");
        }
        let me = self.player;
        let (pos, cur_yaw, cur_pitch, riding) = match ctx.world.entities.get(me) {
            Some(e) => (e.body().pos, e.body().yaw, e.body().pitch, e.body().riding),
            None => return None,
        };
        if !self.has_moved {
            self.last = pos;
            self.has_moved = true;
        }
        // Teleport acknowledgement (mirrors `field_9006_j`): while the
        // client has not echoed the teleported spot, its packets are
        // stale pre-teleport traffic (e.g. the death position right after
        // respawn) and are held, never kicked or applied.
        if let Some(t) = self.teleport_wait {
            if x == t[0] && z == t[2] && (y - t[1]) * (y - t[1]) < 0.01 {
                self.teleport_wait = None;
            } else {
                return None;
            }
        }
        let final_yaw = if rotating { yaw } else { cur_yaw };
        let final_pitch = if rotating { pitch } else { cur_pitch };
        // Riding branch: look + drift only.
        if riding >= 0 {
            if let Some(e) = ctx.world.entities.get_mut(me) {
                let b = e.body_mut();
                b.yaw = final_yaw;
                b.pitch = final_pitch;
                b.on_ground = on_ground;
                b.motion = [0.0; 3];
                if moving && y == -999.0 && stance == -999.0 {
                    b.motion[0] = x.clamp(-1.0, 1.0);
                    b.motion[2] = z.clamp(-1.0, 1.0);
                }
            }
            ctx.world.entities.update_rider_position(riding);
            if let Some(e) = ctx.world.entities.get(me) {
                self.last = e.body().pos;
            }
            return None;
        }
        if moving && y == -999.0 && stance == -999.0 {
            moving = false;
        }
        let [bx, by, bz] = self.last;
        // Reset to the last accepted state, like C++.
        if let Some(e) = ctx.world.entities.get_mut(me) {
            let b = e.body_mut();
            b.set_position(bx, by, bz);
            b.yaw = final_yaw;
            b.pitch = final_pitch;
            b.motion = [0.0; 3];
        }
        if !moving {
            if let Some(e) = ctx.world.entities.get_mut(me) {
                let b = e.body_mut();
                b.yaw = final_yaw;
                b.pitch = final_pitch;
                b.on_ground = on_ground;
            }
            // Fall bookkeeping still runs on stationary packets (vanilla
            // runs Flying/Look through the same fall path): zero delta,
            // client onGround, carried fall distance. Without this a
            // landing reported without position change silently dropped
            // the accumulated fall instead of converting it to damage.
            let in_water = Self::in_water(ctx.world, me);
            let fall =
                ctx.world.entities.get(me).map(|e| e.body().fall_distance).unwrap_or(0.0);
            let input = MovementInput {
                from_x: bx,
                from_y: by,
                from_z: bz,
                to_x: bx,
                to_y: by,
                to_z: bz,
                stance: by + 1.62,
                on_ground,
                is_in_water: in_water,
                fall_distance: fall,
            };
            let res = movement_validate(&input);
            if res.fall_damage > 0 {
                ctx.world.attack_living(me, res.fall_damage, None);
            }
            if let Some(e) = ctx.world.entities.get_mut(me) {
                e.body_mut().fall_distance = res.new_fall_distance;
            }
            self.last = [bx, by, bz];
            return None;
        }
        let in_water = Self::in_water(ctx.world, me);
        let fall = ctx
            .world
            .entities
            .get(me)
            .map(|e| e.body().fall_distance)
            .unwrap_or(0.0);
        let input = MovementInput {
            from_x: bx,
            from_y: by,
            from_z: bz,
            to_x: x,
            to_y: y,
            to_z: z,
            stance,
            on_ground,
            is_in_water: in_water,
            fall_distance: fall,
        };
        let res = movement_validate(&input);
        match res.status {
            1 => return self.kick("Illegal stance"),
            2 => return self.kick("Illegal position"),
            // Vanilla never kicks for distance ("moved wrongly" only snaps
            // the player back): teleport home and wait for the client echo
            // instead of disconnecting legitimate teleports and respawns.
            3 | 4 => {
                self.teleport_to(ctx.world, me, [bx, by, bz], final_yaw, final_pitch);
                return None;
            }
            _ => {}
        }
        // Vanilla NetServerHandler flow: move from last, then check the
        // residual (want - have). res² > 1/16 → moved wrongly → snap back.
        // dy in (-0.5, 0.5) is zeroed before the check (step tolerance).
        if let Some(e) = ctx.world.entities.get_mut(me) {
            e.body_mut().suppress_fall_state = true;
        }
        // Remember pre-move collision state for the wasFree check.
        let was_free = {
            use crate::aabb::AxisAlignedBB;
            let e = ctx.world.entities.get(me)?;
            let b = e.body();
            let probe = AxisAlignedBB::get_bounding_box(
                b.bounding_box.min_x + 0.0625,
                b.bounding_box.min_y + 0.0625,
                b.bounding_box.min_z + 0.0625,
                b.bounding_box.max_x - 0.0625,
                b.bounding_box.max_y - 0.0625,
                b.bounding_box.max_z - 0.0625,
            );
            ctx.world.colliding_boxes(&probe).is_empty()
        };
        ctx.world.move_body(me, x - bx, y - by, z - bz);
        if let Some(e) = ctx.world.entities.get_mut(me) {
            e.body_mut().suppress_fall_state = false;
            e.body_mut().yaw = final_yaw;
            e.body_mut().pitch = final_pitch;
        }
        let (px, py, pz, fall2) = match ctx.world.entities.get(me) {
            Some(e) => (e.body().pos[0], e.body().pos[1], e.body().pos[2], e.body().fall_distance),
            None => return None,
        };
        // Residual check (vanilla var23/var22).
        let rdx = x - px;
        let mut rdy = y - py;
        let rdz = z - pz;
        if rdy > -0.5 && rdy < 0.5 {
            rdy = 0.0;
        }
        let res_sq = rdx * rdx + rdy * rdy + rdz * rdz;
        let moved_wrongly = res_sq > crate::player::movement::VANILLA_WRONGLY_SQ;
        // Force client position like vanilla (setPositionAndRotation(want)),
        // then verify the destination is free when we started free.
        if !moved_wrongly {
            // Within tolerance: accept client position to avoid permanent
            // wall desync (old code kept the clipped server pos forever).
            if let Some(e) = ctx.world.entities.get_mut(me) {
                e.body_mut().set_position(x, y, z);
            }
        }
        let dest_free = {
            use crate::aabb::AxisAlignedBB;
            let e = ctx.world.entities.get(me)?;
            // Test the current body box shrunk by 0.0625 (vanilla func_694_e).
            let cur = &e.body().bounding_box;
            let shrunk = AxisAlignedBB::get_bounding_box(
                cur.min_x + 0.0625,
                cur.min_y + 0.0625,
                cur.min_z + 0.0625,
                cur.max_x - 0.0625,
                cur.max_y - 0.0625,
                cur.max_z - 0.0625,
            );
            ctx.world.colliding_boxes(&shrunk).is_empty()
        };
        if moved_wrongly || (was_free && !dest_free) {
            self.teleport_to(ctx.world, me, [bx, by, bz], final_yaw, final_pitch);
            return None;
        }
        let fall_input = MovementInput {
            from_x: bx,
            from_y: by,
            from_z: bz,
            // Vanilla func_9153_b uses the forced CLIENT Y (want), not the
            // clipped server Y — otherwise stepping onto slabs undercounts.
            to_x: x,
            to_y: y,
            to_z: z,
            stance,
            on_ground,
            is_in_water: in_water,
            fall_distance: fall2,
        };
        let fall_res = movement_validate(&fall_input);
        if fall_res.fall_damage > 0 {
            ctx.world.attack_living(me, fall_res.fall_damage, None);
        }
        if let Some(e) = ctx.world.entities.get_mut(me) {
            e.body_mut().fall_distance = fall_res.new_fall_distance;
            // Vanilla drives player onGround FROM THE PACKET
            // (NetServerHandler.handleFlying: `playerEntity.onGround =
            // packet.onGround`): a standing player sends zero-delta moves,
            // for which moveEntity would report false. Removing this broke
            // digging (5x off-ground penalty) and jumps. Fall *distance*
            // above stays server-measured, so damage is still authoritative.
            e.body_mut().on_ground = on_ground;
        }
        if let Some(e) = ctx.world.entities.get(me) {
            self.last = e.body().pos;
        }
        None
    }
}
