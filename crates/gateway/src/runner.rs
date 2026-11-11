use std::collections::VecDeque;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use dn_core::Dispatch;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::{Instant, sleep_until};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

use crate::inflate::ZlibStream;
use crate::ratelimit::SendLimiter;
use crate::{ConnectionState, Control, DEFAULT_GATEWAY_URL, FatalError, GatewayCommand, GatewayConfig, GatewayEvent, PresenceUpdate};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

const MAX_PENDING_COMMANDS: usize = 512;
/// Non-1000 close code: Discord keeps the session resumable.
const CLOSE_RESUMABLE: u16 = 4000;

#[derive(Debug, Clone, Copy)]
enum Wait {
    Immediate,
    Backoff,
    Fixed(Duration),
}

#[derive(Debug)]
enum Outcome {
    Shutdown,
    /// The event receiver was dropped.
    Stop,
    Fatal(FatalError),
    Reconnect {
        resume: bool,
        wait: Wait,
    },
}

impl Outcome {
    fn resume(wait: Wait) -> Self {
        Outcome::Reconnect { resume: true, wait }
    }
}

struct Session {
    id: String,
    resume_url: Option<String>,
}

#[derive(Deserialize)]
struct Payload<'a> {
    op: u8,
    #[serde(borrow, default)]
    d: Option<&'a RawValue>,
    #[serde(default)]
    s: Option<u64>,
    #[serde(default)]
    t: Option<String>,
}

#[derive(Deserialize)]
struct Hello {
    heartbeat_interval: u64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ReadyMin {
    session_id: String,
    resume_gateway_url: Option<String>,
}

/// Per-connection state.
struct Conn {
    ws: Ws,
    hello: bool,
    interval: Duration,
    next_beat: Instant,
    ack_pending: bool,
    beat_sent_at: Option<Instant>,
    connected: bool,
    limiter: SendLimiter,
}

pub(crate) struct Runner {
    cfg: GatewayConfig,
    events: UnboundedSender<GatewayEvent>,
    rx: UnboundedReceiver<Control>,
    shared_session: Arc<Mutex<Option<String>>>,
    session: Option<Session>,
    seq: Option<u64>,
    presence: Option<PresenceUpdate>,
    attempt: u32,
    last_identify: Option<Instant>,
    pending: VecDeque<GatewayCommand>,
    stopped: bool,
}

impl Runner {
    pub fn new(
        cfg: GatewayConfig,
        events: UnboundedSender<GatewayEvent>,
        rx: UnboundedReceiver<Control>,
        shared_session: Arc<Mutex<Option<String>>>,
    ) -> Self {
        let presence = cfg.presence.clone();
        Runner {
            cfg,
            events,
            rx,
            shared_session,
            session: None,
            seq: None,
            presence,
            attempt: 0,
            last_identify: None,
            pending: VecDeque::new(),
            stopped: false,
        }
    }

    pub async fn run(mut self) {
        loop {
            self.emit(GatewayEvent::State(ConnectionState::Connecting));
            let outcome = if self.stopped { Outcome::Stop } else { self.connect_and_run().await };
            let (resume, wait) = match outcome {
                Outcome::Shutdown | Outcome::Stop => break,
                Outcome::Fatal(e) => {
                    tracing::error!(error = %e, "gateway closed with a fatal error");
                    self.clear_session();
                    self.emit(GatewayEvent::Fatal(e));
                    break;
                }
                Outcome::Reconnect { resume, wait } => (resume, wait),
            };
            if !resume {
                self.clear_session();
            }
            self.attempt = self.attempt.saturating_add(1);
            let mut delay = match wait {
                Wait::Immediate => Duration::ZERO,
                Wait::Fixed(d) => d,
                Wait::Backoff => self.backoff(),
            };
            if self.session.is_none() {
                delay = delay.max(self.identify_wait());
            }
            tracing::info!(attempt = self.attempt, ?delay, resume = self.session.is_some(), "gateway reconnecting");
            self.emit(GatewayEvent::State(ConnectionState::Reconnecting { attempt: self.attempt, delay }));
            if self.stopped || !self.sleep_unless_shutdown(delay).await {
                break;
            }
        }
        self.emit(GatewayEvent::State(ConnectionState::Disconnected));
    }

