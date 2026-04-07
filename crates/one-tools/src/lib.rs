pub mod agent;
pub mod ask_user;
pub mod bash;
pub mod cron_tools;
pub mod file_edit;
pub mod file_read;
pub mod file_write;
pub mod glob;
pub mod grep;
pub mod mcp_resources;
pub mod notebook_edit;
pub mod plan_mode;
pub mod registry;
pub mod script_tool;
pub mod skill_tool;
pub mod sleep;
pub mod todo_write;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod worktree_tools;

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

/// Every tool in One implements this trait.
/// Uses boxed futures for dyn-compatibility — the tool registry stores
/// `Box<dyn Tool>` so we need dynamic dispatch.
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;

    fn execute(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>>;

    fn is_read_only(&self) -> bool {
        false
    }

    /// Whether this tool should be deferred (not sent in initial API request).
    /// Deferred tools are discoverable via ToolSearch. Override to return true
    /// for non-essential tools to reduce prompt token usage.
    fn should_defer(&self) -> bool {
        false
    }

    /// Short capability phrase (3-10 words) used by ToolSearch for scoring.
    /// Not shown to the user — purely for search relevance.
    fn search_hint(&self) -> Option<&str> {
        None
    }

    /// Whether this tool performs destructive operations (rm -rf, DROP TABLE, etc.)
    fn is_destructive(&self) -> bool {
        false
    }

    /// Extended prompt text describing how the AI should use this tool.
    /// Included in the system prompt for always-loaded tools. ToolSearch
    /// returns this for deferred tools when they're loaded on demand.
    fn prompt(&self) -> Option<&str> {
        None
    }
}

/// Context passed to every tool execution
pub struct ToolContext {
    pub working_dir: String,
    pub session_id: String,
    /// Files that have been read in this session (for Edit safety checks).
    /// Shared across tool calls via Arc<Mutex<>>.
    pub read_files: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl ToolContext {
    /// Create a new ToolContext with the given working directory and session ID.
    pub fn new(working_dir: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            working_dir: working_dir.into(),
            session_id: session_id.into(),
            read_files: std::sync::Arc::new(
                std::sync::Mutex::new(std::collections::HashSet::new()),
            ),
        }
    }
}

/// Result of a tool execution
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
}

impl ToolResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
        }
    }

    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
        }
    }
}

/// Create a registry with all built-in tools registered.
/// ToolSearch is automatically included with a snapshot of all deferred tools.
pub fn create_default_registry() -> registry::ToolRegistry {
    // Phase 1: Register all tools
    let mut reg = registry::ToolRegistry::new();
    reg.register(Box::new(agent::AgentTool));
    reg.register(Box::new(skill_tool::SkillTool));
    reg.register(Box::new(todo_write::TodoWriteTool));
    reg.register(Box::new(file_read::FileReadTool));
    reg.register(Box::new(file_write::FileWriteTool));
    reg.register(Box::new(bash::BashTool));
    reg.register(Box::new(grep::GrepTool));
    reg.register(Box::new(glob::GlobTool));
    reg.register(Box::new(file_edit::FileEditTool));
    reg.register(Box::new(web_fetch::WebFetchTool));
    reg.register(Box::new(web_search::WebSearchTool));
    reg.register(Box::new(ask_user::AskUserQuestionTool));
    reg.register(Box::new(sleep::SleepTool));
    reg.register(Box::new(plan_mode::EnterPlanModeTool));
    reg.register(Box::new(plan_mode::ExitPlanModeTool));
    reg.register(Box::new(mcp_resources::ListMcpResourcesTool));
    reg.register(Box::new(mcp_resources::ReadMcpResourceTool));
    reg.register(Box::new(cron_tools::CronCreateTool));
    reg.register(Box::new(cron_tools::CronDeleteTool));
    reg.register(Box::new(cron_tools::CronListTool));
    reg.register(Box::new(notebook_edit::NotebookEditTool));
    reg.register(Box::new(worktree_tools::EnterWorktreeTool));
    reg.register(Box::new(worktree_tools::ExitWorktreeTool));

    // Phase 2: Load plugin tools from ~/.one/plugins/
    if let Some(home) = std::env::var_os("HOME") {
        let plugins_dir = std::path::PathBuf::from(home).join(".one").join("plugins");
        if plugins_dir.is_dir() {
            let plugin_registry = one_core::plugin::PluginRegistry::discover();
            for plugin in plugin_registry.all() {
                if !plugin.enabled {
                    continue;
                }
                let plugin_dir = plugins_dir.join(&plugin.manifest.name);
                let entrypoint = match &plugin.manifest.plugin_type {
                    one_core::plugin::PluginType::Script { entrypoint } => entrypoint.clone(),
                    _ => continue,
                };
                for tool_name in &plugin.manifest.tools {
                    let desc = format!("[Plugin: {}] {}", plugin.manifest.name, tool_name);
                    reg.register(Box::new(script_tool::ScriptTool::new(
                        tool_name.clone(),
                        desc,
                        entrypoint.clone(),
                        plugin_dir.to_string_lossy().to_string(),
                    )));
                }
            }
        }
    }

    // Phase 3: Snapshot deferred tool info, then register ToolSearch
    let deferred_info = reg.collect_deferred_info();
    reg.register(Box::new(tool_search::ToolSearchTool::new(deferred_info)));

    reg
}
