use std::collections::HashMap;


use super::bitfield::Bitfield;
use crate::peer::message::BLOCK_SIZE;

/// Block request descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockRequest {
    pub piece_index: u32,
    pub offset: u32,
    pub length: u32,
}

/// Result of processing a received block.
pub enum BlockResult {
    /// Block was already received -- duplicate, ignore.
    Duplicate,
    /// New block received, piece not yet complete.
    Progress { new_bytes: u32 },
    /// All blocks received -- here's the assembled piece data for verification.
    PieceComplete(Vec<u8>),
}

/// Tracks download progress for a single piece.
struct PieceProgress {
    piece_length: u32,
    num_blocks: u32,
    blocks_received: Vec<bool>,
    blocks_assigned: Vec<bool>,
    received_count: u32,
    /// Linear cursor -- advances without wrapping. Reset to 0 on receive.
    assign_cursor: u32,
    data: Vec<u8>,
    /// Number of distinct peers actively working on this piece.
    active_peers: u8,
}

impl PieceProgress {
    fn new(piece_length: u32) -> Self {
        let num_blocks = (piece_length + BLOCK_SIZE - 1) / BLOCK_SIZE;
        Self {
            piece_length,
            num_blocks,
            blocks_received: vec![false; num_blocks as usize],
            blocks_assigned: vec![false; num_blocks as usize],
            received_count: 0,
            assign_cursor: 0,
            data: vec![0u8; piece_length as usize],
            active_peers: 0,
        }
    }

    fn block_length(&self, block_index: u32) -> u32 {
        if block_index == self.num_blocks - 1 {
            let remainder = self.piece_length % BLOCK_SIZE;
            if remainder == 0 { BLOCK_SIZE } else { remainder }
        } else {
            BLOCK_SIZE
        }
    }

    #[inline]
    fn is_complete(&self) -> bool {
        self.received_count == self.num_blocks
    }

    /// Assign the next block that is neither received nor already assigned.
    /// Linear scan from cursor, no wrapping. Returns None when all blocks are spoken for.
    fn assign_next_block(&mut self) -> Option<u32> {
        while self.assign_cursor < self.num_blocks {
            let idx = self.assign_cursor as usize;
            self.assign_cursor += 1;
            if !self.blocks_received[idx] && !self.blocks_assigned[idx] {
                self.blocks_assigned[idx] = true;
                return Some(idx as u32);
            }
        }
        None
    }

    /// Mark a block as received. Returns true if this was new data.
    fn mark_received(&mut self, block_idx: u32) -> bool {
        let idx = block_idx as usize;
        if idx >= self.blocks_received.len() || self.blocks_received[idx] {
            return false;
        }
        self.blocks_received[idx] = true;
        self.blocks_assigned[idx] = true; // also mark assigned (no need to re-assign)
        self.received_count += 1;
        // Reset cursor so next scan can pick up remaining gaps
        self.assign_cursor = 0;
        true
    }

    /// Unassign a block (peer that was fetching it choked/disconnected).
    fn mark_unassigned(&mut self, block_idx: u32) {
        let idx = block_idx as usize;
        if idx < self.blocks_assigned.len() && !self.blocks_received[idx] {
            self.blocks_assigned[idx] = false;
            // Move cursor back so this block can be re-assigned
            self.assign_cursor = self.assign_cursor.min(block_idx);
        }
    }

    /// Return all unreceived blocks (ignoring assignment), for endgame mode.
    fn unreceived_blocks(&self) -> Vec<(u32, u32)> {
        let mut blocks = Vec::new();
        for i in 0..self.num_blocks {
            if !self.blocks_received[i as usize] {
                blocks.push((i, self.block_length(i)));
            }
        }
        blocks
    }
}

/// Piece selection strategy.
#[derive(Debug, Clone, Copy, PartialEq)]
enum PickerMode {
    RandomFirst,
    RarestFirst,
    Endgame,
}

/// Piece picker: rarest-first selection with per-block assignment tracking.
pub struct PiecePicker {
    num_pieces: usize,
    availability: Vec<u16>,
    have: Bitfield,
    in_progress: HashMap<u32, PieceProgress>,
    mode: PickerMode,
    piece_length: u32,
    total_length: u64,
    verified_count: usize,
}

