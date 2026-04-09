use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use tracing::debug;

use crate::bencode;
use crate::torrent::types::{InfoHash, PeerId};

#[derive(Debug, thiserror::Error)]
pub enum TrackerError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("bencode error: {0}")]
    Bencode(String),
    #[error("tracker error: {0}")]
    TrackerFailure(String),
    #[error("invalid response")]
    InvalidResponse,
    #[error("timeout")]
    Timeout,
}

#[derive(Debug)]
pub struct AnnounceResponse {
    pub interval: u32,
    pub min_interval: Option<u32>,
    pub peers: Vec<SocketAddr>,
    pub seeders: Option<u32>,
    pub leechers: Option<u32>,
}

/// Announce to an HTTP tracker and return peer addresses.
pub async fn http_announce(
    tracker_url: &str,
    info_hash: &InfoHash,
    peer_id: &PeerId,
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: Option<&str>,
) -> Result<AnnounceResponse, TrackerError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    // Build the announce URL with query parameters.
    // info_hash and peer_id must be percent-encoded as raw bytes.
    let mut url = format!(
        "{separator}info_hash={}&peer_id={}&port={port}&uploaded={uploaded}&downloaded={downloaded}&left={left}&compact=1&numwant=200",
        info_hash.url_encode(),
        percent_encode_bytes(&peer_id.0),
        separator = if tracker_url.contains('?') { '&' } else { '?' },
    );

    if let Some(event) = event {
        url.push_str(&format!("&event={event}"));
    }

    let full_url = format!("{tracker_url}{url}");
    debug!(url = %full_url, "Announcing to tracker");

    let response = client.get(&full_url).send().await?;
    let status = response.status();
    let body = response.bytes().await?;

    if !status.is_success() {
        let preview = String::from_utf8_lossy(&body[..body.len().min(200)]);
        return Err(TrackerError::TrackerFailure(format!(
            "HTTP {status}: {preview}"
        )));
    }

    let val = bencode::decode(&body).map_err(|e| {
        let preview = String::from_utf8_lossy(&body[..body.len().min(200)]);
        TrackerError::Bencode(format!("{e} (response: {preview})"))
    })?;

    // Check for failure
    if let Some(failure) = val.get_str("failure reason") {
        if let Some(reason) = failure.as_str() {
            return Err(TrackerError::TrackerFailure(reason.to_string()));
        }
    }

    let interval = val
        .get_str("interval")
        .and_then(|v| v.as_int())
        .unwrap_or(1800) as u32;

    let min_interval = val
        .get_str("min interval")
        .and_then(|v| v.as_int())
        .map(|n| n as u32);

    let seeders = val
        .get_str("complete")
        .and_then(|v| v.as_int())
        .map(|n| n as u32);

    let leechers = val
        .get_str("incomplete")
        .and_then(|v| v.as_int())
        .map(|n| n as u32);

    // Parse compact peer list
    let peers = if let Some(peers_val) = val.get_str("peers") {
        if let Some(compact) = peers_val.as_bytes() {
            parse_compact_peers(compact)
        } else if let Some(peer_list) = peers_val.as_list() {
            // Non-compact format (dictionary model)
            parse_dict_peers(peer_list)
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    eprintln!(
        "Tracker response: {} peers, {} seeders, {} leechers (interval: {}s, min: {}s)",
        peers.len(),
        seeders.map_or("?".to_string(), |n| n.to_string()),
        leechers.map_or("?".to_string(), |n| n.to_string()),
        interval,
        min_interval.map_or("?".to_string(), |n| n.to_string()),
    );

    Ok(AnnounceResponse {
        interval,
        min_interval,
        peers,
        seeders,
        leechers,
    })
}

/// Parse compact peer list: each peer is 6 bytes (4 bytes IP + 2 bytes port, big-endian).
fn parse_compact_peers(data: &[u8]) -> Vec<SocketAddr> {
    data.chunks_exact(6)
        .map(|chunk| {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            SocketAddr::V4(SocketAddrV4::new(ip, port))
        })
        .collect()
}

/// Parse non-compact peer list (list of dicts with "ip" and "port" keys).
fn parse_dict_peers(peers: &[bencode::BValue]) -> Vec<SocketAddr> {
    peers
        .iter()
        .filter_map(|p| {
            let ip_str = p.get_str("ip")?.as_str()?;
            let port = p.get_str("port")?.as_int()? as u16;
            let ip: std::net::IpAddr = ip_str.parse().ok()?;
            Some(SocketAddr::new(ip, port))
        })
        .collect()
}

fn percent_encode_bytes(bytes: &[u8]) -> String {
    use percent_encoding::{percent_encode, NON_ALPHANUMERIC};
    percent_encode(bytes, NON_ALPHANUMERIC).to_string()
}
