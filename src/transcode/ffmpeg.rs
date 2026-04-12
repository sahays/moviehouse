use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::store::Store;
use crate::engine::types::TranscodeState;

use super::probe::{ProbeResult, cpu_count};

/// Re-encode to H.264 MP4 with progress reporting.
#[allow(clippy::too_many_lines)]
pub async fn run_ffmpeg_encode(
    input: &std::path::Path,
    output: &std::path::Path,
    duration_secs: f64,
    media_id: &Uuid,
    store: &Store,
    probe: Option<&ProbeResult>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let cores = cpu_count();
    eprintln!("  Video: H.264 (libx264), CRF 23, {cores} threads");

    let mut args: Vec<String> = vec![
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "fast".into(),
        "-crf".into(),
        "23".into(),
        "-threads".into(),
        cores.to_string(),
    ];

    // Audio: copy if AAC, otherwise encode to AAC
    let audio_is_aac = probe.is_some_and(|p| p.audio_codec == "aac");
    if audio_is_aac {
        args.extend(["-c:a".into(), "copy".into()]);
    } else {
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }

    args.extend([
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-y".into(),
    ]);
    args.push(output.to_string_lossy().into_owned());

    let mut child = tokio::process::Command::new("ffmpeg")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    // Parse progress from stdout
    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        loop {
            tokio::select! {
                result = lines.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            if let Some(time_us_str) = line.strip_prefix("out_time_us=")
                                && let Ok(time_us) = time_us_str.parse::<i64>()
                            {
                                let progress = if duration_secs > 0.0 {
                                    (time_us as f64 / 1_000_000.0 / duration_secs * 100.0).clamp(0.0, 100.0) as f32
                                } else {
                                    -1.0_f32
                                };
                                let _ = store.update_transcode_state(
                                    media_id,
                                    TranscodeState::Transcoding {
                                        progress_percent: progress,
                                        encoder: "software".into(),
                                    },
                                );
                            }
                        }
                        _ => break,
                    }
                }
                () = cancel.cancelled() => {
                    let _ = child.kill().await;
                    anyhow::bail!("Cancelled by user");
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg exited with status {status}");
    }

    Ok(())
}

/// Fast remux: copy video to MP4, re-encode audio to AAC if needed.
/// Adds hvc1 tag for HEVC video (required by Safari).
pub async fn run_remux(
    input: &std::path::Path,
    output: &std::path::Path,
    duration_secs: f64,
    media_id: &Uuid,
    store: &Store,
    probe: Option<&ProbeResult>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let mut args: Vec<String> = vec!["-i".into(), input.to_string_lossy().into()];

    // Copy video stream
    args.extend(["-c:v".into(), "copy".into()]);

    // Add hvc1 tag for HEVC in MP4 (required by Safari for native HEVC playback)
    let is_hevc = probe.is_some_and(|p| matches!(p.video_codec.as_str(), "hevc" | "h265"));
    if is_hevc {
        eprintln!("  Remux: adding hvc1 tag for Safari HEVC playback");
        args.extend(["-tag:v".into(), "hvc1".into()]);
    }

    // Always re-encode audio to AAC for universal browser playback
    let audio_is_aac = probe.is_some_and(|p| p.audio_codec == "aac");
    if audio_is_aac {
        eprintln!("  Remux: audio is already AAC, copying");
        args.extend(["-c:a".into(), "copy".into()]);
    } else {
        eprintln!(
            "  Remux: re-encoding audio to AAC (was {})",
            probe.map_or("?", |p| p.audio_codec.as_str())
        );
        args.extend(["-c:a".into(), "aac".into(), "-b:a".into(), "192k".into()]);
    }

    args.extend(["-movflags".into(), "+faststart".into()]);
    args.push(output.to_string_lossy().into());
    args.extend(["-progress".into(), "pipe:1".into(), "-y".into()]);

    let mut child = tokio::process::Command::new("ffmpeg")
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        let mut lines = reader.lines();
        loop {
            tokio::select! {
                result = lines.next_line() => {
                    match result {
                        Ok(Some(line)) => {
                            if let Some(time_us_str) = line.strip_prefix("out_time_us=")
                                && let Ok(time_us) = time_us_str.parse::<i64>()
                            {
                                let progress = if duration_secs > 0.0 {
                                    (time_us as f64 / 1_000_000.0 / duration_secs * 100.0).clamp(0.0, 100.0) as f32
                                } else {
                                    -1.0_f32
                                };
                                let _ = store.update_transcode_state(
                                    media_id,
                                    TranscodeState::Transcoding {
                                        progress_percent: progress,
                                        encoder: "remux".into(),
                                    },
                                );
                            }
                        }
                        _ => break,
                    }
                }
                () = cancel.cancelled() => {
                    let _ = child.kill().await;
                    anyhow::bail!("Cancelled by user");
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg remux exited with status {status}");
    }

    Ok(())
}
