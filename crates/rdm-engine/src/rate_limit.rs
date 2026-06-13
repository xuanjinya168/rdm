//! Process-local token-bucket rate limiter. Async port of the Python
//! `downloader.rate_limit` module, shared by all download workers.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Longest a single wait sleeps before re-checking the abort signal, so a
/// paused or canceled download unblocks within ~50 ms even under a tight limit.
const MAX_SLEEP: f64 = 0.05;

struct Bucket {
    /// Bytes per second; `0` means unlimited.
    rate: u64,
    tokens: f64,
    last_refill: Instant,
}

/// A token bucket capping aggregate download throughput.
pub struct RateLimiter {
    bucket: Mutex<Bucket>,
}

impl RateLimiter {
    /// Create a limiter at `bytes_per_second` (`0` = unlimited), starting with
    /// a full bucket.
    pub fn new(bytes_per_second: u64) -> Self {
        Self {
            bucket: Mutex::new(Bucket {
                rate: bytes_per_second,
                tokens: bytes_per_second as f64,
                last_refill: Instant::now(),
            }),
        }
    }

    /// The current limit in bytes per second (`0` = unlimited).
    pub fn rate(&self) -> u64 {
        self.lock().rate
    }

    /// Change the limit, never letting the bucket exceed the new capacity.
    pub fn set_rate(&self, bytes_per_second: u64) {
        let mut bucket = self.lock();
        bucket.rate = bytes_per_second;
        bucket.tokens = bucket.tokens.min(bytes_per_second as f64);
        bucket.last_refill = Instant::now();
    }

    /// Wait until `amount` tokens are available, then consume them.
    ///
    /// Returns `false` if `abort` reported `true` while waiting, so callers stay
    /// responsive to pause/cancel even under a tight limit. An unlimited limiter
    /// (or a zero request) returns immediately without touching the bucket.
    pub async fn acquire<F: Fn() -> bool>(&self, amount: u64, abort: F) -> bool {
        if amount == 0 || self.rate() == 0 {
            return true;
        }
        loop {
            let wait_for = {
                let mut bucket = self.lock();
                if bucket.rate == 0 {
                    return true;
                }
                let now = Instant::now();
                let elapsed = now.duration_since(bucket.last_refill).as_secs_f64();
                // The bucket can briefly hold a single oversized request's worth
                // of tokens so a chunk larger than one second of rate still
                // drains rather than deadlocking.
                let capacity = bucket.rate.max(amount) as f64;
                bucket.tokens = (bucket.tokens + elapsed * bucket.rate as f64).min(capacity);
                bucket.last_refill = now;
                if bucket.tokens >= amount as f64 {
                    bucket.tokens -= amount as f64;
                    return true;
                }
                (amount as f64 - bucket.tokens) / bucket.rate as f64
            };
            if abort() {
                return false;
            }
            tokio::time::sleep(Duration::from_secs_f64(wait_for.min(MAX_SLEEP))).await;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Bucket> {
        self.bucket
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlimited_acquire_is_immediate() {
        let limiter = RateLimiter::new(0);
        let started = Instant::now();
        assert!(limiter.acquire(10 * 1024 * 1024, || false).await);
        assert!(started.elapsed() < Duration::from_millis(50));
    }

    #[tokio::test]
    async fn acquire_paces_to_rate() {
        let limiter = RateLimiter::new(1024 * 1024);
        assert!(limiter.acquire(1024 * 1024, || false).await); // drain the initial bucket
        let started = Instant::now();
        assert!(limiter.acquire(256 * 1024, || false).await);
        // 256 KiB at 1 MiB/s needs ~0.25 s.
        assert!(started.elapsed() >= Duration::from_millis(150));
    }

    #[tokio::test]
    async fn abort_unblocks_waiter_quickly() {
        let limiter = RateLimiter::new(1024);
        assert!(limiter.acquire(1024, || false).await); // drain the bucket
        let started = Instant::now();
        assert!(!limiter.acquire(512 * 1024, || true).await);
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[tokio::test]
    async fn set_rate_caps_existing_tokens() {
        let limiter = RateLimiter::new(10 * 1024 * 1024);
        limiter.set_rate(1024);
        assert_eq!(limiter.rate(), 1024);
        // Tokens were capped to the new rate, so a large request must now wait.
        let started = Instant::now();
        assert!(limiter.acquire(4096, || false).await);
        assert!(started.elapsed() >= Duration::from_millis(150));
    }
}
