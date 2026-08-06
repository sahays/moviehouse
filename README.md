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
- **Continue Watching** — resumes anything started but under 90% complete
- **Already Watched** — finished titles get their own rail, with a `⋯` menu to
  delete the files (source, every transcode, and subtitles) and retire the entry

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
- **Clean Up Sources** — bulk: deletes the original (MKV) for anything already
  transcoded, keeping the MP4 **and** the library entry. Distinct from the
  per-title cleanup on the Already Watched rail, which removes everything

## Quick Start

```bash
# Build and install
cargo install --path .

# Configure
cp .env.example .env    # add your TMDB API key

# Run (defaults to 127.0.0.1:9000)
moviehouse serve --open

# Network access (Apple TV, phones, tablets)
moviehouse serve --bind 0.0.0.0:9000 --open
```

Or use the local run script (builds, launches in background, opens browser):

```bash
./scripts/run-local.sh
```

## Watching on your TV

MovieHouse is local-only: it runs on one machine on your home network and every
other device reaches it over the LAN. Nothing is exposed to the internet, so there
is no domain, no TLS, and no hosting bill.

> **There is no login.** Every device that can reach the port has full control —
> the library, the torrent engine, the filesystem browser, and the file-deleting
> cleanup action. That is the intended tradeoff for a home LAN, but it means you
> should not port-forward this, and anyone on your guest Wi-Fi is an admin. Bind
> to `127.0.0.1` if you only ever watch on the host machine.

Start it bound to all interfaces so other devices can reach it:

```bash
./scripts/run-local.sh 0.0.0.0:9000
```

Then open `http://<your-mac>.local:9000` on the other device (run `hostname -s` to
get the name, or use the Mac's LAN IP).

- **Apple TV:** open the library in **iOS Safari**, start a title, then tap the
  AirPlay button in the player controls and pick the Apple TV. tvOS has no browser,
  so AirPlay is the route. The receiver fetches the stream from its own device, and
  since nothing is gated it just works — nothing to configure.
- **Phone / tablet:** open the site directly; HLS and H.264 play in the browser.
- **LG TV (webOS) browser:** open the site; use the **H.264** transcode (HLS/HEVC are unreliable in that browser).

Two macOS notes:

- **The firewall blocks the binary until you allow it.** macOS prompts on first bind,
  but a background launch (`run-local.sh` uses `nohup`) can miss the prompt — the
  symptom is that `http://127.0.0.1:9000` works while the LAN address times out, even
  though `lsof -nP -iTCP:9000` shows it listening on `*:9000`. Allow it explicitly:

  ```bash
  BIN="$PWD/target/release/moviehouse"
  sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add "$BIN"
  sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp "$BIN"
  ```

  The rule is per-binary. `build.rs` rewrites the binary on every build, so macOS may
  re-prompt (or re-block) after a rebuild — re-run the two commands if the LAN stops
  answering.
- The Mac must stay awake while serving. `moviehouse serve` holds a `caffeinate`
  assertion for its lifetime to prevent idle sleep; pass `--allow-sleep` to opt out.

BitTorrent peers use `6881/tcp+udp`; forward that port on your router only if you
want inbound peer connections.

## CLI Commands

```bash
# Web UI server (default bind 127.0.0.1:9000)
moviehouse serve [--bind 0.0.0.0:9000] [--open]

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
```

Values from the process environment take precedence over the file.

### Data locations

Everything lives under `$HOME` by default. The three `MOVIEHOUSE_*_DIR` overrides
below are read from the **process environment only** — the app does not parse them
out of `.env`, so export them at launch if you keep media on an external drive:

```
~/.movies/data/         — sled database (downloads, library, settings)   [MOVIEHOUSE_DATA_DIR]
~/.movies/downloads/    — downloaded media files                        [MOVIEHOUSE_DOWNLOAD_DIR]
~/.movies/transcoded/   — transcoded media files                        [MOVIEHOUSE_TRANSCODE_DIR]
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
TMDB, streaming & progress).

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
cargo test              # unit + router tests
./scripts/check.sh      # Rust fmt/clippy + React prettier/eslint
```

## License

Personal use.
