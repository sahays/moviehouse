# MovieHouse Security Audit — 2026-07-26

- **Date:** 2026-07-26
- **Reviewed commit:** `f0b9d4b` (f0b9d4bec0a53c7a5d1c0ff0b278027472506e3d), branch `main`
- **Status:** findings open (remediation not yet applied)

Pre-public-exposure review across four dimensions (path traversal, command/argument
injection, auth/session/web-layer, BitTorrent parsing/SSRF/DoS). Each finding was
traced in the actual code; the bencode overflow was confirmed with a reverted PoC.

**Context:** app is exposed at `moviehouse.niniconai.com` behind a single shared
access code, with the BitTorrent port 6881 (tcp+udp) directly on the internet.

**Key mitigating fact:** the Docker image runs as **non-root `app`**, so file-write
primitives are confined to `/data` and `/media` in the container — bad, but not root
RCE. Running the raw binary as root (systemd) removes this mitigation.

## Verdict: DO NOT expose publicly until CRITICAL + HIGH are fixed.

Two root causes drive most findings:
- **(A) Unconfined request-body paths** — `put_settings`, `scan_folder`, `migrate_media`
  accept arbitrary absolute paths. One shared helper (`canonicalize` + `starts_with(root)`,
  already implemented correctly in `filesystem.rs`) fixes the cluster.
- **(B) Untrusted torrent-derived data reaching sinks** — subtitle metadata → ffmpeg path,
  tracker URLs → SSRF, bencode length → overflow panic.

---

## CRITICAL

### C1 — Bencode integer-overflow panic → permanent DHT kill (unauthenticated)
`src/bencode/decode.rs:185,189`. `self.pos + length` is unchecked (only `length` itself is
overflow-guarded). A 22-byte input `18446744073709551615:` panics the decoder in both debug
and release. The DHT `recv_loop` (`src/dht/krpc.rs:145`, single task from `dht/node.rs:80`)
decodes every UDP packet inline with no panic isolation → **one unauthenticated UDP datagram
to :6881 permanently disables DHT** (magnet peer discovery) for the process lifetime.
Also crashes `.torrent` upload parsing and per-peer connections (those are task-isolated).
**Fix:** `self.pos.checked_add(length).ok_or(...)?`; wrap DHT per-packet dispatch in
`catch_unwind`. (2-line root fix.)

---

## HIGH

### H1 — Arbitrary file write: `put_settings(download_dir)` + crafted torrent
`src/web/api/settings.rs:26-45` stores caller-supplied `download_dir`/`transcode_dir`/
`media_scan_dir` with no validation; `torrents.rs:210` uses `download_dir` as the torrent
output root. The per-torrent Zip-Slip protection is correct but moot once the *base* is
attacker-chosen: `PUT /api/v1/settings {"download_dir":"/etc"}` then a crafted torrent writes
attacker content to `/etc/cron.d/...`. Root RCE if run as root; in Docker, confined to `app`'s
writable trees (still can corrupt the DB/media). **Fix:** confine these dirs to an allowlisted
base in `put_settings`.

### H2 — Arbitrary file write via crafted-torrent subtitle `language` metadata
`src/transcode/ffmpeg.rs:199-226` (`extract_subtitles`) builds the `.vtt` output path from
`stream.language` (read unsanitized from container metadata, `probe.rs:73`). A torrent whose
subtitle stream language is `../../../../etc/cron.d/evil` writes there when the media auto-
transcodes (`auto_transcode` default true). **Triggered by merely seeding a malicious torrent —
no settings change, no auth interaction.** **Fix:** sanitize `language` to `[A-Za-z0-9_-]`
(reuse `sanitize_filename`), and `file:`-prefix ffmpeg output args.

