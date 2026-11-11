use super::*;

fn d(event: &str, json: serde_json::Value) -> Dispatch {
    Dispatch::parse(event, &json.to_string()).unwrap()
}

const ME: u64 = 1000;
const G: u64 = 1;

/// Guild with: @everyone can view; channel 12 hides from @everyone but role 50 allows;
/// category 20 with text 21 + voice 22; uncategorized text 10, 11 (positions 1,0).
fn setup() -> Store {
    let mut s = Store::new(StoreLimits { inactive_channel_messages: 5, active_channel_messages: 10, max_loaded_channels: 3 });
    s.apply(&d(
        "READY",
        serde_json::json!({"v":10,"user":{"id":ME.to_string(),"username":"bot","bot":true},"guilds":[{"id":"1","unavailable":true}],
          "session_id":"s","resume_gateway_url":"wss://x","application":{"id":"77","flags":0}}),
    ));
    s.apply(&d(
        "GUILD_CREATE",
        serde_json::json!({
            "id":"1","name":"Test Guild","owner_id":"999",
            "roles":[
                {"id":"1","name":"@everyone","permissions":"68608","position":0},
                {"id":"50","name":"Mods","permissions":"0","position":2,"hoist":true,"color":16711680},
                {"id":"51","name":"Colored","permissions":"0","position":1,"color":255}
            ],
            "channels":[
                {"id":"10","type":0,"name":"general","position":1,"last_message_id":"500"},
                {"id":"11","type":0,"name":"rules","position":0},
                {"id":"12","type":0,"name":"secret","position":2,"permission_overwrites":[{"id":"1","type":0,"allow":"0","deny":"1024"}]},
                {"id":"20","type":4,"name":"Cat","position":0},
                {"id":"22","type":2,"name":"Voice","position":0,"parent_id":"20"},
                {"id":"21","type":0,"name":"in-cat","position":5,"parent_id":"20"}
            ],
            "threads":[{"id":"30","type":11,"name":"thread","parent_id":"10","thread_metadata":{"archived":false}}],
            "members":[
                {"user":{"id":ME.to_string(),"username":"bot","bot":true},"roles":[]},
                {"user":{"id":"2","username":"alice"},"roles":["50"],"nick":"Ali"},
                {"user":{"id":"3","username":"bob"},"roles":["51"]}
            ],
            "presences":[{"user":{"id":"2"},"status":"online","activities":[{"name":"Rust","type":0}]},{"user":{"id":"3"},"status":"idle"}],
            "voice_states":[{"user_id":"2","channel_id":"22","session_id":"x"}]
        }),
    ));
    s
}

fn msg(id: u64, channel: u64, author: u64, content: &str) -> Message {
    Message {
        id: Id(id),
        channel_id: Id(channel),
        guild_id: Some(Id(G)),
        author: User { id: Id(author), username: format!("u{author}"), ..Default::default() },
        content: content.into(),
        ..Default::default()
    }
}

#[test]
fn guild_and_channel_tree() {
    let s = setup();
    assert_eq!(s.guild_order, vec![Id(1)]);
    let tree = s.channel_tree(Id(G));
    let names: Vec<Vec<String>> = tree.iter().map(|g| g.channels.iter().map(|c| c.display_name()).collect()).collect();
    // `secret` is hidden; uncategorized first sorted by position; text before voice in category.
    assert_eq!(names, vec![vec!["rules".to_string(), "general".into()], vec!["in-cat".into(), "Voice".into()]]);
    assert_eq!(tree[1].category.unwrap().id, Id(20));
    assert!(s.permissions_in(Id(10)).can_send());
    assert!(!s.permissions_in(Id(12)).can_view());
    // Threads inherit from parent; thread send requires SEND_MESSAGES_IN_THREADS which @everyone lacks here.
    assert!(s.permissions_in(Id(30)).can_view());
    assert!(!s.permissions_in(Id(30)).contains(Permissions::SEND_MESSAGES));
    assert_eq!(s.threads_of(Id(10)).len(), 1);
}

