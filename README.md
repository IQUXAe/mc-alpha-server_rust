# alpha_server

A lightweight, high-performance server implementation for **Minecraft Alpha 1.2.6**, written in pure Rust from scratch. Protocol-compatible with original vanilla Alpha 1.2.6 clients (MultiMC, Betacraft, Prism Launcher, vanilla launcher).

---

## Table of Contents
1. [Overview & Key Features](#overview--key-features)
2. [Alpha 1.2.6 Protocol Implementation](#alpha-126-protocol-implementation)
3. [Architecture](#architecture)
4. [Memory Profiles & Resource Consumption](#memory-profiles--resource-consumption)
5. [Low-Memory & Router Deployment Guide](#low-memory--router-deployment-guide)
6. [Block Digging & Anti-Cheat Parity](#block-digging--anti-cheat-parity)
7. [Building & Running](#building--running)
8. [Configuration (`server.properties`)](#configuration-serverproperties)
9. [License](#license)

---

## Overview & Key Features

- **Protocol Parity**: 100% network and gameplay compatibility with Alpha 1.2.6 clients.
- **Ultra-Low Memory Footprint**: Runs in as little as **~15–18 MB RSS** with an active player on embedded routers, compared to 500+ MB on the JVM vanilla server.
- **1:1 Terrain Generation**: Exact reproduction of Alpha 1.2.6 noise, density fields, biomes, cave carving, tree decorators, and ore distribution.
- **Synchronous Mining Physics**: Natural crack animations and break particles/sounds with anti-cheat protection against speed mining and instant break hacks.
- **Persistence**: LevelDB-backed chunk store (`rusty-leveldb`) and gzip-NBT for `level.dat` and player inventories.
- **Embedded Ready**: Zero JVM dependencies; compiles to a single static binary suitable for OpenWrt, Entware, and low-spec Linux devices.

---

## Alpha 1.2.6 Protocol Implementation

`alpha_server` implements the Alpha 1.2.6 network protocol (Protocol Version 2):
- **Transport**: Raw TCP stream with binary packet framing (`0x00` through `0xFF`).
- **Connection Handshake**: Handles `0x02` (Handshake) and `0x01` (LoginRequest). Supports both online authentication via Mojang/Betacraft session check URLs and LAN offline mode.
- **Chunk Streaming**: Chunk allocation via `Packet 50 (0x32, PreChunk)` followed by compressed chunk data via `Packet 51 (0x33, MapChunk)`. Raw chunk payloads are compressed using zlib level 1.
- **Block Interaction**:
  - `Packet 14 (0x0E, BlockDig)`:
    - `status = 0`: Start digging / click block (triggers immediate harvest for instant-break blocks like torches and redstone).
    - `status = 1`: Progressive mining damage tick.
    - `status = 2`: Cancel digging (aborts current progress and clears latched target).
    - `status = 3`: Client completion signal (sent when client crack animation reaches 100%).
  - `Packet 15 (0x0F, BlockPlace)`: Placing blocks, right-clicking interactive blocks (furnace, chest, lever, button, door), and consuming usable items.
  - `Packet 16 (0x10, BlockItemSwitch)`: Active hotbar slot selection.
  - `Packet 53 (0x35, BlockChange)`: Server block state synchronization.
- **Entities & Spawning**: Full synchronization for players, hostile mobs (Zombies, Skeletons, Spiders, Creepers), passive animals (Pigs, Sheep, Cows, Chickens), primed TNT, falling sand/gravel, dropped items, arrows, and boats.

---

## Architecture

The server codebase is organized into modular components under `src/`:

| Module | Location | Purpose |
| :--- | :--- | :--- |
| **Server Engine** | `src/server/` | Fixed 20 TPS (50 ms) deterministic tick loop, connection lifecycle, chunk streaming queues, entity tracking fanout, console admin commands, LevelDB save manager, and optional async background chunk worker. |
| **Session & Network** | `src/session/` | Binary packet encoders/decoders (`network.rs`), login state machine, player session handler (`play.rs`), block digging physics (`play_digging.rs`), inventory actions, and combat. |
| **World Storage** | `src/world/` | Chunk map grid (`HashMap<(i32, i32), Box<Chunk>>`), block accessors, lighting propagation (skylight and blocklight), block update scheduler, liquid physics, and entity table. |
| **Chunk Engine** | `src/chunk.rs` | 16×128×16 chunk column data structure (blocks, metadata nibbles, blocklight, skylight, heightmap). Zlib compression with immediate buffer eviction post-streaming. |
| **Terrain Generator** | `src/generator.rs`, `density.rs`, `biome.rs`, `caves.rs`, `decorators/` | Bit-exact 1:1 replica of the Alpha 1.2.6 world generator: Perlin/simplex noise octaves, biome temperature/humidity fields, cave carvers, trees, lakes, and ores. |
| **Entity System** | `src/entity/` | Entity representations, AABB spatial collisions, pathfinding, mob AI, gravity, fluid drag, health, and combat. |
| **Persistence** | `src/persist.rs` | LevelDB chunk store (`rusty-leveldb`) with configurable write buffers and block cache; gzip-NBT serialization for player files and `level.dat`. |
| **Config & Admin** | `src/server_config.rs`, `server_admin.rs`, `commands.rs` | `server.properties` parser/serializer, ops list, IP/player bans, and interactive console. |

---

## Memory Profiles & Resource Consumption

Unlike JVM servers that require 500 MB–1 GB of heap just to start, `alpha_server` is optimized for deterministic, minimal memory allocation. Chunks take exactly 80 KB of uncompressed memory per column (32 KB blocks + 16 KB metadata + 16 KB blocklight + 16 KB skylight + 256 B heightmap).

### Real-World Memory Profiles (RSS)

| Profile | Chunks Loaded | Real Resident Memory (RSS) | Description |
| :--- | :---: | :---: | :--- |
| **Cold Start (0 players)** | 0 | **~3–5 MB** | Process started, database opened, listening on port. |
| **Idle (0 players, spawn loaded)** | 49 (7×7) | **~6–8 MB** | Protected spawn area generated/cached. |
| **1 Player (`view-distance=6`)** | 169 (13×13) | **~15–18 MB** | **Recommended for low-memory routers (128 MB RAM).** |
| **1 Player (`view-distance=8`)** | 289 (17×17) | **~25–28 MB** | Balanced profile (smooth view, modest memory). |
| **1 Player (`view-distance=10`)** | 441 (21×21) | **~35–42 MB** | Vanilla PC default view distance. |

### Memory Optimizations Implemented
1. **On-Demand Compressed Cache Invalidation**: Compressed chunk packets (15–25 KB each) are freed immediately after streaming to the client. This prevents 441 chunks from retaining ~9–10 MB of duplicate compressed data in memory.
2. **Persistent Scratch Buffers**: Per-tick vectors for entity tracking, light updates, and chunk drops retain capacity across ticks, eliminating heap churn.
3. **Embedded LevelDB Defaults**: Write buffer (512 KB) and block cache (512 KB) are tuned down from LevelDB's default 4 MB + 4 MB.

---

## Low-Memory & Router Deployment Guide

To run `alpha_server` on an embedded router or low-spec single-board computer (e.g., OpenWrt, Raspberry Pi, ASUS Merlin):

### 1. Recommended `server.properties`
```properties
view-distance=6
max-connections=16
max-players=4
```

### 2. Tunable Environment Variables
You can override LevelDB memory settings via environment variables before launching:
```bash
# Sets LevelDB write buffer to 256 KB (default: 512 KB)
export ALPHA_LEVELDB_WRITE_BUFFER_KB=256

# Sets LevelDB block cache to 256 KB (default: 512 KB)
export ALPHA_LEVELDB_CACHE_KB=256

./alpha_server
```

### 3. Static Musl Build for Routers
Cross-compile a statically linked binary without glibc dependencies:
```bash
# Install musl target
rustup target add x86_64-unknown-linux-musl

# Build static release binary
cargo build --release --target x86_64-unknown-linux-musl
```
*(For ARM/MIPS routers, use `aarch64-unknown-linux-musl`, `armv7-unknown-linux-musleabihf`, or `mips-unknown-linux-musl` with an appropriate cross-toolchain).*

---

## Block Digging & Anti-Cheat Parity

In Minecraft Alpha 1.2.6, mining relies on tight synchronization between client and server:
- **Instant Break**: Blocks with 0.0 hardness (torches, redstone wire, saplings, flowers) are broken immediately on click (`status = 0`), playing sound and spawning particles without delay.
- **Progressive Mining**:
  - The client ticks mining damage every tick (`status = 1`), displaying crack stages 0 through 9.
  - When the client finishes breaking a block (progress reaches 100%), it triggers `sendBlockRemoved`:
    - Spawns break particle effects.
    - Plays block destruction sound.
    - Removes block locally and dispatches `Packet 14 (status = 3)`.
- **Anti-Cheat & Desync Tolerance**:
  - The server verifies that the player has accumulated sufficient mining work (`cur_damage >= 0.70`).
  - To prevent **false rollbacks** when a player is jumping or swimming (where airborne/underwater penalties slow client mining speed), the server also tracks ground-equivalent progress (`ground_damage >= 0.70`).
  - The background server tick (`dig_tick`) advances progress up to 0.80 (for both `cur_damage` and `ground_damage`) to account for network jitter without ever nearing the break threshold, and **never harvests blocks in the background**. Harvesting is strictly client-packet driven so blocks never vanish prematurely without break animations.
  - Instant break cheats (0 ticks) and speed mining cheats (2x–5x speed) are strictly rejected and rolled back via `Packet 53 (BlockChange)`.

---

## Building & Running

### Requirements
- Stable Rust toolchain (1.75+ recommended).

### Compilation
```bash
git clone https://github.com/iquxae/alpha_server.git
cd alpha_server/rust
cargo build --release
```
The compiled binary will be located at `target/release/alpha_server`.

### Running
```bash
./target/release/alpha_server [server.properties] [world-dir] [port]
```
All arguments are optional:
- `server.properties`: Defaults to `./server.properties`.
- `world-dir`: Defaults to `world/<level-name>`.
- `port`: Overrides `server-port` from properties (default: `25565`).

---

## Configuration (`server.properties`)

```properties
# Minecraft server properties
server-port=25565
server-ip=
online-mode=true
level-name=world
level-seed=
difficulty=2
pvp=true
spawn-animals=true
spawn-monsters=true
max-players=20
max-connections=256
spawn-protection-radius=16
auto-save-interval=6000

# View distance (radius in chunks).
# Memory profiles:
#   view-distance=6  -> 13x13 (169 chunks) -> ~15-18 MB RAM (recommended for embedded/low-memory routers)
#   view-distance=8  -> 17x17 (289 chunks) -> ~25-28 MB RAM (balanced)
#   view-distance=10 -> 21x21 (441 chunks) -> ~35-45 MB RAM (vanilla PC default)
view-distance=6
```

---

## License

GPLv3 or later, see [LICENSE](LICENSE). Copyright (C) 2026 IQUXAe.

> *Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.*
