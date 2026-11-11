//! Discord REST API v10 client with per-route rate limiting.
//!
//! Only officially supported credentials are accepted: bot tokens and OAuth2 bearer
//! tokens. Tokens are never logged (`Debug` redacts them, headers are marked sensitive).

mod error;
mod ratelimit;
mod route;
mod types;

pub use error::ApiError;
pub use types::*;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use dn_core::{Channel, Emoji, Id, Member, Message, PartialGuild, User};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::{AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue};
use reqwest::multipart::{Form, Part};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::time::Instant;

use ratelimit::{RateLimiter, header_parse, header_str};
use route::Route;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_RATE_LIMIT_RETRIES: u32 = 3;
const MAX_TRANSIENT_RETRIES: u32 = 2;
const MAX_FILES: usize = 10;

/// RFC 3986 unreserved characters stay literal; everything else is percent-encoded.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'.').remove(b'_').remove(b'~');

/// Credentials for the REST API.
#[derive(Clone)]
pub enum Auth {
    /// Bot token from the Developer Portal.
    Bot(String),
    /// OAuth2 access token.
    Bearer(String),
}

impl Auth {
    pub fn token(&self) -> &str {
        match self {
            Auth::Bot(t) | Auth::Bearer(t) => t,
        }
    }

    pub fn is_bot(&self) -> bool {
        matches!(self, Auth::Bot(_))
    }

    fn header_value(&self) -> Result<HeaderValue, ApiError> {
        let token = self.token().trim();
        if token.is_empty() {
            return Err(ApiError::Invalid("the token is empty".into()));
        }
        let raw = match self {
            Auth::Bot(_) => format!("Bot {token}"),
            Auth::Bearer(_) => format!("Bearer {token}"),
        };
        let mut v = HeaderValue::from_str(&raw).map_err(|_| ApiError::Invalid("the token contains invalid characters".into()))?;
        v.set_sensitive(true);
        Ok(v)
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Auth::Bot(_) => f.write_str("Bot(<redacted>)"),
            Auth::Bearer(_) => f.write_str("Bearer(<redacted>)"),
        }
    }
}

/// Cheaply cloneable Discord REST client; clones share the connection pool and
/// rate-limit state.
#[derive(Clone)]
pub struct RestClient {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    base: String,
    auth: Auth,
    auth_header: HeaderValue,
    limiter: RateLimiter,
}

impl fmt::Debug for RestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RestClient").field("base", &self.inner.base).field("auth", &self.inner.auth).finish()
    }
}

struct PreparedFile {
    name: String,
    mime: String,
    bytes: Bytes,
}

enum Body {
    Empty,
    Json(Bytes),
    Multipart { payload: String, files: Vec<PreparedFile> },
}

struct Request {
    method: Method,
    path: String,
    query: Vec<(&'static str, String)>,
    body: Body,
    timeout: Duration,
}

impl Request {
    fn new(method: Method, path: impl Into<String>) -> Self {
        Request { method, path: path.into(), query: Vec::new(), body: Body::Empty, timeout: REQUEST_TIMEOUT }
    }

    fn query(mut self, key: &'static str, value: impl ToString) -> Self {
        self.query.push((key, value.to_string()));
        self
    }

    fn json<T: Serialize + ?Sized>(mut self, body: &T) -> Result<Self, ApiError> {
        let bytes = serde_json::to_vec(body).map_err(|e| ApiError::Invalid(e.to_string()))?;
        self.body = Body::Json(bytes.into());
        Ok(self)
    }
}

fn build_http() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder()
        .user_agent(dn_core::user_agent())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .gzip(true)
        .build()
        .map_err(|e| ApiError::Network(format!("failed to initialise HTTP client: {e}")))
}

impl RestClient {
    pub fn new(auth: Auth) -> Result<Self, ApiError> {
        Self::with_base_url(auth, dn_core::API_BASE)
    }

    /// Client against a custom API base (e.g. a mock server in tests).
    pub fn with_base_url(auth: Auth, base: &str) -> Result<Self, ApiError> {
        let base = base.trim().trim_end_matches('/');
        url::Url::parse(base).map_err(|e| ApiError::Invalid(format!("invalid API base URL: {e}")))?;
        let auth_header = auth.header_value()?;
        Ok(RestClient {
            inner: Arc::new(Inner { http: build_http()?, base: base.to_string(), auth, auth_header, limiter: RateLimiter::default() }),
        })
    }

