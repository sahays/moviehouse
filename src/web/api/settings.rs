use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;

use super::ApiError;
use crate::web::server::AppState;

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
        "transcode_concurrency": settings.transcode_concurrency,
        "transcode_dir": settings.transcode_dir,
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

pub async fn list_presets() -> impl IntoResponse {
    Json(crate::transcode::presets::builtin_presets())
}

#[allow(clippy::too_many_lines)]
/// Move all transcoded media files to a new directory, updating all paths in the database.
pub async fn migrate_media(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MigrateRequest>,
) -> impl IntoResponse {
    let base = std::path::PathBuf::from(&req.path);
    let new_dir = if base.file_name().and_then(|n| n.to_str()) == Some("moviehouse") {
        base
    } else {
        base.join("moviehouse")
    };

    // Create the destination directory
    if let Err(e) = std::fs::create_dir_all(&new_dir) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": format!("Cannot create directory: {e}") })),
        )
            .into_response();
    }

    let entries = state.store.list_media().unwrap_or_default();
    let mut moved = 0u32;
    let mut errors = 0u32;
    let mut bytes_moved = 0u64;

    for mut e in entries {
        let mut changed = false;

        // Move version files; drop versions whose source no longer exists
        let mut live_versions = std::collections::HashMap::new();
        for (name, path) in &e.versions {
            match move_file(path, &new_dir) {
                MoveResult::Moved(new_path, size) => {
                    live_versions.insert(name.clone(), new_path);
                    moved += 1;
                    bytes_moved += size;
                    changed = true;
                }
                MoveResult::AlreadyThere => {
                    live_versions.insert(name.clone(), path.clone());
                }
                MoveResult::NotFound | MoveResult::Failed => {
                    changed = true;
                }
            }
        }
        e.versions = live_versions;

        // Move transcoded_path
        if let Some(ref mut tp) = e.transcoded_path {
            match move_file(tp, &new_dir) {
                MoveResult::Moved(new_path, size) => {
                    *tp = new_path;
                    changed = true;
                    moved += 1;
                    bytes_moved += size;
                }
                MoveResult::NotFound | MoveResult::Failed => {
                    if let Some(v) = e.versions.values().next() {
                        tp.clone_from(v);
                        changed = true;
                    }
                }
                MoveResult::AlreadyThere => {}
            }
        }

        // Move subtitle files; drop missing ones
        let mut live_subs = Vec::new();
        for mut sub in e.subtitles.drain(..) {
            match move_file(&sub.path, &new_dir) {
                MoveResult::Moved(new_path, _) => {
                    sub.path = new_path;
                    live_subs.push(sub);
                    changed = true;
                }
                MoveResult::AlreadyThere => {
                    live_subs.push(sub);
                }
                MoveResult::NotFound | MoveResult::Failed => {
                    changed = true;
                }
            }
        }
        e.subtitles = live_subs;

        // Update media_file
        match move_file(&e.media_file, &new_dir) {
            MoveResult::Moved(new_path, size) => {
                e.media_file = new_path;
                changed = true;
                moved += 1;
                bytes_moved += size;
            }
            MoveResult::NotFound | MoveResult::Failed => {
                if let Some(v) = e.versions.values().next() {
                    e.media_file = v.clone();
                    changed = true;
                }
            }
            MoveResult::AlreadyThere => {}
        }

        if changed && state.store.put_media(&e).is_err() {
            errors += 1;
        }
    }

    // Move any remaining files in the old directory (HLS segments, etc.)
    let mut settings = state.store.get_settings();
    let old_dir = settings.transcode_dir.clone();
    if old_dir != new_dir && old_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&old_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && let MoveResult::Moved(_, size) = move_file(&path, &new_dir)
                {
                    moved += 1;
                    bytes_moved += size;
                }
            }
        }
        // Remove old directory if now empty
        let _ = std::fs::remove_dir(&old_dir);
    }

    // Update the setting
    settings.transcode_dir = new_dir;
    let _ = state.store.put_settings(&settings);

    let mb = bytes_moved as f64 / (1024.0 * 1024.0);
    eprintln!("Media migration: moved {moved} files ({mb:.1} MB), {errors} errors");

    Json(serde_json::json!({
        "moved": moved,
        "bytes_moved": bytes_moved,
        "moved_mb": format!("{mb:.1}"),
        "errors": errors,
    }))
    .into_response()
}

#[derive(serde::Deserialize)]
pub struct MigrateRequest {
    pub path: String,
}

enum MoveResult {
    /// File was moved to the new path.
    Moved(std::path::PathBuf, u64),
    /// File already exists at destination or source == dest (no action needed).
    AlreadyThere,
    /// Source file does not exist (stale reference).
    NotFound,
    /// Move failed (I/O error).
    Failed,
}

/// Move a single file to a new directory, preserving the filename.
fn move_file(src: &std::path::Path, dest_dir: &std::path::Path) -> MoveResult {
    if !src.exists() {
        return MoveResult::NotFound;
    }
    let Some(filename) = src.file_name() else {
        return MoveResult::Failed;
    };
    let dest = dest_dir.join(filename);
    if dest == src || dest.exists() {
        return MoveResult::AlreadyThere;
    }
    let size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
    // Try rename first (fast, same filesystem), fall back to copy+delete
    if std::fs::rename(src, &dest).is_ok() {
        return MoveResult::Moved(dest, size);
    }
    // Cross-filesystem: copy then delete
    if std::fs::copy(src, &dest).is_ok() {
        if let Err(e) = std::fs::remove_file(src) {
            eprintln!(
                "Warning: copied but failed to delete source {}: {e}",
                src.display()
            );
        }
        MoveResult::Moved(dest, size)
    } else {
        eprintln!("Failed to move: {} -> {}", src.display(), dest.display());
        MoveResult::Failed
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
