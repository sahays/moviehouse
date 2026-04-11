use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MediaType {
    Movie,
    Show,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TranscodeState {
    Pending,
    Transcoding { progress_percent: f32 },
    Ready { output_path: PathBuf },
    Failed { error: String },
    Skipped,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaEntry {
    pub id: Uuid,
    pub title: String,
    pub year: Option<u16>,
    pub media_type: MediaType,
    pub original_path: PathBuf,
    pub media_file: PathBuf,
    pub transcoded_path: Option<PathBuf>,
    pub transcode_state: TranscodeState,
    #[serde(default)]
    pub transcode_started_at: Option<u64>,
    pub download_id: Uuid,
    pub added_at: u64,
    pub file_size: u64,
    #[serde(default)]
    pub poster_url: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub rating: Option<f32>,
    #[serde(default)]
    pub cast: Vec<String>,
    #[serde(default)]
    pub director: Option<String>,
}

/// Parse a torrent/file name into a clean title and optional year.
/// E.g., "The.Matrix.1999.1080p.BluRay.x264-Group" -> ("The Matrix", Some(1999))
pub fn parse_media_title(raw: &str) -> (String, Option<u16>) {
    let mut s = raw.replace(['.', '_'], " ");

    // Extract year (4 digits, 1900-2099)
    let mut year: Option<u16> = None;
    let year_pattern: Vec<&str> = s.split_whitespace().collect();
    for word in &year_pattern {
        if let Ok(y) = word
            .trim_matches(|c: char| !c.is_ascii_digit())
            .parse::<u16>()
            && (1900..=2099).contains(&y)
        {
            year = Some(y);
            break;
        }
    }

    // Strip everything after common quality/codec tags
    let tags = [
        "1080p", "720p", "2160p", "4K", "4k", "BluRay", "Bluray", "BLURAY", "BDRip", "BRRip",
        "WEB-DL", "WEBDL", "WEBRip", "WEBRIP", "WEB DL", "HDTV", "DVDRip", "HDRip", "x264", "x265",
        "H264", "H 264", "H265", "HEVC", "AVC", "AAC", "DTS", "AC3", "FLAC", "REMUX", "HDR",
        "10bit", "10 bit", "AMZN", "NF", "DSNP", "HMAX", "ATVP", "RARBG", "YTS", "YIFY", "EVO",
        "FGT", "SPARKS",
    ];

    let lower = s.to_lowercase();
    let mut cut_pos = s.len();
    for tag in &tags {
        if let Some(pos) = lower.find(&tag.to_lowercase())
            && pos < cut_pos
        {
            cut_pos = pos;
        }
    }

    // Also cut at year position if found
    if let Some(y) = year {
        let year_str = y.to_string();
        if let Some(pos) = s.find(&year_str)
            && pos < cut_pos
        {
            cut_pos = pos;
        }
    }

    s = s[..cut_pos].trim().to_string();

    // Clean up: remove trailing hyphens, brackets
    s = s.trim_end_matches(['-', '(', '[', ' ']).to_string();

    if s.is_empty() {
        s = raw.to_string();
    }

    (s, year)
}

/// Sanitize a title into a lowercase hyphenated filename.
/// "Avatar Fire And Ash" -> "avatar-fire-and-ash"
pub fn sanitize_filename(title: &str) -> String {
    title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == ' ' || c == '_' {
                '-'
            } else {
                '\0'
            }
        })
        .filter(|c| *c != '\0')
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Detect video files in a directory (fully recursive), return sorted by size (largest first).
pub fn detect_video_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let video_extensions = ["mkv", "mp4", "avi", "m4v", "mov", "wmv", "ts", "webm"];
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    collect_video_files_recursive(dir, &video_extensions, &mut files);
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.into_iter().map(|(p, _)| p).collect()
}

fn collect_video_files_recursive(
    dir: &std::path::Path,
    exts: &[&str],
    files: &mut Vec<(PathBuf, u64)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_video_files_recursive(&path, exts, files);
        } else if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && exts.contains(&ext.to_lowercase().as_str())
        {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            files.push((path, size));
        }
    }
}

/// Check if a file extension suggests a web-compatible format.
pub fn is_web_compatible(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase())
            .as_deref(),
        Some("mp4") | Some("m4v") | Some("webm")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_title_basic() {
        let (title, year) = parse_media_title("The.Matrix.1999.1080p.BluRay.x264-Group");
        assert_eq!(title, "The Matrix");
        assert_eq!(year, Some(1999));
    }

    #[test]
    fn test_parse_title_no_year() {
        let (title, year) = parse_media_title("Inception.1080p.BluRay.x264");
        assert_eq!(title, "Inception");
        assert_eq!(year, None);
    }

    #[test]
    fn test_parse_title_with_spaces() {
        let (title, year) = parse_media_title("The Dark Knight 2008 720p");
        assert_eq!(title, "The Dark Knight");
        assert_eq!(year, Some(2008));
    }

    #[test]
    fn test_web_compatible() {
        assert!(is_web_compatible(std::path::Path::new("movie.mp4")));
        assert!(is_web_compatible(std::path::Path::new("movie.m4v")));
        assert!(!is_web_compatible(std::path::Path::new("movie.mkv")));
        assert!(!is_web_compatible(std::path::Path::new("movie.avi")));
    }
}
