//! OAuth2 authorization-code flow with PKCE (RFC 7636) and a loopback redirect (RFC 8252).
//!
//! The user registers their own Discord application and supplies its client ID, so no
//! developer credentials are embedded here. Apps with "Public Client" enabled need no secret;
//! otherwise an optional client secret is sent with `client_secret_post`.

use std::collections::HashMap;
use std::fmt;
use std::net::Ipv4Addr;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use crate::error::{ApiError, Redacted, describe, sanitize};
use crate::{AuthError, http, now_unix};

pub const DEFAULT_REDIRECT_PORT: u16 = 53134;
pub const DEFAULT_SCOPES: [&str; 3] = ["identify", "guilds", "guilds.members.read"];

const CALLBACK_PATH: &str = "/callback";
/// Discord's default access-token lifetime, used when a response omits `expires_in`.
const DEFAULT_EXPIRES_IN: i64 = 604_800;
const CONN_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_HEAD: usize = 16 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OAuthConfig {
    pub client_id: String,
    /// Only for applications that are not "Public Client"; never serialized.
    #[serde(skip_serializing)]
    pub client_secret: Option<String>,
    /// `0` binds an ephemeral port, which is only useful against a mock authorization server.
    pub redirect_port: u16,
    pub scopes: Vec<String>,
    pub authorize_url: String,
    pub token_url: String,
    pub revoke_url: String,
}

impl Default for OAuthConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: None,
            redirect_port: DEFAULT_REDIRECT_PORT,
            scopes: DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect(),
            authorize_url: "https://discord.com/oauth2/authorize".into(),
            token_url: "https://discord.com/api/oauth2/token".into(),
            revoke_url: "https://discord.com/api/oauth2/token/revoke".into(),
        }
    }
}

impl fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &Redacted(&self.client_secret))
            .field("redirect_port", &self.redirect_port)
            .field("scopes", &self.scopes)
            .field("authorize_url", &self.authorize_url)
            .field("token_url", &self.token_url)
            .field("revoke_url", &self.revoke_url)
            .finish()
    }
}

impl OAuthConfig {
    /// Must be registered verbatim under OAuth2 → Redirects in the Developer Portal.
    pub fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}{CALLBACK_PATH}", self.redirect_port)
    }

    pub fn scope_string(&self) -> String {
        self.scopes.join(" ")
    }

    /// Catches the common mistakes (empty or non-numeric client ID, no scopes) before any network I/O.
    pub fn validate(&self) -> Result<(), AuthError> {
        let id = self.client_id.trim();
        if id.is_empty() {
            return Err(AuthError::InvalidConfig("Enter the Client ID of your Discord application first.".into()));
        }
        if id.len() > 20 || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AuthError::InvalidConfig(
                "The Client ID must be the numeric Application ID from the Discord Developer Portal.".into(),
            ));
        }
        if self.scopes.iter().all(|s| s.trim().is_empty()) {
            return Err(AuthError::InvalidConfig("At least one OAuth2 scope is required.".into()));
        }
        Ok(())
    }

    fn secret(&self) -> Option<&str> {
        self.client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }
}

#[derive(Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    /// `challenge = BASE64URL(SHA256(verifier))` per RFC 7636 §4.2.
    pub fn from_verifier(verifier: &str) -> Self {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self { verifier: verifier.to_string(), challenge }
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pkce").field("verifier", &"<redacted>").field("challenge", &self.challenge).finish()
    }
}

/// 64 random bytes encode to an 86-character verifier (RFC 7636 allows 43–128).
pub fn pkce() -> Pkce {
    let mut bytes = [0u8; 64];
    rand::rng().fill(&mut bytes);
    Pkce::from_verifier(&URL_SAFE_NO_PAD.encode(bytes))
}

fn random_state() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn authorize_url(cfg: &OAuthConfig, state: &str, pkce: &Pkce) -> String {
    // Discord documents `%20` for scope separators; `+` from the form serializer is a space only.
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("client_id", cfg.client_id.trim())
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", &cfg.redirect_uri())
        .append_pair("scope", &cfg.scope_string())
        .append_pair("state", state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256")
        .finish()
        .replace('+', "%20");
    let sep = if cfg.authorize_url.contains('?') { '&' } else { '?' };
    format!("{}{sep}{query}", cfg.authorize_url)
}

#[derive(Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Unix seconds.
    pub expires_at: i64,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub token_type: String,
}

