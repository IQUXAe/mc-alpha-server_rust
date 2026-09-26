//! Play-packet constructors, split out of `session.rs`.
//! Re-exported from `session` so `crate::session::pkt_*` keeps working.

use crate::inventory::ItemStack;
use crate::network::{put_f32, put_f64, put_i16, put_i32, put_i64, put_i8, put_str, put_u8};
use crate::world::TileData;

pub fn pkt_handshake(server_id: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + server_id.len());
    put_u8(&mut b, 2);
    put_str(&mut b, server_id);
    b
}

pub fn pkt_kick(reason: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + reason.len());
    put_u8(&mut b, 255);
    put_str(&mut b, reason);
    b
}

pub fn pkt_login_response(entity_id: i32, seed: i64, dimension: i8) -> Vec<u8> {
    let mut b = Vec::with_capacity(16);
    put_u8(&mut b, 1);
    put_i32(&mut b, entity_id);
    put_str(&mut b, "");
    put_str(&mut b, "");
    put_i64(&mut b, seed);
    put_i8(&mut b, dimension);
    b
}

pub fn pkt_chat(msg: &str) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + msg.len());
    put_u8(&mut b, 3);
    put_str(&mut b, msg);
    b
}

pub fn pkt_time(time: i64) -> Vec<u8> {
    let mut b = Vec::with_capacity(9);
    put_u8(&mut b, 4);
    put_i64(&mut b, time);
    b
}

pub fn pkt_spawn_pos(x: i32, y: i32, z: i32) -> Vec<u8> {
    let mut b = Vec::with_capacity(13);
    put_u8(&mut b, 6);
    put_i32(&mut b, x);
    put_i32(&mut b, y);
    put_i32(&mut b, z);
    b
}

pub fn pkt_health(health: i8) -> Vec<u8> {
    let mut b = Vec::with_capacity(2);
    put_u8(&mut b, 8);
    put_i8(&mut b, health);
    b
}

pub fn pkt_teleport(x: f64, y: f64, z: f64, yaw: f32, pitch: f32) -> Vec<u8> {
    let mut b = Vec::with_capacity(42);
    put_u8(&mut b, 13);
    put_f64(&mut b, x);
    put_f64(&mut b, y + 1.62f32 as f64);
    put_f64(&mut b, y);
    put_f64(&mut b, z);
    put_f32(&mut b, yaw);
    put_f32(&mut b, pitch);
    b.push(0);
    b
}

pub fn pkt_block_change(x: i32, y: i32, z: i32, block_type: u8, meta: u8) -> Vec<u8> {
    let mut b = Vec::with_capacity(12);
    put_u8(&mut b, 53);
    crate::network::put_i32(&mut b, x);
    put_i8(&mut b, y as i8);
    crate::network::put_i32(&mut b, z);
    put_u8(&mut b, block_type);
    put_u8(&mut b, meta);
    b
}

fn put_slot(buf: &mut Vec<u8>, s: Option<ItemStack>) {
    match s {
        Some(v) if v.count > 0 => {
            put_i16(buf, v.item_id as i16);
            put_i8(buf, v.count as i8);
            put_i16(buf, v.damage as i16);
        }
        // Vanilla writes empty slots as a bare -1 (2 bytes, no
        // count/damage tail). Anything longer desyncs the stream: the
        // client reads short-only for negative ids, plants phantom
        // Item(0)s into the following slots, and NPEs rendering the
        // hotbar (blocksList[0] is null). This crashed real clients
        // on every fresh login; synthetic tests never render.
        _ => {
            put_i16(buf, -1);
        }
    }
}

pub fn pkt_inventory_section(inv_type: i32, slots: &[Option<ItemStack>]) -> Vec<u8> {
    let mut b = Vec::with_capacity(7 + slots.len() * 5);
    put_u8(&mut b, 5);
    crate::network::put_i32(&mut b, inv_type);
    put_i16(&mut b, slots.len() as i16);
    for s in slots {
        put_slot(&mut b, *s);
    }
    b
}

pub fn pkt_tile_entity(x: i32, y: i32, z: i32, nbt_gz: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(13 + nbt_gz.len());
    put_u8(&mut b, 59);
    crate::network::put_i32(&mut b, x);
    put_i16(&mut b, y as i16);
    crate::network::put_i32(&mut b, z);
    put_i16(&mut b, nbt_gz.len() as i16);
    b.extend_from_slice(nbt_gz);
    b
}

pub fn pkt_respawn() -> Vec<u8> {
    vec![9]
}

pub fn pkt_keepalive() -> Vec<u8> {
    vec![0]
}

