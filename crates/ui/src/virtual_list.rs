//! Variable-height list virtualization: measured-height cache, prefix offsets and
//! visible-range lookup. Only rows intersecting the viewport are laid out each frame.

use std::collections::HashMap;
use std::ops::Range;

#[derive(Debug, Default)]
pub struct HeightCache {
    width: f32,
    heights: HashMap<u64, (u64, f32)>,
}

impl HeightCache {
    /// Width changes re-wrap text, so every measurement is invalidated.
    pub fn set_width(&mut self, width: f32) {
        if (width - self.width).abs() > 0.5 {
            self.width = width;
            self.heights.clear();
        }
    }

    pub fn get(&self, key: u64, signature: u64) -> Option<f32> {
        self.heights.get(&key).filter(|(s, _)| *s == signature).map(|(_, h)| *h)
    }

    pub fn put(&mut self, key: u64, signature: u64, height: f32) {
        self.heights.insert(key, (signature, height));
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }

    /// Bound memory to rows that still exist.
    pub fn retain(&mut self, keep: impl Fn(u64) -> bool) {
        self.heights.retain(|k, _| keep(*k));
    }
}

/// Prefix sums: `offsets[i]` is the top of row i; `offsets[n]` the total height.
pub fn offsets(heights: impl IntoIterator<Item = f32>) -> Vec<f32> {
    let mut out = vec![0.0];
    let mut acc = 0.0;
    for h in heights {
        acc += h.max(0.0);
        out.push(acc);
    }
    out
}

/// Rows intersecting [top, bottom).
pub fn visible_range(offsets: &[f32], top: f32, bottom: f32) -> Range<usize> {
    let n = offsets.len().saturating_sub(1);
    if n == 0 || bottom <= top {
        return 0..0;
    }
    // First row whose bottom edge is below `top`.
    let start = offsets[1..].partition_point(|&end| end <= top);
    // First row whose top edge is at/after `bottom`.
    let end = offsets[..n].partition_point(|&start| start < bottom);
    start.min(n)..end.max(start).min(n)
}

/// Scroll offset that keeps `anchor_row` at the same on-screen position after rows above it changed.
pub fn anchored_offset(offsets: &[f32], anchor_row: usize, anchor_screen_delta: f32) -> f32 {
    offsets.get(anchor_row).copied().unwrap_or(0.0) - anchor_screen_delta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_offsets() {
        assert_eq!(offsets([10.0, 20.0, 5.0]), vec![0.0, 10.0, 30.0, 35.0]);
        assert_eq!(offsets([]), vec![0.0]);
    }

    #[test]
    fn visible_ranges() {
        let o = offsets([10.0, 20.0, 5.0, 40.0]); // 0,10,30,35,75
        assert_eq!(visible_range(&o, 0.0, 10.0), 0..1);
        assert_eq!(visible_range(&o, 9.0, 31.0), 0..3);
        assert_eq!(visible_range(&o, 30.0, 36.0), 2..4);
        assert_eq!(visible_range(&o, 74.0, 200.0), 3..4);
        assert_eq!(visible_range(&o, 80.0, 200.0), 4..4);
        assert_eq!(visible_range(&o, 5.0, 5.0), 0..0);
        assert_eq!(visible_range(&offsets([]), 0.0, 100.0), 0..0);
    }

    #[test]
    fn large_list_is_logarithmic_lookup() {
        let o = offsets((0..100_000).map(|i| 20.0 + (i % 7) as f32));
        let total = *o.last().unwrap();
        let r = visible_range(&o, total / 2.0, total / 2.0 + 800.0);
        assert!(r.len() < 50 && !r.is_empty());
    }

    #[test]
    fn anchoring_after_prepend() {
        // Row 0 at offset 0 was visible 15px below viewport top; 3 rows (60px) prepended.
        let o = offsets([20.0, 20.0, 20.0, 50.0]);
        assert_eq!(anchored_offset(&o, 3, -15.0), 75.0);
    }

    #[test]
    fn height_cache_signature_and_width() {
        let mut c = HeightCache::default();
        c.set_width(500.0);
        c.put(1, 7, 42.0);
        assert_eq!(c.get(1, 7), Some(42.0));
        assert_eq!(c.get(1, 8), None);
        c.set_width(500.2);
        assert_eq!(c.get(1, 7), Some(42.0));
        c.set_width(640.0);
        assert!(c.is_empty());
    }
}
