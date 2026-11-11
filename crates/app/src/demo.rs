//! `--demo`: offline sample data for UI previews, screenshots and manual UI testing without a
//! Discord connection. The data flows through the same `Store::apply` path as real gateway
//! payloads; the UI shows a banner so it can't be mistaken for a live session.

use chrono::{TimeZone, Utc};
use serde_json::json;

use dn_cache::bus::{BackendLink, Command, SearchSource, UiEvent};
use dn_cache::{ConnStatus, SharedStore};
use dn_core::{Dispatch, Id, Message, MessageDelete, MessageUpdate, ReactionEvent, User};

const ME: u64 = 900_000_000_000_000_001;
const GUILD_A: u64 = 800_000_000_000_000_001;
const GUILD_B: u64 = 800_000_000_000_000_002;
const GENERAL: u64 = 810_000_000_000_000_001;
const DM: u64 = 820_000_000_000_000_001;

const PEOPLE: [(u64, &str, &str); 6] = [
    (900_000_000_000_000_010, "alice", "Alice"),
    (900_000_000_000_000_011, "bob", "Bob"),
    (900_000_000_000_000_012, "carol", "Carol"),
    (900_000_000_000_000_013, "dave", "Dave"),
    (900_000_000_000_000_014, "erin", "Erin"),
    (900_000_000_000_000_015, "frank", "Frank"),
];

const LINES: [&str; 14] = [
    "Morning! Did anyone try the new build?",
    "Yes — startup is **way** faster than before 🚀",
    "Memory sits around 80 MB here, idle CPU is basically zero",
    "Here's the snippet I used:\n```rust\nfn main() {\n    println!(\"hello from discord-native\");\n}\n```",
    "> virtualized lists are the secret sauce\nagreed, scrolling 10k messages is smooth",
    "Check the docs: https://discord.com/developers/docs/intro",
    "||spoiler: the voice test is next||",
    "Can someone review my PR? <#810000000000000001>",
    "- [x] markdown\n- [x] reactions\n- [ ] world domination",
    "`cargo test` passes on my machine ✅",
    "ok I'm heading out, see you tomorrow 👋",
    "_italics_, __underline__, ~~strike~~ and ***bold italic***",
    "Meeting at <t:1893456000:t> works for me",
    "lol",
];

fn user_json(id: u64, username: &str, global: &str) -> serde_json::Value {
    json!({ "id": id.to_string(), "username": username, "global_name": global, "discriminator": "0", "avatar": null })
}

fn ts(ms: i64) -> String {
    Utc.timestamp_millis_opt(ms).single().unwrap_or_default().to_rfc3339()
}

fn d(event: &str, v: serde_json::Value) -> Dispatch {
    Dispatch::parse(event, &v.to_string()).expect("demo payload must parse")
}

