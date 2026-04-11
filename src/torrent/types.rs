use std::fmt;

/// 20-byte SHA1 hash used as `info_hash` and piece hashes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sha1Hash(pub [u8; 20]);

impl Sha1Hash {
    pub fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// URL-encode the hash for tracker requests (percent-encoding of raw bytes).
    pub fn url_encode(&self) -> String {
        use percent_encoding::{NON_ALPHANUMERIC, percent_encode};
        percent_encode(&self.0, NON_ALPHANUMERIC).to_string()
    }
}

impl fmt::Debug for Sha1Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sha1Hash({})", hex::encode(self.0))
    }
}

impl fmt::Display for Sha1Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Type alias for clarity.
pub type InfoHash = Sha1Hash;

/// 20-byte peer ID. Generated randomly per session with client prefix.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerId(pub [u8; 20]);

impl PeerId {
    /// Generate a new random peer ID with Azureus-style prefix: -MH0100-
    pub fn generate() -> Self {
        use rand::Rng;
        let mut id = [0u8; 20];
        id[..8].copy_from_slice(b"-MH0100-");
        rand::thread_rng().fill(&mut id[8..]);
        Self(id)
    }

    pub fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Show the prefix as ASCII, rest as hex
        let prefix = String::from_utf8_lossy(&self.0[..8]);
        write!(f, "PeerId({prefix}{})", hex::encode(&self.0[8..]))
    }
}
