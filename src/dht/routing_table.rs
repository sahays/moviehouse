use std::net::SocketAddr;
use std::time::Instant;

/// 20-byte DHT node ID.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NodeId(pub [u8; 20]);

impl NodeId {
    pub fn random() -> Self {
        let mut id = [0u8; 20];
        rand::Rng::fill(&mut rand::thread_rng(), &mut id);
        Self(id)
    }

    /// XOR distance between two IDs.
    pub fn distance(&self, other: &NodeId) -> NodeId {
        let mut d = [0u8; 20];
        for i in 0..20 {
            d[i] = self.0[i] ^ other.0[i];
        }
        NodeId(d)
    }

    /// Leading zeros of XOR distance — determines bucket index.
    pub fn distance_leading_zeros(&self, other: &NodeId) -> u32 {
        let dist = self.distance(other);
        let mut zeros = 0u32;
        for byte in &dist.0 {
            if *byte == 0 {
                zeros += 8;
            } else {
                zeros += byte.leading_zeros();
                break;
            }
        }
        zeros
    }
}

const K: usize = 8;
const NUM_BUCKETS: usize = 160;

#[derive(Debug, Clone)]
pub struct NodeEntry {
    pub id: NodeId,
    pub addr: SocketAddr,
    pub last_seen: Instant,
    pub status: NodeStatus,
    pub failed_queries: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeStatus {
    Good,
    Questionable,
    Bad,
}

struct KBucket {
    nodes: Vec<NodeEntry>,
    replacement_cache: Vec<NodeEntry>,
    last_changed: Instant,
}

impl KBucket {
    fn new() -> Self {
        Self {
            nodes: Vec::with_capacity(K),
            replacement_cache: Vec::with_capacity(K),
            last_changed: Instant::now(),
        }
    }
}

/// Kademlia routing table with 160 k-buckets.
pub struct RoutingTable {
    own_id: NodeId,
    buckets: Vec<KBucket>,
}

pub enum InsertResult {
    Inserted,
    Updated,
    BucketFull,
}

impl RoutingTable {
    pub fn new(own_id: NodeId) -> Self {
        let mut buckets = Vec::with_capacity(NUM_BUCKETS);
        for _ in 0..NUM_BUCKETS {
            buckets.push(KBucket::new());
        }
        Self { own_id, buckets }
    }

    pub fn own_id(&self) -> &NodeId {
        &self.own_id
    }

    /// Get bucket index for a node ID.
    fn bucket_index(&self, id: &NodeId) -> usize {
        let zeros = self.own_id.distance_leading_zeros(id);
        // Bucket 0 = farthest, bucket 159 = closest
        if zeros >= NUM_BUCKETS as u32 {
            NUM_BUCKETS - 1
        } else {
            (NUM_BUCKETS - 1) - zeros as usize
        }
    }

    /// Insert or update a node in the routing table.
    pub fn insert_or_update(&mut self, id: NodeId, addr: SocketAddr) -> InsertResult {
        if id == self.own_id {
            return InsertResult::Updated;
        }

        let idx = self.bucket_index(&id);
        let bucket = &mut self.buckets[idx];

        // Check if already in bucket
        if let Some(node) = bucket.nodes.iter_mut().find(|n| n.id == id) {
            node.last_seen = Instant::now();
            node.addr = addr;
            node.status = NodeStatus::Good;
            node.failed_queries = 0;
            bucket.last_changed = Instant::now();
            return InsertResult::Updated;
        }

        // Bucket not full — insert
        if bucket.nodes.len() < K {
            bucket.nodes.push(NodeEntry {
                id,
                addr,
                last_seen: Instant::now(),
                status: NodeStatus::Good,
                failed_queries: 0,
            });
            bucket.last_changed = Instant::now();
            return InsertResult::Inserted;
        }

        // Bucket full — try to replace a bad node
        if let Some(bad_idx) = bucket.nodes.iter().position(|n| n.status == NodeStatus::Bad) {
            bucket.nodes[bad_idx] = NodeEntry {
                id,
                addr,
                last_seen: Instant::now(),
                status: NodeStatus::Good,
                failed_queries: 0,
            };
            bucket.last_changed = Instant::now();
            return InsertResult::Inserted;
        }

        // Add to replacement cache
        if bucket.replacement_cache.len() < K {
            bucket.replacement_cache.push(NodeEntry {
                id,
                addr,
                last_seen: Instant::now(),
                status: NodeStatus::Good,
                failed_queries: 0,
            });
        }

        InsertResult::BucketFull
    }