pub fn populate(store: &SharedStore) {
    let now = Utc::now().timestamp_millis();
    let mut s = store.lock();
    s.session.demo = true;
    s.session.needs_onboarding = false;
    s.session.conn = ConnStatus::Connected;
    s.session.latency_ms = Some(42);
    s.apply(&d(
        "READY",
        json!({ "v": 10, "user": { "id": ME.to_string(), "username": "native-bot", "global_name": "Native Bot", "bot": true, "discriminator": "0" },
                "guilds": [{ "id": GUILD_A.to_string(), "unavailable": true }, { "id": GUILD_B.to_string(), "unavailable": true }],
                "session_id": "demo", "resume_gateway_url": "wss://demo.invalid", "application": { "id": "900000000000000002", "flags": 0 } }),
    ));
    let members: Vec<serde_json::Value> =
        std::iter::once(json!({ "user": { "id": ME.to_string(), "username": "native-bot", "bot": true }, "roles": [] }))
            .chain(PEOPLE.iter().enumerate().map(
                |(i, (id, u, g))| json!({ "user": user_json(*id, u, g), "roles": if i < 2 { vec!["830000000000000002"] } else { vec![] } }),
            ))
            .collect();
    let presences: Vec<serde_json::Value> = PEOPLE
        .iter()
        .enumerate()
        .map(|(i, (id, _, _))| {
            let status = ["online", "online", "idle", "dnd", "online", "offline"][i];
            let activities = if i == 0 { json!([{ "name": "Rust", "type": 0 }]) } else { json!([]) };
            json!({ "user": { "id": id.to_string() }, "status": status, "activities": activities })
        })
        .collect();
    s.apply(&d(
        "GUILD_CREATE",
        json!({
            "id": GUILD_A.to_string(), "name": "Rustaceans Lounge", "owner_id": PEOPLE[0].0.to_string(), "member_count": 7,
            "roles": [
                { "id": GUILD_A.to_string(), "name": "@everyone", "permissions": "4503599627370495", "position": 0 },
                { "id": "830000000000000002", "name": "Maintainers", "permissions": "0", "position": 1, "hoist": true, "color": 0x5865F2 }
            ],
            "channels": [
                { "id": "810000000000000100", "type": 4, "name": "Text Channels", "position": 0 },
                { "id": GENERAL.to_string(), "type": 0, "name": "general", "position": 0, "parent_id": "810000000000000100", "topic": "Chat about anything", "last_message_id": "0" },
                { "id": "810000000000000002", "type": 0, "name": "showcase", "position": 1, "parent_id": "810000000000000100" },
                { "id": "810000000000000003", "type": 5, "name": "announcements", "position": 2, "parent_id": "810000000000000100" },
                { "id": "810000000000000200", "type": 4, "name": "Voice Channels", "position": 1 },
                { "id": "810000000000000004", "type": 2, "name": "Lounge", "position": 0, "parent_id": "810000000000000200" },
                { "id": "810000000000000005", "type": 2, "name": "Pair programming", "position": 1, "parent_id": "810000000000000200" }
            ],
            "threads": [{ "id": "810000000000000006", "type": 11, "name": "release-planning", "parent_id": GENERAL.to_string(), "thread_metadata": { "archived": false }, "message_count": 12 }],
            "members": members,
            "presences": presences,
            "voice_states": [
                { "user_id": PEOPLE[1].0.to_string(), "channel_id": "810000000000000004", "session_id": "x" },
                { "user_id": PEOPLE[3].0.to_string(), "channel_id": "810000000000000004", "session_id": "y", "self_mute": true }
            ],
            "emojis": []
        }),
    ));
    s.apply(&d(
        "GUILD_CREATE",
        json!({
            "id": GUILD_B.to_string(), "name": "Game Night", "owner_id": ME.to_string(),
            "roles": [{ "id": GUILD_B.to_string(), "name": "@everyone", "permissions": "4503599627370495", "position": 0 }],
            "channels": [{ "id": "811000000000000001", "type": 0, "name": "lobby", "position": 0, "last_message_id": "811000000000000099" }],
            "members": [{ "user": { "id": ME.to_string(), "username": "native-bot", "bot": true }, "roles": [] }]
        }),
    ));

    // History for #general: ~300 messages over the last two days.
    let mut msgs: Vec<Message> = Vec::new();
    let count = 300i64;
    for i in 0..count {
        let at = now - (count - i) * 9 * 60_000 + (i % 7) * 1_000;
        let (uid, uname, gname) = PEOPLE[(i as usize * 7 + i as usize / 3) % PEOPLE.len()];
        let id = Id::from_timestamp_ms(at as u64).0 + i as u64;
        let mut m = json!({
            "id": id.to_string(), "channel_id": GENERAL.to_string(), "guild_id": GUILD_A.to_string(),
            "author": user_json(uid, uname, gname), "content": LINES[i as usize % LINES.len()], "timestamp": ts(at), "type": 0
        });
        if i % 23 == 5 {
            m["reactions"] = json!([{ "count": 3, "me": false, "emoji": { "id": null, "name": "🔥" } }, { "count": 1, "me": true, "emoji": { "id": null, "name": "👍" } }]);
        }
        if i % 41 == 9 {
            m["embeds"] = json!([{ "type": "rich", "title": "Release notes", "description": "**v0.1.0** — native UI, gateway v10, SQLite cache.\nSee `CHANGELOG.md`.", "color": 0x5B6EF5,
                "fields": [{ "name": "Startup", "value": "< 1 s", "inline": true }, { "name": "Idle RAM", "value": "~80 MB", "inline": true }],
                "footer": { "text": "Discord Native" } }]);
        }
        if i % 37 == 11 {
            m["attachments"] = json!([{ "id": (id + 1).to_string(), "filename": "trace.log", "size": 48213, "url": "https://example.invalid/trace.log", "proxy_url": "", "content_type": "text/plain" }]);
        }
        if i == count - 3 {
            m["content"] = json!(format!("<@{ME}> can you post the benchmark numbers?"));
            m["mentions"] = json!([{ "id": ME.to_string(), "username": "native-bot" }]);
        }
        if i == count - 2 {
            let prev = msgs.last().cloned();
            if let Some(prev) = prev {
                m["type"] = json!(19);
                m["message_reference"] = json!({ "message_id": prev.id.to_string(), "channel_id": GENERAL.to_string() });
                m["referenced_message"] = serde_json::to_value(&prev).unwrap_or_default();
                m["content"] = json!("Sure, give me a minute");
            }
        }
        msgs.push(serde_json::from_value(m).expect("demo message"));
    }
    let last = msgs.last().map(|m| m.id);
    if let Some(c) = s.channels.get_mut(&Id(GENERAL)) {
        c.last_message_id = last;
    }
    // Everything except the last few messages counts as read, so the NEW marker shows.
    if let Some(read) = msgs.get(msgs.len() - 4).map(|m| m.id) {
        s.read_states.insert(Id(GENERAL), read);
    }
    *s.mention_counts.entry(Id(GENERAL)).or_default() = 1;
    s.set_active_channel(Some(Id(GENERAL)));
    s.messages_loaded(Id(GENERAL), msgs, None, false);
    for c in [810_000_000_000_000_002u64, 810_000_000_000_000_003, 810_000_000_000_000_006, 811_000_000_000_000_001] {
        let author = PEOPLE[2];
        let at = now - 3_600_000;
        let m: Message = serde_json::from_value(json!({
            "id": Id::from_timestamp_ms(at as u64).0.to_string(), "channel_id": c.to_string(),
            "author": user_json(author.0, author.1, author.2), "content": "Nothing much here yet — say hi!", "timestamp": ts(at)
        }))
        .expect("demo message");
        s.messages_loaded(Id(c), vec![m], None, true);
    }

    // A DM with the bot.
    s.apply(&d(
        "CHANNEL_CREATE",
        json!({ "id": DM.to_string(), "type": 1, "recipients": [user_json(PEOPLE[4].0, PEOPLE[4].1, PEOPLE[4].2)] }),
    ));
    let at = now - 600_000;
    let dm: Message = serde_json::from_value(json!({
        "id": Id::from_timestamp_ms(at as u64).0.to_string(), "channel_id": DM.to_string(),
        "author": user_json(PEOPLE[4].0, PEOPLE[4].1, PEOPLE[4].2), "content": "hey bot, are you there? 👀", "timestamp": ts(at)
    }))
    .expect("demo dm");
    s.messages_loaded(Id(DM), vec![dm], None, true);
    s.session.audio_inputs = vec!["Demo Microphone".into()];
    s.session.audio_outputs = vec!["Demo Speakers".into()];
    s.session.ready = true;
}

