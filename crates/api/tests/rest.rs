use std::time::{Duration, Instant};

use dn_api::{ApiError, Auth, CreateMessage, FileUpload, MessagePage, RestClient, SearchOutcome, SearchQuery};
use dn_core::{Emoji, Id};
use serde_json::{Value, json};
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const TOKEN: &str = "test-token-value";

async fn setup() -> (MockServer, RestClient) {
    let server = MockServer::start().await;
    let client = RestClient::with_base_url(Auth::Bot(TOKEN.into()), &server.uri()).unwrap();
    (server, client)
}

fn message_json(id: u64, channel: u64, content: &str) -> Value {
    json!({
        "id": id.to_string(),
        "channel_id": channel.to_string(),
        "author": { "id": "42", "username": "alice", "discriminator": "0" },
        "content": content,
        "timestamp": "2026-11-01T12:00:00.000000+00:00",
        "type": 0
    })
}

#[tokio::test]
async fn current_user_sends_auth_and_user_agent() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .and(header("authorization", format!("Bot {TOKEN}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "7", "username": "bot", "bot": true})))
        .expect(1)
        .mount(&server)
        .await;
    let user = client.current_user().await.unwrap();
    assert_eq!(user.id, Id(7));
    assert!(user.bot);
    // Checked by hand: wiremock's `header` matcher splits values on commas.
    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs[0].headers.get("user-agent").unwrap().to_str().unwrap(), dn_core::user_agent());
}

#[tokio::test]
async fn bearer_auth_header() {
    let server = MockServer::start().await;
    let client = RestClient::with_base_url(Auth::Bearer("oauth-access".into()), &server.uri()).unwrap();
    Mock::given(method("GET"))
        .and(path("/users/@me/guilds/5/member"))
        .and(header("authorization", "Bearer oauth-access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"nick": "nick", "roles": ["1", "2"]})))
        .expect(1)
        .mount(&server)
        .await;
    let m = client.current_user_guild_member(Id(5)).await.unwrap();
    assert_eq!(m.nick.as_deref(), Some("nick"));
    assert_eq!(m.roles, vec![Id(1), Id(2)]);
}

#[tokio::test]
async fn messages_page_query_and_order() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/channels/10/messages"))
        .and(query_param("limit", "50"))
        .and(query_param("before", "999"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([message_json(3, 10, "newest"), message_json(2, 10, "older")])))
        .expect(1)
        .mount(&server)
        .await;
    let msgs = client.messages(Id(10), MessagePage::before(Id(999), 50)).await.unwrap();
    assert_eq!(msgs.iter().map(|m| m.id.0).collect::<Vec<_>>(), vec![3, 2]);
    assert_eq!(msgs[0].content, "newest");
}

#[tokio::test]
async fn guilds_are_paginated() {
    let (server, client) = setup().await;
    let first: Vec<Value> = (1..=200).map(|i| json!({"id": i.to_string(), "name": format!("g{i}")})).collect();
    Mock::given(method("GET"))
        .and(path("/users/@me/guilds"))
        .and(query_param("after", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": "201", "name": "last"}])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/@me/guilds"))
        .and(query_param("limit", "200"))
        .respond_with(ResponseTemplate::new(200).set_body_json(Value::Array(first)))
        .expect(1)
        .mount(&server)
        .await;
    let guilds = client.current_user_guilds().await.unwrap();
    assert_eq!(guilds.len(), 201);
    assert_eq!(guilds.last().unwrap().name, "last");
}

#[tokio::test]
async fn gateway_bot_and_application_and_threads() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/gateway/bot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "url": "wss://gateway.discord.gg", "shards": 1,
            "session_start_limit": {"total": 1000, "remaining": 999, "reset_after": 14400000, "max_concurrency": 1}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/applications/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "11", "name": "App", "bot_public": true, "flags": 8192})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/guilds/3/threads/active"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"threads": [{"id": "77", "type": 11, "name": "t"}], "members": []})))
        .mount(&server)
        .await;
    let gw = client.gateway_bot().await.unwrap();
    assert_eq!(gw.session_start_limit.remaining, 999);
    assert_eq!(gw.session_start_limit.max_concurrency, 1);
    let app = client.current_application().await.unwrap();
    assert_eq!((app.id, app.name.as_str(), app.bot_public, app.flags), (Id(11), "App", true, 8192));
    let threads = client.active_threads(Id(3)).await.unwrap();
    assert_eq!(threads[0].id, Id(77));
}

