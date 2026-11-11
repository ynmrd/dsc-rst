//! Installs the process-wide subscriber, so this lives in its own test binary and
//! contains a single test.

use std::fs::{self, File};
use std::time::{Duration, SystemTime};

use dn_diagnostics::{LOG_FILE_PREFIX, init_logging};

#[test]
fn logs_reach_the_rolling_file_and_second_init_fails() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    fs::create_dir_all(&logs).unwrap();

    let stale = logs.join(format!("{LOG_FILE_PREFIX}.2020-01-01"));
    File::create(&stale).unwrap().set_modified(SystemTime::now() - Duration::from_secs(10 * 24 * 3600)).unwrap();

    let guard = init_logging(&logs, "debug", false).expect("first init succeeds");
    assert!(!stale.exists(), "files older than seven days are pruned at startup");

    tracing::info!(target: "dn_test", "hello-from-the-test");
    tracing::debug!(target: "dn_test", "debug-line");
    tracing::debug!(target: "hyper", "noisy-hyper-debug");
    tracing::warn!(target: "hyper", "hyper-warning");

    let second = init_logging(&logs, "info", false);
    assert!(second.is_err(), "a global subscriber is already installed");

    // Dropping the guard flushes the non-blocking writer.
    drop(guard);

    let files: Vec<_> = fs::read_dir(&logs).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(LOG_FILE_PREFIX), "{name}");

    let text = fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("hello-from-the-test"), "{text}");
    assert!(text.contains("debug-line"), "level=debug lets debug through: {text}");
    assert!(text.contains("INFO") && text.contains("dn_test"));
    assert!(!text.contains("noisy-hyper-debug"), "noisy dependencies are capped at warn: {text}");
    assert!(text.contains("hyper-warning"));
    assert!(!text.contains('\u{1b}'), "no ANSI escapes in files");
}
