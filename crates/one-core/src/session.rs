use std::path::PathBuf;

use crate::conversation::Conversation;
use crate::provider::ModelConfig;

/// A session represents one AI conversation bound to a project directory.
/// Multiple sessions can be active simultaneously (the "multi-project" feature).
#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub project_path: String,
    pub project_name: String,
    pub model_config: ModelConfig,
    pub conversation: Conversation,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub cost_usd: f64,
    /// Cumulative token usage for this session.
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    /// Name of the tool currently being executed (if any).
    pub active_tool: Option<String>,
    /// Effort level for this session (None = auto/model default).
    /// Valid: "low", "medium", "high", "max"
    pub effort: Option<String>,
    /// Current working directory — starts as project_path, updated by cd commands.
    pub cwd: String,
    /// Path to this session's SQLite database file (`~/.one/{project}/{session}/session.db`).
    /// Empty until wired up by `with_storage_info()`.
    pub db_path: PathBuf,
    /// 6-char lowercase hex identifier — printed on exit, used with `--session <hash>`.
    /// Empty until wired up by `with_storage_info()`.
    pub session_hash: String,
    /// Git branch active when this session was created.
    /// Empty until wired up by `with_storage_info()`.
    pub branch: String,
    /// Debug event log — timestamped messages from background subsystems.
    /// Not persisted; interleaved with turns in the TUI when debug mode is on.
    pub debug_events: Vec<(chrono::DateTime<chrono::Utc>, String)>,
}

impl Session {
    pub fn new(project_path: String, model_config: ModelConfig) -> Self {
        let project_name = std::path::Path::new(&project_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        let cwd = project_path.clone();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            project_path,
            project_name,
            model_config,
            conversation: Conversation::default(),
            created_at: chrono::Utc::now(),
            cost_usd: 0.0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            active_tool: None,
            effort: None,
            cwd,
            db_path: PathBuf::new(),
            session_hash: String::new(),
            branch: String::new(),
            debug_events: Vec::new(),
        }
    }

    /// Attach filesystem storage info produced by `StoragePaths` (in `one-db`).
    /// Kept as plain-value args to avoid a `one-core → one-db` circular dependency.
    pub fn with_storage_info(
        mut self,
        db_path: PathBuf,
        session_hash: String,
        branch: String,
    ) -> Self {
        self.db_path = db_path;
        self.session_hash = session_hash;
        self.branch = branch;
        self
    }

    /// Record token usage from an API response and estimate cost.
    pub fn record_usage(&mut self, input_tokens: u32, output_tokens: u32) {
        self.total_input_tokens += input_tokens as u64;
        self.total_output_tokens += output_tokens as u64;
        // Estimate cost based on model (rough per-token pricing)
        let (input_price, output_price) = token_prices(&self.model_config.model);
        self.cost_usd +=
            (input_tokens as f64 * input_price + output_tokens as f64 * output_price) / 1_000_000.0;
    }
}

/// Per-million-token pricing for common models.
fn token_prices(model: &str) -> (f64, f64) {
    let lower = model.to_lowercase();
    if lower.contains("opus") {
        (15.0, 75.0) // $15/$75 per MTok
    } else if lower.contains("sonnet") {
        (3.0, 15.0) // $3/$15 per MTok
    } else if lower.contains("haiku") {
        (0.80, 4.0) // $0.80/$4 per MTok
    } else if lower.contains("gpt-4o") {
        (2.50, 10.0)
    } else if lower.contains("gpt-4") {
        (10.0, 30.0)
    } else {
        (1.0, 3.0) // Generic default
    }
}
