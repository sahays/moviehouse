use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use uuid::Uuid;

use super::{ApiError, SeasonQuery};
use crate::web::server::AppState;

pub async fn list_library(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match state.store.list_media() {
        Ok(entries) => Json(entries).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn get_library_item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    match state.store.get_media(&id) {
        Ok(Some(entry)) => Json(entry).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "not found".into(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError {
                error: e.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn delete_library_item(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let _ = state.store.remove_media(&id);
    StatusCode::NO_CONTENT
}

/// Delete original source files for entries that have been transcoded.
/// Only deletes source files where a transcoded version (Ready state) exists.
pub async fn cleanup_sources(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();
    let mut deleted_count = 0u32;
    let mut freed_bytes = 0u64;
    let mut errors = 0u32;

    for entry in &entries {
        // Only clean up entries that have been transcoded (Ready state or have versions)
        let has_transcode = matches!(
            entry.transcode_state,
            crate::engine::types::TranscodeState::Ready { .. }
        ) || !entry.versions.is_empty();

        if !has_transcode {
            continue;
        }

        // Don't delete if source = transcoded path (same file)
        if let Some(ref tp) = entry.transcoded_path
            && *tp == entry.media_file
        {
            continue;
        }

        let source = &entry.media_file;
        if source.exists() {
            let size = std::fs::metadata(source).map(|m| m.len()).unwrap_or(0);
            match std::fs::remove_file(source) {
                Ok(()) => {
                    deleted_count += 1;
                    freed_bytes += size;
                    eprintln!("Cleanup: deleted {}", source.display());
                }
                Err(e) => {
                    eprintln!("Cleanup: failed to delete {}: {e}", source.display());
                    errors += 1;
                }
            }
        }
    }

    // Also try to clean empty parent directories
    let mut cleaned_dirs = std::collections::HashSet::new();
    for entry in &entries {
        if let Some(parent) = entry.media_file.parent()
            && cleaned_dirs.insert(parent.to_path_buf())
            && parent.exists()
        {
            // Remove dir only if empty
            let _ = std::fs::remove_dir(parent); // fails silently if not empty
        }
    }

    let freed_mb = freed_bytes as f64 / (1024.0 * 1024.0);
    eprintln!("Cleanup: deleted {deleted_count} files, freed {freed_mb:.1} MB, {errors} errors");

    Json(serde_json::json!({
        "deleted": deleted_count,
        "freed_bytes": freed_bytes,
        "freed_mb": format!("{freed_mb:.1}"),
        "errors": errors,
    }))
}

pub async fn list_groups(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();

    let mut groups: std::collections::HashMap<
        Option<Uuid>,
        Vec<crate::engine::library::MediaEntry>,
    > = std::collections::HashMap::new();
    for entry in entries {
        groups.entry(entry.group_id).or_default().push(entry);
    }

    // Sort episodes within each group by season, then episode
    for episodes in groups.values_mut() {
        episodes.sort_by_key(|e| (e.season.unwrap_or(0), e.episode.unwrap_or(0)));
    }

    Json(serde_json::json!({
        "groups": groups.iter().map(|(gid, entries)| {
            let first = &entries[0];
            serde_json::json!({
                "group_id": gid,
                "show_name": first.show_name,
                "title": first.title,
                "poster_url": first.poster_url,
                "overview": first.overview,
                "rating": first.rating,
                "is_show": first.show_name.is_some(),
                "episode_count": entries.len(),
                "season_count": entries.iter().filter_map(|e| e.season).collect::<std::collections::BTreeSet<_>>().len(),
                "entries": entries,
            })
        }).collect::<Vec<_>>(),
    }))
}

#[allow(clippy::too_many_lines)]
pub async fn refresh_group_metadata(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<Uuid>,
    Query(query): Query<SeasonQuery>,
) -> impl IntoResponse {
    let entries = state.store.list_media().unwrap_or_default();
    let group_entries: Vec<_> = entries
        .into_iter()
        .filter(|e| e.group_id.as_ref() == Some(&group_id))
        .filter(|e| query.season.is_none() || e.season == query.season)
        .collect();

    if group_entries.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiError {
                error: "no entries found".into(),
            }),
        )
            .into_response();
    }

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

    let store = state.store.clone();
    let first = group_entries[0].clone();
    let is_show = first.show_name.is_some();

    tokio::spawn(async move {
        let search_title = first.show_name.as_deref().unwrap_or(&first.title);
        let (clean_title, parsed_year) = crate::engine::library::parse_media_title(search_title);
        let search_year = first.year.or(parsed_year);
        eprintln!("TMDB: searching for \"{clean_title}\" (is_show: {is_show})");

        if let Some(meta) = crate::tmdb::fetch_metadata_auto(
            &settings.tmdb_api_key,
            &clean_title,
            search_year,
            is_show,
        )
        .await
        {
            let tmdb_id = meta.tmdb_id;
            for entry in &group_entries {
                if let Ok(Some(mut e)) = store.get_media(&entry.id) {
                    crate::tmdb::apply_metadata(&mut e, &meta);
                    let _ = store.put_media(&e);
                }
            }
            eprintln!(
                "TMDB: updated {} entries with show metadata (tmdb_id: {tmdb_id})",
                group_entries.len()
            );

            // Fetch per-episode data using the stored TMDB ID
            if is_show {
                let seasons: std::collections::BTreeSet<u16> =
                    group_entries.iter().filter_map(|e| e.season).collect();

                for season_num in seasons {
                    eprintln!("TMDB: fetching episode data for tmdb_id={tmdb_id} S{season_num:02}");
                    if let Some(ep_data) = crate::tmdb::fetch_season_episodes(
                        &settings.tmdb_api_key,
                        Some(tmdb_id),
                        &clean_title,
                        season_num,
                    )
                    .await
                    {
                        for entry in &group_entries {
                            if entry.season != Some(season_num) {
                                continue;
                            }
                            if let Some(ep_num) = entry.episode
                                && let Some(ep_meta) = ep_data.get(&ep_num)
                                && let Ok(Some(mut e)) = store.get_media(&entry.id)
                            {
                                if !ep_meta.name.is_empty() {
                                    e.episode_title = Some(ep_meta.name.clone());
                                }
                                if !ep_meta.overview.is_empty() {
                                    e.overview = Some(ep_meta.overview.clone());
                                }
                                let _ = store.put_media(&e);
                            }
                        }
                        eprintln!(
                            "TMDB: updated episode data for S{season_num:02} ({} episodes)",
                            ep_data.len()
                        );
                    }
                }
            }
        } else {
            eprintln!("TMDB: no results for \"{clean_title}\"");
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "refreshing" })),
    )
        .into_response()
}

pub async fn refresh_metadata(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
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

    let store = state.store.clone();
    let raw_title = entry.title.clone();
    tokio::spawn(async move {
        // Clean the title before searching — strip quality tags, extract year
        let (clean_title, parsed_year) = crate::engine::library::parse_media_title(&raw_title);
        let search_year = entry.year.or(parsed_year);
        let is_show = entry.show_name.is_some();
        let search_title = if is_show {
            entry
                .show_name
                .as_deref()
                .unwrap_or(&clean_title)
                .to_string()
        } else {
            clean_title.clone()
        };
        eprintln!(
            "TMDB: searching for \"{search_title}\" (year: {search_year:?}, is_show: {is_show})"
        );

        match crate::tmdb::fetch_metadata_auto(
            &settings.tmdb_api_key,
            &search_title,
            search_year,
            is_show,
        )
        .await
        {
            Some(meta) => {
                if let Ok(Some(mut entry)) = store.get_media(&id) {
                    crate::tmdb::apply_metadata(&mut entry, &meta);
                    let _ = store.put_media(&entry);
                    eprintln!(
                        "TMDB: refreshed metadata for \"{}\" (tmdb_id: {})",
                        entry.title, meta.tmdb_id
                    );
                }
            }
            None => {
                eprintln!("TMDB: no results for \"{clean_title}\" (year: {search_year:?})");
            }
        }
    });

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "status": "refreshing" })),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
pub struct ScanRequest {
    pub path: String,
}

