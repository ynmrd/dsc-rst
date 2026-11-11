use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle, ThreadId};

use tokio::sync::oneshot;

use crate::{Storage, StorageError};

type Job = Box<dyn FnOnce(&mut Storage) + Send>;

enum Msg {
    Job(Job),
    Stop,
}

struct Inner {
    tx: Sender<Msg>,
    thread: Mutex<Option<JoinHandle<()>>>,
    thread_id: Option<ThreadId>,
}

impl Inner {
    fn stop(&self) {
        let _ = self.tx.send(Msg::Stop);
        let handle = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take();
        if let Some(h) = handle {
            // A job holding the last handle must not join its own thread.
            if self.thread_id != Some(thread::current().id()) {
                let _ = h.join();
            }
        }
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Cloneable async front for a [`Storage`] owned by a dedicated `dn-storage` thread.
#[derive(Clone)]
pub struct StorageHandle {
    inner: Arc<Inner>,
}

impl StorageHandle {
    pub fn spawn(storage: Storage) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let spawned = thread::Builder::new().name("dn-storage".into()).spawn(move || run(storage, rx));
        let (thread, thread_id) = match spawned {
            Ok(h) => {
                let id = h.thread().id();
                (Some(h), Some(id))
            }
            Err(e) => {
                // The receiver went down with the failed closure, so every call reports `Closed`.
                tracing::error!("failed to spawn storage thread: {e}");
                (None, None)
            }
        };
        Self { inner: Arc::new(Inner { tx, thread: Mutex::new(thread), thread_id }) }
    }

    /// Runs `f` on the storage thread. A panicking job is reported as an error and
    /// does not take the thread down.
    pub async fn call<R: Send + 'static>(&self, f: impl FnOnce(&mut Storage) -> R + Send + 'static) -> Result<R, StorageError> {
        self.enqueue(f)?.await.map_err(|_| StorageError::Closed)?
    }

    /// Stops the thread once every job queued before this call has run, and waits for
    /// the database to close. Later calls return [`StorageError::Closed`].
    pub fn shutdown(&self) {
        self.inner.stop();
    }

    pub(crate) fn enqueue<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Storage) -> R + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<R, StorageError>>, StorageError> {
        let (reply, rx) = oneshot::channel();
        let job: Job = Box::new(move |storage| {
            let out = catch_unwind(AssertUnwindSafe(|| f(storage)))
                .map_err(|_| StorageError::Sqlite("storage job panicked".into()));
            let _ = reply.send(out);
        });
        self.inner.tx.send(Msg::Job(job)).map_err(|_| StorageError::Closed)?;
        Ok(rx)
    }
}

fn run(mut storage: Storage, rx: Receiver<Msg>) {
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Job(job) => job(&mut storage),
            Msg::Stop => break,
        }
    }
    storage.close();
}
