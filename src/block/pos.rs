//! Core block coordinates and drop specs.
//!
//! `BlockPos` replaces the `(x, y, z)` triple threaded through every
//! block driver. `DropSpec` replaces the `(item, count, damage)` triple.
//! Both are `Copy` so drivers stay cheap and clippy-clean.

/// Integer block position.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl BlockPos {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub const fn offset(self, dx: i32, dy: i32, dz: i32) -> Self {
        Self {
            x: self.x + dx,
            y: self.y + dy,
            z: self.z + dz,
        }
    }

    #[inline]
    pub const fn below(self) -> Self {
        self.offset(0, -1, 0)
    }

    #[inline]
    pub const fn above(self) -> Self {
        self.offset(0, 1, 0)
    }

    /// Center of the block as floating point (for item spawns).
    #[inline]
    pub fn center(self) -> (f64, f64, f64) {
        (
            self.x as f64 + 0.5,
            self.y as f64 + 0.5,
            self.z as f64 + 0.5,
        )
    }

    /// Center with the vanilla drop height bias (+0.7 on Y).
    #[inline]
    pub fn drop_center(self) -> (f64, f64, f64) {
        (
            self.x as f64 + 0.5,
            self.y as f64 + 0.7,
            self.z as f64 + 0.5,
        )
    }
}

impl From<[i32; 3]> for BlockPos {
    fn from(v: [i32; 3]) -> Self {
        Self::new(v[0], v[1], v[2])
    }
}

impl From<BlockPos> for [i32; 3] {
    fn from(p: BlockPos) -> Self {
        [p.x, p.y, p.z]
    }
}

/// What a block drops: item id + count + damage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropSpec {
    pub item: i32,
    pub count: i32,
    pub damage: i32,
}

impl DropSpec {
    pub const fn new(item: i32, count: i32, damage: i32) -> Self {
        Self {
            item,
            count,
            damage,
        }
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.item <= 0 || self.count <= 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pos_offsets_and_centers() {
        let p = BlockPos::new(1, 2, 3);
        assert_eq!(p.below(), BlockPos::new(1, 1, 3));
        assert_eq!(p.above(), BlockPos::new(1, 3, 3));
        assert_eq!(p.offset(-1, 0, 1), BlockPos::new(0, 2, 4));
        assert_eq!(BlockPos::from([1, 2, 3]), p);
        let [x, y, z]: [i32; 3] = p.into();
        assert_eq!((x, y, z), (1, 2, 3));
        assert!(DropSpec::new(0, 1, 0).is_empty());
        assert!(!DropSpec::new(1, 1, 0).is_empty());
    }
}
