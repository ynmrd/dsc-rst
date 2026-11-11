//! End-to-end backend tests against a local mock of Discord: wiremock for REST and an
//! in-process WebSocket server speaking the gateway v10 protocol. They drive the backend
//! exactly like the UI does (commands in, shared store + UI events out).

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as Ws;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use dn_cache::bus::{self, Command, UiEvent, UiLink};
use dn_cache::{ConnStatus, SharedStore, Store, StoreLimits};
use dn_core::Id;
use dn_settings::{Paths, Settings};

const BOT: &str = "100";
const GUILD: &str = "1";
const CHANNEL: u64 = 10;

enum GatewayMode {
    Normal,
    RejectToken,
}

/// Mock gateway. Returns its URL and a sender for extra dispatches (event name, payload).
async fn mock_gateway(mode: GatewayMode) -> (String, mpsc::UnboundedSender<(String, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (tx, mut rx) = mpsc::unbounded_channel::<(String, Value)>();
    tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else { return };
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        ws.send(Ws::text(json!({"op": 10, "d": {"heartbeat_interval": 45000}}).to_string())).await.unwrap();
        // Wait for IDENTIFY.
        loop {
            let Some(Ok(msg)) = ws.next().await else { return };
            if let Ws::Text(t) = msg {
                let v: Value = serde_json::from_str(&t).unwrap();
                if v["op"] == 2 {
                    assert!(v["d"]["token"].as_str().unwrap().contains("test-token"));
                    break;
                }
            }
        }
        if let GatewayMode::RejectToken = mode {
            let _ = ws.send(Ws::Close(Some(CloseFrame { code: CloseCode::from(4004), reason: "Authentication failed.".into() }))).await;
            return;
        }
        let mut seq = 0;
        let mut dispatch = |t: &str, d: Value| {
            seq += 1;
            Ws::text(json!({"op": 0, "s": seq, "t": t, "d": d}).to_string())
        };
        ws.send(dispatch(
            "READY",
            json!({"v": 10, "user": {"id": BOT, "username": "testbot", "bot": true}, "guilds": [{"id": GUILD, "unavailable": true}],
                   "session_id": "sess", "resume_gateway_url": "ws://127.0.0.1:1", "application": {"id": "77", "flags": 0}}),
        ))
        .await
        .unwrap();
        ws.send(dispatch(
            "GUILD_CREATE",
            json!({"id": GUILD, "name": "Mock Guild", "owner_id": "999", "member_count": 2,
                   "roles": [{"id": GUILD, "name": "@everyone", "permissions": "68608", "position": 0}],
                   "channels": [{"id": CHANNEL.to_string(), "type": 0, "name": "general", "position": 0, "last_message_id": "3"}],
                   "members": [{"user": {"id": BOT, "username": "testbot", "bot": true}, "roles": []},
                               {"user": {"id": "2", "username": "alice"}, "roles": []}]}),
        ))
        .await
        .unwrap();
        loop {
            tokio::select! {
                Some((t, d)) = rx.recv() => {
                    if ws.send(dispatch(&t, d)).await.is_err() { return; }
                }
                msg = ws.next() => match msg {
                    Some(Ok(Ws::Text(t))) => {
                        let v: Value = serde_json::from_str(&t).unwrap();
                        if v["op"] == 1 {
                            let _ = ws.send(Ws::text(json!({"op": 11}).to_string())).await;
                        }
                    }
                    Some(Ok(_)) => {}
                    _ => return,
                }
            }
        }
    });
    (url, tx)
}

fn msg_json(id: u64, author: &str, content: &str) -> Value {
    json!({"id": id.to_string(), "channel_id": CHANNEL.to_string(), "guild_id": GUILD,
           "author": {"id": author, "username": if author == BOT { "testbot" } else { "alice" }},
           "content": content, "timestamp": "2026-01-01T00:00:00+00:00", "type": 0})
}

/// Echoes POST /channels/{id}/messages with the request's nonce, like Discord does.
struct EchoCreate;

impl Respond for EchoCreate {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap_or_default();
        let mut m = msg_json(950, BOT, body["content"].as_str().unwrap_or_default());
        m["nonce"] = body["nonce"].clone();
        ResponseTemplate::new(200).set_body_json(m)
    }
}

async fn mock_rest(bot: bool) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": BOT, "username": "testbot", "bot": bot})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/applications/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "77", "name": "Test App"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/channels/{CHANNEL}/messages")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            msg_json(3, "2", "third"),
            msg_json(2, "2", "second"),
            msg_json(1, "2", "first")
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(path(format!("/channels/{CHANNEL}/messages"))).respond_with(EchoCreate).mount(&server).await;
    server
}

struct Harness {
    ui: UiLink,
    store: SharedStore,
    paths: Paths,
    backend: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

async fn start_backend(api: &str, gateway: &str) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::under(dir.path());
    paths.ensure().unwrap();
    let mut settings = Settings::default();
    settings.advanced.gateway_compression = false;
    let store = Store::shared(StoreLimits::default());
    let (ui, backend_link, cmd_rx) = bus::link(store.clone(), Arc::new(parking_lot::RwLock::new(settings)));
    let cfg = dn_client::BackendConfig {
        paths: paths.clone(),
        api_base: api.to_string(),
        gateway_url: Some(gateway.to_string()),
        gateway_timings: Some(dn_gateway::GatewayConfig::new("x", dn_gateway::Intents::GUILDS).test_timings().timings),
        secret_store: Arc::new(dn_auth::MemoryStore::default()),
        notifications: false,
        audio: false,
        oauth_api_base: api.to_string(),
        startup_warning: None,
    };
    let backend = dn_client::start(&tokio::runtime::Handle::current(), cfg, backend_link, cmd_rx);
    Harness { ui, store, paths, backend, _dir: dir }
}

