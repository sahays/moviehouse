use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};

use super::krpc::{KrpcQuery, KrpcResponse, KrpcSocket};
use super::routing_table::{NodeId, RoutingTable};
use crate::torrent::types::InfoHash;

const ALPHA: usize = 3; // concurrency factor
const K: usize = 8; // result set size

/// Iterative `get_peers` lookup.
/// Returns discovered peer addresses and sends them via the channel as they're found.
pub async fn iterative_get_peers(
    krpc: &Arc<KrpcSocket>,
    routing_table: &Arc<RwLock<RoutingTable>>,
    info_hash: InfoHash,
    peer_tx: &mpsc::Sender<Vec<SocketAddr>>,
) -> Vec<(NodeId, SocketAddr, Option<Vec<u8>>)> {
    // Convert info_hash to NodeId for distance calculations
    let target = NodeId(info_hash.0);

    // Seed with K closest nodes from our routing table
    let initial_nodes = routing_table.read().await.closest_nodes(&target, K);
    let mut candidates: Vec<(NodeId, SocketAddr, CandidateState)> = initial_nodes
        .into_iter()
        .map(|n| (n.id, n.addr, CandidateState::NotQueried))
        .collect();

    let mut all_peers: Vec<SocketAddr> = Vec::new();
    let mut tokens: Vec<(NodeId, SocketAddr, Option<Vec<u8>>)> = Vec::new();
    let mut queried: HashSet<NodeId> = HashSet::new();

    for _ in 0..20 {
        // Safety: max iterations
        // Pick ALPHA closest un-queried candidates
        candidates.sort_by_key(|(id, _, _)| id.distance(&target).0);

        let to_query: Vec<(NodeId, SocketAddr)> = candidates
            .iter()
            .filter(|(id, _, state)| *state == CandidateState::NotQueried && !queried.contains(id))
            .take(ALPHA)
            .map(|(id, addr, _)| (*id, *addr))
            .collect();

        if to_query.is_empty() {
            break;
        }

        // Mark as in-flight
        for (id, _) in &to_query {
            queried.insert(*id);
            if let Some(c) = candidates.iter_mut().find(|(cid, _, _)| *cid == *id) {
                c.2 = CandidateState::InFlight;
            }
        }

        // Query all concurrently
        let mut handles = Vec::new();
        for (id, addr) in to_query {
            let krpc = Arc::clone(krpc);
            let ih = info_hash;
            handles.push(tokio::spawn(async move {
                let result = krpc
                    .query(addr, KrpcQuery::GetPeers { info_hash: ih })
                    .await;
                (id, addr, result)
            }));
        }

        for handle in handles {
            if let Ok((node_id, addr, result)) = handle.await {
                if let Ok(resp) = result {
                    // Handle any response type (decode can't reliably distinguish)
                    let (id, token, peers, nodes) = match resp {
                        KrpcResponse::GetPeers {
                            id,
                            token,
                            peers,
                            nodes,
                        } => (id, token, peers, nodes),
                        KrpcResponse::FindNode { id, nodes } => (id, None, vec![], nodes),
                        KrpcResponse::Ping { id } | KrpcResponse::AnnouncePeer { id } => {
                            (id, None, vec![], vec![])
                        }
                    };

                    // Mark as responded
                    if let Some(c) = candidates.iter_mut().find(|(cid, _, _)| *cid == node_id) {
                        c.2 = CandidateState::Responded;
                    }

                    // Update routing table
                    routing_table.write().await.insert_or_update(id, addr);
                    routing_table.write().await.mark_good(&id);

                    // Store token for later announce
                    tokens.push((id, addr, token));

                    // Add new peers
                    if !peers.is_empty() {
                        let _ = peer_tx.send(peers.clone()).await;
                        all_peers.extend(peers);
                    }

                    // Add new candidates
                    for (nid, naddr) in nodes {
                        if !candidates.iter().any(|(id, _, _)| *id == nid) {
                            candidates.push((nid, naddr, CandidateState::NotQueried));
                        }
                    }
                } else {
                    if let Some(c) = candidates.iter_mut().find(|(cid, _, _)| *cid == node_id) {
                        c.2 = CandidateState::Failed;
                    }
                    routing_table.write().await.mark_failed(&node_id);
                }
            }
        }

        // Check termination: closest un-queried node is farther than closest responded
        let closest_responded = candidates
            .iter()
            .filter(|(_, _, s)| *s == CandidateState::Responded)
            .map(|(id, _, _)| id.distance(&target))
            .min();

        let closest_unqueried = candidates
            .iter()
            .filter(|(_, _, s)| *s == CandidateState::NotQueried)
            .map(|(id, _, _)| id.distance(&target))
            .min();

        if let (Some(resp_dist), Some(unq_dist)) = (closest_responded, closest_unqueried)
            && unq_dist.0 >= resp_dist.0
        {
            break; // No closer nodes to explore
        }
    }

    tokens
}

/// Iterative `find_node` lookup (for bootstrap and bucket refresh).
pub async fn iterative_find_node(
    krpc: &Arc<KrpcSocket>,
    routing_table: &Arc<RwLock<RoutingTable>>,
    target: NodeId,
) {
    let initial_nodes = routing_table.read().await.closest_nodes(&target, K);
    let mut queried: HashSet<NodeId> = HashSet::new();
    let mut candidates: Vec<(NodeId, SocketAddr)> =
        initial_nodes.into_iter().map(|n| (n.id, n.addr)).collect();

    for _ in 0..30 {
        // Sort by distance to target
        candidates.sort_by_key(|(id, _)| id.distance(&target));

        let to_query: Vec<(NodeId, SocketAddr)> = candidates
            .iter()
            .filter(|(id, _)| !queried.contains(id))
            .take(ALPHA)
            .copied()
            .collect();

        if to_query.is_empty() {
            break;
        }

        for (id, _) in &to_query {
            queried.insert(*id);
        }

        let mut handles = Vec::new();
        for (_, addr) in to_query {
            let krpc = Arc::clone(krpc);
            let t = target;
            handles.push(tokio::spawn(async move {
                let result = krpc.query(addr, KrpcQuery::FindNode { target: t }).await;
                (addr, result)
            }));
        }

        let mut found_new = false;
        for handle in handles {
            if let Ok((resp_addr, Ok(resp))) = handle.await {
                // Handle both FindNode and GetPeers responses (decode can't distinguish)
                let (id, nodes) = match resp {
                    KrpcResponse::FindNode { id, nodes }
                    | KrpcResponse::GetPeers { id, nodes, .. } => (id, nodes),
                    KrpcResponse::Ping { id } | KrpcResponse::AnnouncePeer { id } => (id, vec![]),
                };
                routing_table.write().await.insert_or_update(id, resp_addr);
                routing_table.write().await.mark_good(&id);
                for (nid, naddr) in nodes {
                    routing_table.write().await.insert_or_update(nid, naddr);
                    if !candidates.iter().any(|(cid, _)| *cid == nid) {
                        candidates.push((nid, naddr));
                        found_new = true;
                    }
                }
            }
        }

        if !found_new {
            break; // Converged — no new closer nodes
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum CandidateState {
    NotQueried,
    InFlight,
    Responded,
    Failed,
}