/// Minimal local command handling so the demo is interactive.
pub async fn serve(link: BackendLink, mut commands: tokio::sync::mpsc::UnboundedReceiver<Command>) {
    while let Some(cmd) = commands.recv().await {
        let me =
            User { id: Id(ME), username: "native-bot".into(), global_name: Some("Native Bot".into()), bot: true, ..Default::default() };
        match cmd {
            Command::Shutdown => break,
            Command::SendMessage { channel_id, content, reply_to, .. } => {
                let now = Utc::now().timestamp_millis();
                let mut store = link.store.lock();
                let referenced = reply_to.and_then(|r| store.channel_messages(channel_id).and_then(|cm| cm.get(r).cloned()));
                let nonce = store.channel_messages(channel_id).and_then(|cm| cm.pending.last().map(|p| p.nonce.clone()));
                let m = Message {
                    id: Id(Id::from_timestamp_ms(now as u64).0 + 1),
                    channel_id,
                    guild_id: store.guild_of(channel_id),
                    author: me,
                    content,
                    timestamp: ts(now),
                    kind: if referenced.is_some() { 19 } else { 0 },
                    referenced_message: referenced.map(Box::new),
                    nonce: nonce.map(serde_json::Value::String),
                    ..Default::default()
                };
                store.apply(&Dispatch::MessageCreate(Box::new(m)));
            }
            Command::EditMessage { channel_id, message_id, content } => {
                let u = MessageUpdate {
                    id: message_id,
                    channel_id,
                    content: Some(content),
                    edited_timestamp: Some(ts(Utc::now().timestamp_millis())),
                    ..Default::default()
                };
                link.store.lock().apply(&Dispatch::MessageUpdate(Box::new(u)));
            }
            Command::DeleteMessage { channel_id, message_id } => {
                link.store.lock().apply(&Dispatch::MessageDelete(MessageDelete { id: message_id, channel_id, guild_id: None }));
            }
            Command::React { channel_id, message_id, emoji, add } => {
                let ev = Box::new(ReactionEvent { user_id: Id(ME), channel_id, message_id, emoji, ..Default::default() });
                link.store.lock().apply(&if add { Dispatch::MessageReactionAdd(ev) } else { Dispatch::MessageReactionRemove(ev) });
            }
            Command::MarkRead { channel_id, message_id } => link.store.lock().mark_read(channel_id, message_id),
            Command::LoadOlder { channel_id, .. } => {
                let mut s = link.store.lock();
                s.set_loading_older(channel_id, false);
                s.channel_messages_mut(channel_id).reached_start = true;
            }
            Command::LoadPins { channel_id } => link.emit(UiEvent::Pins { channel_id, messages: vec![] }),
            Command::Search(req) => {
                let q = req.query.to_lowercase();
                let hits: Vec<Message> = {
                    let s = link.store.lock();
                    s.messages
                        .values()
                        .flat_map(|cm| cm.messages.iter())
                        .filter(|m| m.content.to_lowercase().contains(&q))
                        .take(25)
                        .cloned()
                        .collect()
                };
                link.emit(UiEvent::SearchResults {
                    request: Box::new(req),
                    total: hits.len() as u64,
                    messages: hits,
                    source: SearchSource::Local,
                    note: Some("Demo data".into()),
                });
            }
            Command::OpenChannel { .. }
            | Command::Typing { .. }
            | Command::FocusChanged { .. }
            | Command::SettingsChanged
            | Command::RequestMembers { .. }
            | Command::FetchProfile { .. } => {}
            other => link.toast(format!("Not available in demo mode ({})", command_name(&other)), false),
        }
        link.repaint.now();
    }
}

fn command_name(c: &Command) -> String {
    let s = format!("{c:?}");
    s.split([' ', '(', '{']).next().unwrap_or("command").to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn demo_data_populates_store() {
        let store = dn_cache::Store::shared(Default::default());
        super::populate(&store);
        let s = store.lock();
        assert_eq!(s.guilds.len(), 2);
        assert!(s.channel_messages(dn_core::Id(super::GENERAL)).unwrap().messages.len() >= 300);
        assert!(s.is_unread(dn_core::Id(super::GENERAL)));
        assert_eq!(s.voice_members(dn_core::Id(super::GUILD_A), dn_core::Id(810_000_000_000_000_004)).len(), 2);
        assert!(s.session.demo);
    }
}
