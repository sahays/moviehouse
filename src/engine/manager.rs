use std::path::PathBuf;
use std::sync::Arc;

use dashmap::DashMap;
use serde::Serialize;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::torrent::metainfo::Metainfo;
use crate::torrent::types::PeerId;
use crate::transcode::runner::TranscodeHandle;

use super::session::{SessionHandle, SessionState, SessionStatus, TorrentSession};
use super::store::{DownloadRecord, Store};

#[derive(Debug, Clone, Serialize)]
pub struct SessionEvent {
    pub id: Uuid,
    pub status: SessionStatus,
}

pub struct DownloadOptions {
    pub port: u16,
    pub max_peers: usize,
    pub output_dir: std::path::PathBuf,
    pub no_dht: bool,
    pub lightspeed: bool,
}

pub struct SessionManager {
    sessions: Arc<DashMap<Uuid, SessionHandle>>,
    event_tx: broadcast::Sender<SessionEvent>,
    store: Arc<Store>,
    transcode: Option<TranscodeHandle>,
    cancel: CancellationToken,
}

impl SessionManager {
    pub fn new(
        cancel: CancellationToken,
        store: Arc<Store>,
        transcode: Option<TranscodeHandle>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            sessions: Arc::new(DashMap::new()),
            event_tx,
            store,
            transcode,
            cancel,
        }
    }

    pub async fn add_torrent(
        &self,
        metainfo: Metainfo,
        metainfo_bytes: Vec<u8>,
        opts: DownloadOptions,
    ) -> Uuid {
        let our_peer_id = PeerId::generate();
        let session_cancel = self.cancel.child_token();

        let session = TorrentSession::new(
            metainfo.clone(),
            our_peer_id,
            opts.port,
            opts.max_peers,
            opts.output_dir.clone(),
            opts.no_dht,
            opts.lightspeed,
            session_cancel,
            Vec::new(),
        );

        let handle = session.handle();
        let id = handle.id;

        // Persist the download record
        let record = DownloadRecord {
            id,
            name: metainfo.info.name.clone(),
            info_hash: metainfo.info_hash.to_string(),
            total_bytes: metainfo.info.total_length,
            pieces_total: metainfo.info.pieces.len(),
            metainfo_bytes,
            output_dir: opts.output_dir,
            lightspeed: opts.lightspeed,
            completed_pieces: Vec::new(),
            status: handle.status.read().unwrap().clone(),
        };
        if let Err(e) = self.store.put_download(&record) {
            tracing::error!(error = %e, "Failed to persist download");
        }

        // Forward status updates to broadcast channel + persist periodically
        let event_tx = self.event_tx.clone();
        let store = self.store.clone();
        let mut status_rx = handle.status_rx.clone();
        tokio::spawn(async move {
            let mut last_persist = std::time::Instant::now();
            while status_rx.changed().await.is_ok() {
                let status = status_rx.borrow().clone();
                let _ = event_tx.send(SessionEvent {
                    id,
                    status: status.clone(),
                });

                // Persist status every 5 seconds or on completion
                let should_persist = last_persist.elapsed().as_secs() >= 5
                    || matches!(
                        status.state,
                        SessionState::Completed | SessionState::Error(_)
                    );
                if should_persist {
                    if let Ok(Some(mut record)) = store.get_download(&id) {
                        record.status = status;
                        let _ = store.put_download(&record);
                    }
                    last_persist = std::time::Instant::now();
                }
            }
        });

        self.sessions.insert(id, handle);

        // Spawn the session task
        let sessions = self.sessions.clone();
        let event_tx = self.event_tx.clone();
        let store = self.store.clone();
        let transcode_for_hook = self.transcode.clone();
        tokio::spawn(async move {
            if let Err(e) = session.run().await {
                tracing::error!(id = %id, error = %e, "Session failed");
            }
            // Update final status
            if let Some(handle) = sessions.get(&id) {
                let final_status = handle.status.read().unwrap().clone();
                let _ = event_tx.send(SessionEvent {
                    id,
                    status: final_status.clone(),
                });
                // Persist final state
                if let Ok(Some(mut record)) = store.get_download(&id) {
                    record.status = final_status.clone();
                    let _ = store.put_download(&record);
                    let _ = store.flush();
                }

                // Download→library integration on completion
                if matches!(final_status.state, SessionState::Completed)
                    && let Ok(Some(record)) = store.get_download(&id)
                {
                    let download_dir = record.output_dir.join(&record.name);
                    let video_files = crate::engine::library::detect_video_files(&download_dir);
                    if let Some(video_file) = video_files.first() {
                        let (title, year) = crate::engine::library::parse_media_title(&record.name);
                        let is_web = crate::engine::library::is_web_compatible(video_file);
                        let settings = store.get_settings();

                        let media_id = Uuid::new_v4();
                        let transcode_state = if is_web {
                            crate::engine::library::TranscodeState::Skipped
                        } else if crate::transcode::runner::ffmpeg_available() {
                            crate::engine::library::TranscodeState::Pending
                        } else {
                            crate::engine::library::TranscodeState::Unavailable
                        };

                        let entry = crate::engine::library::MediaEntry {
                            id: media_id,
                            title: title.clone(),
                            year,
                            media_type: crate::engine::library::MediaType::Unknown,
                            original_path: download_dir.clone(),
                            media_file: video_file.clone(),
                            transcoded_path: None,
                            transcode_state: transcode_state.clone(),
                            transcode_started_at: None,
                            download_id: id,
                            added_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs(),
                            file_size: std::fs::metadata(video_file).map(|m| m.len()).unwrap_or(0),
                            poster_url: None,
                            overview: None,
                            rating: None,
                            cast: Vec::new(),
                            director: None,
                            video_codec: None,
                            audio_codec: None,
                            versions: std::collections::HashMap::new(),
                        };

                        let _ = store.put_media(&entry);
                        eprintln!(
                            "Library: added \"{}\" (state: {:?})",
                            entry.title, entry.transcode_state
                        );

                        // Fetch metadata from TMDB
                        let store_for_tmdb = store.clone();
                        let entry_id = entry.id;
                        let entry_title = entry.title.clone();
                        let entry_year = entry.year;
                        tokio::spawn(async move {
                            let settings = store_for_tmdb.get_settings();
                            if !settings.tmdb_api_key.is_empty()
                                && let Some(meta) = crate::tmdb::fetch_metadata(
                                    &settings.tmdb_api_key,
                                    &entry_title,
                                    entry_year,
                                )
                                .await
                                && let Ok(Some(mut entry)) = store_for_tmdb.get_media(&entry_id)
                            {
                                if let Some(ref title) = meta.title {
                                    entry.title = title.clone();
                                }
                                entry.poster_url = meta.poster_url;
                                entry.overview = meta.overview;
                                entry.rating = meta.rating;
                                entry.cast = meta.cast;
                                entry.director = meta.director;
                                if meta.year.is_some() && entry.year.is_none() {
                                    entry.year = meta.year;
                                }
                                let _ = store_for_tmdb.put_media(&entry);
                                eprintln!("TMDB: fetched metadata for \"{}\"", entry.title);
                            }
                        });

                        // Submit transcode job if auto_transcode is enabled
                        if settings.auto_transcode
                            && matches!(
                                entry.transcode_state,
                                crate::engine::library::TranscodeState::Pending
                            )
                            && let Some(ref tc) = transcode_for_hook
                        {
                            let output_dir = dirs::home_dir()
                                .unwrap_or_else(|| PathBuf::from("."))
                                .join(".movies")
                                .join("transcoded");
                            let sanitized = crate::engine::library::sanitize_filename(&title);
                            let ext = if settings.default_container == "hls" {
                                "m3u8"
                            } else {
                                "mp4"
                            };
                            let output_path = output_dir.join(format!("{sanitized}.{ext}"));

                            let job = crate::transcode::runner::TranscodeJob {
                                media_id,
                                input_path: video_file.clone(),
                                output_path,
                                preset_name: settings.default_preset.clone(),
                                container: settings.default_container.clone(),
                                enable_chunking: settings.enable_chunking,
                            };
                            let tc = tc.clone();
                            tokio::spawn(async move {
                                tc.submit(job).await;
                            });
                        }
                    }
                }
            }
        });

        id
    }

    /// List active sessions (in-memory) merged with persisted history.
    pub fn list(&self) -> Vec<SessionStatus> {
        // Active sessions take priority
        let mut statuses: Vec<SessionStatus> = self
            .sessions
            .iter()
            .map(|entry| entry.value().status.read().unwrap().clone())
            .collect();

        // Add completed/historical downloads from the store
        let active_ids: std::collections::HashSet<Uuid> = statuses.iter().map(|s| s.id).collect();
        if let Ok(records) = self.store.list_downloads() {
            for record in records {
                if !active_ids.contains(&record.id) {
                    statuses.push(record.status);
                }
            }
        }

        statuses
    }

    pub fn get(&self, id: &Uuid) -> Option<SessionStatus> {
        // Check active sessions first
        if let Some(h) = self.sessions.get(id) {
            return Some(h.status.read().unwrap().clone());
        }
        // Fall back to persisted record
        self.store.get_download(id).ok().flatten().map(|r| r.status)
    }

    pub fn remove(&self, id: &Uuid) {
        if let Some((_, handle)) = self.sessions.remove(id) {
            handle.cancel.cancel();
        }
        if let Err(e) = self.store.remove_download(id) {
            tracing::error!(error = %e, "Failed to remove download from store");
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }
}