    pub fn auth(&self) -> &Auth {
        &self.inner.auth
    }

    pub fn base_url(&self) -> &str {
        &self.inner.base
    }

    // ---- users / applications ----

    pub async fn current_user(&self) -> Result<User, ApiError> {
        self.json(Request::new(Method::GET, "/users/@me")).await
    }

    pub async fn current_application(&self) -> Result<Application, ApiError> {
        self.json(Request::new(Method::GET, "/applications/@me")).await
    }

    pub async fn user(&self, id: Id) -> Result<User, ApiError> {
        self.json(Request::new(Method::GET, format!("/users/{id}"))).await
    }

    /// All guilds of the current user/bot, following `after` pagination.
    pub async fn current_user_guilds(&self) -> Result<Vec<PartialGuild>, ApiError> {
        const PAGE: usize = 200;
        let mut out = Vec::new();
        let mut after: Option<Id> = None;
        loop {
            let mut req = Request::new(Method::GET, "/users/@me/guilds").query("limit", PAGE);
            if let Some(a) = after {
                req = req.query("after", a);
            }
            let page: Vec<PartialGuild> = self.json(req).await?;
            let n = page.len();
            let last = page.iter().map(|g| g.id).max();
            out.extend(page);
            match last {
                Some(id) if n >= PAGE && after.is_none_or(|a| id > a) => after = Some(id),
                _ => break,
            }
        }
        Ok(out)
    }

    /// Requires an OAuth2 bearer token with the `guilds.members.read` scope.
    pub async fn current_user_guild_member(&self, guild: Id) -> Result<Member, ApiError> {
        self.json(Request::new(Method::GET, format!("/users/@me/guilds/{guild}/member"))).await
    }

    pub async fn create_dm(&self, recipient: Id) -> Result<Channel, ApiError> {
        self.json(Request::new(Method::POST, "/users/@me/channels").json(&json!({ "recipient_id": recipient }))?).await
    }

    // ---- gateway ----

    pub async fn gateway_bot(&self) -> Result<GatewayBot, ApiError> {
        self.json(Request::new(Method::GET, "/gateway/bot")).await
    }

    /// `GET /gateway` without credentials; `base` defaults to [`dn_core::API_BASE`].
    pub async fn gateway_url_unauthenticated(base: Option<&str>) -> Result<String, ApiError> {
        let base = base.unwrap_or(dn_core::API_BASE).trim().trim_end_matches('/');
        let resp = build_http()?.get(format!("{base}/gateway")).send().await.map_err(ApiError::network)?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.bytes().await.map_err(ApiError::network)?;
        if status == StatusCode::TOO_MANY_REQUESTS {
            let (retry_after, global) = parse_429(&headers, &body);
            return Err(ApiError::RateLimited { retry_after, global });
        }
        if !status.is_success() {
            return Err(ApiError::from_response(status.as_u16(), &body));
        }
        let g: GatewayResponse = serde_json::from_slice(&body).map_err(ApiError::decode)?;
        if g.url.is_empty() {
            return Err(ApiError::Decode("gateway URL missing from response".into()));
        }
        Ok(g.url)
    }

    // ---- guilds ----

    pub async fn guild_channels(&self, guild: Id) -> Result<Vec<Channel>, ApiError> {
        self.json(Request::new(Method::GET, format!("/guilds/{guild}/channels"))).await
    }

    pub async fn guild_member(&self, guild: Id, user: Id) -> Result<Member, ApiError> {
        self.json(Request::new(Method::GET, format!("/guilds/{guild}/members/{user}"))).await
    }

    /// One page of members (`limit` clamped to 1..=1000). Requires the GUILD_MEMBERS intent.
    pub async fn list_guild_members(&self, guild: Id, after: Option<Id>, limit: u16) -> Result<Vec<Member>, ApiError> {
        let mut req = Request::new(Method::GET, format!("/guilds/{guild}/members")).query("limit", limit.clamp(1, 1000));
        if let Some(a) = after {
            req = req.query("after", a);
        }
        self.json(req).await
    }

    pub async fn active_threads(&self, guild: Id) -> Result<Vec<Channel>, ApiError> {
        let r: ThreadsResponse = self.json(Request::new(Method::GET, format!("/guilds/{guild}/threads/active"))).await?;
        Ok(r.threads)
    }

