use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::conversation::{Conversation, ConversationTurn, TurnRole};

/// Which storage backend a session uses.
/// One doesn't force its own format — it speaks each tool's native format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageBackend {
    /// Claude Code: ~/.claude/projects/{sanitized-path}/{session-id}.jsonl
    ClaudeCode { jsonl_path: PathBuf },
    /// Codex: TBD — will add when we discover the format
    Codex { session_path: PathBuf },
    /// One native: ~/.one/one.db (SQLite)
    Native,
}

impl StorageBackend {
    /// Detect storage backend from a session ID by searching known locations.
    pub fn detect(session_id: &str) -> Self {
        // Check Claude Code sessions
        if let Some(path) = find_claude_code_session(session_id) {
            return Self::ClaudeCode { jsonl_path: path };
        }

        // Check Codex sessions
        if let Some(path) = find_codex_session(session_id) {
            return Self::Codex { session_path: path };
        }

        // Default to native
        Self::Native
    }

    /// Load conversation from this backend.
    pub fn load(&self, _session_id: &str) -> Result<Conversation> {
        match self {
            Self::ClaudeCode { jsonl_path } => load_claude_code_session(jsonl_path),
            Self::Codex { session_path } => load_codex_session(session_path),
            Self::Native => Ok(Conversation::default()), // Loaded via DB elsewhere
        }
    }

    /// Append a turn to this backend's storage.
    pub fn append_turn(&self, _session_id: &str, turn: &ConversationTurn) -> Result<()> {
        match self {
            Self::ClaudeCode { jsonl_path } => append_claude_code_turn(jsonl_path, turn),
            Self::Codex { session_path } => append_codex_turn(session_path, turn),
            Self::Native => Ok(()), // Handled by DB persistence task
        }
    }
}

// --- Claude Code backend ---

fn claude_code_projects_dir() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_default()
        .join(".claude")
        .join("projects")
}

/// Find a Claude Code session JSONL file by session ID.
fn find_claude_code_session(session_id: &str) -> Option<PathBuf> {
    let projects_dir = claude_code_projects_dir();
    if !projects_dir.exists() {
        return None;
    }

    let filename = format!("{session_id}.jsonl");

    for project_entry in std::fs::read_dir(&projects_dir).ok()?.flatten() {
        if !project_entry.file_type().ok()?.is_dir() {
            continue;
        }
        let candidate = project_entry.path().join(&filename);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

/// List all Claude Code sessions across all projects.
pub fn list_claude_code_sessions() -> Result<Vec<SessionInfo>> {
    let projects_dir = claude_code_projects_dir();
    if !projects_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for project_entry in std::fs::read_dir(&projects_dir)?.flatten() {
        if !project_entry.file_type()?.is_dir() {
            continue;
        }

        let dir_name = project_entry.file_name().to_string_lossy().to_string();
        let project_path = dir_name.replacen('-', "/", 1).replace('-', "/");

        for file_entry in std::fs::read_dir(project_entry.path())?.flatten() {
            let path = file_entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            // Quick scan: count user/assistant lines and get first user message
            let (turns, first_msg, timestamp) = quick_scan_jsonl(&path);

            if turns > 0 {
                sessions.push(SessionInfo {
                    session_id,
                    project_path: project_path.clone(),
                    backend: "Claude Code".to_string(),
                    turns,
                    first_message: first_msg,
                    timestamp,
                });
            }
        }
    }

    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}

/// Quick scan a JSONL file for turn count and first user message.
fn quick_scan_jsonl(path: &Path) -> (usize, String, String) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (0, String::new(), String::new()),
    };

    let mut turns = 0;
    let mut first_msg = String::new();
    let mut timestamp = String::new();

    for line in content.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = entry["type"].as_str().unwrap_or("");

        if msg_type == "user" || msg_type == "assistant" {
            turns += 1;

            if timestamp.is_empty() {
                timestamp = entry["timestamp"].as_str().unwrap_or("").to_string();
            }

            if first_msg.is_empty() && msg_type == "user" {
                first_msg = extract_text_content(&entry["message"]["content"]);
                if first_msg.len() > 80 {
                    first_msg = format!("{}...", &first_msg[..80]);
                }
            }
        }
    }

    (turns, first_msg, timestamp)
}