#[allow(clippy::too_many_lines)]
pub async fn scan_folder(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ScanRequest>,
) -> impl IntoResponse {
    let scan_path = std::path::PathBuf::from(&req.path);
    if !scan_path.is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiError {
                error: "path is not a directory".into(),
            }),
        )
            .into_response();
    }

    let video_files = crate::engine::library::detect_video_files(&scan_path);

    // Get existing media file paths to avoid duplicates
    let existing: std::collections::HashSet<std::path::PathBuf> = state
        .store
        .list_media()
        .unwrap_or_default()
        .iter()
        .map(|e| e.media_file.clone())
        .collect();

    let mut added = 0u32;
    let mut skipped = 0u32;

    // Parse episode info for all new files and group by show name
    let mut show_groups: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    let mut file_infos: Vec<(
        std::path::PathBuf,
        crate::engine::library::EpisodeInfo,
        String,
        Option<u16>,
    )> = Vec::new();

    // Build a map of existing entries by file path for updating orphans
    let existing_entries: std::collections::HashMap<
        std::path::PathBuf,
        crate::engine::library::MediaEntry,
    > = state
        .store
        .list_media()
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.media_file.clone(), e))
        .collect();

    for video_file in &video_files {
        let filename = video_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown");
        let episode_info = crate::engine::library::parse_episode_info(filename);
        let (title, year) = crate::engine::library::parse_media_title(filename);

        if episode_info.is_show {
            show_groups
                .entry(episode_info.show_name.clone())
                .or_default()
                .push(file_infos.len());
        }

        let is_existing = existing.contains(video_file);
        file_infos.push((video_file.clone(), episode_info, title, year));
        if is_existing {
            skipped += 1;
        }
    }

    // Assign group_ids for shows — reuse existing group_id if any entry in the group already has one
    let mut group_map: std::collections::HashMap<String, Uuid> = std::collections::HashMap::new();
    for (show_name, indices) in &show_groups {
        // Check if any existing entry for this show already has a group_id
        let existing_gid = indices.iter().find_map(|&i| {
            let path = &file_infos[i].0;
            existing_entries.get(path).and_then(|e| e.group_id)
        });
        group_map.insert(show_name.clone(), existing_gid.unwrap_or_else(Uuid::new_v4));
    }

    // Track media_ids per show for group TMDB fetch
    let mut show_media_ids: std::collections::HashMap<String, Vec<Uuid>> =
        std::collections::HashMap::new();

    for (video_file, episode_info, title, year) in &file_infos {
        // Update existing orphan entries (missing group_id) to join their group
        if let Some(mut existing_entry) = existing_entries.get(video_file).cloned() {
            if episode_info.is_show && existing_entry.group_id.is_none() {
                let gid = group_map.get(&episode_info.show_name).copied();
                existing_entry.group_id = gid;
                existing_entry.show_name = Some(episode_info.show_name.clone());
                existing_entry.season = episode_info.season;
                existing_entry.episode = episode_info.episode;
                existing_entry.media_type = crate::engine::library::MediaType::Show;
                let _ = state.store.put_media(&existing_entry);
                eprintln!(
                    "Library: updated orphan \"{}\" → group",
                    existing_entry.title
                );
            }
            continue; // Don't create a duplicate
        }

        let is_web = crate::engine::library::is_web_compatible(video_file);
        let file_size = std::fs::metadata(video_file).map(|m| m.len()).unwrap_or(0);

        let media_type = if episode_info.is_show {
            crate::engine::library::MediaType::Show
        } else {
            crate::engine::library::MediaType::Unknown
        };

        let transcode_state = if is_web {
            crate::engine::library::TranscodeState::Skipped
        } else if crate::transcode::runner::ffmpeg_available() {
            crate::engine::library::TranscodeState::Pending
        } else {
            crate::engine::library::TranscodeState::Unavailable
        };

        let group_id = if episode_info.is_show {
            group_map.get(&episode_info.show_name).copied()
        } else {
            None
        };

        let media_id = Uuid::new_v4();
        let entry = crate::engine::library::MediaEntry {
            id: media_id,
            title: if episode_info.is_show {
                episode_info.show_name.clone()
            } else {
                title.clone()
            },
            year: *year,
            media_type,
            original_path: video_file.parent().unwrap_or(&scan_path).to_path_buf(),
            media_file: video_file.clone(),
            transcoded_path: None,
            transcode_state: transcode_state.clone(),
            transcode_started_at: None,
            download_id: uuid::Uuid::nil(),
            added_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            file_size,
            poster_url: None,
            overview: None,
            rating: None,
            cast: Vec::new(),
            director: None,
            video_codec: None,
            audio_codec: None,
            versions: std::collections::HashMap::new(),
            show_name: if episode_info.is_show {
                Some(episode_info.show_name.clone())
            } else {
                None
            },
            season: episode_info.season,
            episode: episode_info.episode,
            episode_title: episode_info.episode_title.clone(),
            group_id,
            tmdb_id: None,
        };

        let _ = state.store.put_media(&entry);

        if episode_info.is_show {
            show_media_ids
                .entry(episode_info.show_name.clone())
                .or_default()
                .push(media_id);
        } else {
            // Fetch movie metadata individually
            let store_for_tmdb = state.store.clone();
            let entry_id = entry.id;
            let entry_title = entry.title.clone();
            let entry_year = entry.year;
            tokio::spawn(async move {
                let settings = store_for_tmdb.get_settings();
                if !settings.tmdb_api_key.is_empty()
                    && let Some(meta) = crate::tmdb::fetch_metadata(
                        &settings.tmdb_api_key,
                        &entry_title,
                        entry_year,
                    )
                    .await
                    && let Ok(Some(mut entry)) = store_for_tmdb.get_media(&entry_id)
                {
                    crate::tmdb::apply_metadata(&mut entry, &meta);
                    let _ = store_for_tmdb.put_media(&entry);
                    eprintln!("TMDB: fetched metadata for \"{}\"", entry.title);
                }
            });
        }

        // Auto-transcode if enabled and state is Pending
        if matches!(
            transcode_state,
            crate::engine::library::TranscodeState::Pending
        ) {
            let settings = state.store.get_settings();
            if settings.auto_transcode {
                let job = crate::transcode::job::create_job(&entry, &settings.default_preset);
                let tc = state.transcode.clone();
                tokio::spawn(async move {
                    tc.submit(job).await;
                });
            }
        }

        added += 1;
    }

    // Fetch TMDB metadata once per show group, apply to all entries
    for (show_name, media_ids) in show_media_ids {
        let store_for_tmdb = state.store.clone();
        tokio::spawn(async move {
            let settings = store_for_tmdb.get_settings();
            if settings.tmdb_api_key.is_empty() {
                return;
            }
            eprintln!("TMDB: searching TV for \"{show_name}\"");
            if let Some(meta) =
                crate::tmdb::fetch_tv_metadata(&settings.tmdb_api_key, &show_name).await
            {
                for mid in &media_ids {
                    if let Ok(Some(mut entry)) = store_for_tmdb.get_media(mid) {
                        let saved_title = entry.title.clone();
                        crate::tmdb::apply_metadata(&mut entry, &meta);
                        entry.title = saved_title;
                        let _ = store_for_tmdb.put_media(&entry);
                    }
                }
                eprintln!("TMDB: fetched TV metadata for \"{show_name}\"");
            }
        });
    }

    Json(serde_json::json!({ "added": added, "skipped": skipped })).into_response()
}
