use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

/// Sliding-window limiter for client-sent gateway payloads (Discord: 120 per 60 s
/// per connection). Heartbeats bypass it; their share is reserved by lowering the
/// capacity available to everything else.
pub(crate) struct SendLimiter {
    window: Duration,
    limit: usize,
    capacity: usize,
    sent: VecDeque<Instant>,
}

impl SendLimiter {
    pub fn new(window: Duration, limit: u32) -> Self {
        let limit = limit.max(2) as usize;
        SendLimiter { window, limit, capacity: limit - 1, sent: VecDeque::new() }
    }

    /// Reserves enough slots for the heartbeats of one window at `interval`.
    pub fn reserve_for_heartbeats(&mut self, interval: Duration) {
        let per_window = if interval.is_zero() { u64::MAX } else { self.window.as_millis().div_ceil(interval.as_millis().max(1)) as u64 };
        let reserve = (per_window.saturating_add(1)).min(self.limit as u64 / 2) as usize;
        self.capacity = self.limit - reserve.max(1);
    }

    fn prune(&mut self, now: Instant) {
        while self.sent.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
            self.sent.pop_front();
        }
    }

    /// Records a send that must not be delayed (identify/resume).
    pub fn record(&mut self, now: Instant) {
        self.prune(now);
        self.sent.push_back(now);
    }

    pub fn try_acquire(&mut self, now: Instant) -> bool {
        self.prune(now);
        if self.sent.len() < self.capacity {
            self.sent.push_back(now);
            true
        } else {
            false
        }
    }

    pub fn ready_at(&mut self, now: Instant) -> Instant {
        self.prune(now);
        if self.sent.len() < self.capacity {
            return now;
        }
        let idx = self.sent.len() - self.capacity;
        self.sent[idx] + self.window
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_recovers() {
        let mut l = SendLimiter::new(Duration::from_secs(60), 120);
        l.reserve_for_heartbeats(Duration::from_millis(41_250));
        let t0 = Instant::now();
        let mut n = 0;
        while l.try_acquire(t0) {
            n += 1;
        }
        assert_eq!(n, 117);
        assert_eq!(l.ready_at(t0), t0 + Duration::from_secs(60));
        assert!(l.try_acquire(t0 + Duration::from_secs(60)));
    }
}
