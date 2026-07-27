# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

MovieHouse is a self-hosted media library + BitTorrent download manager + transcoder, shipped as a **single Rust binary** with an embedded React SPA. No cloud, no external services beyond TMDB for metadata.

## Commands

```bash
cargo run -- serve --bind 0.0.0.0:9000    # run the web server (dev)
cargo run -- info file.torrent            # inspect a .torrent
cargo build --release                     # release build
cargo test                                # unit + auth + router tests
cargo test <name>                         # single test by substring match
./scripts/run-local.sh [bind]             # release build → background launch → open browser
./scripts/pre-deploy.sh                   # strict CI gate (see below); run before deploy
./scripts/deploy.sh [--no-build] [-f]     # build image, start container, install nginx vhost
cd frontend && npm run lint               # eslint; npx prettier --check src for formatting
```

The web server **refuses to start** without `MOVIEHOUSE_ACCESS_CODE` set (min 16 chars) — copy `.env.example` to `.env` and set it (`openssl rand -hex 24`). `run-local.sh` fails fast with a clear message if it's missing.

`pre-deploy.sh` is verify-only (nothing auto-fixed): `cargo fmt --check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo test`, `cargo check`, `prettier --check`, `eslint`. All must pass. Run your formatters during development, not through this script.

## Build system gotchas

- **`build.rs` does two things on every compile**: (1) runs `npm install` + `npm run build` in `frontend/` if `frontend/package.json` exists, embedding `frontend/dist/` into the binary via `rust-embed`; (2) **auto-increments the patch version in `Cargo.toml`**.
- Because the version churns every build, **never use `cargo --locked`** — it desyncs `Cargo.lock` and fails. Dependencies are still pinned by the committed lockfile; only this crate's own version bumps. This is why `pre-deploy.sh` omits `--locked`.
- Lints are strict: `unsafe_code = "forbid"`, clippy `pedantic` warns, and `unwrap_used`/`expect_used`/`todo`/`dbg_macro` are warnings promoted to errors under `-D warnings`. Write `unwrap`/`expect`-free code — use `let ... else`, `?`, and fail-closed defaults (see `src/web/auth.rs` `sign` for the pattern).

## Architecture

Per-workflow sequence diagrams (traced from the code, with `file:line` anchors) live in [`docs/diagrams/`](docs/diagrams/) — download, magnet metadata exchange, peer/piece exchange, DHT discovery, transcoding, library + TMDB, authentication, and streaming/progress. Read them for the runtime call flow; the summary below is the static module map.

The BitTorrent stack is built bottom-up; each layer depends only on those below it:

```
bencode/   → torrent/  → tracker/ + dht/  → peer/  → piece/  → disk/
(wire fmt)   (.torrent    (peer discovery:   (wire   (piece   (file
             + magnet)     HTTP/UDP/DHT)      proto)  picker)   I/O + mapping)
```

`engine/` ties the stack into running downloads:
- `session.rs` — one `TorrentSession` per download: drives peer manager, piece picker, tracker/DHT, choker.
- `manager.rs` — `SessionManager` owns all sessions in a `DashMap<Uuid, SessionHandle>`, broadcasts `SessionEvent`s, enforces `max_downloads`.
- `store.rs` — **sled** persistence (downloads, library, settings) via bincode/JSON.
- `library.rs` — scanned/downloaded media grouped into movies vs. shows/seasons/episodes.

`transcode/` — concurrent ffmpeg runner (`runner.rs`) processing jobs; two built-in presets (`presets.rs`): `hevc` (remux MKV→MP4, hvc1 tag, no re-encode) and `h264` (universal re-encode). Progress persists across restarts.

