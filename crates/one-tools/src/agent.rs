//! Agent tool — spawns sub-agents with independent context windows.
//!
//! This tool's schema is registered normally, but execution is intercepted
//! by the QueryEngine which runs a separate conversation loop. The tool
//! itself is a no-op — its purpose is to provide the schema for the AI.

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::Value;

use crate::{Tool, ToolContext, ToolResult};

/// Agent tool — spawns a sub-agent to handle focused tasks.
/// Execution is intercepted by the QueryEngine (not by this tool's execute method).
pub struct AgentTool;

impl Tool for AgentTool {
    fn name(&self) -> &str {
        "Agent"
    }

    fn description(&self) -> &str {
        "Launch a sub-agent to handle a focused task. The agent gets its own context window \
         and tool set. Use this for tasks that benefit from independent exploration, \
         research, or parallel work."
    }

    fn input_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The task for the agent to perform. Be specific — the agent \
                                    starts with no context from this conversation."
                },
                "description": {
                    "type": "string",
                    "description": "A short (3-5 word) description of the task for status display."
                },
                "subagent_type": {
                    "type": "string",
                    "enum": ["general-purpose", "Explore", "Plan"],
                    "description": "The type of agent: 'general-purpose' (all tools), \
                                    'Explore' (read-only tools), or 'Plan' (analysis only). \
                                    Default: general-purpose."
                },
                "run_in_background": {
                    "type": "boolean",
                    "description": "Run the agent in the background. Returns immediately with \
                                    an agent ID. You'll be notified when it completes."
                },
                "isolation": {
                    "type": "string",
                    "enum": ["worktree"],
                    "description": "Isolation mode. 'worktree' creates a temporary git worktree \
                                    so the agent works on an isolated copy of the repo."
                },
                "model": {
                    "type": "string",
                    "enum": ["sonnet", "opus", "haiku"],
                    "description": "Model override for this agent. Use a cheaper model (haiku) \
                                    for simple lookups, or a stronger model (opus) for complex tasks. \
                                    Default: inherits parent's model."
                },
                "fork": {
                    "type": "boolean",
                    "description": "Fork mode: agent inherits the parent conversation as context. \
                                    Use when the agent needs full awareness of what happened so far. \
                                    Default: false (fresh context)."
                }
            },
            "required": ["prompt", "description"]
        })
    }

    fn execute(
        &self,
        _input: Value,
        _ctx: &ToolContext,
    ) -> Pin<Box<dyn Future<Output = Result<ToolResult>> + Send + '_>> {
        // This should never be called — the QueryEngine intercepts Agent tool calls.
        // If it IS called, it means the intercept didn't fire.
        Box::pin(async move {
            Ok(ToolResult::error(
                "Agent tool execution was not intercepted by the QueryEngine. \
                 This is a bug — Agent calls should be handled by the engine directly.",
            ))
        })
    }

    fn is_read_only(&self) -> bool {
        true // The Agent tool itself doesn't modify anything
    }

    fn search_hint(&self) -> Option<&str> {
        Some("spawn sub-agent parallel task independent context")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_tool_schema() {
        let tool = AgentTool;
        let schema = tool.input_schema();
        let props = &schema["properties"];

        assert!(props["prompt"].is_object());
        assert!(props["description"].is_object());
        assert!(props["subagent_type"].is_object());

        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("prompt")));
        assert!(required.contains(&serde_json::json!("description")));
    }

    #[test]
    fn test_tool_properties() {
        let tool = AgentTool;
        assert_eq!(tool.name(), "Agent");
        assert!(tool.is_read_only());
        assert!(!tool.should_defer()); // Always loaded — the model needs to see it
    }
}