#[tokio::test]
async fn create_message_json_body() {
    let (server, client) = setup().await;
    Mock::given(method("POST"))
        .and(path("/channels/10/messages"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({
            "content": "hello",
            "tts": false,
            "allowed_mentions": {"parse": ["users", "roles", "everyone"], "replied_user": true},
            "nonce": "n1",
            "enforce_nonce": true,
            "message_reference": {"message_id": "5", "fail_if_not_exists": false}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(message_json(6, 10, "hello")))
        .expect(1)
        .mount(&server)
        .await;
    let msg = CreateMessage {
        content: "hello".into(),
        nonce: Some("n1".into()),
        reply_to: Some(Id(5)),
        mention_replied_user: true,
        ..Default::default()
    };
    let sent = client.create_message(Id(10), &msg).await.unwrap();
    assert_eq!(sent.id, Id(6));
    assert!(matches!(client.create_message(Id(10), &CreateMessage::default()).await, Err(ApiError::Invalid(_))));
}

#[tokio::test]
async fn multipart_upload() {
    let (server, client) = setup().await;
    Mock::given(method("POST"))
        .and(path("/channels/10/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(message_json(8, 10, "pic")))
        .expect(1)
        .mount(&server)
        .await;
    let msg = CreateMessage {
        content: "pic".into(),
        files: vec![
            FileUpload { filename: "cat.png".into(), bytes: b"PNGDATA".to_vec(), content_type: None, spoiler: false },
            FileUpload { filename: "secret.txt".into(), bytes: b"hidden".to_vec(), content_type: Some("text/plain".into()), spoiler: true },
        ],
        ..Default::default()
    };
    client.create_message(Id(10), &msg).await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    assert_eq!(reqs.len(), 1);
    let ct = reqs[0].headers.get("content-type").unwrap().to_str().unwrap().to_string();
    assert!(ct.starts_with("multipart/form-data; boundary="), "{ct}");
    let body = String::from_utf8_lossy(&reqs[0].body).to_string();
    assert!(body.contains("name=\"payload_json\""), "{body}");
    assert!(body.contains("name=\"files[0]\"; filename=\"cat.png\""), "{body}");
    assert!(body.contains("Content-Type: image/png"), "{body}");
    assert!(body.contains("PNGDATA"));
    assert!(body.contains("name=\"files[1]\"; filename=\"SPOILER_secret.txt\""), "{body}");
    let start = body.find("name=\"payload_json\"").unwrap();
    let json_start = start + body[start..].find("\r\n\r\n").unwrap() + 4;
    let json_end = json_start + body[json_start..].find("\r\n--").unwrap();
    let payload: Value = serde_json::from_str(&body[json_start..json_end]).unwrap();
    assert_eq!(payload["content"], "pic");
    assert_eq!(payload["attachments"], json!([{"id": 0, "filename": "cat.png"}, {"id": 1, "filename": "SPOILER_secret.txt"}]));
}

#[tokio::test]
async fn retries_after_429() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/channels/1"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .insert_header("x-ratelimit-scope", "user")
                .set_body_json(json!({"message": "You are being rate limited.", "retry_after": 0.05, "global": false})),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/channels/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "1", "type": 0, "name": "general"})))
        .mount(&server)
        .await;
    let start = Instant::now();
    let ch = client.channel(Id(1)).await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(ch.name.as_deref(), Some("general"));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert!(elapsed >= Duration::from_millis(50), "{elapsed:?}");
    // Must use the precise JSON value, not the rounded 1 s header.
    assert!(elapsed < Duration::from_millis(800), "{elapsed:?}");
}

#[tokio::test]
async fn gives_up_after_three_429_retries() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/channels/1"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({"retry_after": 0.01, "global": false})))
        .mount(&server)
        .await;
    let err = client.channel(Id(1)).await.unwrap_err();
    assert!(matches!(err, ApiError::RateLimited { global: false, .. }), "{err:?}");
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
}

