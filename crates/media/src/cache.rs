use std::hash::Hash;

use lru::LruCache;

/// LRU cache bounded by the total byte size of its values rather than entry count.
/// Used by the UI to cap GPU texture memory.
pub struct BoundedCache<K, V> {
    entries: LruCache<K, (V, usize)>,
    budget: usize,
    bytes: usize,
}

impl<K: Hash + Eq + Clone, V> BoundedCache<K, V> {
    pub fn new(budget_bytes: usize) -> Self {
        Self { entries: LruCache::unbounded(), budget: budget_bytes, bytes: 0 }
    }

    /// Looks up `k` and marks it most recently used.
    pub fn get(&mut self, k: &K) -> Option<&V> {
        self.entries.get(k).map(|(v, _)| v)
    }

    pub fn peek(&self, k: &K) -> Option<&V> {
        self.entries.peek(k).map(|(v, _)| v)
    }

    /// Inserts `v` as most recently used and returns the entries evicted to stay within the
    /// budget. The entry just inserted is never evicted, even if it alone exceeds the budget;
    /// otherwise the caller would reload it on every frame. A replaced value for the same key
    /// is dropped, not reported as evicted.
    pub fn insert(&mut self, k: K, v: V, bytes: usize) -> Vec<(K, V)> {
        if let Some((_, old)) = self.entries.put(k, (v, bytes)) {
            self.bytes -= old;
        }
        self.bytes += bytes;
        self.evict(1)
    }

    pub fn remove(&mut self, k: &K) -> Option<V> {
        let (v, bytes) = self.entries.pop(k)?;
        self.bytes -= bytes;
        Some(v)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Changes the budget and returns whatever had to be evicted to fit the new one.
    pub fn set_budget(&mut self, b: usize) -> Vec<(K, V)> {
        self.budget = b;
        self.evict(0)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }

    fn evict(&mut self, keep_at_least: usize) -> Vec<(K, V)> {
        let mut evicted = Vec::new();
        while self.bytes > self.budget && self.entries.len() > keep_at_least {
            let Some((k, (v, bytes))) = self.entries.pop_lru() else { break };
            self.bytes -= bytes;
            evicted.push((k, v));
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(evicted: &[(&'static str, u32)]) -> Vec<&'static str> {
        evicted.iter().map(|(k, _)| *k).collect()
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let mut c = BoundedCache::new(30);
        assert!(c.insert("a", 1, 10).is_empty());
        assert!(c.insert("b", 2, 10).is_empty());
        assert!(c.insert("c", 3, 10).is_empty());
        assert_eq!(c.get(&"a"), Some(&1));
        let evicted = c.insert("d", 4, 10);
        assert_eq!(keys(&evicted), ["b"]);
        assert_eq!(c.len(), 3);
        assert_eq!(c.bytes(), 30);
        assert!(c.peek(&"b").is_none());
    }

    #[test]
    fn peek_does_not_touch_recency() {
        let mut c = BoundedCache::new(20);
        c.insert("a", 1, 10);
        c.insert("b", 2, 10);
        assert_eq!(c.peek(&"a"), Some(&1));
        let evicted = c.insert("c", 3, 10);
        assert_eq!(keys(&evicted), ["a"]);
    }

    #[test]
    fn insert_can_evict_several_entries() {
        let mut c = BoundedCache::new(30);
        c.insert("a", 1, 10);
        c.insert("b", 2, 10);
        c.insert("c", 3, 10);
        let evicted = c.insert("big", 4, 25);
        assert_eq!(keys(&evicted), ["a", "b", "c"]);
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 25);
    }

    #[test]
    fn oversized_entry_is_kept_alone() {
        let mut c = BoundedCache::new(10);
        c.insert("a", 1, 5);
        let evicted = c.insert("huge", 2, 100);
        assert_eq!(keys(&evicted), ["a"]);
        assert_eq!(c.peek(&"huge"), Some(&2));
        assert_eq!(c.bytes(), 100);
    }

    #[test]
    fn reinserting_a_key_replaces_bytes_without_reporting_eviction() {
        let mut c = BoundedCache::new(100);
        c.insert("a", 1, 40);
        let evicted = c.insert("a", 2, 10);
        assert!(evicted.is_empty());
        assert_eq!(c.len(), 1);
        assert_eq!(c.bytes(), 10);
        assert_eq!(c.peek(&"a"), Some(&2));
    }

    #[test]
    fn remove_and_clear_update_accounting() {
        let mut c = BoundedCache::new(100);
        c.insert("a", 1, 10);
        c.insert("b", 2, 20);
        assert_eq!(c.remove(&"a"), Some(1));
        assert_eq!(c.remove(&"a"), None);
        assert_eq!(c.bytes(), 20);
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.bytes(), 0);
    }

    #[test]
    fn shrinking_budget_evicts_oldest_and_can_empty_the_cache() {
        let mut c = BoundedCache::new(100);
        c.insert("a", 1, 30);
        c.insert("b", 2, 30);
        c.insert("c", 3, 30);
        c.get(&"a");
        let evicted = c.set_budget(60);
        assert_eq!(keys(&evicted), ["b"]);
        assert_eq!(c.bytes(), 60);
        let evicted = c.set_budget(0);
        assert_eq!(keys(&evicted), ["c", "a"]);
        assert!(c.is_empty());
        assert!(c.set_budget(500).is_empty());
    }
}
