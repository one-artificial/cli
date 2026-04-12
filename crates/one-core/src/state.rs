use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::AppConfig;
use crate::event::Notification;
use crate::session::Session;

/// Global application state shared across all subsystems.
/// Wrapped in Arc<RwLock<>> for safe concurrent access.
pub type SharedState = Arc<RwLock<AppState>>;

pub fn new_shared_state() -> SharedState {
    Arc::new(RwLock::new(AppState::default()))
}

#[derive(Debug, Clone)]
pub struct AppState {
    /// All active sessions, keyed by session ID
    pub sessions: HashMap<String, Session>,
    /// Which session is currently focused in the TUI
    pub active_session_id: Option<String>,
    /// Unread notifications from integrations
    pub notifications: Vec<Notification>,
    /// Application configuration
    pub config: AppConfig,
    /// Whether the app is shutting down
    pub quitting: bool,
    /// Plan mode: tools are described but not executed
    pub plan_mode: bool,
    /// Debug mode: show background subsystem activity as muted lines in chat
    pub debug_mode: bool,
    /// Background system toggles — reducing token usage when not needed.
    /// Defaults: evergreen true (built), others false (not yet built).
    pub evergreen_enabled: bool,
    pub chronicle_enabled: bool,  // cross-session synthesis
    pub prelude_enabled: bool,    // speculative pre-computation
    pub calibrate_enabled: bool,  // skill improvement
    pub palimpsest_enabled: bool, // living docs
    /// Session-scoped task manager
    pub tasks: crate::tasks::TaskManager,
    /// Pending permission prompt: the query engine is waiting for user input.
    /// Wrapped in Arc<Mutex<>> because oneshot::Sender isn't Clone.
    pub pending_permission:
        std::sync::Arc<std::sync::Mutex<Option<(String, crate::event::PermissionResponseSender)>>>,
    /// Pending user question: the AI is waiting for the user to answer.
    pub pending_question:
        std::sync::Arc<std::sync::Mutex<Option<crate::event::UserQuestionSender>>>,
    /// Session-scoped cron scheduler
    pub cron: crate::cron::CronScheduler,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sessions: Default::default(),
            active_session_id: Default::default(),
            notifications: Default::default(),
            config: Default::default(),
            quitting: false,
            plan_mode: false,
            debug_mode: false,
            evergreen_enabled: true, // on by default — background compression is low-cost
            chronicle_enabled: false,
            prelude_enabled: false,
            calibrate_enabled: false,
            palimpsest_enabled: false,
            tasks: Default::default(),
            pending_permission: Default::default(),
            pending_question: Default::default(),
            cron: Default::default(),
        }
    }
}

impl AppState {
    pub fn active_session(&self) -> Option<&Session> {
        self.active_session_id
            .as_ref()
            .and_then(|id| self.sessions.get(id))
    }

    pub fn active_session_mut(&mut self) -> Option<&mut Session> {
        let id = self.active_session_id.clone()?;
        self.sessions.get_mut(&id)
    }

    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }
}