#[test]
fn read_states_initialized_on_first_sight() {
    let mut s = setup();
    assert!(!s.is_unread(Id(10)));
    assert_eq!(s.take_dirty_read_states(), vec![(Id(10), Id(500))]);
    s.apply(&Dispatch::MessageCreate(Box::new(msg(600, 10, 2, "hello"))));
    assert!(s.is_unread(Id(10)));
    assert_eq!(s.guild_unread(Id(G)), (true, 0));
    s.mark_read(Id(10), Id(600));
    assert!(!s.is_unread(Id(10)));
}

#[test]
fn mentions_counted_and_own_messages_mark_read() {
    let mut s = setup();
    let mut m = msg(600, 10, 2, "hey <@1000>");
    m.mentions = vec![User { id: Id(ME), ..Default::default() }];
    s.apply(&Dispatch::MessageCreate(Box::new(m)));
    assert_eq!(s.mentions(Id(10)), 1);
    assert_eq!(s.guild_unread(Id(G)), (true, 1));
    s.apply(&Dispatch::MessageCreate(Box::new(msg(601, 10, ME, "reply"))));
    assert!(!s.is_unread(Id(10)));
    assert_eq!(s.mentions(Id(10)), 0);
}

#[test]
fn live_messages_only_append_to_loaded_windows() {
    let mut s = setup();
    s.apply(&Dispatch::MessageCreate(Box::new(msg(600, 10, 2, "a"))));
    assert!(s.channel_messages(Id(10)).is_none());
    s.set_active_channel(Some(Id(10)));
    s.messages_loaded(Id(10), vec![msg(590, 10, 2, "x"), msg(580, 10, 3, "y")], None, false);
    s.apply(&Dispatch::MessageCreate(Box::new(msg(601, 10, 2, "b"))));
    let ids: Vec<u64> = s.channel_messages(Id(10)).unwrap().messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, vec![580, 590, 601]);
}

#[test]
fn history_paging_trims_newest_and_tracks_gap() {
    let mut s = setup();
    s.set_active_channel(Some(Id(10)));
    s.messages_loaded(Id(10), (91..=100).map(|i| msg(i, 10, 2, "m")).collect(), None, false);
    s.messages_loaded(Id(10), (81..=90).rev().map(|i| msg(i, 10, 2, "m")).collect(), Some(Id(91)), false);
    let cm = s.channel_messages(Id(10)).unwrap();
    assert_eq!(cm.messages.len(), 10);
    assert_eq!(cm.oldest_id(), Some(Id(81)));
    assert!(cm.has_newer_gap);
    // Live messages are dropped while the newest end is missing.
    s.apply(&Dispatch::MessageCreate(Box::new(msg(200, 10, 2, "live"))));
    assert!(s.channel_messages(Id(10)).unwrap().get(Id(200)).is_none());
    // Jump to present: latest page replaces the window.
    s.messages_loaded(Id(10), (191..=200).map(|i| msg(i, 10, 2, "m")).collect(), None, false);
    let cm = s.channel_messages(Id(10)).unwrap();
    assert!(!cm.has_newer_gap);
    assert_eq!(cm.newest_id(), Some(Id(200)));
    assert_eq!(cm.oldest_id(), Some(Id(191)));
}

#[test]
fn inactive_channels_shrink_and_lru_evicts() {
    let mut s = setup();
    s.set_active_channel(Some(Id(10)));
    s.messages_loaded(Id(10), (1..=10).map(|i| msg(i, 10, 2, "m")).collect(), None, false);
    s.set_active_channel(Some(Id(11)));
    assert_eq!(s.channel_messages(Id(10)).unwrap().messages.len(), 5);
    s.messages_loaded(Id(11), vec![msg(1, 11, 2, "m")], None, true);
    s.messages_loaded(Id(21), vec![msg(1, 21, 2, "m")], None, true);
    s.messages_loaded(Id(30), vec![msg(1, 30, 2, "m")], None, true);
    // max_loaded_channels = 3: oldest non-active (10) evicted.
    assert!(s.channel_messages(Id(10)).is_none());
    assert!(s.channel_messages(Id(11)).is_some());
}

