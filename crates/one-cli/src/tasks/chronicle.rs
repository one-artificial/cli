//! Chronicle — cross-session synthesis into cold-tier landmark records.
//!
//! Fires after Evergreen compression passes, gated by:
//! 1. `state.chronicle_enabled` toggle
//! 2. Time gate: ≥ 12 hours since last run per project
//! 3. Volume gate: ≥ 3 sessions with new Evergreen chunks since last run
//! 4. File-backed process lock: prevents concurrent runs across One instances
//!
//! Reads warm/hot evergreen chunks from all session DBs for the active project,
//! synthesises an 80–120 word cold-tier landmark record, and saves it to
//! `~/.one/chronicle.db` (shared across all sessions for that project).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use one_core::event::Event;
use one_core::provider::{AiProvider, Message, ModelConfig, Role};
use one_core::state::SharedState;
use one_db::{SessionDb, session_db::EvergreenChunkRow};
use rusqlite::{Connection, params};
use tokio::sync::broadcast;

const MIN_HOURS: i64 = 12;
const MIN_SESSIONS: usize = 3;

// ── Public entry point ────────────────────────────────────────────────────────

pub fn spawn(
    state: SharedState,
    event_tx: broadcast::Sender<Event>,
    provider: Arc<dyn AiProvider>,
    model_config: ModelConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run(state, event_tx, provider, model_config))
}

// ── Task loop ─────────────────────────────────────────────────────────────────

async fn run(
    state: SharedState,
    event_tx: broadcast::Sender<Event>,
    provider: Arc<dyn AiProvider>,
    model_config: ModelConfig,
) {
    let mut rx = event_tx.subscribe();
    let mut touched: HashSet<String> = HashSet::new(); // session IDs with new chunks

    let compress_config = ModelConfig {
        max_tokens: 300,
        temperature: Some(0.2),
        budget_tokens: None,
        ..model_config
    };

    loop {
        let evt = match rx.recv().await {
            Ok(e) => e,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => break,
        };

        match evt {
            Event::EvergreenCompressed { ref session_id, .. } => {
                touched.insert(session_id.clone());

                let (enabled, project_path) = {
                    let s = state.read().await;
                    let path = s
                        .sessions
                        .get(session_id)
                        .map(|s| s.project_path.clone())
                        .unwrap_or_default();
                    (s.chronicle_enabled, path)
                };
                if !enabled || project_path.is_empty() {
                    continue;
                }

                if touched.len() < MIN_SESSIONS {
                    continue;
                }

                if !time_gate_ok(&project_path) {
                    continue;
                }

                let lock_path = chronicle_dir().join("chronicle.lock");
                if !try_acquire_lock(&lock_path) {
                    continue;
                }

                let _ = event_tx.send(Event::DebugLog {
                    session_id: session_id.clone(),
                    message: format!(
                        "chronicle → synthesising cold-tier from {} sessions",
                        touched.len()
                    ),
                });

                match synthesise(
                    &project_path,
                    session_id,
                    &provider,
                    &compress_config,
                    &event_tx,
                    &state,
                )
                .await
                {
                    Ok(()) => {
                        touched.clear();
                        update_last_run(&project_path);
                    }
                    Err(e) => {
                        tracing::warn!("chronicle: synthesis failed: {e}");
                        let _ = event_tx.send(Event::DebugLog {
                            session_id: session_id.clone(),
                            message: format!("chronicle: failed — {e}"),
                        });
                    }
                }

                release_lock(&lock_path);
            }
            Event::Quit => break,
            _ => {}
        }
    }
}

// ── Synthesis ─────────────────────────────────────────────────────────────────

async fn synthesise(
    project_path: &str,
    trigger_session_id: &str,
    provider: &Arc<dyn AiProvider>,
    config: &ModelConfig,
    event_tx: &broadcast::Sender<Event>,
    state: &SharedState,
) -> Result<()> {
    // Collect warm (and hot) chunks from all session DBs for this project.
    let dbs = find_session_dbs(project_path);
    let mut all_chunks: Vec<EvergreenChunkRow> = Vec::new();

    let dbs_found = dbs.len();
    for db_path in dbs {
        if let Ok(db) = SessionDb::open(&db_path)
            && let Ok(chunks) = db.load_evergreen_chunks()
        {
            // Prefer warm chunks; fall back to hot if no warm available.
            let warm: Vec<_> = chunks
                .iter()
                .filter(|c| c.tier == "warm")
                .cloned()
                .collect();
            if warm.is_empty() {
                all_chunks.extend(chunks.into_iter().filter(|c| c.tier == "hot").take(2));
            } else {
                all_chunks.extend(warm);
            }
        }
    }

    if all_chunks.is_empty() {
        return Err(anyhow::anyhow!(
            "no chunks found across {} session DBs",
            dbs_found
        ));
    }

    let combined = all_chunks
        .iter()
        .map(|c| c.summary.as_str())
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let messages = vec![Message {
        role: Role::User,
        content: format!("{}{}", one_core::evergreen::COLD_COMPRESS_PROMPT, combined),
    }];

    let response = provider.send_message(&messages, config).await?;
    let summary = response.content.trim().to_string();
    if summary.is_empty() {
        return Err(anyhow::anyhow!("model returned empty cold summary"));
    }

    // Parse sections from the cold summary.
    let parsed = one_core::evergreen::parse_sections(&summary);

    // Save to chronicle DB.
    let session_ids: Vec<&str> = vec![trigger_session_id]; // simplified
    save_cold_chunk(project_path, &summary, &parsed, &session_ids)?;

    let _ = event_tx.send(Event::DebugLog {
        session_id: trigger_session_id.to_string(),
        message: format!(
            "chronicle ← cold-tier written ({} artefacts)",
            parsed.artefacts.len()
        ),
    });

    // Update active session's recall context to include cold tier.
    {
        let mut s = state.write().await;
        if let Some(session) = s.sessions.get_mut(trigger_session_id) {
            // Append cold chunk to existing recall context.
            let cold_block = format!("\n--- COLD — landmark ---\n{summary}\n");
            match session.evergreen_context {
                Some(ref mut ctx) => ctx.push_str(&cold_block),
                None => {
                    session.evergreen_context = Some(format!(
                        "{}{cold_block}",
                        one_core::evergreen::RECALL_PREAMBLE
                    ))
                }
            }
        }
    }

    Ok(())
}

