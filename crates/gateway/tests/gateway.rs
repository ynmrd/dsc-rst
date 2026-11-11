//! Protocol tests against an in-process WebSocket server.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use dn_core::{Dispatch, Id};
use dn_gateway::{
    ConnectionState, FatalError, GatewayCommand, GatewayConfig, GatewayEvent, GatewayHandle, Intents, VoiceStateUpdate, spawn,
};
use flate2::{Compress, Compression, FlushCompress};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::time::timeout;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::protocol::CloseFrame;

type ServerWs = WebSocketStream<TcpStream>;
const STEP: Duration = Duration::from_secs(5);
const TOKEN: &str = "test-bot-token";

struct TestServer {
    url: String,
    listener: TcpListener,
}

impl TestServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        TestServer { url, listener }
    }

    /// Accepts one WebSocket connection and returns it with the request URI.
    // The callback's Result type is fixed by tungstenite's `Callback` trait.
    #[allow(clippy::result_large_err)]
    async fn accept(&self) -> (ServerWs, String) {
        let (tcp, _) = timeout(STEP, self.listener.accept()).await.expect("no connection").unwrap();
        let uri = Arc::new(Mutex::new(String::new()));
        let seen = uri.clone();
        let ws = tokio_tungstenite::accept_hdr_async(tcp, move |req: &Request, resp: Response| {
            *seen.lock().unwrap() = req.uri().to_string();
            Ok(resp)
        })
        .await
        .unwrap();
        let uri = uri.lock().unwrap().clone();
        (ws, uri)
    }

    async fn assert_no_connection(&self, wait: Duration) {
        assert!(timeout(wait, self.listener.accept()).await.is_err(), "client reconnected unexpectedly");
    }
}

fn config(server: &TestServer) -> GatewayConfig {
    GatewayConfig { url: Some(server.url.clone()), compress: false, ..GatewayConfig::new(TOKEN, Intents::default_client()) }.test_timings()
}