impl fmt::Debug for OAuthTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthTokens")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &Redacted(&self.refresh_token))
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .field("token_type", &self.token_type)
            .finish()
    }
}

impl OAuthTokens {
    pub fn expires_soon(&self, within_secs: i64) -> bool {
        now_unix().saturating_add(within_secs) >= self.expires_at
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    token_type: String,
    #[serde(default)]
    expires_in: i64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: String,
}

impl From<TokenResponse> for OAuthTokens {
    fn from(r: TokenResponse) -> Self {
        let expires_in = if r.expires_in > 0 { r.expires_in } else { DEFAULT_EXPIRES_IN };
        Self {
            access_token: r.access_token,
            refresh_token: r.refresh_token.filter(|t| !t.is_empty()),
            expires_at: now_unix().saturating_add(expires_in),
            scope: r.scope,
            token_type: if r.token_type.is_empty() { "Bearer".into() } else { r.token_type },
        }
    }
}

async fn post_form(url: &str, form: &[(&str, &str)]) -> Result<reqwest::Response, ApiError> {
    let client = http::client().map_err(|e| ApiError::transport(e.to_string()))?;
    let resp = client.post(url).form(form).send().await.map_err(|e| ApiError::transport(describe(e)))?;
    if resp.status().is_success() { Ok(resp) } else { Err(ApiError::from_response(resp).await) }
}

async fn token_request(cfg: &OAuthConfig, mut form: Vec<(&str, &str)>) -> Result<OAuthTokens, ApiError> {
    form.push(("client_id", cfg.client_id.trim()));
    if let Some(secret) = cfg.secret() {
        form.push(("client_secret", secret));
    }
    let resp = post_form(&cfg.token_url, &form).await?;
    let body: TokenResponse =
        resp.json().await.map_err(|e| ApiError::transport(format!("unexpected token response: {}", e.without_url())))?;
    Ok(body.into())
}

pub async fn exchange_code(cfg: &OAuthConfig, code: &str, verifier: &str) -> Result<OAuthTokens, AuthError> {
    let redirect_uri = cfg.redirect_uri();
    let form =
        vec![("grant_type", "authorization_code"), ("code", code), ("redirect_uri", redirect_uri.as_str()), ("code_verifier", verifier)];
    Ok(token_request(cfg, form).await?)
}

/// An `invalid_grant` answer means the refresh token is revoked or expired and maps to
/// [`AuthError::SessionExpired`]. Discord rotates refresh tokens; if a response omits one the old one is kept.
pub async fn refresh(cfg: &OAuthConfig, refresh_token: &str) -> Result<OAuthTokens, AuthError> {
    let form = vec![("grant_type", "refresh_token"), ("refresh_token", refresh_token)];
    let mut tokens = token_request(cfg, form)
        .await
        .map_err(|e| if e.code.as_deref() == Some("invalid_grant") { AuthError::SessionExpired } else { e.into() })?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token.to_string());
    }
    Ok(tokens)
}

/// Revoking either token of a grant invalidates the whole grant on Discord.
pub async fn revoke(cfg: &OAuthConfig, token: &str) -> Result<(), AuthError> {
    let mut form = vec![("token", token), ("client_id", cfg.client_id.trim())];
    if let Some(secret) = cfg.secret() {
        form.push(("client_secret", secret));
    }
    post_form(&cfg.revoke_url, &form).await?;
    Ok(())
}

pub async fn fetch_user(access_token: &str, api_base: &str) -> Result<dn_core::User, AuthError> {
    let client = http::client()?;
    http::get_json(&client, &http::join(api_base, "users/@me"), &format!("Bearer {access_token}"))
        .await
        .map_err(|e| if e.status == 401 { AuthError::SessionExpired } else { e.into() })
}

