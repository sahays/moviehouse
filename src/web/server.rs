use std::sync::Arc;

use axum::Router;
use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::Embed;
use tower_http::cors::CorsLayer;

use crate::engine::manager::SessionManager;
use crate::engine::store::Store;
use crate::transcode::runner::TranscodeHandle;

#[derive(Embed)]
#[folder = "frontend/dist"]
struct FrontendAssets;

pub struct AppState {
    pub manager: Arc<SessionManager>,
    pub store: Arc<Store>,
    pub transcode: TranscodeHandle,
}

pub fn create_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route(
            "/api/v1/torrents",
            axum::routing::get(super::api::list_torrents).post(super::api::add_torrent),
        )
        .route(
            "/api/v1/torrents/{id}",
            axum::routing::get(super::api::get_torrent).delete(super::api::delete_torrent),
        )
        .route("/api/v1/ws", axum::routing::get(super::ws::ws_handler))
        .route(
            "/api/v1/library",
            axum::routing::get(super::api::list_library),
        )
        .route(
            "/api/v1/library/{id}",
            axum::routing::get(super::api::get_library_item)
                .delete(super::api::delete_library_item),
        )
        .route(
            "/api/v1/library/{id}/refresh",
            axum::routing::post(super::api::refresh_metadata),
        )
        .route(
            "/api/v1/media/{id}/stream",
            axum::routing::get(super::api::stream_media),
        )
        .route(
            "/api/v1/system/status",
            axum::routing::get(super::api::system_status),
        )
        .route(
            "/api/v1/settings",
            axum::routing::get(super::api::get_settings).put(super::api::put_settings),
        )
        .route(
            "/api/v1/transcode/presets",
            axum::routing::get(super::api::list_presets),
        )
        .route(
            "/api/v1/library/{id}/transcode",
            axum::routing::post(super::api::manual_transcode),
        )
        .route(
            "/api/v1/library/scan",
            axum::routing::post(super::api::scan_folder),
        )
        .route(
            "/api/v1/filesystem/browse",
            axum::routing::get(super::api::browse_filesystem),
        )
        .route(
            "/api/v1/metadata/search",
            axum::routing::get(super::api::search_metadata),
        )
        .with_state(state.clone());

    Router::new()
        .merge(api)
        .fallback(static_handler)
        .layer(CorsLayer::permissive())
}

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    // Try exact file match
    if let Some(file) = FrontendAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime.as_ref())],
            file.data.into_owned(),
        )
            .into_response();
    }

    // SPA fallback: serve index.html for non-API routes
    if let Some(file) = FrontendAssets::get("index.html") {
        return Html(String::from_utf8_lossy(&file.data).to_string()).into_response();
    }

    StatusCode::NOT_FOUND.into_response()
}
