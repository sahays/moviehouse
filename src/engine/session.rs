use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::dht::node::DhtHandle;
use crate::disk::io::create_disk_manager;
use crate::disk::mapping::FileMapping;
use crate::peer::connection::{PeerCommand, PeerEvent};
use crate::peer::extension::ExtendedHandshake;
use crate::peer::manager::PeerManager;
use crate::piece::bitfield::Bitfield;
use crate::piece::picker::{BlockRequest, BlockResult, PiecePicker};
use crate::piece::store::PieceStore;
use crate::torrent::metainfo::Metainfo;
use crate::torrent::types::PeerId;
use crate::tracker::manager::TrackerManager;

use super::choker;

fn max_pipeline(lightspeed: bool, peer_throughput: f64) -> u32 {
    if !lightspeed {
        return 64;
    }
    // Start at 64 (baseline), scale up for fast peers. Never below baseline.
    let adaptive = (peer_throughput * 8.0) as u32;
    adaptive.clamp(64, 256)
}

pub struct TorrentSession {
    metainfo: Arc<Metainfo>,
    our_peer_id: PeerId,
    port: u16,
    max_peers: usize,
    output_dir: std::path::PathBuf,
    no_dht: bool,
    lightspeed: bool,
    cancel: CancellationToken,
    warm_peers: Vec<SocketAddr>,
}

