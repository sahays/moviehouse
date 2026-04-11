use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::OnceLock;

use dashmap::DashMap;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::store::Store;
use crate::engine::types::TranscodeState;

use super::chunked::run_chunked_ffmpeg;
use super::ffmpeg::{run_ffmpeg_inner, run_remux};
use super::probe::{can_remux, has_videotoolbox, probe_file};

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
                        .and_then(|s| s.lines().next().map(std::string::ToString::to_string))
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
    #[allow(clippy::too_many_lines)]
    pub async fn run(mut self) {
        // Recover any transcodes that were in-progress when we last shut down
        recover_stuck_transcodes(&self.store);

        if ffmpeg_available() {
            eprintln!("Transcode runner ready (ffmpeg available)");
        } else {
            eprintln!(
                "FFmpeg not found. Transcoding disabled. Install: https://ffmpeg.org/download.html"
            );
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
            let duration_secs = probe.as_ref().map_or(0.0, |p| p.duration_secs);

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

            // Spawn ffmpeg -- remux or full transcode
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
