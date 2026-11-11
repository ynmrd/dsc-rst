use std::fmt;

/// Errors surfaced to the UI. Messages are user-presentable and never contain secrets.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    #[error(
        "Port {0} on this computer is already in use or unavailable. Close the program using it, \
         or choose another sign-in port (it must match a redirect URI registered for your Discord application)."
    )]
    PortInUse(u16),
    #[error("Sign-in was cancelled or access was denied in Discord.")]
    Denied,
    #[error("The sign-in response did not match this request and was rejected. Please try signing in again.")]
    StateMismatch,
    #[error("Sign-in timed out while waiting for Discord. Please try again.")]
    Timeout,
    #[error("Sign-in was cancelled.")]
    Cancelled,
    #[error("Discord rejected this bot token. Check that you copied the complete token from the Bot page of your application.")]
    InvalidToken,
    #[error(
        "This is not a bot token. Discord Native only accepts the token of a bot application you own; never paste a personal account token."
    )]
    NotABot,
    #[error("Your Discord sign-in has expired. Please sign in again.")]
    SessionExpired,
    #[error("Discord returned an error (HTTP {status}): {message}")]
    Http { status: u16, message: String },
    #[error("Could not reach Discord: {0}")]
    Network(String),
    #[error("Could not access the system credential store: {0}")]
    Store(String),
    #[error("File error: {0}")]
    Io(String),
    #[error("{0}")]
    InvalidConfig(String),
    #[error("This account no longer exists ({0}).")]
    AccountNotFound(String),
    #[error("{0}")]
    Other(String),
}

/// Transport error text (including the underlying cause) without the request URL.
pub(crate) fn describe(e: reqwest::Error) -> String {
    use std::error::Error as _;
    let e = e.without_url();
    if e.is_timeout() {
        return "the request timed out".into();
    }
    let mut text = e.to_string();
    let mut source = e.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

impl From<reqwest::Error> for AuthError {
    fn from(e: reqwest::Error) -> Self {
        AuthError::Network(describe(e))
    }
}

impl From<std::io::Error> for AuthError {
    fn from(e: std::io::Error) -> Self {
        AuthError::Io(e.to_string())
    }
}

impl From<keyring::Error> for AuthError {
    fn from(e: keyring::Error) -> Self {
        AuthError::Store(e.to_string())
    }
}

impl From<serde_json::Error> for AuthError {
    fn from(e: serde_json::Error) -> Self {
        AuthError::Other(format!("unexpected data format: {e}"))
    }
}

/// Error body of a failed Discord request, kept structured so callers can branch on the OAuth `error` code.
pub(crate) struct ApiError {
    pub status: u16,
    pub code: Option<String>,
    pub message: String,
}

impl ApiError {
    pub async fn from_response(resp: reqwest::Response) -> Self {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Self::parse(status, &body)
    }

    pub fn parse(status: u16, body: &str) -> Self {
        #[derive(serde::Deserialize, Default)]
        #[serde(default)]
        struct Body {
            error: Option<String>,
            error_description: Option<String>,
            message: Option<String>,
        }
        let parsed: Body = serde_json::from_str(body).unwrap_or_default();
        let detail = parsed.error_description.or(parsed.message);
        let mut message = match (&parsed.error, detail) {
            (Some(code), Some(d)) => format!("{code}: {d}"),
            (Some(code), None) => code.clone(),
            (None, Some(d)) => d,
            (None, None) => body.trim().to_string(),
        };
        if parsed.error.as_deref() == Some("invalid_client") {
            message.push_str(" (check the client ID and secret, and enable \"Public Client\" for apps without a secret)");
        }
        Self { status, code: parsed.error, message: sanitize(&message, 300) }
    }

    /// Transport and decode failures never reach Discord's error format; they carry status 0.
    pub fn transport(message: String) -> Self {
        Self { status: 0, code: None, message }
    }
}

impl From<ApiError> for AuthError {
    fn from(e: ApiError) -> Self {
        if e.status == 0 { AuthError::Network(e.message) } else { AuthError::Http { status: e.status, message: e.message } }
    }
}

/// Strips control characters and caps length for text that originates from the network.
pub(crate) fn sanitize(s: &str, max_chars: usize) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    let mut out: String = cleaned.chars().take(max_chars).collect();
    if cleaned.chars().count() > max_chars {
        out.push('…');
    }
    out
}

/// Debug helper so secret-bearing fields share one redaction format.
pub(crate) struct Redacted<'a, T>(pub &'a Option<T>);

impl<T> fmt::Debug for Redacted<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(_) => f.write_str("Some(<redacted>)"),
            None => f.write_str("None"),
        }
    }
}