async fn send(ws: &mut ServerWs, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

async fn recv(ws: &mut ServerWs) -> Value {
    loop {
        match timeout(STEP, ws.next()).await.expect("timed out waiting for client frame") {
            Some(Ok(Message::Text(t))) => return serde_json::from_str(&t).unwrap(),
            Some(Ok(Message::Close(f))) => panic!("client closed: {f:?}"),
            Some(Ok(_)) => continue,
            other => panic!("connection ended: {other:?}"),
        }
    }
}

/// Next client payload with `op`, skipping others (e.g. heartbeats).
async fn recv_op(ws: &mut ServerWs, op: u64) -> Value {
    loop {
        let v = recv(ws).await;
        if v["op"] == op {
            return v;
        }
    }
}

/// Reads until the client's close frame and returns its code.
async fn recv_close(ws: &mut ServerWs) -> Option<u16> {
    loop {
        match timeout(STEP, ws.next()).await.expect("timed out waiting for close") {
            Some(Ok(Message::Close(f))) => return f.map(|f| u16::from(f.code)),
            Some(Ok(_)) => continue,
            _ => return None,
        }
    }
}

fn hello(interval_ms: u64) -> Value {
    json!({ "op": 10, "s": null, "t": null, "d": { "heartbeat_interval": interval_ms } })
}

fn ready(session: &str, resume_url: &str) -> Value {
    json!({
        "op": 0, "s": 1, "t": "READY",
        "d": {
            "v": 10,
            "user": { "id": "100", "username": "testbot", "discriminator": "0", "bot": true },
            "guilds": [{ "id": "10", "unavailable": true }],
            "session_id": session,
            "resume_gateway_url": resume_url,
            "application": { "id": "100", "flags": 0 }
        }
    })
}

fn message_create(seq: u64, id: u64, content: &str) -> Value {
    json!({
        "op": 0, "s": seq, "t": "MESSAGE_CREATE",
        "d": {
            "id": id.to_string(), "channel_id": "20", "guild_id": "10",
            "author": { "id": "5", "username": "alice", "discriminator": "0" },
            "content": content, "timestamp": "2026-11-11T10:00:00.000000+00:00", "type": 0
        }
    })
}

async fn next_event(rx: &mut UnboundedReceiver<GatewayEvent>) -> GatewayEvent {
    timeout(STEP, rx.recv()).await.expect("timed out waiting for gateway event").expect("event channel closed")
}

/// Skips events until `pred` matches.
async fn wait_for(rx: &mut UnboundedReceiver<GatewayEvent>, pred: impl Fn(&GatewayEvent) -> bool) -> GatewayEvent {
    loop {
        let ev = next_event(rx).await;
        if pred(&ev) {
            return ev;
        }
    }
}

fn is_state(ev: &GatewayEvent, s: ConnectionState) -> bool {
    matches!(ev, GatewayEvent::State(x) if *x == s)
}

async fn shutdown_and_expect_1000(handle: GatewayHandle, mut ws: ServerWs) {
    let reader = tokio::spawn(async move { recv_close(&mut ws).await });
    timeout(STEP, handle.shutdown()).await.expect("shutdown hung");
    assert_eq!(reader.await.unwrap(), Some(1000));
}

fn zlib_frames(c: &mut Compress, v: &Value) -> Vec<u8> {
    let data = v.to_string().into_bytes();
    let mut out = Vec::with_capacity(data.len() + 1024);
    let start = c.total_in();
    loop {
        let consumed = (c.total_in() - start) as usize;
        c.compress_vec(&data[consumed..], &mut out, FlushCompress::Sync).unwrap();
        if (c.total_in() - start) as usize == data.len() && out.len() < out.capacity() {
            break;
        }
        out.reserve(out.capacity().max(1024));
    }
    assert!(out.ends_with(&[0, 0, 0xff, 0xff]));
    out
}

#[tokio::test]
async fn identify_ready_and_dispatch() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(config(&server), tx);
    let (mut ws, uri) = server.accept().await;
    assert_eq!(uri, "/?v=10&encoding=json");
    assert!(is_state(&next_event(&mut rx).await, ConnectionState::Connecting));

    send(&mut ws, hello(45_000)).await;
    let identify = recv_op(&mut ws, 2).await;
    let d = &identify["d"];
    assert_eq!(d["token"], TOKEN);
    assert_eq!(d["intents"], Intents::default_client().0);
    // GUILDS|GUILD_MEMBERS|GUILD_EXPRESSIONS|bits 7..=15|GUILD_MESSAGE_POLLS|DIRECT_MESSAGE_POLLS
    assert_eq!(d["intents"], 50_397_067u64);
    assert_eq!(d["properties"], json!({ "os": std::env::consts::OS, "browser": "DiscordNative", "device": "DiscordNative" }));
    assert_eq!(d["compress"], false);
    assert_eq!(d["large_threshold"], 250);
    assert!(is_state(&next_event(&mut rx).await, ConnectionState::Identifying));

    send(&mut ws, ready("sess-1", &server.url)).await;
    match next_event(&mut rx).await {
        GatewayEvent::Dispatch(d) => match *d {
            Dispatch::Ready(r) => {
                assert_eq!(r.session_id, "sess-1");
                assert_eq!(r.user.username, "testbot");
            }
            other => panic!("expected READY, got {other:?}"),
        },
        other => panic!("expected dispatch, got {other:?}"),
    }
    assert!(is_state(&next_event(&mut rx).await, ConnectionState::Connected));
    assert_eq!(handle.session_id().as_deref(), Some("sess-1"));

    // A malformed dispatch only produces a warning; the connection stays up.
    send(&mut ws, json!({ "op": 0, "s": 2, "t": "MESSAGE_CREATE", "d": { "id": "not-a-snowflake" } })).await;
    assert!(matches!(next_event(&mut rx).await, GatewayEvent::Warning(w) if w.contains("MESSAGE_CREATE")));

    send(&mut ws, message_create(3, 555, "hello there")).await;
    match next_event(&mut rx).await {
        GatewayEvent::Dispatch(d) => match *d {
            Dispatch::MessageCreate(m) => {
                assert_eq!(m.id, Id(555));
                assert_eq!(m.content, "hello there");
            }
            other => panic!("expected MESSAGE_CREATE, got {other:?}"),
        },
        other => panic!("expected dispatch, got {other:?}"),
    }

    shutdown_and_expect_1000(handle, ws).await;
    wait_for(&mut rx, |e| is_state(e, ConnectionState::Disconnected)).await;
}

