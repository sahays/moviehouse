#![allow(dead_code)]

mod bencode;
mod cli;
mod dht;
mod disk;
mod engine;
mod error;
mod peer;
mod piece;
mod tmdb;
mod torrent;
mod tracker;
mod transcode;
mod ui;
mod web;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use crate::cli::{Cli, Commands};
use crate::engine::session::TorrentSession;
use crate::torrent::magnet::MagnetLink;
use crate::torrent::metainfo::Metainfo;
use crate::torrent::types::PeerId;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Set up logging
    let filter = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .init();

    match cli.command {
        Commands::Info { torrent_file } => {
            cmd_info(&torrent_file)?;
        }
        Commands::Download {
            ref torrent_file,
            no_dht,
            seed: _,
        } => {
            let metainfo = Metainfo::from_file(torrent_file)?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(cmd_download(metainfo, &cli, no_dht))?;
        }
        Commands::Magnet {
            ref uri,
            no_dht,
            seed: _,
        } => {
            let magnet = MagnetLink::parse(uri)?;
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(cmd_magnet(magnet, &cli, no_dht))?;
        }
        Commands::Serve {
            ref bind,
            open,
            allow_sleep,
        } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(cmd_serve(bind, open, allow_sleep))?;
        }
    }

    Ok(())
}

async fn cmd_download(metainfo: Metainfo, cli: &Cli, no_dht: bool) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();

    // Handle Ctrl+C
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        tracing::info!("Ctrl+C received, shutting down...");
        cancel_clone.cancel();
    });

    let our_peer_id = PeerId::generate();

    let session = TorrentSession::new(
        metainfo,
        our_peer_id,
        cli.port,
        cli.max_peers,
        cli.output.clone(),
        no_dht,
        cli.lightspeed,
        cancel,
        Vec::new(),
    );

    session.run().await
}

async fn cmd_magnet(magnet: MagnetLink, cli: &Cli, no_dht: bool) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    let our_peer_id = PeerId::generate();

    // Phase 1: Download metadata from peers
    let (metainfo, warm_peers) = engine::magnet::download_metadata(
        &magnet,
        our_peer_id,
        cli.port,
        cli.max_peers,
        no_dht,
        cli.lightspeed,
        cancel.clone(),
    )
    .await?;

    eprintln!(
        "Starting download: {} ({:.2} MiB)",
        metainfo.info.name,
        metainfo.info.total_length as f64 / (1024.0 * 1024.0),
    );

    // Phase 2: Normal piece download (same path as .torrent files)
    let session = TorrentSession::new(
        metainfo,
        our_peer_id,
        cli.port,
        cli.max_peers,
        cli.output.clone(),
        no_dht,
        cli.lightspeed,
        cancel,
        warm_peers,
    );
    session.run().await
}

/// Minimum length for `MOVIEHOUSE_ACCESS_CODE` (the sole auth secret + HMAC key).
const MIN_ACCESS_CODE_LEN: usize = 16;

async fn cmd_serve(bind: &str, open: bool, allow_sleep: bool) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    // Prevent macOS from sleeping while the server is running
    #[cfg(target_os = "macos")]
    let _caffeinate = if allow_sleep {
        None
    } else {
        match std::process::Command::new("caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .spawn()
        {
            Ok(child) => {
                eprintln!("Sleep prevention active (caffeinate pid {})", child.id());
                Some(child)
            }
            Err(e) => {
                eprintln!("Warning: could not prevent sleep: {e}");
                None
            }
        }
    };

    let config = Config::load();
    // The access code is the single gate AND the HMAC key — require real entropy
    // so it can't be brute-forced online. 16 chars is the floor; `openssl rand
    // -hex 24` (the documented generator) produces 48.
    if config.access_code.trim().len() < MIN_ACCESS_CODE_LEN {
        anyhow::bail!(
            "MOVIEHOUSE_ACCESS_CODE must be at least {MIN_ACCESS_CODE_LEN} characters \
             (it is the sole auth secret). Generate one: openssl rand -hex 24."
        );
    }
    let store = Arc::new(engine::store::Store::open()?);
    // Seed TMDB key from .env into settings if not already set
    {
        let mut settings = store.get_settings();
        if settings.tmdb_api_key.is_empty() && !config.tmdb_api_key.is_empty() {
            settings.tmdb_api_key.clone_from(&config.tmdb_api_key);
            let _ = store.put_settings(&settings);
        }
    }
    eprintln!("Store opened");
    repair_legacy_dirs(&store);
    ensure_media_dirs(&store)?;
    let (transcode_handle, transcode_runner) = transcode::runner::create(store.clone());
    tokio::spawn(transcode_runner.run());
    eprintln!("Transcode runner spawned");
    let manager = Arc::new(engine::manager::SessionManager::new(
        cancel.clone(),
        store.clone(),
        Some(transcode_handle.clone()),
    ));
    let state = Arc::new(web::server::AppState {
        manager,
        store,
        transcode: transcode_handle,
        access_code: config.access_code.clone(),
        max_downloads: config.max_downloads,
    });
    let transcode_for_shutdown = state.transcode.clone();
    let router = web::server::create_router(&state);

    let listener = tokio::net::TcpListener::bind(bind).await?;
    eprintln!("Web UI running at http://{bind}");

    if open {
        let url = format!("http://{bind}");
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open").arg(&url).spawn();
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
            }
        });
    }

    axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await?;

    // Kill all running ffmpeg processes on shutdown
    eprintln!("Shutting down — cancelling active transcodes...");
    transcode_for_shutdown.cancel_all();
    // Give ffmpeg processes a moment to die
    tokio::time::sleep(Duration::from_millis(500)).await;

    Ok(())
}