impl PiecePicker {
    pub fn new(num_pieces: usize, piece_length: u32, total_length: u64) -> Self {
        Self {
            num_pieces,
            availability: vec![0; num_pieces],
            have: Bitfield::new(num_pieces),
            in_progress: HashMap::new(),
            mode: PickerMode::RandomFirst,
            piece_length,
            total_length,
            verified_count: 0,
        }
    }

    pub fn peer_has_bitfield(&mut self, bitfield: &Bitfield) {
        for i in bitfield.set_indices() {
            if i < self.num_pieces {
                self.availability[i] = self.availability[i].saturating_add(1);
            }
        }
    }

    pub fn peer_has_piece(&mut self, piece_index: u32) {
        if (piece_index as usize) < self.num_pieces {
            self.availability[piece_index as usize] =
                self.availability[piece_index as usize].saturating_add(1);
        }
    }

    pub fn peer_disconnected(&mut self, bitfield: &Bitfield) {
        for i in bitfield.set_indices() {
            if i < self.num_pieces {
                self.availability[i] = self.availability[i].saturating_sub(1);
            }
        }
    }

    /// Check if we are in endgame mode.
    pub fn is_endgame(&self) -> bool {
        self.mode == PickerMode::Endgame
    }

    /// For endgame mode: return all unreceived blocks for pieces the peer has.
    /// Ignores assignment so blocks can be duplicated across peers.
    pub fn endgame_requests(&self, peer_bf: &Bitfield) -> Vec<BlockRequest> {
        let mut requests = Vec::new();
        for (&piece_idx, progress) in &self.in_progress {
            if !peer_bf.has(piece_idx as usize) {
                continue;
            }
            for (block_idx, length) in progress.unreceived_blocks() {
                requests.push(BlockRequest {
                    piece_index: piece_idx,
                    offset: block_idx * BLOCK_SIZE,
                    length,
                });
            }
        }
        requests
    }

    /// Pick the next block to assign to a peer.
    /// Returns None if no useful block is available for this peer's bitfield.
    pub fn pick_block(&mut self, peer_bitfield: &Bitfield) -> Option<BlockRequest> {
        // Try to continue an in-progress piece this peer has
        // Sort candidates by active_peers ascending (prefer less-contended pieces)
        let mut pieces: Vec<(u32, u8)> = self.in_progress.iter()
            .filter(|(idx, _)| peer_bitfield.has(**idx as usize))
            .map(|(idx, p)| (*idx, p.active_peers))
            .collect();
        pieces.sort_by_key(|&(_, active)| active);

        for (piece_idx, _) in pieces {
            let progress = self.in_progress.get_mut(&piece_idx).unwrap();
            if let Some(block_idx) = progress.assign_next_block() {
                progress.active_peers = progress.active_peers.saturating_add(1);
                let length = progress.block_length(block_idx);
                return Some(BlockRequest {
                    piece_index: piece_idx,
                    offset: block_idx * BLOCK_SIZE,
                    length,
                });
            }
        }

        // Start a new piece
        let piece_idx = self.pick_piece(peer_bitfield)?;
        let piece_len = self.actual_piece_length(piece_idx);
        let progress = self
            .in_progress
            .entry(piece_idx)
            .or_insert_with(|| PieceProgress::new(piece_len));

        let block_idx = progress.assign_next_block()?;
        progress.active_peers = progress.active_peers.saturating_add(1);
        let length = progress.block_length(block_idx);
        self.check_mode();

        Some(BlockRequest {
            piece_index: piece_idx,
            offset: block_idx * BLOCK_SIZE,
            length,
        })
    }

    /// O(n) single pass with reservoir sampling for rarest piece.
    fn pick_piece(&self, peer_bitfield: &Bitfield) -> Option<u32> {
        let mut best_idx: Option<u32> = None;
        let mut best_avail = u16::MAX;
        let mut best_count = 0u32;

        for i in 0..self.num_pieces {
            if !peer_bitfield.has(i)
                || self.have.has(i)
                || self.in_progress.contains_key(&(i as u32))
            {
                continue;
            }

            let avail = self.availability[i];
            if avail < best_avail {
                best_avail = avail;
                best_idx = Some(i as u32);
                best_count = 1;
            } else if avail == best_avail {
                best_count += 1;
                if rand::random::<u32>() % best_count == 0 {
                    best_idx = Some(i as u32);
                }
            }
        }

        best_idx
    }

