use std::collections::HashMap;
use std::time::Duration;

use dn_auth::AuthError;
use dn_auth::oauth::{self, OAuthConfig, Pkce};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const CLIENT_ID: &str = "123456789012345678";

fn cfg(server: &MockServer) -> OAuthConfig {
    OAuthConfig {
        client_id: CLIENT_ID.into(),
        redirect_port: 0,
        authorize_url: format!("{}/authorize", server.uri()),
        token_url: format!("{}/token", server.uri()),
        revoke_url: format!("{}/revoke", server.uri()),
        ..Default::default()
    }
}

fn form(req: &Request) -> HashMap<String, String> {
    url::form_urlencoded::parse(&req.body)
        .into_owned()
        .collect()
}

fn query(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .into_owned()
        .collect()
}

async fn mock_token_ok(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-1",
            "token_type": "Bearer",
            "expires_in": 604800,
            "refresh_token": "refresh-1",
            "scope": "identify guilds"
        })))
        .mount(server)
        .await;
}

async fn token_requests(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.url.path() == "/token")
        .collect()
}

/// Minimal raw HTTP client: returns (status, body).
async fn raw_get(port: u16, target: &str) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(
        format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n")
            .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).await.unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    (status, body)
}

struct Browser {
    url_rx: oneshot::Receiver<String>,
}

/// Returns the callback for `sign_in_with` and a handle the "browser" task awaits the URL on.
fn url_channel() -> (impl FnOnce(&str) -> Result<(), AuthError>, Browser) {
    let (tx, rx) = oneshot::channel();
    (
        move |url: &str| {
            tx.send(url.to_string()).ok();
            Ok(())
        },
        Browser { url_rx: rx },
    )
}

fn port_of(authorize_url: &str) -> u16 {
    let redirect = query(authorize_url)["redirect_uri"].clone();
    url::Url::parse(&redirect).unwrap().port().unwrap()
}

async fn run_flow<F, Fut>(
    cfg: &OAuthConfig,
    browser: F,
) -> (Result<oauth::OAuthTokens, AuthError>, Fut::Output)
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future,
{
    let (cb, b) = url_channel();
    let (_cancel_tx, cancel_rx) = oneshot::channel();
    let sign_in = oauth::sign_in_with(cfg, cancel_rx, Duration::from_secs(10), cb);
    let browser_task = async move {
        let url = b.url_rx.await.unwrap();
        browser(url).await
    };
    let (result, out) = tokio::join!(sign_in, browser_task);
    (result, out)
}

