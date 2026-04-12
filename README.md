# MovieHouse

Self-hosted media library and download manager. Download movies and TV shows via BitTorrent, transcode for any device, browse with a web UI, and stream to your TV.

Single binary. No cloud. Your media, your network.

## Features

### Media Library
- **TMDB integration** — poster art, cast, director, ratings, per-episode synopses
- **TV show support** — seasons, episodes, grouped library with drill-down navigation
- **Movies and shows** — separate sections, each with full metadata cards
- **Folder scanning** — import existing media files with recursive directory search

### Web UI
- **React frontend** — embedded in the binary, no separate server
- **Mobile-first** — bottom nav on mobile, persistent sidebar on desktop
- **Light/dark mode** — toggle in sidebar, persists in localStorage
- **shadcn/ui** — accessible components with Tailwind CSS
- **Real-time updates** — WebSocket for download progress, 3-second polling for library

### Transcoding
- **HEVC MP4** (default) — remux MKV to MP4 with hvc1 tag, seconds, no quality loss
- **H.264 MP4** (fallback) — re-encode for universal compatibility
- **Concurrent runner** — configurable parallelism (default: 2 jobs)
- **Batch transcode** — per-season "Transcode Season" button
- **Stop/cancel** — per-job and per-season cancellation
- **Progress persistence** — survives server restarts

### Video Playback
- **Click to play** — click poster to stream in browser
- **HTTP range requests** — seeking, pause/resume
- **Works everywhere** — Safari, Chrome (Mac/Android), Edge
- **AirPlay** — stream to Apple TV from any Apple device

### BitTorrent Engine
- **DHT, magnet links, PEX** — full peer discovery
- **Endgame mode** — fast completion of last pieces
- **Lightspeed mode** — adaptive pipelining, persistent DHT, PEX
- **Security hardened** — path traversal protection, bencode limits, input validation

### Settings
- **Persistent** — sled database with JSON serialization
- **Download folder** — configurable with server-side folder browser
- **Auto-transcode** — toggle + default encoding (HEVC/H.264)
- **TMDB API key** — loaded from `.env` file

## Quick Start

```bash
# Build and install
cargo install --path .

# Configure
cp .env.example .env    # Add your TMDB API key

# Run
moviehouse serve --open

# Network access (Apple TV, phones, tablets)
moviehouse serve --bind 0.0.0.0:3000 --open
```

Or use the install script (builds, launches in background, opens browser):

```bash
./install.sh
```

## CLI Commands

```bash
# Web UI server
moviehouse serve [--bind 0.0.0.0:3000] [--open]

# Download from .torrent file
moviehouse download ubuntu.torrent -o ~/Downloads [--lightspeed]

# Download from magnet link
moviehouse magnet "magnet:?xt=urn:btih:..." -o ~/Downloads

# Inspect a .torrent file
moviehouse info ubuntu.torrent
```

## Requirements

- **Rust** 2024 edition (build-time)
- **Node.js** (build-time, for frontend compilation)
- **FFmpeg** (optional, for transcoding)
- **TMDB API key** (free, for movie/show metadata — [get one here](https://www.themoviedb.org/settings/api))

## Configuration

### `.env` file

```
TMDB_API_KEY=your_api_key_here
```

### Data locations

```
~/.movies/data/         — sled database
~/.movies/transcoded/   — transcoded media files
~/.moviehouse/          — DHT routing table cache
```

## Architecture

```
moviehouse serve
├── axum web server (REST API + WebSocket + embedded React SPA)
├── BitTorrent engine (DHT, trackers, peer wire protocol)
├── Transcode runner (concurrent ffmpeg jobs)
├── sled persistence (downloads, library, settings)
└── TMDB client (movie/show metadata)
```

## Protocol Support

| BEP | Name | Status |
|-----|------|--------|
| 3 | BitTorrent Protocol | Implemented |
| 5 | DHT Protocol | Implemented |
| 6 | Fast Extension | Implemented |
| 9 | Metadata Exchange | Implemented |
| 10 | Extension Protocol | Implemented |
| 11 | Peer Exchange (PEX) | Implemented |
| 12 | Multi-tracker | Implemented |
| 15 | UDP Tracker | Implemented |

## Tests

```bash
cargo test          # 69 unit tests
./pre-deploy.sh     # Rust fmt/clippy + React prettier/eslint
```

## License

Personal use.
