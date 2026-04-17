use std::sync::Arc;

use axum::Router;
use axum::http::Method;
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

pub fn create_router(state: &Arc<AppState>) -> Router {
    use super::api::{filesystem, library, media, settings, torrents, transcode};

    let api = Router::new()
        .route(
            "/api/v1/torrents",
            axum::routing::get(torrents::list_torrents).post(torrents::add_torrent),
        )
        .route(
            "/api/v1/torrents/{id}",
            axum::routing::get(torrents::get_torrent).delete(torrents::delete_torrent),
        )
        .route("/api/v1/ws", axum::routing::get(super::ws::ws_handler))
        .route("/api/v1/library", axum::routing::get(library::list_library))
        .route(
            "/api/v1/library/{id}",
            axum::routing::get(library::get_library_item).delete(library::delete_library_item),
        )
        .route(
            "/api/v1/library/cleanup",
            axum::routing::post(library::cleanup_sources),
        )
        .route(
            "/api/v1/library/{id}/refresh",
            axum::routing::post(library::refresh_metadata),
        )
        .route(
            "/api/v1/media/{id}/stream",
            axum::routing::get(media::stream_media),
        )
        .route(
            "/api/v1/media/{id}/segment/{filename}",
            axum::routing::get(media::stream_segment),
        )
        .route(
            "/api/v1/media/{id}/subtitles",
            axum::routing::get(media::list_subtitles)
                .post(media::upload_subtitle),
        )
        .route(
            "/api/v1/media/{id}/subtitles/{index}",
            axum::routing::get(media::stream_subtitle),
        )
        .route(
            "/api/v1/media/{id}/progress",
            axum::routing::put(media::update_progress),
        )
        .route(
            "/api/v1/system/status",
            axum::routing::get(settings::system_status),
        )
        .route(
            "/api/v1/settings",
            axum::routing::get(settings::get_settings).put(settings::put_settings),
        )
        .route(
            "/api/v1/transcode/presets",
            axum::routing::get(settings::list_presets),
        )
        .route(
            "/api/v1/library/{id}/transcode",
            axum::routing::post(transcode::manual_transcode),
        )
        .route(
            "/api/v1/library/{id}/cancel-transcode",
            axum::routing::post(transcode::cancel_transcode),
        )
        .route(
            "/api/v1/library/scan",
            axum::routing::post(library::scan_folder),
        )
        .route(
            "/api/v1/filesystem/browse",
            axum::routing::get(filesystem::browse_filesystem),
        )
        .route(
            "/api/v1/metadata/search",
            axum::routing::get(filesystem::search_metadata),
        )
        .route(
            "/api/v1/library/groups",
            axum::routing::get(library::list_groups),
        )
        .route(
            "/api/v1/library/groups/{id}/transcode-all",
            axum::routing::post(transcode::transcode_all),
        )
        .route(
            "/api/v1/library/groups/{id}/stop-all",
            axum::routing::post(transcode::stop_group_transcode),
        )
        .route(
            "/api/v1/library/groups/{id}/refresh-metadata",
            axum::routing::post(library::refresh_group_metadata),
        )
        .with_state(state.clone());

    Router::new().merge(api).fallback(static_handler).layer(
        CorsLayer::new()
            .allow_origin(tower_http::cors::Any)
            .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
            .allow_headers([header::CONTENT_TYPE]),
    )
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
