use std::process::Stdio;

pub struct SubtitleStream {
    /// Stream index in the container (for ffmpeg -map 0:{index})
    pub index: usize,
    /// Language tag if available (e.g. "eng", "spa")
    pub language: Option<String>,
    /// Codec name (e.g. "subrip", "ass", "webvtt")
    pub codec: String,
}

pub struct ProbeResult {
    pub duration_secs: f64,
    pub video_codec: String,
    pub audio_codec: String,
    pub pix_fmt: String,
    pub subtitle_streams: Vec<SubtitleStream>,
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

    let subtitle_streams: Vec<SubtitleStream> = streams
        .iter()
        .filter(|s| s["codec_type"].as_str() == Some("subtitle"))
        .filter_map(|s| {
            let index = s["index"].as_u64()? as usize;
            let codec = s["codec_name"].as_str().unwrap_or("").to_string();
            let language = s["tags"]["language"]
                .as_str()
                .map(std::string::ToString::to_string);
            Some(SubtitleStream {
                index,
                language,
                codec,
            })
        })
        .collect();

    Some(ProbeResult {
        duration_secs,
        video_codec,
        audio_codec,
        pix_fmt,
        subtitle_streams,
    })
}

/// Get number of CPU cores for parallel encoding.
pub fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4)
}