    /// Record a received block. Returns status for the session to act on.
    /// Does NOT mark the piece as verified -- call `mark_verified()` separately.
    pub fn block_received(
        &mut self,
        piece_index: u32,
        offset: u32,
        data: &[u8],
    ) -> BlockResult {
        let Some(progress) = self.in_progress.get_mut(&piece_index) else {
            return BlockResult::Duplicate;
        };
        let block_idx = offset / BLOCK_SIZE;

        if !progress.mark_received(block_idx) {
            return BlockResult::Duplicate;
        }

        // Copy data into assembled piece
        let start = offset as usize;
        let end = start + data.len();
        if end <= progress.data.len() {
            progress.data[start..end].copy_from_slice(data);
        }

        if progress.is_complete() {
            let complete = self.in_progress.remove(&piece_index).unwrap();
            // Do NOT set self.have here -- wait for verification
            BlockResult::PieceComplete(complete.data)
        } else {
            BlockResult::Progress {
                new_bytes: data.len() as u32,
            }
        }
    }

    /// Called by session AFTER SHA1 verification succeeds and disk write completes.
    pub fn mark_verified(&mut self, piece_index: u32) {
        self.have.set(piece_index as usize);
        self.verified_count += 1;
        if self.mode == PickerMode::RandomFirst && self.verified_count > 0 {
            self.mode = PickerMode::RarestFirst;
        }
        self.check_mode();
    }

    /// Mark a piece as failed (hash mismatch). Re-queue for download.
    pub fn piece_failed(&mut self, piece_index: u32) {
        self.in_progress.remove(&piece_index);
    }

    /// Unassign a specific block (peer that was fetching it went away).
    pub fn unassign_block(&mut self, piece_index: u32, offset: u32) {
        let block_idx = offset / BLOCK_SIZE;
        if let Some(progress) = self.in_progress.get_mut(&piece_index) {
            progress.mark_unassigned(block_idx);
        }
    }

    fn check_mode(&mut self) {
        let remaining = self.num_pieces - self.have.count();
        if remaining > 0 && self.in_progress.len() == remaining {
            self.mode = PickerMode::Endgame;
        }
    }

    pub fn is_complete(&self) -> bool {
        self.have.is_complete()
    }

    pub fn pieces_done(&self) -> usize {
        self.verified_count
    }

    fn actual_piece_length(&self, piece_index: u32) -> u32 {
        if piece_index as usize == self.num_pieces - 1 {
            let remainder = (self.total_length % self.piece_length as u64) as u32;
            if remainder == 0 { self.piece_length } else { remainder }
        } else {
            self.piece_length
        }
    }

    pub fn is_interested_in(&self, peer_bitfield: &Bitfield) -> bool {
        for i in 0..self.num_pieces {
            if peer_bitfield.has(i) && !self.have.has(i) {
                return true;
            }
        }
        false
    }

    pub fn set_have(&mut self, piece_index: usize) {
        self.have.set(piece_index);
    }

