# alpha_server

[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Protocol](https://img.shields.io/badge/Minecraft%20Alpha-v1.2.3__04%20--%20v1.2.6%20(Protocol%206)-brightgreen.svg)](#alpha-protocol-implementation)
[![Target](https://img.shields.io/badge/Target-Linux%20%7C%20musl%20%7C%20OpenWrt-lightgrey.svg)](#memory-usage--embedded-deployment)

A custom Minecraft Alpha (v1.2.3_04 through v1.2.6 / Protocol Version 6) server written in Rust. Built to run on low-resource hardware and embedded OpenWrt routers in ~15–18 MB of RAM. Compatible with vanilla clients, Prism Launcher, Betacraft, and MultiMC.

---

## Features

- **Protocol 6 Compatibility**: Supports unmodded Alpha v1.2.3_04 through v1.2.6 clients.
- **Low Memory Footprint**: Runs in ~15–18 MB RSS with an active player at view distance 6, compared to 500+ MB on the vanilla JVM server.
- **Vanilla World Generation**: 1:1 reproduction of Alpha terrain noise, biomes, cave carvers, trees, lakes, and ore distribution.
- **World Mechanics & Physics**: Redstone signals (0–15), torch burnout, interactive switches/doors, and fluid flow with obsidian/cobblestone formation.
- **Tile Entities**: Smelting in furnaces, single and double chests, signs, and mob spawners.
- **Mining & Anti-Cheat**: Accurate break stages and break particles with server-side work validation against instant-break and speed-mining hacks.
- **Persistence**: LevelDB chunk storage via `rusty-leveldb` and gzip-NBT for `level.dat` and player data.
- **Standalone Binary**: Compiles to a single static musl binary with zero JVM dependencies, suitable for OpenWrt and low-end Linux boxes.
- **Vanilla Dimension Mode**: Runs either Overworld or Nether via `hellworld=true/false` in `server.properties`, matching original Alpha SMP behavior.
- **Mob Roster & SMP Projectiles**: Tracks standard Alpha mobs (Zombies, Skeletons, Spiders, Creepers, Slimes, Ghasts, PigZombies, Giants) and adds server tracking for arrows, snowballs, fishing hooks, minecarts, and primed TNT that vanilla SMP skipped.
- **Tick Budgets**: 64-slot background gen queue, 15 chunks/tick/player stream limit, and an 8192-cell explosion cap to keep the 50 ms tick stable.
- **Optional GPU Pre-generation**: Experimental WGSL compute pipeline (`--features gpu`) offloading 3D density field interpolation to Vulkan, OpenGL ES 3.1+, DX12, or Metal.

---

## Quick Start

### Requirements
- Stable Rust 1.75+.

### Build
```bash
git clone https://github.com/IQUXAe/mc-alpha-server_rust.git
cd mc-alpha-server_rust
cargo build --release
```
The compiled binary will be located at `target/release/alpha_server`.

### Run
```bash
./target/release/alpha_server [server.properties] [world-dir] [port]
```
All command-line arguments are optional:
- `server.properties`: Defaults to `./server.properties`.
- `world-dir`: Defaults to `world/<level-name>`.
- `port`: Overrides `server-port` from properties (default: `25565`).

### Connecting
1. Launch Minecraft Alpha (v1.2.6 recommended, or v1.2.3_04–v1.2.5) via Prism Launcher, Betacraft, MultiMC, or the official launcher.
2. Connect to `localhost:25565` (or your server's IP).
3. For offline/LAN play without authentication, set `online-mode=false` in `server.properties`.

---

## Configuration (`server.properties`)

```properties
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

# View distance radius in chunks:
# 6  -> 13x13 (169 chunks) -> ~15–18 MB RAM (recommended for routers)
# 8  -> 17x17 (289 chunks) -> ~25–28 MB RAM
# 10 -> 21x21 (441 chunks) -> ~35–45 MB RAM (vanilla default)
view-distance=6

# Set true for Nether (dimension -1), false for Overworld (dimension 0)
hellworld=false

# GPU terrain acceleration (requires build with --features gpu)
gpu-acceleration=false
gpu-backend=auto
```


---

## Memory Usage & Embedded Deployment

Chunk columns occupy 80 KB of uncompressed memory per column (32 KB blocks, 16 KB metadata, 16 KB blocklight, 16 KB skylight, 256 B heightmap). Compressed chunk packets are dropped immediately after streaming to prevent memory ballooning.

### Memory Profiles (RSS)

| Profile | Loaded Chunks | Resident Memory (RSS) | Target |
| :--- | :---: | :---: | :--- |
| Cold start (0 players) | 0 | ~3–5 MB | Daemon idle |
| Idle (spawn loaded) | 49 (7×7) | ~6–8 MB | Ready for joins |
| 1 Player (`view-distance=6`) | 169 (13×13) | **~15–18 MB** | 128 MB RAM routers |
| 1 Player (`view-distance=8`) | 289 (17×17) | ~25–28 MB | Low-spec SBCs |
| 1 Player (`view-distance=10`) | 441 (21×21) | ~35–42 MB | Standard desktop |

### Static Musl Build for Routers

Cross-compile a statically linked binary without glibc dependencies:
```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```
*(For ARM or MIPS routers, target `aarch64-unknown-linux-musl`, `armv7-unknown-linux-musleabihf`, or `mips-unknown-linux-musl`)*.

On devices with tight memory constraints, tune LevelDB write buffers and block cache via environment variables before launching:
```bash
export ALPHA_LEVELDB_WRITE_BUFFER_KB=256
export ALPHA_LEVELDB_CACHE_KB=256
./alpha_server
```


---

## Console Commands

The server console accepts standard commands at runtime:

| Command | Syntax | Description |
| :--- | :--- | :--- |
| `help` / `?` | `help` | Lists available commands |
| `list` | `list` | Displays connected players |
| `stop` | `stop` | Flushes data and shuts down the server |
| `save-all` | `save-all` | Forces immediate save of chunks and player state |
| `save-off` / `save-on` | `save-off` / `save-on` | Pauses or resumes background world saves |
| `op` / `deop` | `op <player>` / `deop <player>` | Grants or revokes operator privileges in `ops.txt` |
| `ban` / `pardon` | `ban <player>` / `pardon <player>` | Manages player blacklist in `banned-players.txt` |
| `ban-ip` / `pardon-ip`| `ban-ip <ip>` / `pardon-ip <ip>` | Manages IP blacklist in `banned-ips.txt` |
| `kick` | `kick <player>` | Disconnects an active player |
| `tp` | `tp <player> [target]` | Teleports player to coordinates or another player |
| `give` | `give <player> <item_id> [count]` | Gives items directly to inventory |
| `summon` | `summon <mob> [count]` | Spawns entities and mobs |
| `say` | `say <message>` | Broadcasts a server announcement |
| `tell` | `tell <player> <message>` | Whispers a private message |
| `boost` | `boost [on\|off\|status]` | Toggles or checks GPU acceleration state |

---

## Alpha Protocol Notes

### Network Framing (Protocol 6)
- **Handshake & Login**: `0x02` (Handshake) and `0x01` (LoginRequest) with Mojang/Betacraft authentication or offline fallback.
- **Chunk Streaming**: Allocation via `Packet 50 (0x32, PreChunk)` followed by zlib-compressed payloads via `Packet 51 (0x33, MapChunk)`.
- **Block Digging (`Packet 14, 0x0E`)**:
  - `status = 0`: Start digging (immediate harvest for 0.0 hardness blocks like torches and redstone).
  - `status = 1`: Progressive mining damage tick.
  - `status = 2`: Cancel digging.
  - `status = 3`: Client completion signal once crack stage reaches 100%.
- **Anti-Cheat Validation**: The server validates that adequate digging work was recorded (`cur_damage >= 0.70`, or `ground_damage >= 0.70` for airborne/underwater states) before accepting block break completion. Instant break and speed-mining cheats are rejected and rolled back with `Packet 53 (0x35, BlockChange)`. Blocks are never harvested purely in the background without client packets, preventing ghost breaks.

---

## Architecture

```
src/
├── server/          # 20 TPS tick loop, chunk streaming queues, LevelDB worker
├── session/         # Packet encoders/decoders, login flow, digging and combat logic
├── world/           # Chunk map, lighting updates, liquid physics, block ticks
├── chunk.rs         # 16x128x16 column layout, heightmap, zlib compression
├── generator.rs     # Alpha noise octaves, biomes, cave carving, decorators
├── entity/          # Spatial AABB collisions, mob AI, projectile tracking
├── persist.rs       # LevelDB backend, gzip-NBT for player files and level.dat
├── gpu/             # Optional WGSL compute pipeline for batch terrain pre-gen
└── server_config.rs # server.properties parser, permissions, bans
```

---

## Nether Dimension & Alpha Client Limitations

Dynamic dimension switching between the Overworld (`dimension = 0`) and the Nether (`dimension = -1`) is impossible on unmodded Alpha clients due to client engine limitations:

1. **Immutable `WorldProvider`**: `World.worldProvider` is `final` and initialized once when receiving `Packet 1 (LoginRequest)`. `WorldClient` cannot reinitialize or swap providers mid-session.
2. **Permanent Freeze on Re-sent `Packet 1`**: Sending `Packet 1` mid-game opens `GuiDownloadTerrain`, but `field_1210_g` is set to `true` upon first movement and never reset, leaving the client stuck on the "Downloading terrain" screen indefinitely.
3. **Zero-Byte `Packet 9` (Respawn)**: In Alpha, `Packet 9` has a 0-byte payload. Dimension parameters in respawn packets were only introduced in Beta 1.3+.
4. **Crash on Nether Death**: In the Nether, `WorldProviderHell.canRespawnHere()` returns `false`. On death, the client calls singleplayer world transition code, attempts to read a local save folder (`null` in SMP), and crashes with a `NullPointerException`.
5. **Disabled Portals**: Portal collision logic is strictly hardcoded for singleplayer in the Alpha client.

To run a Nether world, set `hellworld=true` in `server.properties`, matching original Alpha server behavior.

---

## GPU Acceleration (`--features gpu`)

An optional WGSL compute pipeline offloads 3D terrain density interpolation for batch pre-generation using `wgpu`:

```bash
cargo build --release --features gpu
```

> **Note**: GPU acceleration is experimental and intended primarily for batch pre-generation:
> - **Driver Overhead**: Graphics runtimes add memory overhead (e.g., proprietary NVIDIA drivers add ~150 MB+ RSS on Linux).
> - **Bus Latency**: For real-time single-chunk generation or on low-end integrated graphics, PCIe transfer latency makes it slower than the CPU generator.
> - **Automatic Fallback**: If compute shaders are unsupported or driver initialization fails, the server falls back to CPU generation automatically.

Configure in `server.properties`:
```properties
gpu-acceleration=true
gpu-backend=auto # auto, vulkan, gles, dx12, metal
```
Use `boost on` / `boost off` in the server console to toggle at runtime.

---

## Development

```bash
cargo test
cargo clippy --all-targets
cargo fmt --check
```


---

## License

GPL-3.0 or later. See [LICENSE](LICENSE). Copyright (C) 2026 IQUXAe.

*Not an official Minecraft product. Not approved by or associated with Mojang or Microsoft.*
