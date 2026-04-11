use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use dashmap::DashMap;
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, oneshot};
use tracing::{trace, warn};

use crate::bencode::{self, BValue};
use crate::dht::routing_table::NodeId;
use crate::torrent::types::InfoHash;

/// Transaction ID counter.
static TXN_COUNTER: AtomicU16 = AtomicU16::new(0);

fn next_txn_id() -> [u8; 2] {
    let id = TXN_COUNTER.fetch_add(1, Ordering::Relaxed);
    id.to_be_bytes()
}

/// KRPC query types.
#[derive(Debug)]
pub enum KrpcQuery {
    Ping,
    FindNode {
        target: NodeId,
    },
    GetPeers {
        info_hash: InfoHash,
    },
    AnnouncePeer {
        info_hash: InfoHash,
        port: u16,
        token: Vec<u8>,
    },
}

/// KRPC response data.
#[derive(Debug)]
pub enum KrpcResponse {
    Ping {
        id: NodeId,
    },
    FindNode {
        id: NodeId,
        nodes: Vec<(NodeId, SocketAddr)>,
    },
    GetPeers {
        id: NodeId,
        token: Option<Vec<u8>>,
        peers: Vec<SocketAddr>,
        nodes: Vec<(NodeId, SocketAddr)>,
    },
    AnnouncePeer {
        id: NodeId,
    },
}

/// Inbound query from a remote node.
#[derive(Debug)]
pub struct InboundQuery {
    pub query: KrpcQuery,
    pub sender_id: NodeId,
    pub sender_addr: SocketAddr,
    pub txn_id: Vec<u8>,
}

/// KRPC socket for sending/receiving DHT messages.
pub struct KrpcSocket {
    socket: Arc<UdpSocket>,
    own_id: NodeId,
    /// Pending outbound queries awaiting responses.
    pending: Arc<DashMap<[u8; 2], oneshot::Sender<KrpcResponse>>>,
    /// Inbound queries from remote nodes.
    pub inbound_tx: mpsc::Sender<InboundQuery>,
    pub inbound_rx: Option<mpsc::Receiver<InboundQuery>>,
}

impl KrpcSocket {
    pub async fn bind(addr: SocketAddr, own_id: NodeId) -> std::io::Result<Self> {
        let socket = Arc::new(UdpSocket::bind(addr).await?);
        let (inbound_tx, inbound_rx) = mpsc::channel(256);
        Ok(Self {
            socket,
            own_id,
            pending: Arc::new(DashMap::new()),
            inbound_tx,
            inbound_rx: Some(inbound_rx),
        })
    }

    /// Take the inbound query receiver (can only be called once).
    pub fn take_inbound_rx(&mut self) -> Option<mpsc::Receiver<InboundQuery>> {
        self.inbound_rx.take()
    }