#[tokio::test]
async fn zlib_stream_with_split_frames() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(GatewayConfig { compress: true, ..config(&server) }, tx);
    let (mut ws, uri) = server.accept().await;
    assert_eq!(uri, "/?v=10&encoding=json&compress=zlib-stream");

    let mut z = Compress::new(Compression::default(), true);
    ws.send(Message::Binary(zlib_frames(&mut z, &hello(45_000)).into())).await.unwrap();
    recv_op(&mut ws, 2).await;

    // READY split across two binary frames; only the second ends with the sync marker.
    let frames = zlib_frames(&mut z, &ready("sess-z", &server.url));
    let (a, b) = frames.split_at(frames.len() / 2);
    ws.send(Message::Binary(a.to_vec().into())).await.unwrap();
    ws.send(Message::Binary(b.to_vec().into())).await.unwrap();
    // A large payload that depends on the shared compression context.
    let long = "lorem ipsum dolor sit amet ".repeat(2000);
    ws.send(Message::Binary(zlib_frames(&mut z, &message_create(2, 9, &long)).into())).await.unwrap();

    let ev = wait_for(&mut rx, |e| matches!(e, GatewayEvent::Dispatch(_))).await;
    assert!(matches!(ev, GatewayEvent::Dispatch(d) if matches!(&*d, Dispatch::Ready(r) if r.session_id == "sess-z")));
    let ev = wait_for(&mut rx, |e| matches!(e, GatewayEvent::Dispatch(_))).await;
    assert!(matches!(ev, GatewayEvent::Dispatch(d) if matches!(&*d, Dispatch::MessageCreate(m) if m.content == long)));

    shutdown_and_expect_1000(handle, ws).await;
}

#[tokio::test]
async fn heartbeat_ack_reports_latency() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(config(&server), tx);
    let (mut ws, _) = server.accept().await;
    send(&mut ws, hello(50)).await;
    recv_op(&mut ws, 2).await;
    send(&mut ws, ready("sess-hb", &server.url)).await;

    // The jittered first beat may precede READY (d: null); later ones carry seq 1.
    loop {
        let beat = recv_op(&mut ws, 1).await;
        send(&mut ws, json!({ "op": 11 })).await;
        if !beat["d"].is_null() {
            assert_eq!(beat["d"], 1, "heartbeat must carry the last sequence number");
            break;
        }
    }
    let ev = wait_for(&mut rx, |e| matches!(e, GatewayEvent::Latency(_))).await;
    let GatewayEvent::Latency(rtt) = ev else { unreachable!() };
    assert!(rtt < Duration::from_secs(1));

    // Server-requested heartbeat (op 1) is answered.
    send(&mut ws, json!({ "op": 1, "d": null })).await;
    recv_op(&mut ws, 1).await;
    send(&mut ws, json!({ "op": 11 })).await;

    // Keep ACKing; the connection must stay up with periodic beats.
    for _ in 0..3 {
        recv_op(&mut ws, 1).await;
        send(&mut ws, json!({ "op": 11 })).await;
    }
    shutdown_and_expect_1000(handle, ws).await;
    let mut reconnected = false;
    while let Ok(Some(ev)) = timeout(Duration::from_millis(50), rx.recv()).await {
        reconnected |= matches!(ev, GatewayEvent::State(ConnectionState::Reconnecting { .. }));
    }
    assert!(!reconnected, "acknowledged heartbeats must not trigger a reconnect");
}

