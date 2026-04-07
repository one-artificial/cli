use std::path::Path;

use anyhow::Result;

use crate::conversation::{Conversation, ConversationTurn, TurnRole};

/// A discovered session from another CLI tool.
#[derive(Debug)]
pub struct ImportedSession {
    pub source: SessionSource,
    pub session_id: String,
    pub project_path: String,
    pub conversation: Conversation,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy)]
pub enum SessionSource {
    ClaudeCode,
    Codex,
}

impl std::fmt::Display for SessionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionSource::ClaudeCode => write!(f, "Claude Code"),
            SessionSource::Codex => write!(f, "Codex"),
        }
    }
}

/// Discover all Claude Code sessions from ~/.claude/projects/
pub fn discover_claude_code_sessions() -> Result<Vec<ImportedSession>> {
    let claude_dir = dirs_next::home_dir()
        .unwrap_or_default()
        .join(".claude")
        .join("projects");

    if !claude_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();

    for project_entry in std::fs::read_dir(&claude_dir)?.flatten() {
        if !project_entry.file_type()?.is_dir() {
            continue;
        }

        let project_dir_name = project_entry.file_name().to_string_lossy().to_string();
        // Convert sanitized path back: -Users-me-Projects-repo → /Users/me/Projects/repo
        let project_path = project_dir_name.replacen('-', "/", 1).replace('-', "/");

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

            match parse_claude_code_session(&path, &session_id, &project_path) {
                Ok(Some(session)) => sessions.push(session),
                Ok(None) => {} // Empty session
                Err(e) => {
                    tracing::debug!("Failed to parse session {session_id}: {e}");
                }
            }
        }
    }

    // Sort by timestamp, newest first
    sessions.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(sessions)
}

/// Parse a single Claude Code JSONL session file.
fn parse_claude_code_session(
    path: &Path,
    session_id: &str,
    project_path: &str,
) -> Result<Option<ImportedSession>> {
    let content = std::fs::read_to_string(path)?;
    let mut conversation = Conversation::default();
    let mut first_timestamp = None;

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

        if first_timestamp.is_none() {
            first_timestamp = Some(timestamp);
        }

        match msg_type {
            "user" => {
                let content = extract_content(&entry["message"]["content"]);
                if !content.is_empty() {
                    conversation.turns.push(ConversationTurn {
                        role: TurnRole::User,
                        content,
                        timestamp,
                        tool_calls: Vec::new(),
                        is_streaming: false,
                        tokens_used: None,
                    });
                }
            }
            "assistant" => {
                let content = extract_content(&entry["message"]["content"]);
                if !content.is_empty() {
                    conversation.turns.push(ConversationTurn {
                        role: TurnRole::Assistant,
                        content,
                        timestamp,
                        tool_calls: Vec::new(),
                        is_streaming: false,
                        tokens_used: None,
                    });
                }
            }
            _ => {} // Skip permission-mode, file-history-snapshot, etc.
        }
    }

    if conversation.turns.is_empty() {
        return Ok(None);
    }

    Ok(Some(ImportedSession {
        source: SessionSource::ClaudeCode,
        session_id: session_id.to_string(),
        project_path: project_path.to_string(),
        conversation,
        timestamp: first_timestamp.unwrap_or_else(chrono::Utc::now),
    }))
}

/// Extract text content from Claude Code's message content format.
/// Content can be a string or an array of content blocks.
fn extract_content(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => {
            let mut text_parts = Vec::new();
            for block in blocks {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            text_parts.push(t.to_string());
                        }
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("tool");
                        text_parts.push(format!("[tool: {name}]"));
                    }
                    Some("tool_result") => {
                        if let Some(content) = block["content"].as_str() {
                            let preview = if content.len() > 200 {
                                format!("{}...", &content[..200])
                            } else {
                                content.to_string()
                            };
                            text_parts.push(format!("[result: {preview}]"));
                        }
                    }
                    // Skip thinking blocks, signatures, etc.
                    _ => {}
                }
            }
            text_parts.join("\n")
        }
        _ => String::new(),
    }
}

/// List available sessions for import with summary info.
pub fn list_importable_sessions() -> Result<Vec<SessionSummary>> {
    let mut summaries = Vec::new();

    for session in discover_claude_code_sessions()? {
        let first_msg = session
            .conversation
            .turns
            .iter()
            .find(|t| t.role == TurnRole::User)
            .map(|t| {
                if t.content.len() > 80 {
                    format!("{}...", &t.content[..80])
                } else {
                    t.content.clone()
                }
            })
            .unwrap_or_else(|| "(no user messages)".to_string());

        summaries.push(SessionSummary {
            source: session.source,
            session_id: session.session_id,
            project_path: session.project_path,
            turns: session.conversation.turns.len(),
            first_message: first_msg,
            timestamp: session.timestamp,
        });
    }

    Ok(summaries)
}

#[derive(Debug)]
pub struct SessionSummary {
    pub source: SessionSource,
    pub session_id: String,
    pub project_path: String,
    pub turns: usize,
    pub first_message: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
