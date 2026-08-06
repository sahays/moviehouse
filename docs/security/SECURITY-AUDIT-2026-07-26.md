# MovieHouse Security Audit — 2026-07-26

- **Date:** 2026-07-26
- **Reviewed commit:** `f0b9d4b` (f0b9d4bec0a53c7a5d1c0ff0b278027472506e3d), branch `main`
- **Status:** ✅ **ALL findings remediated** (2026-07-26) — see Remediation summary below.

Pre-public-exposure review across four dimensions (path traversal, command/argument
injection, auth/session/web-layer, BitTorrent parsing/SSRF/DoS). Each finding was
traced in the actual code; the bencode overflow was confirmed with a reverted PoC.

**Context:** app is exposed at `moviehouse.niniconai.com` behind a single shared
access code, with the BitTorrent port 6881 (tcp+udp) directly on the internet.

**Key mitigating fact:** the Docker image runs as **non-root `app`**, so file-write
primitives are confined to `/data` and `/media` in the container — bad, but not root
RCE. Running the raw binary as root (systemd) removes this mitigation.

> **⚠️ Addendum 2026-08-06 — the deployment model this audit describes no longer
> exists.** MovieHouse was reverted to local-only (LAN, plain HTTP, no Docker, no
> nginx). Read the addendum at the bottom before relying on anything above: the
> premises in **Context** and **Key mitigating fact** are both void.

## Remediation summary (2026-07-26)

All findings fixed and verified — full pre-deploy gate green (fmt, clippy `-D warnings`,
95 tests, frontend build+lint) and runtime smoke tests pass.

| Finding | Fix | Commit |
|---|---|---|
| C1 bencode overflow / DHT DoS | `checked_add` + per-packet DHT task isolation + regression test | `9bff7bd` |
| H1 settings download_dir write | path confinement in `put_settings` (`web::api::paths`) | `15e7803` |
| H2 subtitle-lang arbitrary write | sanitize `language` before the `.vtt` path | `ac103ff` |
| H3 scan_folder whole-disk read | confine scan path to allowed roots | `15e7803` |
| H4 tracker SSRF | resolve + reject non-public IPs, pin DNS, no redirects (HTTP+UDP) | `207ff1e` |
| M1 migrate_media path | confine target | `15e7803` |
| M2 ffmpeg protocol injection | `file:`-pin untrusted ffmpeg/ffprobe inputs | `ac103ff` |
| M3 browse over-broad | unified allowed-roots policy | `15e7803` |
| M4 weak access code | min 16 chars enforced at startup | `f5f9d2c` |
| M5 login brute-force | source-IP logging + 500ms failure delay | `f5f9d2c` |
| M6 torrent length vs pieces | consistency check in `parse_info` | `9a68344` |
| M7 resource exhaustion | `MOVIEHOUSE_MAX_DOWNLOADS` cap (default 2) | `bf9cd1f`, `d4a31ed` |
| L1 delete primitive | closed by H3 | `15e7803` |
| L2 error leakage | generic 500 body, log detail server-side | `15e7803` |
| L3 logout replay | documented tradeoff (TTL kept 30d by choice) | `f5f9d2c` |
| L4 no CSP | CSP added to the moviehouse nginx vhost | `e1ba695` |
| L5 0-piece torrent | reject empty `pieces` + `saturating_sub` | `9a68344` |
| L6 preset not whitelisted | reject unknown preset | `ac103ff` |

**Deferred / residual (non-blocking):** M7's disk-free pre-check (needs a filesystem-stats
crate; the concurrency cap + M6 bound the risk); the L4 CSP must be verified against the
live SPA and relaxed if it blocks a legitimate resource; tracker DNS-rebinding is mitigated
by IP-pinning but not fully eliminated. Keep the container's non-root `app` user, and do not
run the raw binary as root.

## Original verdict (pre-fix): DO NOT expose publicly until CRITICAL + HIGH are fixed.

Two root causes drove most findings:
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

---

## Addendum 2026-08-06 — reverted to local-only

The remote deployment was retired (compute + egress cost for a library only watched
at home). Docker, docker-compose, the bharatsc nginx vhost, and the deploy scripts
are deleted. `moviehouse serve --bind 0.0.0.0:9000` now faces the LAN directly over
plain HTTP. Three consequences for the findings above.

### 1. Exposure shrank — the internet is no longer in the threat model

The app is no longer reachable from outside the house, and the BitTorrent port is
only internet-facing if the router forwards it. The attacker set is now devices on
the home network (including anything IoT and compromised) rather than the world.
This lowers the practical severity of most findings; it does not close them.

### 2. What nginx was doing is now in the app

Deleting the vhost silently removed protections the table above credits to it.
Each was re-homed in-process:

