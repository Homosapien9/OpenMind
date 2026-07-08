use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryNode {
    pub id: String,
    pub title: String,
    pub content: String,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    pub children: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundLoopStatus {
    pub running: bool,
    pub last_run_at: Option<String>,
    pub interval_minutes: u32,
    pub next_run_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationEntry {
    pub thread_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: String,
    pub metadata: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSummary {
    pub thread_id: String,
    pub last_updated_at: String,
    pub preview: String,
    pub message_count: u32,
}

const DEFAULT_INTERVAL_MINUTES: u32 = 20;

struct MemoryRow {
    id: i64,
    content: String,
    category: String,
    source: String,
    created_at: String,
    updated_at: String,
}

impl MemoryRow {
    fn into_node(self) -> MemoryNode {
        let title = self
            .content
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(80)
            .collect::<String>();

        MemoryNode {
            id: self.id.to_string(),
            title: if title.is_empty() { self.category.clone() } else { title },
            content: self.content,
            source: self.source,
            created_at: self.created_at,
            updated_at: self.updated_at,
            children: Vec::new(),
        }
    }
}

macro_rules! db_lock {
    ($self:expr) => {
        $self.conn.lock().map_err(|_| {
            $crate::error::AppError::Other(anyhow::anyhow!("database mutex poisoned"))
        })?
    };
}

pub struct MemoryTree {
    #[allow(dead_code)]
    db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl MemoryTree {
    pub fn open(db_path: PathBuf) -> AppResult<Self> {
        let conn = Connection::open(&db_path).map_err(AppError::Database)?;
        conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=ON;
        ",
        )
        .map_err(AppError::Database)?;

        Self::create_tables(&conn)?;
        Self::migrate(&conn)?;
        Self::create_indexes(&conn)?;

        Ok(Self {
            db_path,
            conn: Mutex::new(conn),
        })
    }

    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory().map_err(AppError::Database)?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(AppError::Database)?;
        Self::create_tables(&conn)?;
        Self::migrate(&conn)?;
        Self::create_indexes(&conn)?;

        Ok(Self {
            db_path: PathBuf::from(":memory:"),
            conn: Mutex::new(conn),
        })
    }

    fn create_tables(conn: &Connection) -> AppResult<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS memories (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                content         TEXT    NOT NULL,
                category        TEXT    NOT NULL DEFAULT 'general',
                memory_type     TEXT    NOT NULL DEFAULT 'episodic',
                source          TEXT    NOT NULL DEFAULT '',
                created_at      TEXT    NOT NULL,
                updated_at      TEXT    NOT NULL,
                importance      REAL    NOT NULL DEFAULT 0.5,
                embedding_hash  TEXT    NOT NULL DEFAULT ''
            );

            CREATE TABLE IF NOT EXISTS memory_tags (
                memory_id   INTEGER NOT NULL REFERENCES memories(id) ON DELETE CASCADE,
                tag         TEXT    NOT NULL,
                PRIMARY KEY (memory_id, tag)
            );

            CREATE TABLE IF NOT EXISTS conversations (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT    NOT NULL,
                role            TEXT    NOT NULL,
                content         TEXT    NOT NULL,
                timestamp       TEXT    NOT NULL,
                metadata        TEXT    NOT NULL DEFAULT '{}'
            );
        ",
        )
        .map_err(AppError::Database)?;
        Ok(())
    }

    fn migrate(conn: &Connection) -> AppResult<()> {
        let migrations: &[&str] = &[
            "ALTER TABLE memories ADD COLUMN memory_type TEXT NOT NULL DEFAULT 'episodic'",
            "ALTER TABLE memories ADD COLUMN embedding_hash TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE conversations ADD COLUMN metadata TEXT NOT NULL DEFAULT '{}'",
        ];

        for sql in migrations {
            match conn.execute(sql, []) {
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(_, Some(ref msg))) if msg.contains("duplicate column name") => {}
                Err(e) => return Err(AppError::Database(e)),
            }
        }
        Ok(())
    }

    fn create_indexes(conn: &Connection) -> AppResult<()> {
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
            CREATE INDEX IF NOT EXISTS idx_memories_created  ON memories(created_at);
            CREATE INDEX IF NOT EXISTS idx_memories_type     ON memories(memory_type);
            CREATE INDEX IF NOT EXISTS idx_conversations_cid ON conversations(conversation_id);
            CREATE INDEX IF NOT EXISTS idx_conversations_ts  ON conversations(timestamp);

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                tags,
                content=memories,
                content_rowid=id
            );
        ",
        )
        .map_err(AppError::Database)?;
        Ok(())
    }

    pub fn add_memory(&self, content: &str, category: &str, source: &str, tags: &[String]) -> AppResult<MemoryNode> {
        let conn = db_lock!(self);
        let now = chrono_now();
        conn.execute(
            "INSERT INTO memories (content, category, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![content, category, source, now, now],
        )
        .map_err(AppError::Database)?;

        let mem_id = conn.last_insert_rowid();
        for tag in tags {
            conn.execute(
                "INSERT OR IGNORE INTO memory_tags (memory_id, tag) VALUES (?1, ?2)",
                params![mem_id, tag],
            )
            .map_err(AppError::Database)?;
        }

        conn.execute(
            "INSERT INTO memories_fts (rowid, content, tags) VALUES (?1, ?2, ?3)",
            params![mem_id, content, tags.join(",")],
        )
        .map_err(AppError::Database)?;

        Ok(MemoryRow {
            id: mem_id,
            content: content.to_string(),
            category: category.to_string(),
            source: source.to_string(),
            created_at: now.clone(),
            updated_at: now,
        }
        .into_node())
    }

    pub fn list_tree(&self) -> AppResult<Vec<MemoryNode>> {
        let conn = db_lock!(self);
        let mut stmt = conn
            .prepare(
                "SELECT id, content, category, source, created_at, updated_at
                 FROM memories
                 ORDER BY created_at DESC
                 LIMIT 200",
            )
            .map_err(AppError::Database)?;

        let rows = stmt
            .query_map([], |row| {
                Ok(MemoryRow {
                    id: row.get(0)?,
                    content: row.get(1)?,
                    category: row.get(2)?,
                    source: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                })
            })
            .map_err(AppError::Database)?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row.map_err(AppError::Database)?.into_node());
        }
        Ok(nodes)
    }

    pub fn search(&self, query: &str) -> AppResult<Vec<MemoryNode>> {
        let conn = db_lock!(self);
        // Try FTS5 full-text search first, fall back to LIKE if it fails
        let fts_result = conn
            .prepare(
                "SELECT m.id, m.content, m.category, m.source, m.created_at, m.updated_at
                 FROM memories m
                 JOIN memories_fts fts ON m.id = fts.rowid
                 WHERE memories_fts MATCH ?1
                 ORDER BY rank
                 LIMIT 50",
            )
            .and_then(|mut stmt| {
                let rows = stmt.query_map(params![query], |row| {
                    Ok(MemoryRow {
                        id: row.get(0)?,
                        content: row.get(1)?,
                        category: row.get(2)?,
                        source: row.get(3)?,
                        created_at: row.get(4)?,
                        updated_at: row.get(5)?,
                    })
                })?;
                rows.collect::<Result<Vec<_>, _>>()
            });

        match fts_result {
            Ok(rows) => Ok(rows.into_iter().map(|r| r.into_node()).collect()),
            Err(_) => {
                let like_pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
                let mut stmt = conn
                    .prepare(
                        "SELECT id, content, category, source, created_at, updated_at
                         FROM memories
                         WHERE content LIKE ?1 ESCAPE '\\'
                         ORDER BY created_at DESC
                         LIMIT 50",
                    )
                    .map_err(AppError::Database)?;
                let rows = stmt
                    .query_map(params![like_pattern], |row| {
                        Ok(MemoryRow {
                            id: row.get(0)?,
                            content: row.get(1)?,
                            category: row.get(2)?,
                            source: row.get(3)?,
                            created_at: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    })
                    .map_err(AppError::Database)?;
                let mut nodes = Vec::new();
                for row in rows {
                    nodes.push(row.map_err(AppError::Database)?.into_node());
                }
                Ok(nodes)
            }
        }
    }

    pub fn append_conversation_message(
        &self,
        thread_id: &str,
        role: &str,
        content: &str,
        metadata_json: &str,
    ) -> AppResult<()> {
        let conn = db_lock!(self);
        conn.execute(
            "INSERT INTO conversations (conversation_id, role, content, timestamp, metadata)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![thread_id, role, content, chrono_now(), metadata_json],
        )
        .map_err(AppError::Database)?;
        Ok(())
    }

    pub fn conversation_history(&self, thread_id: &str, limit: usize) -> AppResult<Vec<ConversationEntry>> {
        let conn = db_lock!(self);
        let mut stmt = conn
            .prepare(
                "SELECT conversation_id, role, content, timestamp, metadata
                 FROM conversations
                 WHERE conversation_id = ?1
                 ORDER BY id ASC
                 LIMIT ?2",
            )
            .map_err(AppError::Database)?;
        let rows = stmt
            .query_map(params![thread_id, limit as i64], |row| {
                Ok(ConversationEntry {
                    thread_id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    timestamp: row.get(3)?,
                    metadata: row.get(4)?,
                })
            })
            .map_err(AppError::Database)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(AppError::Database)?);
        }
        Ok(out)
    }

    pub fn list_threads(&self, limit: usize) -> AppResult<Vec<ThreadSummary>> {
        let conn = db_lock!(self);
        let mut stmt = conn
            .prepare(
                "SELECT c.conversation_id,
                        MAX(c.timestamp) AS last_updated_at,
                        COUNT(*) AS message_count,
                        COALESCE((
                          SELECT c2.content
                          FROM conversations c2
                          WHERE c2.conversation_id = c.conversation_id
                          ORDER BY c2.id DESC
                          LIMIT 1
                        ), '') AS preview
                 FROM conversations c
                 GROUP BY c.conversation_id
                 ORDER BY last_updated_at DESC
                 LIMIT ?1",
            )
            .map_err(AppError::Database)?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(ThreadSummary {
                    thread_id: row.get(0)?,
                    last_updated_at: row.get(1)?,
                    message_count: row.get::<_, i64>(2)? as u32,
                    preview: row.get::<_, String>(3)?.chars().take(80).collect(),
                })
            })
            .map_err(AppError::Database)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(AppError::Database)?);
        }
        Ok(out)
    }

    pub fn background_loop_status(&self) -> AppResult<BackgroundLoopStatus> {
        Ok(BackgroundLoopStatus {
            running: false,
            last_run_at: None,
            interval_minutes: DEFAULT_INTERVAL_MINUTES,
            next_run_at: None,
        })
    }
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, mo, d, h, mi, s) = epoch_to_ymd_hms(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

fn epoch_to_ymd_hms(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    let mins = secs / 60;
    let mi = mins % 60;
    let hours = mins / 60;
    let h = hours % 24;
    let days = hours / 24;

    let mut y = 1970u64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let months = [31, if is_leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 1u64;
    for &m in &months {
        if remaining < m {
            break;
        }
        remaining -= m;
        mo += 1;
    }
    (y, mo, remaining + 1, h, mi, s)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_order_allows_opening_db() {
        let tree = MemoryTree::open_in_memory().unwrap();
        let node = tree.add_memory("hello world", "general", "test", &[]).unwrap();
        assert_eq!(node.title, "hello world");
    }

    #[test]
    fn conversation_history_round_trip() {
        let tree = MemoryTree::open_in_memory().unwrap();
        tree.append_conversation_message("thread-1", "user", "hello", "{}").unwrap();
        tree.append_conversation_message("thread-1", "assistant", "hi", "{}").unwrap();
        let history = tree.conversation_history("thread-1", 20).unwrap();
        assert_eq!(history.len(), 2);
        let threads = tree.list_threads(10).unwrap();
        assert_eq!(threads[0].thread_id, "thread-1");
    }
}
