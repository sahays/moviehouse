use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::torrent::types::{InfoHash, PeerId};

pub const PROTOCOL_STRING: &[u8; 19] = b"BitTorrent protocol";
pub const HANDSHAKE_SIZE: usize = 68; // 1 + 19 + 8 + 20 + 20

/// BitTorrent handshake message (68 bytes).
#[derive(Debug, Clone)]
pub struct Handshake {
    pub reserved: [u8; 8],
    pub info_hash: InfoHash,
    pub peer_id: PeerId,
}

impl Handshake {
    pub fn new(info_hash: InfoHash, peer_id: PeerId) -> Self {
        let mut reserved = [0u8; 8];
        // BEP10: set extension protocol bit (byte 5, bit 0x10)
        reserved[5] |= 0x10;
        Self {
            reserved,
            info_hash,
            peer_id,
        }
    }

    /// Whether the peer supports the BEP10 extension protocol.
    pub fn supports_extension_protocol(&self) -> bool {
        self.reserved[5] & 0x10 != 0
    }

    /// Serialize to 68 bytes.
    pub fn to_bytes(&self) -> [u8; HANDSHAKE_SIZE] {
        let mut buf = [0u8; HANDSHAKE_SIZE];
        buf[0] = 19;
        buf[1..20].copy_from_slice(PROTOCOL_STRING);
        buf[20..28].copy_from_slice(&self.reserved);
        buf[28..48].copy_from_slice(&self.info_hash.0);
        buf[48..68].copy_from_slice(&self.peer_id.0);
        buf
    }

    /// Parse from 68 bytes.
    pub fn from_bytes(buf: &[u8; HANDSHAKE_SIZE]) -> Result<Self, HandshakeError> {
        if buf[0] != 19 {
            return Err(HandshakeError::InvalidProtocolLength(buf[0]));
        }
        if &buf[1..20] != PROTOCOL_STRING {
            return Err(HandshakeError::InvalidProtocolString);
        }

        let mut reserved = [0u8; 8];
        reserved.copy_from_slice(&buf[20..28]);

        let mut info_hash_bytes = [0u8; 20];
        info_hash_bytes.copy_from_slice(&buf[28..48]);

        let mut peer_id_bytes = [0u8; 20];
        peer_id_bytes.copy_from_slice(&buf[48..68]);

        Ok(Self {
            reserved,
            info_hash: InfoHash::from_bytes(info_hash_bytes),
            peer_id: PeerId(peer_id_bytes),
        })
    }

    /// Send handshake over a TCP stream.
    pub async fn write_to(&self, stream: &mut TcpStream) -> std::io::Result<()> {
        stream.write_all(&self.to_bytes()).await
    }

    /// Read handshake from a TCP stream.
    pub async fn read_from(stream: &mut TcpStream) -> Result<Self, HandshakeError> {
        let mut buf = [0u8; HANDSHAKE_SIZE];
        stream
            .read_exact(&mut buf)
            .await
            .map_err(HandshakeError::Io)?;
        Self::from_bytes(&buf)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HandshakeError {
    #[error("invalid protocol string length: {0}")]
    InvalidProtocolLength(u8),
    #[error("invalid protocol string")]
    InvalidProtocolString,
    #[error("info hash mismatch")]
    InfoHashMismatch,
    #[error("IO error: {0}")]
    Io(std::io::Error),
}