    fn emit(&mut self, ev: GatewayEvent) {
        if self.events.send(ev).is_err() {
            self.stopped = true;
        }
    }

    fn warn(&mut self, msg: String) {
        tracing::warn!("{msg}");
        self.emit(GatewayEvent::Warning(msg));
    }

    fn clear_session(&mut self) {
        self.session = None;
        self.seq = None;
        if let Ok(mut s) = self.shared_session.lock() {
            *s = None;
        }
    }

    fn backoff(&self) -> Duration {
        let t = &self.cfg.timings;
        let exp = self.attempt.saturating_sub(1).min(16);
        let base = t.backoff_base.saturating_mul(1u32 << exp).min(t.backoff_max);
        base.mul_f64(rand::random_range(0.75..1.25)).min(t.backoff_max)
    }

    fn identify_wait(&self) -> Duration {
        self.last_identify.map(|t| self.cfg.timings.identify_spacing.saturating_sub(t.elapsed())).unwrap_or_default()
    }

    fn queue(&mut self, cmd: GatewayCommand) {
        if let GatewayCommand::UpdatePresence(p) = &cmd {
            // Re-identify must carry the latest presence.
            self.presence = Some(p.clone());
        }
        if self.pending.len() >= MAX_PENDING_COMMANDS {
            self.pending.pop_front();
            self.warn("Too many queued gateway commands; dropped the oldest one".into());
        }
        self.pending.push_back(cmd);
    }

    /// Sleeps for `delay` while buffering commands. Returns `false` on shutdown.
    async fn sleep_unless_shutdown(&mut self, delay: Duration) -> bool {
        let deadline = Instant::now() + delay;
        loop {
            tokio::select! {
                _ = sleep_until(deadline) => return true,
                ctrl = self.rx.recv() => match ctrl {
                    Some(Control::Command(c)) => self.queue(c),
                    Some(Control::Shutdown) | None => return false,
                },
                _ = self.events.closed() => return false,
            }
        }
    }

