//! Which files on disk belong to a library entry.
//!
//! Deliberately pure — it decides *what* would be removed and does no I/O — so
//! the decision is unit-testable and the irreversible part (the unlink) stays in
//! the web layer, next to the `api::paths` confinement that guards it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::types::MediaEntry;

/// Every file this entry alone owns: the playable file, each transcoded version,
/// and any sidecar subtitles found or extracted for it.
///
/// Deduped, because `media_file` is usually also the `transcoded_path` and one of
/// the `versions` values.
///
/// **`original_path` is excluded on purpose.** It holds the *directory* the media
/// was downloaded or scanned from (`library.rs` stores the video file's parent,
/// `manager.rs` the download dir), and every episode of a season shares it —
/// removing it would take the rest of the show with it.
pub fn owned_files(entry: &MediaEntry) -> Vec<PathBuf> {
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    paths.insert(entry.media_file.clone());
    if let Some(transcoded) = &entry.transcoded_path {
        paths.insert(transcoded.clone());
    }
    paths.extend(entry.versions.values().cloned());
    paths.extend(entry.subtitles.iter().map(|s| s.path.clone()));
    paths.into_iter().collect()
}

/// The source file a finished transcode makes redundant.
///
/// The built-in `hevc` preset is a *remux*: the MP4 it writes carries the same
/// streams as the MKV it read, at the same size. Keeping both doubles the disk
/// cost of every download, which is what fills the disk.
///
/// `None` when there is nothing safe to reclaim — the job wrote in place, or the
/// source is itself one of the versions we keep for playback.
pub fn redundant_source(entry: &MediaEntry, output: &Path) -> Option<PathBuf> {
    let source = &entry.media_file;
    if source == output {
        return None;
    }
    if entry.versions.values().any(|v| v == source) {
        return None;
    }
    Some(source.clone())
}

/// Delete `paths`, skipping anything that is not a confined, regular file.
/// Returns `(files removed, bytes reclaimed)`.
///
/// These paths come from the store rather than a request, so confinement is
/// defence in depth — but an unlink is irreversible, and this is the only thing
/// between a legacy or corrupted record and deleting an arbitrary file. Anything
/// outside the allowed roots is logged and left alone, never removed. Directories
/// are skipped too, so a path that names one is a no-op rather than a recursive
/// delete.
pub fn remove_files(media_id: Uuid, paths: &[PathBuf]) -> (usize, u64) {
    let (mut files, mut bytes) = (0usize, 0u64);
    for path in paths {
        let Ok(safe) = crate::paths::confine(path) else {
            tracing::warn!(
                media_id = %media_id, path = %path.display(),
                "refusing to delete a file outside the allowed media directories"
            );
            continue;
        };
        if !safe.is_file() {
            continue;
        }
        let size = std::fs::metadata(&safe).map_or(0, |m| m.len());
        match std::fs::remove_file(&safe) {
            Ok(()) => {
                files += 1;
                bytes += size;
            }
            Err(e) => tracing::warn!(
                media_id = %media_id, path = %safe.display(), error = %e,
                "failed to delete media file"
            ),
        }
    }
    (files, bytes)
}

/// Delete every file an entry owns. Returns `(files removed, bytes reclaimed)`.
pub fn remove_owned_files(entry: &MediaEntry) -> (usize, u64) {
    remove_files(entry.id, &owned_files(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{MediaType, SubtitleTrack, TranscodeState};
    use std::collections::HashMap;
    use uuid::Uuid;

    fn entry() -> MediaEntry {
        MediaEntry {
            id: Uuid::nil(),
            title: "Foo".into(),
            year: None,
            media_type: MediaType::Movie,
            original_path: PathBuf::from("/media/movies"),
            media_file: PathBuf::from("/media/movies/foo.mkv"),
            transcoded_path: None,
            transcode_state: TranscodeState::Skipped,
            transcode_started_at: None,
            download_id: Uuid::nil(),
            added_at: 0,
            file_size: 0,
            poster_url: None,
            overview: None,
            rating: None,
            cast: Vec::new(),
            director: None,
            video_codec: None,
            audio_codec: None,
            versions: HashMap::new(),
            show_name: None,
            season: None,
            episode: None,
            episode_title: None,
            group_id: None,
            tmdb_id: None,
            subtitles: Vec::new(),
            play_position: None,
            duration: None,
            last_played_at: None,
        }
    }

    #[test]
    fn bare_entry_owns_only_its_media_file() {
        assert_eq!(
            owned_files(&entry()),
            vec![PathBuf::from("/media/movies/foo.mkv")]
        );
    }

    #[test]
    fn never_returns_the_shared_source_directory() {
        // Every episode of a season shares `original_path`; deleting it would
        // take the rest of the show with it.
        let e = entry();
        assert!(!owned_files(&e).contains(&e.original_path));
    }

    #[test]
    fn collects_versions_and_subtitles_without_duplicates() {
        let mut e = entry();
        e.transcoded_path = Some(PathBuf::from("/media/transcoded/foo-hevc.mp4"));
        e.versions.insert(
            "hevc".into(),
            PathBuf::from("/media/transcoded/foo-hevc.mp4"), // same as transcoded_path
        );
        e.versions.insert(
            "h264".into(),
            PathBuf::from("/media/transcoded/foo-h264.mp4"),
        );
        e.subtitles.push(SubtitleTrack {
            label: "English".into(),
            language: Some("eng".into()),
            path: PathBuf::from("/media/movies/foo.en.vtt"),
            format: "vtt".into(),
        });

        let files = owned_files(&e);
        assert_eq!(
            files.len(),
            4,
            "duplicate hevc path must collapse: {files:?}"
        );
        assert!(files.contains(&PathBuf::from("/media/movies/foo.mkv")));
        assert!(files.contains(&PathBuf::from("/media/transcoded/foo-hevc.mp4")));
        assert!(files.contains(&PathBuf::from("/media/transcoded/foo-h264.mp4")));
        assert!(files.contains(&PathBuf::from("/media/movies/foo.en.vtt")));
    }

    #[test]
    fn remux_output_makes_the_source_redundant() {
        let e = entry();
        let out = PathBuf::from("/media/transcoded/foo.mp4");
        assert_eq!(redundant_source(&e, &out), Some(e.media_file.clone()));
    }

    #[test]
    fn in_place_transcode_reclaims_nothing() {
        let e = entry();
        // Output == source: deleting it would destroy the only copy.
        assert_eq!(redundant_source(&e, &e.media_file.clone()), None);
    }

    #[test]
    fn a_source_that_is_also_a_kept_version_is_never_reclaimed() {
        let mut e = entry();
        e.versions.insert("hevc".into(), e.media_file.clone());
        let out = PathBuf::from("/media/transcoded/foo-h264.mp4");
        assert_eq!(redundant_source(&e, &out), None);
    }
}
