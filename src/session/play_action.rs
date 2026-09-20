//! Entity use, chat, respawn, held-switch and arm swing on `PlaySession`.
//! Split out of `session.rs`; behavior unchanged.

use crate::entity::table::{AnimalKind, Entity};
use crate::inventory::ItemStack;
use crate::item_data::{item_max_damage, item_tool_kind};
use crate::player::combat::combat_get_weapon_damage;
use crate::server_admin::chat_command;
use crate::session::play::PlaySession;
use crate::session::{SessionBroadcast, SessionCtx, SessionOutcome};
use crate::session_packets::{pkt_health, pkt_respawn, pkt_teleport};
use crate::world::World;

const ATTACK_REACH_SQ: f64 = 25.0;

impl PlaySession {
    // ---- entity interaction (mirrors handleUseEntity) ----

    pub(crate) fn use_entity(
        &mut self,
        ctx: &mut SessionCtx,
        player_entity_id: i32,
        target_entity_id: i32,
        is_left_click: bool,
    ) -> Option<SessionOutcome> {
        let me = self.player;
        if player_entity_id != me {
            return None;
        }
        self.sync_held(ctx.world);
        let target = match ctx.world.entities.get(target_entity_id) {
            Some(t) if target_entity_id != me && !t.body().dead => target_entity_id,
            _ => return None,
        };
        if matches!(ctx.world.entities.get(target), Some(Entity::Arrow(_))) {
            return None;
        }
        if let Some(Entity::Player(_)) = ctx.world.entities.get(target) {
            if !ctx.pvp {
                return None;
            }
        }
        // Reach: attacker eye to target box within 25.
        let eye = match ctx.world.entities.get(me) {
            Some(e) => {
                let b = e.body();
                [b.pos[0], b.pos[1] + World::living_eye_height_pub(e), b.pos[2]]
            }
            None => return None,
        };
        let tb = match ctx.world.entities.get(target) {
            Some(e) => e.body().clone(),
            None => return None,
        };
        let cx = eye[0].clamp(tb.bounding_box.min_x, tb.bounding_box.max_x);
        let cy = eye[1].clamp(tb.bounding_box.min_y, tb.bounding_box.max_y);
        let cz = eye[2].clamp(tb.bounding_box.min_z, tb.bounding_box.max_z);
        let d2 = (eye[0] - cx).powi(2) + (eye[1] - cy).powi(2) + (eye[2] - cz).powi(2);
        if d2 > ATTACK_REACH_SQ {
            return None;
        }
        if !ctx.world.attack_los(me, target) {
            return None;
        }
        if !is_left_click {
            // Cow milking with a held bucket.
            if let Some(Entity::Animal(a)) = ctx.world.entities.get(target) {
                if a.kind == AnimalKind::Cow {
                    let cur = match ctx.world.entities.get(me) {
                        Some(Entity::Player(p)) => p.inventory.current,
                        _ => -1,
                    };
                    let bucket = (0..36).contains(&cur)
                        && matches!(
                            ctx.world.entities.get(me),
                            Some(Entity::Player(p)) if p.inventory.main[cur as usize]
                                .map(|s| s.item_id == 325)
                                .unwrap_or(false)
                        );
                    if bucket {
                        if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                            p.inventory.main[cur as usize] = Some(ItemStack::new(335, 1, 0));
                        }
                        self.send_inventory(ctx.world);
                        return None;
                    }
                }
            }
            // Saddled-pig mount (mirrors Java EntityPig.interact: riding
            // pigs board on right-click; unsaddled pigs ignore it).
            if let Some(Entity::Animal(a)) = ctx.world.entities.get(target) {
                if a.kind == AnimalKind::Pig && a.saddled {
                    ctx.world.entities.mount(me, Some(target));
                    ctx.world.entities.update_rider_position(target);
                    return None;
                }
            }
            // Boat mount toggle with rider rules.
            if matches!(ctx.world.entities.get(target), Some(Entity::Boat(_))) {
                let rider =
                    ctx.world.entities.get(target).map(|e| e.body().ridden_by).unwrap_or(-1);
                if rider >= 0 && rider != me {
                    let rider_is_player =
                        matches!(ctx.world.entities.get(rider), Some(Entity::Player(_)));
                    if rider_is_player {
                        return None;
                    }
                    ctx.world.entities.mount(rider, None);
                }
                let riding =
                    ctx.world.entities.get(me).map(|e| e.body().riding).unwrap_or(-1);
                ctx.world.entities.mount(me, if riding == target { None } else { Some(target) });
                ctx.world.entities.update_rider_position(target);
                return None;
            }
            return None;
        }
        // Left click: melee with the selected stack.
        let alive = match ctx.world.entities.get(me) {
            Some(Entity::Player(p)) => !p.living.body.dead && p.living.health > 0,
            _ => false,
        };
        if !alive {
            return None;
        }
        let held = self.selected_stack(ctx.world);
        let damage = held.map(|s| combat_get_weapon_damage(s.item_id).max(1)).unwrap_or(1);
        if damage <= 0 {
            return None;
        }
        ctx.world.attack_living(target, damage, Some(me));
        // Tool wear only against living targets.
        let living_target = matches!(
            ctx.world.entities.get(target),
            Some(Entity::Mob(_)) | Some(Entity::Animal(_)) | Some(Entity::Player(_))
        );
        if let Some(mut s) = held {
            if living_target {
                let kind = item_tool_kind(s.item_id);
                let wear = if kind == crate::item_data::ItemToolKind::Pickaxe as i32
                    || kind == crate::item_data::ItemToolKind::Spade as i32
                    || kind == crate::item_data::ItemToolKind::Axe as i32
                {
                    2
                } else if kind == crate::item_data::ItemToolKind::Sword as i32 {
                    1
                } else {
                    0
                };
                if wear > 0 {
                    let max = item_max_damage(s.item_id);
                    crate::inventory::item_stack_damage(&mut s, wear, max);
                    if s.count <= 0 {
                        // A spent ghost clears the fallback (and the held
                        // id); a spent real stack clears its own slot.
                        if self.held_fallback.is_some() {
                            self.held_fallback = None;
                            self.held_id = 0;
                            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                                p.inventory.current = 0;
                            }
                        } else {
                            let cur = match ctx.world.entities.get(me) {
                                Some(Entity::Player(p)) => p.inventory.current,
                                _ => -1,
                            };
                            if (0..36).contains(&cur) {
                                if let Some(Entity::Player(p)) =
                                    ctx.world.entities.get_mut(me)
                                {
                                    p.inventory.main[cur as usize] = None;
                                }
                            }
                        }
                        self.send_inventory(ctx.world);
                    } else {
                        // Worn ghost stays in the fallback; a worn real
                        // stack returns to its own slot, 35 included.
                        if self.held_fallback.is_some() {
                            self.held_fallback = Some(s);
                        } else {
                            let cur = match ctx.world.entities.get(me) {
                                Some(Entity::Player(p)) => p.inventory.current,
                                _ => -1,
                            };
                            if (0..36).contains(&cur) {
                                if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
                                    p.inventory.main[cur as usize] = Some(s);
                                }
                            }
                        }
                    }
                }
            }
        }
        None
    }

    // ---- chat / commands / respawn / misc ----

