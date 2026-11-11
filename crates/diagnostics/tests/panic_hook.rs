//! Installs a process-wide panic hook, so this lives in its own test binary.

use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

use dn_diagnostics::{install_panic_hook, latest_crash_report};

static PREVIOUS_HOOK_RAN: AtomicBool = AtomicBool::new(false);

#[test]
fn panic_writes_a_crash_report_and_chains_to_the_previous_hook() {
    let dir = tempfile::tempdir().unwrap();
    let crash_dir = dir.path().join("crashes");
    assert!(latest_crash_report(&crash_dir).is_none());

    std::panic::set_hook(Box::new(|_| PREVIOUS_HOOK_RAN.store(true, Ordering::SeqCst)));
    install_panic_hook(crash_dir.clone());

    let joined = std::thread::Builder::new()
        .name("crashy-worker".into())
        .spawn(|| {
            let answer = 42;
            panic!("synthetic failure {answer}");
        })
        .unwrap()
        .join();
    assert!(joined.is_err());

    let (path, text) = latest_crash_report(&crash_dir).expect("a report was written");
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("crash-") && name.ends_with(".txt"), "{name}");
    assert_eq!(name.len(), "crash-YYYYMMDD-HHMMSS.txt".len());

    assert!(text.contains(&format!("version:  {}", env!("CARGO_PKG_VERSION"))), "{text}");
    assert!(text.contains(std::env::consts::OS) && text.contains(std::env::consts::ARCH));
    assert!(text.contains("thread:   crashy-worker"), "{text}");
    assert!(text.contains("message:  synthetic failure 42"), "{text}");
    assert!(text.contains("panic_hook.rs:"), "location is recorded: {text}");
    assert!(text.contains("backtrace:"));
    assert!(PREVIOUS_HOOK_RAN.load(Ordering::SeqCst), "the earlier hook still runs");

    // A second panic in the same second gets its own file.
    let _ = std::thread::spawn(|| panic!("second")).join();
    assert_eq!(fs::read_dir(&crash_dir).unwrap().count(), 2);
    let (_, newest) = latest_crash_report(&crash_dir).unwrap();
    assert!(newest.contains("message:  second") || newest.contains("message:  synthetic failure 42"));
}
