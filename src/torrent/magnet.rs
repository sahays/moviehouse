use crate::torrent::types::InfoHash;

#[derive(Debug, thiserror::Error)]
pub enum MagnetError {
    #[error("not a magnet URI")]
    NotMagnet,
    #[error("missing info hash (xt parameter)")]
    MissingInfoHash,
    #[error("invalid info hash: {0}")]
    InvalidInfoHash(String),
    #[error("unsupported hash function: {0}")]
    UnsupportedHashFunction(String),
}

/// Parsed magnet link.
#[derive(Debug, Clone)]
pub struct MagnetLink {
    pub info_hash: InfoHash,
    pub display_name: Option<String>,
    pub trackers: Vec<String>,
}

impl MagnetLink {
    /// Parse a magnet URI.
    ///
    /// Format: magnet:?xt=urn:btih:<info_hash>&dn=<name>&tr=<tracker_url>
    /// info_hash can be 40-char hex or 32-char base32.
    pub fn parse(uri: &str) -> Result<Self, MagnetError> {
        if !uri.starts_with("magnet:?") {
            return Err(MagnetError::NotMagnet);
        }

        let query = &uri["magnet:?".len()..];
        let params: Vec<(&str, &str)> = query
            .split('&')
            .filter_map(|p| p.split_once('='))
            .collect();

        let mut info_hash = None;
        let mut display_name = None;
        let mut trackers = Vec::new();

        for (key, value) in params {
            match key {
                "xt" => {
                    let decoded = percent_decode(value);
                    if let Some(hash_str) = decoded.strip_prefix("urn:btih:") {
                        info_hash = Some(parse_info_hash(hash_str)?);
                    } else {
                        return Err(MagnetError::UnsupportedHashFunction(decoded));
                    }
                }
                "dn" => {
                    display_name = Some(percent_decode(value));
                }
                "tr" => {
                    trackers.push(percent_decode(value));
                }
                _ => {} // ignore unknown params
            }
        }

        let info_hash = info_hash.ok_or(MagnetError::MissingInfoHash)?;

        Ok(MagnetLink {
            info_hash,
            display_name,
            trackers,
        })
    }
}

fn parse_info_hash(s: &str) -> Result<InfoHash, MagnetError> {
    let bytes = if s.len() == 40 {
        // Hex-encoded
        hex::decode(s).map_err(|e| MagnetError::InvalidInfoHash(e.to_string()))?
    } else if s.len() == 32 {
        // Base32-encoded
        data_encoding::BASE32
            .decode(s.to_uppercase().as_bytes())
            .map_err(|e| MagnetError::InvalidInfoHash(e.to_string()))?
    } else {
        return Err(MagnetError::InvalidInfoHash(format!(
            "expected 40 hex or 32 base32 chars, got {} chars",
            s.len()
        )));
    };

    if bytes.len() != 20 {
        return Err(MagnetError::InvalidInfoHash(format!(
            "decoded to {} bytes, expected 20",
            bytes.len()
        )));
    }

    let mut hash = [0u8; 20];
    hash.copy_from_slice(&bytes);
    Ok(InfoHash::from_bytes(hash))
}

fn percent_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_magnet() {
        let uri = "magnet:?xt=urn:btih:da39a3ee5e6b4b0d3255bfef95601890afd80709&dn=test&tr=http://tracker.example.com/announce";
        let magnet = MagnetLink::parse(uri).unwrap();
        assert_eq!(magnet.display_name.as_deref(), Some("test"));
        assert_eq!(magnet.trackers.len(), 1);
        assert_eq!(magnet.info_hash.to_string(), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    }

    #[test]
    fn test_parse_base32_magnet() {
        // base32 of the same hash: 3I7HH3PF5NSLBUDV37XZKYAYRCX5QBY
        // Actually let's use a known conversion
        let hex_hash = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
        let bytes = hex::decode(hex_hash).unwrap();
        let base32 = data_encoding::BASE32.encode(&bytes);

        let uri = format!("magnet:?xt=urn:btih:{base32}");
        let magnet = MagnetLink::parse(&uri).unwrap();
        assert_eq!(magnet.info_hash.to_string(), hex_hash);
    }

    #[test]
    fn test_missing_xt() {
        let uri = "magnet:?dn=test";
        assert!(MagnetLink::parse(uri).is_err());
    }

    #[test]
    fn test_not_magnet() {
        assert!(MagnetLink::parse("http://example.com").is_err());
    }
}
