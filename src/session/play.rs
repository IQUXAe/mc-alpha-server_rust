//! `PlaySession` core: state, dispatch (`pump`), movement echo, sends.
//! Split out of `session.rs`; behavior unchanged.

use crate::entity::table::{Entity, EntityId};
use crate::inventory::ItemStack;
use crate::math_helper::floor_double;
use crate::network::PacketData;
use crate::player::digging::{DigState, dig_state_new};
use crate::session::{SessionCtx, SessionOutcome};
use crate::session_packets::{
    pkt_block_change, pkt_inventory_section, pkt_keepalive, pkt_kick, pkt_teleport, tile_packet,
};
use crate::world::World;

// ---- play session state ----

/// Reach for melee (mirrors `kMaxAttackReach`).
/// An authenticated player connection: packet dispatch against the world
/// (mirrors the `NetServerHandler` handlers). Chunk streaming, entity
/// tracking fan-out, and saves belong to the server tick.
pub struct PlaySession {
    pub player: EntityId,
    pub outbox: Vec<Vec<u8>>,
    pub dig: DigState,
    pub has_moved: bool,
    pub last: [f64; 3],
    pub held_id: i32,
    pub held_fallback: Option<ItemStack>,
    pub keepalive_tick: u32,
    pub gone: bool,
    /// Last health byte sent as 0x08 (mirrors the `Packet8` diff-check in
    /// `EntityPlayerMP`); the server tick pushes on change.
    pub last_health: i8,
    /// Server teleport awaiting client echo (mirrors `field_9006_j` in
    /// `NetServerHandler`): while set, movement packets that do not match
    /// the teleported spot are stale pre-teleport traffic and are held,
    /// never kicked (this is what made respawn disconnect far travelers).
    pub teleport_wait: Option<[f64; 3]>,
}

impl PlaySession {
    pub fn new(player: EntityId) -> Self {
        Self {
            player,
            outbox: Vec::new(),
            dig: dig_state_new(),
            has_moved: false,
            last: [0.0; 3],
            held_id: 0,
            held_fallback: None,
            keepalive_tick: 0,
            gone: false,
            last_health: 20,
            teleport_wait: None,
        }
    }

    pub(crate) fn kick(&mut self, reason: &str) -> Option<SessionOutcome> {
        self.outbox.push(pkt_kick(reason));
        self.gone = true;
        Some(SessionOutcome::Kick(reason.to_string()))
    }

