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

#[allow(clippy::too_many_lines)]
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
            let magnet = match MagnetLink::parse(&text) {
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

            // Spawn magnet download: phase 1 (metadata) then phase 2 (pieces)
            let manager = state.manager.clone();
            let opts = default_opts(&state.store);
            let cancel = tokio_util::sync::CancellationToken::new();
            let our_peer_id = crate::torrent::types::PeerId::generate();

            tokio::spawn(async move {
                eprintln!(
                    "Magnet: resolving metadata for {}",
                    magnet.display_name.as_deref().unwrap_or("?")
                );

                // Phase 1: download metadata from peers
                let result = crate::engine::magnet::download_metadata(
                    &magnet,
                    our_peer_id,
                    opts.port,
                    opts.max_peers,
                    opts.no_dht,
                    opts.lightspeed,
                    cancel.clone(),
                )
                .await;

                match result {
                    Ok((metainfo, _warm_peers)) => {
                        eprintln!(
                            "Magnet: metadata resolved — {} ({:.2} MiB)",
                            metainfo.info.name,
                            metainfo.info.total_length as f64 / (1024.0 * 1024.0),
                        );
                        let metainfo_bytes = crate::bencode::encode::encode(
                            &crate::bencode::value::BValue::Bytes(vec![]),
                        ); // placeholder — magnet doesn't have raw bytes
                        manager.add_torrent(metainfo, metainfo_bytes, opts).await;
                    }
                    Err(e) => {
                        eprintln!("Magnet: metadata resolution failed: {e}");
                    }
                }
            });

            return (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "status": "resolving magnet" })),
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

#[allow(clippy::too_many_lines)]
pub async fn stream_media(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let Ok(Some(entry)) = state.store.get_media(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Pick the best available version to play:
    // 1. Best Compatibility (compat-*) — H.264, plays everywhere
    // 2. Best Quality (quality-*) — remuxed, may be HEVC (Safari only)
    // 3. transcoded_path fallback
    // 4. Original file if Skipped
    let file_path = {
        // Prefer compat version (plays everywhere)
        let compat = entry
            .versions
            .iter()
            .find(|(k, _)| k.starts_with("compat-"))
            .map(|(_, v)| v.clone());
        // Then quality version
        let quality = entry
            .versions
            .iter()
            .find(|(k, _)| k.starts_with("quality-"))
            .map(|(_, v)| v.clone());
        // Pick best available
        if let Some(path) = compat {
            path
        } else if let Some(path) = quality {
            path
        } else if let crate::engine::library::TranscodeState::Ready { output_path } =
            &entry.transcode_state
        {
            output_path.clone()
        } else if entry.transcode_state == crate::engine::library::TranscodeState::Skipped {
            entry.media_file.clone()
        } else {
            return StatusCode::NOT_FOUND.into_response();
        }
    };

    // For HLS: rewrite .m3u8 segment URLs to point at our segment API
    let is_hls = file_path.extension().and_then(|e| e.to_str()) == Some("m3u8");
    if is_hls {
        let Ok(content) = tokio::fs::read_to_string(&file_path).await else {
            return StatusCode::NOT_FOUND.into_response();
        };
        // Rewrite segment filenames to API URLs
        let rewritten: String = content
            .lines()
            .map(|line| {
                if !line.starts_with('#') && !line.is_empty() {
                    format!("/api/v1/media/{id}/segment/{line}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        return (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "application/vnd.apple.mpegurl".to_string(),
            )],
            rewritten,
        )
            .into_response();
    }

    let Ok(file) = tokio::fs::File::open(&file_path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(metadata) = file.metadata().await else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
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

/// Serve HLS .ts segment files for a media entry.
pub async fn stream_segment(
    State(state): State<Arc<AppState>>,
    Path((id, filename)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    let Ok(Some(entry)) = state.store.get_media(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let base_dir = match &entry.transcode_state {
        crate::engine::library::TranscodeState::Ready { output_path } => output_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf(),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };

    // Security: prevent path traversal
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let segment_path = base_dir.join(&filename);
    let Ok(file) = tokio::fs::File::open(&segment_path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let content_type = if ext.eq_ignore_ascii_case("ts") {
        "video/mp2t"
    } else if ext.eq_ignore_ascii_case("m3u8") {
        "application/vnd.apple.mpegurl"
    } else {
        "application/octet-stream"
    };

    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, content_type.to_string())],
        body,
    )
        .into_response()
}

// ── Settings ──

pub async fn get_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let settings = state.store.get_settings();
    // Filter sensitive fields — never expose API keys to the frontend
    Json(serde_json::json!({
        "lightspeed": settings.lightspeed,
        "max_download_speed": settings.max_download_speed,
        "download_dir": settings.download_dir,
        "media_scan_dir": settings.media_scan_dir,
        "auto_transcode": settings.auto_transcode,
        "default_preset": settings.default_preset,
        "default_container": settings.default_container,
        "enable_chunking": settings.enable_chunking,
        "transcode_concurrency": settings.transcode_concurrency,
        "safari_mode": settings.safari_mode,
    }))
}

pub async fn put_settings(
    State(state): State<Arc<AppState>>,
    Json(mut settings): Json<crate::engine::store::AppSettings>,
) -> impl IntoResponse {
    // Preserve sensitive fields not exposed to frontend
    let existing = state.store.get_settings();
    if settings.tmdb_api_key.is_empty() {
        settings.tmdb_api_key = existing.tmdb_api_key;
    }
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
    let Ok(Some(entry)) = state.store.get_media(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "not found".into(),
            }),
        )
            .into_response();
    };

    if matches!(
        entry.transcode_state,
        crate::engine::library::TranscodeState::Transcoding { .. }
    ) {
        return (
            StatusCode::CONFLICT,
            Json(ApiError {
                error: "already transcoding".into(),
            }),
        )
            .into_response();
    }

    let output_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".movies")
        .join("transcoded");
    let sanitized = crate::engine::library::sanitize_filename(&entry.title);
    let ep_suffix = match (entry.season, entry.episode) {
        (Some(s), Some(e)) => format!("-s{s:02}e{e:02}"),
        _ => String::new(),
    };
    let ext = if req.container == "hls" {
        "m3u8"
    } else {
        "mp4"
    };
    let output_path = output_dir.join(format!("{sanitized}{ep_suffix}.{ext}"));

    let settings = state.store.get_settings();
    let job = crate::transcode::runner::TranscodeJob {
        media_id: id,
        input_path: entry.media_file,
        output_path,
        preset_name: req.preset,
        container: req.container,
        enable_chunking: settings.enable_chunking,
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

// ── Cancel Transcode ──

pub async fn cancel_transcode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    state.transcode.cancel(&id);
    let _ = state.store.update_transcode_state(
        &id,
        crate::engine::library::TranscodeState::Failed {
            error: "Cancelled".into(),
        },
    );
    StatusCode::OK
}

