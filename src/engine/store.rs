use std::path::PathBuf;

use uuid::Uuid;

pub use super::types::{AppSettings, DownloadRecord, MediaEntry, TranscodeState};

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Persistent KV store backed by sled.
pub struct Store {
    db: sled::Db,
    downloads: sled::Tree,
    library: sled::Tree,
    settings: sled::Tree,
}

impl Store {
    /// Open (or create) the store at ~/MovieHouse/.data/
    pub fn open() -> anyhow::Result<Self> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        Self::open_at(home.join(".movies").join("data"))
    }

    /// Open at a specific path.
    pub fn open_at(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let db = sled::open(path)?;
        let downloads = db.open_tree("downloads")?;
        let library = db.open_tree("library")?;
        let settings = db.open_tree("settings")?;
        tracing::info!(path = %path.display(), "Store opened");
        Ok(Self {
            db,
            downloads,
            library,
            settings,
        })
    }

    /// Save or update a download record.
    pub fn put_download(&self, record: &DownloadRecord) -> anyhow::Result<()> {
        let key = record.id.as_bytes();
        let value = serde_json::to_vec(record)?;
        self.downloads.insert(key, value)?;
        Ok(())
    }

    /// Get a download record by ID.
    pub fn get_download(&self, id: &Uuid) -> anyhow::Result<Option<DownloadRecord>> {
        match self.downloads.get(id.as_bytes())? {
            Some(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            None => Ok(None),
        }
    }

    /// List all download records.
    pub fn list_downloads(&self) -> anyhow::Result<Vec<DownloadRecord>> {
        let mut records = Vec::new();
        let mut corrupt_keys = Vec::new();
        for result in &self.downloads {
            let (key, value) = result?;
            match serde_json::from_slice::<DownloadRecord>(&value) {
                Ok(record) => records.push(record),
                Err(e) => {
                    eprintln!("Skipping corrupt download record: {e}");
                    corrupt_keys.push(key);
                }
            }
        }
        for key in corrupt_keys {
            let _ = self.downloads.remove(key);
        }
        Ok(records)
    }

    /// Remove a download record.
    pub fn remove_download(&self, id: &Uuid) -> anyhow::Result<()> {
        self.downloads.remove(id.as_bytes())?;
        Ok(())
    }

    /// Update just the completed pieces for a download.
    pub fn update_pieces(&self, id: &Uuid, pieces: Vec<u32>) -> anyhow::Result<()> {
        if let Some(mut record) = self.get_download(id)? {
            record.completed_pieces = pieces;
            self.put_download(&record)?;
        }
        Ok(())
    }

    // ── Library CRUD ──

    pub fn put_media(&self, entry: &MediaEntry) -> anyhow::Result<()> {
        let key = entry.id.as_bytes();
        let value = serde_json::to_vec(entry)?;
        self.library.insert(key, value)?;
        Ok(())
    }

    pub fn get_media(&self, id: &Uuid) -> anyhow::Result<Option<MediaEntry>> {
        match self.library.get(id.as_bytes())? {
            Some(bytes) => match serde_json::from_slice(&bytes) {
                Ok(entry) => Ok(Some(entry)),
                Err(e) => {
                    eprintln!("Corrupt media record {id}, removing: {e}");
                    self.library.remove(id.as_bytes())?;
                    Ok(None)
                }
            },
            None => Ok(None),
        }
    }

    pub fn list_media(&self) -> anyhow::Result<Vec<MediaEntry>> {
        let mut entries = Vec::new();
        let mut corrupt_keys = Vec::new();
        for result in &self.library {
            let (key, value) = result?;
            match serde_json::from_slice::<MediaEntry>(&value) {
                Ok(entry) => entries.push(entry),
                Err(e) => {
                    eprintln!("Skipping corrupt media record: {e}");
                    corrupt_keys.push(key);
                }
            }
        }
        // Clean up corrupt records
        for key in corrupt_keys {
            let _ = self.library.remove(key);
        }
        Ok(entries)
    }

    pub fn remove_media(&self, id: &Uuid) -> anyhow::Result<()> {
        self.library.remove(id.as_bytes())?;
        Ok(())
    }

    /// List all media entries belonging to a specific group.
    pub fn list_media_by_group(&self, group_id: &Uuid) -> anyhow::Result<Vec<MediaEntry>> {
        self.list_media().map(|entries| {
            entries
                .into_iter()
                .filter(|e| e.group_id.as_ref() == Some(group_id))
                .collect()
        })
    }

    /// State machine for transcode transitions.
    /// Valid transitions:
    ///   Pending      → Transcoding(0%)   [start]
    ///   Pending      → Unavailable       [no ffmpeg]
    ///   Transcoding  → Transcoding       [progress update only]
    ///   Transcoding  → Ready             [complete]
    ///   Transcoding  → Failed            [error]
    ///   Ready/Failed → Pending           [re-transcode request]
    ///   *            → Pending           [recovery on startup]
    pub fn update_transcode_state(
        &self,
        id: &Uuid,
        new_state: TranscodeState,
    ) -> anyhow::Result<()> {
        if let Some(mut entry) = self.get_media(id)? {
            let old = &entry.transcode_state;

            match (&old, &new_state) {
                // Start transcoding: set timestamp
                (s, TranscodeState::Transcoding { .. })
                    if !matches!(s, TranscodeState::Transcoding { .. }) =>
                {
                    entry.transcode_started_at = Some(now_secs());
                    entry.transcode_state = new_state;
                }

                // Progress update: only update percent, preserve timestamp and encoder
                (
                    TranscodeState::Transcoding {
                        encoder: existing_enc,
                        ..
                    },
                    TranscodeState::Transcoding {
                        progress_percent,
                        encoder: new_enc,
                    },
                ) => {
                    entry.transcode_state = TranscodeState::Transcoding {
                        progress_percent: *progress_percent,
                        encoder: if new_enc.is_empty() {
                            existing_enc.clone()
                        } else {
                            new_enc.clone()
                        },
                    };
                }

                // Complete: store output path
                (TranscodeState::Transcoding { .. }, TranscodeState::Ready { output_path }) => {
                    entry.transcoded_path = Some(output_path.clone());
                    entry.transcode_state = new_state;
                }

                // Failure or no ffmpeg
                (TranscodeState::Transcoding { .. }, TranscodeState::Failed { .. })
                | (TranscodeState::Pending, TranscodeState::Unavailable) => {
                    entry.transcode_state = new_state;
                }

                // Re-transcode or recovery: reset to Pending
                (_, TranscodeState::Pending) => {
                    entry.transcode_started_at = None;
                    entry.transcode_state = new_state;
                }

                // Invalid transition: log and ignore
                _ => {
                    eprintln!(
                        "Invalid transcode state transition: {old:?} → {new_state:?} (ignored)"
                    );
                    return Ok(());
                }
            }

            self.put_media(&entry)?;
        }
        Ok(())
    }

    /// Store a completed transcode version.
    pub fn add_version(
        &self,
        id: &Uuid,
        preset_name: &str,
        output_path: &std::path::Path,
    ) -> anyhow::Result<()> {
        if let Some(mut entry) = self.get_media(id)? {
            entry
                .versions
                .insert(preset_name.to_string(), output_path.to_path_buf());
            self.put_media(&entry)?;
        }
        Ok(())
    }

    /// Update playback progress for a media entry.
    pub fn update_play_progress(
        &self,
        id: &Uuid,
        position: f64,
        duration: f64,
    ) -> anyhow::Result<()> {
        if let Some(mut entry) = self.get_media(id)? {
            entry.play_position = Some(position);
            entry.duration = Some(duration);
            entry.last_played_at = Some(now_secs());
            self.put_media(&entry)?;
        }
        Ok(())
    }

    // ── Settings ──

    pub fn get_settings(&self) -> AppSettings {
        match self.settings.get(b"app") {
            Ok(Some(bytes)) => serde_json::from_slice(&bytes).unwrap_or_default(),
            _ => AppSettings::default(),
        }
    }

    pub fn put_settings(&self, settings: &AppSettings) -> anyhow::Result<()> {
        let value = serde_json::to_vec(settings)?;
        self.settings.insert(b"app", value)?;
        Ok(())
    }

    /// Flush to disk.
    pub fn flush(&self) -> anyhow::Result<()> {
        self.db.flush()?;
        Ok(())
    }
}
