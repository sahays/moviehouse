# torrentclient

A high-performance BitTorrent client written in Rust, designed to saturate full network bandwidth.

## Features

- **Core BitTorrent protocol (BEP3)** — peer wire protocol, piece exchange, SHA1 verification
- **DHT (BEP5)** — distributed hash table for trackerless peer discovery
- **Magnet links (BEP9/BEP10)** — download from magnet URIs via metadata exchange
- **PEX (BEP11)** — peer exchange for continuous peer discovery
- **HTTP & UDP trackers (BEP3/BEP15)** — multi-tracker support with automatic failover
- **Rarest-first piece selection** with endgame mode for fast completion
- **Per-block assignment tracking** — no duplicate downloads, instant recovery on peer choke/disconnect
- **Adaptive request pipelining** — pipeline depth scales with per-peer throughput
- **Multi-file torrent support** — handles torrents with any number of files
- **Progress display** — real-time progress bar with speed, peer count, and ETA
- **`--lightspeed` mode** — all performance optimizations enabled (see below)

## Requirements

- Rust 1.85+ (uses edition 2024)
- macOS, Linux, or Windows

## Build

```bash
# Release build (recommended)
cargo build --release

# Install globally
cargo install --path .
```

## Usage

### Download from a .torrent file

```bash
torrentclient download ubuntu.torrent -o ~/Downloads
```

### Download from a magnet link

```bash
torrentclient magnet "magnet:?xt=urn:btih:..." -o ~/Downloads
```

### Inspect a .torrent file

```bash
torrentclient info ubuntu.torrent
```

### Lightspeed mode

```bash
torrentclient download file.torrent -o ~/Downloads --lightspeed
torrentclient magnet "magnet:?xt=..." -o ~/Downloads --lightspeed
```

## Commands

### `download`

```
torrentclient download [OPTIONS] <TORRENT_FILE>
```

| Option | Default | Description |
|---|---|---|
| `-o, --output <DIR>` | `.` | Output directory |
| `-p, --port <PORT>` | `6881` | Listen port |
| `--max-peers <N>` | `80` | Maximum peer connections |
| `--max-download-rate <N>` | `0` | Max download rate bytes/sec (0 = unlimited) |
| `--max-upload-rate <N>` | `0` | Max upload rate bytes/sec (0 = unlimited) |
| `--no-dht` | | Disable DHT |
| `--seed` | | Continue seeding after download |
| `--lightspeed` | | Enable all performance optimizations |
| `-v, --verbose` | | Increase log verbosity (-v, -vv, -vvv) |

### `magnet`

```
torrentclient magnet [OPTIONS] <URI>
```

Same options as `download`. The magnet flow:
1. Parse magnet URI for info_hash and trackers
2. Find peers via DHT and trackers
3. Download metadata from peers (BEP9)
4. Verify metadata hash matches info_hash
5. Start normal piece download

### `info`

```
torrentclient info <TORRENT_FILE>
```

## Lightspeed Mode

`--lightspeed` enables all performance optimizations:

| Optimization | What it does |
|---|---|
| **PEX (Peer Exchange)** | Peers share their peer lists — continuous peer discovery beyond DHT |
| **Persistent DHT** | Saves routing table to `~/.torrentclient/dht_nodes.json` — instant startup on next run |
| **Adaptive pipeline** | Fast peers get up to 256 outstanding requests (baseline: 64) |
| **Active endgame** | Last pieces are requested from all unchoked peers simultaneously |
| **Batched disk sync** | Skips per-piece fsync, syncs on shutdown — reduces I/O overhead |
| **Piece affinity** | Assigns peers to less-contended pieces to reduce duplicate work |
| **Connection reuse** | Magnet downloads reuse peers from metadata phase |

### Benchmarks (7.5 GiB file, ~37 peers)

| Mode | Avg | Median | Peak | Time |
|---|---|---|---|---|
| Normal | 8.7 MiB/s | — | — | 14:45 |
| Lightspeed | 9.6 MiB/s | 11.1 MiB/s | 18.2 MiB/s | 13:20 |

## Architecture

```
                  CLI (clap)
                     |
              TorrentSession
              /      |      \
         Tracker    DHT    PeerManager -- 80 concurrent peers
         (HTTP/UDP) (BEP5)      |
              \      |      /   |
               peer_tx channel  |
                                |
                          PiecePicker    DiskManager
                        (rarest-first)  (async write)
```

- **No Mutex on hot path** — PiecePicker is accessed directly (single-task event loop)
- **Per-block assignment** — `blocks_assigned` bitfield prevents duplicate requests, `unassign_block` on choke/disconnect for instant recovery
- **Per-peer pipeline control** — outstanding request counter with backpressure from channel capacity
- **Honest progress** — only counts verified pieces, not raw bytes

## Protocol Support

| BEP | Name | Status |
|---|---|---|
| BEP3 | The BitTorrent Protocol | Implemented |
| BEP5 | DHT Protocol | Implemented |
| BEP6 | Fast Extension (HaveAll/HaveNone) | Implemented |
| BEP9 | Extension for Peers to Send Metadata | Implemented |
| BEP10 | Extension Protocol | Implemented |
| BEP11 | Peer Exchange (PEX) | Implemented (lightspeed) |
| BEP12 | Multitracker Metadata Extension | Implemented |
| BEP15 | UDP Tracker Protocol | Implemented |

## Tests

```bash
cargo test
```

## License

Personal use.
