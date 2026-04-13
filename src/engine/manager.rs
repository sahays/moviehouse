use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::torrent::metainfo::Metainfo;
use crate::torrent::types::PeerId;
use crate::transcode::runner::TranscodeHandle;

use super::session::{SessionState, SessionStatus, TorrentSession};
use super::store::{DownloadRecord, Store};
pub use super::types::{DownloadOptions, SessionEvent, SessionHandle};

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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

    // Async for API consistency; spawned tasks use async internally
    // unwrap_used: RwLock unwrap is correct — poisoned lock means a thread panicked
    #[allow(clippy::unused_async, clippy::too_many_lines, clippy::unwrap_used)]
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
                    let group_id = if video_files.len() > 1 {
                        Some(Uuid::new_v4())
                    } else {
                        None
                    };
                    let settings = store.get_settings();

                    let mut media_ids: Vec<Uuid> = Vec::new();

                    for video_file in &video_files {
                        let filename = video_file
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&record.name);
                        let episode_info = crate::engine::library::parse_episode_info(filename);
                        let (title, year) = crate::engine::library::parse_media_title(filename);
                        let is_web = crate::engine::library::is_web_compatible(video_file);

                        let media_type = if episode_info.is_show {
                            crate::engine::library::MediaType::Show
                        } else {
                            crate::engine::library::MediaType::Unknown
                        };

                        let transcode_state = if is_web {
                            crate::engine::library::TranscodeState::Skipped
                        } else if crate::transcode::runner::ffmpeg_available() {
                            crate::engine::library::TranscodeState::Pending
                        } else {
                            crate::engine::library::TranscodeState::Unavailable
                        };

                        let media_id = Uuid::new_v4();
                        let entry = crate::engine::library::MediaEntry {
                            id: media_id,
                            title: if episode_info.is_show {
                                episode_info.show_name.clone()
                            } else {
                                title.clone()
                            },
                            year,
                            media_type,
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
                            show_name: if episode_info.is_show {
                                Some(episode_info.show_name.clone())
                            } else {
                                None
                            },
                            season: episode_info.season,
                            episode: episode_info.episode,
                            episode_title: episode_info.episode_title,
                            group_id,
                            tmdb_id: None,
                        };

                        let _ = store.put_media(&entry);
                        media_ids.push(media_id);

                        // Submit transcode job if auto_transcode is enabled
                        if settings.auto_transcode
                            && matches!(
                                transcode_state,
                                crate::engine::library::TranscodeState::Pending
                            )
                            && let Some(ref tc) = transcode_for_hook
                        {
                            let job =
                                crate::transcode::job::create_job(&entry, &settings.default_preset);
                            let tc = tc.clone();
                            tokio::spawn(async move {
                                tc.submit(job).await;
                            });
                        }
                    }

                    // Log what we indexed
                    if video_files.len() > 1 {
                        eprintln!(
                            "Library: added {} files from \"{}\"",
                            video_files.len(),
                            record.name
                        );
                    } else if video_files.len() == 1 {
                        eprintln!("Library: added \"{}\"", record.name);
                    }

                    // TMDB fetch: once per group, applied to all entries
                    if !video_files.is_empty() {
                        let first_file = video_files[0]
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or(&record.name);
                        let ep_info = crate::engine::library::parse_episode_info(first_file);
                        let (parsed_title, parsed_year) =
                            crate::engine::library::parse_media_title(first_file);
                        let search_title = if ep_info.is_show {
                            ep_info.show_name
                        } else {
                            parsed_title
                        };
                        let is_show = ep_info.is_show;
                        let search_year = parsed_year;

                        let store_for_tmdb = store.clone();
                        let _ = group_id;
                        tokio::spawn(async move {
                            let settings = store_for_tmdb.get_settings();
                            if settings.tmdb_api_key.is_empty() {
                                return;
                            }
                            eprintln!(
                                "TMDB: searching for \"{search_title}\" (year: {search_year:?}, is_show: {is_show})"
                            );
                            let meta = crate::tmdb::fetch_metadata_auto(
                                &settings.tmdb_api_key,
                                &search_title,
                                search_year,
                                is_show,
                            )
                            .await;
                            if let Some(meta) = meta {
                                // Apply metadata to all entries in the group
                                for mid in &media_ids {
                                    if let Ok(Some(mut entry)) = store_for_tmdb.get_media(mid) {
                                        let saved_title = entry.title.clone();
                                        crate::tmdb::apply_metadata(&mut entry, &meta);
                                        if is_show {
                                            entry.title = saved_title;
                                        }
                                        let _ = store_for_tmdb.put_media(&entry);
                                    }
                                }
                                eprintln!("TMDB: fetched metadata for \"{search_title}\"");
                            } else {
                                eprintln!("TMDB: no results for \"{search_title}\"");
                            }
                        });
                    }
                }
            }
        });

        id
    }

    /// List active sessions (in-memory) merged with persisted history.
    #[allow(clippy::unwrap_used)] // RwLock unwrap is correct for unpoisoned locks
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

    #[allow(clippy::unwrap_used)] // RwLock unwrap is correct for unpoisoned locks
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

    /// Register a magnet link as a visible "Resolving" placeholder in the download list.
    /// Returns the UUID that the UI can track.
    pub fn register_magnet(&self, name: String, info_hash: String) -> Uuid {
        let id = Uuid::new_v4();
        let status = SessionStatus {
            id,
            name: name.clone(),
            info_hash: info_hash.clone(),
            state: SessionState::Resolving,
            total_bytes: 0,
            downloaded_bytes: 0,
            pieces_done: 0,
            pieces_total: 0,
            peer_count: 0,
            download_speed: 0.0,
            progress: 0.0,
            started_at: now_secs(),
            completed_at: None,
            uploaded_bytes: 0,
        };

        let (_tx, rx) = tokio::sync::watch::channel(status.clone());
        let handle = SessionHandle {
            id,
            name,
            status: Arc::new(std::sync::RwLock::new(status.clone())),
            status_rx: rx,
            cancel: self.cancel.child_token(),
        };

        // Persist so it survives restarts
        let record = DownloadRecord {
            id,
            name: handle.name.clone(),
            info_hash,
            total_bytes: 0,
            pieces_total: 0,
            metainfo_bytes: vec![],
            output_dir: std::path::PathBuf::from("."),
            lightspeed: false,
            completed_pieces: vec![],
            status: status.clone(),
        };
        let _ = self.store.put_download(&record);

        // Broadcast to WebSocket
        let _ = self.event_tx.send(SessionEvent {
            id,
            status: status.clone(),
        });

        self.sessions.insert(id, handle);
        id
    }

    /// Metadata resolved: remove placeholder, start real download with same ID visible.
    /// The old placeholder is removed and `add_torrent` creates a new session entry.
    pub async fn resolve_magnet(
        &self,
        placeholder_id: Uuid,
        metainfo: Metainfo,
        metainfo_bytes: Vec<u8>,
        opts: DownloadOptions,
    ) -> Uuid {
        // Remove the placeholder session
        self.sessions.remove(&placeholder_id);
        let _ = self.store.remove_download(&placeholder_id);

        // Start real download (gets a new ID, shows up in UI immediately)
        self.add_torrent(metainfo, metainfo_bytes, opts).await
    }

    /// Metadata resolution failed: update placeholder to Error state.
    #[allow(clippy::unwrap_used)]
    pub fn fail_magnet(&self, id: &Uuid, error: String) {
        if let Some(handle) = self.sessions.get(id) {
            let mut status = handle.status.write().unwrap();
            status.state = SessionState::Error(error);
            let _ = self.event_tx.send(SessionEvent {
                id: *id,
                status: status.clone(),
            });
            // Persist the error state
            if let Ok(Some(mut record)) = self.store.get_download(id) {
                record.status = status.clone();
                let _ = self.store.put_download(&record);
            }
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }
}
