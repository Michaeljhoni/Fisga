//! Speed limit shared by every connection of every download (IDM's "Speed
//! Limiter"): a token bucket whose rate can change while downloads run.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct RateLimiter {
    /// Bytes per second; 0 = unlimited.
    rate: AtomicU64,
    /// Last refill and the bytes that may still be read (negative = in debt).
    bucket: Mutex<(Instant, f64)>,
}

impl RateLimiter {
    pub fn new(bytes_per_second: u64) -> Self {
        Self { rate: AtomicU64::new(bytes_per_second), bucket: Mutex::new((Instant::now(), 0.0)) }
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    pub fn set_rate(&self, bytes_per_second: u64) {
        self.rate.store(bytes_per_second, Ordering::Relaxed);
        *self.bucket.lock().unwrap() = (Instant::now(), 0.0);
    }

    /// How long a reader that just got `bytes` must wait before reading more.
    fn reserve(&self, bytes: usize) -> Duration {
        let rate = self.rate() as f64;
        if rate <= 0.0 {
            return Duration::ZERO;
        }
        let mut bucket = self.bucket.lock().unwrap();
        let now = Instant::now();
        // At most one second of burst accumulates while idle.
        let refilled = bucket.1 + now.duration_since(bucket.0).as_secs_f64() * rate;
        bucket.0 = now;
        bucket.1 = refilled.min(rate) - bytes as f64;
        if bucket.1 >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-bucket.1 / rate)
        }
    }

    /// Accounts for `bytes` just received, sleeping as needed to keep the rate.
    pub async fn consume(&self, bytes: usize) {
        let wait = self.reserve(bytes);
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits() {
        let limiter = RateLimiter::new(0);
        assert_eq!(limiter.reserve(10_000_000), Duration::ZERO);
    }

    #[test]
    fn debt_turns_into_waiting_time() {
        let limiter = RateLimiter::new(1000);
        // Fresh bucket: 2000 bytes at 1000 B/s means about 2 s of waiting.
        let wait = limiter.reserve(2000);
        assert!(wait > Duration::from_millis(1900) && wait <= Duration::from_secs(2), "{wait:?}");
        // The debt accumulates for the next reader.
        assert!(limiter.reserve(1000) > wait);
        // Changing the rate resets the bucket.
        limiter.set_rate(0);
        assert_eq!(limiter.reserve(1_000_000), Duration::ZERO);
    }

    #[tokio::test]
    async fn throughput_matches_the_rate() {
        let limiter = RateLimiter::new(1_000_000);
        let start = Instant::now();
        for _ in 0..50 {
            limiter.consume(10_000).await; // 500 kB in total
        }
        let elapsed = start.elapsed().as_secs_f64();
        assert!((0.45..=0.8).contains(&elapsed), "500 kB at 1 MB/s took {elapsed} s");
    }
}
