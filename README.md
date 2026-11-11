# Discord Native

A native desktop client for Discord, written in Rust with an egui/wgpu UI instead of Electron. It targets Windows 10/11 first and also builds on Linux.

> **Read this first: what it can and cannot do.**
> Discord does not let third-party apps chat as your personal account. No OAuth2 scope grants it, and using a user token ("self-bot") breaks Discord's Terms of Service. Discord Native only uses authorized interfaces, so **it connects as a bot that you own**:
> - You chat as that bot in the servers you add it to, and in DMs people send to it.
> - Optionally, "Sign in with Discord" (OAuth2) shows your real profile and server list.
> - Your personal DMs, friends list, group DMs and video/screen sharing are not available.
>
> A complete replacement for the official client is therefore not possible under Discord's rules. The full matrix is in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md).

## Features

- Servers, categories, text/announcement/voice/forum channels and threads, with permission-aware visibility.
- Real-time messaging over gateway v10, with history paging, edits, deletes, replies, mentions, reactions, custom emoji, pins, typing indicators and file uploads/downloads.
- Discord-flavoured Markdown (bold/italic/underline/strike, spoilers, code blocks, quotes, headings, lists, masked links, timestamps, mentions), embeds, stickers, polls (view only) and image previews.
- Search through Discord's guild search API (needs the Message Content intent), with a local full-text index as fallback.
- Unread markers and mention badges. Bots have no server-side read state, so it is stored locally.
- Desktop notifications (Windows toasts; clicking one opens the channel), tray icon, close-to-tray, start on boot, single instance.
- Voice channels through [songbird](https://github.com/serenity-rs/songbird), with DAVE end-to-end encryption:
  - device selection and hot switching, voice activity detection with auto-sensitivity, push-to-talk (global on Windows);
  - RNNoise noise suppression, automatic gain, per-user volume, speaking indicators.
  - **Status: implemented but not yet tested against official clients.**
- Multiple accounts, with tokens kept only in the Windows Credential Manager (or the OS keychain).
- Large histories stay fast: virtualized message list, bounded in-memory windows, GPU texture LRU, and image decoding off the UI thread.
- Themes (dark, light, midnight), zoom, compact options and keyboard shortcuts.

## Getting started

### 1. Install

Pick one of these:

- **Installer:** download `DiscordNative-Setup-<version>.exe` from the latest CI run's artifacts (Actions → CI → *discord-native-windows-x64*). It is a per-user install and needs no admin rights.
- **Portable:** use `discord-native-<version>-windows-x64.exe` from the same artifact. Set `DN_HOME=<folder>` to keep all data next to it.
- **From source:** see [Building](#building).

### 2. Create your bot (once)

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and click **New Application**.
2. On the **Bot** tab, click **Reset Token** and copy the token.
3. On the same tab, under **Privileged Gateway Intents**, enable **Message Content**, **Server Members** and **Presence**. These are needed to read message text, list members and show online status.
4. Start Discord Native and paste the token. The app checks that it is a bot token and stores it in the credential manager.
5. Click **+** in the server list to invite the bot to your servers. This needs *Manage Server* permission in each server.

### 3. Optional: Sign in with Discord

This shows your real profile and lists your servers so you can invite the bot more easily.

1. In the portal, open **OAuth2** and add the redirect `http://127.0.0.1:53134/callback`.
2. Enable **Public Client**, or enter the client secret in the app.
3. In the app, go to Settings → My Account → *Sign in with Discord*.

## Keyboard shortcuts

| Action | Shortcut |
|---|---|
| Quick switcher | Ctrl+K |
| Search | Ctrl+F |
| Previous / next channel | Alt+↑ / Alt+↓ |
| Toggle mute / deafen | Ctrl+Shift+M / Ctrl+Shift+D |
| Edit last message | ↑ in an empty composer |
| Cancel reply/edit, mark channel read | Esc |
| New line | Shift+Enter |

## Command line

```
discord-native [--minimized] [--demo] [--self-check [--network]] [--software-render] [--version]
```

- `--demo` shows offline sample data with a visible banner. It never connects to Discord and is useful for previewing the UI.
- `--self-check` tests the local database, credential store, settings and audio devices. Add `--network` to also check that Discord's API and gateway are reachable.

## Data locations (Windows)

| What | Where |
|---|---|
| Settings, accounts list (no secrets) | `%APPDATA%\DiscordNative\config` |
| Message cache (SQLite), crash reports | `%LOCALAPPDATA%\DiscordNative\data` |
| Image cache | `%LOCALAPPDATA%\DiscordNative\cache` |
| Logs | `%LOCALAPPDATA%\DiscordNative\data\logs` |
| Bot / OAuth tokens | Windows Credential Manager, entries named `DiscordNative` |

## Building

Requires Rust stable (1.88+). On Windows install the MSVC toolchain; on Linux install the packages listed in [.github/workflows/ci.yml](.github/workflows/ci.yml).

```bash
cargo run -p discord-native --release          # run
cargo test --workspace                         # tests
cargo build --release -p discord-native        # native release build (fat LTO)
scripts/package-windows.sh                     # Linux -> Windows cross build + NSIS installer (needs mingw-w64, nsis)
```

Benchmarks live in the crates: `cargo bench -p dn-storage`, `cargo bench -p dn-messaging` and `cargo bench -p dn-cache`. The UI has a built-in stress test under Settings → Diagnostics.

## Project layout

See [HANDOFF.md](HANDOFF.md) for the full architecture, status and roadmap (in Russian). In brief:

`dn-core` (Discord models) · `dn-api` (REST + rate limits) · `dn-gateway` (WebSocket gateway) · `dn-auth` (OAuth2 PKCE, accounts, keyring) · `dn-storage` (SQLite + FTS5) · `dn-cache` (client state, UI bus) · `dn-messaging` (Markdown, emoji) · `dn-media` (images) · `dn-audio` / `dn-voice` (CPAL, songbird) · `dn-notifications` · `dn-settings` · `dn-diagnostics` · `dn-client` (backend) · `dn-ui` (egui) · `discord-native` (app).

## License

MIT. "Discord" is a trademark of Discord Inc.; this project is not affiliated with or endorsed by Discord.
