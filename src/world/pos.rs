//! `BlockPos` accessors on [`World`]: thin delegates over the
//! `(x, y, z)` API so block drivers stay position-based without
//! repeating `pos.x, pos.y, pos.z` at every call site.

use super::World;
use crate::block::pos::{BlockPos, DropSpec};
use crate::material::Material;

impl World {
    #[inline]
    pub fn id_at(&self, pos: BlockPos) -> u8 {
        self.get_block_id(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn meta_at(&self, pos: BlockPos) -> u8 {
        self.get_block_meta(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn set_id_at(&mut self, pos: BlockPos, id: u8) -> bool {
        self.set_block_id(pos.x, pos.y, pos.z, id)
    }

    #[inline]
    pub fn set_meta_at(&mut self, pos: BlockPos, meta: u8) -> bool {
        self.set_block_meta(pos.x, pos.y, pos.z, meta)
    }

    #[inline]
    pub fn material_at_pos(&self, pos: BlockPos) -> Material {
        self.material_at(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn light_at(&self, pos: BlockPos) -> u8 {
        self.block_light_value(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn sky_at(&self, pos: BlockPos) -> bool {
        self.can_see_sky(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn attach_at(&self, pos: BlockPos) -> bool {
        self.block_allows_attachment(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn solid_at(&self, pos: BlockPos) -> bool {
        self.is_solid(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn set_notify_at(&mut self, pos: BlockPos, id: u8) -> bool {
        self.apply_set_notify(pos.x, pos.y, pos.z, id)
    }

    #[inline]
    pub fn set_meta_notify_at(&mut self, pos: BlockPos, id: u8, meta: u8) -> bool {
        self.apply_set_meta_notify(pos.x, pos.y, pos.z, id, meta)
    }

    #[inline]
    pub fn notify_at(&mut self, pos: BlockPos) {
        self.notify_neighbors_of(pos.x, pos.y, pos.z)
    }

    #[inline]
    pub fn schedule_at(&mut self, pos: BlockPos, id: u8, delay: i32) {
        self.schedule_block_update(pos.x, pos.y, pos.z, id, delay)
    }

    #[inline]
    pub fn drop_at(&mut self, drop: DropSpec, pos: BlockPos, chance: f32) -> bool {
        crate::block::ticks::block_base_drop(self, drop, pos, chance)
    }

    #[inline]
    pub fn ignite_at(&mut self, pos: BlockPos, fuse: i32) {
        self.ignite_tnt(pos.x, pos.y, pos.z, fuse)
    }
}