#[test]
fn reactions_updates_and_deletes() {
    let mut s = setup();
    s.set_active_channel(Some(Id(10)));
    s.messages_loaded(Id(10), vec![msg(5, 10, 2, "hi")], None, true);
    let add = |user: u64| {
        d(
            "MESSAGE_REACTION_ADD",
            serde_json::json!({"user_id":user.to_string(),"channel_id":"10","message_id":"5","emoji":{"id":null,"name":"👍"}}),
        )
    };
    s.apply(&add(2));
    s.apply(&add(ME));
    s.apply(&add(ME)); // duplicate echo for us must not double count
    let r = &s.channel_messages(Id(10)).unwrap().get(Id(5)).unwrap().reactions[0];
    assert_eq!((r.count, r.me), (2, true));
    s.apply(&d(
        "MESSAGE_REACTION_REMOVE",
        serde_json::json!({"user_id":ME.to_string(),"channel_id":"10","message_id":"5","emoji":{"id":null,"name":"👍"}}),
    ));
    let r = &s.channel_messages(Id(10)).unwrap().get(Id(5)).unwrap().reactions[0];
    assert_eq!((r.count, r.me), (1, false));
    s.apply(&d(
        "MESSAGE_UPDATE",
        serde_json::json!({"id":"5","channel_id":"10","content":"edited","edited_timestamp":"2024-01-01T00:00:00Z"}),
    ));
    assert_eq!(s.channel_messages(Id(10)).unwrap().get(Id(5)).unwrap().content, "edited");
    s.apply(&d("MESSAGE_DELETE", serde_json::json!({"id":"5","channel_id":"10"})));
    assert!(s.channel_messages(Id(10)).unwrap().messages.is_empty());
}

#[test]
fn pending_message_replaced_by_echo() {
    let mut s = setup();
    s.set_active_channel(Some(Id(10)));
    s.messages_loaded(Id(10), vec![], None, true);
    s.add_pending(Id(10), "n1".into(), "hello".into());
    assert_eq!(s.channel_messages(Id(10)).unwrap().pending.len(), 1);
    let mut echo = msg(700, 10, ME, "hello");
    echo.nonce = Some(serde_json::json!("n1"));
    s.apply(&Dispatch::MessageCreate(Box::new(echo)));
    let cm = s.channel_messages(Id(10)).unwrap();
    assert!(cm.pending.is_empty());
    assert_eq!(cm.messages.len(), 1);
}

#[test]
fn typing_expires_and_clears_on_message() {
    let mut s = setup();
    s.apply(&d("TYPING_START", serde_json::json!({"channel_id":"10","user_id":"2","timestamp":1})));
    assert_eq!(s.typing_users(Id(10)), vec![Id(2)]);
    s.apply(&Dispatch::MessageCreate(Box::new(msg(600, 10, 2, "done"))));
    assert!(s.typing_users(Id(10)).is_empty());
    // Our own typing is ignored.
    s.apply(&d("TYPING_START", serde_json::json!({"channel_id":"10","user_id":ME.to_string(),"timestamp":1})));
    assert!(s.typing_users(Id(10)).is_empty());
}

#[test]
fn voice_states_track_join_move_leave() {
    let mut s = setup();
    assert_eq!(s.voice_members(Id(G), Id(22)).len(), 1);
    s.apply(&d(
        "VOICE_STATE_UPDATE",
        serde_json::json!({"guild_id":"1","channel_id":"22","user_id":"3","session_id":"y","self_mute":true}),
    ));
    assert_eq!(s.voice_members(Id(G), Id(22)).len(), 2);
    s.apply(&d("VOICE_STATE_UPDATE", serde_json::json!({"guild_id":"1","channel_id":null,"user_id":"2","session_id":"x"})));
    let v = s.voice_members(Id(G), Id(22));
    assert_eq!(v.len(), 1);
    assert!(v[0].self_mute);
}

