# DHT peer discovery

The DHT (BEP 5) finds peers for an infohash without a tracker. `DhtHandle` (`src/dht/node.rs`) spawns the node event loop; `KrpcSocket` (`src/dht/krpc.rs`) is the single UDP socket with a transaction map; `lookup.rs` runs the iterative Kademlia search. Discovered peers reach the `TorrentSession` over the same `mpsc<Vec<SocketAddr>>` the tracker uses.

```mermaid
sequenceDiagram
    autonumber
    participant Sess as TorrentSession
    participant DHT as DhtHandle / run_dht_node
    participant RT as RoutingTable
    participant Krpc as KrpcSocket (UDP)
    participant Look as iterative_get_peers
    participant Net as Remote DHT nodes

    rect rgb(238,248,238)
    note over Sess,Net: Startup / bootstrap
    Sess->>DHT: DhtHandle::start(addr, cancel, lightspeed)
    DHT->>RT: load_from_file(~/.moviehouse/dht_nodes.json) [lightspeed] or random NodeId
    DHT->>Krpc: bind UDP socket; spawn recv_loop
    DHT->>DHT: spawn run_dht_node (inbound + lookup + timers)
    alt cached >= 8 nodes (lightspeed)
        DHT->>Look: iterative_find_node (refresh only)
    else
        DHT->>Net: bootstrap — DNS resolve 5 nodes, find_node(own_id)
        Net-->>DHT: compact nodes → RoutingTable.insert_or_update
        DHT->>Look: iterative_find_node (fill buckets)
    end
    DHT-->>Sess: handle (lookup_tx, cancel)
    end

    rect rgb(245,240,255)
    note over Sess,Net: Lookup (get_peers for infohash)
    Sess->>DHT: get_peers(info_hash) [re-queried every 15s]
    DHT->>Look: spawn iterative_get_peers(info_hash, peer_tx)
    Look->>RT: closest_nodes(target, K=8) — seed candidates
    loop max 20 rounds, ALPHA=3 in flight
        Look->>Krpc: query(GetPeers) × 3 (concurrent tasks)
        Krpc->>Krpc: alloc 2-byte txn id, insert oneshot into pending map
        Krpc->>Net: UDP get_peers
        Net-->>Krpc: recv_loop matches txn id → fire oneshot
        Krpc-->>Look: (id, token, peers, nodes)
        Look->>RT: insert_or_update + mark_good / mark_failed
        Look-->>Sess: peer_tx.send(peers) — stream batches as found
        Note over Look: stop when closest un-queried is no closer than closest responded
    end
    end

    rect rgb(255,247,235)
    note over Sess: Deliver to session
    Sess->>Sess: get_peers rx → shared peer_tx → PeerManager.add_peers + connect_pending
    end

    rect rgb(235,244,255)
    note over DHT,Net: Maintenance
    DHT->>DHT: rotate tokens every 5 min
    DHT->>Look: refresh buckets (iterative_find_node) every 15 min
    DHT->>Net: answer inbound ping / find_node / get_peers (issue/verify tokens)
    opt on shutdown (lightspeed)
        DHT->>RT: save_to_file(dht_nodes.json)
    end
    end
```

## Notes

- **Transaction map:** each outbound query allocates a 2-byte id (global `AtomicU16`) and registers a `oneshot` in a `DashMap`; the single `recv_loop` correlates responses by id and wakes the awaiting `query()` (5s timeout).
- **One socket:** an `Arc<UdpSocket>` is shared by the reader loop and all senders.
- **ALPHA=3 / K=8:** each round spawns 3 concurrent queries and awaits them before the next round; candidate set holds the K=8 closest.
- **Two consumers:** `TorrentSession` (this diagram) and the magnet metadata fetcher (`src/engine/magnet.rs`) both use `DhtHandle::start` + `get_peers` identically.
- Source: `src/dht/{node,krpc,lookup,routing_table,token}.rs`, `src/engine/session.rs`.
