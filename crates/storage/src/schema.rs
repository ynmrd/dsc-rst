//! Schema migrations driven by `PRAGMA user_version`.

use rusqlite::Connection;

const V1: &str = r#"
CREATE TABLE messages (
    id           INTEGER PRIMARY KEY,
    channel_id   INTEGER NOT NULL,
    guild_id     INTEGER,
    author_id    INTEGER NOT NULL,
    author_name  TEXT    NOT NULL,
    content      TEXT    NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    json         TEXT    NOT NULL
);
CREATE INDEX idx_messages_channel ON messages (channel_id, id);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    content, author_name,
    content = 'messages', content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2',
    prefix = '2 3'
);

CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts (rowid, content, author_name)
    VALUES (new.id, new.content, new.author_name);
END;
CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, content, author_name)
    VALUES ('delete', old.id, old.content, old.author_name);
END;
-- Skipped when only `json` changed (reactions, embeds) to avoid index churn.
CREATE TRIGGER messages_au AFTER UPDATE ON messages
WHEN old.content IS NOT new.content OR old.author_name IS NOT new.author_name BEGIN
    INSERT INTO messages_fts (messages_fts, rowid, content, author_name)
    VALUES ('delete', old.id, old.content, old.author_name);
    INSERT INTO messages_fts (rowid, content, author_name)
    VALUES (new.id, new.content, new.author_name);
END;

CREATE TABLE read_states (
    channel_id   INTEGER PRIMARY KEY,
    last_read_id INTEGER NOT NULL
);

CREATE TABLE private_channels (
    id   INTEGER PRIMARY KEY,
    json TEXT NOT NULL
);

CREATE TABLE kv (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) WITHOUT ROWID;
"#;

const MIGRATIONS: &[&str] = &[V1];

pub(crate) const LATEST_VERSION: u32 = MIGRATIONS.len() as u32;

pub(crate) fn user_version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

pub(crate) fn migrate(conn: &mut Connection, from: u32) -> rusqlite::Result<()> {
    for (idx, sql) in MIGRATIONS.iter().enumerate().skip(from as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute_batch(&format!("PRAGMA user_version = {}", idx + 1))?;
        tx.commit()?;
    }
    Ok(())
}
