# Test plan

Automated tests run with `cargo test --workspace` and in CI on Linux and Windows. The manual checks below need a real Windows 10/11 machine, a bot application you own, and a second Discord account in the official client. Record every manual run in the table at the end. **Never mark an item passed without running it.**

## Setup for manual tests

- **Bot "DN Test".** Create it in the Developer Portal with all three privileged intents enabled. Invite it to a test server with *Administrator*, or at least the permissions in the invite link the app generates.
- **Second account.** An official Discord client (desktop or mobile) logged into a second, normal account that is a member of the same server.
- **Network shaping for disruption tests.** Use Windows' `clumsy` tool or unplug the network.
- **Resource measurement.** Task Manager → Details: *Memory (private working set)* and *CPU*. Settings → Diagnostics shows the app's own view.

## Areas

| # | Area | Automated coverage | Manual procedure | Expected |
|---|---|---|---|---|
| 1 | Startup / shutdown | `discord-native --self-check` (CI on Windows); session marker unit tests | Cold start after reboot (stopwatch from double-click to visible channel list); close via X (close to tray) and via tray → Quit; start a 2nd instance | Window < 2 s on SSD; tray icon present; 2nd launch focuses the 1st; next start shows no "didn't shut down cleanly" toast |
| 2 | Authentication / session restore | dn-auth (60 tests: PKCE, loopback, refresh, keyring abstraction); dn-client e2e (add bot, user token refused, 4004) | Add bot token; restart app; switch between two bots; remove account; check Credential Manager entries; do OAuth sign-in; revoke the app in Discord user settings and restart | Session restored without re-entering the token; removed account leaves no `DiscordNative` credentials; revoked OAuth shows "sign-in expired" |
| 3 | Server / channel navigation | dn-cache store tests (tree, permissions, threads) | Browse servers, categories, hidden channels, threads, forum; Ctrl+K; Alt+↑/↓ | Hidden channels absent; selection remembered per server |
| 3a | Forum posts and stickers | dn-api (forum post JSON/multipart/validation), dn-core (forum tags), dn-client e2e (post created, tag-required error, unreadable file) | In a forum channel with "Require tag" on: create a post with title, text, one tag and an image; try without a tag; send a server sticker from the 🎫 picker | Post appears for the second account with the tag and image; untagged post shows Discord's error inline; sticker shows in the official client |
| 4 | Messaging and history | dn-api (32), dn-gateway (16), dn-client e2e (6: history, send, live receive, persistence, edits outside open channels, forum posts) | Send / edit / delete / reply / react / pin / upload (image and 9 MB file) / download; scroll to channel start; jump from search and pins | The second account sees every change in the official client within ~1 s and vice versa |
| 5 | Concurrent incoming messages | dn-cache bench (10k live messages) | Second account spams 5 msg/s for 2 min while you scroll history | No stutter; live messages appear; no duplicates |
| 6 | Notifications | dn-notifications (33: rules, rate limit) | Mention the bot from the 2nd account with the window unfocused / minimized to tray / focused on that channel; mute the channel; DND | Toast only when expected; clicking the toast opens the channel; ≤ 5 toasts per 10 s |
| 7 | Voice device detection | dn-audio DSP + headless engine tests | Settings → Voice: lists devices; unplug and replug a USB headset during a call | Device list refreshes; capture falls back to default and recovers without restarting |
| 8 | Join / leave voice | dn-voice unit tests (adapter, mixer, speaking tracker) | Join a voice channel, leave, join another, get moved by a moderator, get disconnected by a moderator | Status panel matches; others see the bot join/leave in the official client |
| 9 | **Two-way audio with official clients** | **none possible offline** | Talk both ways for 5 min between the app (as bot) and the official client; check speaking indicators on both sides; mute/deafen; PTT with app unfocused; per-user volume | Clear audio both ways; indicators correct; **this is the gate for claiming voice interoperability** |
| 10 | Audio under network disruption | — | clumsy: 5 % loss + 80 ms jitter, then cut the network for 10 s | Audio degrades gracefully; reconnects; no crash |
| 11 | Screen sharing / video | — | Not supported by Discord for bots | Others' Go Live shows only as a LIVE badge |
| 12 | High message volumes | dn-storage bench (50k), dn-messaging robustness (200 KB inputs), UI stress test | Settings → Diagnostics → run the stress test with 100k messages; scroll back 2,000+ messages in a busy channel | Stress test p95 frame < 16.7 ms on a modern GPU; memory stays bounded |
| 13 | Memory over extended sessions | — | Leave running 8 h in 5+ active servers; sample memory every hour | No monotonic growth beyond caches (image cache budget + message windows) |
| 14 | CPU idle / active | — | Idle 10 min minimized and focused; typing; scrolling | Idle ≈ 0 % (no frames are drawn while idle) |
| 15 | Crashes and recovery | panic hook + crash report tests; corrupt DB/settings/accounts recovery tests | Kill the process during a send; corrupt `settings.json`; delete the network adapter | Restart works; corrupt files are backed up and replaced; crash report written |
| 16 | Windows integration | — | Installer (per-user), Start menu shortcut, uninstall; start on boot; tray; toasts appear under "Discord Native"; high-DPI 150 %/200 %; dark/light themes | All behave; uninstall removes the Run key and shortcuts |

## Comparison with the official client (performance)

Use the same machine and the same account membership (the official client logged into the second account in the same servers). Measure the following:

- cold start time;
- private working set after 5 minutes idle;
- CPU % idle over 60 s;
- CPU % while scrolling a 2,000-message channel.

Record the results in [BENCHMARKS.md](BENCHMARKS.md).

## Manual run log

| Date | Build | Machine | Items run | Result | Notes |
|---|---|---|---|---|---|
| — | — | — | none yet | — | — |
