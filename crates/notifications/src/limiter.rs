use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Allows at most `max` notifications per sliding `window`; anything beyond that is held
/// and later reported as a single count. Time is passed in so the logic is testable.
#[derive(Debug)]
pub(crate) struct RateLimiter {
    max: usize,
    window: Duration,
    shown: VecDeque<Instant>,
    held: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Show,
    Hold,
}

impl RateLimiter {
    pub(crate) fn new(max: usize, window: Duration) -> Self {
        Self { max: max.max(1), window, shown: VecDeque::new(), held: 0 }
    }

    /// Decides whether a notification arriving at `now` may be shown immediately. Once
    /// anything is held, later arrivals are held too so ordering is preserved.
    pub(crate) fn admit(&mut self, now: Instant) -> Verdict {
        self.expire(now);
        if self.held == 0 && self.shown.len() < self.max {
            self.shown.push_back(now);
            Verdict::Show
        } else {
            self.held += 1;
            Verdict::Hold
        }
    }

    /// When capacity has freed up, claims a slot for a summary and returns how many
    /// notifications it stands for.
    pub(crate) fn take_held(&mut self, now: Instant) -> Option<usize> {
        self.expire(now);
        if self.held > 0 && self.shown.len() < self.max {
            self.shown.push_back(now);
            Some(std::mem::take(&mut self.held))
        } else {
            None
        }
    }

    /// How long until a slot frees up for held notifications; `None` when nothing is held.
    pub(crate) fn next_wakeup(&self, now: Instant) -> Option<Duration> {
        if self.held == 0 {
            return None;
        }
        self.shown.front().map(|oldest| (*oldest + self.window).saturating_duration_since(now))
    }

    fn expire(&mut self, now: Instant) {
        while self.shown.front().is_some_and(|t| now.saturating_duration_since(*t) >= self.window) {
            self.shown.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Duration = Duration::from_secs(10);

    fn at(base: Instant, secs: u64) -> Instant {
        base + Duration::from_secs(secs)
    }

    #[test]
    fn shows_up_to_the_limit_then_holds() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(5, WINDOW);
        for i in 0..5 {
            assert_eq!(l.admit(at(t0, i)), Verdict::Show, "notification {i}");
        }
        assert_eq!(l.admit(at(t0, 5)), Verdict::Hold);
        assert_eq!(l.admit(at(t0, 6)), Verdict::Hold);
        assert_eq!(l.take_held(at(t0, 6)), None);
    }

    #[test]
    fn held_notifications_coalesce_once_the_window_slides() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(5, WINDOW);
        for i in 0..5 {
            l.admit(at(t0, i));
        }
        for i in 5..8 {
            assert_eq!(l.admit(at(t0, i)), Verdict::Hold);
        }
        assert_eq!(l.next_wakeup(at(t0, 8)), Some(Duration::from_secs(2)));
        assert_eq!(l.take_held(at(t0, 9)), None);
        assert_eq!(l.take_held(at(t0, 10)), Some(3));
        assert_eq!(l.take_held(at(t0, 10)), None);
        assert_eq!(l.next_wakeup(at(t0, 10)), None);
    }

    #[test]
    fn summary_consumes_a_slot() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(2, WINDOW);
        l.admit(t0);
        l.admit(t0);
        l.admit(at(t0, 1));
        assert_eq!(l.take_held(at(t0, 10)), Some(1));
        // The summary took one of the two slots.
        assert_eq!(l.admit(at(t0, 10)), Verdict::Show);
        assert_eq!(l.admit(at(t0, 10)), Verdict::Hold);
    }

    #[test]
    fn arrivals_while_holding_stay_behind_the_held_group() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(1, WINDOW);
        assert_eq!(l.admit(t0), Verdict::Show);
        assert_eq!(l.admit(at(t0, 1)), Verdict::Hold);
        // The window has passed but the summary has not been taken yet.
        assert_eq!(l.admit(at(t0, 11)), Verdict::Hold);
        assert_eq!(l.take_held(at(t0, 11)), Some(2));
    }

    #[test]
    fn spaced_out_notifications_are_never_held() {
        let t0 = Instant::now();
        let mut l = RateLimiter::new(5, WINDOW);
        for i in 0..50 {
            assert_eq!(l.admit(at(t0, i * 3)), Verdict::Show);
        }
    }

    #[test]
    fn zero_max_is_clamped_to_one() {
        let mut l = RateLimiter::new(0, WINDOW);
        assert_eq!(l.admit(Instant::now()), Verdict::Show);
    }
}
