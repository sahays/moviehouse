use std::net::SocketAddr;
use std::time::Duration;

use byteorder::{BigEndian, ByteOrder};
use tokio::net::UdpSocket;
use tracing::debug;

use super::http::{AnnounceResponse, TrackerError};
use crate::torrent::types::{InfoHash, PeerId};

const PROTOCOL_ID: u64 = 0x0417_2710_1980;
const ACTION_CONNECT: u32 = 0;
const ACTION_ANNOUNCE: u32 = 1;

/// UDP tracker announce (BEP15).
#[allow(clippy::too_many_arguments)]
pub async fn udp_announce(
    tracker_url: &str,
    info_hash: &InfoHash,
    peer_id: &PeerId,
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
    event: u32, // 0=none, 1=completed, 2=started, 3=stopped
) -> Result<AnnounceResponse, TrackerError> {
    // Parse the URL to get host:port
    let url = url::Url::parse(tracker_url)
        .map_err(|e| TrackerError::Bencode(format!("invalid tracker URL: {e}")))?;

    let host = url
        .host_str()
        .ok_or_else(|| TrackerError::Bencode("missing host".into()))?;
    let tracker_port = url.port().unwrap_or(80);

    // Resolve DNS
    let addr_str = format!("{host}:{tracker_port}");
    let resolved: Vec<SocketAddr> = tokio::net::lookup_host(&addr_str)
        .await
        .map_err(|e| TrackerError::Bencode(format!("DNS resolution failed: {e}")))?
        .collect();

    let tracker_addr = resolved
        .first()
        .ok_or_else(|| TrackerError::Bencode("no addresses resolved".into()))?;

    // Bind a local UDP socket
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(|e| TrackerError::Bencode(format!("bind failed: {e}")))?;
    socket
        .connect(tracker_addr)
        .await
        .map_err(|e| TrackerError::Bencode(format!("connect failed: {e}")))?;

    // Step 1: Connect
    let connection_id = udp_connect(&socket).await?;
    debug!(connection_id, "UDP tracker connected");

    // Step 2: Announce
    let response = udp_announce_request(
        &socket,
        connection_id,
        info_hash,
        peer_id,
        downloaded,
        left,
        uploaded,
        event,
        port,
    )
    .await?;

    Ok(response)
}

async fn udp_connect(socket: &UdpSocket) -> Result<u64, TrackerError> {
    let transaction_id: u32 = rand::random();

    let mut buf = [0u8; 16];
    BigEndian::write_u64(&mut buf[0..8], PROTOCOL_ID);
    BigEndian::write_u32(&mut buf[8..12], ACTION_CONNECT);
    BigEndian::write_u32(&mut buf[12..16], transaction_id);

    // Retry with exponential backoff: 15 * 2^n seconds
    for attempt in 0..4 {
        let timeout = Duration::from_secs(15 * (1 << attempt));

        socket
            .send(&buf)
            .await
            .map_err(|e| TrackerError::Bencode(format!("send failed: {e}")))?;

        let mut recv_buf = [0u8; 16];
        match tokio::time::timeout(timeout, socket.recv(&mut recv_buf)).await {
            Ok(Ok(n)) if n >= 16 => {
                let action = BigEndian::read_u32(&recv_buf[0..4]);
                let txn = BigEndian::read_u32(&recv_buf[4..8]);
                let conn_id = BigEndian::read_u64(&recv_buf[8..16]);

                if action != ACTION_CONNECT || txn != transaction_id {
                    continue;
                }

                return Ok(conn_id);
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(TrackerError::Bencode(format!("recv failed: {e}"))),
            Err(_) => {
                debug!(attempt, "UDP connect timeout, retrying");
            }
        }
    }

    Err(TrackerError::Timeout)
}

#[allow(clippy::too_many_arguments)]
async fn udp_announce_request(
    socket: &UdpSocket,
    connection_id: u64,
    info_hash: &InfoHash,
    peer_id: &PeerId,
    downloaded: u64,
    left: u64,
    uploaded: u64,
    event: u32,
    port: u16,
) -> Result<AnnounceResponse, TrackerError> {
    let transaction_id: u32 = rand::random();

    let mut buf = [0u8; 98];
    BigEndian::write_u64(&mut buf[0..8], connection_id);
    BigEndian::write_u32(&mut buf[8..12], ACTION_ANNOUNCE);
    BigEndian::write_u32(&mut buf[12..16], transaction_id);
    buf[16..36].copy_from_slice(&info_hash.0);
    buf[36..56].copy_from_slice(&peer_id.0);
    BigEndian::write_u64(&mut buf[56..64], downloaded);
    BigEndian::write_u64(&mut buf[64..72], left);
    BigEndian::write_u64(&mut buf[72..80], uploaded);
    BigEndian::write_u32(&mut buf[80..84], event);
    BigEndian::write_u32(&mut buf[84..88], 0); // IP (default)
    BigEndian::write_u32(&mut buf[88..92], rand::random()); // key
    BigEndian::write_i32(&mut buf[92..96], -1); // num_want (-1 = default)
    BigEndian::write_u16(&mut buf[96..98], port);

    for attempt in 0..4 {
        let timeout = Duration::from_secs(15 * (1 << attempt));

        socket
            .send(&buf)
            .await
            .map_err(|e| TrackerError::Bencode(format!("send failed: {e}")))?;

        let mut recv_buf = vec![0u8; 2048];
        match tokio::time::timeout(timeout, socket.recv(&mut recv_buf)).await {
            Ok(Ok(n)) if n >= 20 => {
                let action = BigEndian::read_u32(&recv_buf[0..4]);
                let txn = BigEndian::read_u32(&recv_buf[4..8]);

                if action != ACTION_ANNOUNCE || txn != transaction_id {
                    continue;
                }

                let interval = BigEndian::read_u32(&recv_buf[8..12]);
                let leechers = BigEndian::read_u32(&recv_buf[12..16]);
                let seeders = BigEndian::read_u32(&recv_buf[16..20]);

                // Parse compact peers from remaining bytes
                let peers_data = &recv_buf[20..n];
                let peers = parse_compact_peers_udp(peers_data);

                debug!(
                    peers = peers.len(),
                    interval, seeders, leechers, "UDP announce response"
                );

                return Ok(AnnounceResponse {
                    interval,
                    min_interval: None, // UDP protocol doesn't have min_interval
                    peers,
                    seeders: Some(seeders),
                    leechers: Some(leechers),
                });
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(TrackerError::Bencode(format!("recv failed: {e}"))),
            Err(_) => {
                debug!(attempt, "UDP announce timeout, retrying");
            }
        }
    }

    Err(TrackerError::Timeout)
}

fn parse_compact_peers_udp(data: &[u8]) -> Vec<SocketAddr> {
    use std::net::{Ipv4Addr, SocketAddrV4};
    data.chunks_exact(6)
        .map(|chunk| {
            let ip = Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]);
            let port = u16::from_be_bytes([chunk[4], chunk[5]]);
            SocketAddr::V4(SocketAddrV4::new(ip, port))
        })
        .collect()
}
