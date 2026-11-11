use std::path::{Path, PathBuf};
use std::sync::Arc;

use dn_auth::accounts::{AccountManager, bot_invite_url};
use dn_auth::oauth::{OAuthConfig, OAuthTokens};
use dn_auth::{AuthError, MemoryStore, SecretStore};
use dn_core::{Id, User};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TOKEN: &str = "MTIzNDU2Nzg5MDEyMzQ1Njc4.GabcDE.bot-token-secret-value";
const TOKEN_2: &str = "MTIzNDU2Nzg5MDEyMzQ1Njc4.GxyzXY.rotated-bot-token-secret";
const BOT_ID: &str = "111111111111111111";
const APP_ID: &str = "222222222222222222";

struct Env {
    dir: tempfile::TempDir,
    store: Arc<MemoryStore>,
    server: MockServer,
}

impl Env {
    async fn new() -> Self {
        Self { dir: tempfile::tempdir().unwrap(), store: Arc::new(MemoryStore::new()), server: MockServer::start().await }
    }

    fn file(&self) -> PathBuf {
        self.dir.path().join("nested").join("accounts.json")
    }

    fn manager(&self) -> AccountManager {
        AccountManager::load(self.file(), self.store.clone())
            .unwrap()
            .with_oauth_endpoints(format!("{}/token", self.server.uri()), format!("{}/revoke", self.server.uri()))
    }

    async fn mock_bot(&self, token: &str, user_id: &str, name: &str, bot: bool, app_id: &str) {
        Mock::given(method("GET"))
            .and(path("/users/@me"))
            .and(header("authorization", format!("Bot {token}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": user_id, "username": name, "discriminator": "0", "avatar": "abc", "bot": bot
            })))
            .mount(&self.server)
            .await;
        Mock::given(method("GET"))
            .and(path("/applications/@me"))
            .and(header("authorization", format!("Bot {token}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": app_id, "name": name})))
            .mount(&self.server)
            .await;
    }

    async fn requests(&self, p: &str) -> Vec<Request> {
        self.server.received_requests().await.unwrap().into_iter().filter(|r| r.url.path() == p).collect()
    }

    fn base(&self) -> String {
        self.server.uri()
    }
}

fn human() -> User {
    User { id: Id(5), username: "human".into(), global_name: Some("Human".into()), ..Default::default() }
}

