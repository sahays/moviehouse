use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::krpc::{InboundQuery, KrpcQuery, KrpcResponse, KrpcSocket};
use super::lookup;
use super::routing_table::{NodeId, RoutingTable};
use super::token::TokenManager;
use crate::torrent::types::InfoHash;

const BOOTSTRAP_NODES: &[&str] = &[
    "dht.libtorrent.org:25401",
    "dht.transmissionbt.com:6881",
    "router.bittorrent.com:8991",
    "router.utorrent.com:6881",
    "dht.aelitis.com:6881",
];

fn dht_cache_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".torrentclient/dht_nodes.json")
}

/// Handle for interacting with the DHT from the download engine.
pub struct DhtHandle {
    lookup_tx: mpsc::Sender<DhtLookupRequest>,
    cancel: CancellationToken,
}

struct DhtLookupRequest {
    info_hash: InfoHash,
    peer_tx: mpsc::Sender<Vec<SocketAddr>>,
}

impl DhtHandle {
    /// Start the DHT node. Returns a handle for requesting peer lookups.
    pub async fn start(
        bind_addr: SocketAddr,
        cancel: CancellationToken,
        lightspeed: bool,
    ) -> Result<Self, std::io::Error> {
        // Try loading persistent state in lightspeed mode
        let (own_id, loaded_nodes) = if lightspeed {
            let path = dht_cache_path();
            if let Some((id, nodes)) = RoutingTable::load_from_file(&path) {
                (id, nodes)
            } else {
                (NodeId::random(), Vec::new())
            }
        } else {
            (NodeId::random(), Vec::new())
        };

        let mut krpc = KrpcSocket::bind(bind_addr, own_id).await?;
        let inbound_rx = krpc.take_inbound_rx().unwrap();
        let krpc = Arc::new(krpc);
        let routing_table = Arc::new(RwLock::new(RoutingTable::new(own_id)));

        // Pre-populate routing table with loaded nodes
        if !loaded_nodes.is_empty() {
            let mut rt = routing_table.write().await;
            for (nid, naddr) in &loaded_nodes {
                rt.insert_or_update(*nid, *naddr);
            }
        }

        let token_manager = Arc::new(Mutex::new(TokenManager::new()));

        let (lookup_tx, lookup_rx) = mpsc::channel(32);

        // Spawn recv loop
        let krpc_recv = Arc::clone(&krpc);
        tokio::spawn(async move {
            krpc_recv.recv_loop().await;
        });

        // Spawn main DHT loop
        let krpc_main = Arc::clone(&krpc);
        let rt_main = Arc::clone(&routing_table);
        let tm_main = Arc::clone(&token_manager);
        let cancel_main = cancel.clone();
        tokio::spawn(async move {
            run_dht_node(
                krpc_main,
                rt_main,
                tm_main,
                inbound_rx,
                lookup_rx,
                cancel_main,
                lightspeed,
            )
            .await;
        });

        // Bootstrap: skip full bootstrap if we loaded enough nodes in lightspeed mode
        let loaded_count = loaded_nodes.len();
        if lightspeed && loaded_count >= 8 {
            eprintln!("DHT loaded {loaded_count} nodes from cache, skipping bootstrap");
            // Still do a quick self-lookup to refresh
            let krpc_refresh = Arc::clone(&krpc);
            let rt_refresh = Arc::clone(&routing_table);
            tokio::spawn(async move {
                lookup::iterative_find_node(&krpc_refresh, &rt_refresh, own_id).await;
            });
        } else {
            bootstrap(&krpc, &routing_table).await;
        }

        Ok(Self { lookup_tx, cancel })
    }

    /// Request peer discovery for an info_hash.
    /// Returns a receiver that will stream peer batches as they're found.
    pub async fn get_peers(
        &self,
        info_hash: InfoHash,
    ) -> mpsc::Receiver<Vec<SocketAddr>> {
        let (peer_tx, peer_rx) = mpsc::channel(32);
        let _ = self
            .lookup_tx
            .send(DhtLookupRequest { info_hash, peer_tx })
            .await;
        peer_rx
    }

    pub async fn shutdown(&self) {
        self.cancel.cancel();
    }
}