impl TorrentSession {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        metainfo: Metainfo,
        our_peer_id: PeerId,
        port: u16,
        max_peers: usize,
        output_dir: std::path::PathBuf,
        no_dht: bool,
        lightspeed: bool,
        cancel: CancellationToken,
        warm_peers: Vec<SocketAddr>,
    ) -> Self {
        Self {
            metainfo: Arc::new(metainfo),
            our_peer_id,
            port,
            max_peers,
            output_dir,
            no_dht,
            lightspeed,
            cancel,
            warm_peers,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let info = &self.metainfo.info;
        let num_pieces = info.pieces.len();

        eprintln!(
            "Torrent: {} ({:.2} MiB, {} pieces)",
            info.name,
            info.total_length as f64 / (1024.0 * 1024.0),
            num_pieces,
        );

        if self.lightspeed {
            eprintln!("Lightspeed mode enabled");
        }

        // Subsystems
        let file_mapping = FileMapping::new(info, &self.output_dir);
        let (disk_handle, disk_manager) =
            create_disk_manager(file_mapping, self.cancel.clone(), self.lightspeed);
        let piece_store = PieceStore::new(info.pieces.clone());
        let mut picker = PiecePicker::new(num_pieces, info.piece_length, info.total_length);
        let mut peer_manager = PeerManager::new(
            self.metainfo.info_hash,
            self.our_peer_id,
            self.max_peers,
            self.cancel.clone(),
        );

        let mut peer_bitfields: HashMap<SocketAddr, Bitfield> = HashMap::new();
        // Per-peer outstanding request tracking
        let mut peer_pending: HashMap<SocketAddr, Vec<BlockRequest>> = HashMap::new();

        // Add warm peers from magnet phase
        if !self.warm_peers.is_empty() {
            peer_manager.add_peers(self.warm_peers.iter().copied());
        }

        disk_handle.pre_allocate().await;

        let disk_task: JoinHandle<()> = tokio::spawn(async move {
            disk_manager.run().await;
        });

        // Peer discovery -- shared by tracker and DHT
        let (peer_tx, mut peer_rx) = mpsc::channel::<Vec<SocketAddr>>(64);

        let tracker_urls = self.metainfo.tracker_urls();
        if !tracker_urls.is_empty() {
            eprintln!("Trackers: {}", tracker_urls.join(", "));
        }
        let tracker_manager = TrackerManager::new(
            self.metainfo.info_hash,
            self.our_peer_id,
            self.port,
            tracker_urls,
            peer_tx.clone(),
            self.cancel.clone(),
        );
        let total_length = info.total_length;
        tokio::spawn(async move {
            tracker_manager.run(total_length).await;
        });

        if !self.no_dht {
            let dht_addr: SocketAddr = "0.0.0.0:0".parse().unwrap();
            match DhtHandle::start(dht_addr, self.cancel.clone(), self.lightspeed).await {
                Ok(dht_handle) => {
                    let info_hash = self.metainfo.info_hash;
                    let dht_peer_tx = peer_tx.clone();
                    let cancel = self.cancel.clone();
                    tokio::spawn(async move {
                        loop {
                            let mut rx = dht_handle.get_peers(info_hash).await;
                            loop {
                                tokio::select! {
                                    _ = cancel.cancelled() => return,
                                    result = rx.recv() => {
                                        match result {
                                            Some(peers) if !peers.is_empty() => {
                                                let _ = dht_peer_tx.send(peers).await;
                                            }
                                            None => break,
                                            _ => {}
                                        }
                                    }
                                }
                            }
                            tokio::select! {
                                _ = cancel.cancelled() => return,
                                _ = tokio::time::sleep(Duration::from_secs(15)) => {}
                            }
                        }
                    });
                    eprintln!("DHT started");
                }
                Err(e) => eprintln!("DHT failed: {e}"),
            }
        }
        drop(peer_tx);

        // Progress bar
        let progress = ProgressBar::new(info.total_length);
        progress.set_style(
            ProgressStyle::with_template(
                "[{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}) peers:{msg} ETA {eta}",
            )
            .unwrap()
            .progress_chars("=>-"),
        );
        progress.set_message("0");
        progress.enable_steady_tick(Duration::from_millis(250));

        let mut choke_interval = tokio::time::interval(Duration::from_secs(10));
        choke_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut connect_interval = tokio::time::interval(Duration::from_secs(2));
        connect_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut optimistic_counter = 0u32;

        let mut pending_writes: Vec<JoinHandle<()>> = Vec::new();
        let start_time = Instant::now();
        let mut bytes_new: u64 = 0;

        // Speed tracking -- sample once per second
        let mut speed_samples: Vec<f64> = Vec::new();
        let mut peak_speed: f64 = 0.0;
        let mut min_speed: f64 = f64::MAX;
        let mut last_sample_time = Instant::now();
        let mut last_sample_bytes: u64 = 0;
        let mut speed_tick = tokio::time::interval(Duration::from_secs(1));
        speed_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        let lightspeed = self.lightspeed;

        // Main event loop
        loop {
            if picker.is_complete() {
                break;
            }

            tokio::select! {
                _ = self.cancel.cancelled() => break,

                Some(new_peers) = peer_rx.recv() => {
                    let count = new_peers.len();
                    if count >= 10 {
                        progress.println(format!("Discovered {count} new peers"));
                    }
                    peer_manager.add_peers(new_peers.into_iter());
                    peer_manager.connect_pending();
                }

                Some((addr, event)) = peer_manager.event_rx.recv() => {
                    peer_manager.handle_event(addr, &event);

                    match event {
                        PeerEvent::Connected { supports_extensions, .. } => {
                            let pc = peer_manager.peer_count();
                            if pc <= 5 || pc.is_multiple_of(10) {
                                progress.println(format!("Peers: {pc} connected"));
                            }
                            peer_bitfields.insert(addr, Bitfield::new(num_pieces));
                            peer_pending.insert(addr, Vec::new());
                            progress.set_message(format!("{pc}"));

                            peer_manager.send_command(&addr, PeerCommand::SendInterested);
                            if let Some(s) = peer_manager.peer_state_mut(&addr) {
                                s.am_interested = true;
                            }

                            if supports_extensions {
                                peer_manager.send_command(
                                    &addr,
                                    PeerCommand::SendExtendedHandshake(ExtendedHandshake::ours(None, lightspeed)),
                                );
                            }
                        }

                        PeerEvent::BitfieldReceived(bf_bytes) => {
                            let bf = Bitfield::from_bytes(&bf_bytes, num_pieces);
                            picker.peer_has_bitfield(&bf);
                            peer_bitfields.insert(addr, bf);
                            fill_pipeline(&addr, &mut peer_manager, &peer_bitfields, &mut picker, &mut peer_pending, lightspeed);
                        }

                        PeerEvent::HaveAll => {
                            let mut bf = Bitfield::new(num_pieces);
                            for i in 0..num_pieces { bf.set(i); }
                            picker.peer_has_bitfield(&bf);
                            peer_bitfields.insert(addr, bf);
                            fill_pipeline(&addr, &mut peer_manager, &peer_bitfields, &mut picker, &mut peer_pending, lightspeed);
                        }

                        PeerEvent::Have { piece_index } => {
                            picker.peer_has_piece(piece_index);
                            if let Some(bf) = peer_bitfields.get_mut(&addr) {
                                bf.set(piece_index as usize);
                            }
                            fill_pipeline(&addr, &mut peer_manager, &peer_bitfields, &mut picker, &mut peer_pending, lightspeed);
                        }

                        PeerEvent::Unchoked => {
                            fill_pipeline(&addr, &mut peer_manager, &peer_bitfields, &mut picker, &mut peer_pending, lightspeed);
                        }

                        PeerEvent::Choked => {
                            release_peer_blocks(&addr, &mut peer_pending, &mut picker);
                            if let Some(s) = peer_manager.peer_state_mut(&addr) {
                                s.outstanding_requests = 0;
                            }
                            // Freed blocks -> refill other peers' pipelines
                            refill_all_peers(&mut peer_manager, &peer_bitfields, &mut picker, &mut peer_pending, lightspeed);
                        }

                        PeerEvent::BlockReceived { piece_index, offset, data } => {
                            // Remove from peer's pending list
                            if let Some(pending) = peer_pending.get_mut(&addr) {
                                pending.retain(|b| !(b.piece_index == piece_index && b.offset == offset));
                            }
                            if let Some(s) = peer_manager.peer_state_mut(&addr) {
                                s.outstanding_requests = s.outstanding_requests.saturating_sub(1);
                            }

                            // Throughput sampling
                            if let Some(s) = peer_manager.peer_state_mut(&addr) {
                                s.throughput_sample_bytes += data.len() as u64;
                                let elapsed = s.throughput_sample_time.elapsed().as_secs_f64();
                                if elapsed >= 5.0 {
                                    s.throughput_mibps = s.throughput_sample_bytes as f64 / elapsed / (1024.0 * 1024.0);
                                    s.throughput_sample_bytes = 0;
                                    s.throughput_sample_time = Instant::now();
                                }
                            }

                            // Process block
                            let result = picker.block_received(piece_index, offset, &data);

                            match result {
                                BlockResult::Progress { new_bytes } => {
                                    bytes_new += new_bytes as u64;
                                }
                                BlockResult::PieceComplete(piece_data) => {
                                    bytes_new += data.len() as u64;
                                    if piece_store.verify(piece_index, &piece_data) {
                                        // Mark verified in picker BEFORE writing (so is_complete is honest)
                                        picker.mark_verified(piece_index);

                                        let verified = picker.pieces_done();
                                        let bytes_verified = verified as u64 * info.piece_length as u64;
                                        progress.set_position(bytes_verified.min(info.total_length));

                                        // Write to disk (async, awaited before exit)
                                        let disk = disk_handle.clone();
                                        let handle = tokio::spawn(async move {
                                            if let Err(e) = disk.write_piece(piece_index, piece_data).await {
                                                eprintln!("Disk write error piece {piece_index}: {e}");
                                            }
                                        });
                                        pending_writes.push(handle);

                                        if pending_writes.len() > 100 {
                                            pending_writes.retain(|h| !h.is_finished());
                                        }

                                        peer_manager.broadcast(|| PeerCommand::SendHave { piece_index });
                                    } else {
                                        warn!(piece = piece_index, "SHA1 verification failed");
                                        picker.piece_failed(piece_index);
                                    }
                                }
                                BlockResult::Duplicate => {}
                            }

                            progress.set_message(format!("{}", peer_manager.peer_count()));

                            // Refill pipeline for this peer
                            fill_pipeline(&addr, &mut peer_manager, &peer_bitfields, &mut picker, &mut peer_pending, lightspeed);

                            // Endgame: duplicate requests across all unchoked peers
                            if lightspeed && picker.is_endgame() {
                                let addrs: Vec<SocketAddr> = peer_manager.connected_peers();
                                for peer_addr in addrs {
                                    if peer_manager.peer_state(&peer_addr).is_none_or(|s| s.peer_choking) {
                                        continue;
                                    }
                                    if let Some(bf) = peer_bitfields.get(&peer_addr) {
                                        let blocks = picker.endgame_requests(bf);
                                        for block in blocks {
                                            if !peer_manager.send_command(&peer_addr, PeerCommand::RequestBlock {
                                                index: block.piece_index,
                                                begin: block.offset,
                                                length: block.length,
                                            }) {
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        PeerEvent::Disconnected { .. } => {
                            release_peer_blocks(&addr, &mut peer_pending, &mut picker);
                            peer_pending.remove(&addr);
                            if let Some(bf) = peer_bitfields.remove(&addr) {
                                picker.peer_disconnected(&bf);
                            }
                            progress.set_message(format!("{}", peer_manager.peer_count()));
                            peer_manager.requeue_peer(addr);
                            // Freed blocks -> refill other peers' pipelines
                            refill_all_peers(&mut peer_manager, &peer_bitfields, &mut picker, &mut peer_pending, lightspeed);
                        }

                        PeerEvent::PexPeers(peers) => {
                            if lightspeed {
                                peer_manager.add_peers(peers.into_iter());
                                peer_manager.connect_pending();
                            }
                        }

                        PeerEvent::ExtendedHandshake(_) | PeerEvent::MetadataMessage(_) => {}
                    }
                }

                _ = connect_interval.tick() => {
                    peer_manager.connect_pending();
                }

                _ = speed_tick.tick() => {
                    let now = Instant::now();
                    let dt = now.duration_since(last_sample_time).as_secs_f64();
                    if dt > 0.5 && bytes_new > last_sample_bytes {
                        let speed = (bytes_new - last_sample_bytes) as f64 / dt / (1024.0 * 1024.0);
                        speed_samples.push(speed);
                        if speed > peak_speed { peak_speed = speed; }
                        if speed < min_speed { min_speed = speed; }
                        last_sample_time = now;
                        last_sample_bytes = bytes_new;
                    }
                }

                _ = choke_interval.tick() => {
                    optimistic_counter += 1;
                    let optimistic = optimistic_counter.is_multiple_of(3);
                    let commands = choker::run_choking_algorithm(&peer_manager, optimistic);
                    for (addr, cmd) in commands {
                        peer_manager.send_command(&addr, cmd);
                    }
                }
            }
        }

        progress.abandon();

        eprintln!("Flushing {} pending disk writes...", pending_writes.len());
        for handle in pending_writes {
            let _ = handle.await;
        }

        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), disk_task).await;

        let elapsed = start_time.elapsed();
        let done_mb = bytes_new as f64 / (1024.0 * 1024.0);
        let total_mb = self.metainfo.info.total_length as f64 / (1024.0 * 1024.0);
        let avg_speed = if elapsed.as_secs_f64() > 0.0 {
            done_mb / elapsed.as_secs_f64()
        } else {
            0.0
        };
        let verified = picker.pieces_done();

        let median_speed = if speed_samples.is_empty() {
            avg_speed
        } else {
            let mut sorted = speed_samples.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let mid = sorted.len() / 2;
            if sorted.len().is_multiple_of(2) {
                (sorted[mid - 1] + sorted[mid]) / 2.0
            } else {
                sorted[mid]
            }
        };
        if min_speed == f64::MAX {
            min_speed = 0.0;
        }

        eprintln!(
            "Done: {verified}/{num_pieces} pieces ({done_mb:.1}/{total_mb:.1} MiB) in {:.1}s",
            elapsed.as_secs_f64(),
        );
        eprintln!(
            "Speed -- avg: {avg_speed:.1} | median: {median_speed:.1} | peak: {peak_speed:.1} | low: {min_speed:.1} MiB/s",
        );

        Ok(())
    }
}

/// Fill a peer's request pipeline up to max_pipeline outstanding requests.
fn fill_pipeline(
    addr: &SocketAddr,
    peer_manager: &mut PeerManager,
    peer_bitfields: &HashMap<SocketAddr, Bitfield>,
    picker: &mut PiecePicker,
    peer_pending: &mut HashMap<SocketAddr, Vec<BlockRequest>>,
    lightspeed: bool,
) {
    let Some(state) = peer_manager.peer_state(addr) else {
        return;
    };
    if state.peer_choking {
        return;
    }
    let outstanding = state.outstanding_requests;
    let depth = max_pipeline(lightspeed, state.throughput_mibps);
    if outstanding >= depth {
        return;
    }

    let Some(bf) = peer_bitfields.get(addr) else {
        return;
    };
    let to_request = (depth - outstanding) as usize;

    for _ in 0..to_request {
        let block = picker.pick_block(bf);
        let Some(block) = block else { break };

        if !peer_manager.send_command(
            addr,
            PeerCommand::RequestBlock {
                index: block.piece_index,
                begin: block.offset,
                length: block.length,
            },
        ) {
            // Channel full -- stop sending, try again later
            break;
        }

        if let Some(s) = peer_manager.peer_state_mut(addr) {
            s.outstanding_requests += 1;
        }
        peer_pending.entry(*addr).or_default().push(block);
    }
}

/// After blocks are freed (choke/disconnect), refill all connected peers' pipelines.
fn refill_all_peers(
    peer_manager: &mut PeerManager,
    peer_bitfields: &HashMap<SocketAddr, Bitfield>,
    picker: &mut PiecePicker,
    peer_pending: &mut HashMap<SocketAddr, Vec<BlockRequest>>,
    lightspeed: bool,
) {
    let addrs: Vec<SocketAddr> = peer_manager.connected_peers();
    for addr in addrs {
        fill_pipeline(
            &addr,
            peer_manager,
            peer_bitfields,
            picker,
            peer_pending,
            lightspeed,
        );
    }
}

/// Release all pending blocks for a peer back to the picker.
fn release_peer_blocks(
    addr: &SocketAddr,
    peer_pending: &mut HashMap<SocketAddr, Vec<BlockRequest>>,
    picker: &mut PiecePicker,
) {
    if let Some(blocks) = peer_pending.get_mut(addr) {
        for block in blocks.drain(..) {
            picker.unassign_block(block.piece_index, block.offset);
        }
    }
}