    /// Send a query and await the response (with timeout).
    pub async fn query(
        &self,
        dest: SocketAddr,
        query: KrpcQuery,
    ) -> Result<KrpcResponse, KrpcError> {
        let txn_id = next_txn_id();
        let msg = self.encode_query(txn_id, &query);

        let (tx, rx) = oneshot::channel();
        self.pending.insert(txn_id, tx);

        self.socket
            .send_to(&msg, dest)
            .await
            .map_err(KrpcError::Io)?;

        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => {
                self.pending.remove(&txn_id);
                Err(KrpcError::ChannelClosed)
            }
            Err(_) => {
                self.pending.remove(&txn_id);
                Err(KrpcError::Timeout)
            }
        }
    }

    /// Send a response to an inbound query.
    pub async fn respond(
        &self,
        dest: SocketAddr,
        txn_id: &[u8],
        response: KrpcResponse,
    ) -> Result<(), KrpcError> {
        let msg = self.encode_response(txn_id, &response);
        self.socket
            .send_to(&msg, dest)
            .await
            .map_err(KrpcError::Io)?;
        Ok(())
    }

    /// Run the receive loop. Should be spawned as a background task.
    pub async fn recv_loop(self: Arc<Self>) {
        let mut buf = vec![0u8; 8192];
        loop {
            match self.socket.recv_from(&mut buf).await {
                Ok((n, addr)) => {
                    let data = &buf[..n];
                    if let Err(e) = self.handle_message(data, addr).await {
                        trace!(peer = %addr, error = %e, "Failed to handle KRPC message");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "UDP recv error");
                    break;
                }
            }
        }
    }

    async fn handle_message(&self, data: &[u8], addr: SocketAddr) -> Result<(), KrpcError> {
        let val = bencode::decode(data).map_err(|e| KrpcError::Decode(e.to_string()))?;

        let msg_type = val
            .get_str("y")
            .and_then(|v| v.as_str())
            .ok_or(KrpcError::InvalidMessage)?;

        let txn_id = val
            .get_str("t")
            .and_then(|v| v.as_bytes())
            .ok_or(KrpcError::InvalidMessage)?;

        match msg_type {
            "r" => {
                // Response to our query
                if txn_id.len() == 2 {
                    let key = [txn_id[0], txn_id[1]];
                    if let Some((_, tx)) = self.pending.remove(&key)
                        && let Some(response) = self.decode_response(&val)
                    {
                        let _ = tx.send(response);
                    }
                }
            }
            "q" => {
                // Inbound query
                if let Some(query) = self.decode_query(&val) {
                    let sender_id = val
                        .get_str("a")
                        .and_then(|a| a.get_str("id"))
                        .and_then(|v| v.as_bytes())
                        .and_then(|b| {
                            if b.len() == 20 {
                                let mut id = [0u8; 20];
                                id.copy_from_slice(b);
                                Some(NodeId(id))
                            } else {
                                None
                            }
                        })
                        .unwrap_or(NodeId([0; 20]));

                    let _ = self
                        .inbound_tx
                        .send(InboundQuery {
                            query,
                            sender_id,
                            sender_addr: addr,
                            txn_id: txn_id.to_vec(),
                        })
                        .await;
                }
            }
            "e" => {
                // Error response
                if txn_id.len() == 2 {
                    let key = [txn_id[0], txn_id[1]];
                    self.pending.remove(&key);
                }
            }
            _ => {}
        }

        Ok(())
    }

    // Method takes &self for API consistency even though only own_id is needed
    #[allow(clippy::unused_self)]
    fn encode_query(&self, txn_id: [u8; 2], query: &KrpcQuery) -> Vec<u8> {
        use std::collections::BTreeMap;

        let mut dict = BTreeMap::new();
        dict.insert(b"t".to_vec(), BValue::Bytes(txn_id.to_vec()));
        dict.insert(b"y".to_vec(), BValue::Bytes(b"q".to_vec()));

        let mut args = BTreeMap::new();
        args.insert(b"id".to_vec(), BValue::Bytes(self.own_id.0.to_vec()));

        match query {
            KrpcQuery::Ping => {
                dict.insert(b"q".to_vec(), BValue::Bytes(b"ping".to_vec()));
            }
            KrpcQuery::FindNode { target } => {
                dict.insert(b"q".to_vec(), BValue::Bytes(b"find_node".to_vec()));
                args.insert(b"target".to_vec(), BValue::Bytes(target.0.to_vec()));
            }
            KrpcQuery::GetPeers { info_hash } => {
                dict.insert(b"q".to_vec(), BValue::Bytes(b"get_peers".to_vec()));
                args.insert(b"info_hash".to_vec(), BValue::Bytes(info_hash.0.to_vec()));
            }
            KrpcQuery::AnnouncePeer {
                info_hash,
                port,
                token,
            } => {
                dict.insert(b"q".to_vec(), BValue::Bytes(b"announce_peer".to_vec()));
                args.insert(b"info_hash".to_vec(), BValue::Bytes(info_hash.0.to_vec()));
                args.insert(b"port".to_vec(), BValue::Int(*port as i64));
                args.insert(b"token".to_vec(), BValue::Bytes(token.clone()));
            }
        }

        dict.insert(b"a".to_vec(), BValue::Dict(args));
        bencode::encode(&BValue::Dict(dict))
    }

    fn encode_response(&self, txn_id: &[u8], response: &KrpcResponse) -> Vec<u8> {
        use std::collections::BTreeMap;

        let mut dict = BTreeMap::new();
        dict.insert(b"t".to_vec(), BValue::Bytes(txn_id.to_vec()));
        dict.insert(b"y".to_vec(), BValue::Bytes(b"r".to_vec()));

        let mut r = BTreeMap::new();
        r.insert(b"id".to_vec(), BValue::Bytes(self.own_id.0.to_vec()));

        match response {
            KrpcResponse::Ping { .. } | KrpcResponse::AnnouncePeer { .. } => {}
            KrpcResponse::FindNode { nodes, .. } => {
                r.insert(
                    b"nodes".to_vec(),
                    BValue::Bytes(encode_compact_nodes(nodes)),
                );
            }
            KrpcResponse::GetPeers {
                token,
                peers,
                nodes,
                ..
            } => {
                if let Some(token) = token {
                    r.insert(b"token".to_vec(), BValue::Bytes(token.clone()));
                }
                if !peers.is_empty() {
                    let values: Vec<BValue> = peers
                        .iter()
                        .map(|addr| BValue::Bytes(encode_compact_peer(addr)))
                        .collect();
                    r.insert(b"values".to_vec(), BValue::List(values));
                }
                if !nodes.is_empty() {
                    r.insert(
                        b"nodes".to_vec(),
                        BValue::Bytes(encode_compact_nodes(nodes)),
                    );
                }
            }
        }

        dict.insert(b"r".to_vec(), BValue::Dict(r));
        bencode::encode(&BValue::Dict(dict))
    }

    #[allow(clippy::unused_self)]
    fn decode_response(&self, val: &BValue) -> Option<KrpcResponse> {
        let r = val.get_str("r")?;
        let id_bytes = r.get_str("id")?.as_bytes()?;
        if id_bytes.len() != 20 {
            return None;
        }
        let mut id = [0u8; 20];
        id.copy_from_slice(id_bytes);
        let id = NodeId(id);

        // Check for nodes (find_node or get_peers without values)
        let nodes = r
            .get_str("nodes")
            .and_then(|v| v.as_bytes())
            .map(decode_compact_nodes)
            .unwrap_or_default();

        // Check for values (get_peers with peers)
        let peers: Vec<SocketAddr> = r
            .get_str("values")
            .and_then(|v| v.as_list())
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_bytes())
                    .filter_map(decode_compact_peer)
                    .collect()
            })
            .unwrap_or_default();

        let token = r
            .get_str("token")
            .and_then(|v| v.as_bytes())
            .map(<[u8]>::to_vec);

        if !peers.is_empty() || !nodes.is_empty() || token.is_some() {
            Some(KrpcResponse::GetPeers {
                id,
                token,
                peers,
                nodes,
            })
        } else {
            Some(KrpcResponse::Ping { id })
        }
    }

    #[allow(clippy::unused_self)]
    fn decode_query(&self, val: &BValue) -> Option<KrpcQuery> {
        let q = val.get_str("q")?.as_str()?;
        let args = val.get_str("a")?;

        match q {
            "ping" => Some(KrpcQuery::Ping),
            "find_node" => {
                let target_bytes = args.get_str("target")?.as_bytes()?;
                if target_bytes.len() != 20 {
                    return None;
                }
                let mut target = [0u8; 20];
                target.copy_from_slice(target_bytes);
                Some(KrpcQuery::FindNode {
                    target: NodeId(target),
                })
            }
            "get_peers" => {
                let ih_bytes = args.get_str("info_hash")?.as_bytes()?;
                if ih_bytes.len() != 20 {
                    return None;
                }
                let mut ih = [0u8; 20];
                ih.copy_from_slice(ih_bytes);
                Some(KrpcQuery::GetPeers {
                    info_hash: InfoHash::from_bytes(ih),
                })
            }
            "announce_peer" => {
                let ih_bytes = args.get_str("info_hash")?.as_bytes()?;
                if ih_bytes.len() != 20 {
                    return None;
                }
                let mut ih = [0u8; 20];
                ih.copy_from_slice(ih_bytes);
                let port = args.get_str("port")?.as_int()? as u16;
                let token = args.get_str("token")?.as_bytes()?.to_vec();
                Some(KrpcQuery::AnnouncePeer {
                    info_hash: InfoHash::from_bytes(ih),
                    port,
                    token,
                })
            }
            _ => None,
        }
    }
}