    /// Guild message search. Bots need READ_MESSAGE_HISTORY and the MESSAGE_CONTENT intent.
    pub async fn search_guild_messages(&self, guild: Id, q: &SearchQuery) -> Result<SearchOutcome, ApiError> {
        if q.offset > 9975 {
            return Err(ApiError::Invalid("search offset must be at most 9975".into()));
        }
        let mut req = Request::new(Method::GET, format!("/guilds/{guild}/messages/search"));
        if let Some(content) = q.content.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            if content.chars().count() > 1024 {
                return Err(ApiError::Invalid("search text must be at most 1024 characters".into()));
            }
            req = req.query("content", content);
        }
        for id in q.channel_ids.iter().take(500) {
            req = req.query("channel_id", id);
        }
        for id in q.author_ids.iter().take(100) {
            req = req.query("author_id", id);
        }
        for h in &q.has {
            req = req.query("has", h);
        }
        req = req.query("limit", if q.limit == 0 { 25 } else { q.limit.clamp(1, 25) });
        if q.offset > 0 {
            req = req.query("offset", q.offset);
        }
        if let Some(s) = &q.sort_by {
            req = req.query("sort_by", s);
        }
        if let Some(s) = &q.sort_order {
            req = req.query("sort_order", s);
        }
        let (status, body) = self.execute(req).await?;
        if status == StatusCode::ACCEPTED {
            let r: IndexingResponse = serde_json::from_slice(&body).unwrap_or_default();
            return Ok(SearchOutcome::Indexing { retry_after: r.retry_after.unwrap_or(2.0).max(0.0) });
        }
        let r: SearchResponse = serde_json::from_slice(&body).map_err(ApiError::decode)?;
        let messages = r.messages.into_iter().filter_map(|group| group.into_iter().next()).collect();
        Ok(SearchOutcome::Results { total: r.total_results, messages })
    }

    // ---- channels / messages ----

    pub async fn channel(&self, id: Id) -> Result<Channel, ApiError> {
        self.json(Request::new(Method::GET, format!("/channels/{id}"))).await
    }

    /// Messages in Discord's order (newest first).
    pub async fn messages(&self, channel: Id, page: MessagePage) -> Result<Vec<Message>, ApiError> {
        let mut req = Request::new(Method::GET, format!("/channels/{channel}/messages")).query("limit", page.limit.clamp(1, 100));
        if let Some(id) = page.around {
            req = req.query("around", id);
        } else if let Some(id) = page.before {
            req = req.query("before", id);
        } else if let Some(id) = page.after {
            req = req.query("after", id);
        }
        self.json(req).await
    }

    pub async fn message(&self, channel: Id, id: Id) -> Result<Message, ApiError> {
        self.json(Request::new(Method::GET, format!("/channels/{channel}/messages/{id}"))).await
    }

    /// Sends JSON, or multipart (`payload_json` + `files[n]`) when files are attached.
    pub async fn create_message(&self, channel: Id, msg: &CreateMessage) -> Result<Message, ApiError> {
        if msg.content.trim().is_empty() && msg.files.is_empty() && msg.sticker_ids.is_empty() {
            return Err(ApiError::Invalid("the message is empty".into()));
        }
        if msg.files.len() > MAX_FILES {
            return Err(ApiError::Invalid(format!("at most {MAX_FILES} files can be attached to one message")));
        }
        if msg.sticker_ids.len() > 3 {
            return Err(ApiError::Invalid("at most 3 stickers can be sent in one message".into()));
        }
        let mut payload = serde_json::Map::new();
        payload.insert("content".into(), json!(msg.content));
        payload.insert("tts".into(), json!(msg.tts));
        payload.insert(
            "allowed_mentions".into(),
            json!({ "parse": ["users", "roles", "everyone"], "replied_user": msg.mention_replied_user }),
        );
        if let Some(nonce) = msg.nonce.as_deref().filter(|n| !n.is_empty()) {
            if nonce.len() > 25 {
                return Err(ApiError::Invalid("message nonce must be at most 25 characters".into()));
            }
            payload.insert("nonce".into(), json!(nonce));
            payload.insert("enforce_nonce".into(), json!(true));
        }
        if let Some(reply) = msg.reply_to {
            payload.insert("message_reference".into(), json!({ "message_id": reply, "fail_if_not_exists": false }));
        }
        if !msg.sticker_ids.is_empty() {
            payload.insert("sticker_ids".into(), json!(msg.sticker_ids));
        }
        let path = format!("/channels/{channel}/messages");
        let req = if msg.files.is_empty() {
            Request::new(Method::POST, path).json(&payload)?
        } else {
            let files: Vec<PreparedFile> = msg
                .files
                .iter()
                .map(|f| {
                    let name = f.upload_name();
                    let mime = f
                        .content_type
                        .clone()
                        .filter(|c| c.contains('/') && HeaderValue::from_str(c).is_ok())
                        .unwrap_or_else(|| guess_mime(&name).to_string());
                    PreparedFile { name, mime, bytes: Bytes::copy_from_slice(&f.bytes) }
                })
                .collect();
            let attachments: Vec<_> = files.iter().enumerate().map(|(i, f)| json!({ "id": i, "filename": f.name })).collect();
            payload.insert("attachments".into(), json!(attachments));
            let payload = serde_json::to_string(&payload).map_err(|e| ApiError::Invalid(e.to_string()))?;
            let mut req = Request::new(Method::POST, path);
            req.body = Body::Multipart { payload, files };
            req.timeout = UPLOAD_TIMEOUT;
            req
        };
        self.json(req).await
    }

    pub async fn edit_message(&self, channel: Id, id: Id, content: &str) -> Result<Message, ApiError> {
        let req = Request::new(Method::PATCH, format!("/channels/{channel}/messages/{id}")).json(&json!({ "content": content }))?;
        self.json(req).await
    }

    pub async fn delete_message(&self, channel: Id, id: Id) -> Result<(), ApiError> {
        self.empty(Request::new(Method::DELETE, format!("/channels/{channel}/messages/{id}"))).await
    }

    pub async fn add_reaction(&self, channel: Id, message: Id, emoji: &Emoji) -> Result<(), ApiError> {
        let path = format!("/channels/{channel}/messages/{message}/reactions/{}/@me", encode_emoji(emoji)?);
        self.empty(Request::new(Method::PUT, path)).await
    }

    pub async fn remove_own_reaction(&self, channel: Id, message: Id, emoji: &Emoji) -> Result<(), ApiError> {
        let path = format!("/channels/{channel}/messages/{message}/reactions/{}/@me", encode_emoji(emoji)?);
        self.empty(Request::new(Method::DELETE, path)).await
    }

    /// One page of pins, newest first. `before` is the ISO8601 `pinned_at` of the
    /// last pin of the previous page; `limit` is clamped to 1..=50.
    pub async fn pins(&self, channel: Id, before: Option<&str>, limit: u8) -> Result<PinsPage, ApiError> {
        let mut req = Request::new(Method::GET, format!("/channels/{channel}/messages/pins")).query("limit", limit.clamp(1, 50));
        if let Some(b) = before.filter(|b| !b.is_empty()) {
            req = req.query("before", b);
        }
        self.json(req).await
    }

    pub async fn pin(&self, channel: Id, message: Id) -> Result<(), ApiError> {
        self.empty(Request::new(Method::PUT, format!("/channels/{channel}/messages/pins/{message}"))).await
    }

    pub async fn unpin(&self, channel: Id, message: Id) -> Result<(), ApiError> {
        self.empty(Request::new(Method::DELETE, format!("/channels/{channel}/messages/pins/{message}"))).await
    }

    pub async fn trigger_typing(&self, channel: Id) -> Result<(), ApiError> {
        self.empty(Request::new(Method::POST, format!("/channels/{channel}/typing"))).await
    }

    pub async fn start_thread_from_message(&self, channel: Id, message: Id, name: &str) -> Result<Channel, ApiError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 100 {
            return Err(ApiError::Invalid("thread names must be 1-100 characters".into()));
        }
        let req = Request::new(Method::POST, format!("/channels/{channel}/messages/{message}/threads")).json(&json!({ "name": name }))?;
        self.json(req).await
    }

    /// Fetches a CDN/media URL without credentials, failing with
    /// [`ApiError::TooLarge`] once more than `max_bytes` would be read.
    pub async fn download(&self, url: &str, max_bytes: u64) -> Result<Bytes, ApiError> {
        let parsed = url::Url::parse(url).map_err(|e| ApiError::Invalid(format!("invalid URL: {e}")))?;
        if !matches!(parsed.scheme(), "https" | "http") {
            return Err(ApiError::Invalid(format!("unsupported URL scheme `{}`", parsed.scheme())));
        }
        let mut resp = self.inner.http.get(parsed).timeout(DOWNLOAD_TIMEOUT).send().await.map_err(ApiError::network)?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await.unwrap_or_default();
            return Err(ApiError::from_response(status.as_u16(), &body));
        }
        if resp.content_length().is_some_and(|len| len > max_bytes) {
            return Err(ApiError::TooLarge);
        }
        let initial = resp.content_length().unwrap_or(0).min(max_bytes).min(16 << 20) as usize;
        let mut buf = BytesMut::with_capacity(initial);
        while let Some(chunk) = resp.chunk().await.map_err(ApiError::network)? {
            if (buf.len() + chunk.len()) as u64 > max_bytes {
                return Err(ApiError::TooLarge);
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf.freeze())
    }

    // ---- transport ----

    async fn json<T: DeserializeOwned>(&self, req: Request) -> Result<T, ApiError> {
        let (_, body) = self.execute(req).await?;
        serde_json::from_slice(&body).map_err(ApiError::decode)
    }

    async fn empty(&self, req: Request) -> Result<(), ApiError> {
        self.execute(req).await.map(|_| ())
    }

    fn build(&self, req: &Request, url: &str) -> Result<reqwest::RequestBuilder, ApiError> {
        let mut rb =
            self.inner.http.request(req.method.clone(), url).header(AUTHORIZATION, self.inner.auth_header.clone()).timeout(req.timeout);
        if !req.query.is_empty() {
            rb = rb.query(&req.query);
        }
        Ok(match &req.body {
            // Discord answers 411 to body-less writes without an explicit length.
            Body::Empty if matches!(req.method, Method::POST | Method::PUT | Method::PATCH) => rb.header(CONTENT_LENGTH, 0),
            Body::Empty => rb,
            Body::Json(bytes) => rb.header(CONTENT_TYPE, "application/json").body(bytes.clone()),
            Body::Multipart { payload, files } => rb.multipart(build_form(payload, files)?),
        })
    }

    /// Sends a request through its rate-limit bucket, retrying 429s and (for
    /// idempotent methods) transient failures. Returns the status and body of a 2xx.
    async fn execute(&self, req: Request) -> Result<(StatusCode, Bytes), ApiError> {
        let inner = &*self.inner;
        let route = Route::new(&req.method, &req.path);
        let idempotent = matches!(req.method, Method::GET | Method::HEAD | Method::PUT | Method::DELETE);
        let url = format!("{}{}", inner.base, req.path);
        let bucket = inner.limiter.bucket(&route);
        let mut state = bucket.state.lock().await;
        let mut rate_limit_retries = 0;
        let mut transient_retries = 0;
        loop {
            inner.limiter.wait_global().await;
            if let Some(wait) = state.wait_time(Instant::now()) {
                tracing::debug!(route = %route.key, ?wait, "rate-limit bucket exhausted; waiting");
                tokio::time::sleep(wait).await;
            }
            let resp = match self.build(&req, &url)?.send().await {
                Ok(r) => r,
                Err(e) if idempotent && transient_retries < MAX_TRANSIENT_RETRIES && !e.is_builder() => {
                    transient_retries += 1;
                    tracing::debug!(route = %route.key, error = %e.without_url(), "transient network error; retrying");
                    tokio::time::sleep(backoff(transient_retries)).await;
                    continue;
                }
                Err(e) => return Err(ApiError::network(e)),
            };
            let status = resp.status();
            let headers = resp.headers().clone();
            state.update(&headers, Instant::now());
            if let Some(hash) = header_str(&headers, "x-ratelimit-bucket") {
                inner.limiter.learn(&route, hash, &bucket);
            }
            if status == StatusCode::TOO_MANY_REQUESTS {
                let body = resp.bytes().await.unwrap_or_default();
                let (retry_after, global) = parse_429(&headers, &body);
                tracing::warn!(
                    route = %route.key,
                    retry_after,
                    global,
                    scope = header_str(&headers, "x-ratelimit-scope").unwrap_or("unknown"),
                    "rate limited by Discord"
                );
                if global {
                    inner.limiter.set_global(retry_after);
                } else {
                    state.block_for(retry_after, Instant::now());
                }
                if rate_limit_retries >= MAX_RATE_LIMIT_RETRIES {
                    return Err(ApiError::RateLimited { retry_after, global });
                }
                rate_limit_retries += 1;
                continue;
            }
            if idempotent && matches!(status.as_u16(), 502..=504) && transient_retries < MAX_TRANSIENT_RETRIES {
                transient_retries += 1;
                tracing::debug!(route = %route.key, %status, "transient server error; retrying");
                tokio::time::sleep(backoff(transient_retries)).await;
                continue;
            }
            let body = resp.bytes().await.map_err(ApiError::network)?;
            if status.is_success() {
                return Ok((status, body));
            }
            return Err(ApiError::from_response(status.as_u16(), &body));
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250 << attempt.saturating_sub(1).min(4))
}

