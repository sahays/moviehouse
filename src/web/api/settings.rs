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

pub async fn system_status() -> impl IntoResponse {
    let available = crate::transcode::runner::ffmpeg_available();
    let version = crate::transcode::runner::ffmpeg_version();
    Json(serde_json::json!({
        "ffmpeg_available": available,
        "ffmpeg_version": version,
    }))
}
