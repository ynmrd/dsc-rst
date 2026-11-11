//! Account records (non-secret JSON file) and their secrets (the injected [`SecretStore`]).
//!
//! Secret keys: `bot:{id}` (bot token), `oauth:{id}` (`OAuthTokens` JSON), `oauth-secret:{id}`
//! (optional OAuth client secret). Nothing secret is ever written to the JSON file.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dn_core::{Id, Permissions, User};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::oauth::{self, OAuthConfig, OAuthTokens};
use crate::store::SecretStore;
use crate::{AuthError, http, now_unix};

const SCHEMA_VERSION: u32 = 1;
/// Refresh OAuth tokens this long before they expire.
const REFRESH_MARGIN_SECS: i64 = 300;
const MAX_LABEL_CHARS: usize = 64;

/// Baseline permissions for a chat-client bot. `MANAGE_MESSAGES` is deliberately left out.
pub const RECOMMENDED_BOT_PERMISSIONS: u64 = Permissions::VIEW_CHANNEL.0
    | Permissions::SEND_MESSAGES.0
    | Permissions::SEND_MESSAGES_IN_THREADS.0
    | Permissions::CREATE_PUBLIC_THREADS.0
    | Permissions::EMBED_LINKS.0
    | Permissions::ATTACH_FILES.0
    | Permissions::READ_MESSAGE_HISTORY.0
    | Permissions::ADD_REACTIONS.0
    | Permissions::USE_EXTERNAL_EMOJIS.0
    | Permissions::USE_EXTERNAL_STICKERS.0
    | Permissions::PIN_MESSAGES.0
    | Permissions::CONNECT.0
    | Permissions::SPEAK.0
    | Permissions::USE_VAD.0;

/// URL that lets the user add their own bot to a server they manage.
pub fn bot_invite_url(application_id: Id, permissions: u64, guild_id: Option<Id>) -> String {
    let mut url = format!("https://discord.com/oauth2/authorize?client_id={application_id}&scope=bot&permissions={permissions}");
    if let Some(guild) = guild_id {
        url.push_str(&format!("&guild_id={guild}&disable_guild_select=true"));
    }
    url
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UserSummary {
    pub id: Id,
    pub username: String,
    pub global_name: Option<String>,
    pub avatar: Option<String>,
}

impl From<&User> for UserSummary {
    fn from(u: &User) -> Self {
        Self { id: u.id, username: u.username.clone(), global_name: u.global_name.clone(), avatar: u.avatar.clone() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthLink {
    pub client_id: String,
    pub user: UserSummary,
    pub scopes: Vec<String>,
    pub redirect_port: u16,
    pub has_secret: bool,
    /// The refresh token was rejected; the user has to sign in again.
    pub expired: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountRecord {
    pub id: String,
    pub label: String,
    pub bot_user: UserSummary,
    pub application_id: Option<Id>,
    pub oauth: Option<OAuthLink>,
    pub created_at: i64,
    pub last_used_at: i64,
    pub credentials_invalid: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct AccountsFile {
    version: u32,
    active: Option<String>,
    accounts: Vec<AccountRecord>,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self { version: SCHEMA_VERSION, active: None, accounts: Vec::new() }
    }
}

pub struct AccountManager {
    path: PathBuf,
    store: Arc<dyn SecretStore>,
    file: AccountsFile,
    token_url: Option<String>,
    revoke_url: Option<String>,
}

impl fmt::Debug for AccountManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountManager")
            .field("path", &self.path)
            .field("accounts", &self.file.accounts.len())
            .field("active", &self.file.active)
            .finish_non_exhaustive()
    }
}

fn bot_key(id: &str) -> String {
    format!("bot:{id}")
}
fn oauth_key(id: &str) -> String {
    format!("oauth:{id}")
}
fn oauth_secret_key(id: &str) -> String {
    format!("oauth-secret:{id}")
}

/// Accepts a token pasted with or without the `Bot ` prefix; refuses anything that is not a bare header-safe token.
fn normalize_bot_token(raw: &str) -> Result<String, AuthError> {
    let mut t = raw.trim();
    if t.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("bot ")) {
        t = t[4..].trim_start();
    }
    if t.is_empty() || t.eq_ignore_ascii_case("bot") || t.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AuthError::InvalidToken);
    }
    Ok(t.to_string())
}

fn random_account_id() -> String {
    let mut bytes = [0u8; 8];
    rand::rng().fill(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn backup_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".corrupt");
    path.with_file_name(name)
}

