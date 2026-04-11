use sha1::{Digest, Sha1};

use crate::torrent::types::Sha1Hash;

/// Piece store for SHA1 verification.
pub struct PieceStore {
    piece_hashes: Vec<Sha1Hash>,
}

impl PieceStore {
    pub fn new(piece_hashes: Vec<Sha1Hash>) -> Self {
        Self { piece_hashes }
    }

    /// Verify a complete piece against its expected SHA1 hash.
    pub fn verify(&self, piece_index: u32, data: &[u8]) -> bool {
        if piece_index as usize >= self.piece_hashes.len() {
            return false;
        }
        let expected = &self.piece_hashes[piece_index as usize];
        let actual = Sha1::digest(data);
        actual.as_slice() == expected.0
    }

    pub fn num_pieces(&self) -> usize {
        self.piece_hashes.len()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use sha1::{Digest, Sha1};

    #[test]
    fn test_verify_out_of_bounds() {
        let hashes = vec![Sha1Hash([0u8; 20])];
        let store = PieceStore::new(hashes);
        assert!(!store.verify(1, b"data"));
        assert!(!store.verify(u32::MAX, b"data"));
    }

    #[test]
    fn test_verify_valid_piece() {
        let data = b"hello world";
        let hash: [u8; 20] = Sha1::digest(data).into();
        let store = PieceStore::new(vec![Sha1Hash(hash)]);
        assert!(store.verify(0, data));
    }

    #[test]
    fn test_verify_invalid_piece() {
        let data = b"hello world";
        let hash: [u8; 20] = Sha1::digest(data).into();
        let store = PieceStore::new(vec![Sha1Hash(hash)]);
        assert!(!store.verify(0, b"wrong data"));
    }
}
