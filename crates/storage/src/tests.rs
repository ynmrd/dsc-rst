use dn_core::{
    Attachment, Channel, ChannelKind, Embed, EmbedField, Emoji, Id, Member, Message, MessageReference, Poll, PollAnswer, PollMedia,
    Reaction, StickerItem, User,
};

use super::*;

fn user(id: u64, name: &str) -> User {
    User { id: Id(id), username: name.into(), ..Default::default() }
}

fn msg(id: u64, channel: u64, author: u64, name: &str, content: &str) -> Message {
    Message {
        id: Id(id),
        channel_id: Id(channel),
        guild_id: Some(Id(1)),
        author: user(author, name),
        content: content.into(),
        timestamp: "2024-05-01T12:00:00.000000+00:00".into(),
        ..Default::default()
    }
}

fn ids(msgs: &[Message]) -> Vec<u64> {
    msgs.iter().map(|m| m.id.0).collect()
}

fn search(s: &Storage, query: &str) -> Vec<Message> {
    s.search(&SearchFilter { query: query.into(), ..Default::default() }).unwrap()
}

/// FTS5's own check that the index matches the content table exactly.
fn assert_fts_consistent(s: &Storage) {
    s.conn.execute("INSERT INTO messages_fts(messages_fts, rank) VALUES('integrity-check', 1)", []).unwrap();
}

#[test]
fn full_message_roundtrip() {
    let mut s = Storage::open_in_memory().unwrap();
    let mut m = msg(175_928_847_299_117_063, 42, 7, "alice", "hello **world** <@8>");
    m.edited_timestamp = Some("2024-05-01T12:01:00.000000+00:00".into());
    m.member = Some(Member { nick: Some("Ally".into()), roles: vec![Id(5), Id(6)], ..Default::default() });
    m.mentions = vec![user(8, "bob")];
    m.mention_roles = vec![Id(5)];
    m.attachments = vec![Attachment {
        id: Id(9),
        filename: "cat.png".into(),
        content_type: Some("image/png".into()),
        size: 1234,
        url: "https://cdn.example/cat.png".into(),
        width: Some(640),
        height: Some(480),
        duration_secs: Some(1.5),
        ..Default::default()
    }];
    m.embeds = vec![Embed {
        title: Some("Title".into()),
        color: Some(0x5865f2),
        fields: vec![EmbedField { name: "n".into(), value: "v".into(), inline: true }],
        ..Default::default()
    }];
    m.reactions = vec![Reaction { count: 3, me: true, emoji: Emoji::unicode("🔥") }];
    m.nonce = Some(serde_json::json!("nonce-1"));
    m.pinned = true;
    m.kind = 19;
    m.message_reference = Some(MessageReference { kind: 0, message_id: Some(Id(100)), channel_id: Some(Id(42)), ..Default::default() });
    m.referenced_message = Some(Box::new(msg(100, 42, 8, "bob", "original")));
    m.sticker_items = vec![StickerItem { id: Id(11), name: "wave".into(), format_type: 1 }];
    m.thread = Some(Channel { id: Id(77), kind: ChannelKind::PublicThread, name: Some("t".into()), ..Default::default() });
    m.poll = Some(Poll {
        question: PollMedia { text: Some("q?".into()), emoji: None },
        answers: vec![PollAnswer { answer_id: 1, poll_media: PollMedia { text: Some("yes".into()), emoji: None } }],
        ..Default::default()
    });

    s.upsert_messages(std::slice::from_ref(&m)).unwrap();
    let loaded = s.load_messages(Id(42), None, 10).unwrap();
    assert_eq!(loaded, vec![m.clone()]);

    // Nickname wins over username in the indexed author name.
    assert_eq!(search(&s, "ally").len(), 1);
    assert_eq!(s.latest_message_id(Id(42)).unwrap(), Some(Id(175_928_847_299_117_063)));
    assert_eq!(s.latest_message_id(Id(43)).unwrap(), None);
}

#[test]
fn upsert_is_idempotent_and_replaces() {
    let mut s = Storage::open_in_memory().unwrap();
    s.upsert_messages(&[msg(10, 1, 1, "a", "first")]).unwrap();
    s.upsert_messages(&[msg(10, 1, 1, "a", "second"), msg(11, 1, 1, "a", "other")]).unwrap();
    let all = s.load_messages(Id(1), None, 10).unwrap();
    assert_eq!(ids(&all), vec![10, 11]);
    assert_eq!(all[0].content, "second");
    assert!(search(&s, "first").is_empty());
    assert_fts_consistent(&s);
}

