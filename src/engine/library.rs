use std::path::PathBuf;

pub use super::types::{MediaEntry, MediaType, TranscodeState};

/// Parse a torrent/file name into a clean title and optional year.
/// E.g., "The.Matrix.1999.1080p.BluRay.x264-Group" -> ("The Matrix", Some(1999))
pub fn parse_media_title(raw: &str) -> (String, Option<u16>) {
    let mut s = raw.replace(['.', '_'], " ");

    // Extract year (4 digits, 1900-2099)
    // Prefer year in parentheses like (1968) — avoids movies with years in title like "2001"
    let mut year: Option<u16> = None;
    // First try: year in parentheses
    for window in s.as_bytes().windows(6) {
        if window[0] == b'('
            && window[5] == b')'
            && let Ok(y) = std::str::from_utf8(&window[1..5])
                .unwrap_or("")
                .parse::<u16>()
            && (1900..=2099).contains(&y)
        {
            year = Some(y);
            break;
        }
    }
    // Fallback: standalone 4-digit year (not at the start of title)
    if year.is_none() {
        let words: Vec<&str> = s.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i == 0 {
                continue;
            } // Skip first word to avoid "2001" in "2001 A Space Odyssey"
            if let Ok(y) = word
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse::<u16>()
                && (1900..=2099).contains(&y)
            {
                year = Some(y);
                break;
            }
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

    // Also cut at year position if found, but not if year is at position 0
    // (avoids cutting "2001 A Space Odyssey" at the start)
    if let Some(y) = year {
        let year_str = y.to_string();
        // Try "(YEAR)" first, then bare "YEAR"
        let paren_year = format!("({year_str})");
        let pos = s.find(&paren_year).or_else(|| s.find(&year_str));
        if let Some(pos) = pos
            && pos > 0
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

pub struct EpisodeInfo {
    pub show_name: String,
    pub season: Option<u16>,
    pub episode: Option<u16>,
    pub episode_title: Option<String>,
    pub is_show: bool,
}

/// Parse episode info from a filename.
/// Detects patterns: S01E01, s01e01, 1x01, and extracts show name.
pub fn parse_episode_info(filename: &str) -> EpisodeInfo {
    let s = filename.replace(['.', '_'], " ");

    // Try S01E01 pattern (case insensitive)
    let lower = s.to_lowercase();

    // Pattern 1: S01E01
    if let Some(idx) = find_sxxexx(&lower) {
        let raw_show = s[..idx].trim().trim_end_matches('-').trim().to_string();
        let (show_name, _) = parse_media_title(&raw_show);
        let (season, episode) = parse_sxxexx(&lower[idx..]);
        // Extract episode title: text after SxxExx, before quality tags
        let after_ep = &s[idx + 6..]; // skip "S01E01"
        let ep_title = extract_episode_title(after_ep);
        return EpisodeInfo {
            show_name: if show_name.is_empty() {
                raw_show
            } else {
                show_name
            },
            season,
            episode,
            episode_title: ep_title,
            is_show: true,
        };
    }

    // Pattern 2: 1x01
    if let Some((idx, season, episode)) = find_nxnn(&lower) {
        let show_part = s[..idx].trim().to_string();
        let after_ep = &s[idx + 4..]; // skip "1x01"
        let ep_title = extract_episode_title(after_ep);
        return EpisodeInfo {
            show_name: if show_part.is_empty() {
                s.clone()
            } else {
                show_part
            },
            season: Some(season),
            episode: Some(episode),
            episode_title: ep_title,
            is_show: true,
        };
    }

    // Not a show
    let (title, _) = parse_media_title(filename);
    EpisodeInfo {
        show_name: title,
        season: None,
        episode: None,
        episode_title: None,
        is_show: false,
    }
}

/// Extract episode title from text after the `SxxExx` pattern.
/// Input: " - Elegy (1080p `BluRay` x265 `ImE`)" → Some("Elegy")
fn extract_episode_title(after_ep: &str) -> Option<String> {
    // Strip leading separators: " - ", " ", "-"
    let s = after_ep.trim().trim_start_matches('-').trim();
    if s.is_empty() {
        return None;
    }
    // Use parse_media_title to strip quality tags
    let (title, _) = parse_media_title(s);
    if title.is_empty() { None } else { Some(title) }
}

fn find_sxxexx(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    (0..bytes.len().saturating_sub(5)).find(|&i| {
        bytes[i] == b's'
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3] == b'e'
            && bytes[i + 4].is_ascii_digit()
    })
}

fn parse_sxxexx(s: &str) -> (Option<u16>, Option<u16>) {
    // s starts with "sXXeXX..."
    let season = s.get(1..3).and_then(|v| v.parse().ok());
    let episode = s.get(4..6).and_then(|v| v.parse().ok());
    (season, episode)
}

fn find_nxnn(s: &str) -> Option<(usize, u16, u16)> {
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(3) {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1] == b'x'
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            let season = (bytes[i] - b'0') as u16;
            let episode = s.get(i + 2..i + 4).and_then(|v| v.parse().ok());
            if let Some(ep) = episode {
                return Some((i, season, ep));
            }
        }
    }
    None
}

/// Check if a file extension suggests a web-compatible format.
pub fn is_web_compatible(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(str::to_lowercase)
            .as_deref(),
        Some("mp4" | "m4v" | "webm")
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
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
    fn test_parse_title_year_in_title() {
        // "2001" is part of the title, year is in parentheses
        let (title, year) =
            parse_media_title("2001 A Space Odyssey (1968) [BluRay] [1080p] [YTS.AM]");
        assert_eq!(title, "2001 A Space Odyssey");
        assert_eq!(year, Some(1968));
    }

    #[test]
    fn test_parse_title_parenthesized_year() {
        let (title, year) =
            parse_media_title("Avatar Fire And Ash (2025) [2160p] [4K] [WEB] [5.1] [YTS.BZ]");
        assert_eq!(title, "Avatar Fire And Ash");
        assert_eq!(year, Some(2025));
    }

    #[test]
    fn test_web_compatible() {
        assert!(is_web_compatible(std::path::Path::new("movie.mp4")));
        assert!(is_web_compatible(std::path::Path::new("movie.m4v")));
        assert!(!is_web_compatible(std::path::Path::new("movie.mkv")));
        assert!(!is_web_compatible(std::path::Path::new("movie.avi")));
    }

    #[test]
    fn test_parse_episode_s01e01() {
        let info = parse_episode_info("Breaking.Bad.S01E01.720p.BluRay.x264");
        assert!(info.is_show);
        assert_eq!(info.show_name, "Breaking Bad");
        assert_eq!(info.season, Some(1));
        assert_eq!(info.episode, Some(1));
    }

    #[test]
    fn test_parse_episode_with_title() {
        let info = parse_episode_info(
            "The Twilight Zone (1959) - S01E01 - Where Is Everybody (1080p BluRay x265 ImE)",
        );
        assert!(info.is_show);
        assert_eq!(info.show_name, "The Twilight Zone");
        assert_eq!(info.season, Some(1));
        assert_eq!(info.episode, Some(1));
        assert_eq!(info.episode_title.as_deref(), Some("Where Is Everybody"));
    }

    #[test]
    fn test_parse_episode_1x01() {
        let info = parse_episode_info("Friends 1x01 The Pilot");
        assert!(info.is_show);
        assert_eq!(info.season, Some(1));
        assert_eq!(info.episode, Some(1));
    }

    #[test]
    fn test_parse_episode_movie() {
        let info = parse_episode_info("The.Matrix.1999.1080p.BluRay.x264");
        assert!(!info.is_show);
    }
}
