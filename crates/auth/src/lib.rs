//! OAuth2 PKCE loopback sign-in, bot token validation, OS credential storage and account management.
//!
//! Only official, authorized Discord interfaces are used: the OAuth2 authorization-code flow
//! with PKCE in the system browser, and bot tokens from applications the user owns. User tokens,
//! password logins and QR remote-auth are intentionally unsupported. Secrets are never logged.

pub mod accounts;
mod error;
mod http;
pub mod oauth;
pub mod store;

pub use accounts::{AccountManager, AccountRecord, OAuthLink, UserSummary};
pub use error::AuthError;
pub use oauth::{OAuthConfig, OAuthTokens};
pub use store::{KeyringStore, MemoryStore, SecretStore};

pub(crate) fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}