    async fn connect_and_run(&mut self) -> Outcome {
        let base = match &self.session {
            Some(Session { resume_url: Some(u), .. }) => u.clone(),
            _ => self.cfg.url.clone().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_GATEWAY_URL.into()),
        };
        let url = match gateway_url(&base, self.cfg.compress) {
            Ok(u) => u,
            Err(e) => return Outcome::Fatal(FatalError::Other(e)),
        };
        let connector = match tls_config() {
            Ok(c) => Connector::Rustls(c),
            Err(e) => return Outcome::Fatal(FatalError::Other(e)),
        };
        tracing::debug!(%url, "connecting to gateway");
        let connect = tokio::time::timeout(
            self.cfg.timings.connect_timeout,
            tokio_tungstenite::connect_async_tls_with_config(url.as_str(), None, true, Some(connector)),
        );
        let ws = tokio::select! {
            r = connect => match r {
                Ok(Ok((ws, _))) => ws,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "gateway connection failed");
                    return Outcome::resume(Wait::Backoff);
                }
                Err(_) => {
                    tracing::warn!("gateway connection timed out");
                    return Outcome::resume(Wait::Backoff);
                }
            },
            stop = self.wait_for_shutdown() => return stop,
        };
        let mut conn = Conn {
            ws,
            hello: false,
            interval: Duration::from_secs(41),
            next_beat: Instant::now() + self.cfg.timings.hello_timeout,
            ack_pending: false,
            beat_sent_at: None,
            connected: false,
            limiter: SendLimiter::new(self.cfg.timings.send_window, self.cfg.timings.send_limit),
        };
        let outcome = self.drive(&mut conn).await;
        self.close(conn, &outcome).await;
        outcome
    }

    async fn wait_for_shutdown(&mut self) -> Outcome {
        loop {
            tokio::select! {
                ctrl = self.rx.recv() => match ctrl {
                    Some(Control::Command(c)) => self.queue(c),
                    Some(Control::Shutdown) | None => return Outcome::Shutdown,
                },
                _ = self.events.closed() => return Outcome::Stop,
            }
        }
    }

    async fn drive(&mut self, conn: &mut Conn) -> Outcome {
        let mut inflater = self.cfg.compress.then(ZlibStream::new);
        loop {
            let flush_at = (conn.connected && !self.pending.is_empty()).then(|| conn.limiter.ready_at(Instant::now()));
            tokio::select! {
                msg = conn.ws.next() => {
                    let outcome = match msg {
                        Some(Ok(Message::Text(text))) => self.on_payload(conn, text.as_bytes()).await,
                        Some(Ok(Message::Binary(bin))) => match inflater.as_mut() {
                            Some(z) => match z.push(&bin) {
                                Ok(Some(data)) => self.on_payload(conn, data).await,
                                Ok(None) => None,
                                Err(e) => {
                                    self.warn(format!("Gateway stream corrupted ({e}); reconnecting"));
                                    Some(Outcome::resume(Wait::Backoff))
                                }
                            },
                            None => self.on_payload(conn, &bin).await,
                        },
                        Some(Ok(Message::Close(frame))) => {
                            let code = frame.map(|f| u16::from(f.code));
                            tracing::info!(?code, "gateway closed by server");
                            Some(close_outcome(code))
                        }
                        Some(Ok(_)) => None,
                        Some(Err(e)) => {
                            tracing::warn!(error = %e, "gateway connection error");
                            Some(Outcome::resume(Wait::Backoff))
                        }
                        None => Some(Outcome::resume(Wait::Backoff)),
                    };
                    if let Some(o) = outcome {
                        return o;
                    }
                }
                _ = sleep_until(conn.next_beat) => {
                    if !conn.hello {
                        self.warn("Gateway did not send HELLO in time; reconnecting".into());
                        return Outcome::resume(Wait::Backoff);
                    }
                    if conn.ack_pending {
                        self.warn("Gateway heartbeat was not acknowledged; reconnecting".into());
                        return Outcome::resume(Wait::Backoff);
                    }
                    if let Err(o) = self.heartbeat(conn).await {
                        return o;
                    }
                    conn.next_beat = Instant::now() + conn.interval;
                }
                ctrl = self.rx.recv() => match ctrl {
                    Some(Control::Command(c)) => self.queue(c),
                    Some(Control::Shutdown) | None => return Outcome::Shutdown,
                },
                _ = sleep_until(flush_at.unwrap_or_else(Instant::now)), if flush_at.is_some() => {}
                _ = self.events.closed() => return Outcome::Stop,
            }
            if self.stopped {
                return Outcome::Stop;
            }
            if conn.connected
                && !self.pending.is_empty()
                && let Err(o) = self.flush(conn).await
            {
                return o;
            }
        }
    }

    async fn send_json(&mut self, conn: &mut Conn, v: &Value) -> Result<(), Outcome> {
        conn.ws.send(Message::Text(v.to_string().into())).await.map_err(|e| {
            tracing::warn!(error = %e, "gateway send failed");
            Outcome::resume(Wait::Backoff)
        })
    }

    async fn heartbeat(&mut self, conn: &mut Conn) -> Result<(), Outcome> {
        let payload = json!({ "op": 1, "d": self.seq });
        self.send_json(conn, &payload).await?;
        conn.ack_pending = true;
        conn.beat_sent_at = Some(Instant::now());
        Ok(())
    }

    async fn flush(&mut self, conn: &mut Conn) -> Result<(), Outcome> {
        while let Some(cmd) = self.pending.front() {
            if !conn.limiter.try_acquire(Instant::now()) {
                break;
            }
            let payload = cmd.to_json();
            self.pending.pop_front();
            self.send_json(conn, &payload).await?;
        }
        Ok(())
    }

    async fn identify_or_resume(&mut self, conn: &mut Conn) -> Result<(), Outcome> {
        let session_id = self.session.as_ref().map(|s| s.id.clone());
        let payload = match session_id {
            Some(id) => {
                self.emit(GatewayEvent::State(ConnectionState::Resuming));
                json!({ "op": 6, "d": { "token": self.cfg.token, "session_id": id, "seq": self.seq.unwrap_or(0) } })
            }
            None => {
                self.emit(GatewayEvent::State(ConnectionState::Identifying));
                self.seq = None;
                self.last_identify = Some(Instant::now());
                let p = &self.cfg.properties;
                let mut d = json!({
                    "token": self.cfg.token,
                    "intents": self.cfg.intents.0,
                    "properties": { "os": p.os, "browser": p.browser, "device": p.device },
                    "compress": false,
                    "large_threshold": self.cfg.large_threshold.clamp(50, 250),
                });
                if let Some(presence) = &self.presence {
                    d["presence"] = presence.to_json();
                }
                json!({ "op": 2, "d": d })
            }
        };
        conn.limiter.record(Instant::now());
        self.send_json(conn, &payload).await
    }

    async fn on_payload(&mut self, conn: &mut Conn, data: &[u8]) -> Option<Outcome> {
        let p: Payload = match serde_json::from_slice(data) {
            Ok(p) => p,
            Err(e) => {
                self.warn(format!("Ignored undecodable gateway payload: {e}"));
                return None;
            }
        };
        match p.op {
            0 => {
                if let Some(s) = p.s {
                    self.seq = Some(s);
                }
                let Some(t) = p.t else {
                    self.warn("Ignored dispatch without an event name".into());
                    return None;
                };
                self.on_dispatch(conn, &t, p.d.map(RawValue::get).unwrap_or("null"));
                None
            }
            1 => self.heartbeat(conn).await.err(),
            7 => {
                tracing::info!("gateway requested reconnect");
                Some(Outcome::resume(Wait::Immediate))
            }
            9 => {
                let resumable = p.d.is_some_and(|d| d.get().trim() == "true");
                let t = &self.cfg.timings;
                let delay = if t.invalid_session_max > t.invalid_session_min {
                    rand::random_range(t.invalid_session_min..t.invalid_session_max)
                } else {
                    t.invalid_session_min
                };
                tracing::info!(resumable, "gateway session invalidated");
                Some(Outcome::Reconnect { resume: resumable, wait: Wait::Fixed(delay) })
            }
            10 => {
                let hello: Option<Hello> = p.d.and_then(|d| serde_json::from_str(d.get()).ok());
                let Some(hello) = hello.filter(|h| h.heartbeat_interval > 0) else {
                    self.warn("Gateway HELLO had no heartbeat interval; reconnecting".into());
                    return Some(Outcome::resume(Wait::Backoff));
                };
                conn.interval = Duration::from_millis(hello.heartbeat_interval);
                conn.limiter.reserve_for_heartbeats(conn.interval);
                if !conn.hello {
                    conn.hello = true;
                    conn.next_beat = Instant::now() + conn.interval.mul_f64(rand::random::<f64>());
                    return self.identify_or_resume(conn).await.err();
                }
                None
            }
            11 => {
                conn.ack_pending = false;
                if let Some(sent) = conn.beat_sent_at.take() {
                    self.emit(GatewayEvent::Latency(sent.elapsed()));
                }
                None
            }
            op => {
                tracing::debug!(op, "ignoring unknown gateway opcode");
                None
            }
        }
    }

    fn on_dispatch(&mut self, conn: &mut Conn, t: &str, data: &str) {
        match Dispatch::parse(t, data) {
            Ok(dispatch) => {
                let state_change = match &dispatch {
                    Dispatch::Ready(r) => {
                        self.set_session(r.session_id.clone(), Some(r.resume_gateway_url.clone()));
                        true
                    }
                    Dispatch::Resumed => true,
                    _ => false,
                };
                self.emit(GatewayEvent::Dispatch(Box::new(dispatch)));
                if state_change {
                    self.mark_connected(conn);
                }
            }
            Err(e) => {
                if t == "READY" {
                    // Without READY's session we could never resume; salvage the essentials.
                    let min: ReadyMin = serde_json::from_str(data).unwrap_or_default();
                    if !min.session_id.is_empty() {
                        self.set_session(min.session_id, min.resume_gateway_url);
                    }
                    self.mark_connected(conn);
                }
                self.warn(format!("Ignored malformed {t} event: {e}"));
            }
        }
    }

    fn set_session(&mut self, id: String, resume_url: Option<String>) {
        if let Ok(mut s) = self.shared_session.lock() {
            *s = Some(id.clone());
        }
        let resume_url = resume_url.filter(|u| !u.trim().is_empty());
        self.session = Some(Session { id, resume_url });
    }

    fn mark_connected(&mut self, conn: &mut Conn) {
        conn.connected = true;
        self.attempt = 0;
        self.emit(GatewayEvent::State(ConnectionState::Connected));
    }

    /// Closes the socket. Code 1000 ends the session server-side, so it is only used
    /// when the session is not going to be resumed.
    async fn close(&mut self, mut conn: Conn, outcome: &Outcome) {
        let timeout = self.cfg.timings.close_timeout;
        let code = match outcome {
            Outcome::Shutdown | Outcome::Stop | Outcome::Reconnect { resume: false, .. } => 1000,
            Outcome::Reconnect { resume: true, .. } => CLOSE_RESUMABLE,
            Outcome::Fatal(_) => return,
        };
        let frame = CloseFrame { code: CloseCode::from(code), reason: "".into() };
        if tokio::time::timeout(timeout, conn.ws.close(Some(frame))).await.is_err() {
            return;
        }
        if code == 1000 {
            // Wait for the server's close reply so the handshake completes.
            let _ = tokio::time::timeout(timeout, async { while let Some(Ok(_)) = conn.ws.next().await {} }).await;
        }
    }
}

