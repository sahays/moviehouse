use crate::engine::library::{MediaEntry, sanitize_filename};

use super::runner::TranscodeJob;

pub fn create_job(entry: &MediaEntry, preset: &str) -> TranscodeJob {
    let output_dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".movies")
        .join("transcoded");
    let sanitized = sanitize_filename(&entry.title);
    let ep_suffix = match (entry.season, entry.episode) {
        (Some(s), Some(e)) => format!("-s{s:02}e{e:02}"),
        _ => String::new(),
    };
    TranscodeJob {
        media_id: entry.id,
        input_path: entry.media_file.clone(),
        output_path: output_dir.join(format!("{sanitized}{ep_suffix}.mp4")),
        preset_name: preset.into(),
    }
}
