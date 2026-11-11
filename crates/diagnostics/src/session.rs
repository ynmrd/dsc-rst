use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;

const MARKER_FILE: &str = "session.marker";

/// A file that exists exactly while the app is running. If it is still there at the next
/// start, the previous run crashed or was killed.
///
/// Two instances sharing one data directory would confuse it; the app is expected to
/// enforce a single instance.
#[derive(Debug)]
pub struct SessionMarker {
    path: PathBuf,
}

impl SessionMarker {
    /// Returns the marker and whether the previous session ended uncleanly.
    pub fn begin(data_dir: &Path) -> (Self, bool) {
        let path = data_dir.join(MARKER_FILE);
        let unclean = path.exists();
        let contents = format!("pid={}\nstarted={}\n", std::process::id(), Utc::now().to_rfc3339());
        if fs::create_dir_all(data_dir).and_then(|()| fs::write(&path, contents)).is_err() {
            tracing::warn!("could not write session marker {}", path.display());
        }
        (Self { path }, unclean)
    }

    /// Clean shutdown: removes the marker.
    pub fn end(self) {
        if let Err(e) = fs::remove_file(&self.path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!("could not remove session marker {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_unclean_previous_session() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");

        let (first, unclean) = SessionMarker::begin(&data);
        assert!(!unclean, "no marker on a first run");
        assert!(data.join(MARKER_FILE).exists());

        // The first session never called `end`: it crashed.
        let (second, unclean) = SessionMarker::begin(&data);
        assert!(unclean);
        second.end();
        assert!(!data.join(MARKER_FILE).exists());

        let (third, unclean) = SessionMarker::begin(&data);
        assert!(!unclean, "clean shutdown clears the flag");
        third.end();
        first.end();
    }
}
