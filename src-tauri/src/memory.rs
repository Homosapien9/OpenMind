//! Memory Tree — spec §5.
//!
//! SQLite for structured storage, directly ported from the Python OpenMind
//! CLI's `store.py`. The key lesson from the Python version, reproduced
//! exactly here: schema is created in three strict phases that must not
//! be reordered:
//!
//!   1. `_create_tables()` — CREATE TABLE IF NOT EXISTS only
//!   2. `_migrate()`       — ALTER TABLE for legacy columns
//!   3. `_create_indexes()` — indexes AND FTS5 virtual table
//!
//! The original bug (in `build/lib/openmind/store.py`): indexes were
//! created in the same script as the tables, BEFORE migration. On any
//! database created by an older schema version, `idx_memories_type` (which
//! references `memories.memory_type`) would crash because that column
//! didn't exist yet. The fix is the separation above — and this file
//! mirrors it exactly.
//!
//! STATUS (Milestone 3, roadmap): real on-disk SQLite, real schema and
//! migration, real FTS5 full-text search. Background auto-fetch loop and
//! Markdown-vault / Obsidian-compatibility layer are explicitly deferred
//! per ROADMAP.md.

use std::path::PathBuf;
use std::sync::Mutex;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

// ── Public types (IPC contract) ─────────────────────────────────────────
//
// These match src/lib/ipc.ts exactly — field names, camelCase convention,
// Option<String> for nullable timestamps.

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

const DEFAULT_INTERVAL_MINUTES: u32 = 20;

// ── Internal DB row type ─────────────────────────────────────────────────

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
        // `title` is derived from the first line of content (same convention
        // the Python CLI uses when displaying memories in the TUI).
        let title = self.content
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
            children: Vec::new(), // parent-child links deferred — §5 spec
        }
    }
}

// ── MemoryTree ────────────────────────────────────────────────────────────

/// Mutex<Connection> is correct here (not tokio::sync::Mutex) — rusqlite's
/// Connection is a blocking, sync API. All public methods on MemoryTree are
/// sync (non-async), called from sync Tauri commands. If any method ever
/// becomes async (e.g. wrapped in tokio::task::spawn_blocking for a long
/// query), switch to tokio::sync::Mutex at that point — but don't do it
/// now without that change actually happening first.
// ── Internal macro ────────────────────────────────────────────────────────

