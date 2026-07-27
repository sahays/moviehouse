# Peer wire protocol & pipelined piece download

How a single peer connection goes from TCP connect to downloading verified pieces. One tokio task per peer (`run_peer_connection`, `src/peer/connection.rs`) multiplexes socket reads, commands, and keepalive with a single `select!` loop — there are no separate read/write tasks. The `TorrentSession` loop (`src/engine/session.rs`) owns the `PiecePicker` and drives requests.

```mermaid
sequenceDiagram
    autonumber
    participant Sess as TorrentSession
    participant PM as PeerManager
    participant Conn as PeerConn task
    participant Peer as Remote peer
    participant Picker as PiecePicker
    participant Store as PieceStore (SHA1)
    participant Disk as DiskManager

    rect rgb(238,248,238)
    note over Sess,Peer: Connect & handshake
    Sess->>PM: connect_pending (half-open / max-peer limits)
    PM->>Conn: spawn_connection(addr)
    Conn->>Peer: TCP connect (10s) + 68-byte handshake (BEP10 bit set)
    Peer-->>Conn: handshake
    alt info_hash mismatch
        Conn-->>Sess: Disconnected
    end
    Conn-->>Sess: PeerEvent::Connected { supports_extensions }
    Sess->>Conn: PeerCommand::SendInterested (+ ExtendedHandshake)
    end

    rect rgb(255,247,235)
    note over Sess,Peer: Bitfield & unchoke
    Peer-->>Conn: Bitfield / HaveAll / Have
    Conn-->>Sess: BitfieldReceived / HaveAll / Have
    Sess->>Picker: peer_has_bitfield (bump availability)
    Peer-->>Conn: Unchoke
    Conn-->>Sess: Unchoked (peer_choking = false)
    end

    rect rgb(245,240,255)
    note over Sess,Peer: Pipelined request/piece loop
    loop fill_pipeline while outstanding < depth
        Note over Sess: depth = 64 (normal) or throughput×8 clamped 64..256 (lightspeed)
        Sess->>Picker: pick_block (rarest-first, reservoir tie-break)
        Picker-->>Sess: BlockRequest { index, offset, len }
        Sess->>Conn: RequestBlock
        Conn->>Peer: Request (16 KiB block)
        Peer-->>Conn: Piece { index, begin, data }
        Conn-->>Sess: BlockReceived
        Sess->>Picker: block_received → Progress / Duplicate / PieceComplete
    end
    end

    rect rgb(255,238,244)
    note over Sess,Disk: Piece complete → verify → write → announce
    Sess->>Store: verify(index, data) — SHA1
    alt hash ok
        Sess->>Picker: mark_verified
        Sess->>Disk: write_piece (FileMapping spans, spawn_blocking)
        Sess->>PM: broadcast SendHave → all peers
    else hash mismatch
        Sess->>Picker: piece_failed → re-pick
    end
    end

    rect rgb(235,244,255)
    note over Sess,Peer: Choke / disconnect / endgame
    opt peer chokes or disconnects
        Sess->>Picker: release/unassign pending blocks → refill other peers
    end
    opt endgame (in_progress == remaining)
        Sess->>PM: duplicate-request remaining blocks on all peers, CancelBlock on receipt
    end
    Note over Conn,Peer: keepalive every 60s, choke algorithm every 10s (optimistic unchoke each 3rd)
    end
```

## Notes

- **Channels:** `mpsc<(SocketAddr, PeerEvent)>` cap 512 (all peers → session, single receiver); per-peer `mpsc<PeerCommand>` cap 512 (session → peer, `try_send`); `mpsc<DiskCommand>` cap 64 with per-request `oneshot` reply.
- **Picker strategy:** starts RandomFirst, flips to RarestFirst after the first verified piece; endgame mode fans out duplicate requests to finish the tail quickly.
- **Disk writes** map each piece to per-file byte spans (`FileMapping::piece_spans`) and run on `spawn_blocking`; `sync_all` is skipped in lightspeed mode. Outstanding writes are awaited at shutdown.
- Source: `src/peer/{handshake,connection,manager,message,codec}.rs`, `src/piece/{picker,store,bitfield}.rs`, `src/disk/{io,mapping}.rs`, `src/engine/session.rs`.
