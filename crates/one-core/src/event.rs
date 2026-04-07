use tokio::sync::broadcast;

/// Central event bus for cross-crate communication.
///
/// All subsystems (TUI, AI, integrations, tools) communicate through
/// typed events on this bus rather than direct dependencies.
#[derive(Debug, Clone)]
pub enum Event {
    /// User submitted a message in the active session
    UserMessage {
        session_id: String,
        content: String,
    },
    /// AI response chunk streamed back
    AiResponseChunk {
        session_id: String,
        content: String,
        done: bool,
    },
    /// AI requested a tool call
    ToolRequest {
        session_id: String,
        tool_name: String,
        input: serde_json::Value,
        call_id: String,
    },
    /// Tool execution completed
    ToolResult {
        session_id: String,
        call_id: String,
        output: String,
        is_error: bool,
    },
    /// Permission prompt: query engine is waiting for user approval.
    /// The TUI should display a prompt and send the response via PermissionResponseChannel.
    PermissionPrompt {
        session_id: String,
        call_id: String,
        tool_name: String,
        input_summary: String,
        warning: Option<String>,
    },
    /// Tool use was denied by the permission system
    ToolDenied {
        session_id: String,
        call_id: String,
        tool_name: String,
        reason: String,
        warning: Option<String>,
    },
    /// External notification arrived (Slack, GitHub, Asana, Notion)
    Notification(Notification),
    /// Session lifecycle
    SessionCreated {
        session_id: String,
        project: String,
    },
    SessionClosed {
        session_id: String,
    },
    SessionSwitched {
        session_id: String,
    },
    /// AI is asking the user a question via AskUserQuestion tool.
    /// The TUI should display the question and send the answer back.
    UserQuestion {
        session_id: String,
        call_id: String,
        question: String,
        options: Vec<String>,
    },
    /// Application shutdown requested
    Quit,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub source: NotificationSource,
    pub title: String,
    pub body: String,
    pub url: Option<String>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationSource {
    Slack,
    GitHub,
    Asana,
    Notion,
}

/// User's response to a permission prompt.
#[derive(Debug, Clone)]
pub enum PermissionResponse {
    /// Allow this one tool call.
    Allow,
    /// Deny this one tool call.
    Deny,
    /// Always allow this tool (add session rule).
    AlwaysAllow,
    /// Always deny this tool (add session rule).
    AlwaysDeny,
}

/// Channel for sending permission responses from TUI back to the query engine.
/// The query engine creates a oneshot sender, wraps it in this struct, and
/// stores it in shared state. The TUI reads it and sends the response.
pub type PermissionResponseSender = tokio::sync::oneshot::Sender<PermissionResponse>;
pub type PermissionResponseReceiver = tokio::sync::oneshot::Receiver<PermissionResponse>;

/// Channel for sending user question answers from TUI back to query engine.
pub type UserQuestionSender = tokio::sync::oneshot::Sender<String>;
pub type UserQuestionReceiver = tokio::sync::oneshot::Receiver<String>;

/// Event bus backed by a tokio broadcast channel.
pub struct EventBus {
    sender: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    pub fn sender(&self) -> broadcast::Sender<Event> {
        self.sender.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new(256)
    }
}