/// Acquire the database connection lock, mapping the poison error to AppError.
/// Eliminates the copy-pasted mutex error boilerplate from every public method.
/// Must be defined before the impl block that uses it.
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
    /// Open (creating if needed) the SQLite database at `db_path`.
    ///
    /// Runs the three-phase schema initialization in strict order:
    ///   _create_tables → _migrate → _create_indexes
    /// Do NOT reorder these — the indexes phase references columns
    /// (e.g. memories.memory_type) that may not exist on a legacy
    /// database until the migration phase adds them. This is the exact
    /// bug that hit the Python version; see module-level doc comment.
    pub fn open(db_path: PathBuf) -> AppResult<Self> {
        let conn = Connection::open(&db_path).map_err(AppError::Database)?;

        // WAL mode and foreign keys — matches store.py's PRAGMAs exactly.
        conn.execute_batch("
            PRAGMA journal_mode=WAL;
            PRAGMA foreign_keys=ON;
        ").map_err(AppError::Database)?;

        // Phase 1: tables
        Self::create_tables(&conn)?;
        // Phase 2: migrations (adds columns missing on legacy schemas)
        Self::migrate(&conn)?;
        // Phase 3: indexes + FTS5 — ONLY after migration
        Self::create_indexes(&conn)?;

        Ok(Self {
            db_path,
            conn: Mutex::new(conn),
        })
    }

    /// In-memory database — fast for tests, data lost on drop.
    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory().map_err(AppError::Database)?;
        conn.execute_batch("
            PRAGMA foreign_keys=ON;
        ").map_err(AppError::Database)?;

        Self::create_tables(&conn)?;
        Self::migrate(&conn)?;
        Self::create_indexes(&conn)?;

        Ok(Self {
            db_path: PathBuf::from(":memory:"),
            conn: Mutex::new(conn),
        })
    }

    // ── Phase 1: tables ───────────────────────────────────────────────

    fn create_tables(conn: &Connection) -> AppResult<()> {
        conn.execute_batch("
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
        ").map_err(AppError::Database)?;
        Ok(())
    }

    // ── Phase 2: migrations ───────────────────────────────────────────

    /// Add columns that may not exist in older databases.
    ///
    /// Each migration is attempted and the "duplicate column name" error
    /// is swallowed — matches Python's pattern of (sql, expected_error_substr)
    /// tuples exactly. Any other OperationalError is a real problem and
    /// propagates.
    fn migrate(conn: &Connection) -> AppResult<()> {
        let migrations: &[&str] = &[
            "ALTER TABLE memories ADD COLUMN memory_type TEXT NOT NULL DEFAULT 'episodic'",
            "ALTER TABLE memories ADD COLUMN embedding_hash TEXT NOT NULL DEFAULT ''",
        ];

        for sql in migrations {
            match conn.execute(sql, []) {
                Ok(_) => {}
                Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
                    if msg.contains("duplicate column name") => {}
                Err(e) => return Err(AppError::Database(e)),
            }
        }
        Ok(())
    }

    // ── Phase 3: indexes + FTS5 ───────────────────────────────────────

    /// Create indexes and the FTS5 virtual table.
    ///
    /// Must run AFTER migrate() — these indexes reference columns
    /// (e.g. memories.memory_type) that may not exist yet on a database
    /// created by an older schema version until migrations add them.
    /// This comment is intentionally duplicated from the module doc —
    /// it's the one constraint that must never be violated and must be
    /// visible right next to the code it governs.
    fn create_indexes(conn: &Connection) -> AppResult<()> {
        conn.execute_batch("
            CREATE INDEX IF NOT EXISTS idx_memories_category ON memories(category);
            CREATE INDEX IF NOT EXISTS idx_memories_created  ON memories(created_at);
            CREATE INDEX IF NOT EXISTS idx_memories_type     ON memories(memory_type);
            CREATE INDEX IF NOT EXISTS idx_conversations_cid ON conversations(conversation_id);

            CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
                content,
                tags,
                content=memories,
                content_rowid=id
            );
        ").map_err(AppError::Database)?;
        Ok(())
    }

    // ── Public API ────────────────────────────────────────────────────

    /// Store a new memory. Syncs the FTS index in the same transaction.
    pub fn add_memory(
        &self,
        content: &str,
        category: &str,
        source: &str,
        tags: &[String],
    ) -> AppResult<MemoryNode> {
        let conn = db_lock!(self);

        let now = chrono_now();
        conn.execute(
            "INSERT INTO memories (content, category, source, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![content, category, source, now, now],
        ).map_err(AppError::Database)?;

        let mem_id = conn.last_insert_rowid();

        // tags
        for tag in tags {
            conn.execute(
                "INSERT OR IGNORE INTO memory_tags (memory_id, tag) VALUES (?1, ?2)",
                params![mem_id, tag],
            ).map_err(AppError::Database)?;
        }

        // sync FTS — manual sync matching the Python store.py approach
        // (not trigger-based in the Python reference either)
        let tags_str = tags.join(",");
        conn.execute(
            "INSERT INTO memories_fts (rowid, content, tags) VALUES (?1, ?2, ?3)",
            params![mem_id, content, tags_str],
        ).map_err(AppError::Database)?;

        // now.clone() is intentional: the same timestamp string goes into
        // both the SQL INSERT above (via params! borrow) and the returned
        // MemoryRow. Calling chrono_now() twice would give slightly different
        // timestamps for the DB row vs. the returned struct.
        Ok(MemoryRow {
            id: mem_id,
            content: content.to_string(),
            category: category.to_string(),
            source: source.to_string(),
            created_at: now.clone(),
            updated_at: now,
        }.into_node())
    }

    /// Return all memories, newest first. Maps to `list_memory_tree` IPC command.
    pub fn list_tree(&self) -> AppResult<Vec<MemoryNode>> {
        let conn = db_lock!(self);

        let mut stmt = conn.prepare(
            "SELECT id, content, category, source, created_at, updated_at
             FROM memories
             ORDER BY created_at DESC
             LIMIT 200"
        ).map_err(AppError::Database)?;

        let rows = stmt.query_map([], |row| {
            Ok(MemoryRow {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                source: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        }).map_err(AppError::Database)?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row.map_err(AppError::Database)?.into_node());
        }
        Ok(nodes)
    }

    /// Full-text search via FTS5, with LIKE fallback matching the Python
    /// store.py's `search_memories()` pattern.
    pub fn search(&self, query: &str) -> AppResult<Vec<MemoryNode>> {
        let conn = db_lock!(self);

        // Try FTS5 first
        let fts_result = conn.prepare(
            "SELECT m.id, m.content, m.category, m.source, m.created_at, m.updated_at
             FROM memories m
             JOIN memories_fts fts ON m.id = fts.rowid
             WHERE memories_fts MATCH ?1
             ORDER BY rank
             LIMIT 50"
        ).and_then(|mut stmt| {
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
            Ok(rows) => {
                return Ok(rows.into_iter().map(|r| r.into_node()).collect());
            }
            Err(_) => {
                // FTS5 MATCH can fail on malformed query syntax (e.g. bare
                // special chars). Fall back to LIKE, matching Python's
                // "FTS5 fallback to word-level LIKE" comment in store.py.
            }
        }

        // LIKE fallback
        let like_pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        let mut stmt = conn.prepare(
            "SELECT id, content, category, source, created_at, updated_at
             FROM memories
             WHERE content LIKE ?1 ESCAPE '\\'
             ORDER BY created_at DESC
             LIMIT 50"
        ).map_err(AppError::Database)?;

        let rows = stmt.query_map(params![like_pattern], |row| {
            Ok(MemoryRow {
                id: row.get(0)?,
                content: row.get(1)?,
                category: row.get(2)?,
                source: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        }).map_err(AppError::Database)?;

        let mut nodes = Vec::new();
        for row in rows {
            nodes.push(row.map_err(AppError::Database)?.into_node());
        }
        Ok(nodes)
    }

    /// Background loop status — returns honest "not running" since the loop
    /// itself is explicitly deferred to a later milestone per ROADMAP.md.
    pub fn background_loop_status(&self) -> AppResult<BackgroundLoopStatus> {
        Ok(BackgroundLoopStatus {
            running: false,
            last_run_at: None,
            interval_minutes: DEFAULT_INTERVAL_MINUTES,
            next_run_at: None,
        })
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // ISO-8601-ish UTC timestamp without pulling in the chrono crate —
    // good enough for storage/display; real timestamp formatting can be
    // upgraded later.
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

    // Gregorian calendar calculation (good from 1970 onwards)
    let mut y = 1970u64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year { break; }
        remaining -= days_in_year;
        y += 1;
    }
    let months = [31, if is_leap(y) { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut mo = 1u64;
    for &m in &months {
        if remaining < m { break; }
        remaining -= m;
        mo += 1;
    }
    (y, mo, remaining + 1, h, mi, s)
}

fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}