#[test]
fn pagination_is_ascending_and_before_is_exclusive() {
    let mut s = Storage::open_in_memory().unwrap();
    let base = 1_000_000u64;
    let all: Vec<Message> = (0..1000).map(|i| msg(base + i, if i % 2 == 0 { 1 } else { 2 }, 1, "a", &format!("m{i}"))).collect();
    for chunk in all.chunks(100) {
        s.upsert_messages(chunk).unwrap();
    }

    let mut collected: Vec<u64> = Vec::new();
    let mut before = None;
    let mut pages = 0;
    loop {
        let page = s.load_messages(Id(1), before, 64).unwrap();
        if page.is_empty() {
            break;
        }
        pages += 1;
        let page_ids = ids(&page);
        assert!(page_ids.windows(2).all(|w| w[0] < w[1]), "page must be ascending");
        assert!(page.iter().all(|m| m.channel_id == Id(1)));
        if let Some(b) = before {
            assert!(page_ids.iter().all(|&i| i < b.0), "before is exclusive");
            assert_eq!(page_ids.last().unwrap() + 2, collected[0], "pages must be contiguous");
        }
        before = Some(page[0].id);
        collected.splice(0..0, page_ids);
    }
    assert_eq!(pages, 8);
    assert_eq!(collected.len(), 500);
    assert!(collected.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(collected.first(), Some(&base));
    assert_eq!(collected.last(), Some(&(base + 998)));

    let newest = s.load_messages(Id(2), None, 3).unwrap();
    assert_eq!(ids(&newest), vec![base + 995, base + 997, base + 999]);
    let exact = s.load_messages(Id(2), Some(Id(base + 997)), 2).unwrap();
    assert_eq!(ids(&exact), vec![base + 993, base + 995]);
    assert!(s.load_messages(Id(3), None, 10).unwrap().is_empty());
    assert!(s.load_messages(Id(1), None, 0).unwrap().is_empty());
    assert_eq!(s.stats().unwrap().messages, 1000);
    assert_eq!(s.stats().unwrap().channels, 2);
}

fn search_fixture() -> Storage {
    let mut s = Storage::open_in_memory().unwrap();
    let mut other_guild = msg(5, 3, 2, "bob", "hello from another guild");
    other_guild.guild_id = Some(Id(2));
    s.upsert_messages(&[
        msg(1, 1, 1, "alice", "hello world"),
        msg(2, 1, 2, "bob", "helicopter parts and 🔥 fire"),
        msg(3, 2, 1, "alice", "Café résumé naïve"),
        msg(4, 2, 2, "bob", "東京 is big, hello again"),
        other_guild,
    ])
    .unwrap();
    s
}

#[test]
fn search_prefix_and_ordering() {
    let s = search_fixture();
    assert_eq!(ids(&search(&s, "hel")), vec![5, 4, 2, 1]);
    assert_eq!(ids(&search(&s, "hello")), vec![5, 4, 1]);
    assert_eq!(ids(&search(&s, "HELLO wor")), vec![1]);
    assert_eq!(ids(&search(&s, "cafe resume")), vec![3], "diacritics are folded");
    assert_eq!(ids(&search(&s, "東京")), vec![4]);
    assert_eq!(ids(&search(&s, "fire")), vec![2]);
    assert!(search(&s, "nonexistent").is_empty());
    // Author names are indexed too.
    assert_eq!(ids(&search(&s, "alice")), vec![3, 1]);

    let page = s.search(&SearchFilter { query: "hel".into(), limit: 2, offset: 1, ..Default::default() }).unwrap();
    assert_eq!(ids(&page), vec![4, 2]);
}

#[test]
fn search_filters() {
    let s = search_fixture();
    let by = |f: SearchFilter| ids(&s.search(&f).unwrap());
    let base = SearchFilter { query: "hello".into(), ..Default::default() };

    assert_eq!(by(SearchFilter { channel_id: Some(Id(1)), ..base.clone() }), vec![1]);
    assert_eq!(by(SearchFilter { author_id: Some(Id(2)), ..base.clone() }), vec![5, 4]);
    assert_eq!(by(SearchFilter { guild_id: Some(Id(2)), ..base.clone() }), vec![5]);
    assert_eq!(by(SearchFilter { guild_id: Some(Id(1)), author_id: Some(Id(2)), ..base.clone() }), vec![4]);

    // Empty query lists by filters alone, newest first.
    let listing = |f: SearchFilter| by(SearchFilter { query: "  ".into(), ..f });
    assert_eq!(listing(SearchFilter { channel_id: Some(Id(2)), ..Default::default() }), vec![4, 3]);
    assert_eq!(listing(SearchFilter { author_id: Some(Id(1)), limit: 1, ..Default::default() }), vec![3]);
    assert_eq!(listing(SearchFilter { author_id: Some(Id(1)), limit: 1, offset: 1, ..Default::default() }), vec![1]);
    assert!(listing(SearchFilter::default()).is_empty());
}

#[test]
fn adversarial_queries_never_error() {
    let s = search_fixture();
    let nasty = [
        "\"",
        "\"\"",
        "\"unterminated",
        "*",
        "**",
        "hel*",
        "AND",
        "OR",
        "NOT",
        "a AND",
        "NEAR(",
        "NEAR(a b)",
        "NEAR(a b, 2)",
        "-",
        "--x",
        "-hello",
        "^hello",
        "content:hello",
        "author_name:bob",
        "{content}: hello",
        "(",
        ")",
        "((a)",
        "a OR (b",
        "héllo",
        "🔥",
        "🔥🔥🔥",
        "東京 AND",
        "\u{0}",
        "hello\u{0}world",
        "'; DROP TABLE messages; --",
        "%",
        "_",
        "\\",
        "a\"b\"c",
        "\n\t ",
        "\u{202e}hello",
        "+",
        ":",
        "::",
        "a:b:c",
        "NEAR",
        "hello world foo bar baz qux",
        &"x".repeat(10_000),
    ];
    for q in nasty {
        s.search(&SearchFilter { query: q.into(), ..Default::default() }).unwrap_or_else(|e| panic!("query {q:?} failed: {e}"));
        s.search(&SearchFilter { query: q.into(), channel_id: Some(Id(1)), author_id: Some(Id(1)), ..Default::default() })
            .unwrap_or_else(|e| panic!("filtered query {q:?} failed: {e}"));
    }
    // Symbols are inert literals rather than operators.
    assert!(search(&s, "-hello").len() == 3);
    assert_eq!(ids(&search(&s, "🔥 fire")), vec![2]);
}

#[test]
fn update_message_reindexes() {
    let mut s = Storage::open_in_memory().unwrap();
    s.upsert_messages(&[msg(1, 1, 1, "alice", "alpha banana")]).unwrap();
    assert_eq!(search(&s, "banana").len(), 1);

    let mut edited = msg(1, 1, 1, "alice", "alpha cherry");
    edited.edited_timestamp = Some("2024-05-01T12:05:00.000000+00:00".into());
    s.update_message(&edited).unwrap();
    assert!(search(&s, "banana").is_empty());
    assert_eq!(ids(&search(&s, "cherry")), vec![1]);
    assert_eq!(s.load_messages(Id(1), None, 5).unwrap(), vec![edited.clone()]);

    // A change that does not touch indexed text keeps the index intact.
    edited.pinned = true;
    s.update_message(&edited).unwrap();
    assert_eq!(ids(&search(&s, "cherry")), vec![1]);
    assert!(s.load_messages(Id(1), None, 5).unwrap()[0].pinned);
    assert_fts_consistent(&s);

    // Unknown messages are not created by an edit.
    s.update_message(&msg(2, 1, 1, "alice", "ghost")).unwrap();
    assert!(search(&s, "ghost").is_empty());
    assert_eq!(s.stats().unwrap().messages, 1);
}

#[test]
fn delete_removes_from_index() {
    let mut s = Storage::open_in_memory().unwrap();
    s.upsert_messages(&[msg(1, 1, 1, "a", "keep me"), msg(2, 1, 1, "a", "remove me"), msg(3, 2, 1, "a", "remove me too")]).unwrap();
    s.delete_messages(Id(1), &[Id(2), Id(3), Id(99)]).unwrap();
    assert_eq!(ids(&search(&s, "remove")), vec![3], "other channels are untouched");
    assert_eq!(ids(&s.load_messages(Id(1), None, 10).unwrap()), vec![1]);
    s.delete_messages(Id(2), &[Id(3)]).unwrap();
    assert!(search(&s, "remove").is_empty());
    s.delete_messages(Id(1), &[]).unwrap();
    assert_fts_consistent(&s);
}

#[test]
fn prune_enforces_limits() {
    let mut s = Storage::open_in_memory().unwrap();
    let all: Vec<Message> = (1..=100).map(|i| msg(i, if i <= 80 { 1 } else { 2 }, 1, "a", &format!("word{i}"))).collect();
    s.upsert_messages(&all).unwrap();

    assert_eq!(s.prune(1000, 1000).unwrap(), 0);

    // Channel 1 keeps its newest 50, channel 2 is under the cap.
    assert_eq!(s.prune(1000, 50).unwrap(), 30);
    let ch1 = s.load_messages(Id(1), None, 500).unwrap();
    assert_eq!(ids(&ch1).first(), Some(&31));
    assert_eq!(ch1.len(), 50);
    assert_eq!(s.load_messages(Id(2), None, 500).unwrap().len(), 20);
    assert!(search(&s, "word30").is_empty());
    assert!(!search(&s, "word31").is_empty());

    // Total cap removes the globally oldest.
    assert_eq!(s.prune(30, 50).unwrap(), 40);
    let stats = s.stats().unwrap();
    assert_eq!(stats.messages, 30);
    assert_eq!(ids(&s.load_messages(Id(2), None, 500).unwrap()).len(), 20);
    assert_eq!(s.load_messages(Id(1), None, 500).unwrap().len(), 10);
    assert_fts_consistent(&s);

    assert_eq!(s.prune(0, 100).unwrap(), 30);
    assert_eq!(s.stats().unwrap().messages, 0);
}

#[test]
fn prune_releases_disk_space() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let mut s = Storage::open(&path).unwrap();
    let text = "lorem ipsum dolor sit amet ".repeat(40);
    let all: Vec<Message> = (1..=4000).map(|i| msg(i, 1, 1, "a", &format!("{text}{i}"))).collect();
    for chunk in all.chunks(100) {
        s.upsert_messages(chunk).unwrap();
    }
    let before = s.stats().unwrap().db_bytes;
    assert_eq!(s.prune(100, 100).unwrap(), 3900);
    let after = s.stats().unwrap().db_bytes;
    assert!(after < before / 4, "expected shrink, {before} -> {after}");
}

