use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use reqwest::header::HeaderMap;
use tokio::time::Instant;

use crate::route::Route;

/// Per-bucket state; the async mutex is held for the whole request so concurrent
/// callers on one bucket cannot overshoot `remaining`.
#[derive(Default)]
pub(crate) struct Bucket {
    pub state: tokio::sync::Mutex<BucketState>,
}

#[derive(Default, Debug)]
pub(crate) struct BucketState {
    remaining: Option<u64>,
    reset_at: Option<Instant>,
}

impl BucketState {
    pub fn wait_time(&self, now: Instant) -> Option<Duration> {
        match (self.remaining, self.reset_at) {
            (Some(0), Some(reset)) if reset > now => Some(reset - now),
            _ => None,
        }
    }

    pub fn update(&mut self, headers: &HeaderMap, now: Instant) {
        if let Some(rem) = header_parse::<u64>(headers, "x-ratelimit-remaining") {
            self.remaining = Some(rem);
        }
        if let Some(after) = header_parse::<f64>(headers, "x-ratelimit-reset-after") {
            self.reset_at = Some(now + secs(after));
        }
    }

    pub fn block_for(&mut self, retry_after: f64, now: Instant) {
        self.remaining = Some(0);
        self.reset_at = Some(now + secs(retry_after));
    }
}

#[derive(Default)]
struct Maps {
    by_route: HashMap<String, Arc<Bucket>>,
    by_hash: HashMap<String, Arc<Bucket>>,
}

#[derive(Default)]
pub(crate) struct RateLimiter {
    maps: Mutex<Maps>,
    global_until: Mutex<Option<Instant>>,
}

impl RateLimiter {
    pub fn bucket(&self, route: &Route) -> Arc<Bucket> {
        self.maps.lock().by_route.entry(route.key.clone()).or_default().clone()
    }

    /// Records the `X-RateLimit-Bucket` hash for a route so routes sharing a hash
    /// (and major parameter) share one bucket from now on.
    pub fn learn(&self, route: &Route, hash: &str, current: &Arc<Bucket>) {
        let mut maps = self.maps.lock();
        let key = format!("{hash}:{}", route.major);
        match maps.by_hash.get(&key).cloned() {
            Some(shared) if !Arc::ptr_eq(&shared, current) => {
                maps.by_route.insert(route.key.clone(), shared);
            }
            Some(_) => {}
            None => {
                maps.by_hash.insert(key, current.clone());
            }
        }
    }

    pub fn set_global(&self, retry_after: f64) {
        let until = Instant::now() + secs(retry_after);
        let mut g = self.global_until.lock();
        if g.is_none_or(|cur| cur < until) {
            *g = Some(until);
        }
    }

    pub async fn wait_global(&self) {
        loop {
            let until = *self.global_until.lock();
            match until {
                Some(t) if t > Instant::now() => tokio::time::sleep_until(t).await,
                _ => return,
            }
        }
    }
}

pub(crate) fn header_parse<T: std::str::FromStr>(headers: &HeaderMap, name: &str) -> Option<T> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}

pub(crate) fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name)?.to_str().ok()
}

pub(crate) fn secs(s: f64) -> Duration {
    Duration::try_from_secs_f64(s.max(0.0)).unwrap_or(Duration::from_secs(60)).min(Duration::from_secs(3600))
}
