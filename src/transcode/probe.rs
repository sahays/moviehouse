use std::process::Stdio;

pub struct ProbeResult {
    pub duration_secs: f64,
    pub video_codec: String,
    pub audio_codec: String,
    pub pix_fmt: String,
}

pub async fn probe_file(path: &std::path::Path) -> Option<ProbeResult> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;

    let duration_secs = json["format"]["duration"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(0.0);

    let streams = json["streams"].as_array()?;

    let video_codec = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("video"))
        .and_then(|s| s["codec_name"].as_str())
        .unwrap_or("")
        .to_string();

    let audio_codec = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("audio"))
        .and_then(|s| s["codec_name"].as_str())
        .unwrap_or("")
        .to_string();

    let pix_fmt = streams
        .iter()
        .find(|s| s["codec_type"].as_str() == Some("video"))
        .and_then(|s| s["pix_fmt"].as_str())
        .unwrap_or("")
        .to_string();

    Some(ProbeResult {
        duration_secs,
        video_codec,
        audio_codec,
        pix_fmt,
    })
}

/// Detect if `VideoToolbox` hardware encoder is available.
pub fn has_videotoolbox() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("h264_videotoolbox"))
            .unwrap_or(false)
    })
}

/// Get number of CPU cores for parallel encoding.
pub fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4)
}

/// Video codec is compatible for remux (copy without re-encode).
/// Only H.264 is universally supported by all browsers.
/// HEVC/H.265 only works in Safari -- Chrome/Firefox can't decode it.
pub fn video_is_remux_safe(codec: &str) -> bool {
    matches!(codec, "h264")
}

/// Video codec is recognized (for detection purposes).
pub fn video_is_compatible(codec: &str) -> bool {
    matches!(codec, "h264" | "hevc" | "h265")
}

pub fn audio_is_compatible(codec: &str) -> bool {
    matches!(codec, "aac" | "ac3" | "eac3" | "mp3" | "")
}

/// Check if ALL streams can be remuxed without any re-encoding.
/// Only H.264 video can be remuxed -- HEVC needs re-encode to H.264 for browser support.
pub fn can_remux(probe: &ProbeResult) -> bool {
    video_is_remux_safe(&probe.video_codec) && audio_is_compatible(&probe.audio_codec)
}