| Was nginx | Now |
|---|---|
| CSP + security headers (**L4**) | `src/web/security_headers.rs`, layered over the whole router. Same policy, carried over verbatim. **No HSTS** — plain HTTP on the LAN, and HSTS would pin the browser to `https://` and lock the user out. |
| `limit_req zone=auth` on login (**M5**) | `src/web/ratelimit.rs` — per-IP fixed window, 10 failures / 5 min → `429`. The 500 ms failure delay stays underneath it. |
| Setting trustworthy `X-Forwarded-For` / `-Proto` | Nothing. Both headers are now **caller-controlled** and are no longer read. `auth::PeerIp` takes the TCP peer from `ConnectInfo`; the session cookie is unconditionally non-`Secure`. |

That last row was a live defect for as long as the code outlived the proxy: a
forged `X-Forwarded-For` would have poisoned the login audit log and let a guesser
sidestep any IP-keyed throttle, and a forged `X-Forwarded-Proto: https` would have
set `Secure` on a cookie the browser then refuses to send back over HTTP.

### 3. The "key mitigating fact" no longer holds — read this one carefully

The audit leans on the container's non-root `app` user to bound the file-write
findings (H1, H2, H3, M1, M3, and the `/etc/cron.d` scenario in the detail section).
**There is no container.** The binary runs as the desktop user, with that user's
full privileges: `~/.zshrc`, `~/.ssh/`, LaunchAgents, and every document on the Mac
are writable by the process.

The path confinement in `src/web/api/paths.rs` is now the *only* thing standing
between an authenticated caller and arbitrary user-level file writes. It was
verified correct at audit time and its tests still pass — but it carries the whole
weight now. Treat any change to `confine`/`validate_config_dir` as security-critical
and do not widen the allowed roots casually.

Mitigating this: reaching those endpoints still requires the access code, and the
access code is now only offerable from the LAN.

### Not re-reviewed

This addendum reasons about the deployment change only. No new code audit was
performed, and the BitTorrent parsing / SSRF / DoS findings are unchanged.

---

## Addendum 2026-08-06 (second) — access-code auth removed entirely

The access code is gone: no login, no `mh_session` cookie, no playback tokens, no
login throttle. `src/web/auth.rs` and `src/web/ratelimit.rs` are deleted and every
route — including the WebSocket and the media byte-serving routes — is now open to
anything that can open a TCP connection to the port.

This was a deliberate choice for a home-LAN-only server. Its consequences are not
subtle, so they are recorded here rather than left implicit.

### Every finding that was gated behind "requires the access code" is now ungated

The audit repeatedly treats authentication as the outer gate. There is no outer
gate. Reachable unauthenticated by any device on the network:

- **H1 / M1 / M3** — `put_settings`, `migrate_media`, `browse_filesystem`: rewrite
  the download/transcode/scan roots and enumerate directories, bounded only by
  `api/paths.rs`.
- **H3** — `scan_folder`: import and then stream any video file under an allowed root.
- **M7** — queue downloads until the disk fills, bounded only by `MOVIEHOUSE_MAX_DOWNLOADS`.
- **New since the audit** — `DELETE /api/v1/library/{id}?delete_files=true`
  irreversibly deletes a title's source, transcodes, and subtitles.

Combined with the first addendum's point 3 (no container, so the process runs as
the desktop user), the practical position is: **anyone on the Wi-Fi can read,
import, and delete files anywhere under `api/paths.rs`'s allowed roots — which
include `$HOME`.**

### What is left standing

1. `api/paths.rs` confinement — now the *only* server-side control. Every finding
   above is bounded by it and nothing else. Changes to `confine` /
   `validate_config_dir` / `allowed_roots` are the highest-risk changes in the repo.
2. Network reachability — the server must be bound somewhere the attacker can
   reach. `--bind 127.0.0.1:9000` reduces the attacker set to processes on the host.
3. `security_headers.rs` — CSP and friends still ship on every response (L4 remains
   closed).

### If this ever leaves the LAN

Do not port-forward, reverse-proxy, or tunnel this build to a public address. The
audit's original verdict — *"DO NOT expose publicly"* — applies with more force now
than when it was written, because the control it assumed no longer exists.

### Addendum 2026-08-06 (third) — M7 concurrency cap removed

`MOVIEHOUSE_MAX_DOWNLOADS` and the `active_count() >= max_downloads` check in
`add_torrent` are gone; concurrent downloads are unbounded. **M7 is reopened** and
now has no mitigation at all: its "deferred/residual" note above said the disk-free
pre-check was acceptable to skip *because the concurrency cap bounded the risk*.
That cap no longer exists.

Practically, on a LAN box with no auth (see the second addendum), anything on the
network can queue downloads until the disk, file descriptors, or sockets run out.
The torrent engine's own per-session limits (`max_peers`, piece caps) still bound
each individual download; nothing bounds the number of them.

This is a deliberate usability choice for a single-user home server, where the
person adding torrents is the person who owns the disk.