/// Move stored media paths off superseded built-in defaults.
///
/// Two values were persisted by earlier builds and are wrong for a container
/// deployment:
///
/// * `download_dir` defaulted to `"."` — the working directory, which is `/app`,
///   owned by root while the process runs as `app`. Every piece write failed
///   with `Permission denied`.
/// * `transcode_dir` defaulted to `$HOME/.movies/transcoded`, which lands on the
///   small data volume that holds the sled db rather than the media mount.
///
/// A stored value that still equals one of those fallbacks was never a
/// deliberate choice, so let the current default (env override included) win.
/// Anything else the operator set is left alone.
fn repair_legacy_dirs(store: &engine::store::Store) {
    let mut settings = store.get_settings();
    let mut repaired_download = None;

    if settings.download_dir == std::path::Path::new(".") {
        let want = engine::types::default_download_dir();
        eprintln!(
            "Settings: download_dir \".\" -> {} (legacy default was not writable)",
            want.display()
        );
        settings.download_dir.clone_from(&want);
        repaired_download = Some(want);
    }

    let transcode_fallback = engine::types::home_movies_subdir("transcoded");
    let want_transcode = engine::types::default_transcode_dir();
    let repair_transcode =
        settings.transcode_dir == transcode_fallback && want_transcode != transcode_fallback;
    if repair_transcode {
        eprintln!(
            "Settings: transcode_dir {} -> {}",
            transcode_fallback.display(),
            want_transcode.display()
        );
        settings.transcode_dir = want_transcode;
    }

    if repaired_download.is_none() && !repair_transcode {
        return;
    }
    if let Err(e) = store.put_settings(&settings) {
        tracing::error!(error = %e, "Failed to persist repaired media directories");
        return;
    }

    // Persisted downloads carry their own copy of the output dir — it is how a
    // finished download's files are found for the library and deleted on
    // removal. Repair those too, or they keep pointing at the unwritable path.
    let Some(download_dir) = repaired_download else {
        return;
    };
    let Ok(records) = store.list_downloads() else {
        return;
    };
    for mut record in records {
        if record.output_dir == std::path::Path::new(".") {
            record.output_dir.clone_from(&download_dir);
            if let Err(e) = store.put_download(&record) {
                tracing::error!(error = %e, name = %record.name, "Failed to repair download output_dir");
            }
        }
    }
}

/// Create the download and transcode directories and prove they are writable.
///
/// Failing loudly at boot beats the alternative: the server comes up healthy,
/// accepts a torrent, and only then reports EACCES once per piece for the rest
/// of the download.
fn ensure_media_dirs(store: &engine::store::Store) -> anyhow::Result<()> {
    let settings = store.get_settings();
    for (label, dir) in [
        ("download_dir", &settings.download_dir),
        ("transcode_dir", &settings.transcode_dir),
    ] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("{label}: cannot create {}", dir.display()))?;

        let probe = dir.join(".moviehouse-write-test");
        std::fs::write(&probe, b"")
            .with_context(|| format!("{label}: {} is not writable", dir.display()))?;
        let _ = std::fs::remove_file(&probe);

        eprintln!("{label}: {}", dir.display());
    }
    Ok(())
}