#[test]
fn member_list_groups() {
    let s = setup();
    let groups = s.member_list(Id(G), None);
    let titles: Vec<&str> = groups.iter().map(|g| g.title.as_str()).collect();
    assert_eq!(titles, vec!["Mods", "Online", "Offline"]);
    assert_eq!(groups[0].members[0].name, "Ali");
    assert_eq!(groups[0].members[0].color, Some(16711680));
    assert_eq!(groups[0].members[0].activity.as_deref(), Some("Playing Rust"));
    assert_eq!(groups[1].members[0].status, Status::Idle);
    // Channel 12 is hidden from @everyone: nobody (except owner/admins) is listed.
    assert!(s.member_list(Id(G), Some(Id(12))).is_empty());
}

#[test]
fn dm_synthesized_and_ordered() {
    let mut s = setup();
    let mut m = msg(800, 55, 2, "dm");
    m.guild_id = None;
    s.apply(&Dispatch::MessageCreate(Box::new(m)));
    assert_eq!(s.private_channels, vec![Id(55)]);
    assert_eq!(s.dm_name(Id(55)), "alice");
    assert!(s.is_unread(Id(55)) || s.mentions(Id(55)) == 1);
    assert_eq!(s.mentions(Id(55)), 1);
}

#[test]
fn thread_archive_and_guild_delete() {
    let mut s = setup();
    s.apply(&d(
        "THREAD_UPDATE",
        serde_json::json!({"id":"30","type":11,"guild_id":"1","parent_id":"10","thread_metadata":{"archived":true}}),
    ));
    assert!(s.threads_of(Id(10)).is_empty());
    s.apply(&d("GUILD_DELETE", serde_json::json!({"id":"1","unavailable":true})));
    assert!(!s.guilds[&Id(G)].available);
    s.apply(&d("GUILD_DELETE", serde_json::json!({"id":"1"})));
    assert!(s.guilds.is_empty());
    assert!(!s.channels.contains_key(&Id(10)));
}

#[test]
fn around_page_replaces_window_with_gap() {
    let mut s = setup();
    s.set_active_channel(Some(Id(10)));
    s.messages_loaded(Id(10), (491..=500).map(|i| msg(i, 10, 2, "m")).collect(), None, false);
    s.messages_loaded_around(Id(10), (95..=105).rev().map(|i| msg(i, 10, 2, "m")).collect());
    let cm = s.channel_messages(Id(10)).unwrap();
    assert!(cm.has_newer_gap);
    assert_eq!(cm.oldest_id(), Some(Id(95)));
    // An around-page that reaches the newest message has no gap.
    s.messages_loaded_around(Id(10), (495..=500).map(|i| msg(i, 10, 2, "m")).collect());
    assert!(!s.channel_messages(Id(10)).unwrap().has_newer_gap);
}

#[test]
fn latest_page_without_overlap_replaces_window() {
    let mut s = setup();
    s.set_active_channel(Some(Id(10)));
    s.messages_loaded(Id(10), (1..=5).map(|i| msg(i, 10, 2, "old")).collect(), None, false);
    // Refetching the latest page after a long offline period: no overlap with 1..=5.
    s.messages_loaded(Id(10), (50..=54).map(|i| msg(i, 10, 2, "new")).collect(), None, false);
    let ids: Vec<u64> = s.channel_messages(Id(10)).unwrap().messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, vec![50, 51, 52, 53, 54]);
    // Overlapping refresh merges and keeps older context.
    s.messages_loaded(Id(10), (53..=56).map(|i| msg(i, 10, 2, "new")).collect(), None, false);
    let ids: Vec<u64> = s.channel_messages(Id(10)).unwrap().messages.iter().map(|m| m.id.0).collect();
    assert_eq!(ids, vec![50, 51, 52, 53, 54, 55, 56]);
}

#[test]
fn reset_keeps_accounts() {
    let mut s = setup();
    s.session.accounts.push(AccountView { id: "a".into(), ..Default::default() });
    s.reset_session_data();
    assert!(s.guilds.is_empty());
    assert_eq!(s.session.accounts.len(), 1);
}