#[test]
fn read_states_roundtrip() {
    let mut s = Storage::open_in_memory().unwrap();
    assert!(s.read_states().unwrap().is_empty());
    s.set_read_state(Id(1), Id(100)).unwrap();
    s.set_read_state(Id(2), Id(200)).unwrap();
    s.set_read_state(Id(1), Id(150)).unwrap();
    let states = s.read_states().unwrap();
    assert_eq!(states.len(), 2);
    assert_eq!(states[&Id(1)], Id(150));
    assert_eq!(states[&Id(2)], Id(200));
}

#[test]
fn private_channels_roundtrip() {
    let mut s = Storage::open_in_memory().unwrap();
    let dm = |id: u64, last: u64, name: &str| Channel {
        id: Id(id),
        kind: ChannelKind::Dm,
        last_message_id: Some(Id(last)),
        recipients: vec![user(id + 1000, name)],
        ..Default::default()
    };
    s.upsert_private_channel(&dm(1, 10, "bob")).unwrap();
    s.upsert_private_channel(&dm(2, 30, "carol")).unwrap();
    s.upsert_private_channel(&dm(1, 50, "bob")).unwrap();
    let got = s.private_channels().unwrap();
    assert_eq!(got.iter().map(|c| c.id.0).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(got[0], dm(1, 50, "bob"));
}

#[test]
fn kv_roundtrip() {
    let mut s = Storage::open_in_memory().unwrap();
    assert_eq!(s.kv_get("k").unwrap(), None);
    s.kv_set("k", "v1").unwrap();
    s.kv_set("k", "v2").unwrap();
    assert_eq!(s.kv_get("k").unwrap().as_deref(), Some("v2"));
}

#[test]
fn file_database_persists_and_is_tuned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/dir/cache.sqlite3");
    {
        let mut s = Storage::open(&path).unwrap();
        s.upsert_messages(&[msg(1, 1, 1, "a", "persisted text")]).unwrap();
        s.kv_set("x", "y").unwrap();
        let pragma = |name: &str| s.conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(pragma("auto_vacuum"), 2, "incremental");
        assert_eq!(pragma("synchronous"), 1, "normal");
        assert_eq!(pragma("cache_size"), -8000);
        assert_eq!(pragma("busy_timeout"), 5000);
        assert_eq!(pragma("temp_store"), 2, "memory");
        assert_eq!(pragma("user_version"), schema::LATEST_VERSION as i64);
        let mode: String = s.conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
        assert_eq!(mode, "wal");
    }
    let s = Storage::open(&path).unwrap();
    assert_eq!(ids(&search(&s, "persisted")), vec![1]);
    assert_eq!(s.kv_get("x").unwrap().as_deref(), Some("y"));
}

