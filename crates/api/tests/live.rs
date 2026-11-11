//! Tests against the real Discord API. Run with `cargo test -p dn-api -- --ignored`.

use dn_api::RestClient;

#[tokio::test]
#[ignore = "requires network access to discord.com"]
async fn live_gateway_url_unauthenticated() {
    let url = RestClient::gateway_url_unauthenticated(None).await.expect("GET /gateway failed");
    assert!(url.starts_with("wss://"), "unexpected gateway URL: {url}");
}