/// Signs in through the system browser. With `open_browser == false` nothing opens the URL;
/// use [`sign_in_with`] to receive it. Dropping or firing `cancel` aborts the sign-in.
pub async fn sign_in(
    cfg: &OAuthConfig,
    open_browser: bool,
    cancel: oneshot::Receiver<()>,
    timeout: Duration,
) -> Result<OAuthTokens, AuthError> {
    sign_in_with(cfg, cancel, timeout, |url| {
        if open_browser {
            open::that_detached(url).map_err(|e| AuthError::Other(format!("Could not open your web browser: {e}")))?;
        }
        Ok(())
    })
    .await
}

/// Like [`sign_in`], but hands the authorization URL to `on_authorize_url` so the caller can open
/// it, show it as a copyable fallback, or drive it from a test. The loopback port is bound before
/// the callback runs, so the redirect can never race the listener. Returning `Err` aborts sign-in.
pub async fn sign_in_with<F>(
    cfg: &OAuthConfig,
    mut cancel: oneshot::Receiver<()>,
    timeout: Duration,
    on_authorize_url: F,
) -> Result<OAuthTokens, AuthError>
where
    F: FnOnce(&str) -> Result<(), AuthError>,
{
    cfg.validate()?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, cfg.redirect_port)).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => AuthError::PortInUse(cfg.redirect_port),
        _ => e.into(),
    })?;
    let mut cfg = cfg.clone();
    cfg.redirect_port = listener.local_addr()?.port();

    let pkce = pkce();
    let state = random_state();
    on_authorize_url(&authorize_url(&cfg, &state, &pkce))?;

    let callback = tokio::select! {
        _ = &mut cancel => return Err(AuthError::Cancelled),
        r = tokio::time::timeout(timeout, wait_for_callback(&listener, &state)) => {
            r.map_err(|_| AuthError::Timeout)?
        }
    };
    drop(listener);

    match callback {
        Callback::Fatal(e) => Err(e),
        Callback::Code { code, mut stream } => {
            let result = tokio::select! {
                _ = &mut cancel => Err(AuthError::Cancelled),
                r = exchange_code(&cfg, &code, &pkce.verifier) => r,
            };
            // The browser only sees "signed in" once the token exchange really succeeded.
            match &result {
                Ok(_) => respond_html(&mut stream, 200, "OK", &SUCCESS_PAGE).await,
                Err(_) => respond_html(&mut stream, 502, "Bad Gateway", &FAILURE_PAGE).await,
            }
            result
        }
    }
}

enum Callback {
    Code { code: String, stream: TcpStream },
    Fatal(AuthError),
}

/// Each connection is handled in its own task: browsers open speculative idle connections
/// that must not block the real callback.
async fn wait_for_callback(listener: &TcpListener, state: &str) -> Callback {
    let (tx, mut rx) = mpsc::channel::<Callback>(4);
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let tx = tx.clone();
                    let state = state.to_string();
                    tokio::spawn(async move {
                        if let Some(cb) = handle_connection(stream, &state).await {
                            let _ = tx.send(cb).await;
                        }
                    });
                }
                Err(e) => {
                    tracing::debug!("loopback accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            },
            Some(cb) = rx.recv() => return cb,
        }
    }
}

async fn handle_connection(mut stream: TcpStream, state: &str) -> Option<Callback> {
    let head = match read_request_head(&mut stream).await {
        Head::Complete(h) => h,
        Head::TooLarge => {
            respond_text(&mut stream, 431, "Request Header Fields Too Large", "Request too large").await;
            return None;
        }
        Head::Closed => return None,
    };

    let mut parts = head.lines().next().unwrap_or_default().split_ascii_whitespace();
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        respond_text(&mut stream, 400, "Bad Request", "Bad request").await;
        return None;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != CALLBACK_PATH {
        respond_text(&mut stream, 404, "Not Found", "Not found").await;
        return None;
    }
    if method != "GET" {
        respond_text(&mut stream, 405, "Method Not Allowed", "Method not allowed").await;
        return None;
    }

    let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    let got_state = params.get("state");
    let state_ok = got_state.is_some_and(|s| ct_eq(s.as_bytes(), state.as_bytes()));

    if let Some(error) = params.get("error") {
        if got_state.is_some() && !state_ok {
            respond_html(&mut stream, 400, "Bad Request", &MISMATCH_PAGE).await;
            return Some(Callback::Fatal(AuthError::StateMismatch));
        }
        respond_html(&mut stream, 200, "OK", &DENIED_PAGE).await;
        let err = if error == "access_denied" {
            AuthError::Denied
        } else {
            let detail = params.get("error_description").map(|d| format!(": {}", sanitize(d, 200)));
            AuthError::Other(format!("Discord reported an authorization error ({}){}", sanitize(error, 60), detail.unwrap_or_default()))
        };
        return Some(Callback::Fatal(err));
    }

    let Some(code) = params.get("code").filter(|c| !c.is_empty()) else {
        respond_html(&mut stream, 400, "Bad Request", &MISSING_CODE_PAGE).await;
        return None;
    };
    if !state_ok {
        respond_html(&mut stream, 400, "Bad Request", &MISMATCH_PAGE).await;
        return Some(Callback::Fatal(AuthError::StateMismatch));
    }
    Some(Callback::Code { code: code.clone(), stream })
}