// Compact encoding helpers

fn encode_compact_nodes(nodes: &[(NodeId, SocketAddr)]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(nodes.len() * 26);
    for (id, addr) in nodes {
        match addr {
            SocketAddr::V4(v4) => {
                buf.extend_from_slice(&id.0);
                buf.extend_from_slice(&v4.ip().octets());
                buf.extend_from_slice(&v4.port().to_be_bytes());
            }
            SocketAddr::V6(_) => {} // skip entire entry for IPv6 nodes
        }
    }
    buf
}

fn decode_compact_nodes(data: &[u8]) -> Vec<(NodeId, SocketAddr)> {
    data.chunks_exact(26)
        .map(|chunk| {
            let mut id = [0u8; 20];
            id.copy_from_slice(&chunk[..20]);
            let ip = std::net::Ipv4Addr::new(chunk[20], chunk[21], chunk[22], chunk[23]);
            let port = u16::from_be_bytes([chunk[24], chunk[25]]);
            (
                NodeId(id),
                SocketAddr::V4(std::net::SocketAddrV4::new(ip, port)),
            )
        })
        .collect()
}

fn encode_compact_peer(addr: &SocketAddr) -> Vec<u8> {
    match addr {
        SocketAddr::V4(v4) => {
            let mut buf = Vec::with_capacity(6);
            buf.extend_from_slice(&v4.ip().octets());
            buf.extend_from_slice(&v4.port().to_be_bytes());
            buf
        }
        SocketAddr::V6(_) => vec![],
    }
}

