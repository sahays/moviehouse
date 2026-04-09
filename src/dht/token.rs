use std::net::IpAddr;
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};

/// Token manager for DHT announce_peer verification.
/// Generates tokens bound to an IP address; verifies against current and previous secret.
pub struct TokenManager {
    current_secret: [u8; 16],
    previous_secret: [u8; 16],
    last_rotation: Instant,
    rotation_interval: Duration,
}

impl TokenManager {
    pub fn new() -> Self {
        let mut current = [0u8; 16];
        let mut previous = [0u8; 16];
        rand::Rng::fill(&mut rand::thread_rng(), &mut current);
        rand::Rng::fill(&mut rand::thread_rng(), &mut previous);
        Self {
            current_secret: current,
            previous_secret: previous,
            last_rotation: Instant::now(),
            rotation_interval: Duration::from_secs(5 * 60),
        }
    }

    /// Generate a token for a given IP address.
    pub fn generate(&self, addr: &IpAddr) -> Vec<u8> {
        let mut hasher = Sha1::new();
        match addr {
            IpAddr::V4(ip) => hasher.update(ip.octets()),
            IpAddr::V6(ip) => hasher.update(ip.octets()),
        }
        hasher.update(self.current_secret);
        hasher.finalize().to_vec()
    }

    /// Verify a token from a given IP address (checks both current and previous secret).
    pub fn verify(&self, addr: &IpAddr, token: &[u8]) -> bool {
        // Check current secret
        let expected_current = self.generate_with_secret(addr, &self.current_secret);
        if token == expected_current.as_slice() {
            return true;
        }
        // Check previous secret
        let expected_previous = self.generate_with_secret(addr, &self.previous_secret);
        token == expected_previous.as_slice()
    }

    /// Rotate secrets if enough time has passed.
    pub fn maybe_rotate(&mut self) {
        if self.last_rotation.elapsed() > self.rotation_interval {
            self.previous_secret = self.current_secret;
            rand::Rng::fill(&mut rand::thread_rng(), &mut self.current_secret);
            self.last_rotation = Instant::now();
        }
    }

    fn generate_with_secret(&self, addr: &IpAddr, secret: &[u8; 16]) -> Vec<u8> {
        let mut hasher = Sha1::new();
        match addr {
            IpAddr::V4(ip) => hasher.update(ip.octets()),
            IpAddr::V6(ip) => hasher.update(ip.octets()),
        }
        hasher.update(secret);
        hasher.finalize().to_vec()
    }
}
