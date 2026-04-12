use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use uuid::Uuid;

use super::{ApiError, SeasonQuery};
use crate::web::server::AppState;

#[derive(serde::Deserialize)]
pub struct TranscodeRequest {
    pub preset: String,
}

pub async fn manual_transcode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<TranscodeRequest>,
) -> impl IntoResponse {
    let Ok(Some(entry)) = state.store.get_media(&id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "not found".into(),
            }),
        )
            .into_response();
    };

    if matches!(
        entry.transcode_state,
        crate::engine::library::TranscodeState::Transcoding { .. }
    ) {
        return (
            StatusCode::CONFLICT,
            Json(ApiError {
                error: "already transcoding".into(),
            }),
        )
            .into_response();
    }

    let job = crate::transcode::job::create_job(&entry, &req.preset);

    let _ = state
        .store
        .update_transcode_state(&id, crate::engine::library::TranscodeState::Pending);

    let tc = state.transcode.clone();
    tokio::spawn(async move {
        tc.submit(job).await;
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "queued" })),
    )
        .into_response()
}

pub async fn cancel_transcode(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    state.transcode.cancel(&id);
    let _ = state.store.update_transcode_state(
        &id,
        crate::engine::library::TranscodeState::Failed {
            error: "Cancelled".into(),
        },
    );
    StatusCode::OK
}

pub async fn transcode_all(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Query(query): Query<SeasonQuery>,
) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();
    let group_entries: Vec<_> = entries
        .into_iter()
        .filter(|e| e.group_id.as_ref() == Some(&group_id))
        .filter(|e| query.season.is_none() || e.season == query.season)
        .filter(|e| {
            matches!(
                e.transcode_state,
                crate::engine::library::TranscodeState::Pending
                    | crate::engine::library::TranscodeState::Failed { .. }
                    | crate::engine::library::TranscodeState::Unavailable
            )
        })
        .collect();

    let settings = state.store.get_settings();
    let mut queued = 0u32;

    for entry in &group_entries {
        let job = crate::transcode::job::create_job(entry, &settings.default_preset);

        // Reset state to Pending before queueing
        let _ = state
            .store
            .update_transcode_state(&entry.id, crate::engine::library::TranscodeState::Pending);

        let tc = state.transcode.clone();
        tokio::spawn(async move {
            tc.submit(job).await;
        });
        queued += 1;
    }

    Json(serde_json::json!({
        "queued": queued,
        "total": group_entries.len(),
    }))
}

pub async fn stop_group_transcode(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Query(query): Query<SeasonQuery>,
) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();
    let mut cancelled = 0u32;

    for entry in &entries {
        if entry.group_id.as_ref() == Some(&group_id)
            && (query.season.is_none() || entry.season == query.season)
            && matches!(
                entry.transcode_state,
                crate::engine::library::TranscodeState::Transcoding { .. }
            )
        {
            state.transcode.cancel(&entry.id);
            cancelled += 1;
        }
    }

    Json(serde_json::json!({ "cancelled": cancelled }))
}