#[tokio::test]
async fn reconnect_opcode_resumes_session() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(config(&server), tx);
    let (mut ws, _) = server.accept().await;
    send(&mut ws, hello(45_000)).await;
    recv_op(&mut ws, 2).await;
    send(&mut ws, ready("sess-7", &server.url)).await;
    send(&mut ws, message_create(5, 1, "before reconnect")).await;
    wait_for(&mut rx, |e| matches!(e, GatewayEvent::Dispatch(d) if matches!(**d, Dispatch::MessageCreate(_)))).await;

    send(&mut ws, json!({ "op": 7, "d": null })).await;
    let (mut ws2, uri) = server.accept().await;
    assert_eq!(uri, "/?v=10&encoding=json");
    assert_ne!(recv_close(&mut ws).await, Some(1000), "closing with 1000 would invalidate the session");

    send(&mut ws2, hello(45_000)).await;
    let resume = recv(&mut ws2).await;
    assert_eq!(resume["op"], 6, "expected RESUME, got {resume}");
    assert_eq!(resume["d"], json!({ "token": TOKEN, "session_id": "sess-7", "seq": 5 }));
    wait_for(&mut rx, |e| is_state(e, ConnectionState::Resuming)).await;

    send(&mut ws2, json!({ "op": 0, "s": 6, "t": "RESUMED", "d": {} })).await;
    wait_for(&mut rx, |e| matches!(e, GatewayEvent::Dispatch(d) if **d == Dispatch::Resumed)).await;
    wait_for(&mut rx, |e| is_state(e, ConnectionState::Connected)).await;
    assert_eq!(handle.session_id().as_deref(), Some("sess-7"));
    shutdown_and_expect_1000(handle, ws2).await;
}

#[tokio::test]
async fn invalid_session_false_reidentifies() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(config(&server), tx);
    let (mut ws, _) = server.accept().await;
    send(&mut ws, hello(45_000)).await;
    recv_op(&mut ws, 2).await;
    send(&mut ws, ready("sess-9", &server.url)).await;
    wait_for(&mut rx, |e| is_state(e, ConnectionState::Connected)).await;

    send(&mut ws, json!({ "op": 9, "d": false })).await;
    let ev = wait_for(&mut rx, |e| matches!(e, GatewayEvent::State(ConnectionState::Reconnecting { .. }))).await;
    let GatewayEvent::State(ConnectionState::Reconnecting { delay, .. }) = ev else { unreachable!() };
    assert!(delay >= Duration::from_millis(10), "invalid session must wait before re-identifying: {delay:?}");
    assert_eq!(handle.session_id(), None);

    let (mut ws2, _) = server.accept().await;
    send(&mut ws2, hello(45_000)).await;
    let next = recv(&mut ws2).await;
    assert_eq!(next["op"], 2, "expected a fresh IDENTIFY, got {next}");
    assert_eq!(next["d"]["token"], TOKEN);
    wait_for(&mut rx, |e| is_state(e, ConnectionState::Identifying)).await;
    shutdown_and_expect_1000(handle, ws2).await;
}

#[tokio::test]
async fn authentication_failure_is_fatal() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(config(&server), tx);
    let (mut ws, _) = server.accept().await;
    send(&mut ws, hello(45_000)).await;
    recv_op(&mut ws, 2).await;
    ws.close(Some(CloseFrame { code: 4004.into(), reason: "Authentication failed.".into() })).await.unwrap();

    let ev = wait_for(&mut rx, |e| matches!(e, GatewayEvent::Fatal(_))).await;
    assert_eq!(ev, GatewayEvent::Fatal(FatalError::AuthenticationFailed));
    assert!(is_state(&next_event(&mut rx).await, ConnectionState::Disconnected));
    server.assert_no_connection(Duration::from_millis(300)).await;
    assert!(timeout(STEP, rx.recv()).await.unwrap().is_none(), "task should have ended");
    assert!(handle.is_finished());
    assert!(!handle.send(GatewayCommand::UpdateVoiceState(VoiceStateUpdate {
        guild_id: Id(1),
        channel_id: None,
        self_mute: false,
        self_deaf: false,
    })));
}

