/// Compact bitfield tracking which pieces we have.
#[derive(Debug, Clone)]
pub struct Bitfield {
    bits: Vec<u8>,
    num_pieces: usize,
    count: usize,
}

impl Bitfield {
    pub fn new(num_pieces: usize) -> Self {
        let byte_len = num_pieces.div_ceil(8);
        Self {
            bits: vec![0u8; byte_len],
            num_pieces,
            count: 0,
        }
    }

    /// Create from raw wire bytes (received from peer).
    pub fn from_bytes(bytes: &[u8], num_pieces: usize) -> Self {
        let mut bf = Self {
            bits: bytes.to_vec(),
            num_pieces,
            count: 0,
        };
        // Count set bits
        bf.count = bf.count_set_bits();
        bf
    }

    pub fn set(&mut self, index: usize) {
        if index < self.num_pieces && !self.has(index) {
            self.bits[index / 8] |= 1 << (7 - (index % 8));
            self.count += 1;
        }
    }

    pub fn has(&self, index: usize) -> bool {
        if index >= self.num_pieces {
            return false;
        }
        self.bits[index / 8] & (1 << (7 - (index % 8))) != 0
    }

    pub fn is_complete(&self) -> bool {
        self.count == self.num_pieces
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn num_pieces(&self) -> usize {
        self.num_pieces
    }

    /// Convert to wire bytes for sending Bitfield message.
    pub fn to_bytes(&self) -> &[u8] {
        &self.bits
    }

    fn count_set_bits(&self) -> usize {
        let mut count = 0;
        for i in 0..self.num_pieces {
            if self.has(i) {
                count += 1;
            }
        }
        count
    }

    /// Iterate over all piece indices that are set.
    pub fn set_indices(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.num_pieces).filter(|&i| self.has(i))
    }

    /// Iterate over all piece indices that are NOT set.
    pub fn missing_indices(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.num_pieces).filter(|&i| !self.has(i))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_new_bitfield() {
        let bf = Bitfield::new(10);
        assert_eq!(bf.num_pieces(), 10);
        assert_eq!(bf.count(), 0);
        assert!(!bf.is_complete());
    }

    #[test]
    fn test_set_and_has() {
        let mut bf = Bitfield::new(16);
        bf.set(0);
        bf.set(5);
        bf.set(15);
        assert!(bf.has(0));
        assert!(bf.has(5));
        assert!(bf.has(15));
        assert!(!bf.has(1));
        assert_eq!(bf.count(), 3);
    }

    #[test]
    fn test_complete() {
        let mut bf = Bitfield::new(3);
        bf.set(0);
        bf.set(1);
        bf.set(2);
        assert!(bf.is_complete());
    }

    #[test]
    fn test_from_bytes() {
        // First byte: 0b11100000 = 0xE0 → bits 0,1,2 set
        let bf = Bitfield::from_bytes(&[0xE0], 3);
        assert!(bf.has(0));
        assert!(bf.has(1));
        assert!(bf.has(2));
        assert_eq!(bf.count(), 3);
    }
}
