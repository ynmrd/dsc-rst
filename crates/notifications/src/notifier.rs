use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use dn_core::Id;

use crate::content::Notification;
use crate::limiter::{RateLimiter, Verdict};
use crate::system;

/// Notifications waiting for the worker; beyond this new ones are dropped.
const MAX_PENDING: usize = 8;
const ACTIVATION_BACKLOG: usize = 32;
const MAX_SHOWN_PER_WINDOW: usize = 5;
const RATE_WINDOW: Duration = Duration::from_secs(10);

/// Delivers a notification to the operating system. Clicks are reported by sending the
/// notification's channel id on `activated`, from whatever thread the platform uses.
pub(crate) trait Backend: Send + 'static {
    fn deliver(&mut self, n: &Notification, activated: &Sender<Id>) -> Result<(), String>;
}

#[derive(Clone)]
pub struct Notifier {
    jobs: Sender<Notification>,
    activated: Receiver<Id>,
}

impl Notifier {
    /// AppUserModelID used for toasts on Windows.
    pub const APP_ID: &'static str = "DiscordNative.Desktop";

    /// On Windows this registers the app identity (display name and icon) under
    /// `HKCU\Software\Classes\AppUserModelId`, which unpackaged apps need for toasts to show
    /// their own name; failures are logged and notifications still work.
    pub fn new(display_name: &str, icon_path: Option<PathBuf>) -> Self {
        system::register_app(display_name, icon_path.as_deref());
        let backend = system::SystemBackend::new(display_name, icon_path);
        Self::with_backend(display_name, backend, RateLimiter::new(MAX_SHOWN_PER_WINDOW, RATE_WINDOW))
    }

    pub(crate) fn with_backend(display_name: &str, backend: impl Backend, limiter: RateLimiter) -> Self {
        let (jobs, queue) = crossbeam_channel::bounded(MAX_PENDING);
        let (activated_tx, activated) = crossbeam_channel::bounded(ACTIVATION_BACKLOG);
        let worker = Worker { queue, backend, activated: activated_tx, limiter, title: display_name.to_owned() };
        let spawned = std::thread::Builder::new().name("dn-notifier".into()).spawn(move || worker.run());
        if let Err(e) = spawned {
            tracing::error!(error = %e, "failed to start notification worker");
        }
        Self { jobs, activated }
    }

    /// Queues `n` for display and returns immediately. Errors only mean the notification
    /// was not queued (worker backlog full or worker gone); OS-level failures are logged
    /// by the worker.
    pub fn show(&self, n: &Notification) -> Result<(), String> {
        self.jobs.try_send(n.clone()).map_err(|e| match e {
            TrySendError::Full(_) => "notification queue full, dropped".to_owned(),
            TrySendError::Disconnected(_) => "notification worker is not running".to_owned(),
        })
    }

    /// Channel ids of notifications the user clicked. Only yields where the platform
    /// reports clicks; otherwise it stays empty. All receivers share one queue.
    pub fn on_activated(&self) -> Receiver<Id> {
        self.activated.clone()
    }
}

struct Worker<B> {
    queue: Receiver<Notification>,
    backend: B,
    activated: Sender<Id>,
    limiter: RateLimiter,
    /// Title for summaries of coalesced notifications.
    title: String,
}

