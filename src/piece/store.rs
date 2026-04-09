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
        let expected = &self.piece_hashes[piece_index as usize];
        let actual = Sha1::digest(data);
        actual.as_slice() == expected.0
    }

    pub fn num_pieces(&self) -> usize {
        self.piece_hashes.len()
    }
}