/// Load a full conversation from a Claude Code JSONL file.
fn load_claude_code_session(path: &Path) -> Result<Conversation> {
    let content = std::fs::read_to_string(path)?;
    let mut conversation = Conversation::default();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let msg_type = entry["type"].as_str().unwrap_or("");
        let timestamp = entry["timestamp"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        let role = match msg_type {
            "user" => TurnRole::User,
            "assistant" => TurnRole::Assistant,
            _ => continue,
        };

        let content = extract_text_content(&entry["message"]["content"]);
        if content.is_empty() {
            continue;
        }

        conversation.turns.push(ConversationTurn {
            role,
            content,
            timestamp,
            tool_calls: Vec::new(),
            is_streaming: false,
            tokens_used: None,
        });
    }

    Ok(conversation)
}

/// Append a turn to a Claude Code JSONL file using the exact format
/// Claude Code expects (parentUuid chain, sessionId, cwd, version).
fn append_claude_code_turn(path: &Path, turn: &ConversationTurn) -> Result<()> {
    use std::io::Write;

    let msg_type = match turn.role {
        TurnRole::User => "user",
        TurnRole::Assistant => "assistant",
        _ => return Ok(()),
    };

    let uuid = uuid::Uuid::new_v4().to_string();
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Read last UUID from file for parent chain
    let parent_uuid = read_last_uuid(path);

    // Build entry matching Claude Code's TranscriptMessage format
    // Key order: parentUuid MUST be first
    let entry = format!(
        r#"{{"parentUuid":{},"isSidechain":false,"type":"{}","message":{{"role":"{}","content":{}}},"uuid":"{}","userType":"external","cwd":"{}","sessionId":"{}","version":"one-0.1.0","timestamp":"{}"}}"#,
        match &parent_uuid {
            Some(p) => format!("\"{}\"", p),
            None => "null".to_string(),
        },
        msg_type,
        msg_type,
        serde_json::to_string(&turn.content)?,
        uuid,
        cwd,
        session_id,
        turn.timestamp.to_rfc3339(),
    );

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    writeln!(file, "{}", entry)?;
    Ok(())
}

/// Read the last UUID from a JSONL file for parent chain linking.
fn read_last_uuid(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines().rev() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(uuid) = entry["uuid"].as_str()
        {
            return Some(uuid.to_string());
        }
    }
    None
}

/// Create a new Claude Code-compatible session JSONL file.
/// Returns the path to the created file.
pub fn create_claude_code_session(session_id: &str, project_path: &str) -> Result<PathBuf> {
    let sanitized = sanitize_path(project_path);
    let dir = claude_code_projects_dir().join(&sanitized);
    std::fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{session_id}.jsonl"));

    // Create empty file with restricted permissions
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)?;

    Ok(path)
}

/// Sanitize a project path for use as a directory name.
/// Replaces non-alphanumeric chars with hyphens.
fn sanitize_path(path: &str) -> String {
    let sanitized: String = path
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();

    if sanitized.len() <= 200 {
        sanitized
    } else {
        // Hash long paths
        use sha2::{Digest, Sha256};
        let hash = hex::encode(&Sha256::digest(path.as_bytes())[..8]);
        format!("{}-{}", &sanitized[..200], hash)
    }
}

/// Extract text from Claude Code's content format (string or array of blocks).
fn extract_text_content(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            let mut parts = Vec::new();
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("tool");
                        parts.push(format!("[tool: {name}]"));
                    }
                    _ => {} // Skip thinking, signatures, etc.
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

// --- Codex backend ---
// Codex stores sessions as JSONL at:
//   ~/.codex/sessions/YYYY/MM/DD/rollout-{timestamp}-{session-id}.jsonl
// Thread metadata in ~/.codex/state_5.sqlite (threads table)

fn codex_sessions_dir() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_default()
        .join(".codex")
        .join("sessions")
}

