use anyhow::Result;
use rusqlite::{Connection, params};

pub struct Database {
    conn: Connection,
}

/// A stored session record
pub struct SessionRecord {
    pub id: String,
    pub project_path: String,
    pub project_name: String,
    pub model_provider: String,
    pub model_name: String,
    pub created_at: String,
    pub cost_usd: f64,
}

/// A stored message record
pub struct MessageRecord {
    pub role: String,
    pub content: String,
    pub created_at: String,
}

impl Database {
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Default database path: ~/.one/one.db
    pub fn default_path() -> String {
        dirs_next::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".one")
            .join("one.db")
            .to_string_lossy()
            .to_string()
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                project_path TEXT NOT NULL,
                project_name TEXT NOT NULL,
                model_provider TEXT NOT NULL,
                model_name TEXT NOT NULL,
                created_at TEXT NOT NULL,
                cost_usd REAL DEFAULT 0.0
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id),
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS notifications (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                url TEXT,
                read INTEGER DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session
                ON messages(session_id);
            CREATE INDEX IF NOT EXISTS idx_notifications_read
                ON notifications(read);
            ",
        )?;
        Ok(())
    }

    // --- Session operations ---

    pub fn save_session(&self, session: &SessionRecord) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO sessions (id, project_path, project_name, model_provider, model_name, created_at, cost_usd)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                session.id,
                session.project_path,
                session.project_name,
                session.model_provider,
                session.model_name,
                session.created_at,
                session.cost_usd,
            ],
        )?;
        Ok(())
    }

    /// Find the most recent session for a project path.
    pub fn find_session_by_project(&self, project_path: &str) -> Result<Option<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_path, project_name, model_provider, model_name, created_at, cost_usd
             FROM sessions WHERE project_path = ?1 ORDER BY created_at DESC LIMIT 1",
        )?;

        let result = stmt.query_row(params![project_path], |row| {
            Ok(SessionRecord {
                id: row.get(0)?,
                project_path: row.get(1)?,
                project_name: row.get(2)?,
                model_provider: row.get(3)?,
                model_name: row.get(4)?,
                created_at: row.get(5)?,
                cost_usd: row.get(6)?,
            })
        });

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // --- Message operations ---

    pub fn save_message(
        &self,
        session_id: &str,
        role: &str,
        content: &str,
        created_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO messages (session_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![session_id, role, content, created_at],
        )?;
        Ok(())
    }

    /// Load all messages for a session, ordered chronologically.
    pub fn load_messages(&self, session_id: &str) -> Result<Vec<MessageRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT role, content, created_at
             FROM messages WHERE session_id = ?1 ORDER BY id ASC",
        )?;

        let records = stmt
            .query_map(params![session_id], |row| {
                Ok(MessageRecord {
                    role: row.get(0)?,
                    content: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Count messages in a session.
    pub fn message_count(&self, session_id: &str) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    // --- Notification operations ---

    pub fn save_notification(
        &self,
        source: &str,
        title: &str,
        body: &str,
        url: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO notifications (source, title, body, url, created_at)
             VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            params![source, title, body, url],
        )?;
        Ok(())
    }

    pub fn unread_notification_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// List recent sessions across all projects.
    pub fn recent_sessions(&self, limit: usize) -> Result<Vec<SessionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_path, project_name, model_provider, model_name, created_at, cost_usd
             FROM sessions ORDER BY created_at DESC LIMIT ?1",
        )?;

        let records = stmt
            .query_map(params![limit as i64], |row| {
                Ok(SessionRecord {
                    id: row.get(0)?,
                    project_path: row.get(1)?,
                    project_name: row.get(2)?,
                    model_provider: row.get(3)?,
                    model_name: row.get(4)?,
                    created_at: row.get(5)?,
                    cost_usd: row.get(6)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(records)
    }
}
