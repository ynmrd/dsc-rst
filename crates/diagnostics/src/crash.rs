use std::backtrace::Backtrace;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

use chrono::Utc;

const REPORT_PREFIX: &str = "crash-";
const REPORT_SUFFIX: &str = ".txt";
const MAX_REPORTS: usize = 20;

/// Chains a panic hook in front of the current one that writes
/// `crash-YYYYMMDD-HHMMSS.txt` (UTC) into `crash_dir` and logs the panic. Only the
/// newest 20 reports are kept so a crash loop cannot fill the disk.
pub fn install_panic_hook(crash_dir: PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = build_report(info);
        match write_report(&crash_dir, &report) {
            Some(path) => tracing::error!(report = %path.display(), "panic: {}", summary(info)),
            None => tracing::error!("panic (crash report could not be written): {}", summary(info)),
        }
        previous(info);
    }));
}

/// The newest crash report in `crash_dir` and its text.
pub fn latest_crash_report(crash_dir: &Path) -> Option<(PathBuf, String)> {
    let path = report_files(crash_dir).pop()?;
    let text = String::from_utf8_lossy(&fs::read(&path).ok()?).into_owned();
    Some((path, text))
}

fn payload_message(info: &PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

fn location(info: &PanicHookInfo<'_>) -> String {
    info.location().map_or_else(|| "<unknown>".to_owned(), |l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
}

fn summary(info: &PanicHookInfo<'_>) -> String {
    format!("{} at {}", payload_message(info), location(info))
}

fn build_report(info: &PanicHookInfo<'_>) -> String {
    let thread = std::thread::current();
    format!(
        "Discord Native crash report\n\
         version:  {}\n\
         os:       {} {}\n\
         time:     {}\n\
         thread:   {}\n\
         message:  {}\n\
         location: {}\n\
         \n\
         backtrace:\n{}\n",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        Utc::now().to_rfc3339(),
        thread.name().unwrap_or("<unnamed>"),
        payload_message(info),
        location(info),
        Backtrace::force_capture(),
    )
}

/// Never panics: this runs inside the panic hook.
fn write_report(dir: &Path, body: &str) -> Option<PathBuf> {
    fs::create_dir_all(dir).ok()?;
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    for attempt in 0..100 {
        let name = if attempt == 0 {
            format!("{REPORT_PREFIX}{stamp}{REPORT_SUFFIX}")
        } else {
            format!("{REPORT_PREFIX}{stamp}_{attempt:02}{REPORT_SUFFIX}")
        };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(body.as_bytes()).ok()?;
                let _ = file.sync_all();
                prune_reports(dir, MAX_REPORTS);
                return Some(path);
            }
            // Several threads can panic within the same second.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

/// Report files oldest first. Names embed a UTC timestamp, so name order is time order.
fn report_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(REPORT_PREFIX) && n.ends_with(REPORT_SUFFIX))
                && p.is_file()
        })
        .collect();
    files.sort_by(|a, b| a.file_stem().cmp(&b.file_stem()));
    files
}

fn prune_reports(dir: &Path, keep: usize) {
    let files = report_files(dir);
    for stale in files.iter().take(files.len().saturating_sub(keep)) {
        let _ = fs::remove_file(stale);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_report_is_newest_by_name() {
        let dir = tempfile::tempdir().unwrap();
        assert!(latest_crash_report(dir.path()).is_none());
        for name in ["crash-20260101-010101.txt", "crash-20260101-010101_01.txt", "crash-20251231-235959.txt", "other.txt"] {
            fs::write(dir.path().join(name), name).unwrap();
        }
        let (path, text) = latest_crash_report(dir.path()).unwrap();
        assert_eq!(path.file_name().unwrap(), "crash-20260101-010101_01.txt");
        assert_eq!(text, "crash-20260101-010101_01.txt");
        assert!(latest_crash_report(&dir.path().join("missing")).is_none());
    }

    #[test]
    fn write_report_never_overwrites_and_prunes() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_report(dir.path(), "a").unwrap();
        let b = write_report(dir.path(), "b").unwrap();
        assert_ne!(a, b);
        assert_eq!(fs::read_to_string(&a).unwrap(), "a");

        for i in 0..30 {
            fs::write(dir.path().join(format!("crash-2020010{}-0000{:02}.txt", i % 9 + 1, i)), "x").unwrap();
        }
        prune_reports(dir.path(), MAX_REPORTS);
        assert_eq!(report_files(dir.path()).len(), MAX_REPORTS);
        assert!(b.exists(), "newest reports survive pruning");
    }
}
