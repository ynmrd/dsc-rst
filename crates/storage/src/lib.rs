//! SQLite message cache with FTS5 search, read states and bounded pruning.
//!
//! [`Storage`] is synchronous and meant to be owned by one thread; [`StorageHandle`]
//! runs it on a dedicated thread and exposes an async `call` API.

mod error;
mod handle;
mod schema;
mod search;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dn_core::{Channel, Id, Message};
use rusqlite::types::Value;
use rusqlite::{Connection, ErrorCode, OptionalExtension, Statement, params, params_from_iter};

pub use error::StorageError;
pub use handle::StorageHandle;

const UPSERT_MESSAGE: &str = "INSERT INTO messages \
    (id, channel_id, guild_id, author_id, author_name, content, timestamp_ms, json) \
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
    ON CONFLICT(id) DO UPDATE SET channel_id = excluded.channel_id, guild_id = excluded.guild_id, \
    author_id = excluded.author_id, author_name = excluded.author_name, content = excluded.content, \
    timestamp_ms = excluded.timestamp_ms, json = excluded.json";

const UPDATE_MESSAGE: &str = "UPDATE messages SET channel_id = ?2, guild_id = ?3, author_id = ?4, \
    author_name = ?5, content = ?6, timestamp_ms = ?7, json = ?8 WHERE id = ?1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFilter {
    pub query: String,
    pub guild_id: Option<Id>,
    pub channel_id: Option<Id>,
    pub author_id: Option<Id>,
    pub limit: usize,
    pub offset: usize,
}

impl Default for SearchFilter {
    fn default() -> Self {
        Self { query: String::new(), guild_id: None, channel_id: None, author_id: None, limit: 50, offset: 0 }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageStats {
    /// Cached messages.
    pub messages: u64,
    /// Distinct channels that have cached messages.
    pub channels: u64,
    /// Database size in bytes (pages in use plus free pages, excluding the WAL file).
    pub db_bytes: u64,
}

pub struct Storage {
    conn: Connection,
}

enum OpenError {
    Corrupt(rusqlite::Error),
    Other(StorageError),
}

impl From<rusqlite::Error> for OpenError {
    fn from(e: rusqlite::Error) -> Self {
        match e.sqlite_error_code() {
            Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) => OpenError::Corrupt(e),
            _ => OpenError::Other(e.into()),
        }
    }
}

fn sql_id(id: Id) -> i64 {
    id.0 as i64
}

fn from_sql_id(v: i64) -> Id {
    Id(v as u64)
}

fn sql_count(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn unix_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Storage {
    /// Opens (creating if needed) the database at `path`. A database that SQLite reports
    /// as corrupt or not a database is renamed to `<name>.corrupt-<unix-secs>` and replaced
    /// by a fresh one.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        match Self::open_file(path) {
            Ok(conn) => Ok(Self { conn }),
            Err(OpenError::Corrupt(e)) => {
                let moved = quarantine(path)?;
                tracing::warn!("message cache {} is corrupt ({e}); moved to {} and recreating", path.display(), moved.display());
                match Self::open_file(path) {
                    Ok(conn) => Ok(Self { conn }),
                    Err(OpenError::Corrupt(e)) => Err(e.into()),
                    Err(OpenError::Other(e)) => Err(e),
                }
            }
            Err(OpenError::Other(e)) => Err(e),
        }
    }

