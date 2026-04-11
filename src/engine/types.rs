use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

// ── From library.rs ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MediaType {
    Movie,
    Show,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TranscodeState {
    Pending,
    Transcoding {
        progress_percent: f32,
        #[serde(default)]
        encoder: String,
    },
    Ready {
        output_path: PathBuf,
    },
    Failed {
        error: String,
    },
    Skipped,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaEntry {
    pub id: Uuid,
    pub title: String,
    pub year: Option<u16>,
    pub media_type: MediaType,
    pub original_path: PathBuf,
    pub media_file: PathBuf,
    pub transcoded_path: Option<PathBuf>,
    pub transcode_state: TranscodeState,
    #[serde(default)]
    pub transcode_started_at: Option<u64>,
    pub download_id: Uuid,
    pub added_at: u64,
    pub file_size: u64,
    #[serde(default)]
    pub poster_url: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub rating: Option<f32>,
    #[serde(default)]
    pub cast: Vec<String>,
    #[serde(default)]
    pub director: Option<String>,
    #[serde(default)]
    pub video_codec: Option<String>,
    #[serde(default)]
    pub audio_codec: Option<String>,
    /// Multiple transcoded versions: `preset_name` -> file path
    #[serde(default)]
    pub versions: std::collections::HashMap<String, PathBuf>,
}

// ── From store.rs ──

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
    #[serde(default = "default_true")]
    pub enable_chunking: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            lightspeed: true,
            max_download_speed: 0,
            download_dir: PathBuf::from("."),
            media_scan_dir: None,
            auto_transcode: true,
            default_preset: "compat-1080p".into(),
            default_container: "mp4".into(),
            tmdb_api_key: String::new(),
            enable_chunking: true,
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

// ── From session.rs ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionState {
    Downloading,
    Completed,
    Error(String),
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStatus {
    pub id: Uuid,
    pub name: String,
    pub info_hash: String,
    pub state: SessionState,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub pieces_done: usize,
    pub pieces_total: usize,
    pub peer_count: usize,
    pub download_speed: f64,
    pub progress: f64,
    pub started_at: u64,
    pub completed_at: Option<u64>,
    pub uploaded_bytes: u64,
}

pub struct SessionHandle {
    pub id: Uuid,
    pub name: String,
    pub status: std::sync::Arc<std::sync::RwLock<SessionStatus>>,
    pub status_rx: tokio::sync::watch::Receiver<SessionStatus>,
    pub cancel: tokio_util::sync::CancellationToken,
}

// ── From manager.rs ──

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