// ── Batch Transcode (group) ──

#[derive(serde::Deserialize)]
pub struct SeasonQuery {
    pub season: Option<u16>,
}

pub async fn transcode_all(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Query(query): Query<SeasonQuery>,
) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();
    let group_entries: Vec<_> = entries
        .into_iter()
        .filter(|e| e.group_id.as_ref() == Some(&group_id))
        .filter(|e| query.season.is_none() || e.season == query.season)
        .filter(|e| {
            matches!(
                e.transcode_state,
                crate::engine::library::TranscodeState::Pending
                    | crate::engine::library::TranscodeState::Failed { .. }
                    | crate::engine::library::TranscodeState::Unavailable
            )
        })
        .collect();

    let settings = state.store.get_settings();
    let mut queued = 0u32;

    for entry in &group_entries {
        let output_dir = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".movies")
            .join("transcoded");
        let sanitized = crate::engine::library::sanitize_filename(&entry.title);
        // Include season/episode in filename to avoid collisions
        let ep_suffix = match (entry.season, entry.episode) {
            (Some(s), Some(e)) => format!("-s{s:02}e{e:02}"),
            _ => String::new(),
        };
        let output_path = output_dir.join(format!("{sanitized}{ep_suffix}.mp4"));

        let job = crate::transcode::runner::TranscodeJob {
            media_id: entry.id,
            input_path: entry.media_file.clone(),
            output_path,
            preset_name: settings.default_preset.clone(),
            container: "mp4".into(),
            enable_chunking: settings.enable_chunking,
        };

        // Reset state to Pending before queueing
        let _ = state
            .store
            .update_transcode_state(&entry.id, crate::engine::library::TranscodeState::Pending);

        let tc = state.transcode.clone();
        tokio::spawn(async move {
            tc.submit(job).await;
        });
        queued += 1;
    }

    Json(serde_json::json!({
        "queued": queued,
        "total": group_entries.len(),
    }))
}

pub async fn stop_group_transcode(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Query(query): Query<SeasonQuery>,
) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();
    let mut cancelled = 0u32;

    for entry in &entries {
        if entry.group_id.as_ref() == Some(&group_id)
            && (query.season.is_none() || entry.season == query.season)
            && matches!(
                entry.transcode_state,
                crate::engine::library::TranscodeState::Transcoding { .. }
            )
        {
            state.transcode.cancel(&entry.id);
            cancelled += 1;
        }
    }

    Json(serde_json::json!({ "cancelled": cancelled }))
}

