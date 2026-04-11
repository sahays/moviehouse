use std::path::PathBuf;

use sha1::{Digest, Sha1};

use crate::bencode::{self, BValue, Decoder};
use crate::torrent::types::{InfoHash, Sha1Hash};

#[derive(Debug, thiserror::Error)]
pub enum MetainfoError {
    #[error("bencode decode error: {0}")]
    Decode(#[from] bencode::DecodeError),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("invalid field type for '{0}'")]
    InvalidFieldType(&'static str),
    #[error("invalid pieces length: must be multiple of 20, got {0}")]
    InvalidPiecesLength(usize),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid piece length: {0}")]
    InvalidPieceLength(String),
    #[error("negative file length: {0}")]
    NegativeFileLength(i64),
    #[error("path traversal in {0}")]
    PathTraversal(&'static str),
    #[error("torrent file too large: {0} bytes")]
    FileTooLarge(u64),
}

/// Parsed .torrent metainfo.
#[derive(Debug, Clone)]
pub struct Metainfo {
    pub info_hash: InfoHash,
    pub info: Info,
    pub announce: Option<String>,
    /// BEP12: multi-tracker. List of tiers, each tier is a list of tracker URLs.
    pub announce_list: Option<Vec<Vec<String>>>,
    pub creation_date: Option<i64>,
    pub comment: Option<String>,
    pub created_by: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Info {
    pub piece_length: u32,
    pub pieces: Vec<Sha1Hash>,
    pub name: String,
    pub files: FileLayout,
    pub total_length: u64,
}

#[derive(Debug, Clone)]
pub enum FileLayout {
    Single { length: u64 },
    Multi { files: Vec<FileEntry> },
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub length: u64,
    pub path: PathBuf,
}

impl Metainfo {
    /// Parse a .torrent file from raw bytes.
    /// Computes `info_hash` from the raw bytes of the info dictionary (not re-encoded).
    pub fn from_bytes(data: &[u8]) -> Result<Self, MetainfoError> {
        let mut decoder = Decoder::new(data);
        let result = decoder.decode()?;
        let root = result.value;

        let _root_dict = root
            .as_dict()
            .ok_or(MetainfoError::InvalidFieldType("root"))?;

        // We need the raw bytes of the info dict for info_hash computation.
        // Re-parse to find the exact byte range of the "info" value.
        let info_hash = compute_info_hash(data)?;

        let info_val = root
            .get_str("info")
            .ok_or(MetainfoError::MissingField("info"))?;
        let info = parse_info(info_val)?;

        let announce = root
            .get_str("announce")
            .and_then(|v| v.as_str())
            .map(String::from);

        let announce_list = root.get_str("announce-list").and_then(|v| {
            v.as_list().map(|tiers| {
                tiers
                    .iter()
                    .filter_map(|tier| {
                        tier.as_list().map(|urls| {
                            urls.iter()
                                .filter_map(|u| u.as_str().map(String::from))
                                .collect()
                        })
                    })
                    .collect()
            })
        });

        let creation_date = root
            .get_str("creation date")
            .and_then(super::super::bencode::value::BValue::as_int);
        let comment = root
            .get_str("comment")
            .and_then(|v| v.as_str())
            .map(String::from);
        let created_by = root
            .get_str("created by")
            .and_then(|v| v.as_str())
            .map(String::from);

        Ok(Metainfo {
            info_hash,
            info,
            announce,
            announce_list,
            creation_date,
            comment,
            created_by,
        })
    }

    /// Parse from a file path.
    pub fn from_file(path: &std::path::Path) -> Result<Self, MetainfoError> {
        let meta = std::fs::metadata(path)?;
        if meta.len() > 10 * 1024 * 1024 {
            return Err(MetainfoError::FileTooLarge(meta.len()));
        }
        let data = std::fs::read(path)?;
        Self::from_bytes(&data)
    }

    /// Get all tracker URLs (from announce and announce-list), deduplicated.
    pub fn tracker_urls(&self) -> Vec<String> {
        let mut urls = Vec::new();

        if let Some(ref announce_list) = self.announce_list {
            for tier in announce_list {
                for url in tier {
                    if !urls.contains(url) {
                        urls.push(url.clone());
                    }
                }
            }
        }

        if let Some(ref announce) = self.announce
            && !urls.contains(announce)
        {
            urls.insert(0, announce.clone());
        }

        urls
    }

    /// Number of pieces.
    pub fn num_pieces(&self) -> usize {
        self.info.pieces.len()
    }

    /// Construct from raw info dictionary bytes (for magnet link metadata download).
    /// `raw_info` is the bencoded info dict. `info_hash` was already verified by caller.
    pub fn from_info_bytes(
        raw_info: &[u8],
        info_hash: InfoHash,
        trackers: Vec<String>,
    ) -> Result<Self, MetainfoError> {
        let info_val = bencode::decode(raw_info)?;
        let info = parse_info(&info_val)?;
        let announce = trackers.first().cloned();
        let announce_list = if trackers.len() > 1 {
            Some(vec![trackers])
        } else {
            None
        };
        Ok(Metainfo {
            info_hash,
            info,
            announce,
            announce_list,
            creation_date: None,
            comment: None,
            created_by: None,
        })
    }
}

impl Info {
    /// Number of blocks (16 KiB chunks) in a given piece.
    pub fn blocks_in_piece(&self, piece_index: u32) -> u32 {
        let piece_len = self.piece_length(piece_index);
        piece_len.div_ceil(16384)
    }

    /// Length of a specific piece (last piece may be shorter).
    pub fn piece_length(&self, piece_index: u32) -> u32 {
        let total_pieces = self.pieces.len() as u32;
        if piece_index == total_pieces - 1 {
            // Last piece: may be shorter
            let remainder = (self.total_length % self.piece_length as u64) as u32;
            if remainder == 0 {
                self.piece_length
            } else {
                remainder
            }
        } else {
            self.piece_length
        }
    }
}

/// Compute `info_hash` by finding the raw bytes of the "info" dictionary value.
fn compute_info_hash(data: &[u8]) -> Result<InfoHash, MetainfoError> {
    // Walk the top-level dict to find the byte range of the "info" key's value.
    let decoder = Decoder::new(data);

    // Skip the 'd' of the outer dict
    if decoder.position() >= data.len() || data[decoder.position()] != b'd' {
        return Err(MetainfoError::InvalidFieldType("root"));
    }

    let mut pos = 1; // skip 'd'

    loop {
        if pos >= data.len() {
            return Err(MetainfoError::MissingField("info"));
        }
        if data[pos] == b'e' {
            break; // end of dict
        }

        // Decode the key
        let mut key_decoder = Decoder::new(&data[pos..]);
        let key_result = key_decoder.decode()?;
        let key_end = pos + key_result.end;
        let key = key_result.value;

        // Decode the value (to get its byte range)
        let mut val_decoder = Decoder::new(&data[key_end..]);
        let val_result = val_decoder.decode()?;
        let val_start = key_end;
        let val_end = key_end + val_result.end;

        if key.as_bytes() == Some(b"info") {
            // Found it! Hash the raw bytes.
            let raw_info = &data[val_start..val_end];
            let hash = Sha1::digest(raw_info);
            let mut h = [0u8; 20];
            h.copy_from_slice(&hash);
            return Ok(InfoHash::from_bytes(h));
        }

        pos = val_end;
    }

    Err(MetainfoError::MissingField("info"))
}

fn parse_info(val: &BValue) -> Result<Info, MetainfoError> {
    let _dict = val
        .as_dict()
        .ok_or(MetainfoError::InvalidFieldType("info"))?;

    let piece_length_raw = val
        .get_str("piece length")
        .and_then(super::super::bencode::value::BValue::as_int)
        .ok_or(MetainfoError::MissingField("piece length"))?;
    if piece_length_raw <= 0 || piece_length_raw > 256 * 1024 * 1024 {
        return Err(MetainfoError::InvalidPieceLength(format!(
            "{piece_length_raw} (must be 1..=268435456)"
        )));
    }
    let piece_length = piece_length_raw as u32;

    let pieces_raw = val
        .get_str("pieces")
        .and_then(|v| v.as_bytes())
        .ok_or(MetainfoError::MissingField("pieces"))?;

    if pieces_raw.len() % 20 != 0 {
        return Err(MetainfoError::InvalidPiecesLength(pieces_raw.len()));
    }

    let pieces: Vec<Sha1Hash> = pieces_raw
        .chunks_exact(20)
        .map(|chunk| {
            let mut h = [0u8; 20];
            h.copy_from_slice(chunk);
            Sha1Hash::from_bytes(h)
        })
        .collect();

    let name = val
        .get_str("name")
        .and_then(|v| v.as_str())
        .ok_or(MetainfoError::MissingField("name"))?
        .to_string();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err(MetainfoError::PathTraversal("name"));
    }

    // Single-file or multi-file?
    let files = if let Some(files_val) = val.get_str("files") {
        // Multi-file mode
        let files_list = files_val
            .as_list()
            .ok_or(MetainfoError::InvalidFieldType("files"))?;

        let mut entries = Vec::new();
        for f in files_list {
            let length_raw = f
                .get_str("length")
                .and_then(super::super::bencode::value::BValue::as_int)
                .ok_or(MetainfoError::MissingField("files[].length"))?;
            if length_raw < 0 {
                return Err(MetainfoError::NegativeFileLength(length_raw));
            }
            let length = length_raw as u64;

            let path_list = f
                .get_str("path")
                .and_then(|v| v.as_list())
                .ok_or(MetainfoError::MissingField("files[].path"))?;

            for component in path_list.iter().filter_map(|p| p.as_str()) {
                if component.is_empty()
                    || component == ".."
                    || component.contains('/')
                    || component.contains('\\')
                {
                    return Err(MetainfoError::PathTraversal("files[].path"));
                }
            }

            let path: PathBuf = path_list.iter().filter_map(|p| p.as_str()).collect();

            entries.push(FileEntry { length, path });
        }
        FileLayout::Multi { files: entries }
    } else {
        // Single-file mode
        let length_raw = val
            .get_str("length")
            .and_then(super::super::bencode::value::BValue::as_int)
            .ok_or(MetainfoError::MissingField("length"))?;
        if length_raw < 0 {
            return Err(MetainfoError::NegativeFileLength(length_raw));
        }
        let length = length_raw as u64;
        FileLayout::Single { length }
    };

    let total_length = match &files {
        FileLayout::Single { length } => *length,
        FileLayout::Multi { files } => files.iter().map(|f| f.length).sum(),
    };

    Ok(Info {
        piece_length,
        pieces,
        name,
        files,
        total_length,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::bencode::encode::encode;
    use std::collections::BTreeMap;

    /// Build minimal single-file torrent bencode bytes.
    fn make_single_file_torrent(name: &str, piece_length: i64, length: i64) -> Vec<u8> {
        let mut info = BTreeMap::new();
        info.insert(b"name".to_vec(), BValue::Bytes(name.as_bytes().to_vec()));
        info.insert(b"piece length".to_vec(), BValue::Int(piece_length));
        info.insert(b"pieces".to_vec(), BValue::Bytes(vec![0u8; 20])); // 1 fake hash
        info.insert(b"length".to_vec(), BValue::Int(length));

        let mut root = BTreeMap::new();
        root.insert(b"info".to_vec(), BValue::Dict(info));
        encode(&BValue::Dict(root))
    }

    /// Build minimal multi-file torrent bencode bytes.
    fn make_multi_file_torrent(name: &str, piece_length: i64, files: &[(i64, &[&str])]) -> Vec<u8> {
        let mut info = BTreeMap::new();
        info.insert(b"name".to_vec(), BValue::Bytes(name.as_bytes().to_vec()));
        info.insert(b"piece length".to_vec(), BValue::Int(piece_length));
        info.insert(b"pieces".to_vec(), BValue::Bytes(vec![0u8; 20]));

        let file_list: Vec<BValue> = files
            .iter()
            .map(|(len, path_parts)| {
                let mut f = BTreeMap::new();
                f.insert(b"length".to_vec(), BValue::Int(*len));
                let path = path_parts
                    .iter()
                    .map(|p| BValue::Bytes(p.as_bytes().to_vec()))
                    .collect();
                f.insert(b"path".to_vec(), BValue::List(path));
                BValue::Dict(f)
            })
            .collect();
        info.insert(b"files".to_vec(), BValue::List(file_list));

        let mut root = BTreeMap::new();
        root.insert(b"info".to_vec(), BValue::Dict(info));
        encode(&BValue::Dict(root))
    }

    #[test]
    fn test_valid_single_file_torrent() {
        let data = make_single_file_torrent("test.txt", 16384, 1000);
        let meta = Metainfo::from_bytes(&data).unwrap();
        assert_eq!(meta.info.name, "test.txt");
        assert_eq!(meta.info.piece_length, 16384);
        assert_eq!(meta.info.total_length, 1000);
    }

    #[test]
    fn test_path_traversal_dotdot() {
        let data = make_multi_file_torrent("safe", 16384, &[(100, &["..", "etc", "passwd"])]);
        assert!(Metainfo::from_bytes(&data).is_err());
    }

    #[test]
    fn test_path_traversal_slash_in_component() {
        let data = make_multi_file_torrent("safe", 16384, &[(100, &["sub/dir", "file.txt"])]);
        assert!(Metainfo::from_bytes(&data).is_err());
    }

    #[test]
    fn test_path_traversal_name_dotdot() {
        let data = make_single_file_torrent("../../../etc/passwd", 16384, 100);
        assert!(Metainfo::from_bytes(&data).is_err());
    }

    #[test]
    fn test_name_with_slash() {
        let data = make_single_file_torrent("sub/dir", 16384, 100);
        assert!(Metainfo::from_bytes(&data).is_err());
    }

    #[test]
    fn test_negative_piece_length() {
        let data = make_single_file_torrent("test.txt", -1, 100);
        assert!(Metainfo::from_bytes(&data).is_err());
    }

    #[test]
    fn test_zero_piece_length() {
        let data = make_single_file_torrent("test.txt", 0, 100);
        assert!(Metainfo::from_bytes(&data).is_err());
    }

    #[test]
    fn test_negative_file_length() {
        let data = make_multi_file_torrent("safe", 16384, &[(-1, &["file.txt"])]);
        assert!(Metainfo::from_bytes(&data).is_err());
    }

    #[test]
    fn test_negative_single_file_length() {
        let data = make_single_file_torrent("test.txt", 16384, -100);
        assert!(Metainfo::from_bytes(&data).is_err());
    }
}