#[tokio::test]
async fn global_429_blocks_other_routes() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/channels/1"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({"retry_after": 0.2, "global": true})))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/channels/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "1"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "2"})))
        .mount(&server)
        .await;
    client.channel(Id(1)).await.unwrap();
    // The global block has expired by now; trigger a fresh one and race another route.
    Mock::given(method("GET"))
        .and(path("/guilds/9/channels"))
        .respond_with(ResponseTemplate::new(429).set_body_json(json!({"retry_after": 0.3, "global": true})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/guilds/9/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let c2 = client.clone();
    let start = Instant::now();
    let a = tokio::spawn(async move { c2.guild_channels(Id(9)).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    client.current_user().await.unwrap();
    assert!(start.elapsed() >= Duration::from_millis(280), "{:?}", start.elapsed());
    a.await.unwrap().unwrap();
}

#[tokio::test]
async fn exhausted_bucket_delays_next_request() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/channels/1/messages/2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ratelimit-bucket", "abcd")
                .insert_header("x-ratelimit-limit", "5")
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset-after", "0.3")
                .set_body_json(message_json(2, 1, "x")),
        )
        .mount(&server)
        .await;
    client.message(Id(1), Id(2)).await.unwrap();
    let start = Instant::now();
    client.message(Id(1), Id(2)).await.unwrap();
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_millis(280), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
}

#[tokio::test]
async fn concurrent_requests_share_bucket() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/channels/1/messages/2"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-ratelimit-bucket", "abcd")
                .insert_header("x-ratelimit-remaining", "0")
                .insert_header("x-ratelimit-reset-after", "0.15")
                .set_body_json(message_json(2, 1, "x")),
        )
        .mount(&server)
        .await;
    let start = Instant::now();
    let tasks: Vec<_> = (0..3)
        .map(|_| {
            let c = client.clone();
            tokio::spawn(async move { c.message(Id(1), Id(2)).await })
        })
        .collect();
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    // Three serialized requests with two waits of 150 ms between them.
    assert!(start.elapsed() >= Duration::from_millis(280), "{:?}", start.elapsed());
}

#[tokio::test]
async fn unauthorized_is_not_retried() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"message": "401: Unauthorized", "code": 0})))
        .expect(1)
        .mount(&server)
        .await;
    assert_eq!(client.current_user().await.unwrap_err(), ApiError::Unauthorized);
}

#[tokio::test]
async fn forbidden_and_not_found_and_form_errors() {
    let (server, client) = setup().await;
    Mock::given(method("DELETE"))
        .and(path("/channels/1/messages/2"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"message": "Missing Permissions", "code": 50013})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/channels/404"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"message": "Unknown Channel", "code": 10003})))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/channels/1/messages/3"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "code": 50035, "message": "Invalid Form Body",
            "errors": {"content": {"_errors": [{"code": "BASE_TYPE_MAX_LENGTH", "message": "Must be 2000 or fewer in length."}]}}
        })))
        .mount(&server)
        .await;
    let err = client.delete_message(Id(1), Id(2)).await.unwrap_err();
    assert_eq!(err, ApiError::Forbidden { code: Some(50013), message: "Missing Permissions".into() });
    assert_eq!(err.to_string(), "Missing permissions: Missing Permissions");
    assert_eq!(client.channel(Id(404)).await.unwrap_err(), ApiError::NotFound);
    let err = client.edit_message(Id(1), Id(3), "x").await.unwrap_err();
    assert_eq!(
        err,
        ApiError::Http { status: 400, code: Some(50035), message: "Invalid Form Body (content: Must be 2000 or fewer in length.)".into() }
    );
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn transient_errors_retry_only_idempotent() {
    let (server, client) = setup().await;
    Mock::given(method("GET")).and(path("/channels/1")).respond_with(ResponseTemplate::new(503)).up_to_n_times(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/channels/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "1"})))
        .mount(&server)
        .await;
    Mock::given(method("POST")).and(path("/channels/1/typing")).respond_with(ResponseTemplate::new(502)).expect(1).mount(&server).await;
    client.channel(Id(1)).await.unwrap();
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    let err = client.trigger_typing(Id(1)).await.unwrap_err();
    assert!(matches!(err, ApiError::Http { status: 502, .. }));
    assert!(err.is_retryable());
}

#[tokio::test]
async fn search_indexing_returns_202() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/guilds/5/messages/search"))
        .respond_with(ResponseTemplate::new(202).set_body_json(json!({
            "message": "Index not yet available. Try again later", "code": 110000, "documents_indexed": 0, "retry_after": 2
        })))
        .expect(1)
        .mount(&server)
        .await;
    let out = client.search_guild_messages(Id(5), &SearchQuery { content: Some("hi".into()), ..Default::default() }).await.unwrap();
    assert_eq!(out, SearchOutcome::Indexing { retry_after: 2.0 });
}

