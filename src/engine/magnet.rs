use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::dht::node::DhtHandle;
use crate::peer::connection::{PeerCommand, PeerEvent};
use crate::peer::extension::{ExtendedHandshake, MetadataMessage};
use crate::peer::manager::PeerManager;
use crate::torrent::magnet::MagnetLink;
use crate::torrent::metainfo::Metainfo;
use crate::torrent::types::PeerId;
use crate::tracker::manager::TrackerManager;

use super::magnet_buffer::MetadataBuffer;

// --- State Machine ---

enum State {
    AwaitingSize,
    Downloading(MetadataBuffer),
}

// --- Per-peer extension state ---

struct PeerMeta {
    ut_metadata_id: u8,
}

// --- Orchestrator ---

#[allow(clippy::too_many_lines)]
pub async fn download_metadata(
    magnet: &MagnetLink,
    our_peer_id: PeerId,
    port: u16,
    max_peers: usize,
    no_dht: bool,
    lightspeed: bool,
    cancel: CancellationToken,
) -> anyhow::Result<(Metainfo, Vec<SocketAddr>)> {
    let info_hash = magnet.info_hash;
    let name = magnet.display_name.as_deref().unwrap_or("unknown");
    eprintln!("Magnet: {info_hash} ({name})");

    let mut peer_manager = PeerManager::new(info_hash, our_peer_id, max_peers, cancel.clone());
    let mut peer_ext: HashMap<SocketAddr, PeerMeta> = HashMap::new();
    let mut state = State::AwaitingSize;

    // Peer discovery
    let (peer_tx, mut peer_rx) = mpsc::channel::<Vec<SocketAddr>>(64);

    if !magnet.trackers.is_empty() {
        let tm = TrackerManager::new(
            info_hash,
            our_peer_id,
            port,
            magnet.trackers.clone(),
            peer_tx.clone(),
            cancel.clone(),
        );
        tokio::spawn(async move {
            tm.run(0).await;
        });
    }

    if !no_dht {
        let dht_addr: SocketAddr = SocketAddr::from(([0, 0, 0, 0], 0));
        if let Ok(dht) = DhtHandle::start(dht_addr, cancel.clone(), lightspeed).await {
            let tx = peer_tx.clone();
            let c = cancel.clone();
            tokio::spawn(async move {
                loop {
                    let mut rx = dht.get_peers(info_hash).await;
                    while let Some(peers) = rx.recv().await {
                        if !peers.is_empty() {
                            let _ = tx.send(peers).await;
                        }
                    }
                    tokio::select! {
                        () = c.cancelled() => return,
                        () = tokio::time::sleep(Duration::from_secs(15)) => {}
                    }
                }
            });
            eprintln!("DHT started");
        }
    }
    drop(peer_tx);

    let mut connect_interval = tokio::time::interval(Duration::from_secs(2));
    connect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    // Event loop: discover peers, download metadata
    loop {
        tokio::select! {
            () = cancel.cancelled() => anyhow::bail!("cancelled"),

            Some(new_peers) = peer_rx.recv() => {
                peer_manager.add_peers(new_peers.into_iter());
                peer_manager.connect_pending();
            }

            Some((addr, event)) = peer_manager.event_rx.recv() => {
                peer_manager.handle_event(addr, &event);

                match event {
                    PeerEvent::Connected { supports_extensions, .. } => {
                        if supports_extensions {
                            peer_manager.send_command(&addr, PeerCommand::SendInterested);
                            peer_manager.send_command(
                                &addr,
                                PeerCommand::SendExtendedHandshake(ExtendedHandshake::ours(None, lightspeed)),
                            );
                        }
                    }

                    PeerEvent::ExtendedHandshake(hs) => {
                        const MAX_METADATA_SIZE: u64 = 10 * 1024 * 1024; // 10 MiB
                        if let (Some(ext_id), Some(size)) = (hs.extension_id("ut_metadata"), hs.metadata_size) {
                            if size == 0 || size > MAX_METADATA_SIZE {
                                eprintln!("Peer reported invalid metadata size: {size}, skipping");
                                continue;
                            }
                            peer_ext.insert(addr, PeerMeta { ut_metadata_id: ext_id });

                            // Initialize buffer on first size report
                            if matches!(state, State::AwaitingSize) {
                                eprintln!("Metadata size: {size} bytes");
                                state = State::Downloading(MetadataBuffer::new(size as usize));
                            }

                            // Request pieces from this peer
                            request_metadata_pieces(&addr, &mut state, &peer_ext, &peer_manager);
                        }
                    }

                    PeerEvent::MetadataMessage(msg) => {
                        match msg {
                            MetadataMessage::Data { piece, data, .. } => {
                                if let State::Downloading(ref mut buf) = state {
                                    let complete = buf.on_data(piece, &data);
                                    eprintln!("Metadata piece {piece}/{} received", buf.num_pieces);
                                    if complete {
                                        break; // all pieces received
                                    }
                                }
                                // Request more pieces
                                request_metadata_pieces(&addr, &mut state, &peer_ext, &peer_manager);
                            }
                            MetadataMessage::Reject { piece } => {
                                if let State::Downloading(ref mut buf) = state {
                                    buf.on_reject(piece);
                                }
                            }
                            MetadataMessage::Request { .. } => {} // we don't serve metadata
                        }
                    }

                    PeerEvent::Disconnected { .. } => {
                        peer_ext.remove(&addr);
                        if let State::Downloading(ref mut buf) = state {
                            buf.on_peer_lost(&addr);
                        }
                    }

                    _ => {}
                }
            }

            _ = connect_interval.tick() => {
                peer_manager.connect_pending();
            }
        }
    }

    // Collect connected peers before we drop the manager
    let warm_peers = peer_manager.connected_peers();

    // Verify metadata
    let State::Downloading(buf) = state else {
        anyhow::bail!("metadata download did not complete");
    };

    let raw_info = buf
        .verify(&info_hash)
        .ok_or_else(|| anyhow::anyhow!("metadata hash verification failed"))?;

    eprintln!("Metadata verified");

    let metainfo = Metainfo::from_info_bytes(&raw_info, info_hash, magnet.trackers.clone())
        .map_err(|e| anyhow::anyhow!("failed to parse metadata: {e}"))?;

    Ok((metainfo, warm_peers))
}

fn request_metadata_pieces(
    addr: &SocketAddr,
    state: &mut State,
    peer_ext: &HashMap<SocketAddr, PeerMeta>,
    peer_manager: &PeerManager,
) {
    let State::Downloading(buf) = state else {
        return;
    };
    let Some(meta) = peer_ext.get(addr) else {
        return;
    };

    while let Some(piece) = buf.next_request(*addr) {
        if !peer_manager.send_command(
            addr,
            PeerCommand::SendMetadataRequest {
                ext_id: meta.ut_metadata_id,
                piece,
            },
        ) {
            buf.on_reject(piece); // couldn't send, unassign
            break;
        }
    }
}
