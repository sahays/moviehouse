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
cp .env.example .env                                   # add your TMDB API key
echo "MOVIEHOUSE_ACCESS_CODE=$(openssl rand -hex 24)" >> .env   # required — the app won't start without it

# Run (defaults to 127.0.0.1:9000)
moviehouse serve --open

# Network access (Apple TV, phones, tablets)
moviehouse serve --bind 0.0.0.0:9000 --open
```

Or use the local run script (builds, launches in background, opens browser):

```bash
./scripts/run-local.sh
```

## Production Deployment

MovieHouse deploys as a satellite of the bharatsc shared Docker stack, served at
`https://moviehouse.niniconai.com` behind bharatsc's nginx (wildcard TLS). The app
gates itself with an in-app access code (see `docs/superpowers/specs/` — the
access-code-auth design) rather than nginx Basic Auth.

### Prerequisites

- The bharatsc stack running (provides the `shared` network, nginx, and the
  `*.niniconai.com` wildcard cert via niniconai's `scripts/init-ssl.sh`).
- Host directories for storage (defaults): `/srv/moviehouse/data`, `/srv/moviehouse/media`.

### Steps

```bash
cp .env.example .env         # set TMDB keys + MOVIEHOUSE_ACCESS_CODE (openssl rand -hex 24)
./scripts/pre-deploy.sh      # strict fmt/clippy/test/lint gate
./scripts/deploy.sh          # build image, start container, install nginx vhost, reload
```

Flags: `./scripts/deploy.sh --no-build` (skip image build), `--foreground` (run attached).

### Viewing

- **Phone (travelling):** open the site in the browser; HLS/H.264 play directly.
- **Apple TV:** AirPlay from an iPhone/iPad Safari session (tvOS has no browser).
- **LG TV (webOS) browser:** open the site; use the **H.264** transcode (HLS/HEVC are unreliable in that browser).

Ports: 443 (via bharatsc nginx) for the web UI; `6881/tcp+udp` mapped directly for BitTorrent peers.

## CLI Commands

```bash
# Web UI server (default bind 127.0.0.1:9000; --allow-sleep to skip macOS sleep prevention)
moviehouse serve [--bind 0.0.0.0:9000] [--open] [--allow-sleep]

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

See `.env.example` for the full annotated list. The essentials:

```
TMDB_API_KEY=your_api_key_here              # movie/show metadata (optional but recommended)
TMDB_READ_ACCESS_TOKEN=your_token_here      # TMDB v4 read token (optional)
MOVIEHOUSE_ACCESS_CODE=change_me            # REQUIRED — single auth gate, min 16 chars (openssl rand -hex 24)
MOVIEHOUSE_MAX_DOWNLOADS=2                  # max concurrent downloads
```

Deployment-only keys (`DOMAIN`, `WEB_PORT`, `BT_PORT`, `DATA_DIR`, `MEDIA_DIR`,
`MOVIEHOUSE_DOWNLOAD_DIR`, `MOVIEHOUSE_TRANSCODE_DIR`, `BHARATSC_DIR`) are also in
`.env.example`. Values from the process environment take precedence over the file.

### Data locations

Outside Docker, everything lives under `$HOME` (override the media dirs with the
`MOVIEHOUSE_*_DIR` env vars):

```
~/.movies/data/         — sled database (downloads, library, settings)
~/.movies/downloads/    — downloaded media files
~/.movies/transcoded/   — transcoded media files
~/.moviehouse/          — DHT routing table cache (dht_nodes.json)
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

See [`docs/diagrams/`](docs/diagrams/) for per-workflow sequence diagrams (download,
magnet metadata exchange, peer/piece exchange, DHT discovery, transcoding, library +
TMDB, authentication, streaming & progress).

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
cargo test                 # unit + auth tests
./scripts/pre-deploy.sh    # Rust fmt/clippy + React prettier/eslint
```

## License

Personal use.
