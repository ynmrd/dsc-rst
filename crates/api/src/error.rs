use serde::Deserialize;

/// REST failure. `Display` output is suitable for showing to the user.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ApiError {
    #[error("Discord rejected the credentials (401 Unauthorized). Please sign in again.")]
    Unauthorized,
    #[error("Missing permissions: {message}")]
    Forbidden { code: Option<u64>, message: String },
    #[error("Not found: the item does not exist or you cannot access it.")]
    NotFound,
    #[error("Rate limited by Discord. Try again in {retry_after:.1} s.")]
    RateLimited { retry_after: f64, global: bool },
    #[error("Discord returned HTTP {status}: {message}")]
    Http { status: u16, code: Option<u64>, message: String },
    #[error("Network error: {0}")]
    Network(String),
    #[error("Unexpected response from Discord: {0}")]
    Decode(String),
    #[error("The file is larger than the allowed size.")]
    TooLarge,
    #[error("Invalid request: {0}")]
    Invalid(String),
}

impl ApiError {
    /// Whether retrying the same request later may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            ApiError::RateLimited { .. } | ApiError::Network(_) => true,
            ApiError::Http { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// Discord JSON error code (https://discord.com/developers/docs/topics/opcodes-and-status-codes#json).
    pub fn code(&self) -> Option<u64> {
        match self {
            ApiError::Forbidden { code, .. } | ApiError::Http { code, .. } => *code,
            _ => None,
        }
    }

    pub(crate) fn network(e: reqwest::Error) -> Self {
        let e = e.without_url();
        if e.is_timeout() {
            ApiError::Network("the request timed out".into())
        } else if e.is_connect() {
            ApiError::Network(format!("could not connect to Discord ({e})"))
        } else {
            ApiError::Network(e.to_string())
        }
    }

    pub(crate) fn decode(e: serde_json::Error) -> Self {
        ApiError::Decode(e.to_string())
    }

    /// Maps a non-success, non-429 response to an error.
    pub(crate) fn from_response(status: u16, body: &[u8]) -> Self {
        let (code, message) = describe_error(status, body);
        match status {
            401 => ApiError::Unauthorized,
            403 => ApiError::Forbidden { code, message },
            404 => ApiError::NotFound,
            _ => ApiError::Http { status, code, message },
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ErrorBody {
    code: Option<u64>,
    message: Option<String>,
    errors: Option<serde_json::Value>,
}

/// Extracts `(code, message)` from a Discord error body, appending field errors
/// such as `content: Must be 2000 or fewer in length.`.
pub(crate) fn describe_error(status: u16, body: &[u8]) -> (Option<u64>, String) {
    let fallback = || reqwest::StatusCode::from_u16(status).ok().and_then(|s| s.canonical_reason()).unwrap_or("request failed").to_string();
    let Ok(parsed) = serde_json::from_slice::<ErrorBody>(body) else {
        return (None, fallback());
    };
    let mut message = parsed.message.filter(|m| !m.is_empty()).unwrap_or_else(fallback);
    if let Some(errors) = &parsed.errors {
        let mut details = Vec::new();
        collect_field_errors(errors, &mut String::new(), &mut details);
        if !details.is_empty() {
            details.truncate(5);
            message = format!("{message} ({})", details.join("; "));
        }
    }
    (parsed.code, message)
}

fn collect_field_errors(v: &serde_json::Value, path: &mut String, out: &mut Vec<String>) {
    let serde_json::Value::Object(map) = v else { return };
    for (key, value) in map {
        if key == "_errors" {
            for e in value.as_array().into_iter().flatten() {
                if let Some(msg) = e.get("message").and_then(|m| m.as_str()) {
                    out.push(if path.is_empty() { msg.to_string() } else { format!("{path}: {msg}") });
                }
            }
            continue;
        }
        let len = path.len();
        if key.bytes().all(|b| b.is_ascii_digit()) {
            path.push_str(&format!("[{key}]"));
        } else {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(key);
        }
        collect_field_errors(value, path, out);
        path.truncate(len);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_body_errors_are_flattened() {
        let body = br#"{"code":50035,"message":"Invalid Form Body","errors":{"content":{"_errors":[{"code":"BASE_TYPE_MAX_LENGTH","message":"Must be 2000 or fewer in length."}]},"embeds":{"0":{"title":{"_errors":[{"code":"X","message":"Too long."}]}}}}}"#;
        let (code, msg) = describe_error(400, body);
        assert_eq!(code, Some(50035));
        assert_eq!(msg, "Invalid Form Body (content: Must be 2000 or fewer in length.; embeds[0].title: Too long.)");
    }

    #[test]
    fn non_json_body_uses_reason() {
        assert_eq!(describe_error(502, b"<html>").1, "Bad Gateway");
        assert!(matches!(ApiError::from_response(403, br#"{"code":50013,"message":"Missing Permissions"}"#),
            ApiError::Forbidden { code: Some(50013), ref message } if message == "Missing Permissions"));
        assert_eq!(ApiError::from_response(404, b""), ApiError::NotFound);
    }
}