/// `(retry_after_seconds, global)` from a 429; prefers the precise JSON value over
/// the integer `Retry-After` header.
fn parse_429(headers: &HeaderMap, body: &[u8]) -> (f64, bool) {
    let parsed: RateLimitBody = serde_json::from_slice(body).unwrap_or_default();
    let retry_after = parsed
        .retry_after
        .or_else(|| header_parse::<f64>(headers, "retry-after"))
        .or_else(|| header_parse::<f64>(headers, "x-ratelimit-reset-after"))
        .unwrap_or(1.0)
        .max(0.0);
    let global = parsed.global
        || header_str(headers, "x-ratelimit-global").is_some_and(|v| v.eq_ignore_ascii_case("true"))
        || header_str(headers, "x-ratelimit-scope") == Some("global");
    (retry_after, global)
}

fn encode_emoji(emoji: &Emoji) -> Result<String, ApiError> {
    let key = emoji.reaction_key();
    if key.is_empty() {
        return Err(ApiError::Invalid("the emoji has neither an id nor a name".into()));
    }
    Ok(utf8_percent_encode(&key, PATH_SEGMENT).to_string())
}

fn build_form(payload: &str, files: &[PreparedFile]) -> Result<Form, ApiError> {
    let invalid = |e: reqwest::Error| ApiError::Invalid(e.to_string());
    let mut form = Form::new().part("payload_json", Part::text(payload.to_owned()).mime_str("application/json").map_err(invalid)?);
    for (i, f) in files.iter().enumerate() {
        let part =
            Part::stream_with_length(f.bytes.clone(), f.bytes.len() as u64).file_name(f.name.clone()).mime_str(&f.mime).map_err(invalid)?;
        form = form.part(format!("files[{i}]"), part);
    }
    Ok(form)
}

