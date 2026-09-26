use std::fmt;

use ring::rand::{SecureRandom, SystemRandom};

use crate::infrastructure::llm::governor::Random;

/// OS randomness for production pseudonym draws (tests inject a seeded
/// `XorShift`).
pub struct SystemRng(SystemRandom);

impl SystemRng {
    pub fn new() -> Self {
        Self(SystemRandom::new())
    }
}

impl Default for SystemRng {
    fn default() -> Self {
        Self::new()
    }
}

impl Random for SystemRng {
    fn next_u64(&self) -> u64 {
        let mut bytes = [0u8; 8];
        // The OS source only fails when the platform has none at all.
        self.0
            .fill(&mut bytes)
            .expect("operating system randomness is available");
        u64::from_le_bytes(bytes)
    }
}

impl fmt::Debug for SystemRng {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SystemRng")
    }
}
