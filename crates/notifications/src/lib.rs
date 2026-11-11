//! Desktop notifications and notification rules.
//!
//! [`should_notify`] decides whether a message deserves a notification,
//! [`build_notification`] turns it into display text, and [`Notifier`] shows it through
//! the operating system without blocking the caller.

mod content;
#[cfg(not(windows))]
mod desktop;
mod limiter;
mod notifier;
mod rules;
#[cfg(windows)]
mod toast;

#[cfg(not(windows))]
use desktop as system;
#[cfg(windows)]
use toast as system;

pub use content::{Notification, build_notification};
pub use notifier::Notifier;
pub use rules::{NotifyContext, NotifyLevel, is_mention, should_notify};