    pub(crate) fn username<'a>(&self, world: &'a World) -> &'a str {
        match world.entities.get(self.player) {
            Some(Entity::Player(p)) => &p.username,
            _ => "",
        }
    }

    pub(crate) fn is_op(&self, ctx: &SessionCtx) -> bool {
        // Ops store lowercased like C++; the query lowercases too
        // (mirrors `isOp`, so mixed-case names keep their rights).
        ctx.ops.contains(&self.username(ctx.world).to_ascii_lowercase())
    }

    /// Teleport (mirrors `NetServerHandler::teleport`, stance y+1.62).
    pub fn teleport_to(
        &mut self,
        world: &mut World,
        id: EntityId,
        pos: [f64; 3],
        yaw: f32,
        pitch: f32,
    ) {
        let [x, y, z] = pos;
        if let Some(e) = world.entities.get_mut(id) {
            e.body_mut().set_position(x, y, z);
            e.body_mut().yaw = yaw;
            e.body_mut().pitch = pitch;
        }
        if id == self.player {
            self.last = [x, y, z];
            self.has_moved = false;
            self.teleport_wait = Some([x, y, z]);
        }
        self.outbox.push(pkt_teleport(x, y, z, yaw, pitch));
    }

    /// Full inventory sync (mirrors `sendInventory`: main/craft/armor).
    pub fn send_inventory(&mut self, world: &World) {
        let (main, crafting, armor) = match world.entities.get(self.player) {
            Some(Entity::Player(p)) => (
                p.inventory.main.to_vec(),
                p.inventory.crafting.to_vec(),
                p.inventory.armor.to_vec(),
            ),
            _ => return,
        };
        self.outbox.push(pkt_inventory_section(-1, &main));
        self.outbox.push(pkt_inventory_section(-2, &crafting));
        self.outbox.push(pkt_inventory_section(-3, &armor));
    }

    /// Tile-entity packet for one cell, if a tile row exists.
    pub fn send_tile(&mut self, world: &World, x: i32, y: i32, z: i32) {
        if let Some(tile) = world.tiles.get(&(x, y, z)) {
            self.outbox.push(tile_packet(x, y, z, tile));
        }
    }

    /// Block-change rollback packet with current cell state.
    pub fn send_block_change(&mut self, world: &World, x: i32, y: i32, z: i32) {
        self.outbox.push(pkt_block_change(
            x,
            y,
            z,
            world.get_block_id(x, y, z),
            world.get_block_meta(x, y, z),
        ));
    }

    /// Held-item sync (mirrors `syncHeldItemSelection`).
    pub(crate) fn sync_held(&mut self, world: &mut World) {
        if self.held_id <= 0 {
            self.held_fallback = None;
            return;
        }
        let mut found = false;
        if let Some(Entity::Player(p)) = world.entities.get_mut(self.player) {
            for i in 0..36 {
                if let Some(s) = p.inventory.main[i] {
                    if s.item_id == self.held_id {
                        p.inventory.current = i as i32;
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                // Genuine desync (or creative): park the ghost fallback
                // without touching real slot contents, slot 35 included.
                p.inventory.current = 35;
            }
        }
        if found {
            self.held_fallback = None;
        } else if self.held_fallback.map(|s| s.item_id) != Some(self.held_id) {
            self.held_fallback = Some(Self::ghost_stack(self.held_id));
        }
    }

    /// Login-path held restore (mirrors `restoreHeldItem`, called only
    /// with the saved id when positive): point `current` at the slot
    /// holding that item, else stage the fallback copy in the last slot.
    pub fn restore_held(&mut self, world: &mut World, item_id: i32) {
        if item_id <= 0 {
            if let Some(Entity::Player(p)) = world.entities.get_mut(self.player) {
                p.inventory.current = 0;
            }
            return;
        }
        self.held_id = item_id;
        self.sync_held(world);
    }

    /// Selected stack (mirrors `getSelectedItemStack`): ghost fallback
    /// first while active (its slot-35 shadow holds real content that must
    /// not be consumed as the held item), else the real current slot.
    pub(crate) fn selected_stack(&self, world: &World) -> Option<ItemStack> {
        if let Some(s) = self.held_fallback {
            if self.held_id > 0 && s.item_id == self.held_id {
                return Some(s);
            }
        }
        if let Some(Entity::Player(p)) = world.entities.get(self.player) {
            let cur = p.inventory.current;
            if cur >= 0 && (cur as usize) < p.inventory.main.len() {
                if let Some(s) = p.inventory.main[cur as usize] {
                    return Some(s);
                }
            }
        }
        None
    }

    /// Water probe for digging/movement (mirrors the entity probe; native
    /// rows never store `in_water`).
    pub(crate) fn in_water(world: &World, id: EntityId) -> bool {
        let bb = match world.entities.get(id) {
            Some(e) => e.body().bounding_box.expand(0.0, -0.4, 0.0),
            None => return false,
        };
        for x in floor_double(bb.min_x)..=floor_double(bb.max_x) {
            for y in floor_double(bb.min_y)..=floor_double(bb.max_y) {
                for z in floor_double(bb.min_z)..=floor_double(bb.max_z) {
                    if world.material_at(x, y, z) == crate::material::Material::WATER {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Per-tick upkeep (mirrors the handler `tick` minus chunk streaming:
    /// keep-alive every 20 ticks).
    pub fn tick(&mut self, _ctx: &mut SessionCtx) -> Option<SessionOutcome> {
        if self.gone {
            return None;
        }
        self.keepalive_tick += 1;
        if self.keepalive_tick.is_multiple_of(20) {
            self.outbox.push(pkt_keepalive());
        }
        None
    }

    /// Dispatch one inbound packet (mirrors the handler switch).
    pub fn pump(&mut self, ctx: &mut SessionCtx, pkt: PacketData) -> Option<SessionOutcome> {
        if self.gone {
            return None;
        }
        // Borrow split: the world for rows, the session for state.
        // Handlers take both explicitly to keep this readable.
        match pkt {
            PacketData::Flying { on_ground } => {
                self.movement(ctx, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, false, false, on_ground)
            }
            PacketData::PlayerPosition {
                x,
                y,
                stance,
                z,
                on_ground,
            } => self.movement(ctx, x, y, stance, z, 0.0, 0.0, true, false, on_ground),
            PacketData::PlayerLook {
                yaw,
                pitch,
                on_ground,
            } => self.movement(ctx, 0.0, 0.0, 0.0, 0.0, yaw, pitch, false, true, on_ground),
            PacketData::PlayerLookMove {
                x,
                y,
                stance,
                z,
                yaw,
                pitch,
                on_ground,
            } => self.movement(ctx, x, y, stance, z, yaw, pitch, true, true, on_ground),
            PacketData::BlockDig {
                status,
                x,
                y,
                z,
                face,
            } => self.dig(ctx, status, x, y as i32, z, face),
            PacketData::Place {
                item_id,
                x,
                y,
                z,
                direction,
            } => self.place(ctx, item_id, x, y as i32, z, direction),
            PacketData::UseEntity {
                player_entity_id,
                target_entity_id,
                is_left_click,
            } => self.use_entity(ctx, player_entity_id, target_entity_id, is_left_click),
            PacketData::Chat { message } => self.chat(ctx, &message),
            PacketData::Respawn => self.respawn(ctx),
            PacketData::BlockItemSwitch { item_id, .. } => {
                self.held_switch(ctx, item_id);
                None
            }
            PacketData::ArmAnimation { animate, .. } => {
                self.arm(ctx, animate);
                None
            }
            PacketData::PlayerInventory { inventory_type, slots } => {
                self.apply_inventory(ctx, inventory_type, &slots);
                None
            }
            PacketData::ComplexEntity { x, y, z, nbt_data } => {
                self.complex_entity(ctx, x, y as i32, z, &nbt_data);
                None
            }
            PacketData::PickupSpawn { .. } => None,
            PacketData::KickDisconnect { .. } => {
                self.gone = true;
                Some(SessionOutcome::Gone)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "play_tests.rs"]
mod play_tests;