enum Head {
    Complete(String),
    TooLarge,
    Closed,
}

/// Reads through the blank line so the response is not followed by a TCP reset from unread input.
async fn read_request_head(stream: &mut TcpStream) -> Head {
    let read = async {
        let mut buf = Vec::with_capacity(1024);
        let mut chunk = [0u8; 1024];
        loop {
            let n = match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return Head::Closed,
                Ok(n) => n,
            };
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                return Head::Complete(String::from_utf8_lossy(&buf).into_owned());
            }
            if buf.len() > MAX_REQUEST_HEAD {
                return Head::TooLarge;
            }
        }
    };
    tokio::time::timeout(CONN_TIMEOUT, read).await.unwrap_or(Head::Closed)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && std::hint::black_box(a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y))) == 0
}

async fn respond(stream: &mut TcpStream, status: u16, reason: &str, content_type: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let write = async {
        stream.write_all(head.as_bytes()).await?;
        stream.write_all(body.as_bytes()).await?;
        stream.shutdown().await
    };
    if let Ok(Err(e)) = tokio::time::timeout(CONN_TIMEOUT, write).await {
        tracing::debug!("loopback response failed: {e}");
    }
}

async fn respond_html(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    respond(stream, status, reason, "text/html; charset=utf-8", body).await;
}

async fn respond_text(stream: &mut TcpStream, status: u16, reason: &str, body: &str) {
    respond(stream, status, reason, "text/plain; charset=utf-8", body).await;
}

