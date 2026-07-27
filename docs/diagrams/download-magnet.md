# Magnet link — metadata exchange (BEP 9 / BEP 10)

A magnet link carries only the infohash, not the `info` dict. Before a normal download can start, MovieHouse fetches the metadata from peers using the extension protocol (BEP 10) and metadata exchange (BEP 9), verifies it against the infohash, then hands off to a regular `TorrentSession`.

Orchestrator: `engine::magnet::download_metadata` (`src/engine/magnet.rs`). Buffer/assembly: `MetadataBuffer` (`src/engine/magnet_buffer.rs`). Extension messages: `src/peer/extension.rs`.

```mermaid
sequenceDiagram
    autonumber
    actor Caller as CLI cmd_magnet /<br/>web add (torrents.rs)
    participant DM as download_metadata<br/>(engine/magnet.rs)
    participant Disc as Tracker + DHT
    participant Peers as PeerManager
    participant Peer as run_peer_connection
    participant Buf as MetadataBuffer

    rect rgb(235,244,255)
    note over Caller,DM: Parse & kick off
    Caller->>DM: MagnetLink::parse(uri) → download_metadata(magnet, ...)
    note right of Caller: web path wraps this in a spawned task<br/>after register_magnet (Resolving placeholder)
    end

    rect rgb(255,247,235)
    note over DM,Peers: Discover peers
    DM->>Disc: TrackerManager::run + DhtHandle get_peers loop
    Disc-->>DM: peers via mpsc&lt;Vec&lt;SocketAddr&gt;&gt;
    DM->>Peers: add_peers + connect_pending
    end

    rect rgb(238,248,238)
    note over Peers,Peer: Connect + extension handshake
    Peers->>Peer: spawn_connection(addr)
    Peer->>Peer: TCP connect, BT handshake (reserved bit 0x10 = BEP10)
    alt info_hash mismatch
        Peer-->>DM: disconnect
    end
    Peer-->>DM: PeerEvent::Connected { supports_extensions }
    opt supports_extensions
        DM->>Peer: SendInterested + SendExtendedHandshake (ut_metadata=1)
        Peer-->>DM: PeerEvent::ExtendedHandshake (ut_metadata id, metadata_size)
    end
    alt metadata_size == 0 or > 10 MiB
        DM->>DM: skip this peer
    end
    end

    rect rgb(245,240,255)
    note over DM,Buf: Request & assemble pieces (16 KiB each)
    DM->>Buf: MetadataBuffer::new(size)
    loop until received_count == num_pieces
        DM->>Peer: SendMetadataRequest { ext_id, piece }
        Peer-->>DM: PeerEvent::MetadataMessage(Data / Reject)
        DM->>Buf: on_data(piece, data) / on_reject(piece)
    end
    end

    rect rgb(255,238,244)
    note over DM,Caller: Verify & hand off
    DM->>Buf: verify(info_hash) — SHA1(buffer) == infohash
    alt hash mismatch
        DM-->>Caller: bail! (metadata hash verification failed)
    end
    DM->>DM: Metainfo::from_info_bytes(raw_info, info_hash, trackers)
    DM-->>Caller: Ok(metainfo, warm_peers)
    Caller->>Caller: CLI → TorrentSession::new(seeded with warm_peers)<br/>web → manager.resolve_magnet → add_torrent
    end
```

## Notes

- **`ut_metadata` negotiation:** our extended handshake advertises `ut_metadata=1` (and `ut_pex=2` in lightspeed mode); the peer's handshake tells us its own ids and the total `metadata_size`.
- **Robustness:** pieces rejected or lost when a peer disconnects are unassigned and retried against other peers (`MetadataBuffer::on_reject` / `on_peer_lost`).
- **Hand-off:** the CLI reuses the peers it already warmed up (`warm_peers`); the web path discards them and lets `add_torrent` start fresh — see [download-torrent.md](download-torrent.md).
- Source: `src/torrent/magnet.rs`, `src/engine/magnet.rs`, `src/engine/magnet_buffer.rs`, `src/peer/extension.rs`, `src/torrent/metainfo.rs`.