/// Vanilla chat charset (Java NetServerHandler.handleChat allow-list).
const CHAT_ALLOWED: &str = " !\"#$%&'()*+,-./0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZ[\\]^_'abcdefghijklmnopqrstuvwxyz{|}~⌂ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜø£Ø×ƒáíóúñÑªº¿®¬½¼¡«»";

    pub(crate) fn chat(&mut self, ctx: &mut SessionCtx, message: &str) -> Option<SessionOutcome> {
        // Vanilla gates (Java NetServerHandler.handleChat): chars (not
        // bytes) over 100 kick, then trim, then the allowed-charset kick.
        if message.chars().count() > 100 {
            return Some(SessionOutcome::Kick("Chat message too long".to_string()));
        }
        let msg = message.trim().to_string();
        if !msg.chars().all(|c| Self::CHAT_ALLOWED.contains(c)) {
            return Some(SessionOutcome::Kick("Illegal characters in chat".to_string()));
        }
        if msg.starts_with('/') {
            chat_command(ctx.world, self, ctx.ops, ctx.broadcast, &msg);
            return None;
        }
        let username = self.username(ctx.world).to_string();
        ctx.broadcast.push(SessionBroadcast::Chat(format!("<{username}> {msg}")));
        None
    }

    pub(crate) fn respawn(&mut self, ctx: &mut SessionCtx) -> Option<SessionOutcome> {
        let me = self.player;
        let alive = match ctx.world.entities.get(me) {
            Some(Entity::Player(p)) => p.living.health > 0,
            _ => return None,
        };
        if alive {
            return None;
        }
        let sp = ctx.world.spawn;
        if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(me) {
            p.living.body.dead = false;
            p.living.health = p.living.max_health;
            p.living.hurt_time = 0;
            p.living.death_time = 0;
            p.living.body.fire = 0;
            p.living.body.air = 300;
            p.living.body.fall_distance = 0.0;
            p.living.body.motion = [0.0; 3];
            p.respawn_ticks = 60;
        }
        let (sx, sy, sz) = (sp[0] as f64 + 0.5, sp[1] as f64, sp[2] as f64 + 0.5);
        if let Some(e) = ctx.world.entities.get_mut(me) {
            e.body_mut().set_position(sx, sy, sz);
            e.body_mut().yaw = 0.0;
            e.body_mut().pitch = 0.0;
        }
        self.last = [sx, sy, sz];
        self.has_moved = false;
        self.teleport_wait = Some([sx, sy, sz]);
        // Packet order mirrors C++: respawn, health, teleport, inventory.
        self.outbox.push(pkt_respawn());
        self.outbox.push(pkt_teleport(sx, sy, sz, 0.0, 0.0));
        self.outbox.push(pkt_health(match ctx.world.entities.get(me) {
            Some(Entity::Player(p)) => p.living.health as i8,
            _ => 0,
        }));
        self.send_inventory(ctx.world);
        None
    }

    pub(crate) fn held_switch(&mut self, ctx: &mut SessionCtx, item_id: i16) {
        if item_id == 0 {
            self.held_id = 0;
            self.held_fallback = None;
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(self.player) {
                p.inventory.current = 0;
            }
            return;
        }
        self.held_id = item_id as i32;
        self.sync_held(ctx.world);
    }

    pub(crate) fn arm(&mut self, ctx: &mut SessionCtx, animate: i8) {
        if animate == 1 {
            ctx.broadcast.push(SessionBroadcast::ArmSwing(self.player));
        } else if animate == 104 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(self.player) {
                p.living.sneaking = true;
            }
        } else if animate == 105 {
            if let Some(Entity::Player(p)) = ctx.world.entities.get_mut(self.player) {
                p.living.sneaking = false;
            }
        }
    }
}