fn tokens(expires_in: i64) -> OAuthTokens {
    OAuthTokens {
        access_token: "access-token-secret".into(),
        refresh_token: Some("refresh-token-secret".into()),
        expires_at: chrono_now() + expires_in,
        scope: "identify guilds".into(),
        token_type: "Bearer".into(),
    }
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn oauth_cfg(secret: Option<&str>) -> OAuthConfig {
    OAuthConfig {
        client_id: "123456789012345678".into(),
        client_secret: secret.map(str::to_string),
        redirect_port: 53134,
        ..Default::default()
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

async fn linked_manager(env: &Env, secret: Option<&str>, expires_in: i64) -> (AccountManager, String) {
    env.mock_bot(TOKEN, BOT_ID, "helper-bot", true, APP_ID).await;
    let mut mgr = env.manager();
    let rec = mgr.add_bot_account(TOKEN, &env.base()).await.unwrap();
    mgr.link_oauth(&rec.id, &oauth_cfg(secret), &tokens(expires_in), &human()).unwrap();
    (mgr, rec.id)
}

#[tokio::test]
async fn adds_a_valid_bot_and_stores_the_token_only_in_the_secret_store() {
    let env = Env::new().await;
    env.mock_bot(TOKEN, BOT_ID, "helper-bot", true, APP_ID).await;
    let mut mgr = env.manager();

    let rec = mgr.add_bot_account(&format!("  Bot {TOKEN}\n"), &env.base()).await.unwrap();

    assert_eq!(rec.bot_user.id, Id(111111111111111111));
    assert_eq!(rec.bot_user.username, "helper-bot");
    assert_eq!(rec.bot_user.avatar.as_deref(), Some("abc"));
    assert_eq!(rec.application_id, Some(Id(222222222222222222)));
    assert_eq!(rec.label, "helper-bot");
    assert_eq!(rec.id.len(), 16);
    assert!(rec.oauth.is_none() && !rec.credentials_invalid);
    assert_eq!(mgr.active().map(|a| a.id.clone()), Some(rec.id.clone()));
    assert_eq!(mgr.bot_token(&rec.id).unwrap().as_deref(), Some(TOKEN));
    assert_eq!(env.store.keys(), vec![format!("bot:{}", rec.id)]);

    let file = read(&env.file());
    assert!(!file.contains(TOKEN) && !file.contains("MTIzNDU2"), "token leaked into accounts file");
    assert!(file.contains("helper-bot"));
    assert!(!env.file().with_file_name("accounts.json.tmp").exists());

    let req = &env.requests("/users/@me").await[0];
    assert!(req.headers.get("user-agent").unwrap().to_str().unwrap().starts_with("DiscordBot ("));
}

#[tokio::test]
async fn rejects_user_tokens_that_are_not_bots() {
    let env = Env::new().await;
    env.mock_bot(TOKEN, "999", "a-human", false, APP_ID).await;
    let mut mgr = env.manager();

    let err = mgr.add_bot_account(TOKEN, &env.base()).await.unwrap_err();

    assert_eq!(err, AuthError::NotABot);
    assert!(mgr.accounts().is_empty());
    assert!(env.store.keys().is_empty());
    assert!(env.requests("/applications/@me").await.is_empty());
    assert!(!env.file().exists());
}

#[tokio::test]
async fn unauthorized_token_is_invalid() {
    let env = Env::new().await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"message": "401: Unauthorized", "code": 0})))
        .mount(&env.server)
        .await;
    let mut mgr = env.manager();

    assert_eq!(mgr.add_bot_account("bad-token", &env.base()).await.unwrap_err(), AuthError::InvalidToken);
    assert!(mgr.accounts().is_empty());
    assert!(env.store.keys().is_empty());
}

#[tokio::test]
async fn invalid_application_lookup_is_invalid_token() {
    let env = Env::new().await;
    Mock::given(method("GET"))
        .and(path("/users/@me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": BOT_ID, "username": "b", "bot": true})))
        .mount(&env.server)
        .await;
    Mock::given(method("GET")).and(path("/applications/@me")).respond_with(ResponseTemplate::new(401)).mount(&env.server).await;
    let mut mgr = env.manager();
    assert_eq!(mgr.add_bot_account(TOKEN, &env.base()).await.unwrap_err(), AuthError::InvalidToken);
    assert!(env.store.keys().is_empty());
}