    /// Mark a node as good (responded to our query).
    pub fn mark_good(&mut self, id: &NodeId) {
        let idx = self.bucket_index(id);
        if let Some(node) = self.buckets[idx].nodes.iter_mut().find(|n| n.id == *id) {
            node.status = NodeStatus::Good;
            node.last_seen = Instant::now();
            node.failed_queries = 0;
        }
    }

    /// Mark a node as failed (didn't respond).
    pub fn mark_failed(&mut self, id: &NodeId) {
        let idx = self.bucket_index(id);
        if let Some(node) = self.buckets[idx].nodes.iter_mut().find(|n| n.id == *id) {
            node.failed_queries += 1;
            if node.failed_queries >= 3 {
                node.status = NodeStatus::Bad;
            } else {
                node.status = NodeStatus::Questionable;
            }
        }
    }

    /// Get the K closest nodes to a target.
    pub fn closest_nodes(&self, target: &NodeId, count: usize) -> Vec<NodeEntry> {
        let mut all_nodes: Vec<&NodeEntry> = self
            .buckets
            .iter()
            .flat_map(|b| b.nodes.iter())
            .filter(|n| n.status != NodeStatus::Bad)
            .collect();

        all_nodes.sort_by_key(|n| {
            let dist = n.id.distance(target);
            dist.0
        });

        all_nodes.into_iter().take(count).cloned().collect()
    }

    /// Total number of good nodes.
    pub fn node_count(&self) -> usize {
        self.buckets
            .iter()
            .flat_map(|b| b.nodes.iter())
            .filter(|n| n.status == NodeStatus::Good)
            .count()
    }

    /// Save non-bad nodes to a JSON file for persistent DHT bootstrap.
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        let nodes: Vec<serde_json::Value> = self.buckets.iter()
            .flat_map(|b| b.nodes.iter())
            .filter(|n| n.status != NodeStatus::Bad)
            .map(|n| serde_json::json!({
                "id": hex::encode(n.id.0),
                "addr": n.addr.to_string(),
            }))
            .collect();
        let data = serde_json::json!({
            "own_id": hex::encode(self.own_id.0),
            "nodes": nodes,
        });
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string(&data).unwrap())?;
        Ok(())
    }

    /// Load nodes from a previously saved JSON file.
    pub fn load_from_file(path: &std::path::Path) -> Option<(NodeId, Vec<(NodeId, SocketAddr)>)> {
        let data = std::fs::read_to_string(path).ok()?;
        let json: serde_json::Value = serde_json::from_str(&data).ok()?;

        let own_id_hex = json["own_id"].as_str()?;
        let own_id_bytes = hex::decode(own_id_hex).ok()?;
        if own_id_bytes.len() != 20 { return None; }
        let mut own_id = [0u8; 20];
        own_id.copy_from_slice(&own_id_bytes);

        let nodes: Vec<(NodeId, SocketAddr)> = json["nodes"].as_array()?
            .iter()
            .filter_map(|n| {
                let id_hex = n["id"].as_str()?;
                let id_bytes = hex::decode(id_hex).ok()?;
                if id_bytes.len() != 20 { return None; }
                let mut id = [0u8; 20];
                id.copy_from_slice(&id_bytes);
                let addr: SocketAddr = n["addr"].as_str()?.parse().ok()?;
                Some((NodeId(id), addr))
            })
            .collect();

        Some((NodeId(own_id), nodes))
    }
}
