use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use serde::de::DeserializeOwned;

use crate::error::{ApiError, AuthError, describe};

pub(crate) fn client() -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .user_agent(dn_core::user_agent())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(AuthError::from)
}

pub(crate) fn join(base: &str, path: &str) -> String {
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

/// GET `url` with an `Authorization` value such as `Bot <token>`; non-2xx becomes an `ApiError`.
pub(crate) async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    authorization: &str,
) -> Result<T, ApiError> {
    let mut value = HeaderValue::from_str(authorization).map_err(|_| ApiError {
        status: 401,
        code: None,
        message: "credential contains invalid characters".into(),
    })?;
    value.set_sensitive(true);
    let resp = client
        .get(url)
        .header(AUTHORIZATION, value)
        .send()
        .await
        .map_err(|e| ApiError::transport(describe(e)))?;
    if !resp.status().is_success() {
        return Err(ApiError::from_response(resp).await);
    }
    resp.json::<T>().await.map_err(|e| {
        ApiError::transport(format!(
            "unexpected response from Discord: {}",
            e.without_url()
        ))
    })
}
