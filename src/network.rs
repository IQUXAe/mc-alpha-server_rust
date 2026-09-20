use std::io::Read;

#[derive(Clone, Copy)]
pub struct SlotData {
    pub item_id: i16,
    pub count: i8,
    pub damage: i16,
}

pub enum PacketData {
    KeepAlive,
    Login {
        protocol_version: i32,
        username: String,
        password: String,
        map_seed: i64,
        dimension: i8,
    },
    Handshake {
        username: String,
    },
    Chat {
        message: String,
    },
    PlayerInventory {
        inventory_type: i32,
        slots: Vec<SlotData>,
    },
    UseEntity {
        player_entity_id: i32,
        target_entity_id: i32,
        is_left_click: bool,
    },
    Respawn,
    Flying {
        on_ground: bool,
    },
    PlayerPosition {
        x: f64,
        y: f64,
        stance: f64,
        z: f64,
        on_ground: bool,
    },
    PlayerLook {
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    PlayerLookMove {
        x: f64,
        y: f64,
        stance: f64,
        z: f64,
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    BlockDig {
        status: i8,
        x: i32,
        y: i8,
        z: i32,
        face: i8,
    },
    Place {
        item_id: i16,
        x: i32,
        y: i8,
        z: i32,
        direction: i8,
    },
    BlockItemSwitch {
        entity_id: i32,
        item_id: i16,
    },
    ArmAnimation {
        entity_id: i32,
        animate: i8,
    },
    PickupSpawn {
        entity_id: i32,
        item_id: i16,
        count: i8,
        x: i32,
        y: i32,
        z: i32,
        rotation: i8,
        pitch: i8,
        roll: i8,
    },
    ComplexEntity {
        x: i32,
        y: i16,
        z: i32,
        nbt_data: Vec<u8>,
    },
    KickDisconnect {
        reason: String,
    },
}

/// Outbound tracker packet: only the kinds the tracker emits, with owned payloads.
#[derive(Clone, Debug)]
pub enum Packet {
    BlockItemSwitch { entity_id: i32, item_id: i16 },
    ArmAnimation { entity_id: i32, animate: i8 },
    NamedEntitySpawn {
        entity_id: i32,
        username: String,
        x: i32,
        y: i32,
        z: i32,
        rotation: i8,
        pitch: i8,
        current_item: i16,
    },
    PickupSpawn {
        entity_id: i32,
        item_id: i16,
        count: i8,
        x: i32,
        y: i32,
        z: i32,
        rotation: i8,
        pitch: i8,
        roll: i8,
    },
    VehicleSpawn { entity_id: i32, vehicle_type: i8, x: i32, y: i32, z: i32 },
    Collect { collected_id: i32, collector_id: i32 },
    MobSpawn {
        entity_id: i32,
        mob_type: u8,
        x: i32,
        y: i32,
        z: i32,
        yaw: i8,
        pitch: i8,
    },
    EntityVelocity { entity_id: i32, motion_x: i16, motion_y: i16, motion_z: i16 },
    DestroyEntity { entity_id: i32 },
    Entity { entity_id: i32 },
    RelEntityMove { entity_id: i32, dx: i8, dy: i8, dz: i8 },
    EntityLook { entity_id: i32, yaw: i8, pitch: i8 },
    RelEntityMoveLook {
        entity_id: i32,
        dx: i8,
        dy: i8,
        dz: i8,
        yaw: i8,
        pitch: i8,
    },
    EntityTeleport {
        entity_id: i32,
        x: i32,
        y: i32,
        z: i32,
        yaw: i8,
        pitch: i8,
    },
    EntityStatus { entity_id: i32, status: i8 },
    AttachEntity { entity_id: i32, vehicle_id: i32 },
}

fn read_exact<R: Read>(stream: &mut R, buf: &mut [u8]) -> std::io::Result<()> {
    stream.read_exact(buf)
}

fn read_u8<R: Read>(stream: &mut R) -> std::io::Result<u8> {
    let mut buf = [0; 1];
    stream.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16<R: Read>(stream: &mut R) -> std::io::Result<u16> {
    let mut buf = [0; 2];
    stream.read_exact(&mut buf)?;
    Ok(u16::from_be_bytes(buf))
}

fn read_i32<R: Read>(stream: &mut R) -> std::io::Result<i32> {
    let mut buf = [0; 4];
    stream.read_exact(&mut buf)?;
    Ok(i32::from_be_bytes(buf))
}

fn read_i64<R: Read>(stream: &mut R) -> std::io::Result<i64> {
    let mut buf = [0; 8];
    stream.read_exact(&mut buf)?;
    Ok(i64::from_be_bytes(buf))
}

fn read_utf<R: Read>(stream: &mut R) -> std::io::Result<String> {
    let len = read_u16(stream)? as usize;
    if len == 0 {
        return Ok(String::new());
    }
    let mut bytes = vec![0u8; len];
    read_exact(stream, &mut bytes)?;
    String::from_utf8(bytes).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

fn read_f64<R: Read>(stream: &mut R) -> std::io::Result<f64> {
    let val = read_i64(stream)?;
    Ok(f64::from_bits(val as u64))
}

fn read_f32<R: Read>(stream: &mut R) -> std::io::Result<f32> {
    let val = read_i32(stream)?;
    Ok(f32::from_bits(val as u32))
}

fn read_bool<R: Read>(stream: &mut R) -> std::io::Result<bool> {
    let val = read_u8(stream)?;
    Ok(val != 0)
}

pub(crate) fn read_packet_payload<R: Read>(stream: &mut R, packet_id: u8) -> std::io::Result<PacketData> {
    match packet_id {
        0 => Ok(PacketData::KeepAlive),
        1 => {
            let protocol_version = read_i32(stream)?;
            let username = read_utf(stream)?;
            let password = read_utf(stream)?;
            let map_seed = read_i64(stream)?;
            let dimension = read_u8(stream)? as i8;
            Ok(PacketData::Login {
                protocol_version,
                username,
                password,
                map_seed,
                dimension,
            })
        }
        2 => {
            let username = read_utf(stream)?;
            Ok(PacketData::Handshake { username })
        }
        3 => {
            let message = read_utf(stream)?;
            Ok(PacketData::Chat { message })
        }
        5 => {
            let inventory_type = read_i32(stream)?;
            let item_count = read_u16(stream)?;
            let mut slots = Vec::with_capacity(item_count as usize);
            for _ in 0..item_count {
                let item_id = read_u16(stream)? as i16;
                let mut slot = SlotData { item_id, count: 0, damage: 0 };
                if item_id >= 0 {
                    slot.count = read_u8(stream)? as i8;
                    slot.damage = read_u16(stream)? as i16;
                }
                slots.push(slot);
            }
            Ok(PacketData::PlayerInventory {
                inventory_type,
                slots,
            })
        }
        7 => {
            let player_entity_id = read_i32(stream)?;
            let target_entity_id = read_i32(stream)?;
            let is_left_click = read_bool(stream)?;
            Ok(PacketData::UseEntity {
                player_entity_id,
                target_entity_id,
                is_left_click,
            })
        }
        9 => Ok(PacketData::Respawn),
        10 => {
            let on_ground = read_bool(stream)?;
            Ok(PacketData::Flying { on_ground })
        }
        11 => {
            let x = read_f64(stream)?;
            let y = read_f64(stream)?;
            let stance = read_f64(stream)?;
            let z = read_f64(stream)?;
            let on_ground = read_bool(stream)?;
            Ok(PacketData::PlayerPosition {
                x,
                y,
                stance,
                z,
                on_ground,
            })
        }
        12 => {
            let yaw = read_f32(stream)?;
            let pitch = read_f32(stream)?;
            let on_ground = read_bool(stream)?;
            Ok(PacketData::PlayerLook {
                yaw,
                pitch,
                on_ground,
            })
        }
        13 => {
            let x = read_f64(stream)?;
            let y = read_f64(stream)?;
            let stance = read_f64(stream)?;
            let z = read_f64(stream)?;
            let yaw = read_f32(stream)?;
            let pitch = read_f32(stream)?;
            let on_ground = read_bool(stream)?;
            Ok(PacketData::PlayerLookMove {
                x,
                y,
                stance,
                z,
                yaw,
                pitch,
                on_ground,
            })
        }
        14 => {
            let status = read_u8(stream)? as i8;
            let x = read_i32(stream)?;
            let y = read_u8(stream)? as i8;
            let z = read_i32(stream)?;
            let face = read_u8(stream)? as i8;
            Ok(PacketData::BlockDig {
                status,
                x,
                y,
                z,
                face,
            })
        }
        15 => {
            let item_id = read_u16(stream)? as i16;
            let x = read_i32(stream)?;
            let y = read_u8(stream)? as i8;
            let z = read_i32(stream)?;
            let direction = read_u8(stream)? as i8;
            Ok(PacketData::Place {
                item_id,
                x,
                y,
                z,
                direction,
            })
        }
        16 => {
            let entity_id = read_i32(stream)?;
            let item_id = read_u16(stream)? as i16;
            Ok(PacketData::BlockItemSwitch {
                entity_id,
                item_id,
            })
        }
        18 => {
            let entity_id = read_i32(stream)?;
            let animate = read_u8(stream)? as i8;
            Ok(PacketData::ArmAnimation {
                entity_id,
                animate,
            })
        }
        21 => {
            let entity_id = read_i32(stream)?;
            let item_id = read_u16(stream)? as i16;
            let count = read_u8(stream)? as i8;
            let x = read_i32(stream)?;
            let y = read_i32(stream)?;
            let z = read_i32(stream)?;
            let rotation = read_u8(stream)? as i8;
            let pitch = read_u8(stream)? as i8;
            let roll = read_u8(stream)? as i8;
            Ok(PacketData::PickupSpawn {
                entity_id,
                item_id,
                count,
                x,
                y,
                z,
                rotation,
                pitch,
                roll,
            })
        }
        59 => {
            let x = read_i32(stream)?;
            let y = read_u16(stream)? as i16;
            let z = read_i32(stream)?;
            let len = read_u16(stream)? as usize;
            let mut nbt_data = vec![0; len];
            read_exact(stream, &mut nbt_data)?;
            Ok(PacketData::ComplexEntity {
                x,
                y,
                z,
                nbt_data,
            })
        }
        255 => {
            let reason = read_utf(stream)?;
            Ok(PacketData::KickDisconnect { reason })
        }
        _ => {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unsupported incoming packet ID: {}", packet_id),
            ))
        }
    }
}

/// Try to decode one packet from the front of `buf`.
///
/// If a full packet is present, it is drained from `buf` and `Ok(Some(pkt))`
/// is returned. If more data is needed, `buf` is left untouched and `Ok(None)`
/// is returned. If the data is corrupt or violates protocol, `Err` is returned.
pub(crate) fn try_decode_packet(buf: &mut Vec<u8>) -> std::io::Result<Option<PacketData>> {
    if buf.is_empty() {
        return Ok(None);
    }
    let packet_id = buf[0];
    let mut cursor = std::io::Cursor::new(&buf[1..]);
    match read_packet_payload(&mut cursor, packet_id) {
        Ok(pkt) => {
            let consumed = 1 + cursor.position() as usize;
            buf.drain(..consumed);
            Ok(Some(pkt))
        }
        Err(ref e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn put_u8(buf: &mut Vec<u8>, v: u8) { buf.push(v); }
pub(crate) fn put_i8(buf: &mut Vec<u8>, v: i8) { buf.push(v as u8); }
pub(crate) fn put_i16(buf: &mut Vec<u8>, v: i16) { buf.extend_from_slice(&v.to_be_bytes()); }
pub(crate) fn put_i32(buf: &mut Vec<u8>, v: i32) { buf.extend_from_slice(&v.to_be_bytes()); }
pub(crate) fn put_i64(buf: &mut Vec<u8>, v: i64) { buf.extend_from_slice(&v.to_be_bytes()); }
pub(crate) fn put_f32(buf: &mut Vec<u8>, v: f32) { buf.extend_from_slice(&v.to_bits().to_be_bytes()); }
pub(crate) fn put_f64(buf: &mut Vec<u8>, v: f64) { buf.extend_from_slice(&v.to_bits().to_be_bytes()); }
pub(crate) fn put_str(buf: &mut Vec<u8>, s: &str) {
    let bytes = s.as_bytes();
    put_i16(buf, bytes.len() as i16);
    buf.extend_from_slice(bytes);
}
/// Encode one outbound tracker packet (field order mirrors the vanilla
/// readers byte-for-byte; see the audit against the java/ sources).
pub fn encode_packet(pkt: &Packet, buf: &mut Vec<u8>) {
    match pkt {
        Packet::BlockItemSwitch { entity_id, item_id } => {
            put_u8(buf, 16);
            put_i32(buf, *entity_id);
            put_i16(buf, *item_id);
        }
        Packet::ArmAnimation { entity_id, animate } => {
            put_u8(buf, 18);
            put_i32(buf, *entity_id);
            put_i8(buf, *animate);
        }
        Packet::NamedEntitySpawn {
            entity_id,
            username,
            x,
            y,
            z,
            rotation,
            pitch,
            current_item,
        } => {
            put_u8(buf, 20);
            put_i32(buf, *entity_id);
            put_str(buf, username);
            put_i32(buf, *x);
            put_i32(buf, *y);
            put_i32(buf, *z);
            put_i8(buf, *rotation);
            put_i8(buf, *pitch);
            put_i16(buf, *current_item);
        }
        Packet::PickupSpawn {
            entity_id,
            item_id,
            count,
            x,
            y,
            z,
            rotation,
            pitch,
            roll,
        } => {
            put_u8(buf, 21);
            put_i32(buf, *entity_id);
            put_i16(buf, *item_id);
            put_i8(buf, *count);
            put_i32(buf, *x);
            put_i32(buf, *y);
            put_i32(buf, *z);
            put_i8(buf, *rotation);
            put_i8(buf, *pitch);
            put_i8(buf, *roll);
        }
        Packet::VehicleSpawn { entity_id, vehicle_type, x, y, z } => {
            put_u8(buf, 23);
            put_i32(buf, *entity_id);
            put_i8(buf, *vehicle_type);
            put_i32(buf, *x);
            put_i32(buf, *y);
            put_i32(buf, *z);
        }
        Packet::Collect { collected_id, collector_id } => {
            put_u8(buf, 22);
            put_i32(buf, *collected_id);
            put_i32(buf, *collector_id);
        }
        Packet::MobSpawn { entity_id, mob_type, x, y, z, yaw, pitch } => {
            put_u8(buf, 24);
            put_i32(buf, *entity_id);
            put_u8(buf, *mob_type);
            put_i32(buf, *x);
            put_i32(buf, *y);
            put_i32(buf, *z);
            put_i8(buf, *yaw);
            put_i8(buf, *pitch);
        }
        Packet::EntityVelocity { entity_id, motion_x, motion_y, motion_z } => {
            put_u8(buf, 28);
            put_i32(buf, *entity_id);
            put_i16(buf, *motion_x);
            put_i16(buf, *motion_y);
            put_i16(buf, *motion_z);
        }
        Packet::DestroyEntity { entity_id } => {
            put_u8(buf, 29);
            put_i32(buf, *entity_id);
        }
        Packet::Entity { entity_id } => {
            put_u8(buf, 30);
            put_i32(buf, *entity_id);
        }
        Packet::RelEntityMove { entity_id, dx, dy, dz } => {
            put_u8(buf, 31);
            put_i32(buf, *entity_id);
            put_i8(buf, *dx);
            put_i8(buf, *dy);
            put_i8(buf, *dz);
        }
        Packet::EntityLook { entity_id, yaw, pitch } => {
            put_u8(buf, 32);
            put_i32(buf, *entity_id);
            put_i8(buf, *yaw);
            put_i8(buf, *pitch);
        }
        Packet::RelEntityMoveLook { entity_id, dx, dy, dz, yaw, pitch } => {
            put_u8(buf, 33);
            put_i32(buf, *entity_id);
            put_i8(buf, *dx);
            put_i8(buf, *dy);
            put_i8(buf, *dz);
            put_i8(buf, *yaw);
            put_i8(buf, *pitch);
        }
        Packet::EntityTeleport { entity_id, x, y, z, yaw, pitch } => {
            put_u8(buf, 34);
            put_i32(buf, *entity_id);
            put_i32(buf, *x);
            put_i32(buf, *y);
            put_i32(buf, *z);
            put_i8(buf, *yaw);
            put_i8(buf, *pitch);
        }
        Packet::EntityStatus { entity_id, status } => {
            put_u8(buf, 38);
            put_i32(buf, *entity_id);
            put_i8(buf, *status);
        }
        Packet::AttachEntity { entity_id, vehicle_id } => {
            put_u8(buf, 39);
            put_i32(buf, *entity_id);
            put_i32(buf, *vehicle_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_named_spawn_roundtrip() {
        let pkt = Packet::NamedEntitySpawn {
            entity_id: 7,
            username: "steve".to_string(),
            x: 160,
            y: 1024,
            z: -48,
            rotation: 64,
            pitch: -32,
            current_item: 278,
        };
        let mut buf = Vec::new();
        encode_packet(&pkt, &mut buf);
        assert_eq!(buf[0], 20);
        assert_eq!(&buf[1..5], &7i32.to_be_bytes());
        assert_eq!(&buf[5..7], &5u16.to_be_bytes());
        assert_eq!(&buf[7..12], b"steve");
    }

    #[test]
    fn test_encode_mob_spawn_roundtrip() {
        let pkt = Packet::MobSpawn {
            entity_id: 9,
            mob_type: 50,
            x: 1,
            y: 2,
            z: 3,
            yaw: 10,
            pitch: -10,
        };
        let mut buf = Vec::new();
        encode_packet(&pkt, &mut buf);
        assert_eq!(
            buf,
            vec![24, 0, 0, 0, 9, 50, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 3, 10, 246]
        );
    }

    #[test]
    fn test_encode_collect_roundtrip() {
        let pkt = Packet::Collect { collected_id: 9, collector_id: 1 };
        let mut buf = Vec::new();
        encode_packet(&pkt, &mut buf);
        assert_eq!(buf, vec![22, 0, 0, 0, 9, 0, 0, 0, 1]);
    }
}
