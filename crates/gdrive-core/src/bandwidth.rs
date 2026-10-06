//! Global transfer-rate limits and speed metering.
//!
//! One [`Throttle`] per direction is shared by every concurrent transfer, so a limit
//! caps the *total* rate, like the official client. Limits can be changed while
//! transfers run; they take effect on the next chunk.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;

/// How much unused allowance may accumulate, as time at the configured rate.
const BURST: Duration = Duration::from_millis(250);
/// Window over which speeds are averaged.
const METER_WINDOW: Duration = Duration::from_secs(3);

/// Converts a limit in MB/s (0 or less = unlimited) to bytes per second.
pub fn mb_per_sec_to_bytes(mb: f64) -> u64 {
    if mb.is_finite() && mb > 0.0 {
        (mb * 1_000_000.0).round().max(1.0) as u64
    } else {
        0
    }
}

struct Bucket {
    /// Available bytes; negative when callers have borrowed ahead of the rate.
    tokens: f64,
    last: Instant,
}

/// A token bucket shared by all transfers in one direction, plus a byte counter.
pub struct Throttle {
    /// Bytes per second; 0 means unlimited.
    rate: AtomicU64,
    bucket: Mutex<Bucket>,
    total: AtomicU64,
}

impl Throttle {
    fn new() -> Self {
        Self {
            rate: AtomicU64::new(0),
            bucket: Mutex::new(Bucket { tokens: 0.0, last: Instant::now() }),
            total: AtomicU64::new(0),
        }
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Bytes moved in this direction since startup.
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }

    /// Accounts for `n` bytes, sleeping as long as needed to respect the limit.
    pub async fn consume(&self, n: usize) {
        self.wait_for(n).await;
        // Counted once released, so the speed reflects what actually got through.
        self.total.fetch_add(n as u64, Ordering::Relaxed);
    }

    async fn wait_for(&self, n: usize) {
        let rate = self.rate();
        if rate == 0 {
            return;
        }
        // Each caller takes its bytes immediately (possibly going into debt) and then
        // sleeps off its share of the debt, so concurrent transfers split the rate.
        let wait = {
            let mut b = self.bucket.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let burst = rate as f64 * BURST.as_secs_f64();
            b.tokens = (b.tokens + now.duration_since(b.last).as_secs_f64() * rate as f64).min(burst);
            b.last = now;
            b.tokens -= n as f64;
            if b.tokens < 0.0 {
                Duration::from_secs_f64(-b.tokens / rate as f64)
            } else {
                Duration::ZERO
            }
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }
}

/// Download and upload throttles shared by every transfer of a client.
pub struct Bandwidth {
    pub download: Throttle,
    pub upload: Throttle,
}

impl Default for Bandwidth {
    fn default() -> Self {
        Self { download: Throttle::new(), upload: Throttle::new() }
    }
}

impl Bandwidth {
    pub fn set_limits(&self, download_mb_per_sec: f64, upload_mb_per_sec: f64) {
        self.download.set_rate(mb_per_sec_to_bytes(download_mb_per_sec));
        self.upload.set_rate(mb_per_sec_to_bytes(upload_mb_per_sec));
    }
}

/// Turns an ever-growing byte counter into a smoothed bytes-per-second figure.
#[derive(Default)]
pub struct SpeedMeter {
    samples: VecDeque<(Instant, u64)>,
}

impl SpeedMeter {
    /// Records the counter's current value and returns the average rate over the window.
    pub fn sample(&mut self, total: u64) -> u64 {
        let now = Instant::now();
        self.samples.push_back((now, total));
        while self.samples.len() > 2 && now.duration_since(self.samples[0].0) > METER_WINDOW {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        // Average over at least a second so a single large chunk doesn't read as a spike.
        let dt = now.duration_since(t0).as_secs_f64().max(1.0);
        (total.saturating_sub(b0) as f64 / dt) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_megabytes() {
        assert_eq!(mb_per_sec_to_bytes(0.0), 0);
        assert_eq!(mb_per_sec_to_bytes(-1.0), 0);
        assert_eq!(mb_per_sec_to_bytes(2.5), 2_500_000);
        assert_eq!(mb_per_sec_to_bytes(f64::NAN), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_caps_total_rate_across_concurrent_callers() {
        let bw = std::sync::Arc::new(Bandwidth::default());
        bw.set_limits(1.0, 0.0); // 1 MB/s down, unlimited up
        let start = Instant::now();
        let tasks: Vec<_> = (0..4)
            .map(|_| {
                let bw = bw.clone();
                tokio::spawn(async move {
                    for _ in 0..16 {
                        bw.download.consume(64 * 1024).await;
                    }
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        // 4 × 16 × 64 KiB = 4 MiB ≈ 4.19 MB at 1 MB/s, minus the initial burst allowance.
        let secs = start.elapsed().as_secs_f64();
        assert!((3.9..4.4).contains(&secs), "took {secs}s");
        assert_eq!(bw.download.total(), 4 * 16 * 64 * 1024);

        let start = Instant::now();
        bw.upload.consume(100_000_000).await;
        assert!(start.elapsed() < Duration::from_millis(1), "unlimited direction must not wait");
    }

    #[tokio::test(start_paused = true)]
    async fn meter_averages_over_window() {
        let mut m = SpeedMeter::default();
        assert_eq!(m.sample(0), 0);
        for i in 1..=10 {
            tokio::time::advance(Duration::from_millis(500)).await;
            m.sample(i * 500_000); // 1 MB/s
        }
        let rate = m.sample(5_000_000);
        assert!((950_000..=1_050_000).contains(&rate), "rate {rate}");
    }
}
