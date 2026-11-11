use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use url::Url;

/// Prune after this many writes (a power of two) even when the running total looks fine,
/// since the total is only an estimate (other processes, interrupted writes).
const PRUNE_EVERY_WRITES: u64 = 64;
/// Leftover temp files from a crashed write are removed once they are this old.
const STALE_TMP_AGE: Duration = Duration::from_secs(3600);
const TMP_SUFFIX: &str = ".tmp";

/// File name for `url`: hex SHA-256 of the URL without Discord's volatile signature
/// parameters, so a refreshed CDN link maps to the same cached file.
pub(crate) fn cache_key(url: &Url) -> String {
    let mut stable = url.clone();
    stable.set_fragment(None);
    strip_signature(&mut stable);
    let digest = Sha256::digest(stable.as_str().as_bytes());
    let mut key = String::with_capacity(64);
    for byte in digest {
        key.push_str(&format!("{byte:02x}"));
    }
    key
}

/// Discord signs CDN links with `ex`, `is` and `hm`; they always appear together, which
/// keeps unrelated URLs that merely use one of those names untouched.
fn strip_signature(url: &mut Url) {
    const SIGNATURE: [&str; 3] = ["ex", "is", "hm"];
    let has = |url: &Url, name: &str| url.query_pairs().any(|(k, _)| k == name);
    if !SIGNATURE.iter().all(|name| has(url, name)) {
        return;
    }
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !SIGNATURE.contains(&k.as_ref()))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        url.set_query(None);
    } else {
        url.query_pairs_mut().clear().extend_pairs(kept);
    }
}

/// Flat directory of cached originals, pruned oldest-mtime-first to stay under a byte budget.
pub(crate) struct DiskCache {
    dir: PathBuf,
    budget: u64,
    usage: AtomicU64,
    writes: AtomicU64,
    tmp_seq: AtomicU64,
    prune_lock: Mutex<()>,
}

impl DiskCache {
    pub(crate) fn open(dir: PathBuf, budget: u64) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            budget,
            usage: AtomicU64::new(0),
            writes: AtomicU64::new(0),
            tmp_seq: AtomicU64::new(0),
            prune_lock: Mutex::new(()),
        })
    }

    pub(crate) fn usage(&self) -> u64 {
        self.usage.load(Ordering::Relaxed)
    }

    /// Reads a cached file and bumps its mtime so pruning treats it as recently used.
    /// Files larger than `max_len` are treated as absent.
    pub(crate) fn read(&self, key: &str, max_len: u64) -> Option<Vec<u8>> {
        let path = self.dir.join(key);
        let mut file = OpenOptions::new().read(true).write(true).open(&path).or_else(|_| fs::File::open(&path)).ok()?;
        let len = file.metadata().ok()?.len();
        if len > max_len {
            return None;
        }
        let _ = file.set_modified(SystemTime::now());
        let mut buf = Vec::with_capacity(len as usize);
        file.read_to_end(&mut buf).ok()?;
        Some(buf)
    }

    /// Atomically stores `data` (temp file + rename) and prunes when due.
    pub(crate) fn write(&self, key: &str, data: &[u8]) -> io::Result<()> {
        let len = data.len() as u64;
        if len > self.budget {
            return Ok(());
        }
        let tmp = self.dir.join(format!("{key}.{}.{}{TMP_SUFFIX}", std::process::id(), self.tmp_seq.fetch_add(1, Ordering::Relaxed)));
        let dest = self.dir.join(key);
        let replaced = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        let result = (|| {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(data)?;
            file.flush()?;
            drop(file);
            fs::rename(&tmp, &dest)
        })();
        if let Err(e) = result {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        let added = len.saturating_sub(replaced);
        let usage = self.usage.fetch_add(added, Ordering::Relaxed) + added;
        let writes = self.writes.fetch_add(1, Ordering::Relaxed) + 1;
        if usage > self.budget || writes & (PRUNE_EVERY_WRITES - 1) == 0 {
            self.prune();
        }
        Ok(())
    }

    pub(crate) fn remove(&self, key: &str) {
        if let Ok(meta) = fs::metadata(self.dir.join(key))
            && fs::remove_file(self.dir.join(key)).is_ok()
        {
            // Only an estimate; the next prune recomputes it from the directory.
            self.usage.store(self.usage().saturating_sub(meta.len()), Ordering::Relaxed);
        }
    }

    /// Rescans the directory, deletes the oldest files until usage is at 90% of the budget
    /// (hysteresis avoids pruning on every write), and records the real total. Skipped when
    /// another prune is already running.
    pub(crate) fn prune(&self) {
        let Some(_guard) = self.prune_lock.try_lock() else { return };
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        let now = SystemTime::now();
        let mut files: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let path = entry.path();
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            if is_tmp(&path) {
                if now.duration_since(mtime).is_ok_and(|age| age > STALE_TMP_AGE) {
                    let _ = fs::remove_file(&path);
                }
                continue;
            }
            total += meta.len();
            files.push((mtime, meta.len(), path));
        }
        if total > self.budget {
            files.sort_by_key(|(mtime, _, _)| *mtime);
            let target = self.budget / 10 * 9;
            for (_, len, path) in files {
                if total <= target {
                    break;
                }
                if fs::remove_file(&path).is_ok() {
                    total -= len;
                }
            }
        }
        self.usage.store(total, Ordering::Relaxed);
    }
}