impl AccountManager {
    /// A missing file yields an empty manager. An unreadable one is moved aside to `<file>.corrupt`
    /// so the app can start, and the old data stays available for manual recovery.
    pub fn load(path: PathBuf, store: Arc<dyn SecretStore>) -> Result<Self, AuthError> {
        let file = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<AccountsFile>(&text) {
                Ok(f) if f.version <= SCHEMA_VERSION => f,
                _ => {
                    let backup = backup_path(&path);
                    tracing::warn!("accounts file is unreadable; moving it to {}", backup.display());
                    std::fs::rename(&path, &backup)?;
                    AccountsFile::default()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => AccountsFile::default(),
            Err(e) => return Err(e.into()),
        };
        let mut mgr = Self { path, store, file, token_url: None, revoke_url: None };
        mgr.fix_active();
        Ok(mgr)
    }

    /// Overrides the token and revoke endpoints used for refresh and unlink (mock servers in tests).
    pub fn with_oauth_endpoints(mut self, token_url: impl Into<String>, revoke_url: impl Into<String>) -> Self {
        self.token_url = Some(token_url.into());
        self.revoke_url = Some(revoke_url.into());
        self
    }

    pub fn accounts(&self) -> &[AccountRecord] {
        &self.file.accounts
    }

    pub fn account(&self, id: &str) -> Option<&AccountRecord> {
        self.file.accounts.iter().find(|a| a.id == id)
    }

    pub fn active(&self) -> Option<&AccountRecord> {
        self.file.active.as_deref().and_then(|id| self.account(id))
    }

    pub fn set_active(&mut self, id: &str) -> Result<(), AuthError> {
        let now = now_unix();
        self.account_mut(id)?.last_used_at = now;
        self.file.active = Some(id.to_string());
        self.save()
    }

    /// Validates the token against Discord, then stores it. Only bot users are accepted, so a personal
    /// account token pasted by mistake is refused. Re-adding a known bot replaces its token.
    pub async fn add_bot_account(&mut self, token: &str, api_base: &str) -> Result<AccountRecord, AuthError> {
        let token = normalize_bot_token(token)?;
        let auth = format!("Bot {token}");
        let client = http::client()?;

        let user: User = http::get_json(&client, &http::join(api_base, "users/@me"), &auth).await.map_err(bot_validation_error)?;
        if !user.bot {
            return Err(AuthError::NotABot);
        }
        let app: ApplicationInfo =
            http::get_json(&client, &http::join(api_base, "applications/@me"), &auth).await.map_err(bot_validation_error)?;

        let now = now_unix();
        let summary = UserSummary::from(&user);
        let (id, is_new) = match self.file.accounts.iter().find(|a| a.bot_user.id == user.id) {
            Some(existing) => (existing.id.clone(), false),
            None => {
                let mut id = random_account_id();
                while self.account(&id).is_some() {
                    id = random_account_id();
                }
                (id, true)
            }
        };

        self.store.set(&bot_key(&id), &token)?;
        let previous = self.file.clone();
        if is_new {
            self.file.accounts.push(AccountRecord {
                id: id.clone(),
                label: if user.username.is_empty() { "Bot".into() } else { user.username.clone() },
                bot_user: summary,
                application_id: Some(app.id),
                oauth: None,
                created_at: now,
                last_used_at: now,
                credentials_invalid: false,
            });
        } else if let Some(rec) = self.file.accounts.iter_mut().find(|a| a.id == id) {
            rec.bot_user = summary;
            rec.application_id = Some(app.id);
            rec.credentials_invalid = false;
            rec.last_used_at = now;
        }
        self.file.active = Some(id.clone());

        if let Err(e) = self.save() {
            self.file = previous;
            if is_new {
                let _ = self.store.delete(&bot_key(&id));
            }
            return Err(e);
        }
        self.account(&id).cloned().ok_or(AuthError::AccountNotFound(id))
    }

    pub fn bot_token(&self, account_id: &str) -> Result<Option<String>, AuthError> {
        self.store.get(&bot_key(account_id))
    }

    /// Stores the OAuth tokens (and optional client secret) of a completed sign-in for this account.
    pub fn link_oauth(&mut self, account_id: &str, cfg: &OAuthConfig, tokens: &OAuthTokens, user: &User) -> Result<(), AuthError> {
        self.account_mut(account_id)?;
        let secret = cfg.client_secret.as_deref().map(str::trim).filter(|s| !s.is_empty());

        self.store.set(&oauth_key(account_id), &serde_json::to_string(tokens)?)?;
        match secret {
            Some(s) => self.store.set(&oauth_secret_key(account_id), s)?,
            None => self.store.delete(&oauth_secret_key(account_id))?,
        }

        let granted: Vec<String> = tokens.scope.split_whitespace().map(str::to_string).collect();
        self.account_mut(account_id)?.oauth = Some(OAuthLink {
            client_id: cfg.client_id.trim().to_string(),
            user: UserSummary::from(user),
            scopes: if granted.is_empty() { cfg.scopes.clone() } else { granted },
            redirect_port: cfg.redirect_port,
            has_secret: secret.is_some(),
            expired: false,
        });
        self.save()
    }

    /// Returns tokens that are valid for at least the next few minutes, refreshing and persisting them if needed.
    /// A rejected refresh token marks the link expired and yields [`AuthError::SessionExpired`].
    pub async fn oauth_tokens(&mut self, account_id: &str) -> Result<Option<OAuthTokens>, AuthError> {
        let Some(link) = self.account_mut(account_id)?.oauth.clone() else {
            return Ok(None);
        };
        if link.expired {
            return Err(AuthError::SessionExpired);
        }
        let stored = match self.store.get(&oauth_key(account_id))? {
            Some(json) => serde_json::from_str::<OAuthTokens>(&json).ok(),
            None => None,
        };
        let Some(tokens) = stored else {
            self.set_link_expired(account_id)?;
            return Err(AuthError::SessionExpired);
        };
        if !tokens.expires_soon(REFRESH_MARGIN_SECS) {
            return Ok(Some(tokens));
        }
        let Some(refresh_token) = tokens.refresh_token.clone() else {
            self.set_link_expired(account_id)?;
            return Err(AuthError::SessionExpired);
        };

        let cfg = self.link_config(account_id, &link)?;
        match oauth::refresh(&cfg, &refresh_token).await {
            Ok(new_tokens) => {
                self.store.set(&oauth_key(account_id), &serde_json::to_string(&new_tokens)?)?;
                if !new_tokens.scope.is_empty() {
                    let scopes = new_tokens.scope.split_whitespace().map(str::to_string).collect();
                    if let Some(l) = self.account_mut(account_id)?.oauth.as_mut() {
                        l.scopes = scopes;
                    }
                    self.save()?;
                }
                Ok(Some(new_tokens))
            }
            Err(AuthError::SessionExpired) => {
                self.set_link_expired(account_id)?;
                Err(AuthError::SessionExpired)
            }
            // A transient failure must not log the user out while the old access token still works.
            Err(_) if !tokens.expires_soon(0) => Ok(Some(tokens)),
            Err(e) => Err(e),
        }
    }

    /// Revokes the grant at Discord (best effort), then deletes the local OAuth secrets.
    pub async fn unlink_oauth(&mut self, account_id: &str) -> Result<(), AuthError> {
        let link = self.account_mut(account_id)?.oauth.clone();
        if let Some(link) = &link {
            self.revoke_best_effort(account_id, link).await;
        }
        self.store.delete(&oauth_key(account_id))?;
        self.store.delete(&oauth_secret_key(account_id))?;
        if link.is_some() {
            self.account_mut(account_id)?.oauth = None;
            self.save()?;
        }
        Ok(())
    }

    /// Removes the account together with every secret stored for it.
    pub async fn remove_account(&mut self, account_id: &str) -> Result<(), AuthError> {
        self.account_mut(account_id)?;
        self.unlink_oauth(account_id).await?;
        self.store.delete(&bot_key(account_id))?;
        self.file.accounts.retain(|a| a.id != account_id);
        self.fix_active();
        self.save()
    }

    pub fn mark_credentials_invalid(&mut self, account_id: &str, invalid: bool) -> Result<(), AuthError> {
        self.account_mut(account_id)?.credentials_invalid = invalid;
        self.save()
    }

    pub fn update_label(&mut self, account_id: &str, label: &str) -> Result<(), AuthError> {
        let label: String = label.trim().chars().take(MAX_LABEL_CHARS).collect();
        if label.is_empty() {
            return Err(AuthError::InvalidConfig("The account name cannot be empty.".into()));
        }
        self.account_mut(account_id)?.label = label;
        self.save()
    }

    fn account_mut(&mut self, id: &str) -> Result<&mut AccountRecord, AuthError> {
        self.file.accounts.iter_mut().find(|a| a.id == id).ok_or_else(|| AuthError::AccountNotFound(id.to_string()))
    }

    fn set_link_expired(&mut self, account_id: &str) -> Result<(), AuthError> {
        if let Some(link) = self.account_mut(account_id)?.oauth.as_mut() {
            link.expired = true;
        }
        self.save()
    }

    /// Keeps `active` pointing at a real account, preferring the most recently used one.
    fn fix_active(&mut self) {
        let valid = self.file.active.as_deref().is_some_and(|id| self.account(id).is_some());
        if !valid {
            self.file.active = self.file.accounts.iter().max_by_key(|a| a.last_used_at).map(|a| a.id.clone());
        }
    }

    fn link_config(&self, account_id: &str, link: &OAuthLink) -> Result<OAuthConfig, AuthError> {
        let mut cfg = OAuthConfig {
            client_id: link.client_id.clone(),
            redirect_port: link.redirect_port,
            scopes: link.scopes.clone(),
            ..Default::default()
        };
        if link.has_secret {
            cfg.client_secret = self.store.get(&oauth_secret_key(account_id))?;
        }
        if let Some(url) = &self.token_url {
            cfg.token_url = url.clone();
        }
        if let Some(url) = &self.revoke_url {
            cfg.revoke_url = url.clone();
        }
        Ok(cfg)
    }

    async fn revoke_best_effort(&self, account_id: &str, link: &OAuthLink) {
        let tokens = self.store.get(&oauth_key(account_id)).ok().flatten().and_then(|json| serde_json::from_str::<OAuthTokens>(&json).ok());
        let Some(tokens) = tokens else { return };
        let Ok(cfg) = self.link_config(account_id, link) else {
            return;
        };
        let token = tokens.refresh_token.as_deref().unwrap_or(&tokens.access_token);
        if let Err(e) = oauth::revoke(&cfg, token).await {
            tracing::warn!("could not revoke the OAuth grant at Discord: {e}");
        }
    }

    /// Writes via a temporary file and rename so a crash never leaves a half-written file.
    fn save(&self) -> Result<(), AuthError> {
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp_name = self.path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
        tmp_name.push(".tmp");
        let tmp = self.path.with_file_name(tmp_name);

        let json = serde_json::to_vec_pretty(&self.file)?;
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&json)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.path)
        };
        write().map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            AuthError::from(e)
        })
    }
}