#[tokio::test]
async fn malformed_tokens_never_reach_the_network() {
    let env = Env::new().await;
    let mut mgr = env.manager();
    for bad in ["", "   ", "Bot ", "two words", "abc\r\nX-Injected: 1"] {
        assert_eq!(mgr.add_bot_account(bad, &env.base()).await.unwrap_err(), AuthError::InvalidToken, "{bad:?}");
    }
    assert!(env.server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn server_errors_surface_as_http_errors() {
    let env = Env::new().await;
    Mock::given(method("GET")).and(path("/users/@me")).respond_with(ResponseTemplate::new(503)).mount(&env.server).await;
    let mut mgr = env.manager();
    assert!(matches!(mgr.add_bot_account(TOKEN, &env.base()).await.unwrap_err(), AuthError::Http { status: 503, .. }));
}

#[tokio::test]
async fn re_adding_the_same_bot_updates_its_token_instead_of_duplicating() {
    let env = Env::new().await;
    env.mock_bot(TOKEN, BOT_ID, "helper-bot", true, APP_ID).await;
    env.mock_bot(TOKEN_2, BOT_ID, "helper-bot-renamed", true, APP_ID).await;
    let mut mgr = env.manager();

    let first = mgr.add_bot_account(TOKEN, &env.base()).await.unwrap();
    mgr.mark_credentials_invalid(&first.id, true).unwrap();
    mgr.update_label(&first.id, "My label").unwrap();
    let second = mgr.add_bot_account(TOKEN_2, &env.base()).await.unwrap();

    assert_eq!(mgr.accounts().len(), 1);
    assert_eq!(first.id, second.id);
    assert_eq!(second.label, "My label", "a user-chosen label survives re-adding");
    assert_eq!(second.bot_user.username, "helper-bot-renamed");
    assert!(!second.credentials_invalid, "fresh credentials clear the invalid flag");
    assert_eq!(mgr.bot_token(&first.id).unwrap().as_deref(), Some(TOKEN_2));
    assert_eq!(env.store.keys().len(), 1);
}

#[tokio::test]
async fn multiple_accounts_active_selection_and_not_found() {
    let env = Env::new().await;
    env.mock_bot(TOKEN, BOT_ID, "first-bot", true, APP_ID).await;
    env.mock_bot(TOKEN_2, "333333333333333333", "second-bot", true, "444444444444444444").await;
    let mut mgr = env.manager();

    let a = mgr.add_bot_account(TOKEN, &env.base()).await.unwrap();
    let b = mgr.add_bot_account(TOKEN_2, &env.base()).await.unwrap();
    assert_ne!(a.id, b.id);
    assert_eq!(mgr.active().unwrap().id, b.id);

    mgr.set_active(&a.id).unwrap();
    assert_eq!(mgr.active().unwrap().id, a.id);
    assert_eq!(mgr.set_active("nope").unwrap_err(), AuthError::AccountNotFound("nope".into()));
    assert_eq!(mgr.update_label("nope", "x").unwrap_err(), AuthError::AccountNotFound("nope".into()));
    assert!(matches!(mgr.update_label(&a.id, "   "), Err(AuthError::InvalidConfig(_))));
    assert_eq!(mgr.bot_token("nope").unwrap(), None);

    let reloaded = AccountManager::load(env.file(), env.store.clone()).unwrap();
    assert_eq!(reloaded.accounts().len(), 2);
    assert_eq!(reloaded.active().unwrap().id, a.id);
}

#[tokio::test]
async fn persistence_roundtrip_keeps_secrets_out_of_the_file() {
    let env = Env::new().await;
    let (mut mgr, id) = linked_manager(&env, Some("client-secret-value"), 3600).await;
    mgr.update_label(&id, "Work bot").unwrap();
    mgr.mark_credentials_invalid(&id, true).unwrap();

    let file = read(&env.file());
    for secret in [TOKEN, "access-token-secret", "refresh-token-secret", "client-secret-value"] {
        assert!(!file.contains(secret), "{secret} leaked into the accounts file");
    }

    let reloaded = AccountManager::load(env.file(), env.store.clone()).unwrap();
    assert_eq!(reloaded.accounts().len(), 1);
    let rec = &reloaded.accounts()[0];
    assert_eq!(rec.id, id);
    assert_eq!(rec.label, "Work bot");
    assert!(rec.credentials_invalid);
    let link = rec.oauth.as_ref().unwrap();
    assert_eq!(link.client_id, "123456789012345678");
    assert_eq!(link.user.username, "human");
    assert_eq!(link.scopes, vec!["identify", "guilds"]);
    assert_eq!(link.redirect_port, 53134);
    assert!(link.has_secret && !link.expired);
    assert_eq!(reloaded.bot_token(&id).unwrap().as_deref(), Some(TOKEN));
    assert_eq!(env.store.keys(), vec![format!("bot:{id}"), format!("oauth-secret:{id}"), format!("oauth:{id}")]);
}

#[tokio::test]
async fn corrupt_file_is_backed_up_and_manager_starts_empty() {
    let env = Env::new().await;
    std::fs::create_dir_all(env.file().parent().unwrap()).unwrap();
    std::fs::write(env.file(), "{ this is not json").unwrap();

    let mut mgr = env.manager();
    assert!(mgr.accounts().is_empty() && mgr.active().is_none());
    let backup = env.file().with_file_name("accounts.json.corrupt");
    assert_eq!(read(&backup), "{ this is not json");
    assert!(!env.file().exists());

    env.mock_bot(TOKEN, BOT_ID, "helper-bot", true, APP_ID).await;
    mgr.add_bot_account(TOKEN, &env.base()).await.unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&read(&env.file())).is_ok());
    assert_eq!(read(&backup), "{ this is not json", "the backup is kept");
}

