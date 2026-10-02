# alpha_server

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Protocol](https://img.shields.io/badge/Minecraft%20Alpha-v1.2.3__04%20--%20v1.2.6%20(Protocol%206)-brightgreen.svg)](#alpha-protocol-implementation)
[![Target](https://img.shields.io/badge/Target-Linux%20%7C%20musl%20%7C%20OpenWrt-lightgrey.svg)](#low-memory--router-deployment-guide)

A lightweight, high-performance server implementation for **Minecraft Alpha (v1.2.3_04 through v1.2.6, Protocol Version 6)**, written in pure Rust from scratch. Protocol-compatible with original vanilla Alpha clients (MultiMC, Betacraft, Prism Launcher, vanilla launcher).

---

## Table of Contents
1. [Overview & Key Features](#overview--key-features)
2. [Quick Start & Client Setup](#quick-start--client-setup)
3. [Server Administration & Commands](#server-administration--commands)
4. [Configuration (`server.properties`)](#configuration-serverproperties)
5. [Memory Profiles & Resource Consumption](#memory-profiles--resource-consumption)
6. [Low-Memory & Router Deployment Guide](#low-memory--router-deployment-guide)
7. [Alpha Protocol Implementation](#alpha-protocol-implementation)
8. [Architecture](#architecture)
9. [Mechanics & Anti-Cheat Parity](#mechanics--anti-cheat-parity)
10. [Nether Dimension & Alpha Client Limitations](#nether-dimension--alpha-client-limitations)
11. [Development & Testing](#development--testing)
12. [License](#license)

---

## Overview & Key Features

- **Protocol Parity**: 100% network and gameplay compatibility with Minecraft Alpha clients running **Protocol Version 6** (Alpha v1.2.3_04 through v1.2.6).
- **Ultra-Low Memory Footprint**: Runs in as little as **~15–18 MB RSS** with an active player on embedded routers, compared to 500+ MB on the JVM vanilla server.
- **1:1 Terrain Generation**: Exact reproduction of Alpha noise, density fields, biomes, cave carving, tree decorators, and ore distribution.
- **World Mechanisms & Physics**: Complete redstone signal recalculation (levels 0–15), torch burnout prevention, levers, buttons, pressure plates, doors, water/lava fluid spread, and obsidian/cobblestone formation.
- **Tile Entities**: Interactive furnaces (fuel burning and item smelting), single and double chests, signs, and mob spawners.
- **Synchronous Mining Physics**: Natural crack animations and break particles/sounds with anti-cheat protection against speed mining and instant break hacks.
- **Persistence**: LevelDB-backed chunk store (`rusty-leveldb`) and gzip-NBT for `level.dat` and player inventories.
- **Embedded Ready**: Zero JVM dependencies; compiles to a single static binary suitable for OpenWrt, Entware, and low-spec Linux devices.
- **Vanilla Dimension Mode (`hellworld`)**: Runs either the Overworld (dimension 0) or the Nether (dimension ‑1) via `hellworld=true/false` in `server.properties`, fully reproducing vanilla Minecraft Alpha server behavior without requiring client-side modifications.
- **Full Mob Roster**: Zombie, Skeleton, Spider, Creeper plus Giant (summon-only), Slime (splits), Ghast (fireballs) and PigZombie (neutral until hurt, hell + overworld).
- **SMP Entity Extensions**: Vanilla Alpha SMP never spawns arrows-from-bows, fish hooks, snowballs, minecarts or primed TNT server-side (singleplayer-only guards, no tracker branches). This server spawns and tracks them (snowball 61, hook 90, fireball 65, cart 10, TNT 50) so bows (no draw cooldown, like vanilla — spam is gated by clicks and the packet cap), timing-skill fishing (reel inside the nibble window), snowballs (damage 0 + knockback), rail auto-connects with turns/slopes, pushable minecarts (0.4 speed cap) and visible TNT fuses work in multiplayer. Paintings stay unimplemented like vanilla: Alpha has no painting spawn packet, so they cannot render for other players.
- **Anti-Hog Budgets**: 64-slot background-gen queue, 15 chunks/tick/player, 20-tick bow draw cooldown, one fish hook per player, 8192-cell explosion cap — one player cannot stall the 50ms tick for everyone.

---

## Quick Start & Client Setup

### 1. Requirements
- Stable Rust toolchain (1.75+ recommended).

### 2. Building
```bash
git clone https://github.com/IQUXAe/mc-alpha-server_rust.git
cd mc-alpha-server_rust
cargo build --release
```
The compiled binary will be located at `target/release/alpha_server`.

### 3. Running
```bash
./target/release/alpha_server [server.properties] [world-dir] [port]
```
All command-line arguments are optional:
- `server.properties`: Defaults to `./server.properties`.
- `world-dir`: Defaults to `world/<level-name>`.
- `port`: Overrides `server-port` from properties (default: `25565`).

### 4. Client Connection
1. **Launchers**: Any modern launcher supporting legacy Alpha versions (Prism Launcher, Betacraft, MultiMC, or the official Minecraft launcher).
2. **Version Selection**: Choose **Alpha v1.2.6** (recommended), or any compatible Protocol 6 version:
   - `Alpha v1.2.6`
   - `Alpha v1.2.5`
   - `Alpha v1.2.4 / v1.2.4_01`
   - `Alpha v1.2.3_04 / v1.2.3_05`
3. **Server Address**: `localhost:25565` (or your server's IP address).
4. **Authentication (`online-mode`)**:
   - `online-mode=true` (default): Authenticates players against Mojang / Betacraft session servers.
   - `online-mode=false`: LAN / offline mode allowing players to join without external authentication.

---

## Server Administration & Commands

The server includes an interactive console that accepts commands in real time:

### Console & In-Game Commands

| Command | Syntax | Description |
| :--- | :--- | :--- |
| **`help`** / **`?`** | `help` | Lists all available console commands. |
| **`list`** | `list` | Displays the current player count and list of connected players. |
| **`stop`** | `stop` | Gracefully flushes all world data and player inventories to disk and shuts down the server. |
| **`save-all`** | `save-all` | Forces an immediate write of all loaded chunks and player state to LevelDB and disk. |
| **`save-off`** | `save-off` | Suspends automatic world saving (useful for safe filesystem backups). |
| **`save-on`** | `save-on` | Resumes automated background world saves. |
| **`op`** | `op <player>` | Grants operator privileges and saves to `ops.txt`. |
| **`deop`** | `deop <player>` | Revokes operator privileges and removes from `ops.txt`. |
| **`ban`** | `ban <player>` | Bans a username, saves to `banned-players.txt`, and disconnects them. |
| **`pardon`** | `pardon <player>` | Unbans a username, removing them from `banned-players.txt`. |
| **`ban-ip`** | `ban-ip <ip>` | Bans an IP address, saves to `banned-ips.txt`, and kicks all matching connections. |
| **`pardon-ip`** | `pardon-ip <ip>` | Removes an IP address from `banned-ips.txt`. |
| **`kick`** | `kick <player>` | Forcibly disconnects an active player session. |
| **`tp`** | `tp <player> [target]` | Teleports `<player>` to the position of `[target]`. |
| **`give`** | `give <player> <item_id> [count]` | Gives items directly to a player's inventory (`count` clamped to 1–64). |
| **`summon`** | `summon <mob> [count]` | Spawns mobs (`Zombie`, `Skeleton`, `Spider`, `Creeper`, `Giant`, `Slime`, `Ghast`, `PigZombie`, `Pig`, `Sheep`, `Cow`, `Chicken`). |
| **`say`** | `say <message>` | Broadcasts a server announcement to all players. |
| **`tell`** | `tell <player> <message>` | Whispers a private message to a specific player. |

### Permission & Moderation Lists

The server automatically creates and manages plain-text lists in its working directory:
- **`ops.txt`**: List of server operators (one lowercase username per line).
- **`banned-players.txt`**: Blacklisted player names denied entry.
- **`banned-ips.txt`**: Blacklisted IP addresses rejected at the connection phase.

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

# World dimension: false = Overworld (dimension 0, default), true = Nether (dimension -1).
# Vanilla Alpha SMP servers run either Overworld OR Nether exclusively.
hellworld=false
```

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

## Alpha Protocol Implementation

`alpha_server` implements Minecraft network **Protocol Version 6** (compatible with clients from Alpha v1.2.3_04 through v1.2.6):
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
| **Terrain Generator** | `src/generator.rs`, `density.rs`, `biome.rs`, `caves.rs`, `decorators/` | Bit-exact 1:1 replica of the Alpha world generator: Perlin/simplex noise octaves, biome temperature/humidity fields, cave carvers, trees, lakes, and ores. |
| **Entity System** | `src/entity/` | Entity representations, AABB spatial collisions, pathfinding, mob AI, gravity, fluid drag, health, and combat. |
| **Persistence** | `src/persist.rs` | LevelDB chunk store (`rusty-leveldb`) with configurable write buffers and block cache; gzip-NBT serialization for player files and `level.dat`. |
| **Config & Admin** | `src/server_config.rs`, `server_admin.rs`, `commands.rs` | `server.properties` parser/serializer, ops list, IP/player bans, and interactive console. |

---

## Mechanics & Anti-Cheat Parity

### Mining Physics & Anti-Cheat
In Minecraft Alpha, mining relies on tight synchronization between client and server:
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

## Nether Dimension & Alpha Client Limitations

### Why Dynamic Dimension Switching is Impossible on Unmodded Clients

In vanilla Minecraft Alpha (SMP), running simultaneous multi-world gameplay with dynamic dimension transitions between the Overworld (`dimension = 0`) and the Nether (`dimension = -1`) is **architecturally impossible without client-side modifications**. The original Alpha server by Mojang ran exclusively in one mode at a time—either the Overworld or the Nether—configured via `hellworld=true` in `server.properties`.

Technical constraints within the Alpha client:

1. **Immutable `WorldProvider` per Session**:
   - The client's `World.worldProvider` field is `final` and initialized once when receiving the initial `Packet 1 (0x01, LoginRequest/Response)`.
   - The multiplayer client (`WorldClient`) has no mechanism to swap or reinitialize its `WorldProvider` during an active network session.

2. **Permanent Freeze on Re-sent `Packet 1`**:
   - If the server attempts to switch dimensions by sending `Packet 1` mid-game, the client opens the `GuiDownloadTerrain` screen.
   - However, the client's initial load flag (`field_1210_g`) is set to `true` upon the very first movement packet and is never reset during a session.
   - Because of this, the screen never receives the dismissal trigger, leaving the player permanently stuck on the "Downloading terrain" screen with no way to exit.

3. **Zero-Byte Payload in `Packet 9 (0x09, Respawn)`**:
   - In Alpha, `Packet 9` has a payload length of **0 bytes** (a bare confirmation packet).
   - The dimension parameter in `Packet 9` was only introduced in later Beta versions (Beta 1.3 / Beta 1.6). An unmodded Alpha client cannot receive dimension updates via respawn.

4. **Client Crash on Nether Death (`NullPointerException`)**:
   - When connecting with `dimension = -1`, the client initializes `WorldProviderHell`, where `canRespawnHere()` returns `false`.
   - On death, the client's respawn logic routes through the singleplayer world transition path, which attempts to access a local save directory (`null` in multiplayer), causing a fatal client-side `NullPointerException` and disconnecting with "Connection lost".

5. **Multiplayer Portal Disablement in Client**:
   - In the Alpha client codebase, portal collision teleportation explicitly checks for singleplayer and is disabled in multiplayer. The client never expects to travel through portals in SMP.

---

## Development & Testing

The server comes with a comprehensive suite of unit and integration tests covering protocol serialization, digging physics, redstone circuits, terrain generation, and admin commands.

```bash
# Run unit and integration tests
cargo test

# Check code against Clippy lints
cargo clippy

# Check formatting
cargo fmt --check
```

---

## License

GPLv3 or later, see [LICENSE](LICENSE). Copyright (C) 2026 IQUXAe.

> *Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.*