pub fn pkt_arm(entity_id: i32, animate: i8) -> Vec<u8> {
    let mut b = Vec::with_capacity(6);
    put_u8(&mut b, 18);
    crate::network::put_i32(&mut b, entity_id);
    put_i8(&mut b, animate);
    b
}

pub fn pkt_pre_chunk(x: i32, z: i32, mode: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(10);
    put_u8(&mut b, 50);
    crate::network::put_i32(&mut b, x);
    crate::network::put_i32(&mut b, z);
    put_u8(&mut b, if mode { 1 } else { 0 });
    b
}

pub fn pkt_explosion(x: f64, y: f64, z: f64, radius: f32, cells: &[(i32, i32, i32)]) -> Vec<u8> {
    let mut b = Vec::with_capacity(1 + 32 + cells.len() * 3);
    put_u8(&mut b, 60);
    put_f64(&mut b, x);
    put_f64(&mut b, y);
    put_f64(&mut b, z);
    put_f32(&mut b, radius);
    put_i32(&mut b, cells.len() as i32);
    let ox = x as i32;
    let oy = y as i32;
    let oz = z as i32;
    for &(cx, cy, cz) in cells {
        put_i8(&mut b, (cx - ox) as i8);
        put_i8(&mut b, (cy - oy) as i8);
        put_i8(&mut b, (cz - oz) as i8);
    }
    b
}

pub fn pkt_map_chunk(
    x: i32,
    y: i32,
    z: i32,
    size_x: i32,
    size_y: i32,
    size_z: i32,
    data: &[u8],
) -> Vec<u8> {
    let mut b = Vec::with_capacity(18 + data.len());
    put_u8(&mut b, 51);
    crate::network::put_i32(&mut b, x);
    put_i16(&mut b, y as i16);
    crate::network::put_i32(&mut b, z);
    put_u8(&mut b, (size_x - 1) as u8);
    put_u8(&mut b, (size_y - 1) as u8);
    put_u8(&mut b, (size_z - 1) as u8);
    crate::network::put_i32(&mut b, data.len() as i32);
    b.extend_from_slice(data);
    b
}

pub fn pkt_subchunk_in_chunk(
    world: &crate::world::World,
    x0: i32,
    y0: i32,
    z0: i32,
    sx: i32,
    sy: i32,
    sz: i32,
) -> Option<Vec<u8>> {
    use std::io::Read as _;
    if sx <= 0
        || sy <= 0
        || sz <= 0
        || (y0 & 1) != 0
        || (sy & 1) != 0
        || y0 < 0
        || y0 + sy > crate::world::WORLD_HEIGHT
    {
        return None;
    }
    let (cx, cz) = (x0.div_euclid(16), z0.div_euclid(16));
    if (x0 + sx - 1).div_euclid(16) != cx || (z0 + sz - 1).div_euclid(16) != cz {
        return None;
    }
    let chunk = world.chunk_ref(cx, cz)?;
    let lx0 = x0.rem_euclid(16);
    let lz0 = z0.rem_euclid(16);
    let lx1 = lx0 + sx;
    let lz1 = lz0 + sz;
    let y1 = y0 + sy;
    let vol = (sx * sy * sz) as usize;
    let nibble_len = vol / 2;
    let mut raw = Vec::with_capacity(vol + 3 * nibble_len);
    for lx in lx0..lx1 {
        for lz in lz0..lz1 {
            for y in y0..y1 {
                raw.push(chunk.get_block_id(lx, y, lz));
            }
        }
    }
    for lx in lx0..lx1 {
        for lz in lz0..lz1 {
            for yb in (y0 / 2)..(y1 / 2) {
                let y = yb * 2;
                raw.push(
                    ((chunk.get_block_metadata(lx, y + 1, lz) & 0xF) << 4)
                        | (chunk.get_block_metadata(lx, y, lz) & 0xF),
                );
            }
        }
    }
    for lx in lx0..lx1 {
        for lz in lz0..lz1 {
            for yb in (y0 / 2)..(y1 / 2) {
                let y = yb * 2;
                raw.push(
                    ((chunk.get_blocklight(lx, y + 1, lz) & 0xF) << 4)
                        | (chunk.get_blocklight(lx, y, lz) & 0xF),
                );
            }
        }
    }
    for lx in lx0..lx1 {
        for lz in lz0..lz1 {
            for yb in (y0 / 2)..(y1 / 2) {
                let y = yb * 2;
                raw.push(
                    ((chunk.get_skylight(lx, y + 1, lz) & 0xF) << 4)
                        | (chunk.get_skylight(lx, y, lz) & 0xF),
                );
            }
        }
    }
    let mut encoder = flate2::read::ZlibEncoder::new(raw.as_slice(), flate2::Compression::new(1));
    let mut compressed = Vec::new();
    encoder.read_to_end(&mut compressed).ok()?;
    Some(pkt_map_chunk(x0, y0, z0, sx, sy, sz, &compressed))
}

