//! Block materials (mirrors Java `Material.java`).
//!
//! Flag layout (`isLiquid`, `isSolid`, `canBlockGrass`,
//! `blocksMovement`, plus the `canBurn` flag set by `setBurning()`).
//!
//! The Java subclasses are represented as `const` constructors instead of
//! inheritance: `Material::transparent`, `Material::liquid` and
//! `Material::logic`. Static instances are associated constants with the
//! exact vanilla flags, including the burnable-init pass that
//! marks wood, leaves, cloth and tnt as burning.

/// Block material flags. `Copy` so the shared statics stay usable anywhere.
///
/// Identity (mirrors Java `Material.java`): vanilla compares singleton
/// *references* (`blockMaterial == Material.water`), never flags. Two
/// singletons can share every flag — `water`/`lava` are both
/// `MaterialLiquid`, `ground`/`rock`/`iron`/… are all plain `Material` —
/// so `==` must distinguish them. The `tag` field is that identity (one
/// value per vanilla singleton); flag-only comparison would alias
/// water with lava, ground with rock, air with fire, etc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Material {
    tag: u8,
    is_liquid: bool,
    is_solid: bool,
    can_block_grass: bool,
    blocks_movement: bool,
    can_burn: bool,
}

impl Material {
    /// Mirrors the C++ constructor defaults
    /// `(isLiquid=false, isSolid=true, canBlockGrass=true,
    /// blocksMovement=true)`. `tag` is the singleton identity (see the
    /// struct docs); anonymous instances use `TAG_ANONYMOUS`.
    pub const fn new(
        tag: u8,
        is_liquid: bool,
        is_solid: bool,
        can_block_grass: bool,
        blocks_movement: bool,
    ) -> Self {
        Self {
            tag,
            is_liquid,
            is_solid,
            can_block_grass,
            blocks_movement,
            can_burn: false,
        }
    }

    /// Mirrors `MaterialTransparent` / default-constructed solid material.
    pub const fn transparent(tag: u8) -> Self {
        Self::new(tag, false, false, false, false)
    }

    /// Mirrors `MaterialLiquid`.
    pub const fn liquid(tag: u8) -> Self {
        Self::new(tag, true, false, true, false)
    }

    /// Mirrors `MaterialLogic`.
    pub const fn logic(tag: u8) -> Self {
        Self::new(tag, false, false, false, false)
    }

    /// Const-compatible burning marker for the static table below.
    pub const fn burning(mut self) -> Self {
        self.can_burn = true;
        self
    }

    /// Mirrors `getIsLiquid()`.
    pub const fn is_liquid(self) -> bool {
        self.is_liquid
    }

    /// Mirrors `isSolid()`.
    pub const fn is_solid(self) -> bool {
        self.is_solid
    }

    /// Mirrors `getCanBlockGrass()`.
    pub const fn can_block_grass(self) -> bool {
        self.can_block_grass
    }

    /// Mirrors `blocksMovement()`.
    pub const fn blocks_movement(self) -> bool {
        self.blocks_movement
    }

    /// Mirrors `getBurning()`.
    pub const fn get_burning(self) -> bool {
        self.can_burn
    }

    /// Mirrors `setBurning()` (returns `&mut Self` like the C++ reference).
    pub fn set_burning(&mut self) -> &mut Self {
        self.can_burn = true;
        self
    }

    // Singleton identity tags (one per `Material.java` static; the values
    // mirror `BlockMaterial` where they overlap).
    pub const TAG_ANONYMOUS: u8 = 255;

    // Static instances with the exact flags from Material.cpp.
    pub const AIR: Self = Self::new(0, false, false, false, false);
    pub const GROUND: Self = Self::new(1, false, true, true, true);
    pub const WOOD: Self = Self::new(2, false, true, true, true).burning();
    pub const ROCK: Self = Self::new(3, false, true, true, true);
    pub const IRON: Self = Self::new(4, false, true, true, true);
    pub const WATER: Self = Self::new(5, true, false, true, false);
    pub const LAVA: Self = Self::new(6, true, false, true, false);
    pub const LEAVES: Self = Self::new(7, false, true, true, true).burning();
    pub const PLANTS: Self = Self::new(8, false, false, false, false);
    pub const SPONGE: Self = Self::new(9, false, true, true, true);
    pub const CLOTH: Self = Self::new(10, false, true, true, true).burning();
    pub const FIRE: Self = Self::new(11, false, false, false, false);
    pub const SAND: Self = Self::new(12, false, true, true, true);
    pub const CIRCUITS: Self = Self::new(13, false, false, false, false);
    pub const GLASS: Self = Self::new(14, false, true, true, true);
    pub const TNT: Self = Self::new(15, false, true, true, true).burning();
    pub const UNUSED: Self = Self::new(16, false, true, true, true);
    pub const ICE: Self = Self::new(17, false, true, true, true);
    pub const SNOW: Self = Self::new(18, false, false, false, false);
    pub const BUILT_SNOW: Self = Self::new(19, false, true, true, true);
    pub const CACTUS: Self = Self::new(20, false, true, true, true);
    pub const CLAY: Self = Self::new(21, false, true, true, true);
    pub const PUMPKIN: Self = Self::new(22, false, true, true, true);
    pub const PORTAL: Self = Self::new(23, false, true, true, true);
    pub const WEB: Self = Self::new(24, false, true, true, true);
}