#[tokio::test]
async fn full_flow_exchanges_code_with_pkce_and_no_secret() {
    let server = MockServer::start().await;
    mock_token_ok(&server).await;
    let cfg = cfg(&server);

    let (result, (fav, callback, authorize_url)) = run_flow(&cfg, |url| async move {
        let port = port_of(&url);
        let state = query(&url)["state"].clone();
        let fav = raw_get(port, "/favicon.ico").await;
        let callback = raw_get(port, &format!("/callback?code=abc&state={state}")).await;
        (fav, callback, url)
    })
    .await;

    let tokens = result.unwrap();
    assert_eq!(tokens.access_token, "access-1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));
    assert_eq!(tokens.scope, "identify guilds");
    assert_eq!(tokens.token_type, "Bearer");
    assert!(!tokens.expires_soon(3600));

    assert_eq!(fav.0, 404);
    assert_eq!(callback.0, 200);
    assert!(callback.1.contains("Signed in — you can close this tab"));
    assert!(
        !callback.1.contains("abc"),
        "the code must not be reflected"
    );

    let q = query(&authorize_url);
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["code_challenge_method"], "S256");
    assert_eq!(q["scope"], "identify guilds guilds.members.read");

    let reqs = token_requests(&server).await;
    assert_eq!(reqs.len(), 1, "exactly one exchange");
    let body = form(&reqs[0]);
    assert_eq!(body["grant_type"], "authorization_code");
    assert_eq!(body["code"], "abc");
    assert_eq!(body["client_id"], CLIENT_ID);
    assert_eq!(body["redirect_uri"], q["redirect_uri"]);
    assert_eq!(
        Pkce::from_verifier(&body["code_verifier"]).challenge,
        q["code_challenge"]
    );
    assert!(!body.contains_key("client_secret"));
    assert_eq!(
        reqs[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/x-www-form-urlencoded"
    );
    assert!(
        reqs[0]
            .headers
            .get("user-agent")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("DiscordBot (")
    );
}

#[tokio::test]
async fn client_secret_is_sent_only_when_configured() {
    let server = MockServer::start().await;
    mock_token_ok(&server).await;
    let mut cfg = cfg(&server);
    cfg.client_secret = Some("  s3cret  ".into());

    let (result, _) = run_flow(&cfg, |url| async move {
        let state = query(&url)["state"].clone();
        raw_get(port_of(&url), &format!("/callback?code=abc&state={state}")).await
    })
    .await;
    result.unwrap();

    let body = form(&token_requests(&server).await[0]);
    assert_eq!(body["client_secret"], "s3cret");
}

#[tokio::test]
async fn state_mismatch_is_rejected_without_exchanging() {
    let server = MockServer::start().await;
    mock_token_ok(&server).await;

    let (result, (status, body)) = run_flow(&cfg(&server), |url| async move {
        raw_get(port_of(&url), "/callback?code=abc&state=forged").await
    })
    .await;

    assert_eq!(result.unwrap_err(), AuthError::StateMismatch);
    assert_eq!(status, 400);
    assert!(body.contains("Sign-in request rejected"));
    assert!(token_requests(&server).await.is_empty());
}

#[tokio::test]
async fn code_without_state_is_rejected() {
    let server = MockServer::start().await;
    mock_token_ok(&server).await;
    let (result, (status, _)) = run_flow(&cfg(&server), |url| async move {
        raw_get(port_of(&url), "/callback?code=abc").await
    })
    .await;
    assert_eq!(result.unwrap_err(), AuthError::StateMismatch);
    assert_eq!(status, 400);
    assert!(token_requests(&server).await.is_empty());
}

#[tokio::test]
async fn access_denied_maps_to_denied() {
    let server = MockServer::start().await;
    mock_token_ok(&server).await;

    let (result, (status, body)) = run_flow(&cfg(&server), |url| async move {
        let state = query(&url)["state"].clone();
        raw_get(
            port_of(&url),
            &format!("/callback?error=access_denied&error_description=The+resource+owner+denied&state={state}"),
        )
        .await
    })
    .await;

    assert_eq!(result.unwrap_err(), AuthError::Denied);
    assert_eq!(status, 200);
    assert!(body.contains("Sign-in was cancelled"));
    assert!(
        !body.contains("resource owner"),
        "request text must not be reflected"
    );
    assert!(token_requests(&server).await.is_empty());
}

#[tokio::test]
async fn error_with_forged_state_is_a_mismatch_not_a_denial() {
    let server = MockServer::start().await;
    let (result, _) = run_flow(&cfg(&server), |url| async move {
        raw_get(port_of(&url), "/callback?error=access_denied&state=forged").await
    })
    .await;
    assert_eq!(result.unwrap_err(), AuthError::StateMismatch);
}

#[tokio::test]
async fn other_authorization_errors_are_reported() {
    let server = MockServer::start().await;
    let (result, _) = run_flow(&cfg(&server), |url| async move {
        let state = query(&url)["state"].clone();
        raw_get(
            port_of(&url),
            &format!("/callback?error=invalid_scope&error_description=bad%20scope&state={state}"),
        )
        .await
    })
    .await;
    match result.unwrap_err() {
        AuthError::Other(msg) => assert!(
            msg.contains("invalid_scope") && msg.contains("bad scope"),
            "{msg}"
        ),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn stray_requests_and_idle_connections_do_not_end_or_block_the_flow() {
    let server = MockServer::start().await;
    mock_token_ok(&server).await;

    let (result, statuses) = run_flow(&cfg(&server), |url| async move {
        let port = port_of(&url);
        let state = query(&url)["state"].clone();
        // A speculative browser connection that never sends anything.
        let _idle = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let no_code = raw_get(port, "/callback").await.0;
        let wrong_path = raw_get(port, "/").await.0;
        let ok = raw_get(port, &format!("/callback?code=abc&state={state}"))
            .await
            .0;
        vec![no_code, wrong_path, ok]
    })
    .await;

    result.unwrap();
    assert_eq!(statuses, vec![400, 404, 200]);
    assert_eq!(token_requests(&server).await.len(), 1);
}

#[tokio::test]
async fn failed_exchange_is_reported_to_the_browser_and_caller() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Invalid \"code\" in request."
        })))
        .mount(&server)
        .await;

    let (result, (status, body)) = run_flow(&cfg(&server), |url| async move {
        let state = query(&url)["state"].clone();
        raw_get(port_of(&url), &format!("/callback?code=bad&state={state}")).await
    })
    .await;

    match result.unwrap_err() {
        AuthError::Http { status, message } => {
            assert_eq!(status, 400);
            assert!(message.contains("invalid_grant"), "{message}");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(status, 502);
    assert!(!body.contains("Signed in"));
}

#[tokio::test]
async fn times_out_when_no_callback_arrives() {
    let server = MockServer::start().await;
    let (_cancel_tx, cancel_rx) = oneshot::channel();
    let started = std::time::Instant::now();
    let err = oauth::sign_in_with(&cfg(&server), cancel_rx, Duration::from_millis(200), |_| {
        Ok(())
    })
    .await
    .unwrap_err();
    assert_eq!(err, AuthError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn cancel_returns_early_and_dropping_the_sender_cancels_too() {
    let server = MockServer::start().await;
    let config = cfg(&server);
    let (tx, rx) = oneshot::channel();
    let flow = oauth::sign_in_with(&config, rx, Duration::from_secs(30), |_| Ok(()));
    let cancel = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        tx.send(()).unwrap();
    };
    let (result, _) = tokio::join!(flow, cancel);
    assert_eq!(result.unwrap_err(), AuthError::Cancelled);

    let (tx, rx) = oneshot::channel::<()>();
    drop(tx);
    let err = oauth::sign_in_with(&cfg(&server), rx, Duration::from_secs(30), |_| Ok(()))
        .await
        .unwrap_err();
    assert_eq!(err, AuthError::Cancelled);
}

#[tokio::test]
async fn sign_in_without_browser_honours_cancel() {
    let server = MockServer::start().await;
    let (tx, rx) = oneshot::channel();
    tx.send(()).unwrap();
    let err = oauth::sign_in(&cfg(&server), false, rx, Duration::from_secs(30))
        .await
        .unwrap_err();
    assert_eq!(err, AuthError::Cancelled);
}

#[tokio::test]
async fn port_in_use_fails_before_the_browser_is_opened() {
    let server = MockServer::start().await;
    let blocker = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = blocker.local_addr().unwrap().port();
    let mut cfg = cfg(&server);
    cfg.redirect_port = port;

    let (_tx, rx) = oneshot::channel();
    let mut opened = false;
    let err = oauth::sign_in_with(&cfg, rx, Duration::from_secs(5), |_| {
        opened = true;
        Ok(())
    })
    .await
    .unwrap_err();
    assert_eq!(err, AuthError::PortInUse(port));
    assert!(!opened);
    assert!(err.to_string().contains(&port.to_string()));
}

#[tokio::test]
async fn invalid_config_is_rejected_before_binding() {
    let (_tx, rx) = oneshot::channel();
    let err = oauth::sign_in_with(&OAuthConfig::default(), rx, Duration::from_secs(1), |_| {
        Ok(())
    })
    .await
    .unwrap_err();
    assert!(matches!(err, AuthError::InvalidConfig(_)));
}

#[tokio::test]
async fn callback_error_aborts_sign_in() {
    let server = MockServer::start().await;
    let (_tx, rx) = oneshot::channel();
    let err = oauth::sign_in_with(&cfg(&server), rx, Duration::from_secs(1), |_| {
        Err(AuthError::Other("no browser".into()))
    })
    .await
    .unwrap_err();
    assert_eq!(err, AuthError::Other("no browser".into()));
}

#[tokio::test]
async fn refresh_request_shape_and_token_rotation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-2", "token_type": "Bearer", "expires_in": 100, "scope": "identify"
        })))
        .mount(&server)
        .await;

    let tokens = oauth::refresh(&cfg(&server), "refresh-1").await.unwrap();
    assert_eq!(tokens.access_token, "access-2");
    // The response carried no new refresh token, so the old one must be retained.
    assert_eq!(tokens.refresh_token.as_deref(), Some("refresh-1"));

    let mut with_secret = cfg(&server);
    with_secret.client_secret = Some("s3cret".into());
    oauth::refresh(&with_secret, "refresh-1").await.unwrap();

    let reqs = token_requests(&server).await;
    let public = form(&reqs[0]);
    assert_eq!(public["grant_type"], "refresh_token");
    assert_eq!(public["refresh_token"], "refresh-1");
    assert_eq!(public["client_id"], CLIENT_ID);
    assert!(!public.contains_key("client_secret"));
    assert_eq!(form(&reqs[1])["client_secret"], "s3cret");
}