#[derive(Deserialize)]
struct ApplicationInfo {
    id: Id,
}

fn bot_validation_error(e: crate::error::ApiError) -> AuthError {
    match e.status {
        401 | 403 => AuthError::InvalidToken,
        _ => e.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_normalization() {
        assert_eq!(normalize_bot_token("  abc.def.ghi \n").unwrap(), "abc.def.ghi");
        assert_eq!(normalize_bot_token("Bot abc.def.ghi").unwrap(), "abc.def.ghi");
        assert_eq!(normalize_bot_token("bot   abc.def").unwrap(), "abc.def");
        assert_eq!(normalize_bot_token(""), Err(AuthError::InvalidToken));
        assert_eq!(normalize_bot_token("Bot "), Err(AuthError::InvalidToken));
        assert_eq!(normalize_bot_token("ñ-é-ü token"), Err(AuthError::InvalidToken));
        assert_eq!(normalize_bot_token("abc def"), Err(AuthError::InvalidToken));
        assert_eq!(normalize_bot_token("abc\r\nX-Evil: 1"), Err(AuthError::InvalidToken));
    }

    #[test]
    fn invite_url_shapes() {
        let url = bot_invite_url(Id(123), RECOMMENDED_BOT_PERMISSIONS, None);
        assert_eq!(url, format!("https://discord.com/oauth2/authorize?client_id=123&scope=bot&permissions={RECOMMENDED_BOT_PERMISSIONS}"));
        let url = bot_invite_url(Id(123), 8, Some(Id(456)));
        assert!(url.ends_with("&permissions=8&guild_id=456&disable_guild_select=true"));
    }

    #[test]
    fn recommended_permissions_exclude_manage_messages() {
        let p = Permissions(RECOMMENDED_BOT_PERMISSIONS);
        assert!(!p.contains(Permissions::MANAGE_MESSAGES));
        assert!(!p.contains(Permissions::ADMINISTRATOR));
        for needed in [
            Permissions::VIEW_CHANNEL,
            Permissions::SEND_MESSAGES,
            Permissions::PIN_MESSAGES,
            Permissions::CONNECT,
            Permissions::SPEAK,
            Permissions::USE_VAD,
            Permissions::READ_MESSAGE_HISTORY,
        ] {
            assert!(p.contains(needed));
        }
        assert_ne!(RECOMMENDED_BOT_PERMISSIONS & (1 << 51), 0);
    }

    #[test]
    fn account_ids_are_random_hex() {
        let a = random_account_id();
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(a, random_account_id());
    }
}