#[tokio::test]
async fn search_flattens_nested_results() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/guilds/5/messages/search"))
        .and(query_param("content", "hello world"))
        .and(query_param("limit", "25"))
        .and(query_param("offset", "25"))
        .and(query_param("author_id", "42"))
        .and(query_param("sort_by", "timestamp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "total_results": 57,
            "messages": [[message_json(100, 1, "hello world")], [message_json(90, 2, "hello world again")]],
            "threads": [], "members": []
        })))
        .expect(1)
        .mount(&server)
        .await;
    let q = SearchQuery {
        content: Some("hello world".into()),
        channel_ids: vec![Id(1), Id(2)],
        author_ids: vec![Id(42)],
        offset: 25,
        sort_by: Some("timestamp".into()),
        ..Default::default()
    };
    let SearchOutcome::Results { total, messages } = client.search_guild_messages(Id(5), &q).await.unwrap() else {
        panic!("expected results");
    };
    assert_eq!(total, 57);
    assert_eq!(messages.iter().map(|m| m.id.0).collect::<Vec<_>>(), vec![100, 90]);
    let reqs = server.received_requests().await.unwrap();
    let query = reqs[0].url.query().unwrap();
    assert!(query.contains("channel_id=1&channel_id=2"), "{query}");
}

#[tokio::test]
async fn reaction_emoji_is_url_encoded() {
    let (server, client) = setup().await;
    Mock::given(method("PUT"))
        .and(path("/channels/1/messages/2/reactions/%F0%9F%91%8D/@me"))
        .and(header("content-length", "0"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/channels/1/messages/2/reactions/blob%3A123/@me"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    client.add_reaction(Id(1), Id(2), &Emoji::unicode("👍")).await.unwrap();
    client.remove_own_reaction(Id(1), Id(2), &Emoji::custom(Id(123), "blob", false)).await.unwrap();
}

#[tokio::test]
async fn pins_and_dm_and_thread() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/channels/1/messages/pins"))
        .and(query_param("limit", "50"))
        .and(query_param("before", "2026-11-01T00:00:00+00:00"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{"pinned_at": "2026-10-01T00:00:00+00:00", "message": message_json(9, 1, "pinned")}],
            "has_more": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/channels/1/messages/pins/9"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/channels/1/messages/pins/9"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/users/@me/channels"))
        .and(body_json(json!({"recipient_id": "42"})))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"id": "300", "type": 1, "recipients": [{"id": "42", "username": "alice"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/channels/1/messages/9/threads"))
        .and(body_json(json!({"name": "discussion"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "9", "type": 11, "name": "discussion"})))
        .expect(1)
        .mount(&server)
        .await;
    let page = client.pins(Id(1), Some("2026-11-01T00:00:00+00:00"), 200).await.unwrap();
    assert!(page.has_more);
    assert_eq!(page.items[0].message.content, "pinned");
    assert_eq!(page.items[0].pinned_at, "2026-10-01T00:00:00+00:00");
    client.pin(Id(1), Id(9)).await.unwrap();
    client.unpin(Id(1), Id(9)).await.unwrap();
    let dm = client.create_dm(Id(42)).await.unwrap();
    assert_eq!(dm.display_name(), "alice");
    let thread = client.start_thread_from_message(Id(1), Id(9), "discussion").await.unwrap();
    assert_eq!(thread.id, Id(9));
}

#[tokio::test]
async fn download_enforces_cap_without_auth() {
    let (server, client) = setup().await;
    Mock::given(method("GET"))
        .and(path("/attachments/big.bin"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![7u8; 1000]))
        .mount(&server)
        .await;
    let url = format!("{}/attachments/big.bin", server.uri());
    assert_eq!(client.download(&url, 100).await.unwrap_err(), ApiError::TooLarge);
    let bytes = client.download(&url, 1000).await.unwrap();
    assert_eq!(bytes.len(), 1000);
    for r in server.received_requests().await.unwrap() {
        assert!(r.headers.get("authorization").is_none(), "CDN downloads must not carry credentials");
    }
    assert!(matches!(client.download("file:///etc/passwd", 10).await, Err(ApiError::Invalid(_))));
}

#[tokio::test]
async fn gateway_url_without_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gateway"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"url": "wss://gateway.discord.gg"})))
        .expect(1)
        .mount(&server)
        .await;
    let url = RestClient::gateway_url_unauthenticated(Some(&server.uri())).await.unwrap();
    assert_eq!(url, "wss://gateway.discord.gg");
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs[0].headers.get("authorization").is_none());
}