fn is_tmp(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn key_is_hex_sha256() {
        let key = cache_key(&url("https://example.com/a.png"));
        assert_eq!(key.len(), 64);
        assert!(key.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn signature_params_do_not_change_the_key() {
        let a = cache_key(&url("https://cdn.discordapp.com/attachments/1/2/x.png?ex=65a&is=65b&hm=abc&width=64"));
        let b = cache_key(&url("https://cdn.discordapp.com/attachments/1/2/x.png?ex=999&is=888&hm=def&width=64"));
        let plain = cache_key(&url("https://cdn.discordapp.com/attachments/1/2/x.png?width=64"));
        assert_eq!(a, b);
        assert_eq!(a, plain);
        assert_ne!(a, cache_key(&url("https://cdn.discordapp.com/attachments/1/2/x.png?width=128")));
    }

    #[test]
    fn signature_only_query_is_removed_entirely() {
        let signed = cache_key(&url("https://cdn.discordapp.com/a.png?ex=1&is=2&hm=3"));
        assert_eq!(signed, cache_key(&url("https://cdn.discordapp.com/a.png")));
    }

    #[test]
    fn lone_signature_like_params_are_kept() {
        assert_ne!(cache_key(&url("https://example.com/a?is=cat")), cache_key(&url("https://example.com/a?is=dog")));
    }

    #[test]
    fn fragment_is_ignored() {
        assert_eq!(cache_key(&url("https://example.com/a.png#x")), cache_key(&url("https://example.com/a.png")));
    }

    fn set_age(path: &Path, age: Duration) {
        let f = OpenOptions::new().write(true).open(path).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
    }

    #[test]
    fn write_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf(), 1000).unwrap();
        cache.write("k", b"hello").unwrap();
        assert_eq!(cache.read("k", 100).as_deref(), Some(&b"hello"[..]));
        assert_eq!(cache.read("k", 3), None);
        assert_eq!(cache.read("missing", 100), None);
        assert_eq!(cache.usage(), 5);
        assert!(fs::read_dir(dir.path()).unwrap().flatten().all(|e| !is_tmp(&e.path())));
    }

    #[test]
    fn prune_deletes_oldest_first_until_under_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf(), 1000).unwrap();
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            fs::write(dir.path().join(name), vec![0u8; 400]).unwrap();
            set_age(&dir.path().join(name), Duration::from_secs(1000 - i as u64 * 100));
        }
        cache.prune();
        assert!(!dir.path().join("old").exists());
        assert!(dir.path().join("mid").exists());
        assert!(dir.path().join("new").exists());
        assert_eq!(cache.usage(), 800);
    }

    #[test]
    fn read_refreshes_mtime_so_hits_survive_pruning() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf(), 1000).unwrap();
        for name in ["a", "b", "c"] {
            fs::write(dir.path().join(name), vec![0u8; 400]).unwrap();
        }
        set_age(&dir.path().join("a"), Duration::from_secs(3000));
        set_age(&dir.path().join("b"), Duration::from_secs(2000));
        set_age(&dir.path().join("c"), Duration::from_secs(1000));
        assert!(cache.read("a", 1000).is_some());
        cache.prune();
        assert!(dir.path().join("a").exists());
        assert!(!dir.path().join("b").exists());
    }

    #[test]
    fn write_prunes_when_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf(), 1000).unwrap();
        cache.write("first", &[1u8; 600]).unwrap();
        set_age(&dir.path().join("first"), Duration::from_secs(500));
        cache.write("second", &[2u8; 600]).unwrap();
        assert!(!dir.path().join("first").exists());
        assert!(dir.path().join("second").exists());
        assert_eq!(cache.usage(), 600);
    }

    #[test]
    fn oversized_entries_are_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf(), 10).unwrap();
        cache.write("big", &[0u8; 11]).unwrap();
        assert!(!dir.path().join("big").exists());
    }

    #[test]
    fn prune_removes_only_stale_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf(), 1000).unwrap();
        fs::write(dir.path().join("stale.1.0.tmp"), b"x").unwrap();
        fs::write(dir.path().join("fresh.1.1.tmp"), b"x").unwrap();
        set_age(&dir.path().join("stale.1.0.tmp"), Duration::from_secs(7200));
        cache.prune();
        assert!(!dir.path().join("stale.1.0.tmp").exists());
        assert!(dir.path().join("fresh.1.1.tmp").exists());
        assert_eq!(cache.usage(), 0);
    }

    #[test]
    fn remove_updates_usage() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::open(dir.path().to_path_buf(), 1000).unwrap();
        cache.write("k", &[0u8; 10]).unwrap();
        cache.remove("k");
        assert_eq!(cache.usage(), 0);
        assert!(!dir.path().join("k").exists());
    }
}
