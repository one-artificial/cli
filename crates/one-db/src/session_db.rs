//! Per-session SQLite database — the single source of truth for one conversation.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{Connection, params};

pub struct SessionDb {
    conn: Connection,
    pub path: PathBuf,
}

// ── Row types ─────────────────────────────────────────────────────────────────

pub struct MessageRow {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: String,
    pub tokens_used: Option<i64>,
    pub is_evergreen_compressed: bool,
}

pub struct ToolCallRow {
    pub id: i64,
    pub message_id: i64,
    pub tool_name: String,
    pub input_json: String,
    pub output: Option<String>,
    pub is_error: bool,
    pub duration_ms: Option<i64>,
    pub created_at: String,
}

pub struct EvergreenChunkRow {
    pub id: i64,
    pub span_start_id: i64,
    pub span_end_id: i64,
    pub summary: String,
    /// JSON array of message IDs that must always be retrieved verbatim.
    pub vital_refs: Option<String>,
    pub created_at: String,
}

/// Snapshot of all `session_meta` keys as a typed struct.
pub struct SessionMeta {
    pub session_id: String,
    pub project_path: String,
    pub branch: String,
    pub tab_name: Option<String>,
    pub provider: String,
    pub model: String,
    pub effort: Option<String>,
    pub cwd: String,
    pub cost_usd: f64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub created_at: String,
    pub last_active_at: String,
    /// Source if imported ("claude-code" | "codex" | "gemini" | None)
    pub imported_from: Option<String>,
}

// ── Lifecycle ─────────────────────────────────────────────────────────────────

impl SessionDb {
    /// Open (or create) a session database at `path`.
    /// Creates all parent directories automatically.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn,
            path: PathBuf::from(":memory:"),
        };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS messages (
                id                      INTEGER PRIMARY KEY AUTOINCREMENT,
                role                    TEXT    NOT NULL,
                content                 TEXT    NOT NULL,
                created_at              TEXT    NOT NULL,
                tokens_used             INTEGER,
                is_evergreen_compressed INTEGER NOT NULL DEFAULT 0
            );

            -- Tool executions linked to an assistant message
            CREATE TABLE IF NOT EXISTS tool_calls (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                message_id  INTEGER NOT NULL REFERENCES messages(id),
                tool_name   TEXT    NOT NULL,
                input_json  TEXT    NOT NULL,
                output      TEXT,
                is_error    INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER,
                created_at  TEXT    NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tool_calls_message
                ON tool_calls(message_id);

            -- Arbitrary key-value metadata (provider, model, cwd, tab_name, costs, etc.)
            CREATE TABLE IF NOT EXISTS session_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            -- Compressed context spans produced by the Evergreen background task
            CREATE TABLE IF NOT EXISTS evergreen_chunks (
                id            INTEGER PRIMARY KEY AUTOINCREMENT,
                span_start_id INTEGER NOT NULL,
                span_end_id   INTEGER NOT NULL,
                summary       TEXT    NOT NULL,
                vital_refs    TEXT,       -- JSON array of message IDs to always fetch verbatim
                created_at    TEXT    NOT NULL
            );

            -- Raw JSONL lines preserved for lossless import/export round-trips
            CREATE TABLE IF NOT EXISTS jsonl_turns (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                message_id INTEGER REFERENCES messages(id),
                jsonl_line TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            ",
        )?;
        Ok(())
    }
}

// ── Messages ──────────────────────────────────────────────────────────────────