#[tokio::test]
async fn newer_schema_version_is_treated_like_a_corrupt_file() {
    let env = Env::new().await;
    std::fs::create_dir_all(env.file().parent().unwrap()).unwrap();
    std::fs::write(env.file(), r#"{"version": 99, "active": null, "accounts": []}"#).unwrap();
    let mgr = env.manager();
    assert!(mgr.accounts().is_empty());
    assert!(env.file().with_file_name("accounts.json.corrupt").exists());
}

#[tokio::test]
async fn missing_file_yields_an_empty_manager() {
    let env = Env::new().await;
    let mgr = env.manager();
    assert!(mgr.accounts().is_empty() && mgr.active().is_none());
    assert!(!env.file().exists());
}

#[tokio::test]
async fn remove_account_deletes_all_secrets_revokes_and_fixes_active() {
    let env = Env::new().await;
    Mock::given(method("POST"))
        .and(path("/revoke"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(&env.server)
        .await;
    env.mock_bot(TOKEN_2, "333333333333333333", "second-bot", true, "444444444444444444").await;
    let (mut mgr, id) = linked_manager(&env, Some("client-secret-value"), 3600).await;
    let other = mgr.add_bot_account(TOKEN_2, &env.base()).await.unwrap();
    mgr.set_active(&id).unwrap();

    mgr.remove_account(&id).await.unwrap();

    assert_eq!(env.store.keys(), vec![format!("bot:{}", other.id)]);
    assert_eq!(mgr.accounts().len(), 1);
    assert_eq!(mgr.active().unwrap().id, other.id);
    let reloaded = AccountManager::load(env.file(), env.store.clone()).unwrap();
    assert_eq!(reloaded.accounts().len(), 1);

    let revokes = env.requests("/revoke").await;
    assert_eq!(revokes.len(), 1);
    let body: std::collections::HashMap<String, String> = url::form_urlencoded::parse(&revokes[0].body).into_owned().collect();
    assert_eq!(body["token"], "refresh-token-secret");
    assert_eq!(body["client_secret"], "client-secret-value");

    mgr.remove_account(&other.id).await.unwrap();
    assert!(env.store.keys().is_empty());
    assert!(mgr.active().is_none() && mgr.accounts().is_empty());
    assert_eq!(mgr.remove_account(&other.id).await.unwrap_err(), AuthError::AccountNotFound(other.id));
}

#[tokio::test]
async fn unlink_oauth_is_best_effort_and_always_deletes_secrets() {
    let env = Env::new().await;
    Mock::given(method("POST")).and(path("/revoke")).respond_with(ResponseTemplate::new(500)).mount(&env.server).await;
    let (mut mgr, id) = linked_manager(&env, None, 3600).await;

    mgr.unlink_oauth(&id).await.unwrap();

    assert_eq!(env.requests("/revoke").await.len(), 1);
    assert_eq!(env.store.keys(), vec![format!("bot:{id}")]);
    assert!(mgr.accounts()[0].oauth.is_none());
    assert!(mgr.oauth_tokens(&id).await.unwrap().is_none());
    mgr.unlink_oauth(&id).await.unwrap();
    assert_eq!(env.requests("/revoke").await.len(), 1, "nothing left to revoke");
}

#[tokio::test]
async fn fresh_tokens_are_returned_without_refreshing() {
    let env = Env::new().await;
    let (mut mgr, id) = linked_manager(&env, None, 3600).await;

    let t = mgr.oauth_tokens(&id).await.unwrap().unwrap();

    assert_eq!(t.access_token, "access-token-secret");
    assert!(env.requests("/token").await.is_empty());
}

#[tokio::test]
async fn expiring_tokens_are_refreshed_and_persisted() {
    let env = Env::new().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "access-2", "refresh_token": "refresh-2", "token_type": "Bearer",
            "expires_in": 604800, "scope": "identify guilds guilds.members.read"
        })))
        .mount(&env.server)
        .await;
    let (mut mgr, id) = linked_manager(&env, Some("client-secret-value"), 120).await;

    let t = mgr.oauth_tokens(&id).await.unwrap().unwrap();

    assert_eq!((t.access_token.as_str(), t.refresh_token.as_deref()), ("access-2", Some("refresh-2")));
    let reqs = env.requests("/token").await;
    assert_eq!(reqs.len(), 1);
    let body: std::collections::HashMap<String, String> = url::form_urlencoded::parse(&reqs[0].body).into_owned().collect();
    assert_eq!(body["grant_type"], "refresh_token");
    assert_eq!(body["refresh_token"], "refresh-token-secret");
    assert_eq!(body["client_id"], "123456789012345678");
    assert_eq!(body["client_secret"], "client-secret-value");

    // Persisted: a second call (and a reloaded manager) use the new tokens without another refresh.
    assert_eq!(mgr.oauth_tokens(&id).await.unwrap().unwrap().access_token, "access-2");
    let mut reloaded = AccountManager::load(env.file(), env.store.clone())
        .unwrap()
        .with_oauth_endpoints(format!("{}/token", env.base()), format!("{}/revoke", env.base()));
    assert_eq!(reloaded.oauth_tokens(&id).await.unwrap().unwrap().access_token, "access-2");
    assert_eq!(env.requests("/token").await.len(), 1);
    assert_eq!(mgr.accounts()[0].oauth.as_ref().unwrap().scopes.len(), 3);
    assert!(!read(&env.file()).contains("access-2"));
}

