use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use super::ApiError;
use crate::web::server::AppState;

#[derive(serde::Deserialize)]
pub struct BrowseQuery {
    pub path: Option<String>,
}

pub async fn browse_filesystem(Query(query): Query<BrowseQuery>) -> impl IntoResponse {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"));
    let path = query
        .path
        .map_or_else(|| home.clone(), std::path::PathBuf::from);

    // Security: canonicalize to resolve symlinks, restrict to home directory or /Volumes
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
    let volumes = std::path::Path::new("/Volumes");
    if !canonical.starts_with(&canonical_home) && !canonical.starts_with(volumes) {
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

    // Only offer parent navigation if parent is within allowed paths
    let parent = path.parent().and_then(|p| {
        let canon = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        if canon.starts_with(&canonical_home) || canon.starts_with(volumes) {
            Some(p.to_string_lossy().to_string())
        } else {
            None
        }
    });

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