#[test]
fn corrupt_database_is_quarantined_and_recreated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    std::fs::write(&path, b"this is definitely not a sqlite database, just garbage bytes ".repeat(200)).unwrap();
    std::fs::write(dir.path().join("cache.sqlite3-wal"), b"stale wal").unwrap();

    let mut s = Storage::open(&path).unwrap();
    s.upsert_messages(&[msg(1, 1, 1, "a", "fresh start")]).unwrap();
    assert_eq!(ids(&search(&s, "fresh")), vec![1]);
    drop(s);

    let quarantined: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("cache.sqlite3.corrupt-"))
        .collect();
    assert_eq!(quarantined.len(), 1, "{quarantined:?}");
    let kept = std::fs::read(dir.path().join(&quarantined[0])).unwrap();
    assert!(kept.starts_with(b"this is definitely not"));

    // Reopening the recreated database keeps its data and does not quarantine again.
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.stats().unwrap().messages, 1);
}

#[test]
fn truncated_database_header_is_recovered() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    drop(Storage::open(&path).unwrap());
    std::fs::write(&path, b"SQLite format 3\0 truncated").unwrap();
    let s = Storage::open(&path).unwrap();
    assert_eq!(s.stats().unwrap().messages, 0);
}

#[test]
fn newer_schema_is_an_error_not_a_reset() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    {
        let s = Storage::open(&path).unwrap();
        s.conn.execute_batch("PRAGMA user_version = 99").unwrap();
    }
    let err = Storage::open(&path).err().expect("must fail");
    assert!(matches!(err, StorageError::Sqlite(ref m) if m.contains("newer")), "{err:?}");
    assert!(path.exists());
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains("corrupt")).count(),
        0
    );
}