#[tokio::test]
async fn refresh_invalid_grant_is_session_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .mount(&server)
        .await;
    assert_eq!(
        oauth::refresh(&cfg(&server), "old").await.unwrap_err(),
        AuthError::SessionExpired
    );
}

#[tokio::test]
async fn invalid_client_error_includes_setup_hint() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": "invalid_client"})))
        .mount(&server)
        .await;
    match oauth::exchange_code(&cfg(&server), "c", "v")
        .await
        .unwrap_err()
    {
        AuthError::Http { status, message } => {
            assert_eq!(status, 401);
            assert!(message.contains("Public Client"), "{message}");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn revoke_request_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/revoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&server)
        .await;

    oauth::revoke(&cfg(&server), "refresh-1").await.unwrap();
    let mut with_secret = cfg(&server);
    with_secret.client_secret = Some("s3cret".into());
    oauth::revoke(&with_secret, "access-1").await.unwrap();

    let reqs = server.received_requests().await.unwrap();
    let a = form(&reqs[0]);
    assert_eq!(a["token"], "refresh-1");
    assert_eq!(a["client_id"], CLIENT_ID);
    assert!(!a.contains_key("client_secret"));
    let b = form(&reqs[1]);
    assert_eq!(
        (b["token"].as_str(), b["client_secret"].as_str()),
        ("access-1", "s3cret")
    );
    assert_eq!(
        reqs[0]
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/x-www-form-urlencoded"
    );
}

#[tokio::test]
async fn revoke_failure_is_reported() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/revoke"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    assert!(matches!(
        oauth::revoke(&cfg(&server), "t").await,
        Err(AuthError::Http { status: 500, .. })
    ));
}

#[tokio::test]
async fn fetch_user_uses_bearer_and_maps_401() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .and(header("authorization", "Bearer good"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "80351110224678912", "username": "nelly", "discriminator": "0", "global_name": "Nelly", "avatar": null
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let user = oauth::fetch_user("good", &server.uri()).await.unwrap();
    assert_eq!(user.id.get(), 80351110224678912);
    assert_eq!(user.display_name(), "Nelly");
    assert_eq!(
        oauth::fetch_user("bad", &format!("{}/", server.uri()))
            .await
            .unwrap_err(),
        AuthError::SessionExpired
    );
}

#[tokio::test]
async fn network_failure_is_reported_as_network_error() {
    let closed = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let err = oauth::fetch_user("t", &format!("http://127.0.0.1:{port}"))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Network(_)), "{err:?}");
    assert!(
        !err.to_string().contains("127.0.0.1"),
        "URLs are stripped from transport errors"
    );
}
