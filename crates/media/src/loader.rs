use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use tokio::task::AbortHandle;
use url::Url;

use crate::MediaError;
use crate::decode::{self, MAX_OUTPUT_DIM};
use crate::disk::{self, DiskCache};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 5;

/// Straight (non-premultiplied) RGBA8 pixels, row-major, `width * height * 4` bytes.
#[derive(Debug, Clone)]
pub struct DecodedImage {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum MediaEvent {
    Ready(DecodedImage),
    Failed { url: String, error: String },
}

#[derive(Debug, Clone)]
pub struct MediaConfig {
    /// Directory for cached originals; `None` disables the disk cache.
    pub disk_dir: Option<PathBuf>,
    pub disk_budget_bytes: u64,
    /// Maximum simultaneous fetch+decode pipelines.
    pub max_concurrent: usize,
    pub max_download_bytes: u64,
    /// How long a failed URL is left alone before it may be requested again.
    pub failure_cooldown: Duration,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            disk_dir: None,
            disk_budget_bytes: 512 * 1024 * 1024,
            max_concurrent: 6,
            max_download_bytes: 25 * 1024 * 1024,
            failure_cooldown: Duration::from_secs(300),
        }
    }
}

#[derive(Default)]
struct State {
    /// Bumped by `cancel_all`; results from older generations are discarded.
    generation: u64,
    in_flight: HashMap<String, AbortHandle>,
    /// Delivered to the UI and not yet `forget`-ten.
    delivered: HashSet<String>,
    failed: HashMap<String, Instant>,
}

struct Inner {
    cfg: MediaConfig,
    runtime: Handle,
    on_ready: Arc<dyn Fn() + Send + Sync>,
    client: Result<reqwest::Client, String>,
    permits: Semaphore,
    disk: Option<Arc<DiskCache>>,
    tx: Sender<MediaEvent>,
    rx: Receiver<MediaEvent>,
    state: Mutex<State>,
}

/// Cheap-to-clone handle to the loader; all clones share one pipeline.
#[derive(Clone)]
pub struct MediaLoader {
    inner: Arc<Inner>,
}

impl MediaLoader {
    /// `on_ready` is invoked from a runtime thread after each event is queued (repaint request).
    pub fn new(cfg: MediaConfig, runtime: Handle, on_ready: Arc<dyn Fn() + Send + Sync>) -> Self {
        let client = reqwest::Client::builder()
            .user_agent(dn_core::user_agent())
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(|e| {
                tracing::error!(error = %e, "failed to build image HTTP client");
                e.to_string()
            });

        let disk = cfg.disk_dir.clone().and_then(|dir| match DiskCache::open(dir, cfg.disk_budget_bytes) {
            Ok(cache) => Some(Arc::new(cache)),
            Err(e) => {
                tracing::warn!(error = %e, "image disk cache disabled");
                None
            }
        });
        if let Some(disk) = &disk {
            let disk = disk.clone();
            runtime.spawn_blocking(move || disk.prune());
        }

        let (tx, rx) = crossbeam_channel::unbounded();
        let permits = Semaphore::new(cfg.max_concurrent.max(1));
        Self { inner: Arc::new(Inner { cfg, runtime, on_ready, client, permits, disk, tx, rx, state: Mutex::default() }) }
    }

    /// Queues `url` for loading unless it is already in flight, already delivered (until
    /// `forget`), or failed within the cooldown. Cheap enough to call every frame.
    /// `max_dim` bounds the longer side of the decoded image (0 = no limit beyond a
    /// built-in ceiling); only the first request for a URL decides its size.
    pub fn request(&self, url: &str, max_dim: u32) {
        if url.is_empty() {
            return;
        }
        let mut state = self.inner.state.lock();
        if state.in_flight.contains_key(url) || state.delivered.contains(url) {
            return;
        }
        if let Some(failed_at) = state.failed.get(url) {
            if failed_at.elapsed() < self.inner.cfg.failure_cooldown {
                return;
            }
            state.failed.remove(url);
        }
        let generation = state.generation;
        let task = self.inner.runtime.spawn(self.inner.clone().run(url.to_owned(), max_dim, generation));
        state.in_flight.insert(url.to_owned(), task.abort_handle());
    }

    /// The UI dropped its copy of `url`; a later `request` will load it again.
    pub fn forget(&self, url: &str) {
        self.inner.state.lock().delivered.remove(url);
    }