// ── Refresh Group Metadata ──

#[allow(clippy::too_many_lines)]
pub async fn refresh_group_metadata(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Query(query): Query<SeasonQuery>,
) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();
    let group_entries: Vec<_> = entries
        .into_iter()
        .filter(|e| e.group_id.as_ref() == Some(&group_id))
        .filter(|e| query.season.is_none() || e.season == query.season)
        .collect();

    if group_entries.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "no entries found".into(),
            }),
        )
            .into_response();
    }

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
    let first = group_entries[0].clone();
    let is_show = first.show_name.is_some();

    tokio::spawn(async move {
        let search_title = first.show_name.as_deref().unwrap_or(&first.title);
        let (clean_title, parsed_year) = crate::engine::library::parse_media_title(search_title);
        let search_year = first.year.or(parsed_year);
        eprintln!("TMDB: searching for \"{clean_title}\" (is_show: {is_show})");

        if let Some(meta) = crate::tmdb::fetch_metadata_auto(
            &settings.tmdb_api_key,
            &clean_title,
            search_year,
            is_show,
        )
        .await
        {
            let tmdb_id = meta.tmdb_id;
            for entry in &group_entries {
                if let Ok(Some(mut e)) = store.get_media(&entry.id) {
                    if let Some(ref title) = meta.title {
                        e.title.clone_from(title);
                    }
                    e.poster_url.clone_from(&meta.poster_url);
                    e.overview.clone_from(&meta.overview);
                    e.rating = meta.rating;
                    e.cast.clone_from(&meta.cast);
                    e.director.clone_from(&meta.director);
                    e.tmdb_id = Some(tmdb_id);
                    if meta.year.is_some() && e.year.is_none() {
                        e.year = meta.year;
                    }
                    let _ = store.put_media(&e);
                }
            }
            eprintln!(
                "TMDB: updated {} entries with show metadata (tmdb_id: {tmdb_id})",
                group_entries.len()
            );

            // Fetch per-episode data using the stored TMDB ID
            if is_show {
                let seasons: std::collections::BTreeSet<u16> =
                    group_entries.iter().filter_map(|e| e.season).collect();

                for season_num in seasons {
                    eprintln!("TMDB: fetching episode data for tmdb_id={tmdb_id} S{season_num:02}");
                    if let Some(ep_data) = crate::tmdb::fetch_season_episodes(
                        &settings.tmdb_api_key,
                        Some(tmdb_id),
                        &clean_title,
                        season_num,
                    )
                    .await
                    {
                        for entry in &group_entries {
                            if entry.season != Some(season_num) {
                                continue;
                            }
                            if let Some(ep_num) = entry.episode
                                && let Some(ep_meta) = ep_data.get(&ep_num)
                                && let Ok(Some(mut e)) = store.get_media(&entry.id)
                            {
                                if !ep_meta.name.is_empty() {
                                    e.episode_title = Some(ep_meta.name.clone());
                                }
                                if !ep_meta.overview.is_empty() {
                                    e.overview = Some(ep_meta.overview.clone());
                                }
                                let _ = store.put_media(&e);
                            }
                        }
                        eprintln!(
                            "TMDB: updated episode data for S{season_num:02} ({} episodes)",
                            ep_data.len()
                        );
                    }
                }
            }
        } else {
            eprintln!("TMDB: no results for \"{clean_title}\"");
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "refreshing" })),
    )
        .into_response()
}

// ── Groups ──

pub async fn list_groups(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();

    let mut groups: std::collections::HashMap<
        Option<Uuid>,
        Vec<crate::engine::library::MediaEntry>,
    > = std::collections::HashMap::new();
    for entry in entries {
        groups.entry(entry.group_id).or_default().push(entry);
    }

    // Sort episodes within each group by season, then episode
    for episodes in groups.values_mut() {
        episodes.sort_by_key(|e| (e.season.unwrap_or(0), e.episode.unwrap_or(0)));
    }

    Json(serde_json::json!({
        "groups": groups.iter().map(|(gid, entries)| {
            let first = &entries[0];
            serde_json::json!({
                "group_id": gid,
                "show_name": first.show_name,
                "title": first.title,
                "poster_url": first.poster_url,
                "overview": first.overview,
                "rating": first.rating,
                "is_show": first.show_name.is_some(),
                "episode_count": entries.len(),
                "season_count": entries.iter().filter_map(|e| e.season).collect::<std::collections::BTreeSet<_>>().len(),
                "entries": entries,
            })
        }).collect::<Vec<_>>(),
    }))
}

