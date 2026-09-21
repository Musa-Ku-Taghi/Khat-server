use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;
use tokio::time::interval;

pub struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_rate: f64,
}

impl TokenBucket {
    pub fn new(capacity: u32, refill_rate_per_sec: f64) -> Self {
        Self {
            tokens: capacity as f64,
            last_refill: Instant::now(),
            capacity: capacity as f64,
            refill_rate: refill_rate_per_sec,
        }
    }

    pub fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_rate).min(self.capacity);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub struct RateLimiter {
    map: Arc<Mutex<HashMap<String, TokenBucket>>>,
    capacity: u32,
    refill_rate_per_sec: f64,
    enabled: bool,
}

impl RateLimiter {
    pub fn new(enabled: bool, requests_per_minute: u32) -> Self {
        let refill_rate = requests_per_minute as f64 / 60.0;
        Self {
            map: Arc::new(Mutex::new(HashMap::new())),
            capacity: requests_per_minute,
            refill_rate_per_sec: refill_rate,
            enabled,
        }
    }

    pub async fn check_and_consume(&self, key: &str) -> bool {
        if !self.enabled {
            return true;
        }
        let mut map = self.map.lock().await;
        let bucket = map
            .entry(key.to_string())
            .or_insert_with(|| TokenBucket::new(self.capacity, self.refill_rate_per_sec));
        bucket.try_consume()
    }

    pub async fn cleanup(&self, idle_timeout: Duration) {
        let mut map = self.map.lock().await;
        let now = Instant::now();
        map.retain(|_key, bucket| {
            bucket.tokens > 0.0 || now.duration_since(bucket.last_refill) < idle_timeout
        });
    }
}

pub async fn rate_limiter_cleanup_task(limiter: Arc<RateLimiter>, interval_secs: u64) {
    let mut ticker = interval(Duration::from_secs(interval_secs));
    let idle_timeout = Duration::from_secs(interval_secs * 2);
    loop {
        ticker.tick().await;
        limiter.cleanup(idle_timeout).await;
    }
}
