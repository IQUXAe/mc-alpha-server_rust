# World Storage Architecture & Durability

This document describes the world storage architecture, format, and durability guarantees used in the Minecraft Alpha 1.2.6 server.

---

## 1. Storage Overview

The server uses an embedded **LevelDB** database (`rusty-leveldb`) for chunk storage, combined with standard Notchian NBT files for level metadata and player inventories.

### 1.1 Directory Layout

A world directory (`<level-name>`, default `world`) contains:
* `db/`: LevelDB database directory storing all chunk blobs.
* `level.dat`: Gzip-compressed NBT compound storing world seed, spawn coordinates, world time, and generator settings.
* `players/`: Directory containing `<username>.dat` files (Gzip-compressed NBT) for player inventories, positions, health, and motion.

---

## 2. Chunk Database Format

### 2.1 Key Encoding
Chunk keys in LevelDB are 8-byte little-endian integers computed from chunk coordinates `(cx, cz)`:
```rust
let key = ((cx as u32 as u64) << 32) | (cz as u32 as u64);
let bytes = key.to_le_bytes();
```

### 2.2 Value Compression & Compatibility
Chunk values are NBT compounds containing blocks, metadata, skylight, blocklight, tile entities, and staged creatures.
* **Modern Chunk Writes**: Chunks are compressed using **GZIP** (or **ZSTD**).
* **Automatic Decompression**: The storage reader detects ZSTD frames via the magic header `0xFD2FB528` and decompresses them with `zstd`, otherwise falling back to standard Gzip decompression. This ensures seamless interoperability with modern optimized chunk blobs and legacy backups.

---

## 3. Durability & Atomic Persistence

To prevent world corruption on sudden server termination, system crashes, or power loss:
1. **Atomic File Writes**: `level.dat` and player `.dat` files are written to a temporary file (`.dat_tmp`), flushed to physical disk via `File::sync_all()`, and atomically renamed to the destination file. The parent directory is also synchronized to guarantee metadata persistence.
2. **LevelDB Flushing**: All pending chunk writes, staged unloads, and the LevelDB memtable/WAL are flushed to disk during world autosaves and upon graceful server shutdown (`shutdown`).
3. **Spatial TileEntity Indexing**: Chunk-based indexing (`WorldTiles`) allows saving and streaming chunks without iterating over the entire world's tile entity table.
