use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::engine::manager::DownloadOptions;
use crate::torrent::magnet::MagnetLink;
use crate::torrent::metainfo::Metainfo;

use super::server::AppState;

#[derive(Serialize)]
struct ApiError {
    error: String,
}

pub async fn list_torrents(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.manager.list())
}

pub async fn get_torrent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.manager.get(&id) {
        Some(status) => Json(status).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "not found".into(),
            }),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct DeleteOptions {
    #[serde(default)]
    pub delete_files: bool,
}

pub async fn delete_torrent(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(opts): Query<DeleteOptions>,
) -> impl IntoResponse {
    if opts.delete_files
        && let Ok(Some(record)) = state.store.get_download(&id)
    {
        let download_dir = record.output_dir.join(&record.name);
        if download_dir.exists() {
            let _ = std::fs::remove_dir_all(&download_dir);
        }
        // Also try removing single file
        let single_file = record.output_dir.join(&record.name);
        if single_file.is_file() {
            let _ = std::fs::remove_file(&single_file);
        }
    }
    state.manager.remove(&id);
    StatusCode::NO_CONTENT
}

pub async fn add_torrent(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();

        if name == "torrent" {
            let data = match field.bytes().await {
                Ok(d) => d,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ApiError {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            };
            let metainfo = match Metainfo::from_bytes(&data) {
                Ok(m) => m,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ApiError {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            };
            let opts = default_opts(&state.store);
            let id = state
                .manager
                .add_torrent(metainfo, data.to_vec(), opts)
                .await;
            let status = state.manager.get(&id);
            return (
                StatusCode::CREATED,
                Json(serde_json::json!({ "id": id, "status": status })),
            )
                .into_response();
        }

        if name == "magnet" {
            let text = match field.text().await {
                Ok(t) => t,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ApiError {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            };
            let _magnet = match MagnetLink::parse(&text) {
                Ok(m) => m,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ApiError {
                            error: e.to_string(),
                        }),
                    )
                        .into_response();
                }
            };
            // For now, return an error -- magnet requires metadata download phase which is more complex
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(ApiError {
                    error: "magnet support via API coming soon".into(),
                }),
            )
                .into_response();
        }
    }

    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            error: "expected 'torrent' or 'magnet' field".into(),
        }),
    )
        .into_response()
}

fn default_opts(store: &crate::engine::store::Store) -> DownloadOptions {
    let settings = store.get_settings();
    DownloadOptions {
        port: 6881,
        max_peers: 80,
        output_dir: settings.download_dir,
        no_dht: false,
        lightspeed: settings.lightspeed,
    }
}

pub async fn system_status() -> impl IntoResponse {
    let available = crate::transcode::runner::ffmpeg_available();
    let version = crate::transcode::runner::ffmpeg_version();
    Json(serde_json::json!({
        "ffmpeg_available": available,
        "ffmpeg_version": version,
    }))
}

pub async fn list_library(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.store.list_media() {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn get_library_item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.store.get_media(&id) {
        Ok(Some(entry)) => Json(entry).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "not found".into(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn delete_library_item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let _ = state.store.remove_media(&id);
    StatusCode::NO_CONTENT
}

pub async fn stream_media(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let entry = match state.store.get_media(&id) {
        Ok(Some(e)) => e,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    // Determine which file to serve
    let file_path = match &entry.transcode_state {
        crate::engine::library::TranscodeState::Ready { output_path } => output_path.clone(),
        crate::engine::library::TranscodeState::Skipped => entry.media_file.clone(),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    let file = match tokio::fs::File::open(&file_path).await {
        Ok(f) => f,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };

    let metadata = match file.metadata().await {
        Ok(m) => m,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };

    let total_size = metadata.len();

    // Parse Range header
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());

    if let Some(range_str) = range {
        // Parse "bytes=START-END" or "bytes=START-"
        if let Some(range_val) = range_str.strip_prefix("bytes=") {
            let parts: Vec<&str> = range_val.splitn(2, '-').collect();
            if parts.len() == 2 {
                let start: u64 = parts[0].parse().unwrap_or(0);
                let end: u64 = if parts[1].is_empty() {
                    total_size - 1
                } else {
                    parts[1].parse().unwrap_or(total_size - 1)
                };

                if start >= total_size {
                    return (
                        StatusCode::RANGE_NOT_SATISFIABLE,
                        [(header::CONTENT_RANGE, format!("bytes */{total_size}"))],
                    )
                        .into_response();
                }

                let end = end.min(total_size - 1);
                let content_length = end - start + 1;

                let mut file = file;
                file.seek(std::io::SeekFrom::Start(start)).await.ok();
                let stream = ReaderStream::new(file.take(content_length));
                let body = Body::from_stream(stream);

                return (
                    StatusCode::PARTIAL_CONTENT,
                    [
                        (header::CONTENT_TYPE, "video/mp4".to_string()),
                        (header::ACCEPT_RANGES, "bytes".to_string()),
                        (header::CONTENT_LENGTH, content_length.to_string()),
                        (
                            header::CONTENT_RANGE,
                            format!("bytes {start}-{end}/{total_size}"),
                        ),
                    ],
                    body,
                )
                    .into_response();
            }
        }
    }

    // No range - serve full file
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "video/mp4".to_string()),
            (header::ACCEPT_RANGES, "bytes".to_string()),
            (header::CONTENT_LENGTH, total_size.to_string()),
        ],
        body,
    )
        .into_response()
}

// ── Settings ──

pub async fn get_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.store.get_settings())
}

pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(settings): Json<crate::engine::store::AppSettings>,
) -> impl IntoResponse {
    match state.store.put_settings(&settings) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

// ── Presets ──

pub async fn list_presets() -> impl IntoResponse {
    Json(crate::transcode::presets::builtin_presets())
}

// ── Manual Transcode ──

#[derive(serde::Deserialize)]
pub struct TranscodeRequest {
    pub preset: String,
    pub container: String,
}

pub async fn manual_transcode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<TranscodeRequest>,
) -> impl IntoResponse {
    let entry = match state.store.get_media(&id) {
        Ok(Some(e)) => e,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "not found".into(),
                }),
            )
                .into_response();
        }
    };

    let output_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".movies")
        .join("transcoded");
    let sanitized = crate::engine::library::sanitize_filename(&entry.title);
    let ext = if req.container == "hls" {
        "m3u8"
    } else {
        "mp4"
    };
    let output_path = output_dir.join(format!("{sanitized}.{ext}"));

    let job = crate::transcode::runner::TranscodeJob {
        media_id: id,
        input_path: entry.media_file,
        output_path,
        preset_name: req.preset,
        container: req.container,
    };

    let _ = state
        .store
        .update_transcode_state(&id, crate::engine::library::TranscodeState::Pending);

    let tc = state.transcode.clone();
    tokio::spawn(async move {
        tc.submit(job).await;
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "queued" })),
    )
        .into_response()
}

// ── Filesystem Browse ──

#[derive(serde::Deserialize)]
pub struct BrowseQuery {
    pub path: Option<String>,
}

pub async fn browse_filesystem(Query(query): Query<BrowseQuery>) -> impl IntoResponse {
    let path = query
        .path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/")));

    if !path.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "not a directory".into(),
            }),
        )
            .into_response();
    }

    let parent = path.parent().map(|p| p.to_string_lossy().to_string());

    let mut dirs: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&path) {
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                // Skip hidden directories
                if let Some(name) = entry_path.file_name().and_then(|n| n.to_str())
                    && !name.starts_with('.')
                {
                    dirs.push(name.to_string());
                }
            }
        }
    }
    dirs.sort();

    Json(serde_json::json!({
        "current": path.to_string_lossy(),
        "parent": parent,
        "dirs": dirs,
    }))
    .into_response()
}

// ── Folder Scan ──

#[derive(serde::Deserialize)]
pub struct ScanRequest {
    pub path: String,
}

#[derive(serde::Deserialize)]
pub struct MetadataSearchQuery {
    pub title: String,
    pub year: Option<u16>,
}

