# Download from a `.torrent` file

Adding a torrent via the web API, running it to completion, and the post-completion library/transcode/metadata follow-up.

Entry point: `add_torrent` (`src/web/api/torrents.rs`). The engine is driven by `SessionManager` (`src/engine/manager.rs`) and one `TorrentSession` per download (`src/engine/session.rs`), persisted in the sled `Store` (`src/engine/store.rs`).

```mermaid
sequenceDiagram
    autonumber
    actor Client
    participant API as add_torrent<br/>(web/api/torrents.rs)
    participant Mgr as SessionManager
    participant Store as sled Store
    participant Sess as TorrentSession
    participant Disc as Tracker + DHT
    participant Peers as PeerManager
    participant Disk as DiskManager
    participant WS as WS clients

    rect rgb(235,244,255)
    note over Client,Store: Add
    Client->>API: POST /api/v1/torrents (multipart .torrent)
    alt active_count >= max_downloads
        API-->>Client: 429 Too Many Requests
    end
    API->>API: Metainfo::from_bytes (parse) — 400 on error
    API->>Store: get_settings → DownloadOptions (dir, lightspeed)
    API->>Mgr: add_torrent(metainfo, bytes, opts)
    end

    rect rgb(238,248,238)
    note over Mgr,Store: Start
    Mgr->>Sess: TorrentSession::new (uuid, watch<Status>)
    Mgr->>Store: put_download(record)
    Mgr->>Mgr: spawn Task A (status→event forwarder)
    Mgr->>Mgr: spawn Task B (session.run)
    Mgr-->>API: id
    API-->>Client: 201 Created (status snapshot)
    end

    rect rgb(255,247,235)
    note over Sess,Disk: Discover peers (Task B)
    Sess->>Disk: FileMapping + pre_allocate, spawn disk loop
    Sess->>Disc: TrackerManager announce "started" (spawned)
    Sess->>Disc: DhtHandle::start + get_peers loop (if DHT)
    Disc-->>Sess: peers via mpsc<Vec<SocketAddr>>
    end

    rect rgb(245,240,255)
    note over Sess,WS: Download loop (until picker.is_complete or cancel)
    loop select! on peers / peer events / timers
        Sess->>Peers: add_peers + connect_pending
        Peers-->>Sess: PeerEvent (Connected / Bitfield / Unchoked / BlockReceived)
        Sess->>Sess: PiecePicker.pick_block → fill_pipeline (RequestBlock)
        Sess->>Sess: PieceStore.verify(SHA1)
        alt piece verified
            Sess->>Disk: write_piece (spawned)
            Sess->>Peers: broadcast Have
        else hash mismatch
            Sess->>Sess: picker.piece_failed → re-pick
        end
        Sess->>Sess: speed tick (1s) → status_tx.send(status)
        Sess-->>WS: Task A: broadcast SessionEvent (progress)
    end
    end

    rect rgb(255,238,244)
    note over Sess,WS: Complete + follow-up
    Sess->>Sess: state = Completed/Cancelled, await pending writes, cancel()
    Sess-->>Mgr: Task B: final status
    Mgr-->>WS: broadcast final SessionEvent
    Mgr->>Store: put_download(status) + flush
    opt state == Completed
        Mgr->>Mgr: detect_video_files → parse → dedupe vs list_media
        Mgr->>Store: put_media(entry) per new file
        opt auto_transcode && ffmpeg available
            Mgr->>Mgr: create_job + spawn TranscodeHandle::submit
        end
        opt tmdb_api_key set
            Mgr->>Mgr: spawn fetch_metadata_auto → apply_metadata → put_media
        end
    end
    end
```

## Notes

- **Channels:** per-session `watch<SessionStatus>` (session → forwarder); `broadcast<SessionEvent>` cap 256 (forwarder → all WS clients); `mpsc<Vec<SocketAddr>>` cap 64 (tracker + DHT → session); `mpsc<(SocketAddr, PeerEvent)>` cap 512 (peers → session).
- **The `.torrent` path is synchronous** into `add_torrent`; the magnet path in the same handler is a two-phase async variant — see [download-magnet.md](download-magnet.md).
- **Persistence cadence:** the forwarder task persists status to the store roughly every 5s and on `Completed`/`Error`.
- Source: `src/web/api/torrents.rs`, `src/engine/manager.rs`, `src/engine/session.rs`, `src/engine/store.rs`, `src/web/ws.rs`.
