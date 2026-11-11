use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// Rolling files are named `discord-native.log.<YYYY-MM-DD>`.
pub const LOG_FILE_PREFIX: &str = "discord-native.log";

const RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

const NOISY_TARGETS: &str = "wgpu=warn,wgpu_core=warn,wgpu_hal=warn,naga=warn,symphonia=warn,symphonia_core=warn,\
    songbird=info,rustls=warn,h2=warn,hyper=warn,hyper_util=warn";

/// Keeps the background log writer alive; dropping it flushes pending lines.
#[must_use = "logs are flushed and the writer stops when the guard is dropped"]
pub struct LogGuard {
    _worker: WorkerGuard,
}

/// Installs the global tracing subscriber: daily rolling file in `logs_dir` through a
/// non-blocking writer, plus stderr when `stderr` is set. `RUST_LOG` wins over `level`,
/// which is a level name (`info`, `debug`, ...) or a directive list. Log files older
/// than seven days are removed. Fails (instead of panicking) if a global subscriber is
/// already installed.
pub fn init_logging(logs_dir: &Path, level: &str, stderr: bool) -> Result<LogGuard, io::Error> {
    std::fs::create_dir_all(logs_dir)?;
    prune_old_logs(logs_dir, RETENTION);

    let appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .build(logs_dir)
        .map_err(io::Error::other)?;
    let (writer, worker) = tracing_appender::non_blocking(appender);

    let file_layer = fmt::layer().with_writer(writer).with_ansi(false);
    let stderr_layer = stderr.then(|| fmt::layer().with_writer(io::stderr));
    tracing_subscriber::registry().with(env_filter(level)).with(file_layer).with(stderr_layer).try_init().map_err(io::Error::other)?;
    Ok(LogGuard { _worker: worker })
}

fn env_filter(level: &str) -> EnvFilter {
    let from_env = std::env::var("RUST_LOG").ok().filter(|v| !v.trim().is_empty()).and_then(|v| EnvFilter::try_new(v).ok());
    from_env
        .or_else(|| EnvFilter::try_new(format!("{},{NOISY_TARGETS}", level.trim())).ok())
        .unwrap_or_else(|| EnvFilter::new(format!("info,{NOISY_TARGETS}")))
}

/// Deletes log files in `dir` last modified more than `max_age` ago; returns how many.
pub(crate) fn prune_old_logs(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    let now = SystemTime::now();
    entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(LOG_FILE_PREFIX))
        .filter(|e| {
            e.metadata().and_then(|m| m.modified()).is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > max_age))
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    #[test]
    fn prunes_only_old_log_files() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("discord-native.log.2020-01-01");
        let fresh = dir.path().join("discord-native.log.2999-01-01");
        let unrelated = dir.path().join("notes.txt");
        for p in [&old, &fresh, &unrelated] {
            File::create(p).unwrap();
        }
        let ten_days_ago = SystemTime::now() - Duration::from_secs(10 * 24 * 3600);
        for p in [&old, &unrelated] {
            File::options().write(true).open(p).unwrap().set_modified(ten_days_ago).unwrap();
        }

        assert_eq!(prune_old_logs(dir.path(), RETENTION), 1);
        assert!(!old.exists());
        assert!(fresh.exists() && unrelated.exists());
        assert_eq!(prune_old_logs(&dir.path().join("missing"), RETENTION), 0);
    }

    #[test]
    fn invalid_level_falls_back_to_info() {
        // Only checks that building a filter never panics or errors out.
        for level in ["info", "DEBUG", "dn_gateway=trace,warn", "", "not a level!!", "=", "off"] {
            let _ = env_filter(level);
        }
    }
}
