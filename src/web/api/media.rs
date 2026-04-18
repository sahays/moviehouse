use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::{Multipart, Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::web::server::AppState;

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
    // 1. h264 version (universal)
    // 2. hevc version (remuxed)
    // 3. Any other version
    // 4. transcoded_path fallback
    // 5. Original file if Skipped
    let file_path = {
        let h264_ver = entry.versions.get("h264").cloned();
        let hevc_ver = entry.versions.get("hevc").cloned();
        let any_ver = entry.versions.values().next().cloned();
        if let Some(path) = h264_ver {
            path
        } else if let Some(path) = hevc_ver {
            path
        } else if let Some(path) = any_ver {
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

    // Guard against 0-byte files (prevents u64 underflow in range math)
    if total_size == 0 {
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "video/mp4".to_string()),
                (header::CONTENT_LENGTH, "0".to_string()),
            ],
            Body::empty(),
        )
            .into_response();
    }

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

/// List available subtitle tracks for a media entry.
/// Performs lazy re-detection if no subtitles are stored.
pub async fn list_subtitles(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let Ok(Some(entry)) = state.store.get_media(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let subs: Vec<serde_json::Value> = entry
        .subtitles
        .iter()
        .enumerate()
        .map(|(i, s)| {
            serde_json::json!({
                "index": i,
                "label": s.label,
                "language": s.language,
                "format": s.format,
            })
        })
        .collect();

    Json(subs).into_response()
}

/// Serve a subtitle file, converting SRT to VTT on the fly.
pub async fn stream_subtitle(
    State(state): State<Arc<AppState>>,
    Path((id, index)): Path<(Uuid, usize)>,
) -> impl IntoResponse {
    let Ok(Some(entry)) = state.store.get_media(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Some(track) = entry.subtitles.get(index) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let Ok(raw_bytes) = tokio::fs::read(&track.path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let content = String::from_utf8_lossy(&raw_bytes);

    let vtt = match track.format.as_str() {
        "vtt" => content.into_owned(),
        "srt" => srt_to_vtt(&content),
        _ => srt_to_vtt(&content), // best-effort for other formats
    };

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/vtt; charset=utf-8".to_string())],
        vtt,
    )
        .into_response()
}

/// Convert SRT subtitle format to WebVTT.
fn srt_to_vtt(srt: &str) -> String {
    let mut vtt = String::from("WEBVTT\n\n");
    for line in srt.lines() {
        if line.contains("-->") {
            vtt.push_str(&line.replace(',', "."));
        } else {
            vtt.push_str(line);
        }
        vtt.push('\n');
    }
    vtt
}

#[derive(serde::Deserialize)]
pub struct ProgressRequest {
    pub position: f64,
    pub duration: f64,
}

/// Update playback progress for a media entry.
pub async fn update_progress(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<ProgressRequest>,
) -> impl IntoResponse {
    if !req.position.is_finite()
        || !req.duration.is_finite()
        || req.position < 0.0
        || req.duration <= 0.0
        || req.position > req.duration
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match state.store.update_play_progress(&id, req.position, req.duration) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// Upload a subtitle file for a media entry.
/// Accepts multipart form with a "file" field containing .srt or .vtt.
pub async fn upload_subtitle(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let Ok(Some(mut entry)) = state.store.get_media(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };

    // Read the uploaded file
    let mut file_bytes = Vec::new();
    let mut original_name = String::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            original_name = field
                .file_name()
                .unwrap_or("subtitle.srt")
                .to_string();
            if let Ok(bytes) = field.bytes().await {
                file_bytes = bytes.to_vec();
            }
            break;
        }
    }

    const MAX_SUBTITLE_SIZE: usize = 10 * 1024 * 1024; // 10 MB

    if file_bytes.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "no file uploaded" })),
        )
            .into_response();
    }

    if file_bytes.len() > MAX_SUBTITLE_SIZE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(serde_json::json!({ "error": "subtitle file too large (max 10 MB)" })),
        )
            .into_response();
    }

    // Sanitize filename
    if original_name.contains('/')
        || original_name.contains('\\')
        || original_name.contains("..")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid filename" })),
        )
            .into_response();
    }

    // Determine output path: save next to the video file (or transcoded output)
    let base_dir = entry
        .versions
        .values()
        .next()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .or_else(|| entry.media_file.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let video_stem = entry
        .versions
        .values()
        .next()
        .or(Some(&entry.media_file))
        .and_then(|p| p.file_stem().and_then(|s| s.to_str()))
        .unwrap_or("media");

    let ext = std::path::Path::new(&original_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("srt")
        .to_lowercase();

    let allowed_exts = ["srt", "vtt", "ass", "ssa"];
    if !allowed_exts.contains(&ext.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "unsupported subtitle format (use .srt, .vtt, .ass, .ssa)" })),
        )
            .into_response();
    }

    // Generate unique filename
    let idx = entry.subtitles.len();
    let out_filename = format!("{video_stem}.uploaded{idx}.{ext}");
    let out_path = base_dir.join(&out_filename);

    if let Err(e) = tokio::fs::write(&out_path, &file_bytes).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Parse language from filename if possible
    let name_stem = std::path::Path::new(&original_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let language = name_stem
        .rsplit('.')
        .next()
        .and_then(|s| crate::engine::library::normalize_language_code(s));
    let label = language
        .as_deref()
        .map(crate::engine::library::language_code_to_label)
        .unwrap_or_else(|| original_name.clone());

    entry.subtitles.push(crate::engine::types::SubtitleTrack {
        label,
        language,
        path: out_path,
        format: ext,
    });
    let _ = state.store.put_media(&entry);

    Json(serde_json::json!({ "status": "ok", "count": entry.subtitles.len() })).into_response()
}
