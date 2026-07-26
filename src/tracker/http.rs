use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};

use tracing::debug;

use crate::bencode;
use crate::torrent::types::{InfoHash, PeerId};

/// True only for globally-routable addresses. Rejects loopback, RFC1918 private,
/// link-local (incl. 169.254.169.254 cloud metadata), CGNAT (100.64/10),
/// multicast, unspecified, and broadcast — the SSRF-relevant ranges.
pub fn is_global_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || (o[0] == 100 && (o[1] & 0xc0) == 64))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_global_ip(&IpAddr::V4(v4));
            }
            let seg0 = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_multicast()
                || v6.is_unspecified()
                || (seg0 & 0xfe00) == 0xfc00
                || (seg0 & 0xffc0) == 0xfe80)
        }
    }
}

/// Resolve `host:port` and confirm EVERY resolved address is globally routable,
/// returning one to dial. Blocks SSRF to internal/metadata/loopback targets.
pub async fn resolve_public_addr(host: &str, port: u16) -> Result<SocketAddr, TrackerError> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| TrackerError::TrackerFailure(format!("DNS resolution failed: {e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(TrackerError::TrackerFailure("no addresses resolved".into()));
    }
    for addr in &addrs {
        if !is_global_ip(&addr.ip()) {
            return Err(TrackerError::TrackerFailure(
                "tracker resolves to a non-public address (blocked)".into(),
            ));
        }
    }
    Ok(addrs[0])
}

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
    #[error("response too large: {0} bytes")]
    ResponseTooLarge(u64),
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
#[allow(clippy::too_many_arguments)]
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
    const MAX_RESPONSE_SIZE: u64 = 1_048_576; // 1 MiB

    // SSRF guard: resolve the tracker host, refuse non-public addresses, pin the
    // client to the validated IP (so DNS can't rebind), and disable redirects.
    let parsed = url::Url::parse(tracker_url)
        .map_err(|e| TrackerError::TrackerFailure(format!("invalid tracker URL: {e}")))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| TrackerError::TrackerFailure("tracker URL missing host".into()))?;
    let host_port = parsed.port_or_known_default().unwrap_or(80);
    let addr = resolve_public_addr(host, host_port).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .resolve(host, addr)
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
        use std::fmt::Write;
        let _ = write!(url, "&event={event}");
    }

    let full_url = format!("{tracker_url}{url}");
    debug!(url = %full_url, "Announcing to tracker");

    let response = client.get(&full_url).send().await?;
    let status = response.status();

    if let Some(content_length) = response.content_length()
        && content_length > MAX_RESPONSE_SIZE
    {
        return Err(TrackerError::ResponseTooLarge(content_length));
    }

    let body = response.bytes().await?;

    if body.len() as u64 > MAX_RESPONSE_SIZE {
        return Err(TrackerError::ResponseTooLarge(body.len() as u64));
    }

    if !status.is_success() {
        // Do not echo the response body into errors/logs (it can carry
        // attacker/SSRF-controlled content).
        return Err(TrackerError::TrackerFailure(format!("HTTP {status}")));
    }

    let val = bencode::decode(&body).map_err(|e| TrackerError::Bencode(e.to_string()))?;

    // Check for failure
    if let Some(failure) = val.get_str("failure reason")
        && let Some(reason) = failure.as_str()
    {
        return Err(TrackerError::TrackerFailure(reason.to_string()));
    }

    let interval = val
        .get_str("interval")
        .and_then(super::super::bencode::value::BValue::as_int)
        .unwrap_or(1800) as u32;

    let min_interval = val
        .get_str("min interval")
        .and_then(super::super::bencode::value::BValue::as_int)
        .map(|n| n as u32);

    let seeders = val
        .get_str("complete")
        .and_then(super::super::bencode::value::BValue::as_int)
        .map(|n| n as u32);

    let leechers = val
        .get_str("incomplete")
        .and_then(super::super::bencode::value::BValue::as_int)
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
    use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
    percent_encode(bytes, NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod ssrf_tests {
    use super::is_global_ip;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn blocks_ssrf_ranges() {
        for s in [
            "127.0.0.1",       // loopback
            "10.0.0.5",        // private
            "192.168.1.1",     // private
            "172.16.0.1",      // private
            "169.254.169.254", // cloud metadata (link-local)
            "100.64.0.1",      // CGNAT
            "0.0.0.0",         // unspecified
            "::1",             // v6 loopback
            "fd00::1",         // v6 unique-local
        ] {
            assert!(!is_global_ip(&ip(s)), "{s} must be blocked");
        }
    }

    #[test]
    fn allows_public() {
        for s in [
            "1.1.1.1",
            "8.8.8.8",
            "93.184.216.34",
            "2606:4700:4700::1111",
        ] {
            assert!(is_global_ip(&ip(s)), "{s} must be allowed");
        }
    }
}