fn close_outcome(code: Option<u16>) -> Outcome {
    match code {
        Some(c) => match FatalError::from_close_code(c) {
            Some(fatal) => Outcome::Fatal(fatal),
            None if matches!(c, 4007 | 4009) => Outcome::Reconnect { resume: false, wait: Wait::Backoff },
            None => Outcome::resume(Wait::Backoff),
        },
        None => Outcome::resume(Wait::Backoff),
    }
}

fn gateway_url(base: &str, compress: bool) -> Result<String, String> {
    let mut url = url::Url::parse(base.trim()).map_err(|e| format!("invalid gateway URL: {e}"))?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err(format!("unsupported gateway URL scheme `{}`", url.scheme()));
    }
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("v", &dn_core::GATEWAY_VERSION.to_string()).append_pair("encoding", "json");
        if compress {
            q.append_pair("compress", "zlib-stream");
        }
    }
    Ok(url.into())
}

fn tls_config() -> Result<Arc<rustls::ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            rustls::ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map(|b| Arc::new(b.with_root_certificates(roots).with_no_client_auth()))
                .map_err(|e| format!("TLS setup failed: {e}"))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_building() {
        assert_eq!(
            gateway_url("wss://gateway.discord.gg", true).unwrap(),
            "wss://gateway.discord.gg/?v=10&encoding=json&compress=zlib-stream"
        );
        assert_eq!(
            gateway_url("wss://gateway-us-east1-b.discord.gg/?v=9", false).unwrap(),
            "wss://gateway-us-east1-b.discord.gg/?v=10&encoding=json"
        );
        assert!(gateway_url("https://discord.com", false).is_err());
    }

    #[test]
    fn close_code_mapping() {
        assert!(matches!(close_outcome(Some(4004)), Outcome::Fatal(FatalError::AuthenticationFailed)));
        assert!(matches!(close_outcome(Some(4014)), Outcome::Fatal(FatalError::DisallowedIntents)));
        assert!(matches!(close_outcome(Some(4009)), Outcome::Reconnect { resume: false, .. }));
        assert!(matches!(close_outcome(Some(4000)), Outcome::Reconnect { resume: true, .. }));
        assert!(matches!(close_outcome(Some(1001)), Outcome::Reconnect { resume: true, .. }));
    }
}