    pub fn drain(&self) -> Vec<MediaEvent> {
        self.inner.rx.try_iter().collect()
    }

    /// Aborts everything in flight, discards undelivered events and resets all dedupe
    /// state (e.g. on account switch).
    pub fn cancel_all(&self) {
        let mut state = self.inner.state.lock();
        state.generation += 1;
        for (_, task) in state.in_flight.drain() {
            task.abort();
        }
        state.delivered.clear();
        state.failed.clear();
        while self.inner.rx.try_recv().is_ok() {}
    }

    /// Bytes currently used by the disk cache (0 without one).
    pub fn disk_usage(&self) -> u64 {
        self.inner.disk.as_ref().map_or(0, |d| d.usage())
    }
}

impl Inner {
    async fn run(self: Arc<Self>, url: String, max_dim: u32, generation: u64) {
        let outcome = self.load(&url, max_dim).await;
        {
            let mut state = self.state.lock();
            if state.generation != generation {
                return;
            }
            state.in_flight.remove(&url);
            let event = match outcome {
                Ok(pixels) => {
                    state.delivered.insert(url.clone());
                    MediaEvent::Ready(DecodedImage { url, width: pixels.width, height: pixels.height, rgba: pixels.rgba })
                }
                Err(e) => {
                    tracing::debug!(%url, error = %e, "image load failed");
                    state.failed.insert(url.clone(), Instant::now());
                    MediaEvent::Failed { url, error: e.to_string() }
                }
            };
            // Sent under the lock so `cancel_all` cannot interleave between the check and the send.
            let _ = self.tx.send(event);
        }
        (self.on_ready)();
    }

    async fn load(&self, url: &str, max_dim: u32) -> Result<decode::Pixels, MediaError> {
        let parsed = parse_url(url)?;
        let _permit = self.permits.acquire().await.map_err(|e| MediaError::Task(e.to_string()))?;
        let max_dim = if max_dim == 0 { MAX_OUTPUT_DIM } else { max_dim };
        let key = disk::cache_key(&parsed);

        if let Some(disk) = &self.disk {
            let (reader, name, max_len) = (disk.clone(), key.clone(), self.cfg.max_download_bytes);
            if let Some(bytes) = blocking(move || reader.read(&name, max_len)).await? {
                match decode_blocking(bytes, max_dim).await {
                    Ok(pixels) => return Ok(pixels),
                    Err(e) => {
                        tracing::debug!(%url, error = %e, "dropping unreadable cached image");
                        disk.remove(&key);
                    }
                }
            }
        }

        let bytes: Arc<[u8]> = self.download(parsed).await?.into();
        let pixels = decode_blocking(bytes.clone(), max_dim).await?;
        if let Some(disk) = &self.disk {
            let disk = disk.clone();
            let stored = blocking(move || disk.write(&key, &bytes)).await?;
            if let Err(e) = stored {
                tracing::warn!(error = %e, "failed to write image to disk cache");
            }
        }
        Ok(pixels)
    }

    async fn download(&self, url: Url) -> Result<Vec<u8>, MediaError> {
        let client = self.client.as_ref().map_err(|e| MediaError::Client(e.clone()))?;
        let cap = self.cfg.max_download_bytes;
        let mut response = client.get(url).send().await.map_err(|e| e.without_url())?;
        let status = response.status();
        if !status.is_success() {
            return Err(MediaError::Status(status.as_u16()));
        }
        if response.content_length().is_some_and(|len| len > cap) {
            return Err(MediaError::TooLarge(cap));
        }
        let mut body = Vec::with_capacity(response.content_length().unwrap_or(0).min(cap) as usize);
        while let Some(chunk) = response.chunk().await.map_err(|e| e.without_url())? {
            if body.len() as u64 + chunk.len() as u64 > cap {
                return Err(MediaError::TooLarge(cap));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

fn parse_url(url: &str) -> Result<Url, MediaError> {
    let parsed = Url::parse(url).map_err(|e| MediaError::Url(e.to_string()))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => Err(MediaError::Scheme(other.to_owned())),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, MediaError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| MediaError::Task(e.to_string()))
}

/// Decoding runs off the async workers; a panicking decoder surfaces as a failed load.
async fn decode_blocking(bytes: impl AsRef<[u8]> + Send + 'static, max_dim: u32) -> Result<decode::Pixels, MediaError> {
    blocking(move || decode::decode(bytes.as_ref(), max_dim)).await?
}