impl<B: Backend> Worker<B> {
    fn run(mut self) {
        // Most recent notification that was held back, so a summary click opens its channel.
        let mut held: Option<Notification> = None;
        loop {
            let received = match self.limiter.next_wakeup(Instant::now()) {
                Some(wait) => self.queue.recv_timeout(wait),
                None => self.queue.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(n) => match self.limiter.admit(Instant::now()) {
                    Verdict::Show => self.deliver(&n),
                    Verdict::Hold => held = Some(n),
                },
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            if let Some(count) = self.limiter.take_held(Instant::now())
                && let Some(last) = held.take()
            {
                if count == 1 {
                    self.deliver(&last);
                } else {
                    let summary = Notification { title: self.title.clone(), body: format!("{count} new messages"), ..last };
                    self.deliver(&summary);
                }
            }
        }
    }

    fn deliver(&mut self, n: &Notification) {
        if let Err(e) = self.backend.deliver(n, &self.activated) {
            tracing::warn!(error = %e, "failed to show notification");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Notification>>>);

    impl Recorder {
        fn bodies(&self) -> Vec<String> {
            self.0.lock().unwrap().iter().map(|n| n.body.clone()).collect()
        }
    }

    impl Backend for Recorder {
        fn deliver(&mut self, n: &Notification, _: &Sender<Id>) -> Result<(), String> {
            self.0.lock().unwrap().push(n.clone());
            Ok(())
        }
    }

    fn note(i: u64) -> Notification {
        Notification { title: "t".into(), body: format!("m{i}"), channel_id: Id(i), guild_id: None, message_id: Id(i) }
    }

    fn wait_until(mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "condition not met in time");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn bursts_are_limited_and_the_rest_coalesced_into_a_summary() {
        let rec = Recorder::default();
        let notifier = Notifier::with_backend("Discord Native", rec.clone(), RateLimiter::new(2, Duration::from_millis(300)));
        for i in 1..=5 {
            notifier.show(&note(i)).unwrap();
        }
        wait_until(|| rec.0.lock().unwrap().len() == 3);
        let shown = rec.0.lock().unwrap().clone();
        assert_eq!(&rec.bodies()[..2], ["m1", "m2"]);
        assert_eq!(shown[2].body, "3 new messages");
        assert_eq!(shown[2].title, "Discord Native");
        // The summary opens the most recent held conversation.
        assert_eq!(shown[2].channel_id, Id(5));
    }

    #[test]
    fn a_single_held_notification_is_shown_as_itself() {
        let rec = Recorder::default();
        let notifier = Notifier::with_backend("App", rec.clone(), RateLimiter::new(1, Duration::from_millis(200)));
        notifier.show(&note(1)).unwrap();
        notifier.show(&note(2)).unwrap();
        wait_until(|| rec.0.lock().unwrap().len() == 2);
        assert_eq!(rec.bodies(), ["m1", "m2"]);
    }

    struct Gate {
        started: Sender<()>,
        release: Receiver<()>,
        delivered: Arc<AtomicUsize>,
    }

    impl Backend for Gate {
        fn deliver(&mut self, _: &Notification, _: &Sender<Id>) -> Result<(), String> {
            let _ = self.started.send(());
            let _ = self.release.recv();
            self.delivered.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn show_never_blocks_and_drops_when_the_backlog_is_full() {
        let (started_tx, started) = crossbeam_channel::unbounded();
        let (release_tx, release) = crossbeam_channel::unbounded();
        let delivered = Arc::new(AtomicUsize::new(0));
        let notifier = Notifier::with_backend(
            "App",
            Gate { started: started_tx, release, delivered: delivered.clone() },
            RateLimiter::new(100, Duration::from_secs(10)),
        );

        notifier.show(&note(0)).unwrap();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        // Worker is stuck delivering #0; the queue takes MAX_PENDING more and then refuses.
        for i in 1..=MAX_PENDING as u64 {
            notifier.show(&note(i)).unwrap();
        }
        let started_at = Instant::now();
        assert!(notifier.show(&note(99)).is_err());
        assert!(started_at.elapsed() < Duration::from_secs(1));

        for _ in 0..=MAX_PENDING {
            release_tx.send(()).unwrap();
        }
        wait_until(|| delivered.load(Ordering::SeqCst) == MAX_PENDING + 1);
    }

    #[test]
    fn activations_flow_through_the_shared_receiver() {
        struct Clicker;
        impl Backend for Clicker {
            fn deliver(&mut self, n: &Notification, activated: &Sender<Id>) -> Result<(), String> {
                activated.send(n.channel_id).map_err(|e| e.to_string())
            }
        }
        let notifier = Notifier::with_backend("App", Clicker, RateLimiter::new(5, Duration::from_secs(10)));
        let rx = notifier.on_activated();
        assert!(rx.try_recv().is_err());
        notifier.show(&note(42)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Id(42)));
        assert_eq!(notifier.clone().on_activated().try_recv().ok(), None);
    }

    #[test]
    fn backend_errors_do_not_stop_the_worker() {
        struct Flaky(u32);
        impl Backend for Flaky {
            fn deliver(&mut self, n: &Notification, activated: &Sender<Id>) -> Result<(), String> {
                self.0 += 1;
                if self.0 == 1 {
                    return Err("boom".into());
                }
                let _ = activated.send(n.channel_id);
                Ok(())
            }
        }
        let notifier = Notifier::with_backend("App", Flaky(0), RateLimiter::new(5, Duration::from_secs(10)));
        let rx = notifier.on_activated();
        notifier.show(&note(1)).unwrap();
        notifier.show(&note(2)).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(Id(2)));
    }
}