    pub fn our_bitfield(&self) -> &Bitfield {
        &self.have
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pick_block_basic() {
        let mut picker = PiecePicker::new(4, 32768, 4 * 32768);
        let mut peer_bf = Bitfield::new(4);
        for i in 0..4 { peer_bf.set(i); }
        picker.peer_has_bitfield(&peer_bf);

        let block = picker.pick_block(&peer_bf);
        assert!(block.is_some());
        assert_eq!(block.unwrap().offset, 0);
    }

    #[test]
    fn test_rarest_first() {
        let mut picker = PiecePicker::new(4, 32768, 4 * 32768);
        picker.mode = PickerMode::RarestFirst;
        picker.availability = vec![5, 5, 1, 5];

        let mut peer_bf = Bitfield::new(4);
        for i in 0..4 { peer_bf.set(i); }

        let block = picker.pick_block(&peer_bf);
        assert!(block.is_some());
        assert_eq!(block.unwrap().piece_index, 2);
    }

    #[test]
    fn test_piece_completion() {
        let mut picker = PiecePicker::new(1, 32768, 32768);
        let mut peer_bf = Bitfield::new(1);
        peer_bf.set(0);
        picker.peer_has_bitfield(&peer_bf);

        let b1 = picker.pick_block(&peer_bf).unwrap();
        assert_eq!(b1.offset, 0);
        assert!(matches!(picker.block_received(0, 0, &[1u8; 16384]), BlockResult::Progress { .. }));

        let b2 = picker.pick_block(&peer_bf).unwrap();
        assert_eq!(b2.offset, 16384);
        assert!(matches!(picker.block_received(0, 16384, &[2u8; 16384]), BlockResult::PieceComplete(_)));

        // Not complete until mark_verified is called
        assert!(!picker.is_complete());
        picker.mark_verified(0);
        assert!(picker.is_complete());
    }

    #[test]
    fn test_no_duplicate_assignment() {
        // 1 piece, 2 blocks
        let mut picker = PiecePicker::new(1, 32768, 32768);
        let mut peer_bf = Bitfield::new(1);
        peer_bf.set(0);
        picker.peer_has_bitfield(&peer_bf);

        // Two peers pick blocks -- should get different blocks
        let b1 = picker.pick_block(&peer_bf).unwrap();
        let b2 = picker.pick_block(&peer_bf).unwrap();
        assert_ne!(b1.offset, b2.offset);

        // Third pick: all blocks assigned, should return None
        assert!(picker.pick_block(&peer_bf).is_none());
    }

    #[test]
    fn test_unassign_allows_reassignment() {
        let mut picker = PiecePicker::new(1, 32768, 32768);
        let mut peer_bf = Bitfield::new(1);
        peer_bf.set(0);
        picker.peer_has_bitfield(&peer_bf);

        let b1 = picker.pick_block(&peer_bf).unwrap(); // block 0
        let _b2 = picker.pick_block(&peer_bf).unwrap(); // block 1
        assert!(picker.pick_block(&peer_bf).is_none()); // none left

        // Peer that had block 0 disconnects
        picker.unassign_block(0, b1.offset);

        // Now block 0 is available again
        let b3 = picker.pick_block(&peer_bf).unwrap();
        assert_eq!(b3.offset, b1.offset);
    }

    #[test]
    fn test_duplicate_block_ignored() {
        let mut picker = PiecePicker::new(1, 32768, 32768);
        let mut peer_bf = Bitfield::new(1);
        peer_bf.set(0);
        picker.peer_has_bitfield(&peer_bf);

        picker.pick_block(&peer_bf);
        assert!(matches!(picker.block_received(0, 0, &[1u8; 16384]), BlockResult::Progress { .. }));
        assert!(matches!(picker.block_received(0, 0, &[1u8; 16384]), BlockResult::Duplicate));
    }

    #[test]
    fn test_endgame_mode() {
        // 2 pieces, each 1 block (16384 bytes) to keep it simple
        let mut picker = PiecePicker::new(2, 16384, 2 * 16384);
        let mut peer_bf = Bitfield::new(2);
        peer_bf.set(0);
        peer_bf.set(1);
        picker.peer_has_bitfield(&peer_bf);

        // Pick a block -- could be piece 0 or 1
        let b1 = picker.pick_block(&peer_bf).unwrap();
        let piece_idx = b1.piece_index;

        // Complete that piece
        picker.block_received(piece_idx, 0, &[0u8; 16384]);
        picker.mark_verified(piece_idx);

        // Now pick from the remaining piece -- should enter endgame
        let b2 = picker.pick_block(&peer_bf).unwrap();
        assert_ne!(b2.piece_index, piece_idx);
        assert!(picker.is_endgame());

        // endgame_requests should return unreceived blocks for the remaining piece
        let reqs = picker.endgame_requests(&peer_bf);
        // The piece we just started has 1 block that was assigned but not received
        // Actually we need to check: assign_next_block was called which marks block as assigned
        // but not received. unreceived_blocks returns blocks not received regardless of assignment.
        assert!(!reqs.is_empty());
    }

    #[test]
    fn test_is_endgame_default_false() {
        let picker = PiecePicker::new(4, 32768, 4 * 32768);
        assert!(!picker.is_endgame());
    }
}