#[tokio::test]
async fn invalid_grant_marks_the_link_expired_and_persists_it() {
    let env = Env::new().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .mount(&env.server)
        .await;
    let (mut mgr, id) = linked_manager(&env, None, 10).await;

    assert_eq!(mgr.oauth_tokens(&id).await.unwrap_err(), AuthError::SessionExpired);
    assert!(mgr.accounts()[0].oauth.as_ref().unwrap().expired);

    assert_eq!(mgr.oauth_tokens(&id).await.unwrap_err(), AuthError::SessionExpired);
    assert_eq!(env.requests("/token").await.len(), 1, "no retry once the session is known to be expired");

    let reloaded = AccountManager::load(env.file(), env.store.clone()).unwrap();
    assert!(reloaded.accounts()[0].oauth.as_ref().unwrap().expired);

    // Signing in again clears the flag.
    mgr.link_oauth(&id, &oauth_cfg(None), &tokens(3600), &human()).unwrap();
    assert!(!mgr.accounts()[0].oauth.as_ref().unwrap().expired);
    assert!(mgr.oauth_tokens(&id).await.unwrap().is_some());
}

#[tokio::test]
async fn transient_refresh_failure_keeps_a_still_valid_token_but_not_an_expired_one() {
    let env = Env::new().await;
    Mock::given(method("POST")).and(path("/token")).respond_with(ResponseTemplate::new(500)).mount(&env.server).await;

    let (mut mgr, id) = linked_manager(&env, None, 120).await;
    let t = mgr.oauth_tokens(&id).await.unwrap().unwrap();
    assert_eq!(t.access_token, "access-token-secret");
    assert!(!mgr.accounts()[0].oauth.as_ref().unwrap().expired);

    mgr.link_oauth(&id, &oauth_cfg(None), &tokens(-5), &human()).unwrap();
    assert!(matches!(mgr.oauth_tokens(&id).await.unwrap_err(), AuthError::Http { status: 500, .. }));
    assert!(!mgr.accounts()[0].oauth.as_ref().unwrap().expired, "a server error is not a revoked session");
}

