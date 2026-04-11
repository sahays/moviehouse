use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::library::{MediaEntry, TranscodeState};
use super::session::SessionStatus;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub lightspeed: bool,
    pub max_download_speed: u64,
    pub download_dir: PathBuf,
    pub media_scan_dir: Option<PathBuf>,
    pub auto_transcode: bool,
    pub default_preset: String,
    pub default_container: String,
    pub tmdb_api_key: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            lightspeed: true,
            max_download_speed: 0,
            download_dir: PathBuf::from("."),
            media_scan_dir: None,
            auto_transcode: true,
            default_preset: "1080p".into(),
            default_container: "mp4".into(),
            tmdb_api_key: String::new(),
        }
    }
}

/// Persisted record for a download (what we need to restore it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub id: Uuid,
    pub name: String,
    pub info_hash: String,
    pub total_bytes: u64,
    pub pieces_total: usize,
    /// Raw .torrent metainfo bytes (so we can re-parse and resume)
    pub metainfo_bytes: Vec<u8>,
    /// Download options
    pub output_dir: PathBuf,
    pub lightspeed: bool,
    /// Completed piece indices (for resume)
    pub completed_pieces: Vec<u32>,
    /// Final status snapshot
    pub status: SessionStatus,
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
    pub fn open_at(path: PathBuf) -> anyhow::Result<Self> {
        let db = sled::open(&path)?;
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
        for result in self.downloads.iter() {
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
        for result in self.library.iter() {
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

    pub fn update_transcode_state(&self, id: &Uuid, state: TranscodeState) -> anyhow::Result<()> {
        if let Some(mut entry) = self.get_media(id)? {
            // Set started_at when transitioning to Transcoding
            if matches!(state, TranscodeState::Transcoding { .. })
                && entry.transcode_started_at.is_none()
            {
                entry.transcode_started_at = Some(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs(),
                );
            }
            entry.transcode_state = state;
            if let TranscodeState::Ready { ref output_path } = entry.transcode_state {
                entry.transcoded_path = Some(output_path.clone());
            }
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
