use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
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
    pub enable_chunking: bool,
}

#[derive(Clone)]
pub struct TranscodeHandle {
    job_tx: mpsc::Sender<TranscodeJob>,
    cancel_tokens: Arc<DashMap<Uuid, CancellationToken>>,
}

impl TranscodeHandle {
    pub async fn submit(&self, job: TranscodeJob) -> bool {
        self.job_tx.send(job).await.is_ok()
    }

    pub fn cancel(&self, media_id: &Uuid) {
        if let Some((_, token)) = self.cancel_tokens.remove(media_id) {
            token.cancel();
        }
    }

    /// Cancel all running transcodes (called on shutdown).
    pub fn cancel_all(&self) {
        for entry in self.cancel_tokens.iter() {
            entry.value().cancel();
        }
        self.cancel_tokens.clear();
    }
}

pub struct TranscodeRunner {
    job_rx: mpsc::Receiver<TranscodeJob>,
    store: Arc<Store>,
    cancel_tokens: Arc<DashMap<Uuid, CancellationToken>>,
}

pub fn create(store: Arc<Store>) -> (TranscodeHandle, TranscodeRunner) {
    let (job_tx, job_rx) = mpsc::channel(64);
    let cancel_tokens = Arc::new(DashMap::new());
    let handle = TranscodeHandle {
        job_tx,
        cancel_tokens: cancel_tokens.clone(),
    };
    let runner = TranscodeRunner {
        job_rx,
        store,
        cancel_tokens,
    };
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
                    // Delete partial output for this entry
                    if let Some(ref path) = entry.transcoded_path {
                        let _ = std::fs::remove_file(path);
                        eprintln!("Removed partial file: {}", path.display());
                    }
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
                    "Probe: video={} audio={} pix_fmt={} duration={:.0}s",
                    p.video_codec, p.audio_codec, p.pix_fmt, p.duration_secs
                );
            } else {
                eprintln!("Probe: failed to read streams");
            }

            // Store detected codecs on the media entry
            if let Some(ref p) = probe
                && let Ok(Some(mut entry)) = self.store.get_media(&job.media_id)
            {
                entry.video_codec = Some(p.video_codec.clone());
                entry.audio_codec = Some(p.audio_codec.clone());
                let _ = self.store.put_media(&entry);
            }

            // Decide mode: remux only if source is H.264 (browser-safe)
            // HEVC sources always need re-encode for universal browser playback
            let is_quality = crate::transcode::presets::is_quality_preset(&job.preset_name);
            let source_is_h264 = probe.as_ref().is_some_and(|p| p.video_codec == "h264");
            let remux = (is_quality && source_is_h264) || probe.as_ref().is_some_and(can_remux);

            if remux {
                eprintln!("Mode: Best Quality (remux, no re-encoding)");
            } else {
                eprintln!("Mode: Best Compatibility (H.264 re-encode)");
            }

            // Update state to Transcoding (encoder set once encoding starts)
            let _ = self.store.update_transcode_state(
                &job.media_id,
                TranscodeState::Transcoding {
                    progress_percent: 0.0,
                    encoder: String::new(),
                },
            );

            // Create output directory
            if let Some(parent) = job.output_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }

            // Create cancellation token for this job
            let cancel = CancellationToken::new();
            self.cancel_tokens.insert(job.media_id, cancel.clone());

            // Spawn ffmpeg — remux or full transcode
            let result = if remux {
                run_remux(
                    &job.input_path,
                    &job.output_path,
                    duration_secs,
                    &job.media_id,
                    &self.store,
                    &job.container,
                    probe.as_ref(),
                    &cancel,
                )
                .await
            } else {
                // Check if 10-bit (VideoToolbox can't handle it)
                let is_10bit = probe.as_ref().is_some_and(|p| p.pix_fmt.contains("10"));
                let try_hw = has_videotoolbox() && !is_10bit;

                // Try hardware encoding first
                let hw_result = if try_hw {
                    run_ffmpeg_inner(
                        &job.input_path,
                        &job.output_path,
                        duration_secs,
                        &job.media_id,
                        &self.store,
                        &job.preset_name,
                        &job.container,
                        probe.as_ref(),
                        &cancel,
                        true,
                    )
                    .await
                } else {
                    Err(anyhow::anyhow!(
                        "hardware encoding not available for this input"
                    ))
                };

                match hw_result {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        if cancel.is_cancelled() {
                            Err(e)
                        } else {
                            if try_hw {
                                eprintln!(
                                    "  Hardware encode failed: {e} — falling back to software"
                                );
                            }
                            let _ = std::fs::remove_file(&job.output_path);

                            // Use chunked parallel encoding if enabled and duration is known
                            let settings = self.store.get_settings();
                            if settings.enable_chunking
                                && duration_secs > 30.0
                                && job.container == "mp4"
                            {
                                eprintln!("  Using chunked parallel encoding");
                                run_chunked_ffmpeg(
                                    &job.input_path,
                                    &job.output_path,
                                    duration_secs,
                                    &job.media_id,
                                    &self.store,
                                    &job.preset_name,
                                    probe.as_ref(),
                                    &cancel,
                                )
                                .await
                            } else {
                                run_ffmpeg_inner(
                                    &job.input_path,
                                    &job.output_path,
                                    duration_secs,
                                    &job.media_id,
                                    &self.store,
                                    &job.preset_name,
                                    &job.container,
                                    probe.as_ref(),
                                    &cancel,
                                    false,
                                )
                                .await
                            }
                        }
                    }
                }
            };

            // Remove cancellation token after job completes
            self.cancel_tokens.remove(&job.media_id);

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
                            output_path: final_output.clone(),
                        },
                    );
                    // Save this version and update codecs
                    let _ = self
                        .store
                        .add_version(&job.media_id, &job.preset_name, &final_output);
                    if let Ok(Some(mut entry)) = self.store.get_media(&job.media_id) {
                        if !remux {
                            entry.video_codec = Some("h264".into());
                        }
                        entry.audio_codec = Some("aac".into());
                        let _ = self.store.put_media(&entry);
                    }
                }
                Err(e) => {
                    eprintln!("Transcode failed: {e}");
                    // Clean up partial output file
                    let _ = std::fs::remove_file(&job.output_path);
                    // Also try HLS variant
                    let _ = std::fs::remove_file(job.output_path.with_extension("m3u8"));
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
    pix_fmt: String,
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

/// Video codec is compatible for remux (copy without re-encode).
/// Only H.264 is universally supported by all browsers.
/// HEVC/H.265 only works in Safari — Chrome/Firefox can't decode it.
fn video_is_remux_safe(codec: &str) -> bool {
    matches!(codec, "h264")
}

/// Video codec is recognized (for detection purposes).
fn video_is_compatible(codec: &str) -> bool {
    matches!(codec, "h264" | "hevc" | "h265")
}

fn audio_is_compatible(codec: &str) -> bool {
    matches!(codec, "aac" | "ac3" | "eac3" | "mp3" | "")
}

/// Check if ALL streams can be remuxed without any re-encoding.
/// Only H.264 video can be remuxed — HEVC needs re-encode to H.264 for browser support.
fn can_remux(probe: &ProbeResult) -> bool {
    video_is_remux_safe(&probe.video_codec) && audio_is_compatible(&probe.audio_codec)
}

/// Detect if VideoToolbox hardware encoder is available.
fn has_videotoolbox() -> bool {
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
fn cpu_count() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// Build video encoding args. `use_hw` = try VideoToolbox. `pix_fmt` for bit depth check.
fn build_video_encode_args(
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

#[allow(clippy::too_many_arguments)]
async fn run_ffmpeg_inner(
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
    let preset = crate::transcode::presets::get_preset(preset_name)
        .unwrap_or_else(|| crate::transcode::presets::get_preset("compat-1080p").unwrap());

    let mut args: Vec<String> = vec!["-i".into(), input.to_string_lossy().into()];

    let needs_scale = preset.scale_filter.is_some();
    let video_remux_ok = probe.is_some_and(|p| video_is_remux_safe(&p.video_codec));
    let audio_ok = probe.is_some_and(|p| audio_is_compatible(&p.audio_codec));

    if video_remux_ok && !needs_scale {
        eprintln!(
            "  Video: copy (already {})",
            probe.map(|p| p.video_codec.as_str()).unwrap_or("?")
        );
        args.extend(["-c:v".into(), "copy".into()]);
    } else {
        let pix_fmt = probe.map(|p| p.pix_fmt.as_str()).unwrap_or("");
        let encode_args = build_video_encode_args(&preset, use_hw, pix_fmt);
        args.extend(encode_args);
        if let Some(ref scale) = preset.scale_filter {
            args.extend(["-vf".into(), format!("scale={scale}")]);
        }
    }

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
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    anyhow::bail!("Cancelled by user");
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg exited with status {}", status);
    }

    Ok(())
}

/// Chunked parallel encoding: split video into segments, encode in parallel, concatenate.
#[allow(clippy::too_many_arguments)]
async fn run_chunked_ffmpeg(
    input: &std::path::Path,
    output: &std::path::Path,
    duration_secs: f64,
    media_id: &Uuid,
    store: &Store,
    preset_name: &str,
    probe: Option<&ProbeResult>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let cores = cpu_count();
    // Use cores/2 chunks, each gets ~2 threads. Min 2, max 8 chunks.
    let num_chunks = (cores / 2).clamp(2, 8);
    let chunk_duration = duration_secs / num_chunks as f64;

    eprintln!("  Chunked encoding: {num_chunks} chunks, {chunk_duration:.0}s each, {cores} cores");

    let preset = crate::transcode::presets::get_preset(preset_name)
        .unwrap_or_else(|| crate::transcode::presets::get_preset("compat-1080p").unwrap());

    let out_dir = output.parent().unwrap_or(std::path::Path::new("."));
    let stem = output.file_stem().unwrap_or_default().to_string_lossy();
    let threads_per_chunk = std::cmp::max(1, cores / num_chunks);

    // Phase 1: Encode chunks in parallel
    let mut handles = Vec::new();
    for i in 0..num_chunks {
        let start = i as f64 * chunk_duration;
        let chunk_path = out_dir.join(format!("{stem}_chunk_{i:03}.mp4"));
        let input = input.to_path_buf();
        let cancel = cancel.clone();

        let mut args: Vec<String> = vec![
            "-ss".into(),
            format!("{start:.3}"),
            "-i".into(),
            input.to_string_lossy().into(),
            "-t".into(),
            format!("{chunk_duration:.3}"),
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            "fast".into(),
            "-crf".into(),
            preset.crf.to_string(),
            "-threads".into(),
            threads_per_chunk.to_string(),
        ];

        if let Some(ref scale) = preset.scale_filter {
            args.extend(["-vf".into(), format!("scale={scale}")]);
        }

        let audio_ok = probe.is_some_and(|p| audio_is_compatible(&p.audio_codec));
        if audio_ok {
            args.extend(["-c:a".into(), "copy".into()]);
        } else {
            args.extend([
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                preset.audio_bitrate.clone(),
            ]);
        }

        args.extend(["-movflags".into(), "+faststart".into()]);
        args.push(chunk_path.to_string_lossy().into());
        args.extend(["-y".into()]);

        let chunk_path_clone = chunk_path.clone();
        handles.push((
            chunk_path_clone,
            tokio::spawn(async move {
                if cancel.is_cancelled() {
                    return Err(anyhow::anyhow!("Cancelled"));
                }
                let status = tokio::process::Command::new("ffmpeg")
                    .args(&args)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?
                    .wait()
                    .await?;
                if !status.success() {
                    anyhow::bail!("ffmpeg chunk {i} exited with {status}");
                }
                Ok(())
            }),
        ));
    }

    // Wait for all chunks, reporting progress
    let mut completed = 0usize;
    let mut chunk_paths = Vec::new();
    for (path, handle) in handles {
        let result = tokio::select! {
            r = handle => r?,
            _ = cancel.cancelled() => {
                anyhow::bail!("Cancelled by user");
            }
        };
        result?;
        completed += 1;
        chunk_paths.push(path);
        let progress = (completed as f32 / num_chunks as f32 * 95.0).clamp(0.0, 95.0); // 0-95%
        let _ = store.update_transcode_state(
            media_id,
            TranscodeState::Transcoding {
                progress_percent: progress,
                encoder: "chunked".into(),
            },
        );
        eprintln!("  Chunk {completed}/{num_chunks} complete ({progress:.0}%)");
    }

    // Phase 2: Concatenate chunks
    eprintln!("  Concatenating {num_chunks} chunks...");
    let concat_list = out_dir.join(format!("{stem}_concat.txt"));
    let list_content: String = chunk_paths
        .iter()
        .map(|p| format!("file '{}'", p.to_string_lossy()))
        .collect::<Vec<_>>()
        .join("\n");
    tokio::fs::write(&concat_list, &list_content).await?;

    let status = tokio::process::Command::new("ffmpeg")
        .args([
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &concat_list.to_string_lossy(),
            "-c",
            "copy",
            "-movflags",
            "+faststart",
            "-y",
        ])
        .arg(output)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?
        .wait()
        .await?;

    if !status.success() {
        anyhow::bail!("ffmpeg concat exited with {status}");
    }

    // Cleanup chunk files and concat list
    for path in &chunk_paths {
        let _ = tokio::fs::remove_file(path).await;
    }
    let _ = tokio::fs::remove_file(&concat_list).await;

    let _ = store.update_transcode_state(
        media_id,
        TranscodeState::Transcoding {
            progress_percent: 100.0,
            encoder: "chunked".into(),
        },
    );

    Ok(())
}

/// Fast remux: copy video, re-encode audio to AAC for browser compatibility.
/// Adds hvc1 tag for HEVC video (required by Safari).
/// Much faster than full transcode — only audio is re-encoded.
#[allow(clippy::too_many_arguments)]
async fn run_remux(
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
            probe.map(|p| p.audio_codec.as_str()).unwrap_or("?")
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
                _ = cancel.cancelled() => {
                    let _ = child.kill().await;
                    anyhow::bail!("Cancelled by user");
                }
            }
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("ffmpeg remux exited with status {}", status);
    }

    Ok(())
}
