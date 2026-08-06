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

/// Liveness probe. Public and unauthenticated — as is everything else.
async fn health() -> &'static str {
    "ok"
}

/// Build the API router.
///
/// **There is no authentication.** `MovieHouse` binds to the LAN and trusts
/// everything that can reach it: the whole API — torrent engine, filesystem
/// browser, settings, and the media-deleting cleanup route — is open to any
/// device on the network. Bind to `127.0.0.1` if that is not what you want, and
/// do not expose this to the internet.
#[allow(clippy::too_many_lines)]
pub fn create_router(state: &Arc<AppState>) -> Router {
    use super::api::{filesystem, library, media, settings, torrents, transcode};

    let api = Router::new()
        .route("/health", axum::routing::get(health))
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
            "/api/v1/library/health",
            axum::routing::get(library::library_health),
        )
        .route(
            "/api/v1/library/{id}",
            axum::routing::get(library::get_library_item).delete(library::delete_library_item),
        )
        .route(
            "/api/v1/library/cleanup",
            axum::routing::post(library::cleanup_sources),
        )
        .route(
            "/api/v1/library/fix-paths",
            axum::routing::post(library::fix_paths),
        )
        .route(
            "/api/v1/library/{id}/refresh",
            axum::routing::post(library::refresh_metadata),
        )
        .route(
            "/api/v1/media/{id}/subtitles",
            axum::routing::get(media::list_subtitles).post(media::upload_subtitle),
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
            "/api/v1/library/migrate",
            axum::routing::post(settings::migrate_media),
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
        // Byte-serving routes an external player fetches by URL. An AirPlay or
        // Chromecast receiver is a separate HTTP client on another device; with
        // nothing to authenticate against, it just fetches these directly.
        .route(
            "/api/v1/media/{id}/stream",
            axum::routing::get(media::stream_media),
        )
        .route(
            "/api/v1/media/{id}/segment/{filename}",
            axum::routing::get(media::stream_segment),
        )
        .route(
            "/api/v1/media/{id}/subtitles/{index}",
            axum::routing::get(media::stream_subtitle),
        )
        .with_state(state.clone());

    // CORS: allow any origin for LAN access (phones, TVs, other devices).
    Router::new()
        .merge(api)
        .fallback(static_handler)
        .layer(
            CorsLayer::new()
                .allow_origin(tower_http::cors::Any)
                .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
                .allow_headers([header::CONTENT_TYPE]),
        )
        // Outermost so every response carries them, including the static SPA.
        // Replaces the headers the nginx vhost used to add.
        .layer(axum::middleware::from_fn(
            super::security_headers::security_headers,
        ))
}

async fn static_handler(uri: Uri) -> Response {
    // An unmatched `/api/v1/*` path is a client bug, not a deep link. Without
    // this it would fall through to the SPA and answer 200 text/html, so a stale
    // client calling a removed endpoint (the auth routes, say) would read the
    // index page as success.
    if uri.path().starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(super::api::ApiError {
                error: "no such endpoint".into(),
            }),
        )
            .into_response();
    }

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