fn find_codex_session(session_id: &str) -> Option<PathBuf> {
    let sessions_dir = codex_sessions_dir();
    if !sessions_dir.exists() {
        return None;
    }

    // Walk year/month/day directories looking for the session ID in filenames
    for year in std::fs::read_dir(&sessions_dir).ok()?.flatten() {
        for month in std::fs::read_dir(year.path()).ok()?.flatten() {
            for day in std::fs::read_dir(month.path()).ok()?.flatten() {
                for file in std::fs::read_dir(day.path()).ok()?.flatten() {
                    let name = file.file_name().to_string_lossy().to_string();
                    if name.contains(session_id) && name.ends_with(".jsonl") {
                        return Some(file.path());
                    }
                }
            }
        }
    }

    None
}

fn load_codex_session(path: &Path) -> Result<Conversation> {
    let content = std::fs::read_to_string(path)?;
    let mut conversation = Conversation::default();

    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = entry["type"].as_str().unwrap_or("");

        // Codex uses "response_item" with payload.role = "user" | "assistant"
        if entry_type != "response_item" {
            continue;
        }

        let role_str = entry["payload"]["role"].as_str().unwrap_or("");
        let role = match role_str {
            "user" => TurnRole::User,
            "assistant" => TurnRole::Assistant,
            _ => continue,
        };

        let timestamp = entry["timestamp"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(chrono::Utc::now);

        // Content is an array of blocks with type "input_text" or "output_text" or "text"
        let content = if let Some(blocks) = entry["payload"]["content"].as_array() {
            blocks
                .iter()
                .filter_map(|b| {
                    b["text"]
                        .as_str()
                        .or_else(|| b["input_text"].as_str())
                        .map(|s| s.to_string())
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            continue;
        };

        if content.is_empty()
            || content.starts_with("<permissions")
            || content.starts_with("<environment_context")
        {
            continue; // Skip system/context messages
        }

        conversation.turns.push(ConversationTurn {
            role,
            content,
            timestamp,
            tool_calls: Vec::new(),
            is_streaming: false,
            tokens_used: None,
        });
    }

    Ok(conversation)
}

fn append_codex_turn(path: &Path, turn: &ConversationTurn) -> Result<()> {
    use std::io::Write;

    let role = match turn.role {
        TurnRole::User => "user",
        TurnRole::Assistant => "assistant",
        _ => return Ok(()),
    };

    let entry = serde_json::json!({
        "timestamp": turn.timestamp.to_rfc3339(),
        "type": "response_item",
        "payload": {
            "type": "message",
            "role": role,
            "content": [{"type": "input_text", "text": turn.content}],
        }
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;

    writeln!(file, "{}", serde_json::to_string(&entry)?)?;
    Ok(())
}

/// List Codex sessions from ~/.codex/state_5.sqlite
pub fn list_codex_sessions() -> Result<Vec<SessionInfo>> {
    let db_path = dirs_next::home_dir()
        .unwrap_or_default()
        .join(".codex")
        .join("state_5.sqlite");

    if !db_path.exists() {
        return Ok(Vec::new());
    }

    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;

    let mut stmt = conn.prepare(
        "SELECT id, cwd, tokens_used, first_user_message, created_at
         FROM threads WHERE has_user_event = 1 AND archived = 0
         ORDER BY created_at DESC LIMIT 20",
    )?;

    let sessions = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let cwd: String = row.get(1)?;
            let tokens: i64 = row.get(2)?;
            let first_msg: String = row.get(3)?;
            let created_at: i64 = row.get(4)?;

            Ok(SessionInfo {
                session_id: id,
                project_path: cwd,
                backend: "Codex".to_string(),
                turns: (tokens / 1000) as usize, // Approximate
                first_message: if first_msg.len() > 80 {
                    format!("{}...", &first_msg[..80])
                } else {
                    first_msg
                },
                timestamp: chrono::DateTime::from_timestamp(created_at, 0)
                    .unwrap_or_else(chrono::Utc::now)
                    .to_rfc3339(),
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(sessions)
}

// --- Session listing ---

#[derive(Debug)]
pub struct SessionInfo {
    pub session_id: String,
    pub project_path: String,
    pub backend: String,
    pub turns: usize,
    pub first_message: String,
    pub timestamp: String,
}
