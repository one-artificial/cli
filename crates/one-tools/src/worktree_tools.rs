//! EnterWorktree / ExitWorktree tools — user-requested worktree isolation.
//!
//! These tools let the user explicitly work in a git worktree for isolation.
//! They should be deferred (loaded via ToolSearch when needed).
//! Execution is intercepted by the QueryEngine.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// EnterWorktree — create an isolated git worktree for the session.
pub struct EnterWorktreeTool;

impl Tool for EnterWorktreeTool {
    fn name(&self) -> &str {
        "EnterWorktree"
    }

    fn description(&self) -> &str {
        "Use this tool ONLY when the user explicitly asks to work in a worktree. \
         Creates an isolated copy of the repo via git worktree. Changes can be \
         committed independently without affecting the main working directory."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name for the worktree branch (e.g., 'feature-xyz')"
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    fn execute(
        &self,
        input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        let name = input["name"].as_str().unwrap_or("worktree").to_string();
        Box::pin(async move {
            // Fallback — intercepted by QueryEngine
            Ok(ToolResult::error(format!(
                "EnterWorktree '{name}' not intercepted by engine."
            )))
        })
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("worktree isolation branch")
    }
}

/// ExitWorktree — leave the current worktree session.
pub struct ExitWorktreeTool;

impl Tool for ExitWorktreeTool {
    fn name(&self) -> &str {
        "ExitWorktree"
    }

    fn description(&self) -> &str {
        "Exit a worktree session created by EnterWorktree and return to the \
         original working directory. Choose to keep or discard changes."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "What to do: 'merge' (merge changes back), 'keep' (keep branch), or 'discard'",
                    "enum": ["merge", "keep", "discard"]
                },
                "discard_changes": {
                    "type": "boolean",
                    "description": "If true, discard all uncommitted changes before exiting"
                }
            },
            "additionalProperties": false
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        Box::pin(async move { Ok(ToolResult::error("ExitWorktree not intercepted by engine.")) })
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn search_hint(&self) -> Option<&str> {
        Some("worktree exit leave")
    }
}