`web/` — the `serve` command's HTTP layer (axum 0.8):
- `server.rs` — `AppState` (manager, store, transcode handle, access_code, max_downloads) + `create_router`. Also serves the embedded SPA (`FrontendAssets` from `frontend/dist`, SPA-fallback to `index.html`).
- `api/` — REST handlers under `/api/v1/*` (torrents, library, media, transcode, settings, filesystem). All routes sit behind an auth middleware: `require_auth` (cookie only) for everything except the three media byte-serving routes, which use `require_media_auth`.
- `auth.rs` — **stateless HMAC-SHA256 session cookie** (`mh_session`). The access code is BOTH the login gate AND the HMAC key, so rotating it invalidates every session at once. Public routes: `/api/v1/auth/{login,status,logout}` and `/health`. 30-day TTL, `HttpOnly`. Also mints **playback tokens** — 12-hour capabilities scoped to one media id, signed over `"playback:{id}:{exp}"` (domain-separated from session tokens, which sign the bare `exp`). They exist because an AirPlay/Chromecast receiver fetches the media URL from its own device and has no cookie; `require_media_auth` accepts cookie **or** token on `/stream`, `/segment/{filename}`, `/subtitles/{index}` only, so a leaked playback URL is read-only and single-title.
- `ws.rs` — WebSocket at `/api/v1/ws` for live download progress.
- `api/paths.rs` — **security-critical**: confines caller-supplied filesystem paths (download/scan/transcode roots, folder browser) to allowed roots (home + `/media`, `/mnt`, `/Volumes`, `/data`, `/srv`). Reject `..`, canonicalize before comparing. Route any new endpoint that takes a path through this.

`tmdb.rs` — metadata client (posters, cast, per-episode synopses). `main.rs` wires everything in `cmd_serve`; the CLI (`cli.rs`) also exposes headless `download`/`magnet`/`info` subcommands that bypass the web layer.

Request flow: axum route → `api/*` handler → `SessionManager`/`Store`/`TranscodeHandle` on `AppState` → engine primitives. Keep handlers thin; put logic in `engine/`.

## Code conventions

Beyond the global rules (DRY, composition, layered altitude, ≤15–20-line functions, ≤300-line files, OWASP), this repo has established idioms — match them:

- **`unwrap`/`expect`-free, fail-closed.** These lints are denied. Use `let ... else`, `?`, and safe defaults. When a "can't happen" branch is unavoidable, handle it explicitly and add a comment explaining why the fallback is safe (see `sign` in `src/web/auth.rs` returning `""`, which never verifies).
- **Document the *why*, especially for security/crypto decisions.** Module tops carry `//!` doc comments stating purpose and threat model (`api/paths.rs`, `auth.rs`); non-obvious tradeoffs (session TTL, cast safety, `--locked` omission) get an inline rationale. Keep this up when you change the reasoning.
- **Structured logging via `tracing`.** Identifiers are fields, not interpolated strings: `tracing::warn!(ip = %ip, "access-code login failed")`. Pick the right level.
- **Thin handlers, logic in `engine/`.** `api/*` handlers translate HTTP ↔ engine calls on `AppState`; they don't hold business logic.
- **Validate at the trust boundary.** Any endpoint accepting a caller-supplied path goes through `api/paths.rs` (`confine`/`validate_config_dir`); any caller-supplied secret comparison uses the constant-time helpers in `auth.rs` (`subtle::ConstantTimeEq`).
- **Module layout.** Each subsystem is a directory with a `mod.rs` that declares and re-exports its public surface; tests live in `#[cfg(test)]` modules in the same file (see `auth.rs`).

## Frontend

`frontend/` — React 19 + Vite 8 + Tailwind v4 + shadcn/base-ui. Entry `App.tsx`; feature components in `components/`, shadcn primitives in `components/ui/`, API client in `lib/api.ts`, WebSocket/theme/library hooks in `hooks/`. Dev server proxies `/api` → `localhost:3000` (`vite.config.ts`), but in production the built assets are embedded in the Rust binary — there is no separate frontend server.

## Deployment

Deploys as a **satellite of the bharatsc shared Docker stack** at `https://moviehouse.niniconai.com`, behind bharatsc's nginx (wildcard TLS) on the `shared` Docker network. Requires the bharatsc stack running and host dirs `/srv/moviehouse/{data,media}`. `deploy.sh` sources `.env`, runs `check-deps.sh`, builds the image, and installs the nginx vhost from `nginx/conf.d/moviehouse.conf.template`. Port 443 (via nginx) for the UI; `6881/tcp+udp` mapped directly for BitTorrent peers. Design docs live in `docs/superpowers/specs/`.

Data locations (non-Docker): `~/.movies/data/` (sled), `~/.movies/transcoded/`, `~/.moviehouse/` (DHT cache).