async fn wait_for(store: &SharedStore, what: &str, cond: impl Fn(&Store) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        if cond(&store.lock()) {
            return;
        }
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn next_event(ui: &UiLink, what: &str) -> UiEvent {
    ui.events.recv_timeout(Duration::from_secs(10)).unwrap_or_else(|_| panic!("no UI event: {what}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_session_against_mock_discord() {
    let rest = mock_rest(true).await;
    let (gw_url, gw) = mock_gateway(GatewayMode::Normal).await;
    let h = start_backend(&rest.uri(), &gw_url).await;

    wait_for(&h.store, "onboarding (no accounts)", |s| s.session.needs_onboarding).await;
    h.ui.send(Command::AddBotAccount { token: "Bot test-token".into() });
    loop {
        match next_event(&h.ui, "account added") {
            UiEvent::AccountAdded(r) => {
                assert_eq!(r.unwrap(), "testbot");
                break;
            }
            _ => continue,
        }
    }
    wait_for(&h.store, "READY + GUILD_CREATE", |s| s.session.ready && s.guilds.get(&Id(1)).is_some_and(|g| g.available)).await;
    {
        let s = h.store.lock();
        assert_eq!(s.session.conn, ConnStatus::Connected);
        assert!(!s.session.needs_onboarding);
        assert_eq!(s.session.accounts.len(), 1);
        assert!(s.permissions_in(Id(CHANNEL)).can_send());
    }

    // History over REST.
    h.store.lock().set_active_channel(Some(Id(CHANNEL)));
    h.ui.send(Command::OpenChannel { channel_id: Id(CHANNEL) });
    wait_for(&h.store, "history page", |s| s.channel_messages(Id(CHANNEL)).is_some_and(|cm| cm.messages.len() == 3)).await;
    assert!(h.store.lock().channel_messages(Id(CHANNEL)).unwrap().reached_start);

    // Sending: optimistic pending entry replaced by the server's message (nonce match).
    h.ui.send(Command::SendMessage {
        channel_id: Id(CHANNEL),
        content: "hello from the test".into(),
        reply_to: None,
        mention_reply: false,
        files: vec![],
        sticker_ids: vec![],
    });
    wait_for(&h.store, "sent message", |s| {
        s.channel_messages(Id(CHANNEL))
            .is_some_and(|cm| cm.get(Id(950)).is_some_and(|m| m.content == "hello from the test") && cm.pending.is_empty())
    })
    .await;

    // Live message from another user over the gateway.
    gw.send(("MESSAGE_CREATE".into(), msg_json(960, "2", "live message"))).unwrap();
    wait_for(&h.store, "live message", |s| s.channel_messages(Id(CHANNEL)).is_some_and(|cm| cm.get(Id(960)).is_some())).await;
    assert!(h.store.lock().is_unread(Id(CHANNEL)));
    h.ui.send(Command::MarkRead { channel_id: Id(CHANNEL), message_id: Id(960) });
    wait_for(&h.store, "marked read", |s| !s.is_unread(Id(CHANNEL))).await;

    // Graceful shutdown flushes to SQLite.
    h.ui.send(Command::Shutdown);
    tokio::time::timeout(Duration::from_secs(10), h.backend).await.expect("backend shut down").unwrap();
    let db = dn_storage::Storage::open(&h.paths.database_file()).unwrap();
    let ids: Vec<u64> = db.load_messages(Id(CHANNEL), None, 100).unwrap().iter().map(|m| m.id.0).collect();
    assert!(ids.contains(&960), "live message persisted: {ids:?}");
    assert!(ids.contains(&1) && ids.contains(&3), "history persisted: {ids:?}");
    assert_eq!(db.read_states().unwrap().get(&Id(CHANNEL)), Some(&Id(960)));
    // The bot token never reaches the accounts file on disk.
    let accounts = std::fs::read_to_string(h.paths.accounts_file()).unwrap();
    assert!(!accounts.contains("test-token"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_token_marks_account_invalid() {
    let rest = mock_rest(true).await;
    let (gw_url, _gw) = mock_gateway(GatewayMode::RejectToken).await;
    let h = start_backend(&rest.uri(), &gw_url).await;
    h.ui.send(Command::AddBotAccount { token: "test-token".into() });
    wait_for(&h.store, "fatal state", |s| matches!(s.session.conn, ConnStatus::Fatal(_))).await;
    wait_for(&h.store, "credentials invalid", |s| s.session.accounts.iter().any(|a| a.credentials_invalid) && s.session.needs_onboarding)
        .await;
    h.ui.send(Command::Shutdown);
    tokio::time::timeout(Duration::from_secs(10), h.backend).await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_tokens_are_refused() {
    let rest = mock_rest(false).await;
    let (gw_url, _gw) = mock_gateway(GatewayMode::Normal).await;
    let h = start_backend(&rest.uri(), &gw_url).await;
    h.ui.send(Command::AddBotAccount { token: "test-token".into() });
    loop {
        match next_event(&h.ui, "account rejected") {
            UiEvent::AccountAdded(r) => {
                assert!(r.is_err());
                break;
            }
            _ => continue,
        }
    }
    assert!(h.store.lock().session.accounts.is_empty());
    h.ui.send(Command::Shutdown);
    tokio::time::timeout(Duration::from_secs(10), h.backend).await.unwrap().unwrap();
}
