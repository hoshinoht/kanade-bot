//! The process-wide Open API limiter: request starts at least
//! [`SPACING`] apart (4/s, under the key's 5/s) and one character's calls
//! never overlap. The daily budget is the store's (`calls`), so it survives
//! restarts. Waits run on tokio's monotonic clock.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};
use tokio::time::Instant;

pub const SPACING: Duration = Duration::from_millis(250);

pub struct Limiter {
    spacing: Duration,
    next: AsyncMutex<Option<Instant>>,
    characters: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new(SPACING)
    }
}

impl Limiter {
    pub fn new(spacing: Duration) -> Self {
        Self {
            spacing,
            next: AsyncMutex::new(None),
            characters: Mutex::default(),
        }
    }

    /// Exclusive use of `character_id` until the guard drops: one
    /// character's calls are in flight one at a time.
    pub async fn character(&self, character_id: &str) -> OwnedMutexGuard<()> {
        let lock = {
            let mut characters = self
                .characters
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            characters.retain(|_, lock| lock.strong_count() > 0);
            match characters.get(character_id).and_then(Weak::upgrade) {
                Some(lock) => lock,
                None => {
                    let lock = Arc::new(AsyncMutex::new(()));
                    characters.insert(character_id.to_owned(), Arc::downgrade(&lock));
                    lock
                }
            }
        };
        lock.lock_owned().await
    }

    /// Wait for the next request slot; returns once this request may start.
    pub async fn pace(&self) {
        let mut next = self.next.lock().await;
        let now = Instant::now();
        let start = match *next {
            Some(at) if at > now => {
                tokio::time::sleep_until(at).await;
                at
            }
            _ => now,
        };
        *next = Some(start + self.spacing);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;

    /// Under paused time: concurrent requests start at least 250 ms apart,
    /// and a character's second call waits for its first to finish.
    #[tokio::test(start_paused = true)]
    async fn requests_are_spaced_and_one_per_character() {
        let limiter = Arc::new(Limiter::default());
        let origin = Instant::now();
        let starts = Arc::new(Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for n in 0..5 {
            let (limiter, starts) = (Arc::clone(&limiter), Arc::clone(&starts));
            tasks.push(tokio::spawn(async move {
                limiter.pace().await;
                starts.lock().unwrap().push((n, Instant::now() - origin));
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        let mut times: Vec<Duration> = starts.lock().unwrap().iter().map(|(_, at)| *at).collect();
        times.sort();
        for pair in times.windows(2) {
            assert!(pair[1] - pair[0] >= SPACING, "{times:?}");
        }
        assert_eq!(times[4], SPACING * 4);

        let first = limiter.character("c1").await;
        let other = limiter.character("c2").await;
        let waiting = {
            let limiter = Arc::clone(&limiter);
            tokio::spawn(async move {
                let _guard = limiter.character("c1").await;
                Instant::now()
            })
        };
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(!waiting.is_finished(), "c1 is still in flight");
        let released = Instant::now();
        drop(first);
        assert!(waiting.await.unwrap() >= released);
        drop(other);
    }
}