fn cmd_info(path: &std::path::Path) -> anyhow::Result<()> {
    let metainfo = Metainfo::from_file(path)?;

    println!("Torrent: {}", metainfo.info.name);
    println!("Info Hash: {}", metainfo.info_hash);
    println!(
        "Piece Length: {} bytes ({} KiB)",
        metainfo.info.piece_length,
        metainfo.info.piece_length / 1024
    );
    println!("Pieces: {}", metainfo.info.pieces.len());
    println!(
        "Total Size: {} bytes ({:.2} MiB)",
        metainfo.info.total_length,
        metainfo.info.total_length as f64 / (1024.0 * 1024.0)
    );

    if let Some(ref announce) = metainfo.announce {
        println!("Announce: {announce}");
    }

    if let Some(ref announce_list) = metainfo.announce_list {
        println!("Trackers:");
        for (i, tier) in announce_list.iter().enumerate() {
            for url in tier {
                println!("  Tier {}: {url}", i + 1);
            }
        }
    }

    if let Some(ref comment) = metainfo.comment {
        println!("Comment: {comment}");
    }

    if let Some(ref created_by) = metainfo.created_by {
        println!("Created By: {created_by}");
    }

    match &metainfo.info.files {
        torrent::metainfo::FileLayout::Single { length } => {
            println!("File: {} ({length} bytes)", metainfo.info.name);
        }
        torrent::metainfo::FileLayout::Multi { files } => {
            println!("Files ({}):", files.len());
            for f in files {
                println!("  {} ({} bytes)", f.path.display(), f.length);
            }
        }
    }

    Ok(())
}

/// App configuration resolved from process env first, then the `.env` file.
struct Config {
    tmdb_api_key: String,
    tmdb_read_access_token: String,
    access_code: String,
    max_downloads: usize,
}

/// Precedence: a non-empty env value wins; otherwise the file value.
/// Pure (no env access) so it is testable without mutating the process
/// environment (which `unsafe_code = "forbid"` disallows).
fn pick(env_val: Option<String>, file_val: Option<String>) -> Option<String> {
    match env_val {
        Some(v) if !v.is_empty() => Some(v),
        _ => file_val,
    }
}

/// Resolve a key: process environment wins, then the parsed `.env` map.
/// (The Docker container receives values as process env via compose `env_file`;
/// local runs use the `.env` file. Both must work.)
fn env_or_file(key: &str, file: &std::collections::HashMap<String, String>) -> Option<String> {
    pick(std::env::var(key).ok(), file.get(key).cloned())
}

impl Config {
    fn load() -> Self {
        let mut values = std::collections::HashMap::new();
        if let Ok(contents) = std::fs::read_to_string(".env") {
            for line in contents.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    values.insert(key.trim().to_string(), value.trim().to_string());
                }
            }
        }
        Self {
            tmdb_api_key: env_or_file("TMDB_API_KEY", &values).unwrap_or_default(),
            tmdb_read_access_token: env_or_file("TMDB_READ_ACCESS_TOKEN", &values)
                .unwrap_or_default(),
            access_code: env_or_file("MOVIEHOUSE_ACCESS_CODE", &values)
                .unwrap_or_default()
                .trim()
                .to_string(),
            max_downloads: env_or_file("MOVIEHOUSE_MAX_DOWNLOADS", &values)
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(2),
        }
    }
}

#[cfg(test)]
mod config_tests {
    use super::pick;

    #[test]
    fn env_value_wins_when_present() {
        assert_eq!(
            pick(Some("env".into()), Some("file".into())).as_deref(),
            Some("env")
        );
        assert_eq!(pick(Some("env".into()), None).as_deref(), Some("env"));
    }

    #[test]
    fn empty_env_falls_back_to_file() {
        assert_eq!(
            pick(Some(String::new()), Some("file".into())).as_deref(),
            Some("file")
        );
    }

    #[test]
    fn file_used_when_env_absent() {
        assert_eq!(pick(None, Some("file".into())).as_deref(), Some("file"));
    }

    #[test]
    fn none_when_both_absent() {
        assert_eq!(pick(None, None), None);
        assert_eq!(pick(Some(String::new()), None), None);
    }
}