pub async fn search_metadata(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MetadataSearchQuery>,
) -> impl IntoResponse {
    let settings = state.store.get_settings();
    if settings.tmdb_api_key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "TMDB API key not configured".into(),
            }),
        )
            .into_response();
    }
    match crate::tmdb::fetch_metadata(&settings.tmdb_api_key, &query.title, query.year).await {
        Some(meta) => Json(serde_json::json!({
            "title": meta.title,
            "poster_url": meta.poster_url,
            "overview": meta.overview,
            "rating": meta.rating,
            "cast": meta.cast,
            "director": meta.director,
            "year": meta.year,
        }))
        .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "not found on TMDB".into(),
            }),
        )
            .into_response(),
    }
}

pub async fn refresh_metadata(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let entry = match state.store.get_media(&id) {
        Ok(Some(e)) => e,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    error: "not found".into(),
                }),
            )
                .into_response();
        }
    };

    let settings = state.store.get_settings();
    if settings.tmdb_api_key.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "TMDB API key not configured".into(),
            }),
        )
            .into_response();
    }

    let store = state.store.clone();
    tokio::spawn(async move {
        if let Some(meta) =
            crate::tmdb::fetch_metadata(&settings.tmdb_api_key, &entry.title, entry.year).await
            && let Ok(Some(mut entry)) = store.get_media(&id)
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
            let _ = store.put_media(&entry);
            eprintln!("TMDB: refreshed metadata for \"{}\"", entry.title);
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "refreshing" })),
    )
        .into_response()
}

pub async fn scan_folder(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ScanRequest>,
) -> impl IntoResponse {
    let scan_path = std::path::PathBuf::from(&req.path);
    if !scan_path.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "path is not a directory".into(),
            }),
        )
            .into_response();
    }

    let video_files = crate::engine::library::detect_video_files(&scan_path);

    // Get existing media file paths to avoid duplicates
    let existing: std::collections::HashSet<std::path::PathBuf> = state
        .store
        .list_media()
        .unwrap_or_default()
        .iter()
        .map(|e| e.media_file.clone())
        .collect();

    let mut added = 0u32;
    let mut skipped = 0u32;

    for video_file in video_files {
        if existing.contains(&video_file) {
            skipped += 1;
            continue;
        }

        let filename = video_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown");
        let (title, year) = crate::engine::library::parse_media_title(filename);
        let is_web = crate::engine::library::is_web_compatible(&video_file);
        let file_size = std::fs::metadata(&video_file).map(|m| m.len()).unwrap_or(0);

        let transcode_state = if is_web {
            crate::engine::library::TranscodeState::Skipped
        } else if crate::transcode::runner::ffmpeg_available() {
            crate::engine::library::TranscodeState::Pending
        } else {
            crate::engine::library::TranscodeState::Unavailable
        };

        let entry = crate::engine::library::MediaEntry {
            id: uuid::Uuid::new_v4(),
            title,
            year,
            media_type: crate::engine::library::MediaType::Unknown,
            original_path: video_file.parent().unwrap_or(&scan_path).to_path_buf(),
            media_file: video_file,
            transcoded_path: None,
            transcode_state,
            transcode_started_at: None,
            download_id: uuid::Uuid::nil(),
            added_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            file_size,
            poster_url: None,
            overview: None,
            rating: None,
            cast: Vec::new(),
            director: None,
        };

        let _ = state.store.put_media(&entry);

        // Fetch metadata from TMDB
        {
            let store_for_tmdb = state.store.clone();
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
        }

        // Auto-transcode if enabled and state is Pending
        if matches!(
            entry.transcode_state,
            crate::engine::library::TranscodeState::Pending
        ) {
            let settings = state.store.get_settings();
            if settings.auto_transcode {
                let output_dir = dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".movies")
                    .join("transcoded");
                let sanitized = crate::engine::library::sanitize_filename(&entry.title);
                let ext = if settings.default_container == "hls" {
                    "m3u8"
                } else {
                    "mp4"
                };
                let output_path = output_dir.join(format!("{sanitized}.{ext}"));

                let job = crate::transcode::runner::TranscodeJob {
                    media_id: entry.id,
                    input_path: entry.media_file.clone(),
                    output_path,
                    preset_name: settings.default_preset.clone(),
                    container: settings.default_container.clone(),
                };
                let tc = state.transcode.clone();
                tokio::spawn(async move {
                    tc.submit(job).await;
                });
            }
        }

        added += 1;
    }

    Json(serde_json::json!({ "added": added, "skipped": skipped })).into_response()
}
