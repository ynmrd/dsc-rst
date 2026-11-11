//! GPU texture cache on top of the async media loader. Decoding happens off the UI thread;
//! the UI only uploads ready RGBA buffers and evicts least-recently-used textures beyond
//! a byte budget (dropping a `TextureHandle` frees the GPU texture).

use egui::{ColorImage, TextureHandle, TextureOptions};

use dn_media::{BoundedCache, MediaEvent, MediaLoader};

pub struct Textures {
    loader: Option<MediaLoader>,
    cache: BoundedCache<String, TextureHandle>,
    failed: std::collections::HashSet<String>,
    pending: std::collections::VecDeque<dn_media::DecodedImage>,
    uploads_this_frame: usize,
}

/// Uploading many large textures in one frame causes a visible hitch; spread them out.
const MAX_UPLOADS_PER_FRAME: usize = 8;

impl Textures {
    pub fn new(loader: Option<MediaLoader>, budget_bytes: usize) -> Self {
        Textures {
            loader,
            cache: BoundedCache::new(budget_bytes),
            failed: Default::default(),
            pending: Default::default(),
            uploads_this_frame: 0,
        }
    }

    /// Upload decoded images that arrived since the last frame.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.uploads_this_frame = 0;
        let Some(loader) = self.loader.clone() else { return };
        for ev in loader.drain() {
            match ev {
                MediaEvent::Ready(img) => self.pending.push_back(img),
                MediaEvent::Failed { url, error } => {
                    tracing::debug!(%url, %error, "image failed");
                    self.failed.insert(url);
                }
            }
        }
        while self.uploads_this_frame < MAX_UPLOADS_PER_FRAME {
            let Some(img) = self.pending.pop_front() else { break };
            let size = [img.width as usize, img.height as usize];
            let bytes = img.rgba.len();
            let image = ColorImage::from_rgba_unmultiplied(size, &img.rgba);
            let tex = ctx.load_texture(img.url.clone(), image, TextureOptions::LINEAR);
            for (url, _) in self.cache.insert(img.url, tex, bytes) {
                loader.forget(&url);
            }
            self.uploads_this_frame += 1;
        }
        if !self.pending.is_empty() {
            ctx.request_repaint();
        }
    }

    /// Texture for `url` if ready; otherwise schedules a load and returns None.
    pub fn get(&mut self, url: &str, max_dim: u32) -> Option<TextureHandle> {
        if url.is_empty() {
            return None;
        }
        if let Some(t) = self.cache.get(&url.to_string()) {
            return Some(t.clone());
        }
        if self.failed.contains(url) {
            return None;
        }
        if let Some(loader) = &self.loader {
            loader.request(url, max_dim);
        }
        None
    }

    pub fn has_failed(&self, url: &str) -> bool {
        self.failed.contains(url)
    }

    pub fn set_budget(&mut self, bytes: usize) {
        let evicted = self.cache.set_budget(bytes);
        if let Some(loader) = &self.loader {
            for (url, _) in evicted {
                loader.forget(&url);
            }
        }
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.failed.clear();
        if let Some(loader) = &self.loader {
            loader.cancel_all();
        }
    }

    pub fn stats(&self) -> (usize, usize) {
        (self.cache.len(), self.cache.bytes())
    }
}