#[tokio::test]
async fn zombie_connection_reconnects_and_resumes() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(config(&server), tx);
    let (mut ws, _) = server.accept().await;
    send(&mut ws, hello(50)).await;
    recv_op(&mut ws, 2).await;
    send(&mut ws, ready("sess-zombie", &server.url)).await;
    // Never ACK: the client must give up on this connection.

    let (mut ws2, _) = server.accept().await;
    wait_for(&mut rx, |e| matches!(e, GatewayEvent::Warning(w) if w.contains("not acknowledged"))).await;
    send(&mut ws2, hello(45_000)).await;
    let resume = recv(&mut ws2).await;
    assert_eq!(resume["op"], 6, "expected RESUME, got {resume}");
    assert_eq!(resume["d"]["session_id"], "sess-zombie");
    assert_eq!(resume["d"]["seq"], 1);
    assert_ne!(recv_close(&mut ws).await, Some(1000));
    shutdown_and_expect_1000(handle, ws2).await;
}

#[tokio::test]
async fn voice_state_update_payload() {
    let server = TestServer::start().await;
    let (tx, mut rx) = unbounded_channel();
    let handle = spawn(config(&server), tx);
    let (mut ws, _) = server.accept().await;
    send(&mut ws, hello(45_000)).await;
    recv_op(&mut ws, 2).await;

    // Queued before READY; must be sent once connected.
    assert!(handle.send(GatewayCommand::UpdateVoiceState(VoiceStateUpdate {
        guild_id: Id(10),
        channel_id: Some(Id(30)),
        self_mute: false,
        self_deaf: true,
    })));
    send(&mut ws, ready("sess-v", &server.url)).await;
    let join = recv_op(&mut ws, 4).await;
    assert_eq!(join["d"], json!({ "guild_id": "10", "channel_id": "30", "self_mute": false, "self_deaf": true }));
    wait_for(&mut rx, |e| is_state(e, ConnectionState::Connected)).await;

    handle.send(GatewayCommand::UpdateVoiceState(VoiceStateUpdate {
        guild_id: Id(10),
        channel_id: None,
        self_mute: true,
        self_deaf: false,
    }));
    let leave = recv_op(&mut ws, 4).await;
    assert_eq!(leave["d"], json!({ "guild_id": "10", "channel_id": null, "self_mute": true, "self_deaf": false }));
    assert!(leave["d"].as_object().unwrap().contains_key("channel_id"));
    shutdown_and_expect_1000(handle, ws).await;
}

/// Connects to the real gateway with a well-formed but fake bot token.
#[tokio::test]
#[ignore = "connects to the real Discord gateway"]
async fn live_gateway_rejects_invalid_token() {
    let (tx, mut rx) = unbounded_channel();
    let cfg = GatewayConfig {
        url: Some(dn_gateway::DEFAULT_GATEWAY_URL.into()),
        compress: true,
        ..GatewayConfig::new("MTAwMDAwMDAwMDAwMDAwMDAw.GxXxXx.abcdefghijklmnopqrstuvwxyz0123456789AB", Intents::GUILDS)
    };
    let handle = spawn(cfg, tx);
    let mut identified_after_hello = false;
    let fatal = timeout(Duration::from_secs(30), async {
        while let Some(ev) = rx.recv().await {
            eprintln!("live gateway event: {ev:?}");
            match ev {
                // IDENTIFY is only sent in response to HELLO.
                GatewayEvent::State(ConnectionState::Identifying) => identified_after_hello = true,
                GatewayEvent::State(ConnectionState::Reconnecting { .. }) => panic!("unexpected reconnect"),
                GatewayEvent::Fatal(f) => return Some(f),
                _ => {}
            }
        }
        None
    })
    .await
    .expect("no response from the Discord gateway within 30 s");
    assert!(identified_after_hello, "HELLO was not received");
    assert_eq!(fatal, Some(FatalError::AuthenticationFailed));
    timeout(STEP, handle.shutdown()).await.unwrap();
}