impl Default for Material {
    /// Mirrors a default-constructed `Material` (solid ground-like).
    /// Anonymous tag: like Java `new Material()`, it is *not* the `ground`
    /// singleton even though every flag matches.
    fn default() -> Self {
        Self::new(Self::TAG_ANONYMOUS, false, true, true, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirror of test/TestMaterial.cpp: the raw statics as declared in
    // Material.cpp (before Block::initBlocks() swaps liquids in).

    #[test]
    fn air_properties() {
        assert!(!Material::AIR.is_liquid());
        assert!(!Material::AIR.is_solid());
        assert!(!Material::AIR.blocks_movement());
    }

    #[test]
    fn water_is_liquid() {
        assert!(Material::WATER.is_liquid());
        assert!(!Material::WATER.is_solid());
    }

    #[test]
    fn lava_is_liquid() {
        assert!(Material::LAVA.is_liquid());
        assert!(!Material::LAVA.is_solid());
    }

    #[test]
    fn rock_is_solid() {
        assert!(Material::ROCK.is_solid());
        assert!(Material::ROCK.blocks_movement());
        assert!(!Material::ROCK.is_liquid());
        assert!(!Material::ROCK.get_burning());
    }

    #[test]
    fn wood_is_burning() {
        // Static init sets burning for flammable materials.
        assert!(Material::WOOD.get_burning());
        assert!(Material::LEAVES.get_burning());
        assert!(Material::CLOTH.get_burning());
        assert!(Material::TNT.get_burning());
    }

    #[test]
    fn transparent_subclass() {
        let leaves = Material::transparent(Material::TAG_ANONYMOUS);
        assert!(!leaves.is_solid());
        assert!(!leaves.blocks_movement());
        assert!(!leaves.can_block_grass());
    }

    #[test]
    fn liquid_subclass() {
        let liquid = Material::liquid(Material::TAG_ANONYMOUS);
        assert!(liquid.is_liquid());
        assert!(!liquid.is_solid());
        assert!(!liquid.blocks_movement());
    }

    #[test]
    fn logic_subclass() {
        let logic = Material::logic(Material::TAG_ANONYMOUS);
        assert!(!logic.is_solid());
        assert!(!logic.blocks_movement());
        assert!(!logic.can_block_grass());
    }

    #[test]
    fn fire_default_no_burn() {
        assert!(!Material::FIRE.get_burning());
    }

    #[test]
    fn singleton_identity_water_is_not_lava() {
        // Java compares singleton references: water and lava share every
        // flag but are distinct singletons. Regression test for the live
        // bug where water dealt lava contact damage (and lava
        // self-extinguished) because `==` compared flags only.
        assert_ne!(Material::WATER, Material::LAVA);
        assert_ne!(Material::GROUND, Material::ROCK);
        assert_ne!(Material::AIR, Material::FIRE);
        assert_ne!(Material::AIR, Material::PLANTS);
        assert_eq!(Material::WATER, Material::WATER);
        assert_eq!(Material::LAVA, Material::LAVA);
    }

    #[test]
    fn set_burning() {
        let mut m = Material::default();
        assert!(!m.get_burning());
        m.set_burning();
        assert!(m.get_burning());
    }

    #[test]
    fn ground_properties() {
        assert!(Material::GROUND.is_solid());
        assert!(Material::GROUND.blocks_movement());
        assert!(Material::GROUND.can_block_grass());
        assert!(!Material::GROUND.is_liquid());
    }

    #[test]
    fn sand_properties() {
        assert!(Material::SAND.is_solid());
        assert!(Material::SAND.blocks_movement());
    }

    #[test]
    fn web_properties() {
        assert!(Material::WEB.blocks_movement());
    }
}
