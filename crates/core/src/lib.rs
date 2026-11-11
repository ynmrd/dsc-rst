//! Shared foundation for Discord Native: snowflake IDs, the Discord object model,
//! permission computation, CDN URL helpers and typed gateway dispatch events.

pub mod cdn;
pub mod dispatch;
pub mod id;
pub mod model;
pub mod permissions;

pub use dispatch::Dispatch;
pub use id::*;
pub use model::*;
pub use permissions::Permissions;

/// Discord REST API base (version pinned).
pub const API_BASE: &str = "https://discord.com/api/v10";
/// Gateway protocol version this client implements.
pub const GATEWAY_VERSION: u8 = 10;
/// Product name used in User-Agent headers, as required by Discord for API clients.
pub const PRODUCT_NAME: &str = "DiscordNative";
pub const PRODUCT_URL: &str = "https://github.com/ynmrd/dsc-rst";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `DiscordBot (url, version)` user agent, the documented format for bot API clients.
pub fn user_agent() -> String {
    format!("DiscordBot ({PRODUCT_URL}, {VERSION}) {PRODUCT_NAME}")
}
