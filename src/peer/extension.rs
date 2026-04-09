use std::collections::HashMap;
use std::net::SocketAddr;

use bytes::Bytes;

use crate::bencode::{self, BValue};

/// BEP10 extended handshake message.
#[derive(Debug, Clone)]
pub struct ExtendedHandshake {
    /// Extension name -> message ID mapping
    pub m: HashMap<String, u8>,
    /// Client version string
    pub v: Option<String>,
    /// TCP listen port
    pub p: Option<u16>,
    /// Total metadata size in bytes
    pub metadata_size: Option<u64>,
    /// Max outstanding requests (reqq)
    pub reqq: Option<u64>,
}

impl ExtendedHandshake {
    /// Create our handshake offering ut_metadata support (and optionally ut_pex).
    pub fn ours(metadata_size: Option<u64>, lightspeed: bool) -> Self {
        let mut m = HashMap::new();
        m.insert("ut_metadata".to_string(), 1);
        if lightspeed {
            m.insert("ut_pex".to_string(), 2);
        }
        Self {
            m,
            v: Some("TorrentClient/0.1".to_string()),
            p: None,
            metadata_size,
            reqq: Some(250),
        }
    }

    /// Encode to bencode bytes.
    pub fn to_bencode(&self) -> Vec<u8> {
        use std::collections::BTreeMap;

        let mut dict = BTreeMap::new();

        // "m" dict
        let mut m_dict = BTreeMap::new();
        for (name, id) in &self.m {
            m_dict.insert(name.as_bytes().to_vec(), BValue::Int(*id as i64));
        }
        dict.insert(b"m".to_vec(), BValue::Dict(m_dict));

        if let Some(ref v) = self.v {
            dict.insert(b"v".to_vec(), BValue::Bytes(v.as_bytes().to_vec()));
        }

        if let Some(p) = self.p {
            dict.insert(b"p".to_vec(), BValue::Int(p as i64));
        }

        if let Some(size) = self.metadata_size {
            dict.insert(
                b"metadata_size".to_vec(),
                BValue::Int(size as i64),
            );
        }

        if let Some(reqq) = self.reqq {
            dict.insert(b"reqq".to_vec(), BValue::Int(reqq as i64));
        }

        bencode::encode(&BValue::Dict(dict))
    }

    /// Parse from bencode bytes.
    pub fn from_bencode(data: &[u8]) -> Result<Self, ExtensionError> {
        let val = bencode::decode(data).map_err(|e| ExtensionError::Decode(e.to_string()))?;
        let _dict = val.as_dict().ok_or(ExtensionError::InvalidFormat)?;

        let mut m = HashMap::new();
        if let Some(m_val) = val.get_str("m") {
            if let Some(m_dict) = m_val.as_dict() {
                for (key, val) in m_dict {
                    if let (Ok(name), Some(id)) = (std::str::from_utf8(key), val.as_int()) {
                        m.insert(name.to_string(), id as u8);
                    }
                }
            }
        }

        let v = val.get_str("v").and_then(|v| v.as_str()).map(String::from);
        let p = val.get_str("p").and_then(|v| v.as_int()).map(|n| n as u16);
        let metadata_size = val
            .get_str("metadata_size")
            .and_then(|v| v.as_int())
            .map(|n| n as u64);
        let reqq = val.get_str("reqq").and_then(|v| v.as_int()).map(|n| n as u64);

        Ok(Self {
            m,
            v,
            p,
            metadata_size,
            reqq,
        })
    }

    /// Get the remote peer's message ID for a given extension.
    pub fn extension_id(&self, name: &str) -> Option<u8> {
        self.m.get(name).copied()
    }
}

/// BEP9 metadata message types.
#[derive(Debug, Clone)]
pub enum MetadataMessage {
    Request { piece: u32 },
    Data { piece: u32, total_size: u64, data: Bytes },
    Reject { piece: u32 },
}

impl MetadataMessage {
    /// Encode to bytes for sending as extended message payload.
    pub fn to_bytes(&self) -> Vec<u8> {
        use std::collections::BTreeMap;

        let mut dict = BTreeMap::new();
        match self {
            MetadataMessage::Request { piece } => {
                dict.insert(b"msg_type".to_vec(), BValue::Int(0));
                dict.insert(b"piece".to_vec(), BValue::Int(*piece as i64));
            }
            MetadataMessage::Data {
                piece,
                total_size,
                data,
            } => {
                dict.insert(b"msg_type".to_vec(), BValue::Int(1));
                dict.insert(b"piece".to_vec(), BValue::Int(*piece as i64));
                dict.insert(
                    b"total_size".to_vec(),
                    BValue::Int(*total_size as i64),
                );
                // The data is appended AFTER the bencoded dict (not inside it)
                let mut encoded = bencode::encode(&BValue::Dict(dict));
                encoded.extend_from_slice(data);
                return encoded;
            }
            MetadataMessage::Reject { piece } => {
                dict.insert(b"msg_type".to_vec(), BValue::Int(2));
                dict.insert(b"piece".to_vec(), BValue::Int(*piece as i64));
            }
        }
        bencode::encode(&BValue::Dict(dict))
    }

    /// Parse from extended message payload.
    /// Data messages have trailing binary data after the bencoded dict.
    pub fn from_bytes(payload: &[u8]) -> Result<Self, ExtensionError> {
        // Use partial decode: find where the bencode dict ends
        let mut decoder = crate::bencode::Decoder::new(payload);
        let result = decoder.decode().map_err(|e| ExtensionError::Decode(e.to_string()))?;
        let consumed = result.end;

        let val = result.value;
        let msg_type = val
            .get_str("msg_type")
            .and_then(|v| v.as_int())
            .ok_or(ExtensionError::InvalidFormat)?;
        let piece = val
            .get_str("piece")
            .and_then(|v| v.as_int())
            .ok_or(ExtensionError::InvalidFormat)? as u32;

        match msg_type {
            0 => Ok(MetadataMessage::Request { piece }),
            1 => {
                let total_size = val
                    .get_str("total_size")
                    .and_then(|v| v.as_int())
                    .ok_or(ExtensionError::InvalidFormat)?
                    as u64;
                let data = Bytes::copy_from_slice(&payload[consumed..]);
                Ok(MetadataMessage::Data {
                    piece,
                    total_size,
                    data,
                })
            }
            2 => Ok(MetadataMessage::Reject { piece }),
            _ => Err(ExtensionError::UnknownMessageType(msg_type)),
        }
    }
}

/// BEP11 PEX (Peer Exchange) message.
pub struct PexMessage {
    pub added: Vec<SocketAddr>,
}

impl PexMessage {
    pub fn from_bencode(data: &[u8]) -> Result<Self, ExtensionError> {
        let val = bencode::decode(data).map_err(|e| ExtensionError::Decode(e.to_string()))?;
        let mut added = Vec::new();
        // "added" field is compact IPv4 peers (6 bytes each)
        if let Some(raw) = val.get_str("added").and_then(|v| v.as_bytes()) {
            added.extend(
                raw.chunks_exact(6).map(|c| {
                    let ip = std::net::Ipv4Addr::new(c[0], c[1], c[2], c[3]);
                    let port = u16::from_be_bytes([c[4], c[5]]);
                    SocketAddr::V4(std::net::SocketAddrV4::new(ip, port))
                })
            );
        }
        // "added6" for IPv6 (18 bytes each) -- skip for now
        Ok(Self { added })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtensionError {
    #[error("decode error: {0}")]
    Decode(String),
    #[error("invalid format")]
    InvalidFormat,
    #[error("unknown metadata message type: {0}")]
    UnknownMessageType(i64),
}
