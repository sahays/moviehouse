use std::net::SocketAddr;

use sha1::{Digest, Sha1};

use crate::torrent::types::InfoHash;

const METADATA_PIECE_SIZE: usize = 16384;

pub struct MetadataBuffer {
    total_size: usize,
    pub num_pieces: usize,
    buffer: Vec<u8>,
    received: Vec<bool>,
    received_count: usize,
    assigned: Vec<Option<SocketAddr>>,
}

impl MetadataBuffer {
    pub fn new(total_size: usize) -> Self {
        let num_pieces = total_size.div_ceil(METADATA_PIECE_SIZE);
        Self {
            total_size,
            num_pieces,
            buffer: vec![0u8; total_size],
            received: vec![false; num_pieces],
            received_count: 0,
            assigned: vec![None; num_pieces],
        }
    }

    pub fn on_data(&mut self, piece: u32, data: &[u8]) -> bool {
        let idx = piece as usize;
        if idx >= self.num_pieces || self.received[idx] {
            return false;
        }
        if data.len() > METADATA_PIECE_SIZE {
            return false;
        }
        let offset = idx * METADATA_PIECE_SIZE;
        if offset >= self.total_size {
            return false;
        }
        let end = (offset + data.len()).min(self.total_size);
        self.buffer[offset..end].copy_from_slice(&data[..end - offset]);
        self.received[idx] = true;
        self.received_count += 1;
        self.assigned[idx] = None;
        self.is_complete()
    }

    pub fn on_reject(&mut self, piece: u32) {
        let idx = piece as usize;
        if idx < self.num_pieces {
            self.assigned[idx] = None;
        }
    }

    pub fn on_peer_lost(&mut self, addr: &SocketAddr) {
        for slot in &mut self.assigned {
            if *slot == Some(*addr) {
                *slot = None;
            }
        }
    }

    pub fn next_request(&mut self, addr: SocketAddr) -> Option<u32> {
        for i in 0..self.num_pieces {
            if !self.received[i] && self.assigned[i].is_none() {
                self.assigned[i] = Some(addr);
                return Some(i as u32);
            }
        }
        None
    }

    pub fn is_complete(&self) -> bool {
        self.received_count == self.num_pieces
    }

    pub fn verify(self, info_hash: &InfoHash) -> Option<Vec<u8>> {
        let hash = Sha1::digest(&self.buffer);
        if hash.as_slice() == info_hash.0 {
            Some(self.buffer)
        } else {
            None
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_buffer_normal() {
        let mut buf = MetadataBuffer::new(1000);
        let data = vec![0xAB; 1000];
        assert!(buf.on_data(0, &data));
        assert_eq!(buf.buffer, data);
    }

    #[test]
    fn test_metadata_buffer_oversized_piece() {
        let mut buf = MetadataBuffer::new(1000);
        let data = vec![0u8; METADATA_PIECE_SIZE + 1];
        assert!(!buf.on_data(0, &data));
    }

    #[test]
    fn test_metadata_buffer_out_of_bounds_piece() {
        let mut buf = MetadataBuffer::new(100);
        assert_eq!(buf.num_pieces, 1);
        assert!(!buf.on_data(1, &[0u8; 100]));
        assert!(!buf.on_data(9999, &[0u8; 10]));
    }

    #[test]
    fn test_metadata_buffer_duplicate_piece() {
        let mut buf = MetadataBuffer::new(METADATA_PIECE_SIZE * 2);
        let data = vec![0u8; METADATA_PIECE_SIZE];
        assert!(!buf.on_data(0, &data)); // not complete yet
        assert!(!buf.on_data(0, &data)); // duplicate, rejected
    }
}