// ── Chronicle DB ──────────────────────────────────────────────────────────────

fn chronicle_dir() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".one")
}

struct ChronicleDb {
    conn: Connection,
}

impl ChronicleDb {
    fn open() -> Result<Self> {
        let path = chronicle_dir().join("chronicle.db");
        let conn = Connection::open(&path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS cold_chunks (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                project_path    TEXT NOT NULL,
                summary         TEXT NOT NULL,
                goal            TEXT,
                artefacts_json  TEXT NOT NULL DEFAULT '[]',
                sharp_edges_json TEXT NOT NULL DEFAULT '[]',
                recall_note     TEXT,
                sessions_json   TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chronicle_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    fn last_run(&self, project_path: &str) -> Option<chrono::DateTime<chrono::Utc>> {
        let key = format!("last_run_{}", one_db::slugify_path(project_path));
        self.conn
            .query_row(
                "SELECT value FROM chronicle_meta WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .ok()
            .and_then(|s| s.parse().ok())
    }

    fn set_last_run(&self, project_path: &str) {
        let key = format!("last_run_{}", one_db::slugify_path(project_path));
        let now = chrono::Utc::now().to_rfc3339();
        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO chronicle_meta (key, value) VALUES (?1, ?2)",
            params![key, now],
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn save_chunk(
        &self,
        project_path: &str,
        summary: &str,
        goal: Option<&str>,
        artefacts_json: &str,
        sharp_edges_json: &str,
        recall_note: Option<&str>,
        sessions_json: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO cold_chunks
             (project_path, summary, goal, artefacts_json, sharp_edges_json, recall_note, sessions_json, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                project_path,
                summary,
                goal,
                artefacts_json,
                sharp_edges_json,
                recall_note,
                sessions_json,
                now
            ],
        )?;
        Ok(())
    }
}

fn save_cold_chunk(
    project_path: &str,
    summary: &str,
    parsed: &one_core::evergreen::ParsedSections,
    session_ids: &[&str],
) -> Result<()> {
    let db = ChronicleDb::open()?;
    let artefacts_json = serde_json::to_string(&parsed.artefacts).unwrap_or_default();
    let sharp_edges_json = serde_json::to_string(&parsed.sharp_edges).unwrap_or_default();
    let sessions_json = serde_json::to_string(session_ids).unwrap_or_default();
    db.save_chunk(
        project_path,
        summary,
        parsed.goal.as_deref(),
        &artefacts_json,
        &sharp_edges_json,
        parsed.recall_note.as_deref(),
        &sessions_json,
    )
}

// ── Gates and locks ───────────────────────────────────────────────────────────

fn time_gate_ok(project_path: &str) -> bool {
    let Ok(db) = ChronicleDb::open() else {
        return true; // No DB yet — first run is always ok
    };
    match db.last_run(project_path) {
        None => true,
        Some(last) => {
            let hours = chrono::Utc::now().signed_duration_since(last).num_hours();
            hours >= MIN_HOURS
        }
    }
}

fn update_last_run(project_path: &str) {
    if let Ok(db) = ChronicleDb::open() {
        db.set_last_run(project_path);
    }
}

fn try_acquire_lock(lock_path: &PathBuf) -> bool {
    use std::io::Write;
    let Ok(mut f) = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(lock_path)
    else {
        return false;
    };
    let _ = write!(f, "{}", std::process::id());
    true
}

fn release_lock(lock_path: &PathBuf) {
    let _ = std::fs::remove_file(lock_path);
}

// ── Session DB discovery ──────────────────────────────────────────────────────

fn find_session_dbs(project_path: &str) -> Vec<PathBuf> {
    let Ok(profile) = one_db::profile_dir() else {
        return vec![];
    };
    let project_slug = one_db::slugify_path(project_path);
    let project_dir = profile.join(project_slug);
    if !project_dir.exists() {
        return vec![];
    }
    let Ok(entries) = std::fs::read_dir(&project_dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.path().join("session.db"))
        .filter(|p| p.exists())
        .collect()
}
