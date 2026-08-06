use std::sync::Arc;

use axum::Json;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use uuid::Uuid;

use super::ApiError;
use crate::engine::manager::DownloadOptions;
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
                    Ok((metainfo, _warm_peers)) => {
                        eprintln!(
                            "Magnet: metadata resolved — {} ({:.2} MiB)",
                            metainfo.info.name,
                            metainfo.info.total_length as f64 / (1024.0 * 1024.0),
                        );
                        let metainfo_bytes = crate::bencode::encode::encode(
                            &crate::bencode::value::BValue::Bytes(vec![]),
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
    let settings = store.get_settings();
    DownloadOptions {
        port: 6881,
        max_peers: 80,
        output_dir: settings.download_dir,
        no_dht: false,
        lightspeed: settings.lightspeed,
    }
}
