use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::engine::library::TranscodeState;
use crate::engine::store::Store;

static FFMPEG_AVAILABLE: OnceLock<bool> = OnceLock::new();
static FFMPEG_VERSION: OnceLock<Option<String>> = OnceLock::new();

/// Check if ffmpeg is available on PATH. Result is cached.
pub fn ffmpeg_available() -> bool {
    *FFMPEG_AVAILABLE.get_or_init(|| {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Get ffmpeg version string, or None if not available.
pub fn ffmpeg_version() -> Option<String> {
    FFMPEG_VERSION
        .get_or_init(|| {
            if !ffmpeg_available() {
                return None;
            }
            std::process::Command::new("ffmpeg")
                .arg("-version")
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8(o.stdout)
                        .ok()
                        .and_then(|s| s.lines().next().map(|l| l.to_string()))
                })
        })
        .clone()
}

pub struct TranscodeJob {
    pub media_id: Uuid,
    pub input_path: PathBuf,
    pub output_path: PathBuf,
    pub preset_name: String,
    pub container: String,
}

#[derive(Clone)]
pub struct TranscodeHandle {
    job_tx: mpsc::Sender<TranscodeJob>,
}

impl TranscodeHandle {
    pub async fn submit(&self, job: TranscodeJob) -> bool {
        self.job_tx.send(job).await.is_ok()
    }
}

pub struct TranscodeRunner {
    job_rx: mpsc::Receiver<TranscodeJob>,
    store: std::sync::Arc<Store>,
}

pub fn create(store: std::sync::Arc<Store>) -> (TranscodeHandle, TranscodeRunner) {
    let (job_tx, job_rx) = mpsc::channel(64);
    let handle = TranscodeHandle { job_tx };
    let runner = TranscodeRunner { job_rx, store };
    (handle, runner)
}

/// Scan all media entries for `TranscodeState::Transcoding` and reset them to `Pending`.
/// This recovers from stuck transcodes caused by a crash or restart mid-transcode.
pub fn recover_stuck_transcodes(store: &Store) {
    match store.list_media() {
        Ok(entries) => {
            for entry in entries {
                if matches!(entry.transcode_state, TranscodeState::Transcoding { .. }) {
                    tracing::info!(id = %entry.id, title = %entry.title, "Recovering stuck transcode -> Pending");
                    let _ = store.update_transcode_state(&entry.id, TranscodeState::Pending);
                }
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "Failed to list media for stuck-transcode recovery");
        }
    }
}

impl TranscodeRunner {
    pub async fn run(mut self) {
        // Recover any transcodes that were in-progress when we last shut down
        recover_stuck_transcodes(&self.store);

        if !ffmpeg_available() {
            eprintln!(
                "FFmpeg not found. Transcoding disabled. Install: https://ffmpeg.org/download.html"
            );
        } else {
            eprintln!("Transcode runner ready (ffmpeg available)");
        }

        while let Some(job) = self.job_rx.recv().await {
            if !ffmpeg_available() {
                eprintln!(
                    "Transcode skipped (no ffmpeg): {}",
                    job.input_path.display()
                );
                let _ = self
                    .store
                    .update_transcode_state(&job.media_id, TranscodeState::Unavailable);
                continue;
            }

            eprintln!(
                "Transcode starting: {} -> {}",
                job.input_path.display(),
                job.output_path.display()
            );

            // Probe the input file
            let probe = probe_file(&job.input_path).await;
            let duration_secs = probe.as_ref().map(|p| p.duration_secs).unwrap_or(0.0);

            if let Some(ref p) = probe {
                eprintln!(
                    "Probe: video={} audio={} duration={:.0}s",
                    p.video_codec, p.audio_codec, p.duration_secs
                );
            } else {
                eprintln!("Probe: failed to read streams");
            }

            let remux = probe.as_ref().is_some_and(can_remux);

            if remux {
                eprintln!("Remux mode: streams are already H.264/AAC, copying without re-encode");
            }

            // Update state to Transcoding
            let _ = self.store.update_transcode_state(
                &job.media_id,
                TranscodeState::Transcoding {
                    progress_percent: 0.0,
                },
            );

            // Create output directory
            if let Some(parent) = job.output_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            // Spawn ffmpeg — remux or full transcode
            let result = if remux {
                run_remux(
                    &job.input_path,
                    &job.output_path,
                    duration_secs,
                    &job.media_id,
                    &self.store,
                    &job.container,
                )
                .await
            } else {
                run_ffmpeg(
                    &job.input_path,
                    &job.output_path,
                    duration_secs,
                    &job.media_id,
                    &self.store,
                    &job.preset_name,
                    &job.container,
                    probe.as_ref(),
                )
                .await
            };

            match result {
                Ok(()) => {
                    eprintln!("Transcode complete: {}", job.output_path.display());
                    let final_output = if job.container == "hls" {
                        job.output_path.with_extension("m3u8")
                    } else {
                        job.output_path
                    };
                    let _ = self.store.update_transcode_state(
                        &job.media_id,
                        TranscodeState::Ready {
                            output_path: final_output,
                        },
                    );
                }
                Err(e) => {
                    eprintln!("Transcode failed: {e}");
                    let _ = self.store.update_transcode_state(
                        &job.media_id,
                        TranscodeState::Failed {
                            error: e.to_string(),
                        },
                    );
                }
            }
        }
    }
}

struct ProbeResult {
    duration_secs: f64,
    video_codec: String,
    audio_codec: String,
}

async fn probe_file(path: &std::path::Path) -> Option<ProbeResult> {
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

    Some(ProbeResult {
        duration_secs,
        video_codec,
        audio_codec,
    })
}

fn video_is_compatible(codec: &str) -> bool {
    matches!(codec, "h264" | "hevc" | "h265")
}

fn audio_is_compatible(codec: &str) -> bool {
    matches!(codec, "aac" | "ac3" | "eac3" | "mp3" | "")
}

/// Check if ALL streams can be remuxed without any re-encoding.
fn can_remux(probe: &ProbeResult) -> bool {
    video_is_compatible(&probe.video_codec) && audio_is_compatible(&probe.audio_codec)
}

#[allow(clippy::too_many_arguments)]
async fn run_ffmpeg(
    input: &std::path::Path,
    output: &std::path::Path,
    duration_secs: f64,
    media_id: &Uuid,
    store: &Store,
    preset_name: &str,
    container: &str,
    probe: Option<&ProbeResult>,
) -> anyhow::Result<()> {
    let preset = crate::transcode::presets::get_preset(preset_name)
        .unwrap_or_else(|| crate::transcode::presets::get_preset("1080p").unwrap());

    let mut args: Vec<String> = vec!["-i".into(), input.to_string_lossy().into()];

    let needs_scale = preset.scale_filter.is_some();
    let video_ok = probe.is_some_and(|p| video_is_compatible(&p.video_codec));
    let audio_ok = probe.is_some_and(|p| audio_is_compatible(&p.audio_codec));

    // Video: copy if compatible AND no scaling needed, otherwise re-encode to H.265
    if video_ok && !needs_scale {
        eprintln!(
            "  Video: copy (already {})",
            probe.map(|p| p.video_codec.as_str()).unwrap_or("?")
        );
        args.extend(["-c:v".into(), "copy".into()]);
    } else {
        eprintln!("  Video: encode to H.265 (libx265), CRF {}", preset.crf);
        args.extend(["-c:v".into(), "libx265".into()]);
        args.extend(["-preset".into(), "medium".into()]);
        args.extend(["-crf".into(), preset.crf.to_string()]);
        // Tag for HLS/MP4 compatibility
        args.extend(["-tag:v".into(), "hvc1".into()]);
        if let Some(ref scale) = preset.scale_filter {
            args.extend(["-vf".into(), format!("scale={scale}")]);
        }
    }

    // Audio: copy if compatible, otherwise re-encode to AAC
    if audio_ok {
        eprintln!(
            "  Audio: copy (already {})",
            probe.map(|p| p.audio_codec.as_str()).unwrap_or("?")
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
        while let Ok(Some(line)) = lines.next_line().await {
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
                    },
                );
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg exited with status {}", status);
    }

    Ok(())
}

/// Fast remux: copy streams without re-encoding.
/// Supports both MP4 and HLS output. Takes seconds instead of minutes.
async fn run_remux(
    input: &std::path::Path,
    output: &std::path::Path,
    duration_secs: f64,
    media_id: &Uuid,
    store: &Store,
    container: &str,
) -> anyhow::Result<()> {
    let mut args: Vec<String> = vec!["-i".into(), input.to_string_lossy().into()];
    args.extend(["-c".into(), "copy".into()]);

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
        while let Ok(Some(line)) = lines.next_line().await {
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
                    },
                );
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg remux exited with status {}", status);
    }

    Ok(())
}
