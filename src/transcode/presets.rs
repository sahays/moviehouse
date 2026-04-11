use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodePreset {
    pub name: String,
    pub label: String,
    pub scale_filter: Option<String>,
    pub crf: u8,
    pub video_bitrate: Option<String>,
    pub audio_bitrate: String,
}

/// Built-in presets with CRF values tuned for H.265 (libx265).
/// H.265 CRF ~28 ≈ H.264 CRF ~23 at similar quality.
pub fn builtin_presets() -> Vec<TranscodePreset> {
    vec![
        TranscodePreset {
            name: "4k".into(),
            label: "4K (2160p) — Original resolution".into(),
            scale_filter: None,
            crf: 24,
            video_bitrate: Some("15M".into()),
            audio_bitrate: "256k".into(),
        },
        TranscodePreset {
            name: "1080p".into(),
            label: "1080p (Full HD)".into(),
            scale_filter: Some("-2:1080".into()),
            crf: 28,
            video_bitrate: Some("5M".into()),
            audio_bitrate: "192k".into(),
        },
        TranscodePreset {
            name: "720p".into(),
            label: "720p (HD)".into(),
            scale_filter: Some("-2:720".into()),
            crf: 30,
            video_bitrate: Some("2.5M".into()),
            audio_bitrate: "128k".into(),
        },
        TranscodePreset {
            name: "480p".into(),
            label: "480p (SD)".into(),
            scale_filter: Some("-2:480".into()),
            crf: 32,
            video_bitrate: Some("1M".into()),
            audio_bitrate: "96k".into(),
        },
    ]
}

pub fn get_preset(name: &str) -> Option<TranscodePreset> {
    builtin_presets().into_iter().find(|p| p.name == name)
}