async fn run_dht_node(
    krpc: Arc<KrpcSocket>,
    routing_table: Arc<RwLock<RoutingTable>>,
    token_manager: Arc<Mutex<TokenManager>>,
    mut inbound_rx: mpsc::Receiver<InboundQuery>,
    mut lookup_rx: mpsc::Receiver<DhtLookupRequest>,
    cancel: CancellationToken,
    lightspeed: bool,
) {
    let mut token_rotation = tokio::time::interval(Duration::from_secs(5 * 60));
    token_rotation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut refresh_interval = tokio::time::interval(Duration::from_secs(15 * 60));
    refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                debug!("DHT node shutting down");
                // Save routing table on shutdown in lightspeed mode
                if lightspeed {
                    let path = dht_cache_path();
                    let rt = routing_table.read().await;
                    if let Err(e) = rt.save_to_file(&path) {
                        debug!(error = %e, "Failed to save DHT routing table");
                    } else {
                        debug!("DHT routing table saved to {}", path.display());
                    }
                }
                break;
            }

            // Handle inbound queries
            Some(query) = inbound_rx.recv() => {
                handle_inbound(
                    &krpc,
                    &routing_table,
                    &token_manager,
                    query,
                ).await;
            }

            // Handle peer lookup requests
            Some(req) = lookup_rx.recv() => {
                let krpc = Arc::clone(&krpc);
                let rt = Arc::clone(&routing_table);
                tokio::spawn(async move {
                    lookup::iterative_get_peers(&krpc, &rt, req.info_hash, &req.peer_tx).await;
                });
            }

            // Token rotation
            _ = token_rotation.tick() => {
                token_manager.lock().await.maybe_rotate();
            }

            // Bucket refresh
            _ = refresh_interval.tick() => {
                let own_id = *routing_table.read().await.own_id();
                let krpc = Arc::clone(&krpc);
                let rt = Arc::clone(&routing_table);
                tokio::spawn(async move {
                    lookup::iterative_find_node(&krpc, &rt, own_id).await;
                });
            }
        }
    }
}

async fn handle_inbound(
    krpc: &Arc<KrpcSocket>,
    routing_table: &Arc<RwLock<RoutingTable>>,
    token_manager: &Arc<Mutex<TokenManager>>,
    query: InboundQuery,
) {
    // Update routing table with the sender
    routing_table
        .write()
        .await
        .insert_or_update(query.sender_id, query.sender_addr);

    let own_id = *routing_table.read().await.own_id();

    let response = match query.query {
        KrpcQuery::Ping => KrpcResponse::Ping { id: own_id },
        KrpcQuery::FindNode { target } => {
            let nodes: Vec<(NodeId, SocketAddr)> = routing_table
                .read()
                .await
                .closest_nodes(&target, 8)
                .into_iter()
                .map(|n| (n.id, n.addr))
                .collect();
            KrpcResponse::FindNode { id: own_id, nodes }
        }
        KrpcQuery::GetPeers { info_hash } => {
            let token = token_manager
                .lock()
                .await
                .generate(&query.sender_addr.ip());
            let target = NodeId(info_hash.0);
            let nodes: Vec<(NodeId, SocketAddr)> = routing_table
                .read()
                .await
                .closest_nodes(&target, 8)
                .into_iter()
                .map(|n| (n.id, n.addr))
                .collect();
            KrpcResponse::GetPeers {
                id: own_id,
                token: Some(token),
                peers: vec![],
                nodes,
            }
        }
        KrpcQuery::AnnouncePeer { .. } => KrpcResponse::AnnouncePeer { id: own_id },
    };

    if let Err(e) = krpc.respond(query.sender_addr, &query.txn_id, response).await {
        debug!(error = %e, "Failed to send KRPC response");
    }
}

async fn bootstrap(krpc: &Arc<KrpcSocket>, routing_table: &Arc<RwLock<RoutingTable>>) {
    eprintln!("DHT bootstrapping...");

    let own_id = *routing_table.read().await.own_id();

    // Resolve all bootstrap nodes concurrently
    let mut addrs: Vec<(String, SocketAddr)> = Vec::new();
    for node_str in BOOTSTRAP_NODES {
        if let Ok(mut resolved) = tokio::net::lookup_host(node_str).await {
            if let Some(addr) = resolved.next() {
                addrs.push((node_str.to_string(), addr));
            }
        }
    }

    // Query all bootstrap nodes concurrently (5s timeout each)
    let mut handles = Vec::new();
    for (name, addr) in &addrs {
        let krpc = Arc::clone(krpc);
        let target = own_id;
        let name = name.clone();
        let addr = *addr;
        handles.push(tokio::spawn(async move {
            let result = krpc.query(addr, KrpcQuery::FindNode { target }).await;
            (name, addr, result)
        }));
    }

    for handle in handles {
        if let Ok((name, addr, result)) = handle.await {
            match result {
                Ok(resp) => {
                    // Handle both FindNode and GetPeers (decode_response may return either)
                    let (id, nodes) = match resp {
                        KrpcResponse::FindNode { id, nodes } => (id, nodes),
                        KrpcResponse::GetPeers { id, nodes, .. } => (id, nodes),
                        KrpcResponse::Ping { id } => (id, vec![]),
                        _ => continue,
                    };
                    routing_table.write().await.insert_or_update(id, addr);
                    for (nid, naddr) in nodes {
                        routing_table.write().await.insert_or_update(nid, naddr);
                    }
                    let count = routing_table.read().await.node_count();
                    eprintln!("DHT bootstrap: {name} -> {count} nodes");
                }
                Err(e) => {
                    eprintln!("DHT bootstrap: {name} failed: {e}");
                }
            }
        }
    }

    let node_count = routing_table.read().await.node_count();
    eprintln!("DHT bootstrap complete: {node_count} nodes");

    // Perform iterative find_node on our own ID to populate nearby buckets
    if node_count > 0 {
        lookup::iterative_find_node(krpc, routing_table, own_id).await;
        let final_count = routing_table.read().await.node_count();
        eprintln!("DHT routing table populated: {final_count} nodes");
    } else {
        eprintln!("DHT bootstrap failed: no nodes found");
    }
}
