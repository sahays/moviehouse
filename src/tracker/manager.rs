use std::net::SocketAddr;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::torrent::types::{InfoHash, PeerId};

use super::http;
use super::udp;

/// Manages announces to multiple trackers for a torrent.
pub struct TrackerManager {
    info_hash: InfoHash,
    peer_id: PeerId,
    port: u16,
    tracker_urls: Vec<String>,
    peer_tx: mpsc::Sender<Vec<SocketAddr>>,
    cancel: CancellationToken,
}

impl TrackerManager {
    pub fn new(
        info_hash: InfoHash,
        peer_id: PeerId,
        port: u16,
        tracker_urls: Vec<String>,
        peer_tx: mpsc::Sender<Vec<SocketAddr>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            info_hash,
            peer_id,
            port,
            tracker_urls,
            peer_tx,
            cancel,
        }
    }

    /// Run the tracker manager: announce to all trackers, then re-announce periodically.
    pub async fn run(self, total_length: u64) {
        if self.tracker_urls.is_empty() {
            debug!("No trackers configured");
            return;
        }

        // Initial announce to all trackers
        let mut reannounce_interval = 1800u32;

        for url in &self.tracker_urls {
            match self
                .announce_single(url, total_length, Some("started"))
                .await
            {
                Ok(resp) => {
                    // Prefer min_interval if provided, otherwise use interval
                    let effective = resp.min_interval.unwrap_or(resp.interval);
                    reannounce_interval = reannounce_interval.min(effective);
                    if !resp.peers.is_empty() {
                        let _ = self.peer_tx.send(resp.peers).await;
                    }
                }
                Err(e) => {
                    warn!(tracker = %url, error = %e, "Tracker announce failed");
                }
            }
        }

        // Respect tracker interval, but cap at min 60s
        let interval_secs = reannounce_interval.max(60) as u64;
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        interval.tick().await; // skip first immediate tick

        loop {
            tokio::select! {
                () = self.cancel.cancelled() => {
                    // Send stopped event (best-effort, short timeout)
                    for url in &self.tracker_urls {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(3),
                            self.announce_single(url, total_length, Some("stopped")),
                        ).await;
                    }
                    break;
                }
                _ = interval.tick() => {
                    for url in &self.tracker_urls {
                        match self.announce_single(url, total_length, None).await {
                            Ok(resp) => {
                                if !resp.peers.is_empty() {
                                    let _ = self.peer_tx.send(resp.peers).await;
                                }
                            }
                            Err(e) => {
                                warn!(tracker = %url, error = %e, "Re-announce failed");
                            }
                        }
                    }
                }
            }
        }
    }

    async fn announce_single(
        &self,
        url: &str,
        left: u64,
        event: Option<&str>,
    ) -> Result<http::AnnounceResponse, http::TrackerError> {
        if url.starts_with("udp://") {
            let event_code = match event {
                Some("started") => 2,
                Some("completed") => 1,
                Some("stopped") => 3,
                _ => 0,
            };
            udp::udp_announce(
                url,
                &self.info_hash,
                &self.peer_id,
                self.port,
                0,
                0,
                left,
                event_code,
            )
            .await
        } else {
            http::http_announce(
                url,
                &self.info_hash,
                &self.peer_id,
                self.port,
                0,
                0,
                left,
                event,
            )
            .await
        }
    }
}
