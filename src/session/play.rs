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
    /// For furnaces, follow `Packet59ComplexEntity` with a 1x2x1 subchunk
    /// packet so if the client's `getBlockTileEntity` lazily triggered
    /// `BlockFurnace.onBlockAdded` -> `func_284_h` (overwriting facing
    /// metadata to 3 and queuing a `WorldBlockPositionType` rollback),
    /// `handleMapChunk` clears the rollback and restores the server facing
    /// without replacing the `TileEntityFurnace` in `chunkTileEntityMap`.
    pub fn send_tile(&mut self, world: &World, x: i32, y: i32, z: i32) {
        if let Some(tile) = world.tiles.get(&(x, y, z)) {
            self.outbox.push(tile_packet(x, y, z, tile));
            if matches!(tile, crate::world::TileData::Furnace(_)) {
                if let Some(sc) = crate::session_packets::pkt_subchunk_block(world, x, y, z) {
                    self.outbox.push(sc);
                }
            }
        }
    }

    /// Block-change rollback packet with current cell state.
    /// For furnaces (`61`/`62`), precede `Packet53BlockChange` with a 1x2x1
    /// subchunk packet so `Chunk.setBlockIDWithMetadata` on the client sees
    /// matching block ID and metadata and returns `false` instead of calling
    /// `BlockFurnace.onBlockAdded` (which would replace `GuiFurnace.field_978_j`).
    pub fn send_block_change(&mut self, world: &World, x: i32, y: i32, z: i32) {
        let id = world.get_block_id(x, y, z);
        let meta = world.get_block_meta(x, y, z);
        if matches!(id, 61 | 62) {
            if let Some(sc) = crate::session_packets::pkt_subchunk_block(world, x, y, z) {
                self.outbox.push(sc);
            }
        }
        self.outbox.push(pkt_block_change(x, y, z, id, meta));
    }

    /// Held-item sync (mirrors `syncHeldItemSelection`).
    pub(crate) fn sync_held(&mut self, world: &mut World) {
        if self.held_id <= 0 {
            return;
        }
        if let Some(Entity::Player(p)) = world.entities.get_mut(self.player) {
            for i in 0..36 {
                if let Some(s) = p.inventory.main[i] {
                    if s.item_id == self.held_id {
                        p.inventory.current = i as i32;
                        break;
                    }
                }
            }
        }
    }

    /// Login-path held restore (mirrors `restoreHeldItem`, called only
    /// with the saved id when positive): point `current` at the slot
    /// holding that item.
    pub fn restore_held(&mut self, world: &mut World, item_id: i32) {
        if item_id <= 0 {
            self.held_id = 0;
            if let Some(Entity::Player(p)) = world.entities.get_mut(self.player) {
                p.inventory.current = 0;
            }
            return;
        }
        self.held_id = item_id;
        self.sync_held(world);
    }

    /// Selected stack (mirrors `getSelectedItemStack`): returns only a real
    /// non-empty stack in `p.inventory.main[current]` (matching `self.held_id`
    /// when a held id is active), never a fabricated ghost stack.
    pub(crate) fn selected_stack(&self, world: &World) -> Option<ItemStack> {
        if let Some(Entity::Player(p)) = world.entities.get(self.player) {
            let cur = p.inventory.current;
            if cur >= 0 && (cur as usize) < p.inventory.main.len() {
                if let Some(s) = p.inventory.main[cur as usize] {
                    if !s.is_empty() && (self.held_id <= 0 || s.item_id == self.held_id) {
                        return Some(s);
                    }
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

    /// Eye-in-water probe (mirrors vanilla `Entity::isInsideOfMaterial(Material.water)`).
    /// Used for digging speed penalty where head/eye submersion determines the 5x slowdown.
    pub(crate) fn is_inside_water(world: &World, id: EntityId) -> bool {
        let (pos, y_offset) = match world.entities.get(id) {
            Some(e) => (e.body().pos, e.body().y_offset as f64),
            None => return false,
        };
        // Eye height: Java EntityPlayerMP.func_104_p() = 1.62.
        // Server Entity.posY is feet; eye is posY + 1.62 - y_offset.
        let eye_y = pos[1] + 1.62 - y_offset;
        let bx = floor_double(pos[0]);
        let by = floor_double(eye_y);
        let bz = floor_double(pos[2]);
        if world.material_at(bx, by, bz) == crate::material::Material::WATER {
            let meta = world.get_block_meta(bx, by, bz);
            let var0 = if meta >= 8 { 0 } else { meta };
            let var8 = var0 as f32 / 9.0;
            let surface_y = (by + 1) as f64 - var8 as f64;
            eye_y < surface_y
        } else {
            false
        }
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
            PacketData::PickupSpawn {
                item_id,
                count,
                x,
                y,
                z,
                rotation,
                pitch,
                roll,
                ..
            } => {
                self.pickup_spawn(ctx, item_id, count, x, y, z, rotation, pitch, roll);
                None
            }
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
