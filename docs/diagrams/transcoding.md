# Transcoding

Transcode jobs are queued into an `mpsc` channel and run by `TranscodeRunner` (`src/transcode/runner.rs`) under a semaphore (default 2 concurrent). Each job probes the input with `ffprobe`, chooses a preset (HEVC remux vs H.264 re-encode), spawns `ffmpeg`, and streams progress into the sled `Store`. There is **no** push channel for transcode progress — the frontend polls `GET /api/v1/library`.

```mermaid
sequenceDiagram
    autonumber
    actor Client
    participant API as transcode.rs handlers
    participant Store as sled Store
    participant TH as TranscodeHandle
    participant Run as TranscodeRunner loop
    participant Job as per-job task
    participant FF as ffprobe / ffmpeg

    rect rgb(235,244,255)
    note over Client,TH: Enqueue (3 entry points)
    alt manual single
        Client->>API: POST /library/:id/transcode {preset}
        API->>Store: get_media (404 / 409 if already Transcoding)
        API->>API: create_job(entry, preset, transcode_dir)
    else per-season / group batch
        Client->>API: POST /group/:gid/transcode-all?season=
        API->>Store: list_media → filter Pending/Failed/Unavailable
    else auto after download completes
        Note over API: SessionManager hook (state == Completed)
    end
    API->>Store: update_transcode_state(id, Pending)
    API->>TH: submit(job) → job_tx.send
    API-->>Client: 202 Accepted (queued)
    end

    rect rgb(238,248,238)
    note over Run,FF: Runner picks up job
    Run->>Run: job_rx.recv → semaphore.acquire (limit = transcode_concurrency)
    Run->>Job: spawn task (owns permit)
    alt ffmpeg not available
        Job->>Store: update_transcode_state(Unavailable), drop permit
    end
    Job->>FF: probe_file (ffprobe -show_format -show_streams)
    FF-->>Job: duration, codecs, pix_fmt, subtitle streams
    Job->>Store: persist detected codecs
    Job->>Job: decide remux (hevc/h264 source & preset≠h264) vs re-encode
    Job->>Store: update_transcode_state(Transcoding{0.0}) — records started_at
    end

    rect rgb(245,240,255)
    note over Job,FF: ffmpeg + progress loop
    Job->>FF: run_remux OR run_ffmpeg_encode (libx264 CRF23, AAC)
    loop -progress pipe:1 lines
        FF-->>Job: out_time_us
        Job->>Store: update_transcode_state(Transcoding{percent, encoder})
    end
    alt cancelled
        Job->>FF: child.kill → bail "Cancelled by user"
    else ffmpeg non-zero exit
        Job->>Job: bail "ffmpeg exited ..."
    end
    end

    rect rgb(255,238,244)
    note over Job,Store: Completion
    alt Ok
        opt subtitle streams present
            Job->>FF: extract_subtitles (ffmpeg -c:s webvtt per track)
        end
        Job->>Store: update_transcode_state(Ready{output_path})
        Job->>Store: add_version(preset, output_path) + set final codecs
    else Err
        Job->>Job: remove partial output file
        Job->>Store: update_transcode_state(Failed{error})
    end
    Job->>Job: drop permit → next queued job starts
    end

    rect rgb(255,247,235)
    note over Client,Store: Progress is polled, not pushed
    Client->>API: GET /api/v1/library (every 3s, client-side)
    API->>Store: list_media → transcode_state serialized to JSON
    end
```

## Notes

- **Presets:** `hevc` remuxes MKV→MP4 (hvc1 tag, no re-encode, fast); `h264` re-encodes with libx264 for universal compatibility. Remux requires an h264/hevc source and a non-`h264` preset.
- **Cancellation:** each running job registers a `CancellationToken` in a `DashMap` keyed by media id. Per-job cancel, per-season/group stop, and shutdown (`cancel_all`) all kill the underlying `ffmpeg` child.
- **Crash recovery:** progress is persisted every tick, so a crash leaves entries stuck in `Transcoding`. On startup `recover_stuck_transcodes` resets those to `Pending` and deletes partial outputs. The store also ignores invalid state transitions, so a cancel racing completion can't corrupt state.
- **`-1.0`** is a progress sentinel meaning duration was unknown (probe failed).
- Source: `src/transcode/{runner,job,ffmpeg,presets,probe}.rs`, `src/web/api/transcode.rs`, `src/engine/manager.rs`, `src/engine/store.rs`.
