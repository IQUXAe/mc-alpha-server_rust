use std::sync::OnceLock;

#[repr(i32)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BiomeType {
    Rainforest = 0,
    Swampland = 1,
    SeasonalForest = 2,
    Forest = 3,
    Savanna = 4,
    Shrubland = 5,
    Taiga = 6,
    Desert = 7,
    Plains = 8,
    IceDesert = 9,
    Tundra = 10,
    Hell = 11,
}

#[derive(Clone, Copy, Debug)]
pub struct MobSpawnerBase {
    pub biome_type: BiomeType,
    pub top_block: u8,
    pub filler_block: u8,
}

impl MobSpawnerBase {
    pub const DEFAULT: Self = Self {
        biome_type: BiomeType::Plains,
        top_block: 2,
        filler_block: 3,
    };

    pub const RAINFOREST: Self = Self { biome_type: BiomeType::Rainforest, top_block: 2, filler_block: 3 };
    pub const SWAMPLAND: Self = Self { biome_type: BiomeType::Swampland, top_block: 2, filler_block: 3 };
    pub const SEASONAL_FOREST: Self = Self { biome_type: BiomeType::SeasonalForest, top_block: 2, filler_block: 3 };
    pub const FOREST: Self = Self { biome_type: BiomeType::Forest, top_block: 2, filler_block: 3 };
    pub const SAVANNA: Self = Self { biome_type: BiomeType::Savanna, top_block: 2, filler_block: 3 };
    pub const SHRUBLAND: Self = Self { biome_type: BiomeType::Shrubland, top_block: 2, filler_block: 3 };
    pub const TAIGA: Self = Self { biome_type: BiomeType::Taiga, top_block: 2, filler_block: 3 };
    pub const DESERT: Self = Self { biome_type: BiomeType::Desert, top_block: 12, filler_block: 12 };
    pub const PLAINS: Self = Self { biome_type: BiomeType::Plains, top_block: 2, filler_block: 3 };
    pub const ICE_DESERT: Self = Self { biome_type: BiomeType::IceDesert, top_block: 12, filler_block: 12 };
    pub const TUNDRA: Self = Self { biome_type: BiomeType::Tundra, top_block: 2, filler_block: 3 };
    pub const HELL: Self = Self { biome_type: BiomeType::Hell, top_block: 2, filler_block: 3 };
}

pub fn get_biome(temp: f32, mut humid: f32) -> MobSpawnerBase {
    humid *= temp;
    if temp < 0.1 {
        return MobSpawnerBase::TUNDRA;
    }
    if humid < 0.2 {
        if temp < 0.5 {
            return MobSpawnerBase::TUNDRA;
        }
        if temp < 0.95 {
            return MobSpawnerBase::SAVANNA;
        }
        return MobSpawnerBase::DESERT;
    }
    if humid > 0.5 && temp < 0.7 {
        return MobSpawnerBase::SWAMPLAND;
    }
    if temp < 0.5 {
        return MobSpawnerBase::TAIGA;
    }
    if temp < 0.97 {
        if humid < 0.35 {
            return MobSpawnerBase::SHRUBLAND;
        }
        return MobSpawnerBase::FOREST;
    }
    if humid < 0.45 {
        return MobSpawnerBase::PLAINS;
    }
    if humid < 0.9 {
        return MobSpawnerBase::SEASONAL_FOREST;
    }
    MobSpawnerBase::RAINFOREST
}

static BIOME_LOOKUP_TABLE: OnceLock<[MobSpawnerBase; 4096]> = OnceLock::new();

fn init_biome_lookup() -> &'static [MobSpawnerBase; 4096] {
    BIOME_LOOKUP_TABLE.get_or_init(|| {
        let mut table = [MobSpawnerBase::DEFAULT; 4096];
        for i in 0..64 {
            for j in 0..64 {
                table[i + j * 64] = get_biome(i as f32 / 63.0f32, j as f32 / 63.0f32);
            }
        }
        table
    })
}

pub fn get_biome_from_lookup(temp: f64, humid: f64) -> MobSpawnerBase {
    let table = init_biome_lookup();
    let mut t = (temp * 63.0) as i32;
    let mut h = (humid * 63.0) as i32;
    t = t.clamp(0, 63);
    h = h.clamp(0, 63);
    table[(t + h * 64) as usize]
}
