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
        Commands::Serve { ref bind, open } => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(cmd_serve(bind, open))?;
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

async fn cmd_serve(bind: &str, open: bool) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    let config = Config::load();
    let store = Arc::new(engine::store::Store::open()?);
    // Seed TMDB key from .env into settings if not already set
    {
        let mut settings = store.get_settings();
        if settings.tmdb_api_key.is_empty() && !config.tmdb_api_key.is_empty() {
            settings.tmdb_api_key = config.tmdb_api_key.clone();
            let _ = store.put_settings(&settings);
        }
    }
    eprintln!("Store opened");
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
    });
    let router = web::server::create_router(state);

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

/// App configuration loaded from .env file.
struct Config {
    tmdb_api_key: String,
    tmdb_read_access_token: String,
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
            tmdb_api_key: values.remove("TMDB_API_KEY").unwrap_or_default(),
            tmdb_read_access_token: values.remove("TMDB_READ_ACCESS_TOKEN").unwrap_or_default(),
        }
    }
}