fn guess_mime(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "txt" | "log" => "text/plain",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_debug_redacts_token() {
        let a = Auth::Bot("super.secret.token".into());
        assert_eq!(format!("{a:?}"), "Bot(<redacted>)");
        let c = RestClient::new(Auth::Bearer("abc123".into())).unwrap();
        assert!(!format!("{c:?}").contains("abc123"));
    }

    #[test]
    fn rejects_bad_tokens() {
        assert!(matches!(RestClient::new(Auth::Bot("  ".into())), Err(ApiError::Invalid(_))));
        assert!(matches!(RestClient::new(Auth::Bot("a\nb".into())), Err(ApiError::Invalid(_))));
    }

    #[test]
    fn emoji_encoding() {
        assert_eq!(encode_emoji(&Emoji::unicode("👍")).unwrap(), "%F0%9F%91%8D");
        assert_eq!(encode_emoji(&Emoji::custom(Id(123), "blob_wave", false)).unwrap(), "blob_wave%3A123");
        assert!(encode_emoji(&Emoji::default()).is_err());
    }

    #[test]
    fn spoiler_and_path_stripping() {
        let f = FileUpload { filename: "C:\\pics\\cat.png".into(), spoiler: true, ..Default::default() };
        assert_eq!(f.upload_name(), "SPOILER_cat.png");
        let f = FileUpload { filename: "SPOILER_x.txt".into(), spoiler: true, ..Default::default() };
        assert_eq!(f.upload_name(), "SPOILER_x.txt");
    }

    #[test]
    fn parse_429_prefers_body() {
        let mut h = HeaderMap::new();
        h.insert("retry-after", HeaderValue::from_static("1"));
        assert_eq!(parse_429(&h, br#"{"retry_after":0.25,"global":false}"#), (0.25, false));
        h.insert("x-ratelimit-global", HeaderValue::from_static("true"));
        assert_eq!(parse_429(&h, b""), (1.0, true));
    }
}