#[tokio::test]
async fn handle_runs_jobs_on_dedicated_thread() {
    let handle = StorageHandle::spawn(Storage::open_in_memory().unwrap());
    let name = handle.call(|_| std::thread::current().name().map(str::to_owned)).await.unwrap();
    assert_eq!(name.as_deref(), Some("dn-storage"));

    handle.call(|s| s.upsert_messages(&[msg(1, 1, 1, "a", "via handle")])).await.unwrap().unwrap();
    let found = handle.call(|s| s.search(&SearchFilter { query: "handle".into(), ..Default::default() })).await.unwrap().unwrap();
    assert_eq!(ids(&found), vec![1]);

    let clone = handle.clone();
    let stats = tokio::spawn(async move { clone.call(|s| s.stats()).await }).await.unwrap().unwrap().unwrap();
    assert_eq!(stats.messages, 1);

    handle.shutdown();
    assert_eq!(handle.call(|_| ()).await, Err(StorageError::Closed));
    handle.shutdown();
}

#[tokio::test]
async fn shutdown_drains_queued_jobs() {
    let handle = StorageHandle::spawn(Storage::open_in_memory().unwrap());
    handle.enqueue(|_| std::thread::sleep(Duration::from_millis(50))).unwrap();
    let pending: Vec<_> = (0..20).map(|i| handle.enqueue(move |s| s.kv_set(&format!("k{i}"), "v")).unwrap()).collect();
    handle.shutdown();
    for rx in pending {
        rx.await.unwrap().unwrap().unwrap();
    }
}

#[tokio::test]
async fn panicking_job_does_not_kill_the_thread() {
    let handle = StorageHandle::spawn(Storage::open_in_memory().unwrap());
    let err = handle.call(|_| -> () { panic!("boom") }).await.unwrap_err();
    assert!(matches!(err, StorageError::Sqlite(_)));
    assert_eq!(handle.call(|s| s.stats().map(|s| s.messages)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn dropping_last_handle_closes_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cache.sqlite3");
    let handle = StorageHandle::spawn(Storage::open(&path).unwrap());
    handle.call(|s| s.kv_set("a", "b")).await.unwrap().unwrap();
    drop(handle);
    // The connection is closed after drop returns, so the WAL is checkpointed away.
    assert!(!dir.path().join("cache.sqlite3-wal").exists() || std::fs::metadata(dir.path().join("cache.sqlite3-wal")).unwrap().len() == 0);
    assert_eq!(Storage::open(&path).unwrap().kv_get("a").unwrap().as_deref(), Some("b"));
}