// ── Filesystem Browse ──

#[derive(serde::Deserialize)]
pub struct BrowseQuery {
    pub path: Option<String>,
}

pub async fn browse_filesystem(Query(query): Query<BrowseQuery>) -> impl IntoResponse {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let path = query
        .path
        .map_or_else(|| home.clone(), std::path::PathBuf::from);

    // Security: canonicalize to resolve symlinks, restrict to home directory
    let Ok(canonical) = std::fs::canonicalize(&path) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "path not found".into(),
            }),
        )
            .into_response();
    };
    let canonical_home = std::fs::canonicalize(&home).unwrap_or_else(|_| home.clone());
    if !canonical.starts_with(&canonical_home) {
        return (
            StatusCode::FORBIDDEN,
            Json(ApiError {
                error: "access denied: path outside home directory".into(),
            }),
        )
            .into_response();
    }

    if !canonical.is_dir() {
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
    let Ok(Some(entry)) = state.store.get_media(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "not found".into(),
            }),
        )
            .into_response();
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
    let raw_title = entry.title.clone();
    tokio::spawn(async move {
        // Clean the title before searching — strip quality tags, extract year
        let (clean_title, parsed_year) = crate::engine::library::parse_media_title(&raw_title);
        let search_year = entry.year.or(parsed_year);
        let is_show = entry.show_name.is_some();
        let search_title = if is_show {
            entry
                .show_name
                .as_deref()
                .unwrap_or(&clean_title)
                .to_string()
        } else {
            clean_title.clone()
        };
        eprintln!(
            "TMDB: searching for \"{search_title}\" (year: {search_year:?}, is_show: {is_show})"
        );

        match crate::tmdb::fetch_metadata_auto(
            &settings.tmdb_api_key,
            &search_title,
            search_year,
            is_show,
        )
        .await
        {
            Some(meta) => {
                if let Ok(Some(mut entry)) = store.get_media(&id) {
                    if let Some(ref title) = meta.title {
                        entry.title.clone_from(title);
                    }
                    entry.poster_url = meta.poster_url;
                    entry.overview = meta.overview;
                    entry.rating = meta.rating;
                    entry.cast = meta.cast;
                    entry.director = meta.director;
                    entry.tmdb_id = Some(meta.tmdb_id);
                    if meta.year.is_some() && entry.year.is_none() {
                        entry.year = meta.year;
                    }
                    let _ = store.put_media(&entry);
                    eprintln!(
                        "TMDB: refreshed metadata for \"{}\" (tmdb_id: {})",
                        entry.title, meta.tmdb_id
                    );
                }
            }
            None => {
                eprintln!("TMDB: no results for \"{clean_title}\" (year: {search_year:?})");
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "refreshing" })),
    )
        .into_response()
}

