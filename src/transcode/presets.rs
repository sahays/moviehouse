use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodePreset {
    pub name: String,
    pub label: String,
    pub description: String,
    pub scale_filter: Option<String>,
    pub crf: u8,
    pub audio_bitrate: String,
}

/// Two modes: compatibility (H.264, universal) and quality (remux, instant).
/// Each mode has resolution options.
pub fn builtin_presets() -> Vec<TranscodePreset> {
    vec![
        // Best Compatibility — H.264 re-encode, works on all browsers
        TranscodePreset {
            name: "compat-4k".into(),
            label: "4K".into(),
            description: "H.264, plays everywhere".into(),
            scale_filter: None,
            crf: 20,
            audio_bitrate: "256k".into(),
        },
        TranscodePreset {
            name: "compat-1080p".into(),
            label: "1080p".into(),
            description: "H.264, plays everywhere".into(),
            scale_filter: Some("-2:1080".into()),
            crf: 23,
            audio_bitrate: "192k".into(),
        },
        TranscodePreset {
            name: "compat-720p".into(),
            label: "720p".into(),
            description: "H.264, plays everywhere".into(),
            scale_filter: Some("-2:720".into()),
            crf: 25,
            audio_bitrate: "128k".into(),
        },
        TranscodePreset {
            name: "compat-480p".into(),
            label: "480p".into(),
            description: "H.264, plays everywhere".into(),
            scale_filter: Some("-2:480".into()),
            crf: 28,
            audio_bitrate: "96k".into(),
        },
        // Best Quality — remux (copy streams), instant, Apple/Safari only for HEVC
        TranscodePreset {
            name: "quality-original".into(),
            label: "Original".into(),
            description: "No re-encoding, instant".into(),
            scale_filter: None,
            crf: 0, // not used for remux
            audio_bitrate: "192k".into(),
        },
    ]
}

pub fn get_preset(name: &str) -> Option<TranscodePreset> {
    builtin_presets().into_iter().find(|p| p.name == name)
}

/// Check if a preset is a "quality" (remux) preset.
pub fn is_quality_preset(name: &str) -> bool {
    name.starts_with("quality-")
}

/// Check if a preset is a "compatibility" (re-encode) preset.
pub fn is_compat_preset(name: &str) -> bool {
    name.starts_with("compat-")
}