/// Pages contain only static text; nothing from the request is ever reflected into them.
fn page(title: &str, detail: &str, ok: bool) -> String {
    let accent = if ok { "#3ba55d" } else { "#ed4245" };
    let mark = if ok { "&#10003;" } else { "&#33;" };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>\
         html,body{{height:100%;margin:0}}\
         body{{display:flex;align-items:center;justify-content:center;background:#1e1f22;color:#dbdee1;\
         font:16px/1.5 system-ui,-apple-system,'Segoe UI',sans-serif}}\
         main{{max-width:26rem;padding:2rem;text-align:center;background:#2b2d31;border-radius:12px}}\
         .mark{{width:3.5rem;height:3.5rem;margin:0 auto 1rem;border-radius:50%;background:{accent};color:#fff;\
         font-size:1.8rem;line-height:3.5rem;font-weight:700}}\
         h1{{font-size:1.25rem;margin:0 0 .5rem;color:#f2f3f5}}p{{margin:0;color:#b5bac1}}\
         </style></head><body><main><div class=\"mark\">{mark}</div><h1>{title}</h1><p>{detail}</p></main></body></html>"
    )
}

static SUCCESS_PAGE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| page("Signed in — you can close this tab", "Return to Discord Native to continue.", true));
static DENIED_PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    page("Sign-in was cancelled", "Access was not granted. You can close this tab and try again from Discord Native.", false)
});
static MISMATCH_PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    page(
        "Sign-in request rejected",
        "This response did not match the sign-in started in Discord Native. Close this tab and try again.",
        false,
    )
});
static MISSING_CODE_PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    page("Nothing to sign in with", "This page was opened without an authorization code. Start sign-in from Discord Native.", false)
});
static FAILURE_PAGE: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    page("Sign-in could not be completed", "Discord Native could not finish signing in. Return to the app for details.", false)
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_appendix_b() {
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(p.challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn pkce_verifier_is_unreserved_charset_and_in_range() {
        for _ in 0..20 {
            let p = pkce();
            assert!((43..=128).contains(&p.verifier.len()), "len {}", p.verifier.len());
            assert!(p.verifier.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
            assert_eq!(p.challenge, Pkce::from_verifier(&p.verifier).challenge);
            assert_eq!(p.challenge.len(), 43);
        }
        assert_ne!(pkce().verifier, pkce().verifier);
    }

    #[test]
    fn authorize_url_contains_all_params_encoded() {
        let cfg = OAuthConfig { client_id: "123456789012345678".into(), ..Default::default() };
        let p = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        let url = authorize_url(&cfg, "st&te", &p);
        assert!(url.starts_with("https://discord.com/oauth2/authorize?"));
        for expected in [
            "client_id=123456789012345678",
            "response_type=code",
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A53134%2Fcallback",
            "scope=identify%20guilds%20guilds.members.read",
            "state=st%26te",
            "code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "code_challenge_method=S256",
        ] {
            assert!(url.contains(expected), "missing {expected} in {url}");
        }
        assert!(!url.contains('+'));
        assert!(!url.contains("prompt="));
        let parsed = url::Url::parse(&url).unwrap();
        let q: HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:53134/callback");
        assert_eq!(q["scope"], "identify guilds guilds.members.read");
        assert_eq!(q["state"], "st&te");
    }

    #[test]
    fn default_config_and_redirect_uri() {
        let cfg = OAuthConfig::default();
        assert_eq!(cfg.redirect_port, DEFAULT_REDIRECT_PORT);
        assert_eq!(cfg.redirect_uri(), "http://127.0.0.1:53134/callback");
        assert_eq!(cfg.scope_string(), "identify guilds guilds.members.read");
        assert_eq!(cfg.token_url, "https://discord.com/api/oauth2/token");
        assert_eq!(cfg.revoke_url, "https://discord.com/api/oauth2/token/revoke");
    }

    #[test]
    fn config_serialization_omits_secret_and_accepts_partial_input() {
        let cfg = OAuthConfig { client_id: "1".into(), client_secret: Some("shh-secret".into()), ..Default::default() };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("shh-secret") && !json.contains("client_secret"));
        let back: OAuthConfig = serde_json::from_str(r#"{"client_id":"42"}"#).unwrap();
        assert_eq!(back.client_id, "42");
        assert_eq!(back.redirect_port, DEFAULT_REDIRECT_PORT);
        assert!(back.client_secret.is_none());
    }

    #[test]
    fn validate_rejects_bad_client_ids() {
        let mut cfg = OAuthConfig::default();
        assert!(matches!(cfg.validate(), Err(AuthError::InvalidConfig(_))));
        cfg.client_id = "not-a-snowflake".into();
        assert!(matches!(cfg.validate(), Err(AuthError::InvalidConfig(_))));
        cfg.client_id = "123456789012345678".into();
        assert!(cfg.validate().is_ok());
        cfg.scopes.clear();
        assert!(matches!(cfg.validate(), Err(AuthError::InvalidConfig(_))));
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let cfg = OAuthConfig { client_id: "1".into(), client_secret: Some("shh-client-secret".into()), ..Default::default() };
        let tokens = OAuthTokens {
            access_token: "access-token-value".into(),
            refresh_token: Some("refresh-token-value".into()),
            expires_at: 1,
            scope: "identify".into(),
            token_type: "Bearer".into(),
        };
        let p = Pkce::from_verifier("verifier-value-1234567890123456789012345678901234");
        let out = format!("{cfg:?} {tokens:?} {p:?}");
        for secret in ["shh-client-secret", "access-token-value", "refresh-token-value", "verifier-value"] {
            assert!(!out.contains(secret), "{secret} leaked: {out}");
        }
    }

    #[test]
    fn expires_soon_boundaries() {
        let mut t = OAuthTokens {
            access_token: "a".into(),
            refresh_token: None,
            expires_at: now_unix() + 1000,
            scope: String::new(),
            token_type: "Bearer".into(),
        };
        assert!(!t.expires_soon(300));
        assert!(t.expires_soon(2000));
        t.expires_at = now_unix() - 1;
        assert!(t.expires_soon(0));
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"abcd"));
    }
}
