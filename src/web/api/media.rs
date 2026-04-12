use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, State};
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
