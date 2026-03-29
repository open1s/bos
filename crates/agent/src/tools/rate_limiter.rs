use std::time::{Duration, Instant};

pub struct RateLimiterPolicy {
    limit: usize,
    window: Duration,
    window_start: Instant,
    count: usize,
}

impl RateLimiterPolicy {
    pub fn new(limit: usize, window: Duration) -> Self {
        RateLimiterPolicy {
            limit,
            window,
            window_start: Instant::now(),
            count: 0,
        }
    }

    pub fn try_acquire(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.window_start) > self.window {
            self.window_start = now;
            self.count = 0;
        }
        if self.count < self.limit {
            self.count += 1;
            true
        } else {
            false
        }
    }
}
