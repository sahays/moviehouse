use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::store::Store;
use crate::engine::types::TranscodeState;

use super::probe::{ProbeResult, SubtitleStream, cpu_count};

/// Subtitle codecs that convert cleanly to MP4's `mov_text` (tx3g).
///
/// Bitmap formats (`hdmv_pgs_subtitle`, `dvd_subtitle`, `dvb_subtitle`) have no
/// tx3g representation — ffmpeg errors out on the attempt and fails the whole
/// job — so anything not listed here is skipped rather than mapped.
const MOV_TEXT_CONVERTIBLE: &[&str] = &[
    "subrip",
    "srt",
    "ass",
    "ssa",
    "webvtt",
    "mov_text",
    "text",
    "subviewer",
    "subviewer1",
    "microdvd",
];

/// Explicit stream mapping for an MP4 output, embedding text subtitles as
/// `mov_text`. Empty when there is nothing embeddable.
///
/// `AirPlay` hands the receiver only the media URL. An Apple TV plays the MP4 with
/// `AVFoundation` and never learns that the sidecar `.vtt` files exist — those are
/// `<track>` elements, rendered by the browser, and the browser is out of the
/// picture once playback moves to the TV. A tx3g track *inside* the container is
/// the only subtitle an `AirPlay` receiver can find, and it shows up in the TV's
/// own subtitle menu.
///
/// Streams are mapped by index rather than with `-map 0:s` so a single
/// unconvertible bitmap track cannot take the job down with it. Note that adding
/// any `-map` disables ffmpeg's automatic stream selection, so video and audio
/// have to be named explicitly too.
fn subtitle_map_args(probe: Option<&ProbeResult>) -> Vec<String> {
    let Some(probe) = probe else {
        return Vec::new();
    };

    let embeddable: Vec<&SubtitleStream> = probe
        .subtitle_streams
        .iter()
        .filter(|s| {
            let ok = MOV_TEXT_CONVERTIBLE.contains(&s.codec.as_str());
            if !ok {
                eprintln!(
                    "  Subtitles: skipping stream {} ({}) — no mov_text equivalent",
                    s.index, s.codec
                );
            }
            ok
        })
        .collect();

    if embeddable.is_empty() {
        return Vec::new();
    }

    let mut args: Vec<String> = vec!["-map".into(), "0:v:0".into(), "-map".into(), "0:a:0".into()];

    // Language tags are deliberately NOT set here. `-map` carries the source
    // tag through natively, and it is already the ISO 639-2 three-letter form
    // that MP4's language box requires. Overriding it with
    // `normalize_language_code` would be actively wrong: that helper returns
    // two-letter ISO 639-1 codes for HTML `srcLang`, which the MP4 muxer
    // rejects — it drops them and the track ends up with no language at all.
    for stream in &embeddable {
        args.extend(["-map".into(), format!("0:{}", stream.index)]);
    }

    args.extend(["-c:s".into(), "mov_text".into()]);
    eprintln!(
        "  Subtitles: embedding {} track(s) as mov_text for AirPlay",
        embeddable.len()
    );
    args
}

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
        // Pin the file protocol: the input path derives from the (attacker-
        // controlled) torrent name, so an unpinned value could be reparsed as
        // an ffmpeg protocol (concat:, subfile:) → local file inclusion.
        format!("file:{}", input.to_string_lossy()),
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

    args.extend(subtitle_map_args(probe));

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
    // Pin the file protocol (input path derives from the untrusted torrent name).
    let mut args: Vec<String> = vec!["-i".into(), format!("file:{}", input.to_string_lossy())];

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

    args.extend(subtitle_map_args(probe));

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

/// Extract embedded subtitle streams from a video file as standalone .vtt files.
/// Returns the paths and metadata of extracted subtitle files.
pub async fn extract_subtitles(
    input: &std::path::Path,
    output_dir: &std::path::Path,
    output_stem: &str,
    subtitle_streams: &[SubtitleStream],
) -> Vec<crate::engine::types::SubtitleTrack> {
    let mut tracks = Vec::new();

    for (i, stream) in subtitle_streams.iter().enumerate() {
        // The subtitle `language` tag comes straight from untrusted container
        // metadata; sanitize it before it becomes a filename component, or a
        // value like `../../etc/x` would traverse out of `output_dir`.
        let raw_lang = stream.language.as_deref().unwrap_or("");
        let sanitized = crate::engine::library::sanitize_filename(raw_lang);
        let lang_suffix = if sanitized.is_empty() {
            format!("track{i}")
        } else {
            sanitized
        };
        let vtt_path = output_dir.join(format!("{output_stem}.{lang_suffix}.vtt"));

        let result = tokio::process::Command::new("ffmpeg")
            .args([
                "-i",
                // Pin the file protocol so an attacker-named path can't be
                // reparsed as an ffmpeg protocol (concat:, subfile:, ...).
                &format!("file:{}", input.to_string_lossy()),
                "-map",
                &format!("0:{}", stream.index),
                "-c:s",
                "webvtt",
                "-y",
            ])
            .arg(&vtt_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await;

        if result.is_ok_and(|s| s.success()) && vtt_path.exists() {
            let language = stream
                .language
                .as_deref()
                .and_then(crate::engine::library::normalize_language_code);
            let label = language.as_deref().map_or_else(
                || {
                    stream
                        .language
                        .clone()
                        .unwrap_or_else(|| format!("Track {}", i + 1))
                },
                crate::engine::library::language_code_to_label,
            );
            eprintln!("  Extracted subtitle: {label} (stream {})", stream.index);
            tracks.push(crate::engine::types::SubtitleTrack {
                label,
                language,
                path: vtt_path,
                format: "vtt".into(),
            });
        }
    }

    tracks
}
