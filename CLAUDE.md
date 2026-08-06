# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

MovieHouse is a self-hosted media library + BitTorrent download manager + transcoder, shipped as a **single Rust binary** with an embedded React SPA. No cloud, no external services beyond TMDB for metadata.

## Commands

```bash
cargo run -- serve --bind 0.0.0.0:9000    # run the web server (dev)
cargo run -- info file.torrent            # inspect a .torrent
cargo build --release                     # release build
cargo test                                # unit + router tests
cargo test <name>                         # single test by substring match
./scripts/run-local.sh [bind]             # release build → background launch → open browser
./scripts/check.sh                        # strict CI gate (see below)
cd frontend && npm run lint               # eslint; npx prettier --check src for formatting
```

`check.sh` is verify-only (nothing auto-fixed): `cargo fmt --check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo test`, `cargo check`, `prettier --check`, `eslint`. All must pass. Run your formatters during development, not through this script.

## Build system gotchas

- **`build.rs` does two things on every compile**: (1) runs `npm install` + `npm run build` in `frontend/` if `frontend/package.json` exists, embedding `frontend/dist/` into the binary via `rust-embed`; (2) **auto-increments the patch version in `Cargo.toml`**.
- Because the version churns every build, **never use `cargo --locked`** — it desyncs `Cargo.lock` and fails. Dependencies are still pinned by the committed lockfile; only this crate's own version bumps. This is why `check.sh` omits `--locked`.
- Lints are strict: `unsafe_code = "forbid"`, clippy `pedantic` warns, and `unwrap_used`/`expect_used`/`todo`/`dbg_macro` are warnings promoted to errors under `-D warnings`. Write `unwrap`/`expect`-free code — use `let ... else`, `?`, and fail-closed defaults (see `sign` in `src/transcode/` and the `let ... else` chains in `src/web/api/` for the pattern).

## Architecture

Per-workflow sequence diagrams (traced from the code, with `file:line` anchors) live in [`docs/diagrams/`](docs/diagrams/) — download, magnet metadata exchange, peer/piece exchange, DHT discovery, transcoding, library + TMDB, and streaming/progress. Read them for the runtime call flow; the summary below is the static module map.

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
- `cleanup.rs` — `owned_files(entry)`: which files a library entry owns (media file, transcoded versions, sidecar subtitles). Pure and tested; the unlink lives in `web/api/library.rs` next to the `paths::confine` guard. **Never includes `original_path`** — that is the shared source *directory*, so deleting it would take the rest of a season with it.

`transcode/` — concurrent ffmpeg runner (`runner.rs`) processing jobs; two built-in presets (`presets.rs`): `hevc` (remux MKV→MP4, hvc1 tag, no re-encode) and `h264` (universal re-encode). Progress persists across restarts.

`web/` — the `serve` command's HTTP layer (axum 0.8):
- `server.rs` — `AppState` (manager, store, transcode handle, max_downloads) + `create_router`. Also serves the embedded SPA (`FrontendAssets` from `frontend/dist`, SPA-fallback to `index.html`) — but an unmatched `/api/*` path returns a 404 JSON body rather than falling through to the SPA, so a stale client cannot read the index page as success.
- `api/` — REST handlers under `/api/v1/*` (torrents, library, media, transcode, settings, filesystem). **No auth middleware — every route is open.**
- **There is no authentication.** The access-code login, session cookies, and playback tokens were all removed: this binds to a home LAN and trusts everything that can reach it. Consequences to keep in mind when changing anything here: any device on the network can drive the torrent engine, browse the filesystem within `api/paths.rs`'s allowed roots, rewrite settings, and delete media via `DELETE /api/v1/library/{id}?delete_files=true`. `api/paths.rs` is therefore the *only* remaining trust boundary — treat it accordingly. Do not add a route that widens what an unauthenticated caller can reach on disk.
- `security_headers.rs` — **what nginx used to do.** The app once ran behind a reverse proxy that added a CSP and security headers; local-only serves axum straight to the LAN, so they live in-process now. No HSTS (plain HTTP on the LAN).
- `ws.rs` — WebSocket at `/api/v1/ws` for live download progress.
- `api/paths.rs` — **security-critical**: confines caller-supplied filesystem paths (download/scan/transcode roots, folder browser) to allowed roots (home + `/media`, `/mnt`, `/Volumes`, `/data`, `/srv`). Reject `..`, canonicalize before comparing. Route any new endpoint that takes a path through this.