impl SessionDb {
    /// Append a turn. Returns the new `messages.id`.
    pub fn save_message(
        &self,
        role: &str,
        content: &str,
        created_at: &str,
        tokens_used: Option<i64>,
    ) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO messages (role, content, created_at, tokens_used)
             VALUES (?1, ?2, ?3, ?4)",
            params![role, content, created_at, tokens_used],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Load messages, optionally only uncompressed ones after `since_id`.
    /// If `compressed` is false, skips evergreen-compressed rows.
    pub fn load_messages(
        &self,
        since_id: Option<i64>,
        limit: Option<usize>,
        include_compressed: bool,
    ) -> Result<Vec<MessageRow>> {
        let since = since_id.unwrap_or(0);
        let cap = limit.map(|l| l as i64).unwrap_or(i64::MAX);
        let compressed_filter = if include_compressed { 1 } else { 0 };

        let mut stmt = self.conn.prepare(
            "SELECT id, role, content, created_at, tokens_used, is_evergreen_compressed
             FROM messages
             WHERE id > ?1
               AND (is_evergreen_compressed = 0 OR ?2 = 1)
             ORDER BY id ASC
             LIMIT ?3",
        )?;

        let rows = stmt
            .query_map(params![since, compressed_filter, cap], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    created_at: row.get(3)?,
                    tokens_used: row.get(4)?,
                    is_evergreen_compressed: row.get::<_, i64>(5)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Load the N most-recent uncompressed messages (write tier).
    pub fn load_recent_messages(&self, limit: usize) -> Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, role, content, created_at, tokens_used, is_evergreen_compressed
             FROM messages
             WHERE is_evergreen_compressed = 0
             ORDER BY id DESC
             LIMIT ?1",
        )?;
        let mut rows: Vec<MessageRow> = stmt
            .query_map(params![limit as i64], |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    created_at: row.get(3)?,
                    tokens_used: row.get(4)?,
                    is_evergreen_compressed: row.get::<_, i64>(5)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.reverse(); // return chronological order
        Ok(rows)
    }

    /// Fetch a single message by ID — used by the recall_detail tool.
    pub fn get_message(&self, id: i64) -> Result<Option<MessageRow>> {
        let result = self.conn.query_row(
            "SELECT id, role, content, created_at, tokens_used, is_evergreen_compressed
             FROM messages WHERE id = ?1",
            params![id],
            |row| {
                Ok(MessageRow {
                    id: row.get(0)?,
                    role: row.get(1)?,
                    content: row.get(2)?,
                    created_at: row.get(3)?,
                    tokens_used: row.get(4)?,
                    is_evergreen_compressed: row.get::<_, i64>(5)? != 0,
                })
            },
        );
        match result {
            Ok(row) => Ok(Some(row)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn message_count(&self) -> Result<usize> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Mark a span of messages as evergreen-compressed so they're excluded from
    /// the default context window. Original content is preserved for recall_detail.
    pub fn mark_messages_compressed(&self, start_id: i64, end_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET is_evergreen_compressed = 1
             WHERE id >= ?1 AND id <= ?2",
            params![start_id, end_id],
        )?;
        Ok(())
    }
}

// ── Tool calls ────────────────────────────────────────────────────────────────

impl SessionDb {
    pub fn save_tool_call(
        &self,
        message_id: i64,
        tool_name: &str,
        input_json: &str,
        output: Option<&str>,
        is_error: bool,
        duration_ms: Option<i64>,
    ) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO tool_calls
             (message_id, tool_name, input_json, output, is_error, duration_ms, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                message_id,
                tool_name,
                input_json,
                output,
                is_error as i64,
                duration_ms,
                now,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn load_tool_calls_for_message(&self, message_id: i64) -> Result<Vec<ToolCallRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, message_id, tool_name, input_json, output, is_error, duration_ms, created_at
             FROM tool_calls WHERE message_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![message_id], |row| {
                Ok(ToolCallRow {
                    id: row.get(0)?,
                    message_id: row.get(1)?,
                    tool_name: row.get(2)?,
                    input_json: row.get(3)?,
                    output: row.get(4)?,
                    is_error: row.get::<_, i64>(5)? != 0,
                    duration_ms: row.get(6)?,
                    created_at: row.get(7)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

// ── Metadata ─────────────────────────────────────────────────────────────────

impl SessionDb {
    pub fn get_meta(&self, key: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT value FROM session_meta WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO session_meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// Read all well-known meta keys into a typed struct.
    pub fn load_session_meta(&self) -> Result<SessionMeta> {
        let get = |k: &str| -> String { self.get_meta(k).unwrap_or_default().unwrap_or_default() };
        Ok(SessionMeta {
            session_id: get("session_id"),
            project_path: get("project_path"),
            branch: get("branch"),
            tab_name: self.get_meta("tab_name")?,
            provider: get("provider"),
            model: get("model"),
            effort: self.get_meta("effort")?,
            cwd: get("cwd"),
            cost_usd: get("cost_usd").parse().unwrap_or(0.0),
            input_tokens: get("input_tokens").parse().unwrap_or(0),
            output_tokens: get("output_tokens").parse().unwrap_or(0),
            created_at: get("created_at"),
            last_active_at: get("last_active_at"),
            imported_from: self.get_meta("imported_from")?,
        })
    }

    /// Write all session-level fields from a typed struct.
    pub fn save_session_meta(&self, meta: &SessionMeta) -> Result<()> {
        let pairs: &[(&str, String)] = &[
            ("session_id", meta.session_id.clone()),
            ("project_path", meta.project_path.clone()),
            ("branch", meta.branch.clone()),
            ("provider", meta.provider.clone()),
            ("model", meta.model.clone()),
            ("cwd", meta.cwd.clone()),
            ("cost_usd", meta.cost_usd.to_string()),
            ("input_tokens", meta.input_tokens.to_string()),
            ("output_tokens", meta.output_tokens.to_string()),
            ("created_at", meta.created_at.clone()),
            ("last_active_at", meta.last_active_at.clone()),
        ];
        for (k, v) in pairs {
            self.set_meta(k, v)?;
        }
        if let Some(ref v) = meta.tab_name {
            self.set_meta("tab_name", v)?;
        }
        if let Some(ref v) = meta.effort {
            self.set_meta("effort", v)?;
        }
        if let Some(ref v) = meta.imported_from {
            self.set_meta("imported_from", v)?;
        }
        Ok(())
    }
}

// ── Evergreen ─────────────────────────────────────────────────────────────────

impl SessionDb {
    pub fn save_evergreen_chunk(
        &self,
        span_start_id: i64,
        span_end_id: i64,
        summary: &str,
        vital_refs: Option<&str>,
    ) -> Result<i64> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO evergreen_chunks
             (span_start_id, span_end_id, summary, vital_refs, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![span_start_id, span_end_id, summary, vital_refs, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn load_evergreen_chunks(&self) -> Result<Vec<EvergreenChunkRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, span_start_id, span_end_id, summary, vital_refs, created_at
             FROM evergreen_chunks ORDER BY span_start_id ASC",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok(EvergreenChunkRow {
                    id: row.get(0)?,
                    span_start_id: row.get(1)?,
                    span_end_id: row.get(2)?,
                    summary: row.get(3)?,
                    vital_refs: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Count uncompressed messages not yet covered by an evergreen chunk.
    /// Used by the compressor to decide whether a pass is worthwhile.
    pub fn uncompressed_message_count(&self) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE is_evergreen_compressed = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }
}

// ── JSONL (import / export) ───────────────────────────────────────────────────

impl SessionDb {
    pub fn save_jsonl_turn(&self, message_id: Option<i64>, jsonl_line: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO jsonl_turns (message_id, jsonl_line, created_at)
             VALUES (?1, ?2, ?3)",
            params![message_id, jsonl_line, now],
        )?;
        Ok(())
    }

    pub fn load_jsonl_turns(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT jsonl_line FROM jsonl_turns ORDER BY id ASC")?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> SessionDb {
        SessionDb::open_in_memory().unwrap()
    }

    #[test]
    fn test_message_round_trip() {
        let db = open();
        let id = db
            .save_message("user", "hello", "2026-04-11T10:00:00Z", None)
            .unwrap();
        assert_eq!(id, 1);
        let msgs = db.load_recent_messages(10).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "hello");
    }

    #[test]
    fn test_meta_round_trip() {
        let db = open();
        db.set_meta("model", "claude-sonnet-4-6").unwrap();
        assert_eq!(
            db.get_meta("model").unwrap(),
            Some("claude-sonnet-4-6".into())
        );
        assert_eq!(db.get_meta("missing").unwrap(), None);
    }

    #[test]
    fn test_evergreen_compression() {
        let db = open();
        let id1 = db
            .save_message("user", "msg1", "2026-04-11T10:00:00Z", None)
            .unwrap();
        let id2 = db
            .save_message("assistant", "msg2", "2026-04-11T10:00:01Z", Some(100))
            .unwrap();
        db.mark_messages_compressed(id1, id2).unwrap();
        db.save_evergreen_chunk(id1, id2, "Summary of msgs 1-2", None)
            .unwrap();

        let chunks = db.load_evergreen_chunks().unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].summary, "Summary of msgs 1-2");

        // Compressed messages excluded from default load
        let visible = db.load_recent_messages(10).unwrap();
        assert!(visible.is_empty());

        // But retrievable directly
        let raw = db.get_message(id1).unwrap();
        assert!(raw.is_some());
    }

    #[test]
    fn test_tool_call_round_trip() {
        let db = open();
        let msg_id = db
            .save_message("assistant", "", "2026-04-11T10:00:00Z", None)
            .unwrap();
        db.save_tool_call(
            msg_id,
            "Bash",
            r#"{"cmd":"ls"}"#,
            Some("file.rs"),
            false,
            Some(42),
        )
        .unwrap();
        let calls = db.load_tool_calls_for_message(msg_id).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool_name, "Bash");
        assert_eq!(calls[0].duration_ms, Some(42));
    }
}
