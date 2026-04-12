use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscodePreset {
    pub name: String,
    pub label: String,
    pub description: String,
}

pub fn builtin_presets() -> Vec<TranscodePreset> {
    vec![
        TranscodePreset {
            name: "hevc".into(),
            label: "HEVC".into(),
            description: "Remux to MP4 (fast, keeps original quality)".into(),
        },
        TranscodePreset {
            name: "h264".into(),
            label: "H.264".into(),
            description: "Re-encode to H.264 MP4 (slower, universal)".into(),
        },
    ]
}

pub fn get_preset(name: &str) -> Option<TranscodePreset> {
    builtin_presets().into_iter().find(|p| p.name == name)
}
