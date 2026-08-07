use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use uuid::Uuid;

use super::ApiError;
use crate::engine::manager::DownloadOptions;
use crate::engine::session::SessionState;
use crate::torrent::magnet::MagnetLink;
use crate::torrent::metainfo::Metainfo;
use crate::web::server::AppState;

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
        // Derived media first, while the entries still exist to name it.
        //
        // Transcodes and extracted subtitles live in `transcode_dir`, *outside*
        // the torrent's own folder, so removing that folder alone leaves them
        // behind forever — invisible to the app and impossible to clean up from
        // the UI. That is how orphaned multi-GB MP4s accumulate.
        let entries = state.store.list_media().unwrap_or_default();
        let (mut files, mut bytes) = (0usize, 0u64);
        for entry in entries.iter().filter(|e| e.download_id == id) {
            let (f, b) = crate::engine::cleanup::remove_owned_files(entry);
            files += f;
            bytes += b;
            let _ = state.store.remove_media(&entry.id);
        }

        // Then the downloaded payload itself (a folder for multi-file torrents,
        // a single file otherwise).
        let payload = record.output_dir.join(&record.name);
        if payload.is_dir() {
            let _ = std::fs::remove_dir_all(&payload);
        } else if payload.is_file() {
            let _ = std::fs::remove_file(&payload);
        }
        tracing::info!(
            download_id = %id, name = %record.name, derived_files = files, derived_bytes = bytes,
            "deleted download and its derived media"
        );
    }
    state.manager.remove(&id);
    StatusCode::NO_CONTENT
}

/// True when a download record no longer refers to anything real: it errored
/// out, was cancelled, or finished but its payload is no longer on disk.
///
/// Deliberately conservative about "missing files" — only a *finished* download
/// is judged that way. An in-flight one legitimately has nothing on disk yet.
fn is_stale(record: &crate::engine::store::DownloadRecord) -> bool {
    match record.status.state {
        SessionState::Error(_) | SessionState::Cancelled => true,
        SessionState::Completed => !record.output_dir.join(&record.name).exists(),
        SessionState::Downloading | SessionState::Resolving => false,
    }
}

/// Drop download records that point at nothing: errored, cancelled, or finished
/// with their files gone.
///
/// **Records only — never deletes files.** Two records can name the same folder
/// (a failed attempt plus the retry that succeeded), so removing files here
/// would let clearing a dead entry destroy a live one's data.
pub async fn cleanup_downloads(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let records = state.store.list_downloads().unwrap_or_default();
    let mut removed = 0u32;
    for record in records.iter().filter(|r| is_stale(r)) {
        state.manager.remove(&record.id);
        removed += 1;
        tracing::info!(id = %record.id, name = %record.name, "cleared stale download record");
    }
    let _ = state.store.flush();
    Json(serde_json::json!({ "removed": removed }))
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

            let opts = default_opts(&state.store);
            let display_name = magnet
                .display_name
                .clone()
                .unwrap_or_else(|| "Unknown magnet".into());
            let info_hash_hex = hex::encode(magnet.info_hash.0);

            // Register placeholder so the UI can see it immediately
            let placeholder_id = state
                .manager
                .register_magnet(display_name.clone(), info_hash_hex);

            // Spawn magnet download: phase 1 (metadata) then phase 2 (pieces)
            let manager = state.manager.clone();
            let cancel = tokio_util::sync::CancellationToken::new();
            let our_peer_id = crate::torrent::types::PeerId::generate();

            tokio::spawn(async move {
                eprintln!("Magnet: resolving metadata for {display_name}");

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
                    Ok((metainfo, metainfo_bytes, _warm_peers)) => {
                        eprintln!(
                            "Magnet: metadata resolved — {} ({:.2} MiB)",
                            metainfo.info.name,
                            metainfo.info.total_length as f64 / (1024.0 * 1024.0),
                        );
                        manager
                            .resolve_magnet(placeholder_id, metainfo, metainfo_bytes, opts)
                            .await;
                    }
                    Err(e) => {
                        eprintln!("Magnet: metadata resolution failed: {e}");
                        manager.fail_magnet(&placeholder_id, e.to_string());
                    }
                }
            });

            return (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({ "id": placeholder_id, "status": "resolving magnet" })),
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
    DownloadOptions::from_settings(&store.get_settings())
}