`tmdb.rs` — metadata client (posters, cast, per-episode synopses). `main.rs` wires everything in `cmd_serve`; the CLI (`cli.rs`) also exposes headless `download`/`magnet`/`info` subcommands that bypass the web layer.

Request flow: axum route → `api/*` handler → `SessionManager`/`Store`/`TranscodeHandle` on `AppState` → engine primitives. Keep handlers thin; put logic in `engine/`.

## Code conventions

Beyond the global rules (DRY, composition, layered altitude, ≤15–20-line functions, ≤300-line files, OWASP), this repo has established idioms — match them:

- **`unwrap`/`expect`-free, fail-closed.** These lints are denied. Use `let ... else`, `?`, and safe defaults. When a "can't happen" branch is unavoidable, handle it explicitly and add a comment explaining why the fallback is safe (see `remove_owned_files` in `web/api/library.rs` skipping — never deleting — a path it cannot confine).
- **Document the *why*, especially for security/crypto decisions.** Module tops carry `//!` doc comments stating purpose and threat model (`api/paths.rs`, `security_headers.rs`, `engine/cleanup.rs`); non-obvious tradeoffs (cast safety, `--locked` omission, why `original_path` is never deleted) get an inline rationale. Keep this up when you change the reasoning.
- **Structured logging via `tracing`.** Identifiers are fields, not interpolated strings: `tracing::warn!(media_id = %entry.id, path = %path.display(), "refusing to delete ...")`. Pick the right level.
- **Thin handlers, logic in `engine/`.** `api/*` handlers translate HTTP ↔ engine calls on `AppState`; they don't hold business logic.
- **Validate at the trust boundary.** Any endpoint accepting a caller-supplied path goes through `api/paths.rs` (`confine`/`validate_config_dir`). With no auth layer left, that confinement is the last line of defence — see the "no authentication" note under Architecture.
- **Module layout.** Each subsystem is a directory with a `mod.rs` that declares and re-exports its public surface; tests live in `#[cfg(test)]` modules in the same file (see `engine/cleanup.rs`, `web/api/paths.rs`).

## Frontend

`frontend/` — React 19 + Vite 8 + Tailwind v4 + shadcn/base-ui. Entry `App.tsx`; feature components in `components/`, shadcn primitives in `components/ui/`, helpers in `lib/`, WebSocket/theme/library hooks in `hooks/`. Components call `fetch` directly — there is no API client wrapper, since with no auth there are no 401s to intercept. Dev server proxies `/api` → `localhost:3000` (`vite.config.ts`), but in production the built assets are embedded in the Rust binary — there is no separate frontend server.

## Running locally

**Local-only by design.** MovieHouse binds to the LAN and is reached over plain HTTP from other devices in the house (`http://<host>.local:9000`) — no domain, no TLS, no container, nothing exposed to the internet. It was briefly packaged for remote Docker deployment; that was removed because compute and egress cost money for a library only watched at home. If you are tempted to re-add a reverse proxy, note that `security_headers.rs` assumes nothing sits in front, and that **nothing authenticates callers** — putting this on a public address exposes the whole API.

The typical device path is iOS Safari → AirPlay → Apple TV; with nothing gated, the receiver just fetches the stream URL directly. `serve` holds a macOS `caffeinate` assertion so idle sleep cannot drop a stream mid-film; `--allow-sleep` opts out.

Data locations: `~/.movies/data/` (sled), `~/.movies/downloads/`, `~/.movies/transcoded/`, `~/.moviehouse/` (DHT cache). The first three take `MOVIEHOUSE_{DATA,DOWNLOAD,TRANSCODE}_DIR` overrides read from the **process environment only** — `Config::load` parses `.env` for the TMDB keys and max-downloads, not for these.