    pub fn open_in_memory() -> Result<Self, StorageError> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn, false)?;
        schema::migrate(&mut conn, 0)?;
        Ok(Self { conn })
    }

    fn open_file(path: &Path) -> Result<Connection, OpenError> {
        let mut conn = Connection::open(path)?;
        configure(&conn, true)?;
        let version = schema::user_version(&conn)?;
        if version > schema::LATEST_VERSION {
            return Err(OpenError::Other(StorageError::Sqlite(format!(
                "database schema v{version} is newer than supported v{}",
                schema::LATEST_VERSION
            ))));
        }
        schema::migrate(&mut conn, version)?;
        Ok(conn)
    }

    pub fn upsert_messages(&mut self, msgs: &[Message]) -> Result<(), StorageError> {
        if msgs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(UPSERT_MESSAGE)?;
            for m in msgs {
                write_message(&mut stmt, m)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Replaces a cached message (reindexing its text). Messages that are not cached
    /// are left alone so a lone edit never creates a gap-ridden channel history.
    pub fn update_message(&mut self, msg: &Message) -> Result<(), StorageError> {
        let mut stmt = self.conn.prepare_cached(UPDATE_MESSAGE)?;
        write_message(&mut stmt, msg)
    }

    pub fn delete_messages(&mut self, channel: Id, ids: &[Id]) -> Result<(), StorageError> {
        if ids.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM messages WHERE channel_id = ?1 AND id = ?2")?;
            for id in ids {
                stmt.execute(params![sql_id(channel), sql_id(*id)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Newest `limit` messages with an id below `before`, oldest first.
    pub fn load_messages(&self, channel: Id, before: Option<Id>, limit: usize) -> Result<Vec<Message>, StorageError> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT json FROM messages WHERE channel_id = ?1 AND id < ?2 ORDER BY id DESC LIMIT ?3")?;
        let before = before.map_or(i64::MAX, sql_id);
        let rows = stmt.query_map(params![sql_id(channel), before, sql_count(limit)], |r| r.get::<_, String>(0))?;
        let mut out = decode_messages(rows)?;
        out.reverse();
        Ok(out)
    }

    pub fn latest_message_id(&self, channel: Id) -> Result<Option<Id>, StorageError> {
        let id: Option<i64> = self
            .conn
            .prepare_cached("SELECT MAX(id) FROM messages WHERE channel_id = ?1")?
            .query_row([sql_id(channel)], |r| r.get(0))?;
        Ok(id.map(from_sql_id))
    }

    /// Full-text search, newest first. With an empty query the filters alone select
    /// the messages; with neither a query nor a filter the result is empty. A query
    /// with no searchable characters (only symbols) matches nothing.
    pub fn search(&self, f: &SearchFilter) -> Result<Vec<Message>, StorageError> {
        let query = f.query.trim();
        let fts = if query.is_empty() {
            None
        } else {
            match search::fts_query(query) {
                Some(q) => Some(q),
                None => return Ok(Vec::new()),
            }
        };
        if fts.is_none() && f.guild_id.is_none() && f.channel_id.is_none() && f.author_id.is_none() {
            return Ok(Vec::new());
        }

        let mut args: Vec<Value> = Vec::new();
        let mut sql = match &fts {
            Some(q) => {
                args.push(Value::Text(q.clone()));
                String::from(
                    "SELECT m.json FROM messages_fts JOIN messages m ON m.id = messages_fts.rowid \
                     WHERE messages_fts MATCH ?",
                )
            }
            None => String::from("SELECT m.json FROM messages m WHERE 1 = 1"),
        };
        for (column, id) in [("guild_id", f.guild_id), ("channel_id", f.channel_id), ("author_id", f.author_id)] {
            if let Some(id) = id {
                sql.push_str(&format!(" AND m.{column} = ?"));
                args.push(Value::Integer(sql_id(id)));
            }
        }
        sql.push_str(if fts.is_some() { " ORDER BY messages_fts.rowid DESC" } else { " ORDER BY m.id DESC" });
        sql.push_str(" LIMIT ? OFFSET ?");
        args.push(Value::Integer(sql_count(f.limit)));
        args.push(Value::Integer(sql_count(f.offset)));

        let mut stmt = self.conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
        decode_messages(rows)
    }

    pub fn set_read_state(&mut self, channel: Id, last_read: Id) -> Result<(), StorageError> {
        self.conn
            .prepare_cached(
                "INSERT INTO read_states (channel_id, last_read_id) VALUES (?1, ?2) \
                 ON CONFLICT(channel_id) DO UPDATE SET last_read_id = excluded.last_read_id",
            )?
            .execute(params![sql_id(channel), sql_id(last_read)])?;
        Ok(())
    }

    pub fn read_states(&self) -> Result<HashMap<Id, Id>, StorageError> {
        let mut stmt = self.conn.prepare_cached("SELECT channel_id, last_read_id FROM read_states")?;
        let rows = stmt.query_map([], |r| Ok((from_sql_id(r.get(0)?), from_sql_id(r.get(1)?))))?;
        Ok(rows.collect::<Result<HashMap<_, _>, _>>()?)
    }

    /// Remembers a DM channel so it survives restarts (bots cannot list their DMs).
    pub fn upsert_private_channel(&mut self, ch: &Channel) -> Result<(), StorageError> {
        let json = serde_json::to_string(ch)?;
        self.conn
            .prepare_cached(
                "INSERT INTO private_channels (id, json) VALUES (?1, ?2) \
                 ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            )?
            .execute(params![sql_id(ch.id), json])?;
        Ok(())
    }

    /// Stored DM channels, most recently active first.
    pub fn private_channels(&self) -> Result<Vec<Channel>, StorageError> {
        let mut stmt = self.conn.prepare_cached("SELECT json FROM private_channels")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for json in rows {
            match serde_json::from_str::<Channel>(&json?) {
                Ok(ch) => out.push(ch),
                Err(e) => tracing::warn!("skipping undecodable private channel: {e}"),
            }
        }
        out.sort_by(|a, b| b.last_message_id.cmp(&a.last_message_id).then(b.id.cmp(&a.id)));
        Ok(out)
    }

    pub fn kv_get(&self, key: &str) -> Result<Option<String>, StorageError> {
        Ok(self.conn.prepare_cached("SELECT value FROM kv WHERE key = ?1")?.query_row([key], |r| r.get(0)).optional()?)
    }

    pub fn kv_set(&mut self, key: &str, value: &str) -> Result<(), StorageError> {
        self.conn
            .prepare_cached("INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value")?
            .execute(params![key, value])?;
        Ok(())
    }

    /// Deletes the oldest messages beyond `max_per_channel` per channel, then beyond
    /// `max_total` overall (ordered by snowflake, i.e. age), and returns the number
    /// removed. A limit of 0 removes everything. Freed pages are released to the OS.
    pub fn prune(&mut self, max_total: usize, max_per_channel: usize) -> Result<usize, StorageError> {
        let per_channel = sql_count(max_per_channel);
        let tx = self.conn.transaction()?;
        let crowded: Vec<i64> = {
            let mut stmt = tx.prepare("SELECT channel_id FROM messages GROUP BY channel_id HAVING COUNT(*) > ?1")?;
            stmt.query_map([per_channel], |r| r.get(0))?.collect::<Result<_, _>>()?
        };
        let mut deleted = 0;
        for channel in crowded {
            deleted += tx.execute(
                "DELETE FROM messages WHERE channel_id = ?1 AND id <= \
                 (SELECT id FROM messages WHERE channel_id = ?1 ORDER BY id DESC LIMIT 1 OFFSET ?2)",
                params![channel, per_channel],
            )?;
        }
        deleted += tx.execute(
            "DELETE FROM messages WHERE id <= (SELECT id FROM messages ORDER BY id DESC LIMIT 1 OFFSET ?1)",
            [sql_count(max_total)],
        )?;
        tx.commit()?;

        // SQLite frees one freelist page per step, so the statement must run to the end.
        let mut vacuum = self.conn.prepare("PRAGMA incremental_vacuum")?;
        let mut rows = vacuum.query([])?;
        while rows.next()?.is_some() {}
        drop(rows);
        drop(vacuum);
        if deleted > 0 {
            // Fold the WAL back so the file on disk actually shrinks.
            let _ = self.conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
        }
        Ok(deleted)
    }

    pub fn stats(&self) -> Result<StorageStats, StorageError> {
        let count = |sql: &str| -> Result<u64, StorageError> {
            Ok(self.conn.query_row(sql, [], |r| r.get::<_, i64>(0))?.max(0) as u64)
        };
        Ok(StorageStats {
            messages: count("SELECT COUNT(*) FROM messages")?,
            channels: count("SELECT COUNT(DISTINCT channel_id) FROM messages")?,
            db_bytes: count("PRAGMA page_count")? * count("PRAGMA page_size")?,
        })
    }

    fn close(self) {
        let _ = self.conn.execute_batch("PRAGMA optimize;");
    }
}

fn configure(conn: &Connection, wal: bool) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_millis(5000))?;
    // Must precede the first write: auto_vacuum cannot be switched on once tables exist.
    conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
    if wal {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "journal_size_limit", 32 * 1024 * 1024)?;
    }
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -8000)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.set_prepared_statement_cache_capacity(32);
    Ok(())
}

fn write_message(stmt: &mut Statement<'_>, m: &Message) -> Result<(), StorageError> {
    let json = serde_json::to_string(m)?;
    stmt.execute(params![
        sql_id(m.id),
        sql_id(m.channel_id),
        m.guild_id.map(sql_id),
        sql_id(m.author.id),
        m.author_name(),
        m.content,
        m.timestamp_ms() as i64,
        json,
    ])?;
    Ok(())
}

/// Rows that no longer decode (for example after a model change) are skipped rather
/// than failing the whole page.
fn decode_messages(rows: impl Iterator<Item = rusqlite::Result<String>>) -> Result<Vec<Message>, StorageError> {
    let mut out = Vec::new();
    for json in rows {
        match serde_json::from_str::<Message>(&json?) {
            Ok(m) => out.push(m),
            Err(e) => tracing::warn!("skipping undecodable cached message: {e}"),
        }
    }
    Ok(out)
}

fn quarantine(path: &Path) -> Result<PathBuf, StorageError> {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".corrupt-{}", unix_secs()));
    let dest = path.with_file_name(name);
    std::fs::rename(path, &dest)?;
    for suffix in ["-wal", "-shm"] {
        let mut side = path.as_os_str().to_os_string();
        side.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(side));
    }
    Ok(dest)
}
