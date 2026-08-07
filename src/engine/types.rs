use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

// ── From library.rs ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtitleTrack {
    pub label: String,
    pub language: Option<String>,
    pub path: PathBuf,
    pub format: String,
}

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
    #[serde(default)]
    pub show_name: Option<String>,
    #[serde(default)]
    pub season: Option<u16>,
    #[serde(default)]
    pub episode: Option<u16>,
    #[serde(default)]
    pub episode_title: Option<String>,
    #[serde(default)]
    pub group_id: Option<Uuid>,
    #[serde(default)]
    pub tmdb_id: Option<u64>,
    /// Detected subtitle files alongside this media
    #[serde(default)]
    pub subtitles: Vec<SubtitleTrack>,
    /// Unix timestamp of when this was last played
    #[serde(default)]
    pub last_played_at: Option<u64>,
    /// Playback progress in seconds
    #[serde(default)]
    pub play_position: Option<f64>,
    /// Total duration of media in seconds
    #[serde(default)]
    pub duration: Option<f64>,
}

// ── From store.rs ──

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub lightspeed: bool,
    pub max_download_speed: u64,
    pub download_dir: PathBuf,
    pub media_scan_dir: Option<PathBuf>,
    pub auto_transcode: bool,
    pub default_preset: String,
    pub tmdb_api_key: String,
    pub transcode_concurrency: usize,
    pub transcode_dir: PathBuf,
    /// Delete the source file once a transcode of it succeeds.
    ///
    /// On by default: the `hevc` preset is a remux, so the output is the same
    /// size as the input and keeping both doubles the disk cost of every
    /// download. Turn off to keep originals, and watch your free space.
    #[serde(default = "default_true")]
    pub delete_source_after_transcode: bool,
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            lightspeed: true,
            max_download_speed: 0,
            download_dir: default_download_dir(),
            media_scan_dir: None,
            auto_transcode: true,
            default_preset: "hevc".into(),
            tmdb_api_key: String::new(),
            transcode_concurrency: 2,
            transcode_dir: default_transcode_dir(),
            delete_source_after_transcode: true,
        }
    }
}

/// A non-empty env override, if set. Env wins over the built-in default so a
/// deployment can point these at its mounted volume without a settings write.
fn dir_from_env(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

/// Where downloads land by default.
///
/// This must be an absolute, writable path: the process runs as an unprivileged
/// user with a root-owned working directory (`/app` in the container), so a
/// relative default like `"."` makes every piece write fail with EACCES.
pub fn default_download_dir() -> PathBuf {
    dir_from_env("MOVIEHOUSE_DOWNLOAD_DIR").unwrap_or_else(|| home_movies_subdir("downloads"))
}

pub fn default_transcode_dir() -> PathBuf {
    dir_from_env("MOVIEHOUSE_TRANSCODE_DIR").unwrap_or_else(|| home_movies_subdir("transcoded"))
}

/// Where the sled database lives. Unlike the media dirs this is never stored in
/// settings — settings live *inside* it — so the env var is the only override.
pub fn default_data_dir() -> PathBuf {
    dir_from_env("MOVIEHOUSE_DATA_DIR").unwrap_or_else(|| home_movies_subdir("data"))
}

/// `$HOME/.movies/<name>` — the fallback when no env override is set. `$HOME` is
/// writable by definition (the container sets it to the mounted data volume).
/// Public so startup repair can recognise a stored value that is still this
/// untouched fallback rather than a deliberate operator choice.
pub fn home_movies_subdir(name: &str) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".movies")
        .join(name)
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
    Resolving,
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
    /// The download was removed, not updated. The websocket turns this into a
    /// `remove` message so the UI drops the row; without it a deleted download
    /// lingers in the list until the page is reloaded.
    pub removed: bool,
}

impl SessionEvent {
    /// A progress/state update for a live download.
    #[must_use]
    pub fn update(id: Uuid, status: SessionStatus) -> Self {
        Self {
            id,
            status,
            removed: false,
        }
    }

    /// The download is gone. `status` is its last known state, for subscribers
    /// that want it; the UI only needs the id.
    #[must_use]
    pub fn removed(id: Uuid, status: SessionStatus) -> Self {
        Self {
            id,
            status,
            removed: true,
        }
    }
}

pub struct DownloadOptions {
    pub port: u16,
    pub max_peers: usize,
    pub output_dir: std::path::PathBuf,
    pub no_dht: bool,
    pub lightspeed: bool,
}

impl DownloadOptions {
    /// Inbound `BitTorrent` peer port — the one mapped on the router.
    pub const DEFAULT_PEER_PORT: u16 = 6881;
    /// Peer connections per download.
    pub const DEFAULT_MAX_PEERS: usize = 80;

    /// Options for a download started from the UI or resumed at startup.
    #[must_use]
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            port: Self::DEFAULT_PEER_PORT,
            max_peers: Self::DEFAULT_MAX_PEERS,
            output_dir: settings.download_dir.clone(),
            no_dht: false,
            lightspeed: settings.lightspeed,
        }
    }
}
