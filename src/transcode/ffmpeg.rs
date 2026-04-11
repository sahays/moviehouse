use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::store::Store;
use crate::engine::types::TranscodeState;

use super::probe::{
    ProbeResult, audio_is_compatible, cpu_count, has_videotoolbox, video_is_remux_safe,
};

/// Build video encoding args. `use_hw` = try `VideoToolbox`. `pix_fmt` for bit depth check.
pub fn build_video_encode_args(
    preset: &crate::transcode::presets::TranscodePreset,
    use_hw: bool,
    pix_fmt: &str,
) -> Vec<String> {
    let mut args = Vec::new();

    // VideoToolbox H.264 only supports 8-bit input (yuv420p, nv12, etc.)
    // 10-bit (yuv420p10le, etc.) must use software encoder
    let is_10bit = pix_fmt.contains("10");
    if is_10bit && use_hw {
        eprintln!("  Video: 10-bit source detected — VideoToolbox unsupported, using libx264");
    }

    if use_hw && has_videotoolbox() && !is_10bit {
        eprintln!("  Video: H.264 via VideoToolbox (hardware)");
        args.extend(["-c:v".into(), "h264_videotoolbox".into()]);
        let quality = match preset.crf {
            0..=20 => "75",
            21..=23 => "65",
            24..=26 => "55",
            _ => "45",
        };
        args.extend(["-q:v".into(), quality.into()]);
        args.extend(["-bf".into(), "3".into()]);
        args.extend(["-allow_sw".into(), "0".into()]);
    } else {
        let cores = cpu_count();
        eprintln!(
            "  Video: H.264 via libx264 (software), CRF {}, {} threads",
            preset.crf, cores
        );
        args.extend(["-c:v".into(), "libx264".into()]);
        args.extend(["-preset".into(), "fast".into()]);
        args.extend(["-crf".into(), preset.crf.to_string()]);
        args.extend(["-threads".into(), cores.to_string()]);
    }

    args
}

#[allow(clippy::too_many_lines)]
pub async fn run_ffmpeg_inner(
    input: &std::path::Path,
    output: &std::path::Path,
    duration_secs: f64,
    media_id: &Uuid,
    store: &Store,
    preset_name: &str,
    container: &str,
    probe: Option<&ProbeResult>,
    cancel: &CancellationToken,
    use_hw: bool,
) -> anyhow::Result<()> {
    let preset = crate::transcode::presets::get_preset(preset_name).unwrap_or_else(|| {
        crate::transcode::presets::TranscodePreset {
            name: "compat-1080p".into(),
            label: "1080p (Full HD)".into(),
            description: "H.264, plays everywhere".into(),
            scale_filter: Some("-2:1080".into()),
            crf: 23,
            audio_bitrate: "192k".into(),
        }
    });

    let mut args: Vec<String> = vec!["-i".into(), input.to_string_lossy().into()];

    let needs_scale = preset.scale_filter.is_some();
    let video_remux_ok = probe.is_some_and(|p| video_is_remux_safe(&p.video_codec));
    let audio_ok = probe.is_some_and(|p| audio_is_compatible(&p.audio_codec));

    if video_remux_ok && !needs_scale {
        eprintln!(
            "  Video: copy (already {})",
            probe.map_or("?", |p| p.video_codec.as_str())
        );
        args.extend(["-c:v".into(), "copy".into()]);
    } else {
        let pix_fmt = probe.map_or("", |p| p.pix_fmt.as_str());
        let encode_args = build_video_encode_args(&preset, use_hw, pix_fmt);
        args.extend(encode_args);
        if let Some(ref scale) = preset.scale_filter {
            args.extend(["-vf".into(), format!("scale={scale}")]);
        }
    }

    if audio_ok {
        eprintln!(
            "  Audio: copy (already {})",
            probe.map_or("?", |p| p.audio_codec.as_str())
        );
        args.extend(["-c:a".into(), "copy".into()]);
    } else {
        eprintln!("  Audio: encode to AAC {}", preset.audio_bitrate);
        args.extend(["-c:a".into(), "aac".into()]);
        args.extend(["-b:a".into(), preset.audio_bitrate.clone()]);
    }

    // Container-specific
    if container == "hls" {
        let out_dir = output.parent().unwrap_or(std::path::Path::new("."));
        let stem = output.file_stem().unwrap_or_default().to_string_lossy();
        let segment_pattern = out_dir.join(format!("{stem}_%03d.ts"));
        args.extend([
            "-f".into(),
            "hls".into(),
            "-hls_time".into(),
            "6".into(),
            "-hls_list_size".into(),
            "0".into(),
            "-hls_segment_filename".into(),
            segment_pattern.to_string_lossy().into(),
        ]);
        let m3u8_path = output.with_extension("m3u8");
        args.push(m3u8_path.to_string_lossy().into());
    } else {
        args.extend(["-movflags".into(), "+faststart".into()]);
        args.push(output.to_string_lossy().into());
    }

    args.extend(["-progress".into(), "pipe:1".into(), "-y".into()]);

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
                                    // Known duration: compute real percentage
                                    (time_us as f64 / 1_000_000.0 / duration_secs * 100.0).clamp(0.0, 100.0) as f32
                                } else {
                                    // Unknown duration: use -1.0 as sentinel for indeterminate progress
                                    -1.0_f32
                                };
                                let _ = store.update_transcode_state(
                                    media_id,
                                    TranscodeState::Transcoding {
                                        progress_percent: progress,
                                        encoder: if use_hw { "hardware".into() } else { "software".into() },
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

/// Fast remux: copy video, re-encode audio to AAC for browser compatibility.
/// Adds hvc1 tag for HEVC video (required by Safari).
/// Much faster than full transcode -- only audio is re-encoded.
pub async fn run_remux(
    input: &std::path::Path,
    output: &std::path::Path,
    duration_secs: f64,
    media_id: &Uuid,
    store: &Store,
    container: &str,
    probe: Option<&ProbeResult>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let mut args: Vec<String> = vec!["-i".into(), input.to_string_lossy().into()];

    // Copy video stream
    args.extend(["-c:v".into(), "copy".into()]);

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

    if container == "hls" {
        let out_dir = output.parent().unwrap_or(std::path::Path::new("."));
        let stem = output.file_stem().unwrap_or_default().to_string_lossy();
        let segment_pattern = out_dir.join(format!("{stem}_%03d.ts"));
        args.extend([
            "-f".into(),
            "hls".into(),
            "-hls_time".into(),
            "6".into(),
            "-hls_list_size".into(),
            "0".into(),
            "-hls_segment_filename".into(),
            segment_pattern.to_string_lossy().into(),
        ]);
        let m3u8_path = output.with_extension("m3u8");
        args.push(m3u8_path.to_string_lossy().into());
    } else {
        args.extend(["-movflags".into(), "+faststart".into()]);
        args.push(output.to_string_lossy().into());
    }

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
