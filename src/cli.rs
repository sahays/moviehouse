use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "moviehouse")]
#[command(version, about = "Self-hosted media library and download manager")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output directory for downloaded files
    #[arg(short, long, global = true, default_value = ".")]
    pub output: PathBuf,

    /// Listening port for incoming peer connections
    #[arg(short, long, global = true, default_value_t = 6881)]
    pub port: u16,

    /// Maximum number of peer connections
    #[arg(long, global = true, default_value_t = 80)]
    pub max_peers: usize,

    /// Maximum download rate in bytes/sec (0 = unlimited)
    #[arg(long, global = true, default_value_t = 0)]
    pub max_download_rate: u64,

    /// Maximum upload rate in bytes/sec (0 = unlimited)
    #[arg(long, global = true, default_value_t = 0)]
    pub max_upload_rate: u64,

    /// Enable verbose logging (repeatable: -v, -vv, -vvv)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Enable lightspeed mode (all performance optimizations)
    #[arg(long, global = true)]
    pub lightspeed: bool,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Download a torrent from a .torrent file
    Download {
        /// Path to the .torrent file
        torrent_file: PathBuf,

        /// Disable DHT (use tracker only)
        #[arg(long)]
        no_dht: bool,

        /// Seed after download completes
        #[arg(long)]
        seed: bool,
    },

    /// Download a torrent from a magnet link
    Magnet {
        /// Magnet URI
        uri: String,

        /// Disable DHT
        #[arg(long)]
        no_dht: bool,

        /// Seed after download completes
        #[arg(long)]
        seed: bool,
    },

    /// Show information about a .torrent file
    Info {
        /// Path to the .torrent file
        torrent_file: PathBuf,
    },

    /// Start the web UI server
    Serve {
        /// Address to bind the web server
        #[arg(long, default_value = "127.0.0.1:9000")]
        bind: String,

        /// Open browser on start
        #[arg(long)]
        open: bool,

        /// Allow system to sleep while serving (by default, idle sleep is prevented)
        #[arg(long)]
        allow_sleep: bool,
    },
}