#[allow(clippy::too_many_lines)]
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

    // Parse episode info for all new files and group by show name
    let mut show_groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    let mut file_infos: Vec<(
        std::path::PathBuf,
        crate::engine::library::EpisodeInfo,
        String,
        Option<u16>,
    )> = Vec::new();

    // Build a map of existing entries by file path for updating orphans
    let existing_entries: std::collections::HashMap<
        std::path::PathBuf,
        crate::engine::library::MediaEntry,
    > = state
        .store
        .list_media()
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.media_file.clone(), e))
        .collect();

    for video_file in &video_files {
        let filename = video_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown");
        let episode_info = crate::engine::library::parse_episode_info(filename);
        let (title, year) = crate::engine::library::parse_media_title(filename);

        if episode_info.is_show {
            show_groups
                .entry(episode_info.show_name.clone())
                .or_default()
                .push(file_infos.len());
        }

        let is_existing = existing.contains(video_file);
        file_infos.push((video_file.clone(), episode_info, title, year));
        if is_existing {
            skipped += 1;
        }
    }

    // Assign group_ids for shows — reuse existing group_id if any entry in the group already has one
    let mut group_map: std::collections::HashMap<String, Uuid> = std::collections::HashMap::new();
    for (show_name, indices) in &show_groups {
        // Check if any existing entry for this show already has a group_id
        let existing_gid = indices.iter().find_map(|&i| {
            let path = &file_infos[i].0;
            existing_entries.get(path).and_then(|e| e.group_id)
        });
        group_map.insert(show_name.clone(), existing_gid.unwrap_or_else(Uuid::new_v4));
    }

    // Track media_ids per show for group TMDB fetch
    let mut show_media_ids: std::collections::HashMap<String, Vec<Uuid>> =
        std::collections::HashMap::new();

    for (video_file, episode_info, title, year) in &file_infos {
        // Update existing orphan entries (missing group_id) to join their group
        if let Some(mut existing_entry) = existing_entries.get(video_file).cloned() {
            if episode_info.is_show && existing_entry.group_id.is_none() {
                let gid = group_map.get(&episode_info.show_name).copied();
                existing_entry.group_id = gid;
                existing_entry.show_name = Some(episode_info.show_name.clone());
                existing_entry.season = episode_info.season;
                existing_entry.episode = episode_info.episode;
                existing_entry.media_type = crate::engine::library::MediaType::Show;
                let _ = state.store.put_media(&existing_entry);
                eprintln!(
                    "Library: updated orphan \"{}\" → group",
                    existing_entry.title
                );
            }
            continue; // Don't create a duplicate
        }

        let is_web = crate::engine::library::is_web_compatible(video_file);
        let file_size = std::fs::metadata(video_file).map(|m| m.len()).unwrap_or(0);

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

        let group_id = if episode_info.is_show {
            group_map.get(&episode_info.show_name).copied()
        } else {
            None
        };

        let media_id = Uuid::new_v4();
        let entry = crate::engine::library::MediaEntry {
            id: media_id,
            title: if episode_info.is_show {
                episode_info.show_name.clone()
            } else {
                title.clone()
            },
            year: *year,
            media_type,
            original_path: video_file.parent().unwrap_or(&scan_path).to_path_buf(),
            media_file: video_file.clone(),
            transcoded_path: None,
            transcode_state: transcode_state.clone(),
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
            episode_title: episode_info.episode_title.clone(),
            group_id,
            tmdb_id: None,
        };

        let _ = state.store.put_media(&entry);

        if episode_info.is_show {
            show_media_ids
                .entry(episode_info.show_name.clone())
                .or_default()
                .push(media_id);
        } else {
            // Fetch movie metadata individually
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
                        entry.title.clone_from(title);
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
            transcode_state,
            crate::engine::library::TranscodeState::Pending
        ) {
            let settings = state.store.get_settings();
            if settings.auto_transcode {
                let output_dir = dirs::home_dir()
                    .unwrap_or_else(|| std::path::PathBuf::from("."))
                    .join(".movies")
                    .join("transcoded");
                let sanitized = crate::engine::library::sanitize_filename(&entry.title);
                let ep_suffix = match (entry.season, entry.episode) {
                    (Some(s), Some(e)) => format!("-s{s:02}e{e:02}"),
                    _ => String::new(),
                };
                let ext = if settings.default_container == "hls" {
                    "m3u8"
                } else {
                    "mp4"
                };
                let output_path = output_dir.join(format!("{sanitized}{ep_suffix}.{ext}"));

                let job = crate::transcode::runner::TranscodeJob {
                    media_id: entry.id,
                    input_path: entry.media_file.clone(),
                    output_path,
                    preset_name: settings.default_preset.clone(),
                    container: settings.default_container.clone(),
                    enable_chunking: settings.enable_chunking,
                };
                let tc = state.transcode.clone();
                tokio::spawn(async move {
                    tc.submit(job).await;
                });
            }
        }

        added += 1;
    }

    // Fetch TMDB metadata once per show group, apply to all entries
    for (show_name, media_ids) in show_media_ids {
        let store_for_tmdb = state.store.clone();
        tokio::spawn(async move {
            let settings = store_for_tmdb.get_settings();
            if settings.tmdb_api_key.is_empty() {
                return;
            }
            eprintln!("TMDB: searching TV for \"{show_name}\"");
            if let Some(meta) =
                crate::tmdb::fetch_tv_metadata(&settings.tmdb_api_key, &show_name).await
            {
                for mid in &media_ids {
                    if let Ok(Some(mut entry)) = store_for_tmdb.get_media(mid) {
                        entry.poster_url.clone_from(&meta.poster_url);
                        entry.overview.clone_from(&meta.overview);
                        entry.rating = meta.rating;
                        entry.cast.clone_from(&meta.cast);
                        entry.director.clone_from(&meta.director);
                        if meta.year.is_some() && entry.year.is_none() {
                            entry.year = meta.year;
                        }
                        let _ = store_for_tmdb.put_media(&entry);
                    }
                }
                eprintln!("TMDB: fetched TV metadata for \"{show_name}\"");
            }
        });
    }

    Json(serde_json::json!({ "added": added, "skipped": skipped })).into_response()
}
