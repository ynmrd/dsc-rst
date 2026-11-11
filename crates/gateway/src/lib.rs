//! Discord gateway v10 WebSocket client: heartbeat, identify, resume, reconnect and
//! `zlib-stream` transport compression.
//!
//! Authenticates with a bot token only (`IDENTIFY` with the documented bot fields);
//! the token is never logged.

mod inflate;
mod intents;
mod ratelimit;
mod runner;

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dn_core::{Id, Status};
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub use intents::Intents;

/// Used when [`GatewayConfig::url`] is `None`.
pub const DEFAULT_GATEWAY_URL: &str = "wss://gateway.discord.gg";

/// `IDENTIFY` connection properties.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionProperties {
    pub os: String,
    pub browser: String,
    pub device: String,
}

impl Default for ConnectionProperties {
    fn default() -> Self {
        ConnectionProperties {
            os: std::env::consts::OS.to_string(),
            browser: dn_core::PRODUCT_NAME.to_string(),
            device: dn_core::PRODUCT_NAME.to_string(),
        }
    }
}

/// Internal timing knobs; production values by default.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timings {
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub identify_spacing: Duration,
    pub invalid_session_min: Duration,
    pub invalid_session_max: Duration,
    pub connect_timeout: Duration,
    pub hello_timeout: Duration,
    pub close_timeout: Duration,
    pub send_window: Duration,
    pub send_limit: u32,
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            backoff_base: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            identify_spacing: Duration::from_secs(5),
            invalid_session_min: Duration::from_secs(1),
            invalid_session_max: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(30),
            hello_timeout: Duration::from_secs(20),
            close_timeout: Duration::from_secs(2),
            send_window: Duration::from_secs(60),
            send_limit: 120,
        }
    }
}

#[derive(Clone)]
pub struct GatewayConfig {
    /// Bot token (without the `Bot ` prefix).
    pub token: String,
    pub intents: Intents,
    /// Gateway URL from `GET /gateway/bot`; `None` uses [`DEFAULT_GATEWAY_URL`].
    pub url: Option<String>,
    /// `compress=zlib-stream` transport compression.
    pub compress: bool,
    /// 50..=250 (clamped): member count above which offline members are not sent.
    pub large_threshold: u8,
    pub presence: Option<PresenceUpdate>,
    pub properties: ConnectionProperties,
    #[doc(hidden)]
    pub timings: Timings,
}

impl GatewayConfig {
    pub fn new(token: impl Into<String>, intents: Intents) -> Self {
        GatewayConfig { token: token.into(), intents, ..Default::default() }
    }

    /// Millisecond-scale backoffs and delays for tests.
    #[doc(hidden)]
    pub fn test_timings(mut self) -> Self {
        self.timings = Timings {
            backoff_base: Duration::from_millis(20),
            backoff_max: Duration::from_millis(100),
            identify_spacing: Duration::from_millis(30),
            invalid_session_min: Duration::from_millis(10),
            invalid_session_max: Duration::from_millis(30),
            connect_timeout: Duration::from_secs(3),
            hello_timeout: Duration::from_secs(3),
            close_timeout: Duration::from_millis(300),
            ..Timings::default()
        };
        self
    }
}

impl Default for GatewayConfig {
    fn default() -> Self {
        GatewayConfig {
            token: String::new(),
            intents: Intents::default_client(),
            url: None,
            compress: true,
            large_threshold: 250,
            presence: None,
            properties: ConnectionProperties::default(),
            timings: Timings::default(),
        }
    }
}

impl fmt::Debug for GatewayConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayConfig")
            .field("token", &"<redacted>")
            .field("intents", &self.intents)
            .field("url", &self.url)
            .field("compress", &self.compress)
            .field("large_threshold", &self.large_threshold)
            .field("presence", &self.presence)
            .field("properties", &self.properties)
            .finish()
    }
}

/// Own presence (op 3 / identify `presence`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PresenceUpdate {
    pub status: Status,
    /// Custom status text, sent as a type 4 (custom) activity.
    pub activity: Option<String>,
    pub afk: bool,
}

impl PresenceUpdate {
    pub(crate) fn to_json(&self) -> Value {
        let since = (self.status == Status::Idle)
            .then(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0));
        let activities: Vec<Value> = self
            .activity
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|text| json!({ "name": "Custom Status", "type": 4, "state": text }))
            .into_iter()
            .collect();
        json!({ "since": since, "activities": activities, "status": self.status.as_str(), "afk": self.afk })
    }
}

/// Join/move/leave voice (op 4). `channel_id: None` leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceStateUpdate {
    pub guild_id: Id,
    pub channel_id: Option<Id>,
    pub self_mute: bool,
    pub self_deaf: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayCommand {
    UpdatePresence(PresenceUpdate),
    UpdateVoiceState(VoiceStateUpdate),
    /// op 8; with `user_ids` empty, `query` (default `""`) is used.
    RequestGuildMembers {
        guild_id: Id,
        query: Option<String>,
        limit: u32,
        presences: bool,
        user_ids: Vec<Id>,
        nonce: Option<String>,
    },
}