### H3 — `scan_folder` enumerates/reads any directory on the host
`src/web/api/library.rs:405-420`. `POST /api/v1/library/scan {"path":"/"}` recursively walks
the whole filesystem and imports every video file (any user's media under `/home/*`, etc.);
imported entries are then streamable and auto-transcoded → arbitrary video-file read/exfil.
**Fix:** confine `req.path` under an allowlisted root (`canonicalize` + `starts_with`).

### H4 — SSRF via unvalidated tracker URLs
`src/tracker/http.rs:35`, `src/tracker/udp.rs:17`. Tracker URLs from a user-supplied
magnet/`.torrent` are dialed with no scheme/host/IP validation and default redirect-following
(10 hops). Attacker points `announce` at `http://169.254.169.254/...` (cloud metadata) or
`http://127.0.0.1:<port>/...`; UDP path enables internal port probing. Response snippets are
echoed into logs (`http.rs:84,91`). **Fix:** SSRF-safe resolve (reject loopback/link-local/
private/CGNAT/multicast), `reqwest::redirect::Policy::none()`, drop response-body previews from
logged errors.

---

## MEDIUM

### M1 — `migrate_media` arbitrary directory creation / file move
`src/web/api/settings.rs:53-71`. `req.path` unconfined → `create_dir_all(path/moviehouse)` and
relocates library files anywhere writable. **Fix:** same confinement as H1.

### M2 — ffmpeg/ffprobe args not protocol-pinned
`probe.rs:30` (bare positional input), `ffmpeg.rs:28,54,144,215,222`. With `download_dir` set
to `""` (via H1's unvalidated settings), torrent-name-derived paths lose the accidental `./`
prefix and can be reparsed as ffmpeg protocols (`subfile:`, `concat:`) → local file read.
**Fix:** `-i` for ffprobe; prefix all untrusted paths with `file:`; validate settings (H1).

### M3 — `browse_filesystem` allowlist too broad
`src/web/api/filesystem.rs:34`. Traversal containment is correct, but the allowed set is the
entire `$HOME` tree + all of `/Volumes` → any code-holder enumerates the owner's home layout and
every mounted volume (recon). **Fix:** narrow to the configured media/download roots.

### M4 — No access-code strength enforcement
`src/main.rs:182`. Only empty is rejected; `MOVIEHOUSE_ACCESS_CODE=1234` is accepted, yet the
code is the sole secret and the HMAC key. **Fix:** reject `< 16` chars at startup. (Cheap,
high value — the whole model depends on it.)

### M5 — No in-app login rate-limit / lockout; no source-IP logging
`src/web/auth.rs:140`. Brute-force protection is nginx-only (~3 r/s); anyone reaching the
container directly (LAN, other container, misconfig) gets unlimited guessing, and failures log
no IP. **Fix:** per-IP sliding-window/backoff in `login`, log `X-Forwarded-For` on failure.

### M6 — Torrent declared length not cross-checked vs piece count
`src/torrent/metainfo.rs parse_info`. A tiny `.torrent` can declare an enormous `length`;
`disk/io.rs:249 set_len` allocates it (sparse) with no real piece coverage. **Fix:** require
`total_length ≈ pieces.len() * piece_length`; cap absolute size.

### M7 — No global concurrent-download/connection cap; no disk pre-check
`src/engine/manager.rs`. No `MAX_CONCURRENT_TORRENTS`, no global socket ceiling, no free-space
check before allocating. **Fix:** global caps + disk pre-flight.

---

## LOW

- **L1** `cleanup_sources` becomes an arbitrary-file-delete primitive when combined with H3
  (`library.rs:126,144`). Closed by fixing H3.
- **L2** Raw `e.to_string()` in 500 bodies leaks sled/FS internals (`library.rs:18,41`,
  `settings.rs:40`). Return generic body, log detail server-side.
- **L3** Stateless logout: token valid for full 30-day TTL, non-revocable except by rotating the
  code. Acceptable; shorten TTL (e.g. 7d) and document.
- **L4** App sets no security headers/CSP (`server.rs:158`). Verify bharatsc nginx adds
  X-Frame-Options/nosniff/HSTS/CSP; else add app middleware.
- **L5** 0-piece torrent → `piece_length()` subtract-overflow (`metainfo.rs:210`). Reject empty
  `pieces` in `parse_info`.
- **L6** `TranscodeRequest.preset` not whitelisted (`transcode.rs`/`job.rs`) — data-integrity
  only, not injection.

---

## Confirmed CORRECT (no action — preserve these)

- Torrent **Zip-Slip blocked**: `metainfo.rs:304,331-339` rejects `/`,`\`,`..`,empty in name and
  every path component; same path for peer-supplied metadata. Tested.
- **HLS segment param** sanitized before join (`media.rs:186`).
- **upload_subtitle** sanitizes name, allowlists extension, server-generates output name;
  SRT→VTT is pure Rust (no ffmpeg on uploads). 10 MiB cap.
- **browse_filesystem traversal containment** correct (canonicalize + `starts_with`); scope is
  the only issue (M3).
- **No shell anywhere** — all subprocess calls use `Command` + array args (CWE-78 not reachable).
  `run_ffmpeg_encode`/`run_remux` use explicit `-i` and sanitized outputs.
- **Auth**: guard `route_layer` covers all `/api/v1/*` except `/auth/*` + `/health`; constant-time
  `code_matches`/MAC compare; token can't leak the code; fail-closed verify; `HttpOnly` +
  conditional `Secure` cookie; **no GET mutates state** (SameSite=Lax vector closed); CORS `Any`
  without credentials blocks cross-origin reads. No access code / token in logs.
- **bencode limits**: depth 64, elements 1M (tested). **Frame cap** 1 MiB. **Metadata cap** 10 MiB
  (0 rejected). Peer connections task-isolated. Kademlia routing table bounded. `known_addrs`
  capped 10k. No `unsafe`; no `.unwrap()`/`.expect()` in parsing code.

---

## Recommended remediation order (before going public)

1. **C1** — bencode `checked_add` + DHT `catch_unwind` (trivial, stops unauth DHT DoS).
2. **Path-confinement cluster (H1, H3, M1, M3)** — one `confine_under_root()` helper applied to
   `put_settings`, `scan_folder`, `migrate_media`, and to narrow `browse_filesystem`.
3. **H2** — sanitize subtitle `language`; `file:`-pin ffmpeg paths (also covers M2).
4. **H4** — SSRF-safe tracker resolution + no redirects.
5. **M4** — enforce access-code minimum length (cheap, foundational).
6. **M5–M7, L-series** — rate-limit/backoff, torrent length/piece + disk caps, generic errors,
   headers/CSP, TTL, degenerate-torrent rejection.
7. Ensure the app is **never run as root** outside Docker; keep the container's non-root `app` user.
