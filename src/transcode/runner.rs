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

use super::ffmpeg::{extract_subtitles, run_ffmpeg_encode, run_remux};
use super::probe::probe_file;

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
            eprintln!("FFmpeg not found. Transcoding disabled.");
        }

        let concurrency = self.store.get_settings().transcode_concurrency.max(1);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
        eprintln!("Transcode concurrency: {concurrency} parallel jobs");

        while let Some(job) = self.job_rx.recv().await {
            let Ok(permit) = semaphore.clone().acquire_owned().await else {
                continue;
            };

            let store = self.store.clone();
            let cancel_tokens = self.cancel_tokens.clone();

            tokio::spawn(async move {
                if !ffmpeg_available() {
                    eprintln!(
                        "Transcode skipped (no ffmpeg): {}",
                        job.input_path.display()
                    );
                    let _ =
                        store.update_transcode_state(&job.media_id, TranscodeState::Unavailable);
                    drop(permit);
                    return;
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
                    && let Ok(Some(mut entry)) = store.get_media(&job.media_id)
                {
                    entry.video_codec = Some(p.video_codec.clone());
                    entry.audio_codec = Some(p.audio_codec.clone());
                    let _ = store.put_media(&entry);
                }

                // Simple decision: can we remux or must we re-encode?
                let source_is_hevc = probe
                    .as_ref()
                    .is_some_and(|p| matches!(p.video_codec.as_str(), "hevc" | "h265"));
                let source_is_h264 = probe.as_ref().is_some_and(|p| p.video_codec == "h264");
                let can_remux = source_is_hevc || source_is_h264;

                // If user explicitly chose h264 preset, force re-encode even if remuxable
                let force_h264 = job.preset_name == "h264";

                // Update state to Transcoding
                let _ = store.update_transcode_state(
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
                cancel_tokens.insert(job.media_id, cancel.clone());

                let remux = can_remux && !force_h264;

                let result = if remux {
                    eprintln!("Mode: Remux (copy video to MP4)");
                    run_remux(
                        &job.input_path,
                        &job.output_path,
                        duration_secs,
                        &job.media_id,
                        &store,
                        probe.as_ref(),
                        &cancel,
                    )
                    .await
                } else {
                    eprintln!("Mode: Re-encode to H.264");
                    run_ffmpeg_encode(
                        &job.input_path,
                        &job.output_path,
                        duration_secs,
                        &job.media_id,
                        &store,
                        probe.as_ref(),
                        &cancel,
                    )
                    .await
                };

                // Remove cancellation token after job completes
                cancel_tokens.remove(&job.media_id);

                match result {
                    Ok(()) => {
                        eprintln!("Transcode complete: {}", job.output_path.display());

                        // Extract embedded subtitles from the source file
                        if let Some(ref p) = probe
                            && !p.subtitle_streams.is_empty()
                        {
                            let output_dir = job
                                .output_path
                                .parent()
                                .unwrap_or(std::path::Path::new("."));
                            let output_stem = job
                                .output_path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("subs");
                            let sub_tracks = extract_subtitles(
                                &job.input_path,
                                output_dir,
                                output_stem,
                                &p.subtitle_streams,
                            )
                            .await;
                            if !sub_tracks.is_empty() {
                                if let Ok(Some(mut entry)) = store.get_media(&job.media_id) {
                                    entry.subtitles = sub_tracks;
                                    let _ = store.put_media(&entry);
                                }
                                eprintln!(
                                    "Extracted subtitles for: {}",
                                    job.output_path.display()
                                );
                            }
                        }

                        let _ = store.update_transcode_state(
                            &job.media_id,
                            TranscodeState::Ready {
                                output_path: job.output_path.clone(),
                            },
                        );
                        // Save this version and update codecs
                        let _ =
                            store.add_version(&job.media_id, &job.preset_name, &job.output_path);
                        if let Ok(Some(mut entry)) = store.get_media(&job.media_id) {
                            if !remux {
                                entry.video_codec = Some("h264".into());
                            }
                            entry.audio_codec = Some("aac".into());
                            let _ = store.put_media(&entry);
                        }
                    }
                    Err(e) => {
                        eprintln!("Transcode failed: {e}");
                        // Clean up partial output file
                        let _ = std::fs::remove_file(&job.output_path);
                        let _ = store.update_transcode_state(
                            &job.media_id,
                            TranscodeState::Failed {
                                error: e.to_string(),
                            },
                        );
                    }
                }

                drop(permit); // Release semaphore when done
            });
        }
    }
}