fn decode_compact_peer(data: &[u8]) -> Option<SocketAddr> {
    if data.len() == 6 {
        let ip = std::net::Ipv4Addr::new(data[0], data[1], data[2], data[3]);
        let port = u16::from_be_bytes([data[4], data[5]]);
        Some(SocketAddr::V4(std::net::SocketAddrV4::new(ip, port)))
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    #[test]
    fn test_compact_nodes_ipv4_only() {
        let nodes = vec![
            (
                NodeId([1u8; 20]),
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6881)),
            ),
            (
                NodeId([2u8; 20]),
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 8080)),
            ),
            (
                NodeId([3u8; 20]),
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 443)),
            ),
        ];
        let encoded = encode_compact_nodes(&nodes);
        assert_eq!(encoded.len(), nodes.len() * 26);
    }

    #[test]
    fn test_compact_nodes_skips_ipv6() {
        let nodes = vec![
            (
                NodeId([1u8; 20]),
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 6881)),
            ),
            (
                NodeId([2u8; 20]),
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 6882, 0, 0)),
            ),
            (
                NodeId([3u8; 20]),
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 1), 443)),
            ),
            (
                NodeId([4u8; 20]),
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 9999, 0, 0)),
            ),
        ];
        let num_ipv4 = 2;
        let encoded = encode_compact_nodes(&nodes);
        assert_eq!(encoded.len(), num_ipv4 * 26);
    }

    #[test]
    fn test_compact_nodes_all_ipv6() {
        let nodes = vec![
            (
                NodeId([1u8; 20]),
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 6881, 0, 0)),
            ),
            (
                NodeId([2u8; 20]),
                SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 6882, 0, 0)),
            ),
        ];
        let encoded = encode_compact_nodes(&nodes);
        assert!(encoded.is_empty());
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KrpcError {
    #[error("IO error: {0}")]
    Io(std::io::Error),
    #[error("decode error: {0}")]
    Decode(String),
    #[error("invalid message")]
    InvalidMessage,
    #[error("timeout")]
    Timeout,
    #[error("channel closed")]
    ChannelClosed,
}
