use std::collections::{HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::time::Instant;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

use super::connection::{PeerCommand, PeerEvent, run_peer_connection};
use crate::torrent::types::{InfoHash, PeerId};

/// Maximum concurrent outbound connection attempts.
const HALF_OPEN_LIMIT: usize = 20;

/// Manages the pool of peer connections for a torrent.
pub struct PeerManager {
    info_hash: InfoHash,
    our_peer_id: PeerId,
    max_peers: usize,

    /// Active peer connections: addr → command sender
    peers: HashMap<SocketAddr, PeerState>,

    /// Peers waiting to be connected
    pending_peers: VecDeque<SocketAddr>,

    /// Peers we've already tried (avoid reconnecting too fast)
    known_addrs: HashSet<SocketAddr>,

    /// Currently connecting (half-open)
    connecting: HashSet<SocketAddr>,

    /// Channel to receive events from all peer connections
    event_tx: mpsc::Sender<(SocketAddr, PeerEvent)>,
    pub event_rx: mpsc::Receiver<(SocketAddr, PeerEvent)>,

    cancel: CancellationToken,
}

pub struct PeerState {
    pub cmd_tx: mpsc::Sender<PeerCommand>,
    pub am_interested: bool,
    pub peer_choking: bool,
    pub connected_at: Instant,
    pub bytes_downloaded: u64,
    pub outstanding_requests: u32,
    pub supports_extensions: bool,
    pub remote_ext_handshake: Option<super::extension::ExtendedHandshake>,
    pub throughput_sample_bytes: u64,
    pub throughput_sample_time: Instant,
    pub throughput_mibps: f64,
}

impl PeerManager {
    pub fn new(
        info_hash: InfoHash,
        our_peer_id: PeerId,
        max_peers: usize,
        cancel: CancellationToken,
    ) -> Self {
        let (event_tx, event_rx) = mpsc::channel(512);
        Self {
            info_hash,
            our_peer_id,
            max_peers,
            peers: HashMap::new(),
            pending_peers: VecDeque::new(),
            known_addrs: HashSet::new(),
            connecting: HashSet::new(),
            event_tx,
            event_rx,
            cancel,
        }
    }

    /// Add discovered peer addresses to the pending queue.
    pub fn add_peers(&mut self, addrs: impl Iterator<Item = SocketAddr>) {
        for addr in addrs {
            if !self.known_addrs.contains(&addr) {
                self.known_addrs.insert(addr);
                self.pending_peers.push_back(addr);
            }
        }
    }

    /// Re-queue a known peer for reconnection (e.g., after disconnect).
    pub fn requeue_peer(&mut self, addr: SocketAddr) {
        if !self.peers.contains_key(&addr) && !self.connecting.contains(&addr) {
            self.pending_peers.push_back(addr);
        }
    }

    /// Spawn new connections up to the limit.
    pub fn connect_pending(&mut self) {
        while self.peers.len() + self.connecting.len() < self.max_peers
            && self.connecting.len() < HALF_OPEN_LIMIT
        {
            let Some(addr) = self.pending_peers.pop_front() else {
                break;
            };

            if self.peers.contains_key(&addr) || self.connecting.contains(&addr) {
                continue;
            }

            self.spawn_connection(addr);
        }
    }

    fn spawn_connection(&mut self, addr: SocketAddr) {
        let (cmd_tx, cmd_rx) = mpsc::channel(512);
        let event_tx = self.event_tx.clone();
        let info_hash = self.info_hash;
        let our_peer_id = self.our_peer_id;
        let cancel = self.cancel.child_token();

        self.connecting.insert(addr);

        tokio::spawn(async move {
            run_peer_connection(addr, info_hash, our_peer_id, event_tx, cmd_rx, cancel).await;
        });

        // Store the command sender so we can send commands later
        // (we store it in `peers` once we get the Connected event)
        // For now, store it temporarily so we don't lose the tx
        self.peers.insert(
            addr,
            PeerState {
                cmd_tx,
                am_interested: false,
                peer_choking: true,
                connected_at: Instant::now(),
                bytes_downloaded: 0,
                outstanding_requests: 0,
                supports_extensions: false,
                remote_ext_handshake: None,
                throughput_sample_bytes: 0,
                throughput_sample_time: Instant::now(),
                throughput_mibps: 0.0,
            },
        );
    }

    /// Handle a peer event. Returns the event for further processing by the session.
    pub fn handle_event(&mut self, addr: SocketAddr, event: &PeerEvent) {
        match event {
            PeerEvent::Connected {
                supports_extensions,
                ..
            } => {
                self.connecting.remove(&addr);
                if let Some(state) = self.peers.get_mut(&addr) {
                    state.supports_extensions = *supports_extensions;
                }
                debug!(peer = %addr, "Peer connected (total: {})", self.peers.len());
            }
            PeerEvent::Unchoked => {
                if let Some(state) = self.peers.get_mut(&addr) {
                    state.peer_choking = false;
                }
            }
            PeerEvent::Choked => {
                if let Some(state) = self.peers.get_mut(&addr) {
                    state.peer_choking = true;
                }
            }
            PeerEvent::BlockReceived { data, .. } => {
                if let Some(state) = self.peers.get_mut(&addr) {
                    state.bytes_downloaded += data.len() as u64;
                }
            }
            PeerEvent::ExtendedHandshake(hs) => {
                if let Some(state) = self.peers.get_mut(&addr) {
                    state.remote_ext_handshake = Some(hs.clone());
                }
            }
            PeerEvent::Disconnected { reason } => {
                self.peers.remove(&addr);
                self.connecting.remove(&addr);
                debug!(peer = %addr, reason = %reason, "Peer disconnected (total: {})", self.peers.len());
            }
            _ => {}
        }
    }

    /// Send a command to a specific peer.
    pub fn send_command(&self, addr: &SocketAddr, cmd: PeerCommand) -> bool {
        if let Some(state) = self.peers.get(addr) {
            state.cmd_tx.try_send(cmd).is_ok()
        } else {
            false
        }
    }

    /// Send a command to all connected peers.
    pub fn broadcast(&self, cmd_fn: impl Fn() -> PeerCommand) {
        for state in self.peers.values() {
            let _ = state.cmd_tx.try_send(cmd_fn());
        }
    }

    /// Get all connected peer addresses.
    pub fn connected_peers(&self) -> Vec<SocketAddr> {
        self.peers.keys().copied().collect()
    }

    /// Number of active connections.
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// Get mutable state for a peer.
    pub fn peer_state_mut(&mut self, addr: &SocketAddr) -> Option<&mut PeerState> {
        self.peers.get_mut(addr)
    }

    /// Get immutable state for a peer.
    pub fn peer_state(&self, addr: &SocketAddr) -> Option<&PeerState> {
        self.peers.get(addr)
    }

    /// Get all peers that are unchoked and we're interested in (can request from).
    pub fn downloadable_peers(&self) -> Vec<SocketAddr> {
        self.peers
            .iter()
            .filter(|(_, s)| s.am_interested && !s.peer_choking)
            .map(|(a, _)| *a)
            .collect()
    }
}
