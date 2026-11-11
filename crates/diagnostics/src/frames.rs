use std::time::Duration;

/// Rolling window of recent frame times for the performance overlay.
#[derive(Debug, Clone)]
pub struct FrameStats {
    samples_ms: Vec<f32>,
    capacity: usize,
    next: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameSummary {
    pub avg_ms: f32,
    /// 95th percentile (nearest rank) of the frame times in the window.
    pub p95_ms: f32,
    pub p99_ms: f32,
    pub max_ms: f32,
    /// `1000 / avg_ms`.
    pub fps: f32,
}

impl FrameStats {
    /// Keeps the most recent `capacity` frames (at least one).
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self { samples_ms: Vec::with_capacity(capacity), capacity, next: 0 }
    }

    pub fn record(&mut self, frame_time: Duration) {
        let ms = (frame_time.as_secs_f64() * 1000.0) as f32;
        if self.samples_ms.len() < self.capacity {
            self.samples_ms.push(ms);
        } else {
            self.samples_ms[self.next] = ms;
            self.next = (self.next + 1) % self.capacity;
        }
    }

    pub fn len(&self) -> usize {
        self.samples_ms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples_ms.is_empty()
    }

    pub fn clear(&mut self) {
        self.samples_ms.clear();
        self.next = 0;
    }

    pub fn summary(&self) -> FrameSummary {
        if self.samples_ms.is_empty() {
            return FrameSummary::default();
        }
        let mut sorted = self.samples_ms.clone();
        sorted.sort_by(f32::total_cmp);
        let n = sorted.len();
        let avg = sorted.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
        let nearest_rank = |p: f64| sorted[((p * n as f64).ceil() as usize).clamp(1, n) - 1];
        FrameSummary {
            avg_ms: avg as f32,
            p95_ms: nearest_rank(0.95),
            p99_ms: nearest_rank(0.99),
            max_ms: sorted[n - 1],
            fps: if avg > 0.0 { (1000.0 / avg) as f32 } else { 0.0 },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn empty_summary_is_zero() {
        let s = FrameStats::new(10);
        assert!(s.is_empty());
        assert_eq!(s.summary(), FrameSummary::default());
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let mut s = FrameStats::new(200);
        for v in (1..=100).rev() {
            s.record(ms(v));
        }
        let sum = s.summary();
        assert_eq!(s.len(), 100);
        assert!((sum.avg_ms - 50.5).abs() < 1e-3);
        assert_eq!(sum.p95_ms, 95.0);
        assert_eq!(sum.p99_ms, 99.0);
        assert_eq!(sum.max_ms, 100.0);
        assert!((sum.fps - 1000.0 / 50.5).abs() < 1e-2);
    }

    #[test]
    fn spike_shows_in_tail_but_not_average_much() {
        let mut s = FrameStats::new(100);
        for _ in 0..99 {
            s.record(Duration::from_micros(16_667));
        }
        s.record(ms(120));
        let sum = s.summary();
        assert!((sum.p95_ms - 16.667).abs() < 1e-2);
        assert!((sum.p99_ms - 16.667).abs() < 1e-2);
        assert_eq!(sum.max_ms, 120.0);
        assert!(sum.avg_ms < 18.0);
    }

    #[test]
    fn window_keeps_only_the_latest_frames() {
        let mut s = FrameStats::new(10);
        for _ in 0..50 {
            s.record(ms(100));
        }
        for _ in 0..10 {
            s.record(ms(10));
        }
        let sum = s.summary();
        assert_eq!(s.len(), 10);
        assert_eq!((sum.avg_ms, sum.max_ms), (10.0, 10.0));
        assert!((sum.fps - 100.0).abs() < 1e-3);

        s.clear();
        assert!(s.is_empty());
        s.record(ms(5));
        assert_eq!(s.summary().max_ms, 5.0);
    }

    #[test]
    fn zero_capacity_is_treated_as_one() {
        let mut s = FrameStats::new(0);
        s.record(ms(4));
        s.record(ms(8));
        assert_eq!((s.len(), s.summary().max_ms), (1, 8.0));
    }
}