pub fn pkt_subchunk_block(world: &crate::world::World, x: i32, y: i32, z: i32) -> Option<Vec<u8>> {
    if !(0..crate::world::WORLD_HEIGHT).contains(&y) {
        return None;
    }
    pkt_subchunk_in_chunk(world, x, y & !1, z, 1, 2, 1)
}

/// Build per-chunk `Packet51MapChunk` subchunks covering the 13-block
/// blocklight radius around a furnace at `(x, y, z)`.
///
/// Because preceding `Packet53BlockChange` with a subchunk suppresses
/// `Chunk.setBlockIDWithMetadata`'s `onBlockAdded` (keeping
/// `GuiFurnace.field_978_j` alive), it also skips the client's own
/// `func_616_a(EnumSkyBlock.Block, ...)` relight pass. Shipping the
/// server's freshly calculated light maps for the 13-block radius
/// updates client lighting and triggers `func_701_b` re-rendering.
pub fn pkt_subchunk_furnace_light(
    world: &crate::world::World,
    x: i32,
    y: i32,
    z: i32,
) -> Vec<((i32, i32), Vec<u8>)> {
    if !(0..crate::world::WORLD_HEIGHT).contains(&y) {
        return Vec::new();
    }
    const R: i32 = 13;
    let wx0 = x - R;
    let wx_end = x + R + 1;
    let wz0 = z - R;
    let wz_end = z + R + 1;
    let y0 = (y - R).max(0) & !1;
    let y1 = ((y + R + 1).min(crate::world::WORLD_HEIGHT) + 1) & !1;
    let y1 = y1.min(crate::world::WORLD_HEIGHT).max(y0 + 2);
    let sy = y1 - y0;
    let cx0 = wx0.div_euclid(16);
    let cx1 = (wx_end - 1).div_euclid(16);
    let cz0 = wz0.div_euclid(16);
    let cz1 = (wz_end - 1).div_euclid(16);
    let mut out = Vec::new();
    for cx in cx0..=cx1 {
        let x_start = wx0.max(cx * 16);
        let x_end = wx_end.min((cx + 1) * 16);
        for cz in cz0..=cz1 {
            let z_start = wz0.max(cz * 16);
            let z_end = wz_end.min((cz + 1) * 16);
            if let Some(pkt) = pkt_subchunk_in_chunk(
                world,
                x_start,
                y0,
                z_start,
                x_end - x_start,
                sy,
                z_end - z_start,
            ) {
                out.push(((cx, cz), pkt));
            }
        }
    }
    out
}

pub fn tile_packet(x: i32, y: i32, z: i32, tile: &TileData) -> Vec<u8> {
    use crate::nbt::write_root;
    use std::io::Write as _;
    // Root is the tile compound itself, like C++ writeRoot.
    let mut comp = crate::persist::tile_nbt(x, y, z, tile);
    // Alpha 1.2.6 client `TileEntityFurnace.readFromNBT` ignores `"ItemBurnTime"`
    // and recomputes `currentItemBurnTime = getItemBurnTime(furnaceItemStacks[1])`
    // (defaulting to 200 in `getBurnTimeRemainingScaled` when slot 1 is empty).
    // Scale the wire `"BurnTime"` to that denominator so the GUI flame indicator
    // reflects the true remaining burn fraction instead of sticking at >=100%.
    if let TileData::Furnace(s) = tile {
        let slot_fuel = if s.slots[crate::tile_entity::furnace::SLOT_FUEL].count > 0 {
            crate::tile_entity::furnace::fuel_burn_time(
                s.slots[crate::tile_entity::furnace::SLOT_FUEL].item_id,
            )
        } else {
            0
        };
        let client_max = if slot_fuel > 0 { slot_fuel } else { 200 };
        let server_max = crate::tile_entity::furnace::infer_furnace_max_burn_time(
            s.burn_time,
            s.current_item_burn_time,
            slot_fuel,
        );
        let net_burn = if s.burn_time > 0 {
            ((s.burn_time as i32 * client_max) / server_max).clamp(1, client_max) as i16
        } else {
            0
        };
        comp.map
            .insert("BurnTime".to_string(), crate::nbt::NbtTag::Short(net_burn));
    }
    let mut bytes = Vec::new();
    write_root(&mut bytes, "", &comp).ok();
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&bytes).ok();
    let gz = enc.finish().ok().unwrap_or_default();
    pkt_tile_entity(x, y, z, &gz)
}
