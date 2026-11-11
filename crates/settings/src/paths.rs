use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;

/// Where the application keeps its files. All four roots are created by [`Paths::ensure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub logs_dir: PathBuf,
}

impl Paths {
    /// Honors `DN_HOME` (portable mode), otherwise the platform's per-user directories.
    pub fn detect() -> Self {
        Self::from_override(std::env::var_os("DN_HOME"))
    }

    /// `detect` with the `DN_HOME` value passed in; an empty value counts as unset.
    pub fn from_override(home: Option<OsString>) -> Self {
        if let Some(root) = home.filter(|h| !h.is_empty()) {
            return Self::under(Path::new(&root));
        }
        match ProjectDirs::from("", "", "DiscordNative") {
            Some(dirs) => Self {
                config_dir: dirs.config_dir().to_path_buf(),
                data_dir: dirs.data_local_dir().to_path_buf(),
                cache_dir: dirs.cache_dir().to_path_buf(),
                logs_dir: dirs.data_local_dir().join("logs"),
            },
            // No home directory (service accounts, stripped containers).
            None => Self::under(&std::env::temp_dir().join("DiscordNative")),
        }
    }

    /// Portable layout: `config/`, `data/`, `cache/` and `logs/` below `root`.
    pub fn under(root: &Path) -> Self {
        Self { config_dir: root.join("config"), data_dir: root.join("data"), cache_dir: root.join("cache"), logs_dir: root.join("logs") }
    }

    pub fn ensure(&self) -> io::Result<()> {
        for dir in [&self.config_dir, &self.data_dir, &self.cache_dir, &self.logs_dir, &self.image_cache_dir(), &self.crash_dir()] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    /// Account metadata only; tokens live in the OS credential store.
    pub fn accounts_file(&self) -> PathBuf {
        self.config_dir.join("accounts.json")
    }

    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("cache.sqlite3")
    }

    pub fn image_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("images")
    }

    pub fn crash_dir(&self) -> PathBuf {
        self.data_dir.join("crashes")
    }
}