impl GatewayCommand {
    pub(crate) fn to_json(&self) -> Value {
        match self {
            GatewayCommand::UpdatePresence(p) => json!({ "op": 3, "d": p.to_json() }),
            GatewayCommand::UpdateVoiceState(v) => json!({
                "op": 4,
                "d": {
                    "guild_id": v.guild_id,
                    "channel_id": v.channel_id,
                    "self_mute": v.self_mute,
                    "self_deaf": v.self_deaf,
                }
            }),
            GatewayCommand::RequestGuildMembers { guild_id, query, limit, presences, user_ids, nonce } => {
                let mut d = json!({ "guild_id": guild_id, "limit": limit, "presences": presences });
                if user_ids.is_empty() {
                    d["query"] = json!(query.as_deref().unwrap_or(""));
                } else {
                    d["user_ids"] = json!(user_ids);
                }
                if let Some(n) = nonce {
                    d["nonce"] = json!(n);
                }
                json!({ "op": 8, "d": d })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Identifying,
    Resuming,
    Connected,
    Reconnecting { attempt: u32, delay: Duration },
    Disconnected,
}

/// Unrecoverable gateway close codes; the client stops after emitting one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FatalError {
    #[error("Authentication failed: the bot token is invalid or was reset (close code 4004).")]
    AuthenticationFailed,
    #[error("Invalid shard (close code 4010).")]
    InvalidShard,
    #[error("Sharding required: the bot is in too many guilds for one connection (close code 4011).")]
    ShardingRequired,
    #[error("Invalid gateway API version (close code 4012).")]
    InvalidApiVersion,
    #[error("Invalid intents (close code 4013).")]
    InvalidIntents,
    #[error(
        "Disallowed intents: enable the Server Members, Presence and Message Content intents for this \
         application in the Developer Portal, or connect without privileged intents (close code 4014)."
    )]
    DisallowedIntents,
    #[error("{0}")]
    Other(String),
}

impl FatalError {
    pub fn from_close_code(code: u16) -> Option<FatalError> {
        Some(match code {
            4004 => FatalError::AuthenticationFailed,
            4010 => FatalError::InvalidShard,
            4011 => FatalError::ShardingRequired,
            4012 => FatalError::InvalidApiVersion,
            4013 => FatalError::InvalidIntents,
            4014 => FatalError::DisallowedIntents,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum GatewayEvent {
    State(ConnectionState),
    Dispatch(Box<dn_core::Dispatch>),
    /// Heartbeat round-trip time.
    Latency(Duration),
    Fatal(FatalError),
    /// Non-fatal problem, e.g. one dispatch that failed to parse.
    Warning(String),
}

pub(crate) enum Control {
    Command(GatewayCommand),
    Shutdown,
}

/// Handle to a running gateway task. Dropping it also shuts the connection down.
pub struct GatewayHandle {
    tx: mpsc::UnboundedSender<Control>,
    task: tokio::task::JoinHandle<()>,
    session: Arc<Mutex<Option<String>>>,
}

impl GatewayHandle {
    /// Queues a command; it is sent once connected, within the gateway send limit.
    /// Returns `false` if the gateway task has stopped.
    pub fn send(&self, cmd: GatewayCommand) -> bool {
        self.tx.send(Control::Command(cmd)).is_ok()
    }

    /// Closes with code 1000 (ending the session) and waits for the task to finish.
    pub async fn shutdown(self) {
        let _ = self.tx.send(Control::Shutdown);
        let _ = self.task.await;
    }

    pub fn session_id(&self) -> Option<String> {
        self.session.lock().ok().and_then(|s| s.clone())
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl fmt::Debug for GatewayHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayHandle").field("finished", &self.task.is_finished()).finish()
    }
}

/// Starts the gateway on the current tokio runtime (panics outside one).
pub fn spawn(config: GatewayConfig, events: mpsc::UnboundedSender<GatewayEvent>) -> GatewayHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let session = Arc::new(Mutex::new(None));
    let runner = runner::Runner::new(config, events, rx, session.clone());
    let task = tokio::spawn(runner.run());
    GatewayHandle { tx, task, session }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_debug_redacts_token() {
        let c = GatewayConfig::new("very.secret.token", Intents::GUILDS);
        assert!(!format!("{c:?}").contains("very.secret.token"));
    }

    #[test]
    fn presence_json() {
        let p = PresenceUpdate { status: Status::Dnd, activity: Some("busy".into()), afk: false };
        assert_eq!(
            p.to_json(),
            json!({"since": null, "activities": [{"name": "Custom Status", "type": 4, "state": "busy"}], "status": "dnd", "afk": false})
        );
        let idle = PresenceUpdate { status: Status::Idle, ..Default::default() }.to_json();
        assert!(idle["since"].as_u64().unwrap() > 0);
        assert_eq!(idle["activities"], json!([]));
    }

    #[test]
    fn request_members_json() {
        let c = GatewayCommand::RequestGuildMembers {
            guild_id: Id(1),
            query: None,
            limit: 0,
            presences: true,
            user_ids: vec![],
            nonce: Some("n".into()),
        };
        assert_eq!(c.to_json(), json!({"op": 8, "d": {"guild_id": "1", "query": "", "limit": 0, "presences": true, "nonce": "n"}}));
        let c = GatewayCommand::RequestGuildMembers {
            guild_id: Id(1),
            query: Some("x".into()),
            limit: 5,
            presences: false,
            user_ids: vec![Id(2)],
            nonce: None,
        };
        assert_eq!(c.to_json()["d"]["user_ids"], json!(["2"]));
        assert!(c.to_json()["d"].get("query").is_none());
    }
}
