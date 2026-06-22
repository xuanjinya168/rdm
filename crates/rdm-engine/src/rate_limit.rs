//! 进程内的令牌桶速率限制器。所有下载工作线程共享，
//! 从 Python 的 `downloader.rate_limit` 模块异步化移植而来。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 单次等待最大的睡眠时长，超过后会再次检查中止信号；
/// 这样即使在严格的限速下，暂停 / 取消也能在约 50 ms 内解除阻塞。
const MAX_SLEEP: f64 = 0.05;

struct Bucket {
    /// 每秒允许的字节数；`0` 表示不限速。
    rate: u64,
    tokens: f64,
    last_refill: Instant,
}

/// 用于限制整体下载吞吐量的令牌桶。
pub struct RateLimiter {
    bucket: Mutex<Bucket>,
}

impl RateLimiter {
    /// 以 `bytes_per_second`（`0` = 不限速）创建一个限速器，
    /// 初始桶内令牌为满。
    pub fn new(bytes_per_second: u64) -> Self {
        Self {
            bucket: Mutex::new(Bucket {
                rate: bytes_per_second,
                tokens: bytes_per_second as f64,
                last_refill: Instant::now(),
            }),
        }
    }

    /// 当前限速值（字节 / 秒，`0` 表示不限速）。
    pub fn rate(&self) -> u64 {
        self.lock().rate
    }

    /// 修改限速值，确保桶内已有的令牌不会超过新的容量。
    pub fn set_rate(&self, bytes_per_second: u64) {
        let mut bucket = self.lock();
        bucket.rate = bytes_per_second;
        bucket.tokens = bucket.tokens.min(bytes_per_second as f64);
        bucket.last_refill = Instant::now();
    }

    /// 等待直到累积出 `amount` 个令牌，然后消费它们。
    ///
    /// 若在等待过程中 `abort` 返回 `true`，则返回 `false`，使调用者
    /// 在严格限速下也能及时响应暂停 / 取消。不限速的限速器
    /// （或请求量为 0）会立即返回，且不会修改桶的状态。
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
                // 桶可以短暂持有单个超大请求的令牌数，因此大于一秒速率的块
                // 仍可排空，而非死锁。
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
        // 以 1 MiB/s 的速度传输 256 KiB 需要约 0.25 秒。
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
        // 令牌已被限制到新的速率，因此大请求现在必须等待。
        let started = Instant::now();
        assert!(limiter.acquire(4096, || false).await);
        assert!(started.elapsed() >= Duration::from_millis(150));
    }
}
