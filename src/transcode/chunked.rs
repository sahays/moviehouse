use std::process::Stdio;

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::store::Store;
use crate::engine::types::TranscodeState;

use super::probe::{ProbeResult, audio_is_compatible, cpu_count};

/// Chunked parallel encoding: split video into segments, encode in parallel, concatenate.
#[allow(clippy::too_many_lines)]
pub async fn run_chunked_ffmpeg(
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
        args.push("-y".into());

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
            () = cancel.cancelled() => {
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
