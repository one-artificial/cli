use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single turn in the conversation, richer than the raw API Message.
/// This is what the TUI renders — it includes metadata, tool calls,
/// streaming state, and timing information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: TurnRole,
    pub content: String,
    pub timestamp: DateTime<Utc>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub is_streaming: bool,
    pub tokens_used: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnRole {
    User,
    Assistant,
    System,
    ToolResult,
}

/// Record of a tool call and its result, for display in the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub id: String,
    pub tool_name: String,
    pub input_summary: String,
    pub output: Option<String>,
    pub is_error: bool,
    pub duration_ms: Option<u64>,
    /// Index into the processing-dot animation pool for this specific tool call.
    /// Not persisted — restored sessions always have output set so the style is unused.
    #[serde(skip)]
    pub dot_style: usize,
}

/// The full conversation for a session, with helper methods.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    pub turns: Vec<ConversationTurn>,
}

impl Conversation {
    pub fn push_user_message(&mut self, content: String) {
        self.turns.push(ConversationTurn {
            role: TurnRole::User,
            content,
            timestamp: Utc::now(),
            tool_calls: Vec::new(),
            is_streaming: false,
            tokens_used: None,
        });
    }

    pub fn start_assistant_response(&mut self) {
        self.turns.push(ConversationTurn {
            role: TurnRole::Assistant,
            content: String::new(),
            timestamp: Utc::now(),
            tool_calls: Vec::new(),
            is_streaming: true,
            tokens_used: None,
        });
    }

    pub fn append_to_current(&mut self, chunk: &str) {
        if let Some(last) = self.turns.last_mut()
            && last.role == TurnRole::Assistant
            && last.is_streaming
        {
            last.content.push_str(chunk);
        }
    }

    pub fn finish_current(&mut self, tokens: Option<u32>) {
        if let Some(last) = self.turns.last_mut() {
            last.is_streaming = false;
            last.tokens_used = tokens;
        }
    }

    pub fn last_is_streaming(&self) -> bool {
        self.turns.last().is_some_and(|t| t.is_streaming)
    }
}