#[tokio::test]
async fn missing_or_unrefreshable_oauth_secrets_expire_the_link() {
    let env = Env::new().await;
    let (mut mgr, id) = linked_manager(&env, None, 3600).await;
    env.store.delete(&format!("oauth:{id}")).unwrap();
    assert_eq!(mgr.oauth_tokens(&id).await.unwrap_err(), AuthError::SessionExpired);
    assert!(mgr.accounts()[0].oauth.as_ref().unwrap().expired);

    let mut no_refresh = tokens(10);
    no_refresh.refresh_token = None;
    mgr.link_oauth(&id, &oauth_cfg(None), &no_refresh, &human()).unwrap();
    assert_eq!(mgr.oauth_tokens(&id).await.unwrap_err(), AuthError::SessionExpired);
    assert!(env.requests("/token").await.is_empty());
}

#[tokio::test]
async fn link_oauth_for_unknown_account_fails_and_secret_is_cleared_when_dropped() {
    let env = Env::new().await;
    let (mut mgr, id) = linked_manager(&env, Some("client-secret-value"), 3600).await;
    assert_eq!(mgr.link_oauth("nope", &oauth_cfg(None), &tokens(10), &human()).unwrap_err(), AuthError::AccountNotFound("nope".into()));
    assert!(env.store.get(&format!("oauth-secret:{id}")).unwrap().is_some());
    mgr.link_oauth(&id, &oauth_cfg(None), &tokens(3600), &human()).unwrap();
    assert!(env.store.get(&format!("oauth-secret:{id}")).unwrap().is_none());
    assert!(!mgr.accounts()[0].oauth.as_ref().unwrap().has_secret);
}

#[tokio::test]
async fn debug_output_never_contains_secrets() {
    let env = Env::new().await;
    let (mgr, id) = linked_manager(&env, Some("client-secret-value"), 3600).await;
    let t = tokens(3600);
    let cfg = oauth_cfg(Some("client-secret-value"));
    let out = format!("{mgr:?} {:?} {t:?} {cfg:?} {:?}", mgr.accounts(), dn_auth::oauth::pkce());
    for secret in [TOKEN, "access-token-secret", "refresh-token-secret", "client-secret-value"] {
        assert!(!out.contains(secret), "{secret} leaked via Debug");
    }
    assert!(out.contains(&id));
    assert!(!format!("{:?}", env.store).contains(TOKEN));
}

#[test]
fn invite_url_for_the_recommended_permissions() {
    let url = bot_invite_url(Id(222222222222222222), dn_auth::accounts::RECOMMENDED_BOT_PERMISSIONS, Some(Id(7)));
    let parsed = url::Url::parse(&url).unwrap();
    assert_eq!(parsed.host_str(), Some("discord.com"));
    assert_eq!(parsed.path(), "/oauth2/authorize");
    let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
    assert_eq!(q["client_id"], "222222222222222222");
    assert_eq!(q["scope"], "bot");
    assert_eq!(q["guild_id"], "7");
    assert_eq!(q["disable_guild_select"], "true");
    assert_eq!(q["permissions"].parse::<u64>().unwrap(), dn_auth::accounts::RECOMMENDED_BOT_PERMISSIONS);
}
