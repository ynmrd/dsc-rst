# Feature compatibility matrix

Research date: 2026-11-11. Sources are the official Discord developer docs (`discord/discord-api-docs` on GitHub, docs.discord.com), Discord's support and developer-policy articles, and crates.io. Re-check them before relying on this table, because Discord changes its policies and APIs.

## Summary

A full replacement for the official Discord client **cannot be built** with Discord's authorized interfaces:

- No OAuth2 scope lets a third-party application read or send messages as the user, or read the user's DMs. The only user-context messaging is the Discord Social SDK, which is game-only and limited to in-app DMs and lobbies.
- "Self-bots" are forbidden. Discord's wording: "Automating normal user accounts (generally called "self-bots") outside of the OAuth2/bot API is forbidden, and can result in an account termination if found." ([support article 115002192352](https://support.discord.com/hc/en-us/articles/115002192352)). Using a user token in a third-party client counts as a self-bot.
- The Developer Policy (#4) forbids collecting user credentials or tokens. It also forbids embedding developer credentials in open-source projects, so each user must bring their own application.

Discord Native therefore uses only the following:

1. **Bot account (primary).** The user creates their own application in the Developer Portal and pastes the bot token. The client then works as that bot over the full Bot API and gateway v10: the servers it was added to, and DMs that people send to the bot.
2. **OAuth2 Authorization Code + PKCE (optional).** "Sign in with Discord" in the system browser over a loopback redirect. It shows the user's real profile and server list (`identify`, `guilds`, `guilds.members.read`) and helps invite the bot into the user's servers.

## Matrix

| Capability | Status | Mechanism / reason |
|---|---|---|
| Sign in with an existing Discord account | Supported (identity only) | OAuth2 + PKCE (S256, `none` auth for public clients), loopback `http://127.0.0.1:53134/callback` |
| Chat as your personal account | **Blocked** | No scope exists for it; user tokens are against the ToS |
| Sessions, restore after restart, account switching, logout, credential cleanup | Supported | Bot token and OAuth tokens in the OS credential manager (Windows Credential Manager) |
| Profile, avatar, name, status | Supported | REST `/users/@me`; status set through gateway op 3 (the bot's status) |
| Servers, channels, categories, permissions | Supported as bot | GUILD_CREATE plus permission calculation |
| Send, receive, edit, delete; replies, mentions | Supported as bot | REST v10 plus gateway; MESSAGE_CONTENT intent (privileged) |
| Reactions, custom emoji | Supported as bot | Emoji from servers the bot is in |
| Stickers | Partial | PNG/APNG/GIF are shown; Lottie is not; the sticker picker sends the current server's stickers (Discord only accepts same-server stickers in `sticker_ids`) |
| GIF picker | **Blocked** | The Tenor API shut down in 2026 and Discord's own picker uses private endpoints; GIF links and embeds are displayed |
| File upload/download, image previews, embeds | Supported as bot | multipart `files[n]`, media-proxy resize |
| History, incremental loading | Supported as bot | `GET /channels/{id}/messages` with before/after/around, plus local SQLite cache |
| Search | Supported as bot | `GET /guilds/{id}/messages/search` (2026, needs MESSAGE_CONTENT) plus local FTS5 |
| Pinned messages | Supported as bot | `GET /channels/{id}/messages/pins`, `PUT/DELETE .../pins/{id}` |
| Unread markers, mentions | Partial | Bots have no server-side read state, so it is kept locally |
| Typing indicators | Supported as bot | GUILD_MESSAGE_TYPING / DIRECT_MESSAGE_TYPING |
| Notifications | Supported | Windows toasts (AppUserModelID registered in HKCU) |
| Threads, forum posts | Supported as bot | Viewing, writing, creating threads from a message; new forum/media posts with title, tags (`REQUIRE_TAG` and moderated tags respected) and attachments via `POST /channels/{id}/threads` |
| DMs | Partial | Only the bot's DMs; the user's personal DMs are unavailable |
| Group DMs, friends, friend requests | **Blocked** | Not available to bots or OAuth applications |
| Others' presence | Supported as bot | GUILD_PRESENCES (privileged) |
| Rich Presence / game activity | **Blocked** | `rpc*` scopes are partner-only |
| Voice: join/leave, two-way audio | Implemented, **untested** | songbird 0.6.1 (voice gateway v4, DAVE E2EE mandatory since 2026-03-01, `aead_aes256_gcm_rtpsize`). Bots cannot join DM calls |
| Device selection, hot switching, VAD, PTT, noise suppression, AGC, per-user volume | Implemented locally | CPAL, nnnoiseless; no echo cancellation |
| Video, webcam, screen sharing (Go Live) | **Blocked** | No API for bots or third-party clients |
| Server management, events, polls (voting) | Partly unavailable | Polls are view-only; management is not implemented |
| Tray, start on boot, shortcuts | Supported (Windows) | tray-icon, HKCU\…\Run |

## Rust stack (versions as of 2026-11)

- UI: eframe/egui 0.37 (wgpu 29/30, glow fallback). egui 0.37 changed its API: `App::ui(&mut Ui)` plus `App::logic()`, which is also called while the window is hidden, and the unified `egui::Panel::left/right/top/bottom`.
- Network: reqwest 0.12 (rustls), tokio-tungstenite 0.26, flate2 (zlib-stream).
- Voice: songbird 0.6.1 (driver, receive, DAVE via `davey` 0.1.x), opus2 with feature `bundled`.
- Storage: rusqlite 0.42 (bundled SQLite, FTS5), keyring 3.6.
- Audio: cpal 0.18, nnnoiseless 0.5.
