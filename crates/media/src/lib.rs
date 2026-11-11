//! Async image fetch/decode pipeline with bounded memory and disk caches.
//!
//! UI-agnostic: the UI calls [`MediaLoader::request`] each frame, polls
//! [`MediaLoader::drain`] for decoded RGBA buffers, and keeps its textures in a
//! [`BoundedCache`].

mod cache;
mod decode;
mod disk;
mod loader;

pub use cache::BoundedCache;
pub use loader::{DecodedImage, MediaConfig, MediaEvent, MediaLoader};

/// Why a single image load failed; only its text is surfaced in [`MediaEvent::Failed`].
#[derive(Debug, thiserror::Error)]
pub(crate) enum MediaError {
    #[error("invalid URL: {0}")]
    Url(String),
    #[error("unsupported URL scheme `{0}`")]
    Scheme(String),
    #[error("HTTP client unavailable: {0}")]
    Client(String),
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("HTTP status {0}")]
    Status(u16),
    #[error("download exceeds {0} bytes")]
    TooLarge(u64),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("image dimensions {0}x{1} are not supported")]
    Dimensions(u32, u32),
    #[error("background task failed: {0}")]
    Task(String),
}
