use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

/// Injected randomness so jitter is reproducible under test.
pub trait Random: Send + Sync {
    fn next_u64(&self) -> u64;
}

/// xorshift64* — adequate for jitter, not for secrets.
#[derive(Debug)]
pub struct XorShift(AtomicU64);

impl XorShift {
    pub fn new(seed: u64) -> Self {
        Self(AtomicU64::new(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        }))
    }
}

impl Random for XorShift {
    fn next_u64(&self) -> u64 {
        let mut current = self.0.load(Ordering::Relaxed);
        loop {
            let mut next = current;
            next ^= next >> 12;
            next ^= next << 25;
            next ^= next >> 27;
            match self
                .0
                .compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return next.wrapping_mul(0x2545_F491_4F6C_DD1D),
                Err(seen) => current = seen,
            }
        }
    }
}

/// Uniform in [0, 1).
pub(super) fn unit(random: &dyn Random) -> f64 {
    (random.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

/// Full jitter: uniform in [0, `cap`).
pub(in crate::infrastructure::llm) fn full(random: &dyn Random, cap: Duration) -> Duration {
    cap.mul_f64(unit(random))
}
